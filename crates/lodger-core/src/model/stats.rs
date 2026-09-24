//! Live VM stats (PRD F3, TAD 4.3).
//!
//! libvirt reports counters that only grow, such as the CPU time in
//! nanoseconds or the bytes read from all disks. The UI shows rates.
//! [`Counters`] is one sample of one VM, read from the named values of
//! `virConnectGetAllDomainStats`. [`VmStats::between`] turns two samples into
//! rates.

use std::time::Duration;

use serde::Serialize;
use uuid::Uuid;

/// One typed value from libvirt's stats, copied out of C.
#[derive(Debug, Clone, PartialEq)]
pub enum StatValue {
    Int(i64),
    UInt(u64),
    Double(f64),
    Bool(bool),
    Text(String),
}

impl StatValue {
    /// The value as a counter. A negative or non-integer value is `None`.
    fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Int(v) => u64::try_from(*v).ok(),
            Self::UInt(v) => Some(*v),
            _ => None,
        }
    }
}

/// One sample of the counters that Lodger shows. A missing value is `None`:
/// with the `NOWAIT` flag, libvirt skips what it cannot read at once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counters {
    /// `cpu.time`: CPU time of all vCPUs, in nanoseconds.
    pub cpu_time_ns: Option<u64>,
    /// `balloon.current`: the memory that the guest has now, in KiB.
    pub memory_current_kib: Option<u64>,
    /// `balloon.available` and `balloon.unused`: the guest's own view. The
    /// guest reports them only with a balloon driver and a stats period.
    pub memory_available_kib: Option<u64>,
    pub memory_unused_kib: Option<u64>,
    /// The sums of `block.<n>.rd.bytes` and `block.<n>.wr.bytes`.
    pub disk_read_bytes: Option<u64>,
    pub disk_write_bytes: Option<u64>,
    /// The sums of `net.<n>.rx.bytes` and `net.<n>.tx.bytes`.
    pub net_rx_bytes: Option<u64>,
    pub net_tx_bytes: Option<u64>,
}

impl Counters {
    /// Reads the counters from libvirt's named values. Unknown names are
    /// ignored, so a newer libvirt with more values changes nothing.
    pub fn from_params<'a>(params: impl IntoIterator<Item = (&'a str, &'a StatValue)>) -> Self {
        let mut c = Self::default();
        for (name, value) in params {
            let Some(v) = value.as_u64() else { continue };
            let add = |sum: &mut Option<u64>| *sum = Some(sum.unwrap_or(0).saturating_add(v));
            match name {
                "cpu.time" => c.cpu_time_ns = Some(v),
                "balloon.current" => c.memory_current_kib = Some(v),
                "balloon.available" => c.memory_available_kib = Some(v),
                "balloon.unused" => c.memory_unused_kib = Some(v),
                _ => match indexed(name) {
                    Some(("block", "rd.bytes")) => add(&mut c.disk_read_bytes),
                    Some(("block", "wr.bytes")) => add(&mut c.disk_write_bytes),
                    Some(("net", "rx.bytes")) => add(&mut c.net_rx_bytes),
                    Some(("net", "tx.bytes")) => add(&mut c.net_tx_bytes),
                    _ => {}
                },
            }
        }
        c
    }
}

/// Splits `block.0.rd.bytes` into `("block", "rd.bytes")`. The middle part
/// must be a device number.
fn indexed(name: &str) -> Option<(&str, &str)> {
    let (group, rest) = name.split_once('.')?;
    let (index, field) = rest.split_once('.')?;
    index.parse::<u32>().ok()?;
    Some((group, field))
}

/// The stats of one VM as the UI gets them. Rates need 2 samples, so they
/// are `None` after the first one.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VmStats {
    pub uuid: Uuid,
    /// CPU use as a share of the VM's vCPUs: 100 means every vCPU is busy.
    pub cpu_percent: Option<f64>,
    /// The memory that the guest has now, in KiB.
    pub memory_kib: Option<u64>,
    /// The memory that the guest uses, in KiB, from the guest's own view.
    pub memory_used_kib: Option<u64>,
    /// Bytes per second.
    pub disk_read_bps: Option<u64>,
    pub disk_write_bps: Option<u64>,
    pub net_rx_bps: Option<u64>,
    pub net_tx_bps: Option<u64>,
}

