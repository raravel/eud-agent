mod events;
mod foreground;
mod parser;
mod process;
mod request;
mod state;
mod structured;
mod tool_results;

pub use state::ProductionClaudeCodeAdapter;

#[cfg(test)]
use crate::provider::{ProviderConversationState, ProviderId};
#[cfg(test)]
#[cfg_attr(not(windows), allow(unused_imports))]
use crate::provider_runtime::{
    AdapterEventKind, AdapterRequestKind, AdapterStepOutcome, AdapterStepRequest, NormalizedBlock,
    ProviderAdapter, ProviderRuntimeError, RunIdentity,
};
#[cfg(test)]
use events::ParsedEvent;
#[cfg(test)]
use parser::ClaudeStreamParser;
#[cfg(test)]
use request::MAX_STDOUT_BYTES;
#[cfg(test)]
use serde_json::{json, Value};
#[cfg(test)]
use state::CLAUDE_PROVIDER_DEFAULT;
#[cfg(test)]
use std::path::{Path, PathBuf};

// The process tests drive PowerShell fixtures and run on Windows only.
#[cfg(test)]
#[cfg_attr(not(windows), allow(dead_code, unused_imports))]
mod tests {
    include!("adapter_tests.rs");
}
