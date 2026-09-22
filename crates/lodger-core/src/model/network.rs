use serde::Serialize;
use uuid::Uuid;

/// A libvirt virtual network.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Network {
    pub uuid: Uuid,
    pub name: String,
    pub active: bool,
    pub persistent: bool,
    pub autostart: bool,
    /// The host bridge device, for example `virbr0`. libvirt reports none for
    /// some forward modes, such as macvtap.
    pub bridge: Option<String>,
}
