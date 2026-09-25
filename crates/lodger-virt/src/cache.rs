//! The in-memory inventory: every VM, storage pool, and network that
//! libvirt knows. The API answers reads from it. libvirt stays the source of
//! truth, so the inventory is never written to disk (PRD 5.6).
//!
//! The functions here run inside one `Virt::read` closure, on a blocking
//! thread.

use std::collections::HashMap;

use lodger_core::model::{Network, Pool, PoolState, Vm, VmState};
use uuid::Uuid;
use virt::connect::Connect;
use virt::error::{Error, ErrorNumber};

use crate::events::Event;

/// One copy of libvirt's inventory, keyed by UUID.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Inventory {
    pub vms: HashMap<Uuid, Vm>,
    pub pools: HashMap<Uuid, Pool>,
    pub networks: HashMap<Uuid, Network>,
}

/// The new state of the one object that an event names. `None` means that
/// the object is gone.
#[derive(Debug)]
pub(crate) enum Refresh {
    Vm(Uuid, Option<Vm>),
    Pool(Uuid, Option<Pool>),
    Network(Uuid, Option<Network>),
}

impl Inventory {
    /// Reads the whole inventory. An object that disappears between the
    /// list and its details is left out.
    pub(crate) fn load(conn: &Connect) -> Result<Self, Error> {
        let mut inv = Self::default();
        for dom in conn.list_all_domains(0)? {
            if let Some(vm) = gone_is_none(vm_of(&dom))? {
                inv.vms.insert(vm.uuid, vm);
            }
        }
        for pool in conn.list_all_storage_pools(0)? {
            if let Some(pool) = gone_is_none(pool_of(&pool))? {
                inv.pools.insert(pool.uuid, pool);
            }
        }
        for net in conn.list_all_networks(0)? {
            if let Some(net) = gone_is_none(network_of(&net))? {
                inv.networks.insert(net.uuid, net);
            }
        }
        Ok(inv)
    }

    /// Stores the result of [`refresh`].
    pub(crate) fn apply(&mut self, refresh: Refresh) {
        match refresh {
            Refresh::Vm(id, Some(vm)) => drop(self.vms.insert(id, vm)),
            Refresh::Vm(id, None) => drop(self.vms.remove(&id)),
            Refresh::Pool(id, Some(pool)) => drop(self.pools.insert(id, pool)),
            Refresh::Pool(id, None) => drop(self.pools.remove(&id)),
            Refresh::Network(id, Some(net)) => drop(self.networks.insert(id, net)),
            Refresh::Network(id, None) => drop(self.networks.remove(&id)),
        }
    }
}

/// Reads the current state of the object that `event` names. The event
/// type does not matter: the read shows what is true now, so events that
/// arrive late or twice still leave the right state.
pub(crate) fn refresh(conn: &Connect, event: &Event) -> Result<Option<Refresh>, Error> {
    Ok(Some(match *event {
        Event::Domain { id, .. } => {
            let vm = conn.lookup_domain_by_uuid(id);
            Refresh::Vm(id, gone_is_none(vm.and_then(|d| vm_of(&d)))?)
        }
        Event::Pool { id, .. } => {
            let pool = conn.lookup_storage_pool_by_uuid(id);
            Refresh::Pool(id, gone_is_none(pool.and_then(|p| pool_of(&p)))?)
        }
        Event::Network { id, .. } => {
            let net = conn.lookup_network_by_uuid(id);
            Refresh::Network(id, gone_is_none(net.and_then(|n| network_of(&n)))?)
        }
        Event::Closed { .. } => return Ok(None),
    }))
}

