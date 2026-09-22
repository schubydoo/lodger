use serde::Serialize;
use uuid::Uuid;

/// A libvirt storage pool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Pool {
    pub uuid: Uuid,
    pub name: String,
    pub state: PoolState,
    /// Sizes in bytes. libvirt reports 0 for an inactive pool.
    pub capacity_bytes: u64,
    pub allocation_bytes: u64,
    pub available_bytes: u64,
    pub persistent: bool,
    pub autostart: bool,
}

/// The state of a storage pool (`virStoragePoolState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PoolState {
    Inactive,
    Building,
    Running,
    /// Running, but with reduced performance or redundancy.
    Degraded,
    /// Running, but the storage cannot be reached, for example a lost NFS share.
    Inaccessible,
    Unknown,
}

impl PoolState {
    pub fn from_code(code: u32) -> Self {
        match code {
            0 => Self::Inactive,
            1 => Self::Building,
            2 => Self::Running,
            3 => Self::Degraded,
            4 => Self::Inaccessible,
            _ => Self::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_libvirt_state_maps() {
        let states: Vec<_> = (0..=5).map(PoolState::from_code).collect();
        assert_eq!(
            states,
            [
                PoolState::Inactive,
                PoolState::Building,
                PoolState::Running,
                PoolState::Degraded,
                PoolState::Inaccessible,
                PoolState::Unknown,
            ]
        );
    }

    #[test]
    fn state_serializes_as_snake_case() {
        assert_eq!(
            serde_json::to_value(PoolState::Inaccessible).unwrap(),
            serde_json::json!("inaccessible")
        );
    }
}
