use std::fs;
use std::path::{Component, Path, PathBuf};

use super::e2_fixture::{FixtureBundle, FixtureError, FixtureRoot, MAX_RESOURCE_BYTES};

pub(crate) const MAX_FIXTURE_BYTES: usize = 512 * 1024;

pub(crate) fn oracle_json(bundle: &FixtureBundle) -> Result<String, FixtureError> {
    serde_json::to_string_pretty(&bundle.oracle)
        .map_err(|error| FixtureError::OracleMismatch(error.to_string()))
}

pub(crate) fn assert_safe_fixture_root(root: &Path) -> Result<(), FixtureError> {
    let canonical = fs::canonicalize(root).map_err(FixtureError::from)?;
    if canonical.components().any(|component| {
        matches!(component, Component::Normal(value) if value == ".research" || value == "defense_mob")
    }) {
        return Err(FixtureError::InvalidRoot(canonical.display().to_string()));
    }
    Ok(())
}

pub(crate) fn write_confined_fixture_file(
    root: &Path,
    relative: &str,
    bytes: &[u8],
) -> Result<PathBuf, FixtureError> {
    FixtureRoot::new(root)?.write(relative, bytes, MAX_RESOURCE_BYTES)
}
