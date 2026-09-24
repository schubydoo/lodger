//! Delete a VM, and on request the volumes that only it uses (PRD F4).
//!
//! The VM must be shut off. Lodger reads the data disks from the domain XML
//! before it removes the definition, then deletes each disk that is a
//! volume in a libvirt storage pool. It keeps a disk that another VM uses, a
//! read-only or shareable disk, and a file outside every pool, and it lists
//! each one as skipped with the reason. Lodger deletes only through libvirt,
//! never a file on its own.

use std::collections::HashMap;

use lodger_core::xml::{Disk, DiskSource, domain_disks};
use uuid::Uuid;
use virt::connect::Connect;
use virt::domain::Domain;
use virt::error::ErrorNumber;
use virt::sys::{
    VIR_DOMAIN_UNDEFINE_CHECKPOINTS_METADATA, VIR_DOMAIN_UNDEFINE_MANAGED_SAVE,
    VIR_DOMAIN_UNDEFINE_NVRAM, VIR_DOMAIN_UNDEFINE_SNAPSHOTS_METADATA, VIR_DOMAIN_XML_INACTIVE,
};

use crate::conn::{Error, Virt};

/// What a delete did with the VM's disks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Removal {
    /// The paths of the volumes that libvirt deleted.
    pub removed: Vec<String>,
    /// The disks that stay, each with the reason.
    pub skipped: Vec<Skipped>,
}

/// A disk that a delete kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    /// The path, or `pool/volume` for a volume that libvirt cannot find.
    pub path: String,
    pub reason: SkipReason,
}

/// Why a delete kept a disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    /// Another VM uses the disk. The field holds its name.
    UsedBy(String),
    /// The disk is read-only or shareable, so other VMs may use it.
    Shared,
    /// No libvirt storage pool holds the disk.
    NotInPool,
    /// libvirt refused to delete the volume. The field holds its message.
    Failed(String),
}

impl Virt {
    /// Removes the definition of the shut-off domain `id`. With
    /// `remove_volumes`, it also deletes the volumes that only this domain
    /// uses. Without it, every disk stays and the report is empty.
    pub async fn delete(&self, id: Uuid, remove_volumes: bool) -> Result<Removal, Error> {
        self.job(move |c| Ok(delete_on(c, id, remove_volumes, |_| {})))
            .await?
    }
}

/// One data disk of the domain to delete.
struct Target {
    /// libvirt's path: a pool volume's own path, or the path as written.
    path: String,
    /// The same file with symlinks resolved, to compare with other VMs.
    key: String,
    shared: bool,
}

/// The delete itself, on one connection. `before_undefine` runs between the
/// state check and the undefine; the tests use it to start the domain there.
fn delete_on(
    c: &Connect,
    id: Uuid,
    remove_volumes: bool,
    before_undefine: impl FnOnce(&Connect),
) -> Result<Removal, Error> {
    let domain = c.lookup_domain_by_uuid(id)?;
    if domain.is_active()? {
        return Err(Error::WrongState("shut the VM down first"));
    }
    let mut report = Removal::default();
    // Read every disk before the undefine: afterwards the XML is gone.
    let targets = if remove_volumes {
        let xml = domain.xml_desc(VIR_DOMAIN_XML_INACTIVE)?;
        resolve(c, domain_disks(&xml)?, &mut report)
    } else {
        Vec::new()
    };
    let users = if targets.is_empty() {
        HashMap::new()
    } else {
        paths_of_other_domains(c, c.list_all_domains(0)?, id)?
    };
    before_undefine(c);
    undefine(&domain)?;
    // libvirt undefines a running domain too and makes it transient. If the
    // domain still exists, it started after the state check: its guest may
    // be writing to every disk now.
    match c.lookup_domain_by_uuid(id) {
        Ok(_) => {
            return Err(Error::WrongState(
                "the VM started during the delete: libvirt removed its definition, but it runs until it stops, and every disk stays",
            ));
        }
        Err(e) if e.code().known() == Some(ErrorNumber::NoDomain) => {}
        Err(e) => return Err(e.into()),
    }
    for Target { path, key, shared } in targets {
        let reason = if shared {
            Some(SkipReason::Shared)
        } else if let Some(user) = users.get(&key) {
            Some(SkipReason::UsedBy(user.clone()))
        } else {
            match c.lookup_storage_vol_by_path(&path) {
                Ok(volume) => volume
                    .delete(0)
                    .err()
                    .map(|e| SkipReason::Failed(e.message().to_owned())),
                Err(e) if e.code().known() == Some(ErrorNumber::NoStorageVolume) => {
                    Some(SkipReason::NotInPool)
                }
                Err(e) => Some(SkipReason::Failed(e.message().to_owned())),
            }
        };
        match reason {
            None => report.removed.push(path),
            Some(reason) => report.skipped.push(Skipped { path, reason }),
        }
    }
    Ok(report)
}

