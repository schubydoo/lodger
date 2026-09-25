//! Storage pool operations (PRD F6, flow 4.4): create, start, stop,
//! autostart, and remove. The XML comes from `lodger_core::xml::pool`.
//!
//! A new pool is defined, built, and started in one call, with autostart on
//! by default. If any step fails, for example an NFS mount, the pool is
//! stopped and undefined again, so no definition stays behind. Lodger never
//! deletes the pool's folder on that path: it may be a folder that the user
//! had already.

use std::collections::BTreeSet;

use lodger_core::xml::pool::PoolXml;
use lodger_core::xml::{DiskSource, domain_disks};
use uuid::Uuid;
use virt::connect::Connect;
use virt::error::ErrorNumber;
use virt::storage_pool::StoragePool;
use virt::storage_vol::StorageVol;
use virt::sys::VIR_DOMAIN_XML_INACTIVE;

use crate::conn::{Error, Virt};
use crate::events::Event;

/// Lodger's own pool event code, sent after an autostart change: libvirt has
/// no event for it. Every pool event only makes the inventory read the pool
/// again, so the code is never compared.
pub(crate) const POOL_AUTOSTART: i32 = -1;

impl Virt {
    /// The XML of every pool, with its name, for the checks of a new pool.
    /// A pool that disappears during the list is left out.
    pub async fn pool_xmls(&self) -> Result<Vec<(String, String)>, Error> {
        self.read(|c| xmls_of(c.list_all_storage_pools(0)?)).await
    }

    /// The XML of pool `id`.
    pub async fn pool_xml(&self, id: Uuid) -> Result<String, Error> {
        self.read(move |c| c.lookup_storage_pool_by_uuid(id)?.xml_desc(0))
            .await
    }

    /// Defines, builds, and starts a pool from `xml`, and returns its UUID.
    /// On any failure, the pool is gone again and the error is libvirt's.
    pub async fn create_pool(&self, xml: String, autostart: bool) -> Result<Uuid, Error> {
        self.job(move |c| create_pool_on(c, &xml, autostart, |_| {}))
            .await
    }

    /// Starts or stops pool `id`.
    pub async fn set_pool_active(&self, id: Uuid, active: bool) -> Result<(), Error> {
        self.job(move |c| {
            let pool = c.lookup_storage_pool_by_uuid(id)?;
            match (active, pool.is_active()?) {
                (true, true) => Ok(Err(Error::WrongState("the pool is running already"))),
                (false, false) => Ok(Err(Error::WrongState("the pool is not running"))),
                (true, false) => Ok(pool.create(0).map_err(Error::from)),
                (false, true) => Ok(pool.destroy().map_err(Error::from)),
            }
        })
        .await?
    }

    /// Switches autostart of pool `id`, and sends Lodger's own event, because
    /// libvirt sends none.
    pub async fn set_pool_autostart(&self, id: Uuid, on: bool) -> Result<(), Error> {
        self.job(move |c| c.lookup_storage_pool_by_uuid(id)?.set_autostart(on))
            .await?;
        let _ = self.hub.send(Event::Pool {
            id,
            event: POOL_AUTOSTART,
        });
        Ok(())
    }

    /// The names of the VMs with a disk in pool `id`, sorted.
    pub async fn pool_users(&self, id: Uuid) -> Result<Vec<String>, Error> {
        self.read(move |c| {
            let pool = c.lookup_storage_pool_by_uuid(id)?;
            Ok(users_of(c, &pool))
        })
        .await?
    }

    /// Removes pool `id`: stops it and undefines it. With `delete_files`, it
    /// first deletes every volume in the pool, and then the pool's folder or
    /// NFS mount folder. With `remove_empty_folder` alone, it removes the
    /// folder only if it is empty: libvirt deletes a stopped file-system pool's
    /// folder with rmdir, which keeps a folder that holds anything. A
    /// transient pool is gone after the stop, so its folder stays.
    pub async fn remove_pool(
        &self,
        id: Uuid,
        delete_files: bool,
        remove_empty_folder: bool,
    ) -> Result<(), Error> {
        self.job(move |c| {
            remove_pool_on(
                c,
                id,
                delete_files,
                remove_empty_folder,
                |v| v.delete(0),
                |_| {},
            )
        })
        .await
    }
}

