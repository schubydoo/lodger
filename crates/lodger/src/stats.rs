//! Live VM stats for the browsers that ask for them (PRD F3, TAD 4.3 and 6.2).
//!
//! A socket on `/ws/events` subscribes with `{"subscribe":"stats"}`. The
//! first subscriber starts one poll task. It calls `get_all_domain_stats` at
//! once and then every [`PERIOD`], and it publishes the rates to every
//! subscriber through a watch channel. Before each call, the task checks for
//! subscribers. When the last one is gone, it stops, so Lodger makes no stats
//! call while nobody watches.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use lodger_core::model::{Counters, VmStats};
use lodger_virt::Host;
use tokio::sync::watch;
use uuid::Uuid;

/// The time between 2 stats calls (TAD 6.2).
pub const PERIOD: Duration = Duration::from_secs(5);

/// The stats of every running VM from the latest call.
pub type Snapshot = Arc<Vec<VmStats>>;

/// The stats hub. Clone it freely; every clone shares one poll task.
#[derive(Debug, Clone)]
pub struct Stats {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    host: Arc<Host>,
    period: Duration,
    tx: watch::Sender<Snapshot>,
    /// Whether the poll task runs. [`Stats::subscribe`] and the task change
    /// it only while they hold the lock, so a new subscriber never meets a
    /// task that is about to stop.
    running: Mutex<bool>,
    /// The stats calls so far, for the tests.
    calls: AtomicU64,
}

impl Stats {
    pub fn new(host: Arc<Host>, period: Duration) -> Self {
        let (tx, _) = watch::channel(Snapshot::default());
        Self {
            inner: Arc::new(Inner {
                host,
                period,
                tx,
                running: Mutex::new(false),
                calls: AtomicU64::new(0),
            }),
        }
    }

    /// A receiver for the stats. It sees each new snapshot, but not the one
    /// from before it subscribed. Dropping it ends the subscription.
    pub fn subscribe(&self) -> watch::Receiver<Snapshot> {
        let mut running = self
            .inner
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let rx = self.inner.tx.subscribe();
        if !*running {
            *running = true;
            tokio::spawn(poll(Arc::clone(&self.inner)));
        }
        rx
    }

    /// How many stats calls the hub made.
    #[cfg(test)]
    pub fn calls(&self) -> u64 {
        self.inner.calls.load(Ordering::SeqCst)
    }
}

/// The poll task: one stats call per period while anyone subscribes.
async fn poll(inner: Arc<Inner>) {
    let mut ticks = tokio::time::interval(inner.period);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut before: HashMap<Uuid, (Instant, Counters)> = HashMap::new();
    loop {
        ticks.tick().await;
        {
            let mut running = inner.running.lock().unwrap_or_else(PoisonError::into_inner);
            if inner.tx.receiver_count() == 0 {
                *running = false;
                return;
            }
        }
        // While libvirt is not connected, skip the round. The banner says so.
        let Some(virt) = inner.host.virt() else {
            continue;
        };
        inner.calls.fetch_add(1, Ordering::SeqCst);
        let samples = match virt.domain_stats().await {
            Ok(samples) => samples,
            Err(e) => {
                eprintln!("lodger: stats: {e}");
                continue;
            }
        };
        let at = Instant::now();
        let vms = inner.host.inventory().vms;
        let mut stats = Vec::with_capacity(samples.len());
        let mut now = HashMap::with_capacity(samples.len());
        for (id, counters) in samples {
            let vcpus = vms.get(&id).map_or(0, |vm| vm.vcpus);
            let last = before.get(&id);
            let elapsed = last.map_or(Duration::ZERO, |(t, _)| at - *t);
            stats.push(VmStats::between(
                id,
                last.map(|(_, c)| c),
                &counters,
                elapsed,
                vcpus,
            ));
            now.insert(id, (at, counters));
        }
        // VMs that stopped drop out, so a restart starts without a rate.
        before = now;
        stats.sort_by_key(|s| s.uuid);
        inner.tx.send_replace(Arc::new(stats));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use lodger_virt::{Host, Virt};

    use super::Stats;

    const TEST_URI: &str = "test:///default";
    const FAST: Duration = Duration::from_millis(40);

    async fn connected_host() -> Arc<Host> {
        let host = Arc::new(Host::start(TEST_URI).unwrap());
        let mut state = host.watch_state();
        tokio::time::timeout(
            Duration::from_secs(5),
            state.wait_for(|s| *s == lodger_virt::ConnState::Connected),
        )
        .await
        .expect("connected within 5 seconds")
        .unwrap();
        host
    }

    #[tokio::test]
    async fn no_subscriber_means_no_stats_calls() {
        let stats = Stats::new(connected_host().await, FAST);
        tokio::time::sleep(FAST * 5).await;
        assert_eq!(stats.calls(), 0);

        let mut rx = stats.subscribe();
        tokio::time::timeout(Duration::from_secs(5), rx.changed())
            .await
            .expect("a snapshot within 5 seconds")
            .unwrap();
        assert!(stats.calls() >= 1);
        drop(rx);
        // The task sees no subscriber at its next tick and stops.
        tokio::time::sleep(FAST * 3).await;
        let after = stats.calls();
        tokio::time::sleep(FAST * 5).await;
        assert_eq!(stats.calls(), after, "calls went on without a subscriber");
    }

    #[tokio::test]
    async fn a_running_vm_gets_a_snapshot_on_every_period() {
        let host = connected_host().await;
        let outside = Virt::open(TEST_URI).await.unwrap();
        let id = outside
            .job(|c| {
                let d = c.define_domain_xml(
                    "<domain type='test'><name>stats-hub-running</name><memory>1024</memory>\
                     <os><type>hvm</type></os></domain>",
                )?;
                d.create()?;
                d.uuid()
            })
            .await
            .unwrap();
        let stats = Stats::new(host, FAST);
        let mut rx = stats.subscribe();
        for _ in 0..3 {
            tokio::time::timeout(Duration::from_secs(5), rx.changed())
                .await
                .expect("a snapshot within 5 seconds")
                .unwrap();
            let snapshot = rx.borrow_and_update().clone();
            assert!(snapshot.iter().any(|s| s.uuid == id), "{snapshot:?}");
        }
        outside
            .job(move |c| {
                let d = c.lookup_domain_by_uuid(id)?;
                d.destroy()?;
                d.undefine()
            })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_new_subscriber_restarts_a_stopped_task() {
        let stats = Stats::new(connected_host().await, FAST);
        drop(stats.subscribe());
        tokio::time::sleep(FAST * 3).await;
        let mut rx = stats.subscribe();
        tokio::time::timeout(Duration::from_secs(5), rx.changed())
            .await
            .expect("the task runs again")
            .unwrap();
    }
}