/// Removes the definition with its NVRAM file, managed save image, and
/// snapshot and checkpoint metadata. A driver that does not know a flag,
/// such as the test driver with NVRAM, gets the flags that every driver
/// knows.
fn undefine(domain: &Domain) -> Result<(), virt::error::Error> {
    let all = VIR_DOMAIN_UNDEFINE_NVRAM
        | VIR_DOMAIN_UNDEFINE_MANAGED_SAVE
        | VIR_DOMAIN_UNDEFINE_SNAPSHOTS_METADATA
        | VIR_DOMAIN_UNDEFINE_CHECKPOINTS_METADATA;
    match domain.undefine_flags(all) {
        Err(e) if e.code().known() == Some(ErrorNumber::InvalidArg) => domain.undefine_flags(
            VIR_DOMAIN_UNDEFINE_MANAGED_SAVE | VIR_DOMAIN_UNDEFINE_SNAPSHOTS_METADATA,
        ),
        other => other,
    }
}

/// The targets of the disks. A pool volume that libvirt cannot find goes to
/// the report as skipped.
fn resolve(c: &Connect, disks: Vec<Disk>, report: &mut Removal) -> Vec<Target> {
    let mut targets = Vec::new();
    for disk in disks {
        match path_of(c, &disk.source) {
            Some(path) => targets.push(Target {
                key: canonical(&path),
                path,
                shared: disk.shared,
            }),
            None => {
                if let DiskSource::Volume { pool, volume } = disk.source {
                    report.skipped.push(Skipped {
                        path: format!("{pool}/{volume}"),
                        reason: SkipReason::NotInPool,
                    });
                }
            }
        }
    }
    targets
}

/// libvirt's path of a disk: the volume's own path when a pool holds it,
/// else the path as written.
fn path_of(c: &Connect, source: &DiskSource) -> Option<String> {
    match source {
        // libvirt's lookup cleans a double slash, but it follows no symlink.
        DiskSource::File(path) | DiskSource::Block(path) => Some(
            c.lookup_storage_vol_by_path(path)
                .and_then(|v| v.path())
                .unwrap_or_else(|_| path.clone()),
        ),
        DiskSource::Volume { pool, volume } => c
            .lookup_storage_pool_by_name(pool)
            .and_then(|p| p.lookup_storage_vol_by_name(volume))
            .and_then(|v| v.path())
            .ok(),
    }
}

/// The path with every symlink and `..` resolved, so that two names of one
/// file compare equal. A path that the file system cannot resolve, such as a
/// missing file, stays as it is.
fn canonical(path: &str) -> String {
    std::fs::canonicalize(path).map_or_else(|_| path.to_owned(), |p| p.display().to_string())
}

