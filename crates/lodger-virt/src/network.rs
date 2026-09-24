//! Virtual network operations (PRD F8): create, start, stop, autostart, and
//! delete. The XML comes from `lodger_core::xml::network`.
//!
//! A new network is defined and started in one call, with autostart on by
//! default. If the start fails, the network is undefined again, so no
//! definition stays behind. Lodger never creates or changes a host network
//! interface (PRD W6): it only reads the list of host bridges.

use std::collections::BTreeSet;
use std::path::Path;

use lodger_core::xml::domain_networks;
use uuid::Uuid;
use virt::connect::Connect;
use virt::error::ErrorNumber;
use virt::network::Network;
use virt::sys::VIR_DOMAIN_XML_INACTIVE;

use crate::conn::{Error, Virt};
use crate::events::Event;

/// Lodger's own network event code, sent after an autostart change: libvirt
/// has no event for it. Every network event only makes the inventory read
/// the network again, so the code is never compared.
pub(crate) const NETWORK_AUTOSTART: i32 = -1;

/// Where Linux lists the network interfaces. A bridge has a `bridge` folder.
const SYS_CLASS_NET: &str = "/sys/class/net";

impl Virt {
    /// The XML of every network, with its name, for the checks of a new
    /// network. A network that disappears during the list is left out.
    pub async fn network_xmls(&self) -> Result<Vec<(String, String)>, Error> {
        self.read(|c| xmls_of(c.list_all_networks(0)?)).await
    }

    /// The XML of network `id`.
    pub async fn network_xml(&self, id: Uuid) -> Result<String, Error> {
        self.read(move |c| c.lookup_network_by_uuid(id)?.xml_desc(0))
            .await
    }

    /// Defines and starts a network from `xml`, and returns its UUID. On any
    /// failure, the network is gone again and the error is libvirt's.
    pub async fn create_network(&self, xml: String, autostart: bool) -> Result<Uuid, Error> {
        self.job(move |c| create_network_on(c, &xml, autostart, |_| {}))
            .await
    }

    /// Starts or stops network `id`.
    pub async fn set_network_active(&self, id: Uuid, active: bool) -> Result<(), Error> {
        self.job(move |c| {
            let network = c.lookup_network_by_uuid(id)?;
            match (active, network.is_active()?) {
                (true, true) => Ok(Err(Error::WrongState("the network is running already"))),
                (false, false) => Ok(Err(Error::WrongState("the network is not running"))),
                (true, false) => Ok(network.create().map_err(Error::from)),
                (false, true) => Ok(network.destroy().map_err(Error::from)),
            }
        })
        .await?
    }

    /// Switches autostart of network `id`, and sends Lodger's own event,
    /// because libvirt sends none.
    pub async fn set_network_autostart(&self, id: Uuid, on: bool) -> Result<(), Error> {
        self.job(move |c| c.lookup_network_by_uuid(id)?.set_autostart(on))
            .await?;
        let _ = self.hub.send(Event::Network {
            id,
            event: NETWORK_AUTOSTART,
        });
        Ok(())
    }

    /// The names of the VMs with a NIC on network `id`, sorted. Both the
    /// saved and the live configuration count.
    pub async fn network_users(&self, id: Uuid) -> Result<Vec<String>, Error> {
        self.read(move |c| {
            let name = c.lookup_network_by_uuid(id)?.name()?;
            Ok(users_of(c, &name))
        })
        .await?
    }

    /// Stops network `id` if it runs, and undefines it.
    pub async fn delete_network(&self, id: Uuid) -> Result<(), Error> {
        self.job(move |c| {
            let network = c.lookup_network_by_uuid(id)?;
            if network.is_active()? {
                network.destroy()?;
            }
            network.undefine()
        })
        .await
    }
}

/// The names of the bridges on the host, sorted. It only reads the list.
pub fn host_bridges() -> Vec<String> {
    bridges_in(Path::new(SYS_CLASS_NET))
}

fn bridges_in(root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().join("bridge").is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    names
}

