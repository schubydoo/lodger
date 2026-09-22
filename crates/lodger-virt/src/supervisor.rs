//! The supervisor keeps Lodger connected to libvirt and keeps the inventory
//! current.
//!
//! One background task runs this loop:
//!
//! 1. Open the connections and subscribe to the hub.
//! 2. Load the whole inventory. Events that arrive during the load wait in
//!    the subscription, so no change is lost.
//! 3. For each event, read the object again, update the inventory, and then
//!    pass the event on to [`Host::subscribe`] receivers.
//! 4. When the connection closes or a call fails, report `Disconnected`,
//!    wait 5 seconds, and start again at step 1.

use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use lodger_core::validate::check_text;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{broadcast, watch};

use crate::cache::{self, Inventory};
use crate::conn::{Error, Virt};
use crate::events::{Event, HUB_CAPACITY};

/// The wait between two connection attempts (TAD section 4.3).
const RETRY: Duration = Duration::from_secs(5);

/// The connection state, for the UI banner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnState {
    /// The first attempt, or a new attempt after a wait.
    Connecting,
    Connected,
    /// The last attempt failed or the connection closed. The inventory keeps
    /// its last content until the next connection loads it again.
    Disconnected {
        error: String,
    },
}

/// A handle to the supervisor. Dropping it stops the task and closes the
/// connections.
#[derive(Debug)]
pub struct Host {
    shared: Arc<Shared>,
    task: tokio::task::JoinHandle<()>,
}

#[derive(Debug)]
struct Shared {
    inventory: RwLock<Inventory>,
    virt: RwLock<Option<Virt>>,
    state: watch::Sender<ConnState>,
    events: broadcast::Sender<Event>,
}

impl Host {
    /// Starts the supervisor for `uri`, for example `qemu:///system`. It
    /// must run inside a Tokio runtime. A libvirt that cannot be reached is
    /// not an error here: the supervisor keeps trying, and [`Host::state`]
    /// shows why it cannot connect.
    pub fn start(uri: &str) -> Result<Self, Error> {
        Self::start_with(uri, RETRY)
    }

    fn start_with(uri: &str, retry: Duration) -> Result<Self, Error> {
        let uri = check_text("libvirt URI", uri)?.to_owned();
        let shared = Arc::new(Shared {
            inventory: RwLock::default(),
            virt: RwLock::default(),
            state: watch::Sender::new(ConnState::Connecting),
            events: broadcast::Sender::new(HUB_CAPACITY),
        });
        let task = tokio::spawn(supervise(Arc::clone(&shared), uri, retry));
        Ok(Self { shared, task })
    }

    /// The connection state now.
    pub fn state(&self) -> ConnState {
        self.shared.state.borrow().clone()
    }

    /// A receiver that sees every change of the connection state.
    pub fn watch_state(&self) -> watch::Receiver<ConnState> {
        self.shared.state.subscribe()
    }

    /// A copy of the inventory now.
    pub fn inventory(&self) -> Inventory {
        read(&self.shared.inventory).clone()
    }

    /// The open connections, or `None` while Lodger is not connected.
    pub fn virt(&self) -> Option<Virt> {
        read(&self.shared.virt).clone()
    }

