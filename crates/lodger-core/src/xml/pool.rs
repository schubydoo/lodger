//! Storage pool XML (PRD F6): build a new directory or NFS pool, and read any
//! pool that libvirt returns.
//!
//! [`PoolXml`] keeps the whole document, so writing it back keeps every
//! element that Lodger does not model, such as permissions and mount options.

use xmltree::{Element, XMLNode};

use super::{XmlError, element_with, parse, text_element, write};
use crate::validate::{InputError, Name, parse_host, parse_path};

/// Folders that must never hold a pool: removing a pool can delete the files
/// in its folder.
const SYSTEM_PATHS: [&str; 17] = [
    "/", "/bin", "/boot", "/dev", "/etc", "/home", "/lib", "/lib32", "/lib64", "/opt", "/proc",
    "/root", "/run", "/sbin", "/sys", "/usr", "/var",
];

/// Where the pool's volumes live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PoolSource {
    /// A folder on the host.
    Dir,
    /// An NFS export, which libvirt mounts on the target path.
    Nfs { host: String, export: String },
}

/// A new pool, with every input checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPool {
    pub name: Name,
    /// The folder on the host, in its plain form.
    pub path: String,
    pub source: PoolSource,
}

impl NewPool {
    /// A directory pool. libvirt creates the folder when the pool is built.
    pub fn dir(name: Name, path: &str) -> Result<Self, InputError> {
        Ok(Self {
            name,
            path: target_path(path)?,
            source: PoolSource::Dir,
        })
    }

    /// An NFS pool: `host:export` mounted on `path`.
    pub fn nfs(name: Name, host: &str, export: &str, path: &str) -> Result<Self, InputError> {
        Ok(Self {
            name,
            path: target_path(path)?,
            source: PoolSource::Nfs {
                host: parse_host("NFS server", host)?,
                export: parse_path("NFS export", export)?,
            },
        })
    }

    /// The pool XML for `virStoragePoolDefineXML`.
    pub fn to_xml(&self) -> String {
        let kind = match self.source {
            PoolSource::Dir => "dir",
            PoolSource::Nfs { .. } => "netfs",
        };
        let mut pool = element_with("pool", &[("type", kind)]);
        push(&mut pool, text_element("name", self.name.as_str()));
        if let PoolSource::Nfs { host, export } = &self.source {
            let mut source = Element::new("source");
            push(&mut source, element_with("host", &[("name", host)]));
            push(&mut source, element_with("dir", &[("path", export)]));
            push(&mut source, element_with("format", &[("type", "nfs")]));
            push(&mut pool, source);
        }
        let mut target = Element::new("target");
        push(&mut target, text_element("path", &self.path));
        push(&mut pool, target);
        write(&pool)
    }

    /// Rejects the pool if an existing pool uses its folder or, for NFS, its
    /// export. `existing` holds each pool's name and XML.
    pub fn check_against<'a>(
        &self,
        existing: impl IntoIterator<Item = (&'a str, &'a PoolXml)>,
    ) -> Result<(), InputError> {
        for (name, other) in existing {
            if other.target_path().and_then(plain) == Some(self.path.clone()) {
                return Err(InputError::PathInUse {
                    field: "Pool folder",
                    other_name: name.to_owned(),
                });
            }
            if let (PoolSource::Nfs { host, export }, Some((other_host, other_export))) =
                (&self.source, other.nfs_source())
                && host.eq_ignore_ascii_case(other_host)
                && plain(other_export).as_ref() == Some(export)
            {
                return Err(InputError::NfsExportInUse {
                    other_name: name.to_owned(),
                });
            }
        }
        Ok(())
    }
}

/// Checks the pool folder: an absolute path that is not a system folder.
fn target_path(value: &str) -> Result<String, InputError> {
    const FIELD: &str = "Pool folder";
    let path = parse_path(FIELD, value)?;
    if SYSTEM_PATHS.contains(&path.as_str()) {
        return Err(InputError::SystemPath { field: FIELD, path });
    }
    Ok(path)
}

/// The plain form of a path from existing XML, or `None` if it is not one.
fn plain(path: &str) -> Option<String> {
    parse_path("path", path).ok()
}

fn push(parent: &mut Element, child: Element) {
    parent.children.push(XMLNode::Element(child));
}

/// A pool document as libvirt returns it.
#[derive(Debug, Clone, PartialEq)]
pub struct PoolXml(Element);

impl PoolXml {
    pub fn parse(xml: &str) -> Result<Self, XmlError> {
        parse(xml, "pool").map(Self)
    }

    /// The pool type, such as `dir`, `netfs`, or `logical`.
    pub fn pool_type(&self) -> Option<&str> {
        self.0.attributes.get("type").map(String::as_str)
    }

    pub fn name(&self) -> Option<&str> {
        child_text(&self.0, "name")
    }

    /// The folder of the pool on the host.
    pub fn target_path(&self) -> Option<&str> {
        self.0
            .get_child("target")
            .and_then(|t| child_text(t, "path"))
    }