/// Turns libvirt's "no such object" errors into `None`.
fn gone_is_none<T>(result: Result<T, Error>) -> Result<Option<T>, Error> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(e)
            if matches!(
                e.code().known(),
                Some(ErrorNumber::NoDomain | ErrorNumber::NoStoragePool | ErrorNumber::NoNetwork)
            ) =>
        {
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

fn vm_of(dom: &virt::domain::Domain) -> Result<Vm, Error> {
    let info = dom.info()?;
    // XML that Lodger cannot read counts as no display: the console page then
    // explains, instead of opening a socket that fails.
    let has_vnc = lodger_core::xml::domain_has_vnc(&dom.xml_desc(0)?).unwrap_or(false);
    Ok(Vm {
        uuid: dom.uuid()?,
        name: dom.name()?,
        state: VmState::from_code(info.state.to_raw()),
        vcpus: info.nr_virt_cpu,
        memory_kib: info.memory,
        persistent: dom.is_persistent()?,
        autostart: dom.autostart()?,
        has_vnc,
    })
}

fn pool_of(pool: &virt::storage_pool::StoragePool) -> Result<Pool, Error> {
    let info = pool.info()?;
    Ok(Pool {
        uuid: pool.uuid()?,
        name: pool.name()?,
        state: PoolState::from_code(info.state),
        capacity_bytes: info.capacity,
        allocation_bytes: info.allocation,
        available_bytes: info.available,
        persistent: pool.is_persistent()?,
        autostart: pool.autostart()?,
    })
}

fn network_of(net: &virt::network::Network) -> Result<Network, Error> {
    Ok(Network {
        uuid: net.uuid()?,
        name: net.name()?,
        active: net.is_active()?,
        persistent: net.is_persistent()?,
        autostart: net.autostart()?,
        // Some forward modes, such as macvtap, have no bridge.
        bridge: net.bridge_name().ok(),
    })
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;
    use virt::connect::Connect;

    use super::{Inventory, Refresh, refresh};
    use crate::events::{DomainChange, Event};

    fn conn() -> Connect {
        Connect::open(Some("test:///default")).unwrap()
    }

    #[test]
    fn the_load_finds_the_test_driver_objects() {
        let inv = Inventory::load(&conn()).unwrap();
        assert!(inv.vms.values().any(|vm| vm.name == "test"));
        assert!(inv.pools.values().any(|pool| pool.name == "default-pool"));
        assert!(inv.networks.values().any(|net| net.name == "default"));
    }

    #[test]
    fn a_vm_has_vnc_only_with_a_vnc_display() {
        let conn = conn();
        let define = |name: &str, devices: &str| {
            conn.define_domain_xml(&format!(
                "<domain type='test'><name>{name}</name><memory>1024</memory>\
                 <os><type>hvm</type></os><devices>{devices}</devices></domain>"
            ))
            .unwrap()
        };
        let with = define("cache-vnc-with", "<graphics type='vnc' port='-1'/>");
        let without = define("cache-vnc-without", "<serial type='pty'/>");
        let inv = Inventory::load(&conn).unwrap();
        let has_vnc = |name: &str| inv.vms.values().find(|vm| vm.name == name).unwrap().has_vnc;
        let (yes, no) = (has_vnc("cache-vnc-with"), has_vnc("cache-vnc-without"));
        with.undefine().unwrap();
        without.undefine().unwrap();
        assert!(yes);
        assert!(!no);
    }

    #[test]
    fn a_refresh_reads_the_object_again() {
        let conn = conn();
        let mut inv = Inventory::load(&conn).unwrap();
        let pool = conn.lookup_storage_pool_by_name("default-pool").unwrap();
        let net = conn.lookup_network_by_name("default").unwrap();
        let (pool_id, net_id) = (pool.uuid().unwrap(), net.uuid().unwrap());

        let before = inv.clone();
        let pool_event = Event::Pool {
            id: pool_id,
            event: 0,
        };
        let net_event = Event::Network {
            id: net_id,
            event: 0,
        };
        inv.apply(refresh(&conn, &pool_event).unwrap().unwrap());
        inv.apply(refresh(&conn, &net_event).unwrap().unwrap());
        assert_eq!(inv.pools[&pool_id], before.pools[&pool_id]);
        assert_eq!(inv.networks[&net_id], before.networks[&net_id]);
    }

    #[test]
    fn a_missing_object_is_removed() {
        let conn = conn();
        let gone = Uuid::from_u128(0xdead);
        let domain = Event::Domain {
            id: gone,
            change: DomainChange::Reboot,
        };
        let pool = Event::Pool { id: gone, event: 0 };
        let net = Event::Network { id: gone, event: 0 };
        assert!(matches!(
            refresh(&conn, &domain),
            Ok(Some(Refresh::Vm(_, None)))
        ));
        assert!(matches!(
            refresh(&conn, &pool),
            Ok(Some(Refresh::Pool(_, None)))
        ));
        assert!(matches!(
            refresh(&conn, &net),
            Ok(Some(Refresh::Network(_, None)))
        ));

        let mut inv = Inventory::load(&conn).unwrap();
        let some_vm = *inv.vms.keys().next().unwrap();
        inv.apply(Refresh::Vm(some_vm, None));
        assert!(!inv.vms.contains_key(&some_vm));
    }

    #[test]
    fn a_close_changes_nothing() {
        let closed = Event::Closed { reason: 1 };
        assert!(refresh(&conn(), &closed).unwrap().is_none());
    }
}
