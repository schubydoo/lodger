//! Live stats of the running domains (PRD F3, TAD 4.3 and 6.2): one
//! `virConnectGetAllDomainStats` call for every domain at once.
//!
//! The call uses the `NOWAIT` flag, so a domain that is busy with a long job
//! gives fewer values instead of blocking the call. That also makes it safe
//! on the read connection. The raw C calls are in `ffi.rs`.

mod ffi;

use lodger_core::model::Counters;
use uuid::Uuid;
use virt::sys;

use crate::conn::{Error, Virt};

/// The stats groups that Lodger shows: CPU time, memory, disks, and NICs.
const GROUPS: u32 = sys::VIR_DOMAIN_STATS_CPU_TOTAL
    | sys::VIR_DOMAIN_STATS_BALLOON
    | sys::VIR_DOMAIN_STATS_BLOCK
    | sys::VIR_DOMAIN_STATS_INTERFACE;

/// Only running domains, and nothing that would wait for a domain job.
const FLAGS: u32 =
    sys::VIR_CONNECT_GET_ALL_DOMAINS_STATS_ACTIVE | sys::VIR_CONNECT_GET_ALL_DOMAINS_STATS_NOWAIT;

impl Virt {
    /// The counters of every running domain.
    pub async fn domain_stats(&self) -> Result<Vec<(Uuid, Counters)>, Error> {
        self.read(|c| {
            let records = ffi::all_domain_stats(c, GROUPS, FLAGS)?;
            Ok(records
                .iter()
                .map(|r| {
                    let params = r.params.iter().map(|(name, value)| (name.as_str(), value));
                    (r.uuid, Counters::from_params(params))
                })
                .collect())
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use crate::Virt;

    const TEST_URI: &str = "test:///default";

    /// Defines a domain with a unique name, and starts it when `run` is set.
    async fn domain(virt: &Virt, name: &'static str, run: bool) -> Uuid {
        virt.read(move |c| {
            let xml = format!(
                "<domain type='test'><name>{name}</name><memory>1024</memory>\
                 <os><type>hvm</type></os></domain>"
            );
            let d = c.define_domain_xml(&xml)?;
            if run {
                d.create()?;
            }
            d.uuid()
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn only_running_domains_report_stats() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let running = domain(&virt, "stats-running", true).await;
        let stopped = domain(&virt, "stats-stopped", false).await;
        let stats = virt.domain_stats().await.unwrap();
        let ids: Vec<Uuid> = stats.iter().map(|(id, _)| *id).collect();
        assert!(ids.contains(&running), "{ids:?}");
        assert!(!ids.contains(&stopped), "{ids:?}");
    }

    /// Many calls, with domains that start and stop on the way, so the copy
    /// and the free run many times. The nightly job runs this test under
    /// the address sanitizer.
    #[tokio::test]
    async fn stress_two_thousand_stats_calls() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let id = domain(&virt, "stats-stress", false).await;
        for round in 0..2000 {
            if round % 100 == 0 {
                virt.read(move |c| {
                    let d = c.lookup_domain_by_uuid(id)?;
                    if d.is_active()? {
                        d.destroy()
                    } else {
                        d.create().map(drop)
                    }
                })
                .await
                .unwrap();
            }
            virt.domain_stats().await.unwrap();
        }
    }
}