impl VmStats {
    /// The stats from the sample `now` and, for the rates, the sample before
    /// it, taken `elapsed` earlier. A counter that went down (the VM
    /// restarted) gives no rate for this round.
    pub fn between(
        uuid: Uuid,
        before: Option<&Counters>,
        now: &Counters,
        elapsed: Duration,
        vcpus: u32,
    ) -> Self {
        let secs = elapsed.as_secs_f64();
        let delta = |pick: fn(&Counters) -> Option<u64>| {
            let before = pick(before?)?;
            let now = pick(now)?;
            let d = now.checked_sub(before)?;
            (secs > 0.0).then_some(d as f64 / secs)
        };
        let rate = |pick: fn(&Counters) -> Option<u64>| delta(pick).map(|r| r.round() as u64);
        let cpu_percent = delta(|c| c.cpu_time_ns)
            .filter(|_| vcpus > 0)
            .map(|ns_per_s| (ns_per_s / 1e9 / f64::from(vcpus) * 100.0).clamp(0.0, 100.0));
        let memory_used_kib = match (now.memory_available_kib, now.memory_unused_kib) {
            (Some(available), Some(unused)) => available.checked_sub(unused),
            _ => None,
        };
        Self {
            uuid,
            cpu_percent,
            memory_kib: now.memory_current_kib,
            memory_used_kib,
            disk_read_bps: rate(|c| c.disk_read_bytes),
            disk_write_bps: rate(|c| c.disk_write_bytes),
            net_rx_bps: rate(|c| c.net_rx_bytes),
            net_tx_bps: rate(|c| c.net_tx_bytes),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(list: &[(&'static str, StatValue)]) -> Counters {
        Counters::from_params(list.iter().map(|(n, v)| (*n, v)))
    }

    #[test]
    fn named_values_become_counters_and_devices_add_up() {
        let c = params(&[
            ("state.state", StatValue::Int(1)),
            ("cpu.time", StatValue::UInt(5_000_000_000)),
            ("balloon.current", StatValue::UInt(1_048_576)),
            ("balloon.available", StatValue::UInt(1_000_000)),
            ("balloon.unused", StatValue::UInt(600_000)),
            ("block.count", StatValue::UInt(2)),
            ("block.0.name", StatValue::Text("vda".into())),
            ("block.0.rd.bytes", StatValue::UInt(100)),
            ("block.1.rd.bytes", StatValue::UInt(50)),
            ("block.0.wr.bytes", StatValue::UInt(7)),
            ("net.0.rx.bytes", StatValue::UInt(1000)),
            ("net.1.rx.bytes", StatValue::UInt(24)),
            ("net.0.tx.bytes", StatValue::UInt(9)),
            ("net.count", StatValue::UInt(2)),
        ]);
        assert_eq!(
            c,
            Counters {
                cpu_time_ns: Some(5_000_000_000),
                memory_current_kib: Some(1_048_576),
                memory_available_kib: Some(1_000_000),
                memory_unused_kib: Some(600_000),
                disk_read_bytes: Some(150),
                disk_write_bytes: Some(7),
                net_rx_bytes: Some(1024),
                net_tx_bytes: Some(9),
            }
        );
    }

    #[test]
    fn counts_names_negatives_and_other_types_are_not_counters() {
        let c = params(&[
            ("block.count", StatValue::UInt(3)),
            ("block.x.rd.bytes", StatValue::UInt(3)),
            ("net.0.rx.bytes", StatValue::Int(-1)),
            ("cpu.time", StatValue::Double(1.5)),
            ("balloon.current", StatValue::Text("1".into())),
            ("block.0.rd.bytes", StatValue::Bool(true)),
        ]);
        assert_eq!(c, Counters::default());
    }

    #[test]
    fn two_samples_give_rates() {
        let id = Uuid::from_u128(1);
        let before = Counters {
            cpu_time_ns: Some(10_000_000_000),
            disk_read_bytes: Some(0),
            disk_write_bytes: Some(1_000),
            net_rx_bytes: Some(0),
            net_tx_bytes: Some(0),
            ..Counters::default()
        };
        let now = Counters {
            // 5 s later, 2 vCPUs: 5 s of CPU time is half of 10 vCPU-seconds.
            cpu_time_ns: Some(15_000_000_000),
            memory_current_kib: Some(2_097_152),
            memory_available_kib: Some(2_000_000),
            memory_unused_kib: Some(500_000),
            disk_read_bytes: Some(5_000_000),
            disk_write_bytes: Some(1_000),
            net_rx_bytes: Some(2_500),
            net_tx_bytes: Some(7),
        };
        let s = VmStats::between(id, Some(&before), &now, Duration::from_secs(5), 2);
        assert_eq!(
            s,
            VmStats {
                uuid: id,
                cpu_percent: Some(50.0),
                memory_kib: Some(2_097_152),
                memory_used_kib: Some(1_500_000),
                disk_read_bps: Some(1_000_000),
                disk_write_bps: Some(0),
                net_rx_bps: Some(500),
                net_tx_bps: Some(1),
            }
        );
    }

    #[test]
    fn the_first_sample_a_reset_and_no_time_give_no_rates() {
        let id = Uuid::from_u128(1);
        let now = Counters {
            cpu_time_ns: Some(1_000),
            disk_read_bytes: Some(10),
            memory_current_kib: Some(512),
            ..Counters::default()
        };
        let first = VmStats::between(id, None, &now, Duration::from_secs(5), 1);
        assert_eq!((first.cpu_percent, first.disk_read_bps), (None, None));
        assert_eq!(first.memory_kib, Some(512));
        // The VM restarted: its counters went down.
        let before = Counters {
            cpu_time_ns: Some(9_000),
            disk_read_bytes: Some(99),
            ..Counters::default()
        };
        let reset = VmStats::between(id, Some(&before), &now, Duration::from_secs(5), 1);
        assert_eq!((reset.cpu_percent, reset.disk_read_bps), (None, None));
        let instant = VmStats::between(id, Some(&now), &now, Duration::ZERO, 1);
        assert_eq!(instant.cpu_percent, None);
        let no_vcpus = VmStats::between(id, Some(&before), &before, Duration::from_secs(5), 0);
        assert_eq!(no_vcpus.cpu_percent, None);
    }

    #[test]
    fn cpu_use_stays_between_0_and_100() {
        let id = Uuid::from_u128(1);
        let before = Counters {
            cpu_time_ns: Some(0),
            ..Counters::default()
        };
        // 3 s of CPU time in 1 s on 1 vCPU: a timing effect, shown as 100.
        let now = Counters {
            cpu_time_ns: Some(3_000_000_000),
            ..Counters::default()
        };
        let s = VmStats::between(id, Some(&before), &now, Duration::from_secs(1), 1);
        assert_eq!(s.cpu_percent, Some(100.0));
    }
}
