//! Native project identity used to prepare the durable project workspace.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectSnapshot {
    pub project: String,
    pub identity: String,
}
