//! Storage volume XML (PRD F7): build a new qcow2 or raw volume, and read any
//! volume that libvirt returns.

use xmltree::Element;

use super::{XmlError, child_text, element_with, parse, text_element, write};
use crate::validate::{InputError, Name};

/// The smallest volume: 1 MiB.
pub const MIN_BYTES: u64 = 1 << 20;
/// The largest volume: 1 PiB, far above any disk on one host, so a size with
/// a typo of several digits fails here and not in libvirt.
pub const MAX_BYTES: u64 = 1 << 50;

/// The disk formats that Lodger creates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeFormat {
    /// QEMU's copy-on-write format, which grows as the guest writes.
    Qcow2,
    /// A plain image file.
    Raw,
}

impl VolumeFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Qcow2 => "qcow2",
            Self::Raw => "raw",
        }
    }
}

/// A new volume, with every input checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewVolume {
    pub name: Name,
    pub format: VolumeFormat,
    pub capacity_bytes: u64,
}

impl NewVolume {
    pub fn new(name: Name, format: VolumeFormat, capacity_bytes: u64) -> Result<Self, InputError> {
        if !(MIN_BYTES..=MAX_BYTES).contains(&capacity_bytes) {
            return Err(InputError::SizeOutOfRange {
                field: "size",
                min: MIN_BYTES,
                max: MAX_BYTES,
            });
        }
        Ok(Self {
            name,
            format,
            capacity_bytes,
        })
    }

    /// The volume XML for `virStorageVolCreateXML`.
    pub fn to_xml(&self) -> String {
        let mut volume = Element::new("volume");
        push(&mut volume, text_element("name", self.name.as_str()));
        let mut capacity = element_with("capacity", &[("unit", "bytes")]);
        capacity
            .children
            .push(xmltree::XMLNode::Text(self.capacity_bytes.to_string()));
        push(&mut volume, capacity);
        let mut target = Element::new("target");
        push(
            &mut target,
            element_with("format", &[("type", self.format.as_str())]),
        );
        push(&mut volume, target);
        write(&volume)
    }

    /// Rejects the volume if pool `pool` has a volume with its name already.
    pub fn check_against<'a>(
        &self,
        pool: &str,
        existing: impl IntoIterator<Item = &'a str>,
    ) -> Result<(), InputError> {
        if existing.into_iter().any(|name| name == self.name.as_str()) {
            return Err(InputError::VolumeNameInUse {
                pool: pool.to_owned(),
                name: self.name.as_str().to_owned(),
            });
        }
        Ok(())
    }
}

fn push(parent: &mut Element, child: Element) {
    parent.children.push(xmltree::XMLNode::Element(child));
}

/// A volume document as libvirt returns it.
#[derive(Debug, Clone, PartialEq)]
pub struct VolumeXml(Element);

impl VolumeXml {
    pub fn parse(xml: &str) -> Result<Self, XmlError> {
        parse(xml, "volume").map(Self)
    }

    pub fn name(&self) -> Option<&str> {
        child_text(&self.0, "name")
    }

    /// The file of the volume on the host.
    pub fn path(&self) -> Option<&str> {
        self.0
            .get_child("target")
            .and_then(|t| child_text(t, "path"))
    }

    /// The disk format, such as `qcow2` or `raw`.
    pub fn format(&self) -> Option<&str> {
        self.0
            .get_child("target")?
            .get_child("format")?
            .attributes
            .get("type")
            .map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(value: &str) -> Name {
        Name::parse("name", value).unwrap()
    }

    #[test]
    fn a_new_volume_writes_its_name_size_and_format() {
        let volume = NewVolume::new(name("disk1.qcow2"), VolumeFormat::Qcow2, 20 << 30).unwrap();
        assert_eq!(
            volume.to_xml(),
            "<volume>\n  <name>disk1.qcow2</name>\n  <capacity unit=\"bytes\">21474836480</capacity>\n  <target>\n    <format type=\"qcow2\" />\n  </target>\n</volume>"
        );
        let raw = NewVolume::new(name("data.img"), VolumeFormat::Raw, MIN_BYTES).unwrap();
        assert!(raw.to_xml().contains("<format type=\"raw\" />"));
    }

    #[test]
    fn the_size_must_be_between_1_mib_and_1_pib() {
        for bad in [0, MIN_BYTES - 1, MAX_BYTES + 1] {
            let err = NewVolume::new(name("d"), VolumeFormat::Raw, bad).unwrap_err();
            assert!(matches!(err, InputError::SizeOutOfRange { .. }), "{bad}");
        }
        for good in [MIN_BYTES, MAX_BYTES] {
            NewVolume::new(name("d"), VolumeFormat::Raw, good).unwrap();
        }
    }

    #[test]
    fn a_name_in_use_is_rejected() {
        let volume = NewVolume::new(name("disk1.qcow2"), VolumeFormat::Qcow2, MIN_BYTES).unwrap();
        volume
            .check_against("images", ["disk2.qcow2", "disk1.qcow"])
            .unwrap();
        let err = volume
            .check_against("images", ["a", "disk1.qcow2"])
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "pool \"images\" has a volume \"disk1.qcow2\" already"
        );
    }

    #[test]
    fn libvirt_volume_xml_is_read() {
        let xml = r"<volume type='file'>
  <name>disk1.qcow2</name>
  <key>/var/lib/libvirt/images/disk1.qcow2</key>
  <capacity unit='bytes'>21474836480</capacity>
  <allocation unit='bytes'>200704</allocation>
  <physical unit='bytes'>196616</physical>
  <target>
    <path>/var/lib/libvirt/images/disk1.qcow2</path>
    <format type='qcow2'/>
    <permissions><mode>0600</mode></permissions>
  </target>
</volume>";
        let volume = VolumeXml::parse(xml).unwrap();
        assert_eq!(volume.name(), Some("disk1.qcow2"));
        assert_eq!(volume.path(), Some("/var/lib/libvirt/images/disk1.qcow2"));
        assert_eq!(volume.format(), Some("qcow2"));
    }

    #[test]
    fn a_volume_without_a_format_has_none() {
        let volume = VolumeXml::parse("<volume><name>x</name></volume>").unwrap();
        assert_eq!(volume.format(), None);
        assert_eq!(volume.path(), None);
        assert!(VolumeXml::parse("<pool/>").is_err());
    }
}