/// The name and XML of each pool. A pool that another client removed after
/// the list is left out.
fn xmls_of(pools: Vec<StoragePool>) -> Result<Vec<(String, String)>, virt::error::Error> {
    let mut out = Vec::new();
    for pool in pools {
        match pool.name().and_then(|name| Ok((name, pool.xml_desc(0)?))) {
            Ok(found) => out.push(found),
            Err(e) if e.code().known() == Some(ErrorNumber::NoStoragePool) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(out)
}

/// The removal itself. `delete_volume` deletes one volume, and `before_stop`
/// runs after the volumes are gone and before the pool stops; the tests use
/// them to fail a delete and to look at the pool. If a volume
/// cannot be deleted, a pool that was stopped is stopped again. If the folder
/// cannot be deleted at the end, the pool stays defined, stopped, and empty,
/// so the user can try again.
fn remove_pool_on(
    c: &Connect,
    id: Uuid,
    delete_files: bool,
    remove_empty_folder: bool,
    delete_volume: impl Fn(&StorageVol) -> Result<(), virt::error::Error>,
    before_stop: impl FnOnce(&StoragePool),
) -> Result<(), virt::error::Error> {
    let pool = c.lookup_storage_pool_by_uuid(id)?;
    if delete_files {
        // The volumes are listed only while the pool runs.
        let was_active = pool.is_active()?;
        if !was_active {
            pool.create(0)?;
        }
        let deleted = pool.refresh(0).and_then(|()| {
            for volume in pool.list_all_volumes(0)? {
                delete_volume(&volume)?;
            }
            Ok(())
        });
        if let Err(e) = deleted {
            // Best effort: a failed removal leaves the pool as it was.
            if !was_active {
                let _ = pool.destroy();
            }
            return Err(e);
        }
    }
    before_stop(&pool);
    // Read before the stop: libvirt removes a transient pool when it stops,
    // so its folder cannot be deleted and it has no definition to remove.
    let persistent = pool.is_persistent()?;
    if pool.is_active()? {
        pool.destroy()?;
    }
    if persistent {
        if delete_files {
            pool.delete(0)?;
        } else if remove_empty_folder {
            // Best effort: a folder that holds anything, or that is gone
            // already, stays as it is, and the removal goes on.
            let _ = pool.delete(0);
        }
        pool.undefine()?;
    }
    Ok(())
}

/// The create itself. `before_start` runs after the build and the autostart;
/// the tests use it to make the start fail.
fn create_pool_on(
    c: &Connect,
    xml: &str,
    autostart: bool,
    before_start: impl FnOnce(&StoragePool),
) -> Result<Uuid, virt::error::Error> {
    let pool = c.define_storage_pool_xml(xml, 0)?;
    // Autostart before the start: the start event makes the inventory read
    // the pool, and libvirt sends no event for autostart itself.
    let started = pool
        .build(0)
        .and_then(|()| pool.set_autostart(autostart))
        .and_then(|()| {
            before_start(&pool);
            pool.create(0)
        });
    match started {
        Ok(()) => pool.uuid(),
        Err(e) => {
            // Best effort: the libvirt error that the user sees is the
            // first one.
            if pool.is_active().unwrap_or(false) {
                let _ = pool.destroy();
            }
            let _ = pool.undefine();
            Err(e)
        }
    }
}

/// The VMs whose disks are in `pool`: a volume of the pool by name, or a
/// file below the pool's folder. Both the saved and the live configuration
/// count. A VM that disappears during the scan is left out.
fn users_of(c: &Connect, pool: &StoragePool) -> Result<Vec<String>, Error> {
    let pool_name = pool.name()?;
    let xml = PoolXml::parse(&pool.xml_desc(0)?)?;
    let folder = xml
        .target_path()
        .map(|p| format!("{}/", p.trim_end_matches('/')));
    let mut users = BTreeSet::new();
    for domain in c.list_all_domains(0)? {
        let Ok(name) = domain.name() else { continue };
        let mut xmls = Vec::new();
        if let Ok(saved) = domain.xml_desc(VIR_DOMAIN_XML_INACTIVE) {
            xmls.push(saved);
        }
        if domain.is_active().unwrap_or(false)
            && let Ok(live) = domain.xml_desc(0)
        {
            xmls.push(live);
        }
        let uses = xmls.iter().any(|xml| {
            domain_disks(xml)
                .unwrap_or_default()
                .iter()
                .any(|disk| match &disk.source {
                    DiskSource::Volume { pool, .. } => *pool == pool_name,
                    DiskSource::File(path) | DiskSource::Block(path) => {
                        folder.as_deref().is_some_and(|f| path.starts_with(f))
                    }
                })
        });
        if uses {
            users.insert(name);
        }
    }
    Ok(users.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use lodger_core::validate::Name;
    use lodger_core::xml::pool::NewPool;
    use uuid::Uuid;
    use virt::storage_vol::StorageVol;

    use super::POOL_AUTOSTART;
    use crate::{Event, Virt};

    const TEST_URI: &str = "test:///default";

    fn dir_xml(name: &str) -> String {
        NewPool::dir(
            Name::parse("Pool name", name).unwrap(),
            &format!("/srv/{name}"),
        )
        .unwrap()
        .to_xml()
    }

    async fn exists(virt: &Virt, name: &'static str) -> bool {
        virt.read(move |c| Ok(c.lookup_storage_pool_by_name(name).is_ok()))
            .await
            .unwrap()
    }

    async fn state(virt: &Virt, id: Uuid) -> (bool, bool) {
        virt.read(move |c| {
            let p = c.lookup_storage_pool_by_uuid(id)?;
            Ok((p.is_active()?, p.autostart()?))
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn a_new_pool_runs_with_autostart_and_starts_and_stops() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let id = virt.create_pool(dir_xml("pool-life"), true).await.unwrap();
        assert_eq!(state(&virt, id).await, (true, true));
        let err = virt.set_pool_active(id, true).await.unwrap_err();
        assert!(err.is_invalid_operation(), "{err}");
        virt.set_pool_active(id, false).await.unwrap();
        assert_eq!(state(&virt, id).await, (false, true));
        let err = virt.set_pool_active(id, false).await.unwrap_err();
        assert!(err.is_invalid_operation(), "{err}");
        virt.set_pool_active(id, true).await.unwrap();
        virt.remove_pool(id, false, false).await.unwrap();
        assert!(!exists(&virt, "pool-life").await);
    }

    #[tokio::test]
    async fn autostart_can_be_off_and_changes_with_an_event() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let id = virt.create_pool(dir_xml("pool-auto"), false).await.unwrap();
        assert_eq!(state(&virt, id).await, (true, false));
        let mut events = virt.subscribe();
        virt.set_pool_autostart(id, true).await.unwrap();
        assert_eq!(state(&virt, id).await, (true, true));
        let want = Event::Pool {
            id,
            event: POOL_AUTOSTART,
        };
        tokio::time::timeout(Duration::from_secs(5), async {
            while events.recv().await.unwrap() != want {}
        })
        .await
        .expect("no autostart event within 5 seconds");
        virt.remove_pool(id, false, false).await.unwrap();
    }

    #[tokio::test]
    async fn a_failed_start_leaves_no_definition_behind() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let xml = dir_xml("pool-fail");
        let err = virt
            .job(move |c| {
                // Another client starts the pool first, so Lodger's start fails.
                let start = |p: &virt::storage_pool::StoragePool| p.create(0).unwrap();
                Ok(super::create_pool_on(c, &xml, true, start))
            })
            .await
            .unwrap()
            .unwrap_err();
        assert!(!err.message().is_empty());
        assert!(!exists(&virt, "pool-fail").await);
    }

    #[tokio::test]
    async fn autostart_is_set_before_the_start_event() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let xml = dir_xml("pool-order");
        let seen = virt
            .job(move |c| {
                let mut seen = None;
                let look =
                    |p: &virt::storage_pool::StoragePool| seen = Some(p.autostart().unwrap());
                let id = super::create_pool_on(c, &xml, true, look)?;
                Ok((id, seen))
            })
            .await
            .unwrap();
        assert_eq!(
            seen.1,
            Some(true),
            "autostart was off when the pool started"
        );
        virt.remove_pool(seen.0, false, false).await.unwrap();
    }

    #[tokio::test]
    async fn a_failed_volume_delete_stops_the_pool_again() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let id = virt.create_pool(dir_xml("pool-stuck"), true).await.unwrap();
        virt.read(move |c| {
            let pool = c.lookup_storage_pool_by_uuid(id)?;
            let xml = "<volume><name>busy.img</name><capacity>1024</capacity></volume>";
            StorageVol::create_xml(&pool, xml, 0).map(drop)
        })
        .await
        .unwrap();
        virt.set_pool_active(id, false).await.unwrap();
        let result = virt
            .job(move |c| {
                let busy = |_: &StorageVol| {
                    let pool = c.lookup_storage_pool_by_uuid(Uuid::nil());
                    pool.map(drop)
                };
                Ok(super::remove_pool_on(c, id, true, false, busy, |_| {}))
            })
            .await
            .unwrap();
        assert!(result.is_err());
        // Still defined, and stopped as before.
        assert_eq!(state(&virt, id).await, (false, true));
        virt.remove_pool(id, false, false).await.unwrap();
    }

    #[tokio::test]
    async fn the_users_of_a_pool_are_the_vms_with_a_disk_in_it() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let id = virt.create_pool(dir_xml("pool-users"), true).await.unwrap();
        let domain = |name: &str, disk: &str| {
            format!(
                "<domain type='test'><name>{name}</name><memory>1024</memory>\
                 <os><type>hvm</type></os><devices>{disk}</devices></domain>"
            )
        };
        let by_volume = domain(
            "pool-users-b",
            "<disk type='volume' device='disk'><source pool='pool-users' volume='v.img'/>\
             <target dev='vda'/></disk>",
        );
        let by_file = domain(
            "pool-users-a",
            "<disk type='file' device='disk'><source file='/srv/pool-users/x.img'/>\
             <target dev='vda'/></disk>",
        );
        let elsewhere = domain(
            "pool-users-c",
            "<disk type='file' device='disk'><source file='/srv/pool-users-other/x.img'/>\
             <target dev='vda'/></disk>",
        );
        virt.read(move |c| {
            for xml in [by_volume, by_file, elsewhere] {
                c.define_domain_xml(&xml)?;
            }
            Ok(())
        })
        .await
        .unwrap();
        assert_eq!(
            virt.pool_users(id).await.unwrap(),
            ["pool-users-a", "pool-users-b"]
        );
        virt.remove_pool(id, false, false).await.unwrap();
    }

    #[tokio::test]
    async fn remove_with_files_deletes_every_volume_first() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let id = virt.create_pool(dir_xml("pool-files"), true).await.unwrap();
        virt.read(move |c| {
            let pool = c.lookup_storage_pool_by_uuid(id)?;
            for name in ["a.img", "b.img"] {
                let xml = format!("<volume><name>{name}</name><capacity>1024</capacity></volume>");
                StorageVol::create_xml(&pool, &xml, 0)?;
            }
            Ok(())
        })
        .await
        .unwrap();
        // Stopped first: the delete must start it to list the volumes.
        virt.set_pool_active(id, false).await.unwrap();
        let left = virt
            .job(move |c| {
                let mut left = None;
                let look = |p: &virt::storage_pool::StoragePool| {
                    left = Some(p.list_all_volumes(0).unwrap().len());
                };
                super::remove_pool_on(c, id, true, false, |v| v.delete(0), look)?;
                Ok(left)
            })
            .await
            .unwrap();
        assert_eq!(left, Some(0), "volumes were left before the stop");
        assert!(!exists(&virt, "pool-files").await);
        let volume_left = virt
            .read(|c| {
                Ok(c.lookup_storage_vol_by_path("/srv/pool-files/a.img")
                    .is_ok())
            })
            .await
            .unwrap();
        assert!(!volume_left);
    }

    #[tokio::test]
    async fn a_pool_that_disappears_during_the_list_is_left_out() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let gone = virt.create_pool(dir_xml("pool-gone"), true).await.unwrap();
        let kept = virt.create_pool(dir_xml("pool-kept"), true).await.unwrap();
        let names = virt
            .job(move |c| {
                let pools = c.list_all_storage_pools(0)?;
                // Another client removes one pool after the list.
                let p = c.lookup_storage_pool_by_uuid(gone)?;
                p.destroy()?;
                p.undefine()?;
                let xmls = super::xmls_of(pools)?;
                Ok(xmls.into_iter().map(|(n, _)| n).collect::<Vec<_>>())
            })
            .await
            .unwrap();
        assert!(names.contains(&"pool-kept".to_owned()), "{names:?}");
        assert!(!names.contains(&"pool-gone".to_owned()), "{names:?}");
        virt.remove_pool(kept, false, false).await.unwrap();
    }

    #[tokio::test]
    async fn removing_the_empty_folder_still_undefines_the_pool() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let id = virt
            .create_pool(dir_xml("pool-empty-folder"), true)
            .await
            .unwrap();
        virt.remove_pool(id, false, true).await.unwrap();
        assert!(!exists(&virt, "pool-empty-folder").await);
    }

    #[tokio::test]
    async fn a_transient_pool_is_removed_without_an_error() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let xml = dir_xml("pool-transient");
        let id = virt
            .job(move |c| c.create_storage_pool_xml(&xml, 0)?.uuid())
            .await
            .unwrap();
        virt.remove_pool(id, true, false).await.unwrap();
        assert!(!exists(&virt, "pool-transient").await);
    }

    #[tokio::test]
    async fn every_pool_xml_is_listed_with_its_name() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let id = virt.create_pool(dir_xml("pool-list"), true).await.unwrap();
        let all = virt.pool_xmls().await.unwrap();
        let (_, xml) = all.iter().find(|(n, _)| n == "pool-list").unwrap();
        assert!(xml.contains("<path>/srv/pool-list</path>"), "{xml}");
        assert_eq!(&virt.pool_xml(id).await.unwrap(), xml);
        virt.remove_pool(id, false, false).await.unwrap();
    }
}
