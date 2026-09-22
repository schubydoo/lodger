use serde::Serialize;

/// A domain snapshot. The domain and the name identify one, and `parent`
/// links the snapshots into a tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Snapshot {
    pub name: String,
    /// `None` for a root of the tree.
    pub parent: Option<String>,
    /// `true` for the snapshot that a revert without a name would use.
    pub current: bool,
}
