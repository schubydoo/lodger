use serde::Serialize;

/// A volume in a storage pool. Volumes have no UUID: the pool and the name
/// identify one, and `key` is unique on the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Volume {
    pub name: String,
    pub key: String,
    pub path: String,
    pub kind: VolumeKind,
    /// The disk format, such as `qcow2` or `raw`, if libvirt names one.
    pub format: Option<String>,
    /// Sizes in bytes. A sparse or qcow2 file allocates less than its capacity.
    pub capacity_bytes: u64,
    pub allocation_bytes: u64,
    /// The VMs with a disk on this volume, sorted by name.
    pub used_by: Vec<String>,
    /// The qcow2 overlays in the same pool that use this volume as their
    /// backing file, sorted by name.
    pub backing_for: Vec<String>,
}

/// The type of a volume (`virStorageVolType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VolumeKind {
    File,
    Block,
    Dir,
    Network,
    NetDir,
    Ploop,
    Unknown,
}

impl VolumeKind {
    pub fn from_code(code: u32) -> Self {
        match code {
            0 => Self::File,
            1 => Self::Block,
            2 => Self::Dir,
            3 => Self::Network,
            4 => Self::NetDir,
            5 => Self::Ploop,
            _ => Self::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_libvirt_type_maps() {
        let kinds: Vec<_> = (0..=6).map(VolumeKind::from_code).collect();
        assert_eq!(
            kinds,
            [
                VolumeKind::File,
                VolumeKind::Block,
                VolumeKind::Dir,
                VolumeKind::Network,
                VolumeKind::NetDir,
                VolumeKind::Ploop,
                VolumeKind::Unknown,
            ]
        );
    }

    #[test]
    fn kind_serializes_as_snake_case() {
        assert_eq!(
            serde_json::to_value(VolumeKind::NetDir).unwrap(),
            serde_json::json!("net_dir")
        );
    }
}
