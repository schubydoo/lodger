//! Virtual network XML (PRD F8): build a new NAT, isolated, or bridge
//! network, and read any network that libvirt returns.
//!
//! [`NetworkXml`] keeps the whole document, so writing it back keeps every
//! element that Lodger does not model, such as DNS hosts and DHCP host entries.
//! libvirt accepts a network whose subnet overlaps another one and fails
//! only when both start, so Lodger checks every new subnet itself.

use std::net::Ipv4Addr;

use ipnet::Ipv4Net;
use xmltree::{Element, XMLNode};

use super::{XmlError, element_with, parse, text_element, write};
use crate::validate::{InputError, Name, check_no_overlap, parse_interface, parse_subnet};

/// How VMs on the network reach other hosts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkMode {
    /// A private subnet that reaches the LAN through the host's address.
    Nat { subnet: Ipv4Net },
    /// A private subnet that reaches only the host and the other VMs.
    Isolated { subnet: Ipv4Net },
    /// VMs join the LAN through an existing host bridge.
    Bridge { bridge: String },
}

/// A new network, with every input checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewNetwork {
    pub name: Name,
    pub mode: NetworkMode,
}

impl NewNetwork {
    /// A NAT network on `subnet`, such as `192.168.150.0/24`.
    pub fn nat(name: Name, subnet: &str) -> Result<Self, InputError> {
        let subnet = parse_subnet("Subnet", subnet)?;
        Ok(Self {
            name,
            mode: NetworkMode::Nat { subnet },
        })
    }

    /// An isolated network on `subnet`.
    pub fn isolated(name: Name, subnet: &str) -> Result<Self, InputError> {
        let subnet = parse_subnet("Subnet", subnet)?;
        Ok(Self {
            name,
            mode: NetworkMode::Isolated { subnet },
        })
    }

    /// A bridge network on the host bridge `bridge`, which must be one of
    /// `host_bridges`. Lodger never creates a host bridge, because a wrong
    /// one can cut a headless host off the network (PRD W6).
    pub fn bridge(name: Name, bridge: &str, host_bridges: &[String]) -> Result<Self, InputError> {
        let bridge = parse_interface("Host bridge", bridge)?;
        if !host_bridges.contains(&bridge) {
            return Err(InputError::NoSuchBridge { name: bridge });
        }
        Ok(Self {
            name,
            mode: NetworkMode::Bridge { bridge },
        })
    }

    /// The subnet of a NAT or isolated network.
    pub fn subnet(&self) -> Option<Ipv4Net> {
        match self.mode {
            NetworkMode::Nat { subnet } | NetworkMode::Isolated { subnet } => Some(subnet),
            NetworkMode::Bridge { .. } => None,
        }
    }

    /// The network XML for `virNetworkDefineXML`. A NAT or isolated network
    /// gets the first address of its subnet as the host's gateway, and DHCP
    /// hands out every other address. libvirt picks the bridge name.
    pub fn to_xml(&self) -> String {
        let mut network = Element::new("network");
        push(&mut network, text_element("name", self.name.as_str()));
        match &self.mode {
            NetworkMode::Nat { subnet } => {
                push(&mut network, element_with("forward", &[("mode", "nat")]));
                push(&mut network, ip(*subnet));
            }
            NetworkMode::Isolated { subnet } => push(&mut network, ip(*subnet)),
            NetworkMode::Bridge { bridge } => {
                push(&mut network, element_with("forward", &[("mode", "bridge")]));
                push(&mut network, element_with("bridge", &[("name", bridge)]));
            }
        }
        write(&network)
    }

    /// Rejects the network if its subnet overlaps a subnet of an existing
    /// network, and names that network. `existing` holds each network's name
    /// and XML.
    pub fn check_against<'a>(
        &self,
        existing: impl IntoIterator<Item = (&'a str, &'a NetworkXml)>,
    ) -> Result<(), InputError> {
        let Some(subnet) = self.subnet() else {
            return Ok(());
        };
        let others: Vec<(&str, Ipv4Net)> = existing
            .into_iter()
            .flat_map(|(name, xml)| xml.subnets().into_iter().map(move |net| (name, net)))
            .collect();
        check_no_overlap("Subnet", subnet, others)
    }
}

