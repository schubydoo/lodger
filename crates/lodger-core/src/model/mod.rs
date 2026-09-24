//! Model types for the objects that Lodger shows: the host, VMs, storage
//! pools, volumes, networks, and snapshots.
//!
//! Names here are plain strings, because other tools can give libvirt objects
//! names outside Lodger's allowlist. The UI shows them only as text. New names
//! from users go through [`crate::validate::Name`].
//!
//! The `from_code` functions map libvirt's C enum values. An unknown value from
//! a newer libvirt becomes `Unknown` instead of a panic.

mod host;
mod network;
mod pool;
mod snapshot;
mod stats;
mod vm;
mod volume;

pub use host::HostInfo;
pub use network::Network;
pub use pool::{Pool, PoolState};
pub use snapshot::Snapshot;
pub use stats::{Counters, StatValue, VmStats};
pub use vm::{Vm, VmState};
pub use volume::{Volume, VolumeKind};
