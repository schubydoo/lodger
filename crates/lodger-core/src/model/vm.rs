use serde::Serialize;
use uuid::Uuid;

/// A libvirt domain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Vm {
    pub uuid: Uuid,
    pub name: String,
    pub state: VmState,
    pub vcpus: u32,
    /// Current memory in KiB, the unit libvirt uses.
    pub memory_kib: u64,
    /// `false` for a transient domain, which disappears when it stops.
    pub persistent: bool,
    pub autostart: bool,
    /// Whether the domain has a VNC display, which the browser console needs.
    pub has_vnc: bool,
}

/// The state of a domain (`virDomainState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VmState {
    NoState,
    Running,
    Blocked,
    Paused,
    ShuttingDown,
    Shutoff,
    Crashed,
    /// Suspended by guest power management.
    Suspended,
    Unknown,
}

impl VmState {
    pub fn from_code(code: u32) -> Self {
        match code {
            0 => Self::NoState,
            1 => Self::Running,
            2 => Self::Blocked,
            3 => Self::Paused,
            4 => Self::ShuttingDown,
            5 => Self::Shutoff,
            6 => Self::Crashed,
            7 => Self::Suspended,
            _ => Self::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_libvirt_state_maps() {
        let states: Vec<_> = (0..=8).map(VmState::from_code).collect();
        assert_eq!(
            states,
            [
                VmState::NoState,
                VmState::Running,
                VmState::Blocked,
                VmState::Paused,
                VmState::ShuttingDown,
                VmState::Shutoff,
                VmState::Crashed,
                VmState::Suspended,
                VmState::Unknown,
            ]
        );
        assert_eq!(VmState::from_code(u32::MAX), VmState::Unknown);
    }

    #[test]
    fn serializes_for_the_api() {
        let vm = Vm {
            uuid: Uuid::from_u128(1),
            name: "web01".into(),
            state: VmState::ShuttingDown,
            vcpus: 2,
            memory_kib: 2_097_152,
            persistent: true,
            autostart: false,
            has_vnc: true,
        };
        assert_eq!(
            serde_json::to_value(&vm).unwrap(),
            serde_json::json!({
                "uuid": "00000000-0000-0000-0000-000000000001",
                "name": "web01",
                "state": "shutting_down",
                "vcpus": 2,
                "memory_kib": 2_097_152,
                "persistent": true,
                "autostart": false,
                "has_vnc": true,
            })
        );
    }
}
