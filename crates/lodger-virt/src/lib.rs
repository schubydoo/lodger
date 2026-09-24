//! The libvirt adapter. This is the only crate that imports `virt`.

mod cache;
mod conn;
mod console;
mod delete;
mod errors;
mod events;
mod power;
mod stats;
mod supervisor;

pub use cache::Inventory;
pub use conn::{Error, Virt};
pub use delete::{Removal, SkipReason, Skipped};
pub use errors::{Explanation, explain};
pub use events::{DomainChange, Event};
pub use power::Power;
pub use supervisor::{ConnState, Host};

/// Returns the version of the libvirt client library that Lodger links to,
/// as `(major, minor, micro)`.
pub fn client_library_version() -> Result<(u32, u32, u32), virt::error::Error> {
    let v = virt::connect::Connect::version()?;
    Ok((v / 1_000_000, v / 1000 % 1000, v % 1000))
}

#[cfg(test)]
mod tests {
    use lodger_core::validate::Name;
    use lodger_core::xml::pool::{NewPool, PoolXml};

    #[test]
    fn reports_the_client_library_version() {
        let (major, _, _) = super::client_library_version().unwrap();
        assert!(major >= 9, "Lodger needs libvirt 9.0.0 or newer");
    }

    /// The pool XML from `lodger-core` goes through libvirt and back.
    #[tokio::test]
    async fn libvirt_accepts_the_pool_xml_and_returns_the_same_pool() {
        let virt = crate::Virt::open("test:///default").await.unwrap();
        let name = |n| Name::parse("Pool name", n).unwrap();
        let pools = [
            NewPool::dir(name("lodger-xml-dir"), "/srv/lodger-xml-dir").unwrap(),
            NewPool::nfs(
                name("lodger-xml-nfs"),
                "nas.lan",
                "/export/vm",
                "/mnt/lodger-xml",
            )
            .unwrap(),
        ];
        for pool in pools {
            let xml = pool.to_xml();
            let back = virt
                .read(move |c| {
                    let defined = c.define_storage_pool_xml(&xml, 0)?;
                    let back = defined.xml_desc(0)?;
                    defined.undefine()?;
                    Ok(back)
                })
                .await
                .unwrap();
            let read = PoolXml::parse(&back).unwrap();
            assert_eq!(read.name(), Some(pool.name.as_str()));
            assert_eq!(read.target_path(), Some(pool.path.as_str()));
            // libvirt's answer counts as an existing pool with this folder.
            assert!(pool.check_against([("same", &read)]).is_err());
        }
    }
}