    /// A receiver for libvirt events. Each event arrives after the
    /// inventory contains its change.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.shared.events.subscribe()
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn supervise(shared: Arc<Shared>, uri: String, retry: Duration) {
    loop {
        let error = match session(&shared, &uri).await {
            Ok(reason) => reason,
            Err(e) => e.to_string(),
        };
        *write(&shared.virt) = None;
        shared.state.send_replace(ConnState::Disconnected { error });
        tokio::time::sleep(retry).await;
        shared.state.send_replace(ConnState::Connecting);
    }
}

/// One connection, from open to close. Returns why it closed.
async fn session(shared: &Shared, uri: &str) -> Result<String, Error> {
    let virt = Virt::open(uri).await?;
    let mut rx = virt.subscribe();
    *write(&shared.inventory) = virt.read(Inventory::load).await?;
    *write(&shared.virt) = Some(virt.clone());
    shared.state.send_replace(ConnState::Connected);

    loop {
        match rx.recv().await {
            Ok(Event::Closed { reason }) => return Ok(close_reason(reason)),
            Ok(event) => {
                if let Some(refresh) = virt.read(move |c| cache::refresh(c, &event)).await? {
                    write(&shared.inventory).apply(refresh);
                }
                let _ = shared.events.send(event);
            }
            // Too many events to follow one by one: load everything again.
            Err(RecvError::Lagged(_)) => {
                *write(&shared.inventory) = virt.read(Inventory::load).await?;
            }
            // `virt` holds a sender, so the hub stays open while it lives.
            Err(RecvError::Closed) => return Ok("the event hub closed".into()),
        }
    }
}

/// Explains a `virConnectCloseReason` code.
fn close_reason(code: i32) -> String {
    let why = match code {
        0 => "an error",
        1 => "the end of the stream",
        2 => "a keepalive timeout",
        3 => "a request from the client",
        _ => "an unknown reason",
    };
    format!("libvirt closed the connection because of {why}")
}

/// The locks guard plain data, so a panic while one is held cannot leave
/// the data half changed. A poisoned lock is still safe to use.
fn read<T>(lock: &RwLock<T>) -> std::sync::RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

fn write<T>(lock: &RwLock<T>) -> std::sync::RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use lodger_core::model::VmState;
    use tokio::sync::watch;
    use virt::connect::Connect;

    use super::{ConnState, Host};
    use crate::cache::Inventory;
    use crate::events::Event;

    const TEST_URI: &str = "test:///default";
    const QUICK: Duration = Duration::from_millis(500);

    async fn wait_for(rx: &mut watch::Receiver<ConnState>, want: fn(&ConnState) -> bool) {
        tokio::time::timeout(Duration::from_secs(5), rx.wait_for(want))
            .await
            .expect("the state did not change within 5 seconds")
            .unwrap();
    }

    fn connected(s: &ConnState) -> bool {
        *s == ConnState::Connected
    }

    /// Polls the inventory until `check` passes or 5 seconds pass (PRD F3).
    async fn eventually(host: &Host, check: impl Fn(&Inventory) -> bool) {
        let found = tokio::time::timeout(Duration::from_secs(5), async {
            while !check(&host.inventory()) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        assert!(
            found.is_ok(),
            "the inventory did not change within 5 seconds"
        );
    }

    fn vm_named<'a>(inv: &'a Inventory, name: &str) -> Option<&'a lodger_core::model::Vm> {
        inv.vms.values().find(|vm| vm.name == name)
    }

    fn domain_xml(name: &str) -> String {
        format!(
            "<domain type='test'><name>{name}</name><memory>65536</memory>\
             <os><type>hvm</type></os></domain>"
        )
    }

    #[tokio::test]
    async fn a_change_from_another_connection_reaches_the_inventory() {
        let host = Host::start_with(TEST_URI, QUICK).unwrap();
        wait_for(&mut host.watch_state(), connected).await;
        let name = "lodger-spike-cache-change";

        let outside = Connect::open(Some(TEST_URI)).unwrap();
        let domain = outside.define_domain_xml(&domain_xml(name)).unwrap();
        eventually(&host, |inv| vm_named(inv, name).is_some()).await;
        domain.create().unwrap();
        eventually(&host, |inv| {
            vm_named(inv, name).is_some_and(|vm| vm.state == VmState::Running)
        })
        .await;
        domain.destroy().unwrap();
        domain.undefine().unwrap();
        eventually(&host, |inv| vm_named(inv, name).is_none()).await;
    }