/// `<ip>` with the gateway and a DHCP range over every other host address.
fn ip(subnet: Ipv4Net) -> Element {
    let first = u32::from(subnet.network());
    let last = u32::from(subnet.broadcast());
    let gateway = Ipv4Addr::from(first + 1).to_string();
    let start = Ipv4Addr::from(first + 2).to_string();
    let end = Ipv4Addr::from(last - 1).to_string();
    let prefix = subnet.prefix_len().to_string();
    let mut ip = element_with("ip", &[("address", &gateway), ("prefix", &prefix)]);
    let mut dhcp = Element::new("dhcp");
    push(
        &mut dhcp,
        element_with("range", &[("start", &start), ("end", &end)]),
    );
    push(&mut ip, dhcp);
    ip
}

fn push(parent: &mut Element, child: Element) {
    parent.children.push(XMLNode::Element(child));
}

/// The prefix of an address class: A is /8, B is /16, and C and above /24.
fn classful_prefix(address: Ipv4Addr) -> u8 {
    match address.octets()[0] {
        0..=127 => 8,
        128..=191 => 16,
        _ => 24,
    }
}

/// A network document as libvirt returns it.
#[derive(Debug, Clone, PartialEq)]
pub struct NetworkXml(Element);

impl NetworkXml {
    pub fn parse(xml: &str) -> Result<Self, XmlError> {
        parse(xml, "network").map(Self)
    }

    pub fn name(&self) -> Option<&str> {
        self.0
            .get_child("name")?
            .children
            .iter()
            .find_map(|n| match n {
                XMLNode::Text(text) => Some(text.as_str()),
                _ => None,
            })
    }

    /// The forward mode, such as `nat` or `bridge`. `None` means isolated.
    pub fn forward_mode(&self) -> Option<&str> {
        let forward = self.0.get_child("forward")?;
        // libvirt's default forward mode is NAT.
        Some(forward.attributes.get("mode").map_or("nat", String::as_str))
    }

    /// The name of the network's bridge on the host.
    pub fn bridge(&self) -> Option<&str> {
        self.0
            .get_child("bridge")?
            .attributes
            .get("name")
            .map(String::as_str)
    }

    /// The IPv4 subnets of the network, from `prefix` or `netmask`. An IPv6
    /// address does not parse as IPv4, so it is left out, as is an
    /// unreadable one.
    pub fn subnets(&self) -> Vec<Ipv4Net> {
        self.0
            .children
            .iter()
            .filter_map(XMLNode::as_element)
            .filter(|e| e.name == "ip")
            .filter_map(|e| {
                let address: Ipv4Addr = e.attributes.get("address")?.parse().ok()?;
                let net = match (e.attributes.get("prefix"), e.attributes.get("netmask")) {
                    (Some(prefix), _) => Ipv4Net::new(address, prefix.parse().ok()?).ok()?,
                    (None, Some(mask)) => {
                        Ipv4Net::with_netmask(address, mask.parse().ok()?).ok()?
                    }
                    // Without either, libvirt uses the classful default.
                    (None, None) => Ipv4Net::new(address, classful_prefix(address)).ok()?,
                };
                Some(net.trunc())
            })
            .collect()
    }

