//! Storage volume operations (PRD F7): list, create, and delete the volumes of
//! any running pool. The XML comes from `lodger_core::xml::volume`.
//!
//! libvirt sends no event for a volume, so a create or a delete sends Lodger's
//! own pool event: the inventory then reads the pool's sizes again.

use std::collections::BTreeSet;

use lodger_core::model::{Volume, VolumeKind};
use lodger_core::xml::domain_disks;
use lodger_core::xml::volume::VolumeXml;
use uuid::Uuid;
use virt::connect::Connect;
use virt::storage_pool::StoragePool;
use virt::storage_vol::StorageVol;
use virt::sys::VIR_DOMAIN_XML_INACTIVE;

use crate::conn::{Error, Virt};
use crate::delete::{canonical, path_of};
use crate::events::Event;

/// Lodger's own pool event code, sent after a volume create or delete. Like
/// the autostart code, it only makes the inventory read the pool again.
pub(crate) const POOL_VOLUMES: i32 = -2;

impl Virt {
    /// The volumes of running pool `pool`, sorted by name, each with the VMs
    /// that use it. The pool is refreshed first, so a file that another
    /// program added counts too.
    pub async fn volumes(&self, pool: Uuid) -> Result<Vec<Volume>, Error> {
        self.job(move |c| Ok(volumes_on(c, pool))).await?
    }

    /// Creates a volume from `xml` in running pool `pool`. libvirt refuses a
    /// name that the pool has already, even if the caller did not check.
    pub async fn create_volume(&self, pool: Uuid, xml: String) -> Result<(), Error> {
        self.job(move |c| {
            Ok(running_pool(c, pool).and_then(|found| {
                StorageVol::create_xml(&found, &xml, 0)?;
                Ok(())
            }))
        })
        .await??;
        self.pool_changed(pool);
        Ok(())
    }

    /// Deletes volume `name` of running pool `pool`, unless a VM uses it: then
    /// the error names every such VM.
    pub async fn delete_volume(&self, pool: Uuid, name: String) -> Result<(), Error> {
        self.job(move |c| Ok(delete_on(c, pool, &name))).await??;
        self.pool_changed(pool);
        Ok(())
    }

    fn pool_changed(&self, id: Uuid) {
        let _ = self.hub.send(Event::Pool {
            id,
            event: POOL_VOLUMES,
        });
    }
}

fn volumes_on(c: &Connect, id: Uuid) -> Result<Vec<Volume>, Error> {
    let pool = running_pool(c, id)?;
    pool.refresh(0)?;
    let disks = disks_of_all_domains(c)?;
    let mut out = Vec::new();
    for volume in pool.list_all_volumes(0)? {
        out.push(describe(&volume, &disks)?);
    }
    sort_by_name(&mut out);
    Ok(out)
}

/// A directory pool lists its files in the order of the folder scan, so the
/// list is sorted here. The test driver returns them sorted already.
fn sort_by_name(volumes: &mut [Volume]) {
    volumes.sort_by(|a, b| a.name.cmp(&b.name));
}

fn delete_on(c: &Connect, id: Uuid, name: &str) -> Result<(), Error> {
    let pool = running_pool(c, id)?;
    let volume = pool.lookup_storage_vol_by_name(name)?;
    let users = describe(&volume, &disks_of_all_domains(c)?)?.used_by;
    if !users.is_empty() {
        return Err(Error::InUse(users));
    }
    volume.delete(0)?;
    Ok(())
}

/// Pool `id`, if it runs: libvirt lists and changes volumes only then.
fn running_pool(c: &Connect, id: Uuid) -> Result<StoragePool, Error> {
    let pool = c.lookup_storage_pool_by_uuid(id)?;
    if !pool.is_active()? {
        return Err(Error::WrongState("the pool is not running"));
    }
    Ok(pool)
}

/// One disk of a VM: the VM's name and libvirt's path of the disk, with every
/// link resolved.
struct DiskOf {
    vm: String,
    key: String,
}

/// The disks of every VM, from both the saved and the live configuration. A
/// VM that disappears during the scan is left out.
fn disks_of_all_domains(c: &Connect) -> Result<Vec<DiskOf>, Error> {
    let mut out = Vec::new();
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
        for xml in xmls {
            for disk in domain_disks(&xml).unwrap_or_default() {
                if let Some(path) = path_of(c, &disk.source) {
                    out.push(DiskOf {
                        vm: vm.clone(),
                        key: canonical(&path),
                    });
                }
            }
        }
    }
    Ok(out)
}