    #[tokio::test]
    async fn events_reach_subscribers_after_the_inventory() {
        let host = Host::start_with(TEST_URI, QUICK).unwrap();
        wait_for(&mut host.watch_state(), connected).await;
        let mut rx = host.subscribe();
        let name = "lodger-spike-cache-order";

        let outside = Connect::open(Some(TEST_URI)).unwrap();
        let domain = outside.define_domain_xml(&domain_xml(name)).unwrap();
        let id = domain.uuid().unwrap();
        loop {
            let event = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .unwrap()
                .unwrap();
            if matches!(event, Event::Domain { id: got, .. } if got == id) {
                break;
            }
        }
        assert!(host.inventory().vms.contains_key(&id));
        domain.undefine().unwrap();
    }

    /// A test driver loaded from a file starts from that file on each new
    /// connection. Changing the file while Lodger is disconnected is thus a
    /// change that no event reports: only the rebuild can find it.
    #[tokio::test]
    async fn after_a_disconnect_the_rebuilt_inventory_matches_libvirt() {
        let file = temp_file("reconnect");
        std::fs::write(&file, node_xml("lodger-spike-before")).unwrap();
        let uri = format!("test://{}", file.display());
        let host = Host::start_with(&uri, QUICK).unwrap();
        let mut state = host.watch_state();
        wait_for(&mut state, connected).await;
        assert!(vm_named(&host.inventory(), "lodger-spike-before").is_some());

        host.virt().unwrap().inject(Event::Closed { reason: 2 });
        wait_for(&mut state, |s| matches!(s, ConnState::Disconnected { .. })).await;
        assert!(host.virt().is_none());
        assert_eq!(
            host.state(),
            ConnState::Disconnected {
                error: "libvirt closed the connection because of a keepalive timeout".into()
            }
        );
        std::fs::write(&file, node_xml("lodger-spike-after")).unwrap();

        wait_for(&mut state, connected).await;
        let fresh = Connect::open(Some(&uri)).unwrap();
        assert_eq!(host.inventory(), Inventory::load(&fresh).unwrap());
        assert!(vm_named(&host.inventory(), "lodger-spike-after").is_some());
        assert!(vm_named(&host.inventory(), "lodger-spike-before").is_none());
        std::fs::remove_file(file).unwrap();
    }

    #[tokio::test]
    async fn an_unreachable_libvirt_shows_as_disconnected_and_retries() {
        let file = temp_file("missing");
        let uri = format!("test://{}", file.display());
        let host = Host::start_with(&uri, QUICK).unwrap();
        let mut state = host.watch_state();
        wait_for(&mut state, |s| matches!(s, ConnState::Disconnected { .. })).await;
        let ConnState::Disconnected { error } = host.state() else {
            panic!("expected Disconnected, got {:?}", host.state());
        };
        assert!(error.starts_with("libvirt: "), "{error}");

        // The file appears, and the next attempt connects.
        std::fs::write(&file, node_xml("lodger-spike-late")).unwrap();
        wait_for(&mut state, connected).await;
        assert!(vm_named(&host.inventory(), "lodger-spike-late").is_some());
        std::fs::remove_file(file).unwrap();
    }

    #[tokio::test]
    async fn a_uri_with_a_nul_byte_is_rejected() {
        assert!(Host::start("test:///default\0").is_err());
    }

    fn temp_file(test: &str) -> PathBuf {
        std::env::temp_dir().join(format!("lodger-{test}-{}.xml", std::process::id()))
    }

    /// A driver file with one domain. The fixed UUID keeps it the same on
    /// every load, so two connections can compare their inventories.
    fn node_xml(domain: &str) -> String {
        format!(
            "<node><domain type='test'><name>{domain}</name>\
             <uuid>6f1c8a52-5a4b-4c6e-9d2f-0a1b2c3d4e5f</uuid>\
             <memory>65536</memory><os><type>hvm</type></os></domain></node>"
        )
    }
}
