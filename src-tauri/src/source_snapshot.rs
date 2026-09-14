//! Coherent native project source snapshot used by durable workspace mirrors.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSnapshotFile {
    pub path: String,
    pub ftype: String,
    pub content: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSnapshot {
    pub project: String,
    pub identity: String,
    pub files: Vec<ProjectSnapshotFile>,
}