/// The name and XML of each network. A network that another client removed
/// after the list is left out.
fn xmls_of(networks: Vec<Network>) -> Result<Vec<(String, String)>, virt::error::Error> {
    let mut out = Vec::new();
    for network in networks {
        match network
            .name()
            .and_then(|name| Ok((name, network.xml_desc(0)?)))
        {
            Ok(found) => out.push(found),
            Err(e) if e.code().known() == Some(ErrorNumber::NoNetwork) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(out)
}

/// The create itself. `before_start` runs after the define and the
/// autostart; the tests use it to make the start fail.
fn create_network_on(
    c: &Connect,
    xml: &str,
    autostart: bool,
    before_start: impl FnOnce(&Network),
) -> Result<Uuid, virt::error::Error> {
    let network = c.define_network_xml(xml)?;
    // Autostart before the start: the start event makes the inventory read
    // the network, and libvirt sends no event for autostart itself.
    let started = network.set_autostart(autostart).and_then(|()| {
        before_start(&network);
        network.create()
    });
    match started {
        Ok(()) => network.uuid(),
        Err(e) => {
            // Best effort: the libvirt error that the user sees is the
            // first one.
            if network.is_active().unwrap_or(false) {
                let _ = network.destroy();
            }
            let _ = network.undefine();
            Err(e)
        }
    }
}

/// The VMs with a NIC on network `name`. A VM that disappears during the
/// scan, or whose XML Lodger cannot read, is left out.
fn users_of(c: &Connect, name: &str) -> Result<Vec<String>, Error> {
    let mut users = BTreeSet::new();
    for domain in c.list_all_domains(0)? {
        let Ok(vm) = domain.name() else { continue };
        let mut xmls = Vec::new();
        if let Ok(saved) = domain.xml_desc(VIR_DOMAIN_XML_INACTIVE) {
            xmls.push(saved);
        }
        if domain.is_active().unwrap_or(false)
            && let Ok(live) = domain.xml_desc(0)
        {
            xmls.push(live);
        }
        if xmls.iter().any(|xml| {
            domain_networks(xml)
                .unwrap_or_default()
                .iter()
                .any(|n| n == name)
        }) {
            users.insert(vm);
        }
    }
    Ok(users.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use lodger_core::validate::Name;
    use lodger_core::xml::network::NewNetwork;
    use uuid::Uuid;

    use super::NETWORK_AUTOSTART;
    use crate::{Event, Virt};

    const TEST_URI: &str = "test:///default";

    /// A NAT network on its own subnet. Each test uses its own name and
    /// subnet, because the test driver shares its state.
    fn nat(name: &str, subnet: &str) -> String {
        NewNetwork::nat(Name::parse("Network name", name).unwrap(), subnet)
            .unwrap()
            .to_xml()
    }

    async fn exists(virt: &Virt, name: &'static str) -> bool {
        virt.read(move |c| Ok(c.lookup_network_by_name(name).is_ok()))
            .await
            .unwrap()
    }

    async fn state(virt: &Virt, id: Uuid) -> (bool, bool) {
        virt.read(move |c| {
            let n = c.lookup_network_by_uuid(id)?;
            Ok((n.is_active()?, n.autostart()?))
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn a_new_network_runs_with_autostart_and_starts_and_stops() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let id = virt
            .create_network(nat("net-life", "10.201.0.0/24"), true)
            .await
            .unwrap();
        assert_eq!(state(&virt, id).await, (true, true));
        let err = virt.set_network_active(id, true).await.unwrap_err();
        assert!(err.is_invalid_operation(), "{err}");
        virt.set_network_active(id, false).await.unwrap();
        assert_eq!(state(&virt, id).await, (false, true));
        let err = virt.set_network_active(id, false).await.unwrap_err();
        assert!(err.is_invalid_operation(), "{err}");
        virt.set_network_active(id, true).await.unwrap();
        virt.delete_network(id).await.unwrap();
        assert!(!exists(&virt, "net-life").await);
    }

    #[tokio::test]
    async fn autostart_can_be_off_and_changes_with_an_event() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let id = virt
            .create_network(nat("net-auto", "10.202.0.0/24"), false)
            .await
            .unwrap();
        assert_eq!(state(&virt, id).await, (true, false));
        let mut events = virt.subscribe();
        virt.set_network_autostart(id, true).await.unwrap();
        assert_eq!(state(&virt, id).await, (true, true));
        let want = Event::Network {
            id,
            event: NETWORK_AUTOSTART,
        };
        tokio::time::timeout(Duration::from_secs(5), async {
            while events.recv().await.unwrap() != want {}
        })
        .await
        .expect("no autostart event within 5 seconds");
        virt.delete_network(id).await.unwrap();
    }

    #[tokio::test]
    async fn a_failed_start_leaves_no_definition_behind() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let xml = nat("net-fail", "10.203.0.0/24");
        let err = virt
            .job(move |c| {
                // Another client starts the network first, so Lodger's start
                // fails.
                let start = |n: &virt::network::Network| n.create().unwrap();
                Ok(super::create_network_on(c, &xml, true, start))
            })
            .await
            .unwrap()
            .unwrap_err();
        assert!(!err.message().is_empty());
        assert!(!exists(&virt, "net-fail").await);
    }

    #[tokio::test]
    async fn autostart_is_set_before_the_start_event() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let xml = nat("net-order", "10.207.0.0/24");
        let (id, seen) = virt
            .job(move |c| {
                let mut seen = None;
                let look = |n: &virt::network::Network| seen = Some(n.autostart().unwrap());
                let id = super::create_network_on(c, &xml, true, look)?;
                Ok((id, seen))
            })
            .await
            .unwrap();
        assert_eq!(
            seen,
            Some(true),
            "autostart was off when the network started"
        );
        virt.delete_network(id).await.unwrap();
    }

    #[tokio::test]
    async fn the_users_of_a_network_are_the_vms_with_a_nic_on_it() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let id = virt
            .create_network(nat("net-users", "10.204.0.0/24"), true)
            .await
            .unwrap();
        let domain = |name: &str, network: &str| {
            format!(
                "<domain type='test'><name>{name}</name><memory>1024</memory>\
                 <os><type>hvm</type></os><devices><interface type='network'>\
                 <source network='{network}'/></interface></devices></domain>"
            )
        };
        let xmls = [
            domain("net-users-b", "net-users"),
            domain("net-users-a", "net-users"),
            domain("net-users-c", "net-users-other"),
        ];
        virt.read(move |c| {
            for xml in &xmls {
                c.define_domain_xml(xml)?;
            }
            Ok(())
        })
        .await
        .unwrap();
        assert_eq!(
            virt.network_users(id).await.unwrap(),
            ["net-users-a", "net-users-b"]
        );
        virt.delete_network(id).await.unwrap();
    }

    #[tokio::test]
    async fn a_network_that_disappears_during_the_list_is_left_out() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let gone = virt
            .create_network(nat("net-gone", "10.205.0.0/24"), true)
            .await
            .unwrap();
        let kept = virt
            .create_network(nat("net-kept", "10.206.0.0/24"), true)
            .await
            .unwrap();
        let names = virt
            .job(move |c| {
                let networks = c.list_all_networks(0)?;
                // Another client deletes one network after the list.
                let n = c.lookup_network_by_uuid(gone)?;
                n.destroy()?;
                n.undefine()?;
                let xmls = super::xmls_of(networks)?;
                Ok(xmls.into_iter().map(|(n, _)| n).collect::<Vec<_>>())
            })
            .await
            .unwrap();
        assert!(names.contains(&"net-kept".to_owned()), "{names:?}");
        assert!(!names.contains(&"net-gone".to_owned()), "{names:?}");
        let xml = virt.network_xml(kept).await.unwrap();
        assert!(xml.contains("10.206.0.1"), "{xml}");
        virt.delete_network(kept).await.unwrap();
    }

    #[test]
    fn host_bridges_are_the_interfaces_with_a_bridge_folder() {
        let dir = tempfile::tempdir().unwrap();
        for (name, bridge) in [
            ("br0", true),
            ("eth0", false),
            ("virbr0", true),
            ("lo", false),
        ] {
            let path = dir.path().join(name);
            std::fs::create_dir(&path).unwrap();
            if bridge {
                std::fs::create_dir(path.join("bridge")).unwrap();
            }
        }
        assert_eq!(super::bridges_in(dir.path()), ["br0", "virbr0"]);
        assert!(super::bridges_in(&dir.path().join("missing")).is_empty());
    }
}