/// The disks of the `domains` other than `id`, each canonical path with the
/// name of one domain that uses it. A running domain counts with its live
/// and its saved configuration, because a hot-plugged disk is only in the
/// live one. A domain that disappears during the scan uses no disk any more.
fn paths_of_other_domains(
    c: &Connect,
    domains: Vec<Domain>,
    id: Uuid,
) -> Result<HashMap<String, String>, Error> {
    let mut users = HashMap::new();
    for domain in domains {
        let (name, xmls) = match disks_config(&domain, id) {
            Ok(Some(found)) => found,
            Ok(None) => continue,
            Err(e) if e.code().known() == Some(ErrorNumber::NoDomain) => continue,
            Err(e) => return Err(e.into()),
        };
        for xml in xmls {
            for disk in domain_disks(&xml)? {
                if let Some(path) = path_of(c, &disk.source) {
                    users
                        .entry(canonical(&path))
                        .or_insert_with(|| name.clone());
                }
            }
        }
    }
    Ok(users)
}

/// The name and the XML documents of `domain`, or `None` for domain `id`.
fn disks_config(
    domain: &Domain,
    id: Uuid,
) -> Result<Option<(String, Vec<String>)>, virt::error::Error> {
    if domain.uuid()? == id {
        return Ok(None);
    }
    let mut xmls = vec![domain.xml_desc(VIR_DOMAIN_XML_INACTIVE)?];
    if domain.is_active()? {
        xmls.push(domain.xml_desc(0)?);
    }
    Ok(Some((domain.name()?, xmls)))
}

#[cfg(test)]
mod tests {
    use uuid::Uuid;
    use virt::storage_vol::StorageVol;

    use super::{Removal, SkipReason, Skipped};
    use crate::Virt;

    const TEST_URI: &str = "test:///default";
    /// The test driver's pool. Each test uses its own volume names.
    const POOL: &str = "default-pool";

    async fn volume(virt: &Virt, name: &'static str) -> String {
        virt.read(move |c| {
            let pool = c.lookup_storage_pool_by_name(POOL)?;
            let xml = format!("<volume><name>{name}</name><capacity>1024</capacity></volume>");
            StorageVol::create_xml(&pool, &xml, 0)?.path()
        })
        .await
        .unwrap()
    }

    fn file_disk(path: &str, target: &str, extra: &str) -> String {
        format!(
            "<disk type='file' device='disk'><source file='{path}'/>\
             <target dev='{target}'/>{extra}</disk>"
        )
    }

    async fn define(virt: &Virt, name: &'static str, disks: String) -> Uuid {
        virt.read(move |c| {
            let xml = format!(
                "<domain type='test'><name>{name}</name><memory>1024</memory>\
                 <os><type>hvm</type></os><devices>{disks}</devices></domain>"
            );
            c.define_domain_xml(&xml)?.uuid()
        })
        .await
        .unwrap()
    }

    async fn volume_exists(virt: &Virt, path: String) -> bool {
        virt.read(move |c| Ok(c.lookup_storage_vol_by_path(&path).is_ok()))
            .await
            .unwrap()
    }

    async fn domain_exists(virt: &Virt, id: Uuid) -> bool {
        virt.read(move |c| Ok(c.lookup_domain_by_uuid(id).is_ok()))
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn delete_with_volumes_leaves_no_definition_and_no_volumes() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let a = volume(&virt, "del-both-a.img").await;
        let b = volume(&virt, "del-both-b.img").await;
        let disks = file_disk(&a, "vda", "")
            + &format!(
                "<disk type='volume' device='disk'><source pool='{POOL}' \
                 volume='del-both-b.img'/><target dev='vdb'/></disk>"
            );
        let id = define(&virt, "del-both", disks).await;
        let report = virt.delete(id, true).await.unwrap();
        assert_eq!(
            report,
            Removal {
                removed: vec![a.clone(), b.clone()],
                skipped: vec![],
            }
        );
        assert!(!domain_exists(&virt, id).await);
        assert!(!volume_exists(&virt, a).await);
        assert!(!volume_exists(&virt, b).await);
    }

