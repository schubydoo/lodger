//! Reads and builds libvirt XML with xmltree, never with a string search
//! or by joining strings (TAD 5.2).
//!
//! Every parse rejects a `<!DOCTYPE`: libvirt never writes one, and a
//! document type is the way in for entity attacks. The `attribute-order`
//! feature keeps attributes in their order, so [`write()`] gives the same text
//! on every run, and tests can compare it.

pub mod network;
pub mod pool;

use xmltree::{Element, EmitterConfig, XMLNode};

/// Why a document cannot be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum XmlError {
    #[error("the XML has a DOCTYPE, which Lodger never accepts")]
    Doctype,
    #[error("the XML does not parse: {0}")]
    Parse(String),
    #[error("the XML root is <{found}>, not <{expected}>")]
    Root {
        found: String,
        expected: &'static str,
    },
}

/// Parses `xml` and checks that its root element is `root`.
pub fn parse(xml: &str, root: &'static str) -> Result<Element, XmlError> {
    if xml.contains("<!DOCTYPE") {
        return Err(XmlError::Doctype);
    }
    let element = Element::parse(xml.as_bytes()).map_err(|e| XmlError::Parse(e.to_string()))?;
    if element.name != root {
        return Err(XmlError::Root {
            found: element.name,
            expected: root,
        });
    }
    Ok(element)
}

/// Writes `element` as indented XML without a declaration, like libvirt.
pub fn write(element: &Element) -> String {
    let mut out = Vec::new();
    let config = EmitterConfig::new()
        .perform_indent(true)
        .indent_string("  ")
        .write_document_declaration(false);
    element
        .write_with_config(&mut out, config)
        .expect("writing to a Vec cannot fail");
    String::from_utf8(out).expect("xmltree writes UTF-8")
}

/// A new element with a text child, such as `<name>web</name>`.
pub(crate) fn text_element(name: &str, text: &str) -> Element {
    let mut element = Element::new(name);
    element.children.push(XMLNode::Text(text.to_owned()));
    element
}

/// A new element with attributes in the given order.
pub(crate) fn element_with(name: &str, attributes: &[(&str, &str)]) -> Element {
    let mut element = Element::new(name);
    for (key, value) in attributes {
        element
            .attributes
            .insert((*key).to_owned(), (*value).to_owned());
    }
    element
}

/// Where the data of a disk lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiskSource {
    /// `<disk type="file">`: a file on the host.
    File(String),
    /// `<disk type="block">`: a block device on the host.
    Block(String),
    /// `<disk type="volume">`: a volume in a libvirt storage pool.
    Volume { pool: String, volume: String },
}

/// One disk of a domain that holds data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disk {
    /// The device name in the guest, such as `vda`.
    pub target: Option<String>,
    pub source: DiskSource,
    /// True for a `<readonly/>` or `<shareable/>` disk, which other VMs may
    /// also use.
    pub shared: bool,
}

/// The data disks of a domain: `device="disk"` with a file, block, or
/// volume source. CD-ROMs, floppies, empty drives, and network disks are
/// left out, because Lodger never deletes them with a VM.
pub fn domain_disks(xml: &str) -> Result<Vec<Disk>, XmlError> {
    let domain = parse(xml, "domain")?;
    let Some(devices) = domain.get_child("devices") else {
        return Ok(Vec::new());
    };
    Ok(devices
        .children
        .iter()
        .filter_map(XMLNode::as_element)
        .filter(|e| e.name == "disk")
        // libvirt's default device is "disk".
        .filter(|d| attr(d, "device").is_none_or(|v| v == "disk"))
        .filter_map(disk)
        .collect())
}

fn disk(element: &Element) -> Option<Disk> {
    let source = element.get_child("source")?;
    // libvirt's default disk type is "file".
    let source = match attr(element, "type").unwrap_or("file") {
        "file" => DiskSource::File(attr(source, "file")?.to_owned()),
        "block" => DiskSource::Block(attr(source, "dev")?.to_owned()),
        "volume" => DiskSource::Volume {
            pool: attr(source, "pool")?.to_owned(),
            volume: attr(source, "volume")?.to_owned(),
        },
        _ => return None,
    };
    Some(Disk {
        target: element
            .get_child("target")
            .and_then(|t| attr(t, "dev"))
            .map(str::to_owned),
        source,
        shared: element.get_child("readonly").is_some() || element.get_child("shareable").is_some(),
    })
}

