use serde::Serialize;

/// Facts about the host that libvirt manages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostInfo {
    pub hostname: String,
    /// The libvirt version that the daemon runs, for example `11.3.0`.
    pub libvirt_version: String,
    /// Logical CPUs.
    pub cpus: u32,
    /// Memory in KiB, the unit libvirt uses.
    pub memory_kib: u64,
}