    #[tokio::test]
    async fn without_volume_removal_every_disk_stays() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let a = volume(&virt, "del-keep-a.img").await;
        let id = define(&virt, "del-keep", file_disk(&a, "vda", "")).await;
        assert_eq!(virt.delete(id, false).await.unwrap(), Removal::default());
        assert!(!domain_exists(&virt, id).await);
        assert!(volume_exists(&virt, a).await);
    }

    #[tokio::test]
    async fn a_volume_that_another_vm_uses_is_kept_and_listed() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let shared = volume(&virt, "del-used-shared.img").await;
        let own = volume(&virt, "del-used-own.img").await;
        let id = define(
            &virt,
            "del-used-a",
            file_disk(&shared, "vda", "") + &file_disk(&own, "vdb", ""),
        )
        .await;
        // The other VM names the same volume through its pool.
        let other = format!(
            "<disk type='volume' device='disk'><source pool='{POOL}' \
             volume='del-used-shared.img'/><target dev='vda'/></disk>"
        );
        define(&virt, "del-used-b", other).await;
        let report = virt.delete(id, true).await.unwrap();
        assert_eq!(report.removed, [own.as_str()]);
        assert_eq!(
            report.skipped,
            [Skipped {
                path: shared.clone(),
                reason: SkipReason::UsedBy("del-used-b".into()),
            }]
        );
        assert!(volume_exists(&virt, shared).await);
        assert!(!volume_exists(&virt, own).await);
    }

    #[tokio::test]
    async fn a_disk_only_in_a_running_vms_live_config_counts_as_used() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let live = volume(&virt, "del-live-shared.img").await;
        let id = define(&virt, "del-live-a", file_disk(&live, "vda", "")).await;
        let other = define(&virt, "del-live-b", file_disk(&live, "vda", "")).await;
        virt.power(other, crate::Power::Start).await.unwrap();
        // A new saved config without the disk: only the live one keeps it,
        // like a disk that was hot-plugged.
        virt.read(move |c| {
            let xml = format!(
                "<domain type='test'><name>del-live-b</name><uuid>{other}</uuid>\
                 <memory>1024</memory><os><type>hvm</type></os></domain>"
            );
            c.define_domain_xml(&xml).map(drop)
        })
        .await
        .unwrap();
        let report = virt.delete(id, true).await.unwrap();
        assert_eq!(
            report.skipped,
            [Skipped {
                path: live.clone(),
                reason: SkipReason::UsedBy("del-live-b".into()),
            }]
        );
        assert!(volume_exists(&virt, live).await);
    }

    #[tokio::test]
    async fn shared_foreign_and_missing_disks_are_kept() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let ro = volume(&virt, "del-kept-ro.img").await;
        let disks = file_disk(&ro, "vda", "<readonly/>")
            + &file_disk("/srv/not-in-a-pool.img", "vdb", "")
            + &format!(
                "<disk type='volume' device='disk'><source pool='{POOL}' \
                 volume='del-kept-missing.img'/><target dev='vdc'/></disk>"
            )
            + "<disk type='file' device='cdrom'><source file='/default-pool/x.iso'/>\
               <target dev='sda'/></disk>";
        let id = define(&virt, "del-kept", disks).await;
        let report = virt.delete(id, true).await.unwrap();
        assert!(report.removed.is_empty(), "{report:?}");
        let mut skipped = report.skipped;
        skipped.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(
            skipped,
            [
                Skipped {
                    path: ro.clone(),
                    reason: SkipReason::Shared,
                },
                Skipped {
                    path: "/srv/not-in-a-pool.img".into(),
                    reason: SkipReason::NotInPool,
                },
                Skipped {
                    path: format!("{POOL}/del-kept-missing.img"),
                    reason: SkipReason::NotInPool,
                },
            ]
        );
        assert!(volume_exists(&virt, ro).await);
        assert!(!domain_exists(&virt, id).await);
    }

    #[tokio::test]
    async fn a_vm_that_disappears_during_the_scan_is_skipped() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let gone = define(&virt, "del-scan-gone", String::new()).await;
        let used = volume(&virt, "del-scan-used.img").await;
        let user = define(&virt, "del-scan-user", file_disk(&used, "vda", "")).await;
        let users = virt
            .read(move |c| {
                let domains = c.list_all_domains(0)?;
                // Another client undefines one domain after the list.
                c.lookup_domain_by_uuid(gone)?.undefine()?;
                Ok(super::paths_of_other_domains(c, domains, Uuid::nil()))
            })
            .await
            .unwrap()
            .unwrap();
        assert_eq!(users.get(&used).map(String::as_str), Some("del-scan-user"));
        assert!(domain_exists(&virt, user).await);
    }

    #[tokio::test]
    async fn a_vm_that_starts_during_the_delete_keeps_every_disk() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let disk = volume(&virt, "del-race.img").await;
        let id = define(&virt, "del-race", file_disk(&disk, "vda", "")).await;
        let result = virt
            .read(move |c| {
                // Another client starts the VM after Lodger's state check.
                let start = |c: &virt::connect::Connect| {
                    c.lookup_domain_by_uuid(id).unwrap().create().unwrap();
                };
                Ok(super::delete_on(c, id, true, start))
            })
            .await
            .unwrap();
        let err = result.unwrap_err();
        assert!(err.is_invalid_operation(), "{err}");
        assert!(err.to_string().contains("every disk stays"), "{err}");
        assert!(volume_exists(&virt, disk).await);
        // libvirt keeps the running VM as a transient domain.
        let transient = virt
            .read(move |c| {
                let d = c.lookup_domain_by_uuid(id)?;
                Ok(d.is_active()? && !d.is_persistent()?)
            })
            .await
            .unwrap();
        assert!(transient);
        virt.power(id, crate::Power::ForceOff).await.unwrap();
    }

    #[tokio::test]
    async fn a_disk_named_through_a_symlink_counts_as_used() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::fs::write(real.join("disk.img"), b"").unwrap();
        std::os::unix::fs::symlink(&real, dir.path().join("link")).unwrap();
        let direct = real.join("disk.img").display().to_string();
        let linked = dir.path().join("link/disk.img").display().to_string();
        let a = define(&virt, "del-link-a", file_disk(&direct, "vda", "")).await;
        let b = define(&virt, "del-link-b", file_disk(&linked, "vda", "")).await;
        // The deleted VM names the file directly, and the other one through
        // the symlink.
        let report = virt.delete(a, true).await.unwrap();
        assert_eq!(
            report.skipped,
            [Skipped {
                path: direct.clone(),
                reason: SkipReason::UsedBy("del-link-b".into()),
            }]
        );
        // The other way around.
        define(&virt, "del-link-a", file_disk(&direct, "vda", "")).await;
        let report = virt.delete(b, true).await.unwrap();
        assert_eq!(
            report.skipped,
            [Skipped {
                path: linked,
                reason: SkipReason::UsedBy("del-link-a".into()),
            }]
        );
    }

    #[test]
    fn a_path_that_does_not_resolve_stays_as_written() {
        assert_eq!(
            super::canonical("/nonexistent/lodger/x.img"),
            "/nonexistent/lodger/x.img"
        );
    }

    #[tokio::test]
    async fn a_running_vm_is_not_deleted() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let id = define(&virt, "del-running", String::new()).await;
        virt.power(id, crate::Power::Start).await.unwrap();
        let err = virt.delete(id, true).await.unwrap_err();
        assert!(err.is_invalid_operation(), "{err}");
        assert!(domain_exists(&virt, id).await);
    }

    #[tokio::test]
    async fn an_unknown_vm_is_not_found() {
        let virt = Virt::open(TEST_URI).await.unwrap();
        let err = virt.delete(Uuid::from_u128(0xde1), true).await.unwrap_err();
        assert!(err.is_not_found(), "{err}");
    }
}