/// The names of the libvirt networks that the NICs of a domain use:
/// `<interface type="network">` with a `<source network="...">`. Each name
/// appears once, in the order of the first NIC that uses it.
pub fn domain_networks(xml: &str) -> Result<Vec<String>, XmlError> {
    let domain = parse(xml, "domain")?;
    let Some(devices) = domain.get_child("devices") else {
        return Ok(Vec::new());
    };
    let mut names: Vec<String> = Vec::new();
    for nic in devices
        .children
        .iter()
        .filter_map(XMLNode::as_element)
        .filter(|e| e.name == "interface" && attr(e, "type") == Some("network"))
    {
        if let Some(name) = nic.get_child("source").and_then(|s| attr(s, "network"))
            && !names.iter().any(|n| n == name)
        {
            names.push(name.to_owned());
        }
    }
    Ok(names)
}

fn attr<'a>(element: &'a Element, name: &str) -> Option<&'a str> {
    element.attributes.get(name).map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOMAIN: &str = r#"<domain type="kvm">
  <name>web</name>
  <devices>
    <disk type="file" device="disk">
      <driver name="qemu" type="qcow2"/>
      <source file="/var/lib/libvirt/images/web.qcow2"/>
      <target dev="vda" bus="virtio"/>
    </disk>
    <disk type="volume" device="disk">
      <source pool="default" volume="web-data.qcow2"/>
      <target dev="vdb" bus="virtio"/>
    </disk>
    <disk type="block">
      <source dev="/dev/vg0/web"/>
      <target dev="vdc" bus="virtio"/>
      <shareable/>
    </disk>
    <disk type="file" device="cdrom">
      <source file="/var/lib/libvirt/images/debian.iso"/>
      <target dev="sda" bus="sata"/>
      <readonly/>
    </disk>
    <disk type="file" device="cdrom">
      <target dev="sdb" bus="sata"/>
    </disk>
    <disk type="network" device="disk">
      <source protocol="rbd" name="pool/image"/>
      <target dev="vdd" bus="virtio"/>
    </disk>
    <disk device="disk">
      <source file="/srv/plain.img"/>
    </disk>
    <interface type="network"><source network="default"/></interface>
  </devices>
</domain>"#;

    #[test]
    fn data_disks_are_read_and_the_rest_left_out() {
        let disks = domain_disks(DOMAIN).unwrap();
        assert_eq!(
            disks,
            [
                Disk {
                    target: Some("vda".into()),
                    source: DiskSource::File("/var/lib/libvirt/images/web.qcow2".into()),
                    shared: false,
                },
                Disk {
                    target: Some("vdb".into()),
                    source: DiskSource::Volume {
                        pool: "default".into(),
                        volume: "web-data.qcow2".into(),
                    },
                    shared: false,
                },
                Disk {
                    target: Some("vdc".into()),
                    source: DiskSource::Block("/dev/vg0/web".into()),
                    shared: true,
                },
                Disk {
                    target: None,
                    source: DiskSource::File("/srv/plain.img".into()),
                    shared: false,
                },
            ]
        );
    }

    #[test]
    fn a_readonly_disk_is_shared() {
        let xml = r#"<domain><devices><disk type="file" device="disk">
            <source file="/base.img"/><readonly/></disk></devices></domain>"#;
        assert!(domain_disks(xml).unwrap()[0].shared);
    }

    #[test]
    fn the_networks_of_the_nics_are_read_once_each() {
        let xml = r#"<domain><devices>
            <interface type="network"><source network="lab"/><model type="virtio"/></interface>
            <interface type="bridge"><source bridge="br0"/></interface>
            <interface type="network"><source network="default"/></interface>
            <interface type="network"><source network="lab"/></interface>
            <interface type="network"/>
            </devices></domain>"#;
        assert_eq!(
            domain_networks(xml),
            Ok(vec!["lab".into(), "default".into()])
        );
        assert_eq!(domain_networks("<domain/>"), Ok(vec![]));
        assert_eq!(
            domain_networks("<pool/>").unwrap_err().to_string(),
            "the XML root is <pool>, not <domain>"
        );
    }

    #[test]
    fn a_domain_without_devices_has_no_disks() {
        assert_eq!(domain_disks("<domain><name>x</name></domain>"), Ok(vec![]));
    }

    #[test]
    fn a_doctype_is_rejected_before_parsing() {
        let xml = r#"<?xml version="1.0"?><!DOCTYPE d [<!ENTITY x "y">]><domain/>"#;
        assert_eq!(domain_disks(xml), Err(XmlError::Doctype));
    }

    #[test]
    fn the_root_must_match() {
        assert_eq!(
            domain_disks("<network/>"),
            Err(XmlError::Root {
                found: "network".into(),
                expected: "domain"
            })
        );
    }

    #[test]
    fn broken_xml_is_an_error() {
        assert!(matches!(domain_disks("<domain>"), Err(XmlError::Parse(_))));
    }
}