    /// The server and the export of an NFS pool.
    pub fn nfs_source(&self) -> Option<(&str, &str)> {
        if self.pool_type() != Some("netfs") {
            return None;
        }
        let source = self.0.get_child("source")?;
        let format = source
            .get_child("format")
            .and_then(|f| f.attributes.get("type"));
        if format.is_some_and(|f| f != "nfs" && f != "auto") {
            return None;
        }
        let host = source.get_child("host")?.attributes.get("name")?;
        let export = source.get_child("dir")?.attributes.get("path")?;
        Some((host, export))
    }

    /// The document with every element that it came with.
    pub fn to_xml(&self) -> String {
        write(&self.0)
    }
}

fn child_text<'a>(element: &'a Element, name: &str) -> Option<&'a str> {
    element
        .get_child(name)?
        .children
        .iter()
        .find_map(|n| match n {
            XMLNode::Text(text) => Some(text.as_str()),
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory pool from libvirt 11.3.0 on Debian 13.
    const DIR: &str = include_str!("fixtures/pool-dir.xml");
    /// An NFS pool with permissions and mount options, from libvirt 10.0.0.
    const NETFS: &str = include_str!("fixtures/pool-netfs.xml");

    fn name(value: &str) -> Name {
        Name::parse("Pool name", value).unwrap()
    }

    #[test]
    fn a_directory_pool_has_a_fixed_text() {
        let pool = NewPool::dir(name("images"), "/srv/vm//images/").unwrap();
        assert_eq!(
            pool.to_xml(),
            "<pool type=\"dir\">\n  <name>images</name>\n  <target>\n    \
             <path>/srv/vm/images</path>\n  </target>\n</pool>"
        );
    }

    #[test]
    fn an_nfs_pool_has_a_fixed_text() {
        let pool = NewPool::nfs(
            name("nas"),
            "nas.lan",
            "/volume1/vm/",
            "/var/lib/libvirt/nas",
        )
        .unwrap();
        assert_eq!(
            pool.to_xml(),
            "<pool type=\"netfs\">\n  <name>nas</name>\n  <source>\n    \
             <host name=\"nas.lan\" />\n    <dir path=\"/volume1/vm\" />\n    \
             <format type=\"nfs\" />\n  </source>\n  <target>\n    \
             <path>/var/lib/libvirt/nas</path>\n  </target>\n</pool>"
        );
    }

    #[test]
    fn the_text_is_the_same_on_every_run() {
        let pool = NewPool::nfs(name("nas"), "10.0.0.5", "/export", "/mnt/nas").unwrap();
        let first = pool.to_xml();
        for _ in 0..50 {
            assert_eq!(pool.to_xml(), first);
        }
        let read = PoolXml::parse(NETFS).unwrap();
        let first = read.to_xml();
        for _ in 0..50 {
            assert_eq!(PoolXml::parse(NETFS).unwrap().to_xml(), first);
        }
    }

    /// Every element path in the document, with its attributes and text.
    fn inventory(element: &Element, prefix: &str, out: &mut Vec<String>) {
        let path = format!("{prefix}/{}", element.name);
        let text = element.get_text().unwrap_or_default();
        out.push(format!("{path} {:?} {text}", element.attributes));
        for child in element.children.iter().filter_map(XMLNode::as_element) {
            inventory(child, &path, out);
        }
    }

    #[test]
    fn a_round_trip_keeps_every_element() {
        for fixture in [DIR, NETFS] {
            let original = xmltree::Element::parse(fixture.as_bytes()).unwrap();
            let again = PoolXml::parse(&PoolXml::parse(fixture).unwrap().to_xml()).unwrap();
            assert_eq!(again.0, original, "{fixture}");
            let (mut before, mut after) = (Vec::new(), Vec::new());
            inventory(&original, "", &mut before);
            inventory(&again.0, "", &mut after);
            assert_eq!(after, before);
        }
        // The elements that Lodger does not model are still in the text.
        let text = PoolXml::parse(NETFS).unwrap().to_xml();
        for kept in [
            "<protocol ver=\"4.1\" />",
            "<label>system_u:object_r:virt_image_t:s0</label>",
            "<fs:option name=\"noatime\" />",
            "xmlns:fs=\"http://libvirt.org/schemas/storagepool/fs/1.0\"",
        ] {
            assert!(text.contains(kept), "{kept} is missing from\n{text}");
        }
    }

    #[test]
    fn a_new_pool_reads_back_the_same() {
        let pool = NewPool::nfs(name("nas"), "nas.lan", "/volume1/vm", "/mnt/nas").unwrap();
        let read = PoolXml::parse(&pool.to_xml()).unwrap();
        assert_eq!(read.pool_type(), Some("netfs"));
        assert_eq!(read.name(), Some("nas"));
        assert_eq!(read.target_path(), Some("/mnt/nas"));
        assert_eq!(read.nfs_source(), Some(("nas.lan", "/volume1/vm")));
    }

    #[test]
    fn the_fixtures_read_as_libvirt_wrote_them() {
        let dir = PoolXml::parse(DIR).unwrap();
        assert_eq!(
            (
                dir.pool_type(),
                dir.name(),
                dir.target_path(),
                dir.nfs_source()
            ),
            (
                Some("dir"),
                Some("images"),
                Some("/var/lib/libvirt/images"),
                None
            )
        );
        let nfs = PoolXml::parse(NETFS).unwrap();
        assert_eq!(nfs.nfs_source(), Some(("nas.lan", "/volume1/vm")));
        assert_eq!(nfs.target_path(), Some("/var/lib/libvirt/nas"));
    }

    #[test]
    fn a_glusterfs_netfs_pool_is_not_an_nfs_source() {
        let xml = r#"<pool type="netfs"><name>g</name><source><host name="g1"/>
            <dir path="/vol"/><format type="glusterfs"/></source>
            <target><path>/mnt/g</path></target></pool>"#;
        assert_eq!(PoolXml::parse(xml).unwrap().nfs_source(), None);
    }

    #[test]
    fn a_folder_that_another_pool_uses_fails_and_names_it() {
        let existing = PoolXml::parse(DIR).unwrap();
        for path in [
            "/var/lib/libvirt/images",
            "/var/lib/libvirt/images/",
            "/var//lib/libvirt/images",
        ] {
            let pool = NewPool::dir(name("again"), path).unwrap();
            let err = pool.check_against([("images", &existing)]).unwrap_err();
            assert_eq!(
                err,
                InputError::PathInUse {
                    field: "Pool folder",
                    other_name: "images".into()
                }
            );
            assert_eq!(
                err.to_string(),
                "Pool folder is the folder of pool \"images\" already"
            );
        }
        let other = NewPool::dir(name("other"), "/var/lib/libvirt/images2").unwrap();
        assert_eq!(other.check_against([("images", &existing)]), Ok(()));
        // Another tool may have written the old pool's path with extra slashes.
        let messy = PoolXml::parse(
            "<pool type='dir'><name>old</name><target><path>/srv//vm/</path></target></pool>",
        )
        .unwrap();
        let pool = NewPool::dir(name("new"), "/srv/vm").unwrap();
        assert!(matches!(
            pool.check_against([("old", &messy)]),
            Err(InputError::PathInUse { .. })
        ));
    }

    #[test]
    fn an_nfs_export_that_another_pool_mounts_fails() {
        let existing = PoolXml::parse(NETFS).unwrap();
        let pool = NewPool::nfs(name("nas2"), "NAS.lan", "/volume1/vm/", "/mnt/nas2").unwrap();
        assert_eq!(
            pool.check_against([("nas-images", &existing)]),
            Err(InputError::NfsExportInUse {
                other_name: "nas-images".into()
            })
        );
        let messy = PoolXml::parse(
            "<pool type='netfs'><name>old</name><source><host name='nas.lan'/>\
             <dir path='/volume1//vm/'/><format type='nfs'/></source>\
             <target><path>/mnt/old</path></target></pool>",
        )
        .unwrap();
        assert!(matches!(
            pool.check_against([("old", &messy)]),
            Err(InputError::NfsExportInUse { .. })
        ));
        let other = NewPool::nfs(name("nas3"), "nas.lan", "/volume1/iso", "/mnt/nas3").unwrap();
        assert_eq!(other.check_against([("nas-images", &existing)]), Ok(()));
        // A directory pool never clashes with an export.
        let dir = NewPool::dir(name("d"), "/volume1/vm").unwrap();
        assert_eq!(dir.check_against([("nas-images", &existing)]), Ok(()));
    }

    #[test]
    fn bad_input_is_rejected_with_the_field() {
        let dir = |path: &str| NewPool::dir(name("p"), path).unwrap_err();
        assert!(matches!(dir("srv/vm"), InputError::PathNotAbsolute { .. }));
        assert!(matches!(
            dir("/srv/../etc"),
            InputError::PathDotSegment { .. }
        ));
        assert!(matches!(
            dir("/srv/./vm"),
            InputError::PathDotSegment { .. }
        ));
        assert!(matches!(dir("/srv/v\0m"), InputError::Nul { .. }));
        for system in SYSTEM_PATHS
            .iter()
            .map(|p| (*p).to_owned())
            .chain(["/etc/".into(), "//usr".into()])
        {
            assert!(
                matches!(dir(&system), InputError::SystemPath { .. }),
                "{system}"
            );
        }
        // A folder below a system folder is fine.
        assert!(NewPool::dir(name("p"), "/home/vm").is_ok());
        assert!(NewPool::dir(name("p"), "/var/lib/libvirt/images").is_ok());
        let nfs = |host, export| NewPool::nfs(name("p"), host, export, "/mnt/p").unwrap_err();
        assert!(matches!(
            nfs("nas lan", "/x"),
            InputError::HostSyntax { .. }
        ));
        assert!(matches!(nfs("-o", "/x"), InputError::HostSyntax { .. }));
        assert!(matches!(nfs("nas;rm", "/x"), InputError::HostSyntax { .. }));
        assert!(matches!(
            nfs("nas.lan", "x"),
            InputError::PathNotAbsolute { .. }
        ));
        assert!(NewPool::nfs(name("p"), "fd00::5", "/x", "/mnt/p").is_ok());
    }
}