    /// The document with every element that it came with.
    pub fn to_xml(&self) -> String {
        write(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A NAT network from libvirt 11.3.0 on Debian 13.
    const NAT: &str = include_str!("fixtures/network-nat.xml");
    /// An isolated network with DNS and a DHCP host, from libvirt 10.0.0.
    const ISOLATED: &str = include_str!("fixtures/network-isolated.xml");
    /// A bridge network from libvirt 10.0.0.
    const BRIDGE: &str = include_str!("fixtures/network-bridge.xml");

    fn name(value: &str) -> Name {
        Name::parse("Network name", value).unwrap()
    }

    fn net(value: &str) -> Ipv4Net {
        value.parse().unwrap()
    }

    #[test]
    fn a_nat_network_has_a_fixed_text() {
        let network = NewNetwork::nat(name("lab"), "192.168.150.0/24").unwrap();
        assert_eq!(
            network.to_xml(),
            "<network>\n  <name>lab</name>\n  <forward mode=\"nat\" />\n  \
             <ip address=\"192.168.150.1\" prefix=\"24\">\n    <dhcp>\n      \
             <range start=\"192.168.150.2\" end=\"192.168.150.254\" />\n    </dhcp>\n  \
             </ip>\n</network>"
        );
    }

    #[test]
    fn an_isolated_network_has_no_forward() {
        let network = NewNetwork::isolated(name("iso"), "10.77.0.0/30").unwrap();
        assert_eq!(
            network.to_xml(),
            "<network>\n  <name>iso</name>\n  <ip address=\"10.77.0.1\" prefix=\"30\">\n    \
             <dhcp>\n      <range start=\"10.77.0.2\" end=\"10.77.0.2\" />\n    </dhcp>\n  \
             </ip>\n</network>"
        );
    }

    #[test]
    fn a_bridge_network_names_the_host_bridge() {
        let bridges = vec!["br0".to_owned(), "virbr0".to_owned()];
        let network = NewNetwork::bridge(name("lan"), "br0", &bridges).unwrap();
        assert_eq!(
            network.to_xml(),
            "<network>\n  <name>lan</name>\n  <forward mode=\"bridge\" />\n  \
             <bridge name=\"br0\" />\n</network>"
        );
        assert_eq!(network.subnet(), None);
    }

    #[test]
    fn bridge_mode_without_the_host_bridge_explains_what_to_do() {
        for bridges in [vec![], vec!["br1".to_owned()]] {
            let err = NewNetwork::bridge(name("lan"), "br0", &bridges).unwrap_err();
            assert_eq!(err, InputError::NoSuchBridge { name: "br0".into() });
            let text = err.to_string();
            assert!(text.contains("A host bridge must exist first"), "{text}");
            assert!(
                text.contains("NetworkManager or systemd-networkd"),
                "{text}"
            );
        }
        for bad in ["", "br 0", "br0/x", "a-very-long-bridge", ".", ".."] {
            assert!(
                matches!(
                    NewNetwork::bridge(name("lan"), bad, &[bad.to_owned()]),
                    Err(InputError::InterfaceName { .. })
                ),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn the_text_is_the_same_on_every_run() {
        let network = NewNetwork::nat(name("lab"), "172.20.0.0/16").unwrap();
        let first = network.to_xml();
        for _ in 0..50 {
            assert_eq!(network.to_xml(), first);
        }
    }

    #[test]
    fn a_round_trip_keeps_unknown_elements() {
        for fixture in [NAT, ISOLATED, BRIDGE] {
            let original = Element::parse(fixture.as_bytes()).unwrap();
            let again = NetworkXml::parse(&NetworkXml::parse(fixture).unwrap().to_xml()).unwrap();
            assert_eq!(again.0, original, "{fixture}");
        }
        let text = NetworkXml::parse(ISOLATED).unwrap().to_xml();
        for kept in [
            "<domain name=\"lab.internal\" localOnly=\"yes\" />",
            "<hostname>db</hostname>",
            "<host mac=\"52:54:00:aa:bb:cc\" name=\"db\" ip=\"10.77.0.10\" />",
            "<bridge name=\"virbr1\" stp=\"on\" delay=\"0\" />",
        ] {
            assert!(text.contains(kept), "{kept} is missing from\n{text}");
        }
    }

    #[test]
    fn the_fixtures_read_as_libvirt_wrote_them() {
        let nat = NetworkXml::parse(NAT).unwrap();
        assert_eq!(
            (nat.name(), nat.forward_mode(), nat.bridge()),
            (Some("default"), Some("nat"), Some("virbr0"))
        );
        assert_eq!(nat.subnets(), [net("192.168.122.0/24")]);
        let iso = NetworkXml::parse(ISOLATED).unwrap();
        assert_eq!(
            (iso.forward_mode(), iso.subnets()),
            (None, vec![net("10.77.0.0/24")])
        );
        let bridge = NetworkXml::parse(BRIDGE).unwrap();
        assert_eq!(
            (bridge.forward_mode(), bridge.bridge(), bridge.subnets()),
            (Some("bridge"), Some("br0"), vec![])
        );
    }

    #[test]
    fn subnets_read_prefix_netmask_class_and_skip_ipv6() {
        let xml = r#"<network><name>n</name><forward/>
            <ip address="10.1.2.1" prefix="16"/>
            <ip address="172.16.5.1" netmask="255.255.0.0"/>
            <ip address="10.9.0.1"/>
            <ip address="172.31.0.1"/>
            <ip address="192.168.9.1"/>
            <ip family="ipv6" address="fd00::1" prefix="64"/>
            <ip address="not-an-address" prefix="24"/>
            </network>"#;
        let network = NetworkXml::parse(xml).unwrap();
        assert_eq!(network.forward_mode(), Some("nat"));
        assert_eq!(
            network.subnets(),
            [
                net("10.1.0.0/16"),
                net("172.16.0.0/16"),
                net("10.0.0.0/8"),
                net("172.31.0.0/16"),
                net("192.168.9.0/24"),
            ]
        );
    }

    #[test]
    fn an_overlapping_subnet_fails_and_names_the_other_network() {
        let existing = NetworkXml::parse(NAT).unwrap();
        for subnet in ["192.168.122.0/24", "192.168.122.128/25", "192.168.0.0/16"] {
            let network = NewNetwork::nat(name("new"), subnet).unwrap();
            let err = network.check_against([("default", &existing)]).unwrap_err();
            assert_eq!(
                err,
                InputError::SubnetOverlap {
                    field: "Subnet",
                    other_name: "default".into(),
                    other_subnet: net("192.168.122.0/24"),
                },
                "{subnet}"
            );
            assert!(err.to_string().contains("which network \"default\" uses"));
        }
        let isolated = NewNetwork::isolated(name("new"), "192.168.122.0/24").unwrap();
        assert!(isolated.check_against([("default", &existing)]).is_err());
        let apart = NewNetwork::nat(name("new"), "192.168.123.0/24").unwrap();
        assert_eq!(apart.check_against([("default", &existing)]), Ok(()));
        // A bridge network has no subnet of its own.
        let bridge = NewNetwork::bridge(name("lan"), "br0", &["br0".to_owned()]).unwrap();
        assert_eq!(bridge.check_against([("default", &existing)]), Ok(()));
    }

    #[test]
    fn every_subnet_of_an_existing_network_counts() {
        let two = NetworkXml::parse(
            r#"<network><name>two</name><ip address="10.5.0.1" prefix="24"/>
               <ip address="10.6.0.1" prefix="24"/></network>"#,
        )
        .unwrap();
        let network = NewNetwork::nat(name("new"), "10.6.0.0/24").unwrap();
        assert!(matches!(
            network.check_against([("two", &two)]),
            Err(InputError::SubnetOverlap { .. })
        ));
    }

    #[test]
    fn a_bad_subnet_is_rejected() {
        assert!(matches!(
            NewNetwork::nat(name("n"), "8.8.8.0/24"),
            Err(InputError::SubnetNotPrivate { .. })
        ));
        assert!(matches!(
            NewNetwork::isolated(name("n"), "10.0.0.0/31"),
            Err(InputError::SubnetTooSmall { .. })
        ));
    }
}
