//! The one identity of an open native project.
//!
//! A `ProjectId` is derived only from [`NativeProject::root`], which
//! `NativeProject::open` has already canonicalized, so every spelling of the
//! same folder (`/` or `\`, a trailing separator, a `\\?\` prefix, another
//! letter case) names the same project. The hash input is exactly the one the
//! Map side has always used (`map_context::project_id_for_path`), so Map
//! sessions, candidate directories, and palette stamps keep their keys.
//!
//! The manifest name is display text only and never an identity.

use std::fmt;

use crate::native_project::NativeProject;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProjectId(String);

impl ProjectId {
    /// The identity of an opened project.
    pub fn of(project: &NativeProject) -> Self {
        Self(crate::map_context::project_id_for_path(
            project.root().to_path_buf(),
        ))
    }

    /// The identity of the project folder `root` without reading its
    /// manifest: `root` canonicalized exactly as `NativeProject::open` does
    /// before [`ProjectId::of`] hashes it, so a project whose `project.eap`
    /// is broken mid-turn keeps the id (and git scope) it opened with. A
    /// root that can no longer be canonicalized is hashed as given, which is
    /// how `NativeProject::open` received it.
    pub fn of_root(root: &std::path::Path) -> Self {
        let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        Self(crate::map_context::project_id_for_path(root))
    }

    /// An id this app recorded earlier (a session's `projectId`, a Map
    /// session's `project`, an autonomous checkpoint). It is never computed
    /// from a path here: only [`ProjectId::of`] turns a folder into an id.
    pub(crate) fn from_stored(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for ProjectId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Fixture projects for tests anywhere in the crate.
#[cfg(test)]
pub(crate) mod test_support {
    use std::path::{Path, PathBuf};

    use crate::native_project::{NativeProject, ProjectManifest, PROJECT_SCHEMA_VERSION};

    /// A fresh empty directory under the system temp dir.
    pub(crate) fn temp_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "eud-agent-{label}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    /// Create a native project named `name` at `root` with a fixture source map.
    pub(crate) fn create_project(root: &Path, name: &str) -> NativeProject {
        std::fs::create_dir_all(root.join("maps")).unwrap();
        std::fs::write(root.join("maps/source.scx"), b"fixture map").unwrap();
        NativeProject::create(
            root,
            ProjectManifest {
                schema_version: PROJECT_SCHEMA_VERSION,
                name: name.to_string(),
                source_map: "maps/source.scx".to_string(),
                output_map: "build/output.scx".to_string(),
                main_file: "src/main.eps".to_string(),
                settings: Default::default(),
                plugins: Vec::new(),
                python_entrypoints: Vec::new(),
                python_dependencies: Vec::new(),
                python_lock: None,
                editor_compatibility: None,
                e2_import: None,
            },
        )
        .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{create_project, temp_root};
    use super::*;

    #[test]
    fn every_spelling_of_one_folder_is_one_project_id() {
        let temp = temp_root("project-id-spelling");
        let root = temp.join("Proj").join("X");
        let created = create_project(&root, "Sample");
        let expected = ProjectId::of(&created);

        let canonical = std::fs::canonicalize(&root).unwrap();
        let plain = canonical
            .to_string_lossy()
            .trim_start_matches(r"\\?\")
            .to_string();
        let mut spellings = vec![
            root.clone(),
            canonical.clone(),
            std::path::PathBuf::from(format!("{plain}{}", std::path::MAIN_SEPARATOR)),
        ];
        if cfg!(windows) {
            spellings.push(std::path::PathBuf::from(plain.replace('\\', "/")));
            spellings.push(std::path::PathBuf::from(format!(
                "{}/",
                plain.replace('\\', "/")
            )));
            spellings.push(std::path::PathBuf::from(format!(r"\\?\{plain}")));
            spellings.push(std::path::PathBuf::from(plain.to_uppercase()));
            spellings.push(std::path::PathBuf::from(plain.to_lowercase()));
        }
        for spelling in spellings {
            let opened = NativeProject::open(&spelling)
                .unwrap_or_else(|error| panic!("{}: {error}", spelling.display()));
            assert_eq!(ProjectId::of(&opened), expected, "{}", spelling.display());
        }
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn a_project_id_is_the_key_map_sessions_already_carry() {
        let temp = temp_root("project-id-map-key");
        let project = create_project(&temp.join("p"), "Sample");
        // The Map side has always keyed a project by SHA-256 of the lowercased
        // `<canonical root>\project.json` (that file does not exist for an
        // `.eap` project, so the joined path itself is hashed).
        let identity = std::fs::canonicalize(temp.join("p"))
            .unwrap()
            .join("project.json")
            .to_string_lossy()
            .to_lowercase();
        let historical = crate::map_model::hex_sha256(identity.as_bytes());
        assert_eq!(ProjectId::of(&project).as_str(), historical);
        std::fs::remove_dir_all(temp).unwrap();
    }
}
