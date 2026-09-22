//! libvirt events and the hub that fans them out.
//!
//! The read connection registers callbacks for domain, network, and pool
//! changes, and for the close of the connection. Each callback copies the
//! object's UUID and the event codes into an [`Event`] and sends it to the
//! hub, a Tokio broadcast channel. The supervisor subscribes to the hub,
//! updates the inventory, and passes each event on through `Host::subscribe`.
//!
//! The registration code lives in [`ffi`], the only module that calls
//! `virt::sys`.

mod ffi;

pub(crate) use ffi::{Registration, register};
use uuid::Uuid;

/// How many events the hub keeps for a slow subscriber. A subscriber that
/// falls further behind gets `RecvError::Lagged` and must reload its state.
pub(crate) const HUB_CAPACITY: usize = 1024;

/// One change that libvirt reported. The codes are libvirt's own numbers,
/// for example `VIR_DOMAIN_EVENT_STARTED` for a domain lifecycle event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Domain {
        id: Uuid,
        change: DomainChange,
    },
    /// A `virNetworkEventLifecycleType` code.
    Network {
        id: Uuid,
        event: i32,
    },
    /// A `virStoragePoolEventLifecycleType` code.
    Pool {
        id: Uuid,
        event: i32,
    },
    /// The read connection closed. The `virConnectCloseReason` code tells
    /// why.
    Closed {
        reason: i32,
    },
}

/// What changed on a domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainChange {
    /// A `virDomainEventType` code and its detail code.
    Lifecycle {
        event: i32,
        detail: i32,
    },
    Reboot,
    DeviceAdded,
    DeviceRemoved,
    Metadata,
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use tokio::sync::broadcast::Receiver;
    use tokio::sync::broadcast::error::RecvError;
    use uuid::Uuid;
    use virt::connect::Connect;
    use virt::domain::Domain;

    use super::{DomainChange, Event};
    use crate::Virt;

    const TEST_URI: &str = "test:///default";
    // From libvirt's virDomainEventType.
    const STARTED: i32 = 2;
    const STOPPED: i32 = 5;

    /// Defines a domain on its own test-driver connection, the way `virsh`
    /// would. Each test uses its own name, so parallel tests do not clash.
    fn define(name: &str) -> (Connect, Domain) {
        let outside = Connect::open(Some(TEST_URI)).unwrap();
        let xml = format!(
            "<domain type='test'><name>{name}</name><memory>65536</memory>\
             <os><type>hvm</type></os></domain>"
        );
        let domain = outside.define_domain_xml(&xml).unwrap();
        (outside, domain)
    }

    fn uuid_of(domain: &Domain) -> Uuid {
        Uuid::parse_str(&domain.uuid_string().unwrap()).unwrap()
    }

    /// Waits for the next lifecycle event of domain `id`, and skips all
    /// other events.
    async fn next_lifecycle(rx: &mut Receiver<Event>, id: Uuid) -> i32 {
        loop {
            let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .expect("no event within 5 seconds")
                .unwrap();
            if let Event::Domain {
                id: got,
                change: DomainChange::Lifecycle { event, .. },
            } = event
                && got == id
            {
                return event;
            }
        }
    }

    #[tokio::test]
    async fn a_domain_started_elsewhere_reaches_the_hub() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let mut rx = virt.subscribe();
        let (_outside, domain) = define("lodger-spike-events-start");
        let id = uuid_of(&domain);

        domain.create().unwrap();
        // Defining the domain sent an event first. Skip to STARTED.
        while next_lifecycle(&mut rx, id).await != STARTED {}
        domain.destroy().unwrap();
        assert_eq!(next_lifecycle(&mut rx, id).await, STOPPED);
        domain.undefine().unwrap();
    }

    #[tokio::test]
    async fn every_handle_gets_the_events() {
        let first = Virt::open(TEST_URI).await.unwrap();
        let second = Virt::open(TEST_URI).await.unwrap();
        let mut rx1 = first.subscribe();
        let mut rx2 = second.subscribe();
        let (_outside, domain) = define("lodger-spike-events-fanout");
        let id = uuid_of(&domain);

        domain.create().unwrap();
        while next_lifecycle(&mut rx1, id).await != STARTED {}
        while next_lifecycle(&mut rx2, id).await != STARTED {}
        domain.destroy().unwrap();
        domain.undefine().unwrap();
    }

    #[tokio::test]
    async fn dropping_the_handle_frees_every_registration() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let mut rx = virt.subscribe();
        drop(virt);
        // Each registration holds a copy of the hub's sender, and only
        // libvirt's free callback drops it. The channel closes once libvirt
        // freed every copy.
        let closed = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Err(RecvError::Closed) = rx.recv().await {
                    break;
                }
            }
        })
        .await;
        assert!(closed.is_ok(), "libvirt did not free every registration");
    }

    /// The stress test. It also runs under `AddressSanitizer` in the nightly
    /// workflow (TAD section 9.3), and by hand with:
    ///
    /// ```text
    /// RUSTFLAGS=-Zsanitizer=address \
    /// LSAN_OPTIONS=suppressions=$PWD/.github/lsan-suppressions.txt \
    ///   cargo +nightly test -Zbuild-std \
    ///   --target x86_64-unknown-linux-gnu -p lodger-virt -- --exact \
    ///   events::tests::stress_ten_thousand_events
    /// ```
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stress_ten_thousand_events() {
        const EVENTS: usize = 10_000;
        // A test driver loaded from a file gets state of its own for each
        // connection. The flood of events then reaches only this hub, and
        // not the hubs of the tests that share `test:///default`.
        let file = std::env::temp_dir().join(format!("lodger-stress-{}.xml", std::process::id()));
        std::fs::write(&file, "<node/>").unwrap();
        let virt = Virt::open(&format!("test://{}", file.display()))
            .await
            .unwrap();
        let mut rx = virt.subscribe();

        // Each cycle sends STARTED and STOPPED. Other handles open and drop
        // on the way, so registration and the free callback run many times.
        let driver = {
            let virt = virt.clone();
            tokio::spawn(async move {
                virt.read(|c| {
                    let xml = "<domain type='test'><name>lodger-spike-events-stress</name>\
                               <memory>65536</memory><os><type>hvm</type></os></domain>";
                    let domain = c.define_domain_xml(xml)?;
                    for cycle in 0..EVENTS / 2 {
                        domain.create()?;
                        domain.destroy()?;
                        if cycle % 50 == 0 {
                            let rt = tokio::runtime::Handle::current();
                            drop(rt.block_on(Virt::open(TEST_URI)).unwrap());
                        }
                    }
                    domain.undefine()
                })
                .await
            })
        };

        let mut seen = 0;
        while seen < EVENTS {
            match tokio::time::timeout(Duration::from_secs(30), rx.recv())
                .await
                .expect("the events stopped")
            {
                Ok(Event::Domain {
                    change: DomainChange::Lifecycle { .. },
                    ..
                }) => seen += 1,
                Ok(_) => {}
                Err(RecvError::Lagged(n)) => seen += usize::try_from(n).unwrap(),
                Err(RecvError::Closed) => panic!("the hub closed"),
            }
        }
        driver.await.unwrap().unwrap();
        std::fs::remove_file(file).unwrap();
    }
}