/// The facts of `volume` and the VMs among `disks` that use it.
fn describe(volume: &StorageVol, disks: &[DiskOf]) -> Result<Volume, Error> {
    let xml = VolumeXml::parse(&volume.xml_desc(0)?)?;
    let info = volume.info()?;
    let path = volume.path()?;
    let key = canonical(&path);
    let used_by: BTreeSet<String> = disks
        .iter()
        .filter(|d| d.key == key)
        .map(|d| d.vm.clone())
        .collect();
    Ok(Volume {
        name: volume.name()?,
        key: volume.key()?,
        path,
        kind: VolumeKind::from_code(info.kind),
        format: xml.format().map(str::to_owned),
        capacity_bytes: info.capacity,
        allocation_bytes: info.allocation,
        used_by: used_by.into_iter().collect(),
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use lodger_core::validate::Name;
    use lodger_core::xml::pool::NewPool;
    use lodger_core::xml::volume::{NewVolume, VolumeFormat};
    use uuid::Uuid;

    use super::POOL_VOLUMES;
    use crate::{Error, Event, Virt};

    const TEST_URI: &str = "test:///default";

    async fn pool(virt: &Virt, name: &str) -> Uuid {
        let xml = NewPool::dir(
            Name::parse("Pool name", name).unwrap(),
            &format!("/srv/{name}"),
        )
        .unwrap()
        .to_xml();
        virt.create_pool(xml, false).await.unwrap()
    }

    fn volume(name: &str, format: VolumeFormat, bytes: u64) -> String {
        NewVolume::new(Name::parse("name", name).unwrap(), format, bytes)
            .unwrap()
            .to_xml()
    }

    /// Defines a stopped VM with one file disk at `path`, and returns its name.
    async fn vm_with_disk(virt: &Virt, name: &'static str, path: String) {
        let xml = format!(
            "<domain type='test'><name>{name}</name><memory>65536</memory>\
             <os><type>hvm</type></os><devices><disk type='file' device='disk'>\
             <source file='{path}'/><target dev='vda'/></disk></devices></domain>"
        );
        virt.read(move |c| c.define_domain_xml(&xml).map(|_| ()))
            .await
            .unwrap();
    }

    async fn undefine(virt: &Virt, name: &'static str) {
        virt.read(move |c| c.lookup_domain_by_name(name)?.undefine())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_volume_is_created_listed_and_deleted_with_events() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let id = pool(&virt, "vol-life").await;
        let mut events = virt.subscribe();
        virt.create_volume(id, volume("disk1.qcow2", VolumeFormat::Qcow2, 20 << 30))
            .await
            .unwrap();
        let want = Event::Pool {
            id,
            event: POOL_VOLUMES,
        };
        tokio::time::timeout(Duration::from_secs(5), async {
            while events.recv().await.unwrap() != want {}
        })
        .await
        .expect("no pool event within 5 seconds");

        let listed = virt.volumes(id).await.unwrap();
        assert_eq!(listed.len(), 1, "{listed:?}");
        assert_eq!(listed[0].name, "disk1.qcow2");
        assert_eq!(listed[0].capacity_bytes, 20 << 30);
        assert_eq!(listed[0].path, "/srv/vol-life/disk1.qcow2");
        assert!(listed[0].used_by.is_empty());

        virt.delete_volume(id, "disk1.qcow2".into()).await.unwrap();
        assert!(virt.volumes(id).await.unwrap().is_empty());
        let err = virt
            .delete_volume(id, "disk1.qcow2".into())
            .await
            .unwrap_err();
        assert!(err.is_not_found(), "{err}");
        virt.remove_pool(id, false).await.unwrap();
    }

    #[tokio::test]
    async fn a_duplicate_name_fails_in_libvirt_too() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let id = pool(&virt, "vol-dup").await;
        let xml = volume("same.img", VolumeFormat::Raw, 1 << 20);
        virt.create_volume(id, xml.clone()).await.unwrap();
        assert!(virt.create_volume(id, xml).await.is_err());
        assert_eq!(virt.volumes(id).await.unwrap().len(), 1);
        // The list is sorted by name, whatever order libvirt keeps.
        for name in ["zz.img", "aa.img"] {
            virt.create_volume(id, volume(name, VolumeFormat::Raw, 1 << 20))
                .await
                .unwrap();
        }
        let names: Vec<String> = virt
            .volumes(id)
            .await
            .unwrap()
            .into_iter()
            .map(|v| v.name)
            .collect();
        assert_eq!(names, ["aa.img", "same.img", "zz.img"]);
        virt.remove_pool(id, true).await.unwrap();
    }

    #[tokio::test]
    async fn a_volume_that_a_vm_uses_is_kept_and_names_the_vm() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let id = pool(&virt, "vol-used").await;
        virt.create_volume(id, volume("root.qcow2", VolumeFormat::Qcow2, 1 << 30))
            .await
            .unwrap();
        vm_with_disk(&virt, "vol-used-vm", "/srv/vol-used/root.qcow2".into()).await;

        let listed = virt.volumes(id).await.unwrap();
        assert_eq!(listed[0].used_by, ["vol-used-vm"]);
        let err = virt
            .delete_volume(id, "root.qcow2".into())
            .await
            .unwrap_err();
        assert!(
            matches!(&err, Error::InUse(vms) if vms == &["vol-used-vm"]),
            "{err}"
        );
        assert_eq!(err.to_string(), "in use by vol-used-vm");
        assert_eq!(virt.volumes(id).await.unwrap().len(), 1, "the volume stays");

        undefine(&virt, "vol-used-vm").await;
        virt.delete_volume(id, "root.qcow2".into()).await.unwrap();
        virt.remove_pool(id, false).await.unwrap();
    }

    #[tokio::test]
    async fn a_stopped_pool_has_no_volume_calls() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let id = pool(&virt, "vol-stopped").await;
        virt.set_pool_active(id, false).await.unwrap();
        for err in [
            virt.volumes(id).await.unwrap_err(),
            virt.create_volume(id, volume("x", VolumeFormat::Raw, 1 << 20))
                .await
                .unwrap_err(),
            virt.delete_volume(id, "x".into()).await.unwrap_err(),
        ] {
            assert!(err.is_invalid_operation(), "{err}");
            // Lodger's own words, not libvirt's "storage pool is not active".
            assert_eq!(err.to_string(), "the pool is not running");
        }
        virt.remove_pool(id, false).await.unwrap();
    }

    #[test]
    fn volumes_are_sorted_by_name() {
        let volume = |name: &str| lodger_core::model::Volume {
            name: name.to_owned(),
            key: String::new(),
            path: String::new(),
            kind: lodger_core::model::VolumeKind::File,
            format: None,
            capacity_bytes: 0,
            allocation_bytes: 0,
            used_by: Vec::new(),
        };
        let mut list = vec![volume("b"), volume("c"), volume("a")];
        super::sort_by_name(&mut list);
        let names: Vec<&str> = list.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names, ["a", "b", "c"]);
    }
}
