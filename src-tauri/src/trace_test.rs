//! Isolated epScript runtime tracing and single-map test execution.
//!
//! The runner clones the editor-generated euddraft build directory, appends one
//! request-owned epScript plugin, builds into the user's StarCraft Maps folder,
//! launches a dedicated 32-bit client, and reads a structured ring buffer from
//! that exact process. The connected source map and editor project are read-only.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::bridge_io::{EpsSnapshot, EpsSnapshotFile};
use crate::config::DataDirs;
use crate::eps_preflight::normalize_editor_path;

const TRACE_MAGIC: &[u8; 16] = b"EUDAGENTTRACEV1!";
const TRACE_VERSION: u32 = 1;
const TRACE_CAPACITY: u32 = 256;
const TRACE_RECORD_DWORDS: u32 = 8;
const TRACE_HEADER_BYTES: usize = 64;
const TRACE_RECORD_BYTES: usize = TRACE_RECORD_DWORDS as usize * 4;
const TRACE_BUFFER_BYTES: usize = TRACE_HEADER_BYTES + TRACE_CAPACITY as usize * TRACE_RECORD_BYTES;
const MAX_TEST_CODE_BYTES: usize = 128 * 1024;
const MAX_TEST_NAME_BYTES: usize = 512;
const MAX_SYMBOLS: usize = TRACE_CAPACITY as usize;
const MAX_BUILD_COPY_BYTES: u64 = 256 * 1024 * 1024;
const MAX_BUILD_COPY_FILES: usize = 8_192;
const EUDDRAFT_TIMEOUT: Duration = Duration::from_secs(300);
const WINDOW_TIMEOUT: Duration = Duration::from_secs(20);
const DEFAULT_TEST_TIMEOUT_MS: u64 = 30_000;
const MIN_TEST_TIMEOUT_MS: u64 = 1_000;
const MAX_TEST_TIMEOUT_MS: u64 = 120_000;
const TEST_MAP_PREFIX: &str = "zzzz-eud-agent-";
const PERSISTENT_TEST_ROOT: &str = "tests/";
const PERSISTENT_TEST_SUFFIX: &str = ".tests.eps";
const MAX_PERSISTENT_TESTS: usize = 256;
const INTERNAL_BEGIN_EVENT: u32 = 0xffff_ff00;
#[cfg(windows)]
const TRACE_INJECTOR_EXE: &[u8] = include_bytes!(env!("EUD_TRACE_INJECTOR_EXE"));

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TraceTestInput {
    pub name: String,
    pub code: String,
    #[serde(default)]
    pub symbols: Vec<TraceSymbol>,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TraceSuiteInput {
    #[serde(default)]
    pub tests: Option<Vec<String>>,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PersistentTraceTest {
    pub path: String,
    pub code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PersistentTraceSelection {
    pub discovered: Vec<String>,
    pub tests: Vec<PersistentTraceTest>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TraceSymbol {
    pub event_id: u32,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TracePhase {
    Build,
    Launch,
    Run,
}

impl TracePhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Launch => "launch",
            Self::Run => "run",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TraceTestStatus {
    Passed,
    Failed,
    Inconclusive,
}

impl TraceTestStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Inconclusive => "inconclusive",
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TraceEvent {
    pub sequence: u32,
    pub tick: u32,
    pub event_id: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_name: Option<String>,
    pub severity: String,
    pub values: [u32; 4],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TraceTestResult {
    pub run_id: String,
    pub name: String,
    pub status: TraceTestStatus,
    pub reason: String,
    pub summary: String,
    pub events: Vec<TraceEvent>,
    pub dropped_events: u32,
    pub duration_ms: u64,
    pub source_map_unchanged: bool,
    pub log_dir: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TraceSuiteCaseResult {
    pub path: String,
    pub status: TraceTestStatus,
    pub reason: String,
    pub summary: String,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_map_unchanged: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TraceSuiteResult {
    pub suite_id: String,
    pub status: TraceTestStatus,
    pub reason: String,
    pub discovered: Vec<String>,
    pub selected: Vec<String>,
    pub passed: usize,
    pub failed: usize,
    pub inconclusive: usize,
    pub tests: Vec<TraceSuiteCaseResult>,
    pub duration_ms: u64,
    pub log_dir: String,
}

#[derive(Debug)]
struct TraceOutcome {
    status: TraceTestStatus,
    reason: String,
    events: Vec<TraceEvent>,
    dropped_events: u32,
}

impl TraceOutcome {
    fn inconclusive(reason: impl Into<String>) -> Self {
        Self {
            status: TraceTestStatus::Inconclusive,
            reason: reason.into(),
            events: Vec::new(),
            dropped_events: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TraceHeader {
    write_sequence: u32,
    dropped_events: u32,
    test_state: u32,
    failure_event_id: u32,
    heartbeat_tick: u32,
}

#[derive(Debug)]
struct PreparedBuild {
    copied_eds: PathBuf,
    euddraft: PathBuf,
    source_map: PathBuf,
    staged_map: PathBuf,
    marker: [u8; 32],
}

#[derive(Default)]
struct CopyBudget {
    bytes: u64,
    files: usize,
}

fn default_timeout_ms() -> u64 {
    DEFAULT_TEST_TIMEOUT_MS
}

pub fn validate_input(input: &TraceTestInput) -> Result<(), String> {
    let name = input.name.trim();
    if name.is_empty() || name.len() > MAX_TEST_NAME_BYTES {
        return Err(format!(
            "trace test name must contain 1 to {MAX_TEST_NAME_BYTES} UTF-8 bytes"
        ));
    }
    if input.code.trim().is_empty() || input.code.len() > MAX_TEST_CODE_BYTES {
        return Err(format!(
            "trace test code must contain 1 to {MAX_TEST_CODE_BYTES} UTF-8 bytes"
        ));
    }
    if !input.code.contains("function eudAgentTestSetup(")
        || !input.code.contains("function eudAgentTestStep(")
    {
        return Err(
            "trace test code must define eudAgentTestSetup() and eudAgentTestStep(tick)"
                .to_string(),
        );
    }
    if !(MIN_TEST_TIMEOUT_MS..=MAX_TEST_TIMEOUT_MS).contains(&input.timeout_ms) {
        return Err(format!(
            "timeoutMs must be between {MIN_TEST_TIMEOUT_MS} and {MAX_TEST_TIMEOUT_MS}"
        ));
    }
    if input.symbols.len() > MAX_SYMBOLS {
        return Err(format!("trace test supports at most {MAX_SYMBOLS} symbols"));
    }
    let mut ids = BTreeSet::new();
    for symbol in &input.symbols {
        if symbol.event_id == INTERNAL_BEGIN_EVENT {
            return Err(format!(
                "eventId {INTERNAL_BEGIN_EVENT} is reserved for the trace runtime"
            ));
        }
        if !ids.insert(symbol.event_id) {
            return Err(format!(
                "duplicate trace symbol eventId {}",
                symbol.event_id
            ));
        }
        if symbol.name.trim().is_empty() || symbol.name.len() > 128 {
            return Err("trace symbol names must contain 1 to 128 UTF-8 bytes".to_string());
        }
        if symbol
            .source
            .as_ref()
            .is_some_and(|source| source.len() > 512)
        {
            return Err("trace symbol source must not exceed 512 UTF-8 bytes".to_string());
        }
        if symbol.line == Some(0) {
            return Err("trace symbol line must be greater than zero".to_string());
        }
    }
    Ok(())
}

pub(crate) fn select_persistent_tests(
    snapshot: &EpsSnapshot,
    input: &TraceSuiteInput,
) -> Result<PersistentTraceSelection, String> {
    if !(MIN_TEST_TIMEOUT_MS..=MAX_TEST_TIMEOUT_MS).contains(&input.timeout_ms) {
        return Err(format!(
            "timeoutMs must be between {MIN_TEST_TIMEOUT_MS} and {MAX_TEST_TIMEOUT_MS}"
        ));
    }
    if input.tests.as_ref().is_some_and(Vec::is_empty) {
        return Err(
            "tests must be omitted to run all persistent tests, or contain at least one path"
                .to_string(),
        );
    }

    let mut sources = BTreeMap::<String, (String, Option<String>)>::new();
    for file in &snapshot.files {
        let Some(path) = logical_eps_path(file)? else {
            continue;
        };
        if !is_persistent_test_path(&path) {
            continue;
        }
        let key = path.to_lowercase();
        if let Some((previous, _)) = sources.insert(key, (path.clone(), file.content.clone())) {
            return Err(format!(
                "persistent test paths collide case-insensitively: {previous} and {path}"
            ));
        }
    }

    let discovered = sources
        .values()
        .map(|(path, _)| path.clone())
        .collect::<Vec<_>>();
    let selected_keys = match &input.tests {
        None => sources.keys().cloned().collect::<Vec<_>>(),
        Some(requested) => {
            let mut seen = BTreeSet::new();
            let mut keys = Vec::with_capacity(requested.len());
            for raw_path in requested {
                let path = normalize_editor_path(raw_path)?;
                if !is_persistent_test_path(&path) {
                    return Err(format!(
                        "persistent test path must match tests/**/*.tests.eps: {raw_path}"
                    ));
                }
                let key = path.to_lowercase();
                if !seen.insert(key.clone()) {
                    return Err(format!(
                        "persistent test path is selected more than once: {raw_path}"
                    ));
                }
                if !sources.contains_key(&key) {
                    return Err(format!(
                        "persistent test does not exist in the current project snapshot: {raw_path}"
                    ));
                }
                keys.push(key);
            }
            keys
        }
    };
    if selected_keys.len() > MAX_PERSISTENT_TESTS {
        return Err(format!(
            "trace suite supports at most {MAX_PERSISTENT_TESTS} selected tests"
        ));
    }

    let mut tests = Vec::with_capacity(selected_keys.len());
    for key in selected_keys {
        let (path, content) = sources
            .get(&key)
            .expect("selected persistent test key must exist");
        tests.push(PersistentTraceTest {
            path: path.clone(),
            code: content.clone(),
        });
    }
    Ok(PersistentTraceSelection { discovered, tests })
}

fn logical_eps_path(file: &EpsSnapshotFile) -> Result<Option<String>, String> {
    let editor_path = normalize_editor_path(&file.path)?;
    if editor_path.to_lowercase().ends_with(".eps") {
        Ok(Some(editor_path))
    } else if file.ftype.eq_ignore_ascii_case("CUIEps") {
        Ok(Some(format!("{editor_path}.eps")))
    } else {
        Ok(None)
    }
}

fn is_persistent_test_path(path: &str) -> bool {
    let path = path.to_lowercase();
    path.starts_with(PERSISTENT_TEST_ROOT) && path.ends_with(PERSISTENT_TEST_SUFFIX)
}

pub(crate) fn run_suite(
    dirs: &DataDirs,
    source_eds: &Path,
    euddraft: &Path,
    starcraft_setting: &Path,
    selection: PersistentTraceSelection,
    timeout_ms: u64,
    mut on_phase: impl FnMut(TracePhase),
) -> Result<TraceSuiteResult, String> {
    if !(MIN_TEST_TIMEOUT_MS..=MAX_TEST_TIMEOUT_MS).contains(&timeout_ms) {
        return Err(format!(
            "timeoutMs must be between {MIN_TEST_TIMEOUT_MS} and {MAX_TEST_TIMEOUT_MS}"
        ));
    }
    let started = Instant::now();
    let suite_id = format!("suite-{}", uuid::Uuid::new_v4().simple());
    let suite_root = dirs.logs_dir().join("trace-tests").join(&suite_id);
    fs::create_dir_all(&suite_root).map_err(|error| {
        format!(
            "failed to create trace suite log directory '{}': {error}",
            suite_root.display()
        )
    })?;
    let selected = selection
        .tests
        .iter()
        .map(|test| test.path.clone())
        .collect::<Vec<_>>();
    let mut tests = Vec::with_capacity(selection.tests.len());

    for test in selection.tests {
        let path = test.path;
        let result = match test.code {
            None => TraceSuiteCaseResult {
                path,
                status: TraceTestStatus::Inconclusive,
                reason: "unreadable_test_source".to_string(),
                summary: "INCONCLUSIVE: persistent test source is unreadable".to_string(),
                duration_ms: 0,
                run_id: None,
                source_map_unchanged: None,
                log_dir: None,
            },
            Some(code) => {
                let input = TraceTestInput {
                    name: path.clone(),
                    code,
                    symbols: Vec::new(),
                    timeout_ms,
                };
                match validate_input(&input) {
                    Err(error) => TraceSuiteCaseResult {
                        path,
                        status: TraceTestStatus::Failed,
                        reason: "invalid_test_definition".to_string(),
                        summary: format!("FAILED: {error}"),
                        duration_ms: 0,
                        run_id: None,
                        source_map_unchanged: None,
                        log_dir: None,
                    },
                    Ok(()) => match run(
                        dirs,
                        source_eds,
                        euddraft,
                        starcraft_setting,
                        input,
                        &mut on_phase,
                    ) {
                        Ok(result) => TraceSuiteCaseResult {
                            path,
                            status: result.status,
                            reason: result.reason,
                            summary: result.summary,
                            duration_ms: result.duration_ms,
                            run_id: Some(result.run_id),
                            source_map_unchanged: Some(result.source_map_unchanged),
                            log_dir: Some(result.log_dir),
                        },
                        Err(error) => TraceSuiteCaseResult {
                            path,
                            status: TraceTestStatus::Inconclusive,
                            reason: "infrastructure_error".to_string(),
                            summary: format!("INCONCLUSIVE: {error}"),
                            duration_ms: 0,
                            run_id: None,
                            source_map_unchanged: None,
                            log_dir: None,
                        },
                    },
                }
            }
        };
        tests.push(result);
    }

    let passed = tests
        .iter()
        .filter(|test| test.status == TraceTestStatus::Passed)
        .count();
    let failed = tests
        .iter()
        .filter(|test| test.status == TraceTestStatus::Failed)
        .count();
    let inconclusive = tests
        .iter()
        .filter(|test| test.status == TraceTestStatus::Inconclusive)
        .count();
    let (status, reason) = if selected.is_empty() {
        (
            TraceTestStatus::Inconclusive,
            "no_persistent_tests_found".to_string(),
        )
    } else if failed > 0 {
        (TraceTestStatus::Failed, "suite_has_failures".to_string())
    } else if inconclusive > 0 {
        (
            TraceTestStatus::Inconclusive,
            "suite_has_inconclusive_tests".to_string(),
        )
    } else {
        (TraceTestStatus::Passed, "suite_passed".to_string())
    };
    let result = TraceSuiteResult {
        suite_id,
        status,
        reason,
        discovered: selection.discovered,
        selected,
        passed,
        failed,
        inconclusive,
        tests,
        duration_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
        log_dir: suite_root.to_string_lossy().into_owned(),
    };
    write_json(&suite_root.join("suite.json"), &result)?;
    Ok(result)
}

pub fn run(
    dirs: &DataDirs,
    source_eds: &Path,
    euddraft: &Path,
    starcraft_setting: &Path,
    input: TraceTestInput,
    mut on_phase: impl FnMut(TracePhase),
) -> Result<TraceTestResult, String> {
    validate_input(&input)?;
    let started = Instant::now();
    let run_id = format!("trace-{}", uuid::Uuid::new_v4().simple());
    let run_root = dirs.logs_dir().join("trace-tests").join(&run_id);
    fs::create_dir_all(&run_root).map_err(|error| {
        format!(
            "failed to create trace test log directory '{}': {error}",
            run_root.display()
        )
    })?;

    let prepared = prepare_build(
        &run_root,
        source_eds,
        euddraft,
        &input,
        run_id.trim_start_matches("trace-"),
    )?;
    let source_hash_before = sha256_file(&prepared.source_map)?;
    write_json(&run_root.join("symbols.json"), &input.symbols)?;

    let outcome = match run_inner(
        &run_root,
        &prepared,
        starcraft_setting,
        &input,
        &mut on_phase,
    ) {
        Ok(outcome) => outcome,
        Err(error) => TraceOutcome::inconclusive(format!("infrastructure_error: {error}")),
    };
    let _ = fs::remove_file(&prepared.staged_map);

    let source_map_unchanged = sha256_file(&prepared.source_map)
        .map(|after| after == source_hash_before)
        .unwrap_or(false);
    let mut outcome = outcome;
    if !source_map_unchanged {
        outcome.status = TraceTestStatus::Inconclusive;
        outcome.reason = "source_map_changed_during_test".to_string();
    }

    write_trace_jsonl(&run_root.join("trace.jsonl"), &outcome.events)?;
    let duration_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    let summary = format!(
        "{}: {} event(s), {} dropped, reason={}",
        outcome.status.as_str().to_ascii_uppercase(),
        outcome.events.len(),
        outcome.dropped_events,
        outcome.reason
    );
    let result = TraceTestResult {
        run_id,
        name: input.name,
        status: outcome.status,
        reason: outcome.reason,
        summary,
        events: outcome.events,
        dropped_events: outcome.dropped_events,
        duration_ms,
        source_map_unchanged,
        log_dir: run_root.to_string_lossy().into_owned(),
    };
    write_json(&run_root.join("result.json"), &result)?;
    let _ = fs::remove_dir_all(run_root.join("build"));
    Ok(result)
}

fn run_inner(
    run_root: &Path,
    prepared: &PreparedBuild,
    starcraft_setting: &Path,
    input: &TraceTestInput,
    on_phase: &mut impl FnMut(TracePhase),
) -> Result<TraceOutcome, String> {
    on_phase(TracePhase::Build);
    let captured = crate::edd_runner::run_euddraft_process(
        &prepared.euddraft,
        &prepared.copied_eds,
        EUDDRAFT_TIMEOUT,
    )?;
    write_build_log(run_root, &captured.stdout, &captured.stderr)?;
    if !captured.success || !prepared.staged_map.is_file() {
        return Ok(TraceOutcome {
            status: TraceTestStatus::Failed,
            reason: "test_build_failed".to_string(),
            events: Vec::new(),
            dropped_events: 0,
        });
    }

    on_phase(TracePhase::Launch);
    let executable = resolve_x86_starcraft(starcraft_setting)?;
    if client_machine(&executable)? != 0x014c {
        return Ok(TraceOutcome::inconclusive(
            "unsupported_client_architecture",
        ));
    }
    #[cfg(windows)]
    {
        let injector_path = run_root.join("eud_trace_injector.exe");
        fs::write(&injector_path, TRACE_INJECTOR_EXE)
            .map_err(|error| format!("failed to stage x86 trace injector: {error}"))?;
        let outcome = windows::run_client(&executable, &prepared.marker, input, &injector_path);
        let _ = fs::remove_file(injector_path);
        outcome
    }
    #[cfg(not(windows))]
    {
        let _ = (executable, input);
        Ok(TraceOutcome::inconclusive("windows_runtime_required"))
    }
}

fn prepare_build(
    run_root: &Path,
    source_eds: &Path,
    euddraft: &Path,
    input: &TraceTestInput,
    run_token: &str,
) -> Result<PreparedBuild, String> {
    if !source_eds.is_absolute() || !source_eds.is_file() {
        return Err(format!(
            "generated EDS is missing or not absolute: '{}'",
            source_eds.display()
        ));
    }
    if !euddraft.is_absolute() || !euddraft.is_file() {
        return Err(format!(
            "euddraft executable is missing or not absolute: '{}'",
            euddraft.display()
        ));
    }
    let source_parent = source_eds
        .parent()
        .ok_or_else(|| format!("EDS path has no parent: '{}'", source_eds.display()))?;
    let source_build_root = source_parent.parent().ok_or_else(|| {
        format!(
            "EDS build directory has no parent: '{}'",
            source_eds.display()
        )
    })?;
    let copied_build_root = run_root.join("build");
    fs::create_dir_all(&copied_build_root).map_err(|error| error.to_string())?;
    let mut budget = CopyBudget::default();
    for entry in fs::read_dir(source_build_root).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        if entry
            .file_name()
            .to_string_lossy()
            .eq_ignore_ascii_case("backup")
        {
            continue;
        }
        copy_tree(
            &entry.path(),
            &copied_build_root.join(entry.file_name()),
            &mut budget,
        )?;
    }

    let copied_parent = copied_build_root.join(
        source_parent
            .file_name()
            .ok_or_else(|| "EDS parent has no filename".to_string())?,
    );
    let copied_eds = copied_parent.join(
        source_eds
            .file_name()
            .ok_or_else(|| "EDS path has no filename".to_string())?,
    );
    let original_eds = fs::read_to_string(source_eds)
        .map_err(|error| format!("failed to read generated EDS: {error}"))?;
    let source_map = resolve_eds_input(source_parent, &original_eds)?;
    let extension = source_map
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| value.eq_ignore_ascii_case("scx") || value.eq_ignore_ascii_case("scm"))
        .ok_or_else(|| "EDS input is not an SCM/SCX map".to_string())?;
    let input_copy = run_root.join(format!("input.{extension}"));
    fs::copy(&source_map, &input_copy).map_err(|error| {
        format!(
            "failed to snapshot source map '{}' for trace build: {error}",
            source_map.display()
        )
    })?;

    let documents = documents_dir()?;
    let map_folder = documents.join("StarCraft").join("Maps");
    fs::create_dir_all(&map_folder).map_err(|error| {
        format!(
            "failed to create StarCraft trace-test map folder '{}': {error}",
            map_folder.display()
        )
    })?;
    remove_stale_test_maps(&map_folder)?;
    let staged_map = map_folder.join(format!("{TEST_MAP_PREFIX}{run_token}.{extension}"));

    let marker = marker_for_run(run_token)?;
    let plugin = generate_plugin(input, &marker);
    let plugin_path = copied_parent.join("eud_agent_trace_test.eps");
    fs::write(&plugin_path, plugin.as_bytes())
        .map_err(|error| format!("failed to write trace test plugin: {error}"))?;
    fs::write(run_root.join("test.eps"), plugin.as_bytes())
        .map_err(|error| format!("failed to retain trace test source: {error}"))?;
    let rewritten = rewrite_eds(&original_eds, &input_copy, &staged_map, &plugin_path)?;
    fs::write(&copied_eds, rewritten.as_bytes())
        .map_err(|error| format!("failed to write isolated EDS: {error}"))?;
    fs::write(run_root.join("test.eds"), rewritten.as_bytes())
        .map_err(|error| format!("failed to retain isolated EDS: {error}"))?;

    Ok(PreparedBuild {
        copied_eds,
        euddraft: euddraft.to_path_buf(),
        source_map,
        staged_map,
        marker,
    })
}

fn copy_tree(source: &Path, destination: &Path, budget: &mut CopyBudget) -> Result<(), String> {
    let metadata = fs::symlink_metadata(source).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() {
        return Err(format!(
            "trace build copy refuses symlink '{}'",
            source.display()
        ));
    }
    if metadata.is_dir() {
        fs::create_dir_all(destination).map_err(|error| error.to_string())?;
        for entry in fs::read_dir(source).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            if entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case("__pycache__")
            {
                continue;
            }
            copy_tree(&entry.path(), &destination.join(entry.file_name()), budget)?;
        }
        return Ok(());
    }
    if !metadata.is_file() {
        return Err(format!(
            "trace build copy refuses non-file '{}'",
            source.display()
        ));
    }
    budget.files += 1;
    budget.bytes = budget.bytes.saturating_add(metadata.len());
    if budget.files > MAX_BUILD_COPY_FILES || budget.bytes > MAX_BUILD_COPY_BYTES {
        return Err("generated EDS build directory exceeds trace-copy limits".to_string());
    }
    fs::copy(source, destination).map_err(|error| error.to_string())?;
    Ok(())
}

fn resolve_eds_input(eds_parent: &Path, text: &str) -> Result<PathBuf, String> {
    let mut section = "";
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            section = &trimmed[1..trimmed.len() - 1];
            continue;
        }
        if section == "main" {
            if let Some(value) = config_value(trimmed, "input") {
                let path = PathBuf::from(value);
                let joined = if path.is_absolute() {
                    path
                } else {
                    eds_parent.join(path)
                };
                return dunce::canonicalize(&joined).map_err(|error| {
                    format!(
                        "EDS input map '{}' could not be resolved: {error}",
                        joined.display()
                    )
                });
            }
        }
    }
    Err("generated EDS has no [main] input".to_string())
}

fn rewrite_eds(text: &str, input: &Path, output: &Path, plugin: &Path) -> Result<String, String> {
    let mut section = String::new();
    let mut replaced_input = false;
    let mut replaced_output = false;
    let mut lines = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            section.clear();
            section.push_str(&trimmed[1..trimmed.len() - 1]);
        }
        if section == "main" && config_value(trimmed, "input").is_some() {
            lines.push(format!("input: {}", input.display()));
            replaced_input = true;
        } else if section == "main" && config_value(trimmed, "output").is_some() {
            lines.push(format!("output: {}", output.display()));
            replaced_output = true;
        } else {
            lines.push(line.to_string());
        }
    }
    if !replaced_input || !replaced_output {
        return Err("generated EDS [main] must contain input and output".to_string());
    }
    lines.push(format!("[{}]", plugin.display()));
    lines.push(String::new());
    Ok(lines.join("\r\n"))
}

fn config_value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let (left, right) = line.split_once(':').or_else(|| line.split_once('='))?;
    left.trim().eq(key).then(|| right.trim())
}

fn marker_for_run(run_token: &str) -> Result<[u8; 32], String> {
    let token = run_token.as_bytes();
    if token.len() < 16 || !token[..16].iter().all(u8::is_ascii_hexdigit) {
        return Err("trace run token must start with 16 ASCII hex digits".to_string());
    }
    let mut marker = [0u8; 32];
    marker[..16].copy_from_slice(TRACE_MAGIC);
    marker[16..].copy_from_slice(&token[..16]);
    Ok(marker)
}

fn generate_plugin(input: &TraceTestInput, marker: &[u8; 32]) -> String {
    let mut initial = Vec::with_capacity(TRACE_HEADER_BYTES);
    initial.extend_from_slice(marker);
    for value in [
        TRACE_VERSION,
        TRACE_CAPACITY,
        TRACE_RECORD_DWORDS,
        0,
        0,
        0,
        0,
        0,
    ] {
        initial.extend_from_slice(&value.to_le_bytes());
    }
    let initial_hex = hex_bytes(&initial);
    let records_bytes = TRACE_CAPACITY as usize * TRACE_RECORD_BYTES;
    format!(
        r#"const EUDAGENT_TRACE_CAPACITY = {capacity};
const EUDAGENT_TRACE_RECORD_DWORDS = {record_dwords};
const EUDAGENT_TRACE_BASE = Db(py_eval("bytes.fromhex('{initial_hex}') + bytes({records_bytes})"));
const EUDAGENT_TRACE_EPD = EPD(EUDAGENT_TRACE_BASE);
var eudAgentTraceTick = 0;
var eudAgentTraceSequence = 0;
var eudAgentTestState = 0;

function eudAgentTrace(eventId, severity, value0, value1, value2, value3) {{
    eudAgentTraceSequence += 1;
    const slot = eudAgentTraceSequence % EUDAGENT_TRACE_CAPACITY;
    const record = EUDAGENT_TRACE_EPD + 16 + slot * EUDAGENT_TRACE_RECORD_DWORDS;
    dwwrite_epd(record, 0);
    dwwrite_epd(record + 1, eudAgentTraceTick);
    dwwrite_epd(record + 2, eventId);
    dwwrite_epd(record + 3, severity);
    dwwrite_epd(record + 4, value0);
    dwwrite_epd(record + 5, value1);
    dwwrite_epd(record + 6, value2);
    dwwrite_epd(record + 7, value3);
    dwwrite_epd(record, eudAgentTraceSequence);
    dwwrite_epd(EUDAGENT_TRACE_EPD + 11, eudAgentTraceSequence);
    if (eudAgentTraceSequence > EUDAGENT_TRACE_CAPACITY) {{
        dwwrite_epd(EUDAGENT_TRACE_EPD + 12, eudAgentTraceSequence - EUDAGENT_TRACE_CAPACITY);
    }}
}}

function eudAgentFail(eventId, actual, expected) {{
    if (eudAgentTestState == 0) {{
        eudAgentTrace(eventId, 2, actual, expected, 0, 0);
        eudAgentTestState = 2;
        dwwrite_epd(EUDAGENT_TRACE_EPD + 14, eventId);
        dwwrite_epd(EUDAGENT_TRACE_EPD + 13, eudAgentTestState);
    }}
}}

function eudAgentAssertEq(eventId, actual, expected) {{
    if (actual == expected) {{
        eudAgentTrace(eventId, 1, actual, expected, 0, 0);
        return true;
    }}
    eudAgentFail(eventId, actual, expected);
    return false;
}}

function eudAgentPass(eventId) {{
    if (eudAgentTestState == 0) {{
        eudAgentTrace(eventId, 1, 0, 0, 0, 0);
        eudAgentTestState = 1;
        dwwrite_epd(EUDAGENT_TRACE_EPD + 13, eudAgentTestState);
    }}
}}

{code}

function onPluginStart() {{
    eudAgentTrace({internal_begin}, 0, 0, 0, 0, 0);
    eudAgentTestSetup();
}}

function beforeTriggerExec() {{
    eudAgentTraceTick += 1;
    dwwrite_epd(EUDAGENT_TRACE_EPD + 15, eudAgentTraceTick);
    if (eudAgentTestState == 0) {{
        eudAgentTestStep(eudAgentTraceTick);
    }}
}}
"#,
        code = input.code.trim(),
        capacity = TRACE_CAPACITY,
        record_dwords = TRACE_RECORD_DWORDS,
        internal_begin = INTERNAL_BEGIN_EVENT,
    )
}

fn parse_header(bytes: &[u8], marker: &[u8; 32]) -> Result<TraceHeader, String> {
    if bytes.len() < TRACE_HEADER_BYTES || &bytes[..32] != marker {
        return Err("trace header marker is invalid".to_string());
    }
    if read_u32(bytes, 32)? != TRACE_VERSION
        || read_u32(bytes, 36)? != TRACE_CAPACITY
        || read_u32(bytes, 40)? != TRACE_RECORD_DWORDS
    {
        return Err("trace header contract is invalid".to_string());
    }
    Ok(TraceHeader {
        write_sequence: read_u32(bytes, 44)?,
        dropped_events: read_u32(bytes, 48)?,
        test_state: read_u32(bytes, 52)?,
        failure_event_id: read_u32(bytes, 56)?,
        heartbeat_tick: read_u32(bytes, 60)?,
    })
}

fn decode_snapshot(
    bytes: &[u8],
    marker: &[u8; 32],
    symbols: &[TraceSymbol],
) -> Result<TraceOutcome, String> {
    if bytes.len() < TRACE_BUFFER_BYTES {
        return Err("trace buffer snapshot is truncated".to_string());
    }
    let header = parse_header(bytes, marker)?;
    let first = header
        .write_sequence
        .saturating_sub(TRACE_CAPACITY.saturating_sub(1))
        .max(1);
    let symbol_map: BTreeMap<u32, &TraceSymbol> = symbols
        .iter()
        .map(|symbol| (symbol.event_id, symbol))
        .collect();
    let mut events = Vec::with_capacity(
        header
            .write_sequence
            .saturating_sub(first)
            .saturating_add(1) as usize,
    );
    if header.write_sequence != 0 {
        for sequence in first..=header.write_sequence {
            let slot = sequence % TRACE_CAPACITY;
            let offset = TRACE_HEADER_BYTES + slot as usize * TRACE_RECORD_BYTES;
            let actual_sequence = read_u32(bytes, offset)?;
            if actual_sequence != sequence {
                return Err(format!(
                    "trace record {sequence} is torn or overwritten (found {actual_sequence})"
                ));
            }
            let event_id = read_u32(bytes, offset + 8)?;
            let severity_number = read_u32(bytes, offset + 12)?;
            let symbol = symbol_map.get(&event_id).copied();
            events.push(TraceEvent {
                sequence,
                tick: read_u32(bytes, offset + 4)?,
                event_id,
                event_name: if event_id == INTERNAL_BEGIN_EVENT {
                    Some("trace.begin".to_string())
                } else {
                    symbol.map(|item| item.name.clone())
                },
                severity: match severity_number {
                    0 => "info",
                    1 => "pass",
                    2 => "fail",
                    _ => "unknown",
                }
                .to_string(),
                values: [
                    read_u32(bytes, offset + 16)?,
                    read_u32(bytes, offset + 20)?,
                    read_u32(bytes, offset + 24)?,
                    read_u32(bytes, offset + 28)?,
                ],
                source: symbol.and_then(|item| item.source.clone()),
                line: symbol.and_then(|item| item.line),
            });
        }
    }
    let (status, reason) = match header.test_state {
        1 if header.dropped_events == 0 => (TraceTestStatus::Passed, "test_passed".to_string()),
        1 => (
            TraceTestStatus::Inconclusive,
            "trace_buffer_overflow".to_string(),
        ),
        2 => (
            TraceTestStatus::Failed,
            format!("assertion_failed:{}", header.failure_event_id),
        ),
        state => {
            return Err(format!(
                "trace snapshot is not terminal (state={state}, heartbeat={})",
                header.heartbeat_tick
            ))
        }
    };
    Ok(TraceOutcome {
        status,
        reason,
        events,
        dropped_events: header.dropped_events,
    })
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| "trace buffer field is truncated".to_string())?;
    Ok(u32::from_le_bytes(
        value.try_into().expect("four-byte slice"),
    ))
}

fn resolve_x86_starcraft(setting: &Path) -> Result<PathBuf, String> {
    if setting
        .file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("StarCraft.exe"))
        && setting.is_file()
    {
        return Ok(setting.to_path_buf());
    }
    let root = setting.parent().ok_or_else(|| {
        format!(
            "StarCraft setting has no install root: '{}'",
            setting.display()
        )
    })?;
    let candidate = root.join("x86").join("StarCraft.exe");
    if candidate.is_file() {
        Ok(candidate)
    } else {
        Err(format!(
            "32-bit StarCraft client is missing: '{}'",
            candidate.display()
        ))
    }
}

fn client_machine(executable: &Path) -> Result<u16, String> {
    let mut file = File::open(executable)
        .map_err(|error| format!("failed to inspect StarCraft executable: {error}"))?;
    let mut dos = [0u8; 64];
    file.read_exact(&mut dos)
        .map_err(|error| format!("StarCraft executable has an invalid DOS header: {error}"))?;
    if &dos[..2] != b"MZ" {
        return Err("StarCraft executable has no MZ header".to_string());
    }
    let pe_offset = u32::from_le_bytes(dos[60..64].try_into().expect("four-byte slice")) as u64;
    use std::io::{Seek, SeekFrom};
    file.seek(SeekFrom::Start(pe_offset))
        .map_err(|error| format!("failed to seek StarCraft PE header: {error}"))?;
    let mut pe = [0u8; 6];
    file.read_exact(&mut pe)
        .map_err(|error| format!("StarCraft executable has an invalid PE header: {error}"))?;
    if &pe[..4] != b"PE\0\0" {
        return Err("StarCraft executable has no PE signature".to_string());
    }
    Ok(u16::from_le_bytes([pe[4], pe[5]]))
}

fn remove_stale_test_maps(folder: &Path) -> Result<(), String> {
    for entry in fs::read_dir(folder).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(TEST_MAP_PREFIX)
            && entry
                .path()
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|ext| {
                    ext.eq_ignore_ascii_case("scx") || ext.eq_ignore_ascii_case("scm")
                })
        {
            fs::remove_file(entry.path()).map_err(|error| {
                format!("failed to remove stale trace-test map '{name}': {error}")
            })?;
        }
    }
    Ok(())
}

fn write_build_log(root: &Path, stdout: &str, stderr: &str) -> Result<(), String> {
    let mut file = BufWriter::new(
        File::create(root.join("build.log"))
            .map_err(|error| format!("failed to create trace build log: {error}"))?,
    );
    writeln!(file, "[stdout]\n{stdout}\n[stderr]\n{stderr}")
        .map_err(|error| format!("failed to write trace build log: {error}"))
}

fn write_trace_jsonl(path: &Path, events: &[TraceEvent]) -> Result<(), String> {
    let mut writer = BufWriter::new(
        File::create(path).map_err(|error| format!("failed to create trace.jsonl: {error}"))?,
    );
    for event in events {
        serde_json::to_writer(&mut writer, event)
            .map_err(|error| format!("failed to encode trace event: {error}"))?;
        writer
            .write_all(b"\n")
            .map_err(|error| format!("failed to write trace event: {error}"))?;
    }
    writer
        .flush()
        .map_err(|error| format!("failed to flush trace.jsonl: {error}"))
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("failed to encode '{}': {error}", path.display()))?;
    fs::write(path, bytes).map_err(|error| format!("failed to write '{}': {error}", path.display()))
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path)
        .map_err(|error| format!("failed to hash '{}': {error}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| format!("failed to hash '{}': {error}", path.display()))?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn hex_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(windows)]
fn documents_dir() -> Result<PathBuf, String> {
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{FOLDERID_Documents, SHGetKnownFolderPath};
    let mut raw = std::ptr::null_mut();
    let result =
        unsafe { SHGetKnownFolderPath(&FOLDERID_Documents, 0, std::ptr::null_mut(), &mut raw) };
    if result < 0 || raw.is_null() {
        return Err(format!(
            "Windows Documents folder could not be resolved (HRESULT {result:#x})"
        ));
    }
    let length = unsafe {
        let mut length = 0usize;
        while *raw.add(length) != 0 {
            length += 1;
        }
        length
    };
    let path = PathBuf::from(String::from_utf16_lossy(unsafe {
        std::slice::from_raw_parts(raw, length)
    }));
    unsafe { CoTaskMemFree(raw.cast()) };
    Ok(path)
}

#[cfg(not(windows))]
fn documents_dir() -> Result<PathBuf, String> {
    Err("Windows Documents folder is unavailable".to_string())
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::mem::{size_of, zeroed};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::process::CommandExt;
    use std::process::Command;
    use std::thread;
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, BOOL, HANDLE, HWND, INVALID_HANDLE_VALUE, LPARAM, WAIT_OBJECT_0,
        WAIT_TIMEOUT,
    };

    use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Memory::{
        VirtualQueryEx, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_GUARD, PAGE_NOACCESS,
    };
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, OpenProcess, ResumeThread, TerminateProcess, WaitForSingleObject,
        CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, INFINITE,
        PROCESS_INFORMATION, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ, STARTF_USESHOWWINDOW,
        STARTUPINFOW,
    };
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        VK_E, VK_END, VK_G, VK_M, VK_MENU, VK_O, VK_RETURN,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetForegroundWindow, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
        PostMessageW, ShowWindow, SW_MINIMIZE, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP,
    };

    const PROCESS_SCAN_MAX_X86: usize = 0x8000_0000;
    const MEMORY_CHUNK_BYTES: usize = 1024 * 1024;
    const MAX_MARKER_MATCHES: usize = 32;

    struct OwnedChild {
        process: HandleGuard,
        pid: u32,
    }

    impl OwnedChild {
        fn has_exited(&self) -> Result<bool, String> {
            let wait = unsafe { WaitForSingleObject(self.process.0, 0) };
            if wait == WAIT_OBJECT_0 {
                Ok(true)
            } else if wait == WAIT_TIMEOUT {
                Ok(false)
            } else {
                Err(last_error("failed to query owned StarCraft process"))
            }
        }
    }

    impl Drop for OwnedChild {
        fn drop(&mut self) {
            unsafe {
                let _ = TerminateProcess(self.process.0, 1);
                let _ = WaitForSingleObject(self.process.0, INFINITE);
            }
        }
    }

    pub(super) struct HandleGuard(pub(super) HANDLE);

    impl Drop for HandleGuard {
        fn drop(&mut self) {
            if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
                unsafe { CloseHandle(self.0) };
            }
        }
    }

    fn minimized_startup_info() -> STARTUPINFOW {
        let mut startup: STARTUPINFOW = unsafe { zeroed() };
        startup.cb = size_of::<STARTUPINFOW>() as u32;
        startup.dwFlags = STARTF_USESHOWWINDOW;
        startup.wShowWindow = 7;
        startup
    }

    fn launch_guarded_client(
        executable: &Path,
        injector_path: &Path,
    ) -> Result<OwnedChild, String> {
        let current_dir = executable.parent().and_then(Path::parent).ok_or_else(|| {
            format!(
                "StarCraft executable has no install root: '{}'",
                executable.display()
            )
        })?;
        let mut application = executable.as_os_str().encode_wide().collect::<Vec<_>>();
        application.push(0);
        let mut command_line = Vec::new();
        command_line.push(b'\"' as u16);
        command_line.extend(executable.as_os_str().encode_wide());
        command_line.push(b'\"' as u16);
        command_line.extend(
            " -launch -uid s1 -displayMode 0 -windowwidth 1024 -windowheight 768 -windowx 32000 -windowy 32000"
                .encode_utf16(),
        );
        command_line.push(0);
        let mut current_dir = current_dir.as_os_str().encode_wide().collect::<Vec<_>>();
        current_dir.push(0);
        let startup = minimized_startup_info();
        let mut process_info: PROCESS_INFORMATION = unsafe { zeroed() };
        let created = unsafe {
            CreateProcessW(
                application.as_ptr(),
                command_line.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                CREATE_UNICODE_ENVIRONMENT | CREATE_SUSPENDED,
                std::ptr::null(),
                current_dir.as_ptr(),
                &startup,
                &mut process_info,
            )
        };
        if created == 0 {
            return Err(last_error("failed to launch suspended 32-bit StarCraft"));
        }
        let child = OwnedChild {
            process: HandleGuard(process_info.hProcess),
            pid: process_info.dwProcessId,
        };
        let primary_thread = HandleGuard(process_info.hThread);
        run_trace_injector(injector_path, child.pid)?;
        if unsafe { ResumeThread(primary_thread.0) } == u32::MAX {
            return Err(last_error("failed to resume isolated StarCraft"));
        }
        Ok(child)
    }

    fn run_trace_injector(injector_path: &Path, pid: u32) -> Result<(), String> {
        let mut command = Command::new(injector_path);
        command
            .arg(pid.to_string())
            .creation_flags(CREATE_NO_WINDOW);
        let status = command
            .status()
            .map_err(|error| format!("failed to launch x86 trace isolation helper: {error}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!(
                "x86 trace isolation helper failed with exit code {}",
                status.code().unwrap_or(-1)
            ))
        }
    }
    pub(super) fn run_client(
        executable: &Path,
        marker: &[u8; 32],
        input: &TraceTestInput,
        injector_path: &Path,
    ) -> Result<TraceOutcome, String> {
        if starcraft_running()? {
            return Ok(TraceOutcome::inconclusive("starcraft_already_running"));
        }
        let child = launch_guarded_client(executable, injector_path)?;
        let pid = child.pid;
        let hwnd = wait_for_window(pid, &child, WINDOW_TIMEOUT)?;
        if unsafe { GetForegroundWindow() } == hwnd {
            unsafe {
                ShowWindow(hwnd, SW_MINIMIZE);
            }
            return Ok(TraceOutcome::inconclusive(
                "test_client_activated_foreground",
            ));
        }
        if unsafe { IsIconic(hwnd) } == 0 {
            unsafe {
                ShowWindow(hwnd, SW_MINIMIZE);
            }
            thread::sleep(Duration::from_millis(100));
            if unsafe { IsIconic(hwnd) } == 0 {
                return Ok(TraceOutcome::inconclusive(
                    "test_client_could_not_be_minimized",
                ));
            }
        }
        thread::sleep(Duration::from_secs(10));
        automate_to_test_map(hwnd)?;
        let process = open_process(pid)?;
        let mut active = None;
        let mut last_marker_matches = 0usize;
        for attempt in 0..3 {
            let (found, marker_matches) = inspect_trace_buffers(process.0, marker)?;
            last_marker_matches = marker_matches;
            if let Some(found) = found {
                active = Some(found);
                break;
            }
            if marker_matches != 0 {
                post_alt_window_key(hwnd, VK_O as u8)?;
                thread::sleep(Duration::from_secs(2));
                burst_window_key(hwnd, VK_O as u8)?;
            }
            thread::sleep(Duration::from_secs(4));
            if attempt == 2 && child.has_exited()? {
                return Ok(TraceOutcome::inconclusive(
                    "game_exited_before_trace_started",
                ));
            }
        }
        let Some(address) = active else {
            return Ok(TraceOutcome::inconclusive(format!(
                "trace_buffer_not_activated:marker_matches={last_marker_matches}"
            )));
        };

        let deadline = Instant::now() + Duration::from_millis(input.timeout_ms);
        loop {
            if child.has_exited()? {
                return Ok(TraceOutcome::inconclusive("game_exited_during_test"));
            }
            let header_bytes = read_memory(process.0, address, TRACE_HEADER_BYTES)?;
            let header = match parse_header(&header_bytes, marker) {
                Ok(header) => header,
                Err(_) => return Ok(TraceOutcome::inconclusive("trace_buffer_lost")),
            };
            if header.test_state == 1 || header.test_state == 2 {
                thread::sleep(Duration::from_millis(250));
                let snapshot = read_memory(process.0, address, TRACE_BUFFER_BYTES)?;
                return decode_snapshot(&snapshot, marker, &input.symbols)
                    .map_err(|error| format!("trace protocol error: {error}"));
            }
            if Instant::now() >= deadline {
                return Ok(TraceOutcome::inconclusive(format!(
                    "test_timeout_at_tick:{}",
                    header.heartbeat_tick
                )));
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn automate_to_test_map(hwnd: HWND) -> Result<(), String> {
        // Supported LAN/UDP glue path: post directly to the minimized owned
        // window. Never activate it, synthesize global input, or move the cursor.
        // G invokes the game's CreateGame binding from the UDP game list.
        post_window_key(hwnd, VK_M as u8)?;
        thread::sleep(Duration::from_millis(1_500));
        post_window_key(hwnd, VK_E as u8)?;
        thread::sleep(Duration::from_secs(2));
        post_window_key(hwnd, VK_END as u8)?;
        thread::sleep(Duration::from_millis(300));
        post_window_key(hwnd, VK_RETURN as u8)?;
        thread::sleep(Duration::from_secs(2));
        post_window_key(hwnd, VK_O as u8)?;
        thread::sleep(Duration::from_secs(2));
        post_window_key(hwnd, VK_G as u8)?;
        thread::sleep(Duration::from_secs(2));
        post_window_key(hwnd, VK_END as u8)?;
        thread::sleep(Duration::from_millis(500));
        post_window_key(hwnd, VK_O as u8)?;
        thread::sleep(Duration::from_secs(4));
        Ok(())
    }

    fn burst_window_key(hwnd: HWND, key: u8) -> Result<(), String> {
        for _ in 0..10 {
            post_window_key(hwnd, key)?;
            thread::sleep(Duration::from_millis(100));
        }
        Ok(())
    }

    fn post_window_key(hwnd: HWND, key: u8) -> Result<(), String> {
        for (message, key, lparam) in background_key_messages(key) {
            post_window_message(hwnd, message, key, lparam)?;
        }
        Ok(())
    }

    fn post_alt_window_key(hwnd: HWND, key: u8) -> Result<(), String> {
        for (message, key, lparam) in background_alt_key_messages(key) {
            post_window_message(hwnd, message, key, lparam)?;
        }
        Ok(())
    }

    pub(super) fn background_key_messages(key: u8) -> [(u32, u8, LPARAM); 2] {
        [
            (WM_KEYDOWN, key, 1),
            (WM_KEYUP, key, 0xc000_0001u32 as LPARAM),
        ]
    }

    pub(super) fn background_alt_key_messages(key: u8) -> [(u32, u8, LPARAM); 4] {
        [
            (WM_SYSKEYDOWN, VK_MENU as u8, 0x2000_0001),
            (WM_SYSKEYDOWN, key, 0x2000_0001),
            (WM_SYSKEYUP, key, 0xe000_0001u32 as LPARAM),
            (WM_SYSKEYUP, VK_MENU as u8, 0xc000_0001u32 as LPARAM),
        ]
    }

    fn post_window_message(
        hwnd: HWND,
        message: u32,
        key: u8,
        lparam: LPARAM,
    ) -> Result<(), String> {
        let posted = unsafe { PostMessageW(hwnd, message, key as usize, lparam) };
        if posted == 0 {
            Err(last_error("failed to post background StarCraft key"))
        } else {
            Ok(())
        }
    }

    fn wait_for_window(pid: u32, child: &OwnedChild, timeout: Duration) -> Result<HWND, String> {
        let started = Instant::now();
        loop {
            let mut context = WindowSearch {
                pid,
                hwnd: std::ptr::null_mut(),
            };
            unsafe {
                EnumWindows(
                    Some(enum_window),
                    (&mut context as *mut WindowSearch) as LPARAM,
                );
            }
            if !context.hwnd.is_null() {
                return Ok(context.hwnd);
            }
            if child.has_exited()? {
                return Err("StarCraft exited before creating its main window".to_string());
            }
            if started.elapsed() >= timeout {
                return Err("StarCraft did not create a visible window within 20s".to_string());
            }
            thread::sleep(Duration::from_millis(100));
        }
    }

    struct WindowSearch {
        pid: u32,
        hwnd: HWND,
    }

    unsafe extern "system" fn enum_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let context = &mut *(lparam as *mut WindowSearch);
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == context.pid && IsWindowVisible(hwnd) != 0 {
            context.hwnd = hwnd;
            return 0;
        }
        1
    }

    fn starcraft_running() -> Result<bool, String> {
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(last_error("process snapshot failed"));
        }
        let _guard = HandleGuard(snapshot);
        let mut entry: PROCESSENTRY32W = unsafe { zeroed() };
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = unsafe { Process32FirstW(snapshot, &mut entry) };
        while ok != 0 {
            let end = entry
                .szExeFile
                .iter()
                .position(|value| *value == 0)
                .unwrap_or(entry.szExeFile.len());
            if String::from_utf16_lossy(&entry.szExeFile[..end])
                .eq_ignore_ascii_case("StarCraft.exe")
            {
                return Ok(true);
            }
            ok = unsafe { Process32NextW(snapshot, &mut entry) };
        }
        Ok(false)
    }

    pub(super) fn open_process(pid: u32) -> Result<HandleGuard, String> {
        let handle = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid) };
        if handle.is_null() {
            Err(last_error("OpenProcess(PROCESS_VM_READ) failed"))
        } else {
            Ok(HandleGuard(handle))
        }
    }

    #[cfg(test)]
    pub(super) fn find_active_buffer(
        handle: HANDLE,
        marker: &[u8; 32],
    ) -> Result<Option<usize>, String> {
        inspect_trace_buffers(handle, marker).map(|(active, _)| active)
    }

    fn inspect_trace_buffers(
        handle: HANDLE,
        marker: &[u8; 32],
    ) -> Result<(Option<usize>, usize), String> {
        let matches = scan_marker(handle, marker)?;
        let match_count = matches.len();
        let mut active = Vec::new();
        for address in matches {
            let bytes = match read_memory(handle, address, TRACE_HEADER_BYTES) {
                Ok(bytes) => bytes,
                Err(_) => continue,
            };
            let Ok(header) = parse_header(&bytes, marker) else {
                continue;
            };
            if header.write_sequence != 0 || header.heartbeat_tick != 0 {
                active.push(address);
            }
        }
        match active.as_slice() {
            [] => Ok((None, match_count)),
            [address] => Ok((Some(*address), match_count)),
            _ => Err("multiple active trace buffers matched one run marker".to_string()),
        }
    }

    fn scan_marker(handle: HANDLE, marker: &[u8; 32]) -> Result<Vec<usize>, String> {
        scan_pattern(handle, marker)
    }

    fn scan_pattern(handle: HANDLE, needle: &[u8]) -> Result<Vec<usize>, String> {
        if needle.is_empty() || needle.len() > 64 {
            return Err("process scan pattern must contain 1 to 64 bytes".to_string());
        }
        let mut matches = Vec::new();
        let mut address = 0usize;
        let mut info: MEMORY_BASIC_INFORMATION = unsafe { zeroed() };
        let mut buffer = vec![0u8; MEMORY_CHUNK_BYTES];
        let mut boundary = [0u8; 128];
        while address < PROCESS_SCAN_MAX_X86 {
            let queried = unsafe {
                VirtualQueryEx(
                    handle,
                    address as *const _,
                    &mut info,
                    size_of::<MEMORY_BASIC_INFORMATION>(),
                )
            };
            if queried == 0 {
                address = address.saturating_add(0x1000);
                continue;
            }
            let base = info.BaseAddress as usize;
            let size = info.RegionSize;
            if info.State == MEM_COMMIT
                && info.Protect & PAGE_GUARD == 0
                && info.Protect & PAGE_NOACCESS == 0
            {
                let mut offset = 0usize;
                let mut carry_len = 0usize;
                while offset < size {
                    let wanted = buffer.len().min(size - offset);
                    let mut read = 0usize;
                    let ok = unsafe {
                        ReadProcessMemory(
                            handle,
                            (base + offset) as *const _,
                            buffer.as_mut_ptr().cast(),
                            wanted,
                            &mut read,
                        )
                    };
                    if ok != 0 && read != 0 {
                        if carry_len != 0 {
                            let boundary_read = read.min(needle.len() - 1);
                            boundary[carry_len..carry_len + boundary_read]
                                .copy_from_slice(&buffer[..boundary_read]);
                            if let Some(position) =
                                find_bytes(&boundary[..carry_len + boundary_read], needle)
                            {
                                matches.push(base + offset - carry_len + position);
                            }
                        }
                        let mut search = 0usize;
                        while search + needle.len() <= read {
                            let Some(position) = find_bytes(&buffer[search..read], needle) else {
                                break;
                            };
                            let found = search + position;
                            matches.push(base + offset + found);
                            if matches.len() > MAX_MARKER_MATCHES {
                                return Err("process pattern matched too many regions".to_string());
                            }
                            search = found + 1;
                        }
                        carry_len = (needle.len() - 1).min(read);
                        boundary[..carry_len].copy_from_slice(&buffer[read - carry_len..read]);
                    } else {
                        carry_len = 0;
                    }
                    offset = offset.saturating_add(wanted);
                }
            }
            let next = base.saturating_add(size.max(0x1000));
            address = if next > address {
                next
            } else {
                address.saturating_add(0x1000)
            };
        }
        matches.sort_unstable();
        matches.dedup();
        Ok(matches)
    }

    fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        if needle.is_empty() {
            return Some(0);
        }
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }

    pub(super) fn read_memory(
        handle: HANDLE,
        address: usize,
        length: usize,
    ) -> Result<Vec<u8>, String> {
        let mut bytes = vec![0u8; length];
        let mut read = 0usize;
        let ok = unsafe {
            ReadProcessMemory(
                handle,
                address as *const _,
                bytes.as_mut_ptr().cast(),
                length,
                &mut read,
            )
        };
        if ok == 0 || read != length {
            Err(last_error("ReadProcessMemory failed"))
        } else {
            Ok(bytes)
        }
    }

    fn last_error(message: &str) -> String {
        format!("{message} (Windows error {})", unsafe { GetLastError() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> TraceTestInput {
        TraceTestInput {
            name: "counter smoke".to_string(),
            code: "function eudAgentTestSetup() { eudAgentPass(2); }\nfunction eudAgentTestStep(tick) {}"
                .to_string(),
            symbols: vec![TraceSymbol {
                event_id: 2,
                name: "counter.ready".to_string(),
                source: Some("main.eps".to_string()),
                line: Some(12),
            }],
            timeout_ms: 5_000,
        }
    }

    fn persistent_snapshot() -> EpsSnapshot {
        EpsSnapshot {
            project: "Demo".to_string(),
            identity: "C:/projects/demo".to_string(),
            files: vec![
                EpsSnapshotFile {
                    path: "tests/wave.tests".to_string(),
                    ftype: "CUIEps".to_string(),
                    content: Some(input().code),
                },
                EpsSnapshotFile {
                    path: "tests/nested/reward.tests.eps".to_string(),
                    ftype: "RawText".to_string(),
                    content: Some(
                        "function eudAgentTestSetup() {}\nfunction eudAgentTestStep(tick) { eudAgentPass(3); }"
                            .to_string(),
                    ),
                },
                EpsSnapshotFile {
                    path: "main".to_string(),
                    ftype: "CUIEps".to_string(),
                    content: Some("function onPluginStart() {}".to_string()),
                },
            ],
        }
    }

    fn marker() -> [u8; 32] {
        marker_for_run("0123456789abcdef0123456789abcdef").unwrap()
    }

    fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn terminal_buffer(
        state: u32,
        dropped: u32,
        failure_event: u32,
        records: &[(u32, u32, u32, u32, [u32; 4])],
    ) -> Vec<u8> {
        let marker = marker();
        let mut bytes = vec![0u8; TRACE_BUFFER_BYTES];
        bytes[..32].copy_from_slice(&marker);
        put_u32(&mut bytes, 32, TRACE_VERSION);
        put_u32(&mut bytes, 36, TRACE_CAPACITY);
        put_u32(&mut bytes, 40, TRACE_RECORD_DWORDS);
        put_u32(&mut bytes, 44, records.last().map_or(0, |record| record.0));
        put_u32(&mut bytes, 48, dropped);
        put_u32(&mut bytes, 52, state);
        put_u32(&mut bytes, 56, failure_event);
        put_u32(&mut bytes, 60, 20);
        for (sequence, tick, event_id, severity, values) in records {
            let offset =
                TRACE_HEADER_BYTES + (*sequence % TRACE_CAPACITY) as usize * TRACE_RECORD_BYTES;
            put_u32(&mut bytes, offset, *sequence);
            put_u32(&mut bytes, offset + 4, *tick);
            put_u32(&mut bytes, offset + 8, *event_id);
            put_u32(&mut bytes, offset + 12, *severity);
            for (index, value) in values.iter().enumerate() {
                put_u32(&mut bytes, offset + 16 + index * 4, *value);
            }
        }
        bytes
    }

    #[test]
    fn validation_requires_callbacks_bounds_and_unique_symbols() {
        let mut value = input();
        assert!(validate_input(&value).is_ok());

        value.code = "function eudAgentTestSetup() {}".to_string();
        assert!(validate_input(&value)
            .unwrap_err()
            .contains("eudAgentTestStep"));

        value = input();
        value.symbols.push(value.symbols[0].clone());
        assert!(validate_input(&value).unwrap_err().contains("duplicate"));

        value = input();
        value.timeout_ms = MAX_TEST_TIMEOUT_MS + 1;
        assert!(validate_input(&value).unwrap_err().contains("timeoutMs"));
    }

    #[test]
    #[cfg(windows)]
    fn background_key_messages_target_only_the_owned_window_queue() {
        assert_eq!(
            windows::background_key_messages(b'G'),
            [(0x0100, b'G', 1), (0x0101, b'G', 0xc000_0001u32 as isize),]
        );
        assert_eq!(
            windows::background_alt_key_messages(b'O'),
            [
                (0x0104, 0x12, 0x2000_0001),
                (0x0104, b'O', 0x2000_0001),
                (0x0105, b'O', 0xe000_0001u32 as isize),
                (0x0105, 0x12, 0xc000_0001u32 as isize),
            ]
        );
    }

    #[test]
    #[cfg(windows)]
    fn embedded_input_isolation_injector_is_x86_and_names_bounded_patches() {
        let pe = u32::from_le_bytes(TRACE_INJECTOR_EXE[0x3c..0x40].try_into().unwrap()) as usize;
        assert_eq!(
            u16::from_le_bytes(TRACE_INJECTOR_EXE[pe + 4..pe + 6].try_into().unwrap()),
            0x014c
        );
        for name in [b"SetForegroundWindow\0".as_slice(), b"SetCursorPos\0"] {
            assert!(TRACE_INJECTOR_EXE
                .windows(name.len())
                .any(|window| window == name));
        }
    }

    #[test]
    fn persistent_selection_discovers_logical_test_paths_and_preserves_requested_order() {
        let all = select_persistent_tests(
            &persistent_snapshot(),
            &TraceSuiteInput {
                tests: None,
                timeout_ms: 5_000,
            },
        )
        .unwrap();
        assert_eq!(
            all.discovered,
            vec![
                "tests/nested/reward.tests.eps".to_string(),
                "tests/wave.tests.eps".to_string(),
            ]
        );
        assert_eq!(
            all.tests
                .iter()
                .map(|test| test.path.as_str())
                .collect::<Vec<_>>(),
            vec!["tests/nested/reward.tests.eps", "tests/wave.tests.eps",]
        );

        let selected = select_persistent_tests(
            &persistent_snapshot(),
            &TraceSuiteInput {
                tests: Some(vec![
                    "tests/wave.tests.eps".to_string(),
                    "tests/nested/reward.tests.eps".to_string(),
                ]),
                timeout_ms: 5_000,
            },
        )
        .unwrap();
        assert_eq!(
            selected
                .tests
                .iter()
                .map(|test| test.path.as_str())
                .collect::<Vec<_>>(),
            vec!["tests/wave.tests.eps", "tests/nested/reward.tests.eps",]
        );
    }

    #[test]
    fn persistent_selection_rejects_missing_outside_and_duplicate_paths() {
        for tests in [
            vec!["tests/missing.tests.eps".to_string()],
            vec!["feature.tests.eps".to_string()],
            vec![
                "tests/wave.tests.eps".to_string(),
                "TESTS/WAVE.TESTS.EPS".to_string(),
            ],
        ] {
            assert!(select_persistent_tests(
                &persistent_snapshot(),
                &TraceSuiteInput {
                    tests: Some(tests),
                    timeout_ms: 5_000,
                },
            )
            .is_err());
        }

        let mut unreadable = persistent_snapshot();
        unreadable.files[0].content = None;
        let selected = select_persistent_tests(
            &unreadable,
            &TraceSuiteInput {
                tests: Some(vec!["tests/wave.tests.eps".to_string()]),
                timeout_ms: 5_000,
            },
        )
        .unwrap();
        assert_eq!(selected.tests.len(), 1);
        assert!(selected.tests[0].code.is_none());
    }

    #[test]
    fn suite_records_invalid_and_unreadable_tests_without_launching_a_client() {
        let base =
            std::env::temp_dir().join(format!("eud-agent-trace-suite-{}", uuid::Uuid::new_v4()));
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        let path = "tests/invalid.tests.eps".to_string();
        let result = run_suite(
            &dirs,
            Path::new("missing.eds"),
            Path::new("missing-euddraft.exe"),
            Path::new("missing-starcraft.exe"),
            PersistentTraceSelection {
                discovered: vec![path.clone()],
                tests: vec![PersistentTraceTest {
                    path: path.clone(),
                    code: Some("function eudAgentTestSetup() {}".to_string()),
                }],
            },
            5_000,
            |_| panic!("an invalid persistent test must not reach a runtime phase"),
        )
        .unwrap();

        assert_eq!(result.status, TraceTestStatus::Failed);
        assert_eq!(result.failed, 1);
        assert_eq!(result.selected, vec![path.clone()]);
        assert_eq!(result.tests[0].reason, "invalid_test_definition");
        assert!(Path::new(&result.log_dir).join("suite.json").is_file());
        let unreadable = run_suite(
            &dirs,
            Path::new(""),
            Path::new(""),
            Path::new(""),
            PersistentTraceSelection {
                discovered: vec![path.clone()],
                tests: vec![PersistentTraceTest { path, code: None }],
            },
            5_000,
            |_| panic!("an unreadable persistent test must not reach a runtime phase"),
        )
        .unwrap();
        assert_eq!(unreadable.status, TraceTestStatus::Inconclusive);
        assert_eq!(unreadable.inconclusive, 1);
        assert_eq!(unreadable.tests[0].reason, "unreadable_test_source");
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn empty_persistent_suite_is_inconclusive_without_runtime_dependencies() {
        let base = std::env::temp_dir().join(format!(
            "eud-agent-empty-trace-suite-{}",
            uuid::Uuid::new_v4()
        ));
        let dirs = DataDirs::from_bases(&base.join("roaming"), &base.join("local"));
        let result = run_suite(
            &dirs,
            Path::new(""),
            Path::new(""),
            Path::new(""),
            PersistentTraceSelection {
                discovered: Vec::new(),
                tests: Vec::new(),
            },
            5_000,
            |_| panic!("an empty persistent suite must not reach a runtime phase"),
        )
        .unwrap();

        assert_eq!(result.status, TraceTestStatus::Inconclusive);
        assert_eq!(result.reason, "no_persistent_tests_found");
        assert!(result.selected.is_empty());
        assert!(result.tests.is_empty());
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn isolated_eds_rewrite_changes_only_main_paths_and_appends_plugin() {
        let original = "[other]\ninput: keep.scx\n[main]\ninput: ..\\map.scx\noutput = ..\\out.scx\n[plugin]\nvalue: 1\n";
        let rewritten = rewrite_eds(
            original,
            Path::new(r"C:\trace\input.scx"),
            Path::new(r"C:\trace\output.scx"),
            Path::new(r"C:\trace\test.eps"),
        )
        .unwrap();

        assert!(rewritten.contains("[other]\r\ninput: keep.scx"));
        assert!(rewritten.contains(r"input: C:\trace\input.scx"));
        assert!(rewritten.contains(r"output: C:\trace\output.scx"));
        assert!(rewritten.ends_with("[C:\\trace\\test.eps]\r\n"));
        assert!(original.contains("..\\map.scx"));
    }

    #[test]
    fn generated_plugin_embeds_unique_protocol_and_public_test_api() {
        let generated = generate_plugin(&input(), &marker());
        assert!(generated.contains("4555444147454e545452414345563121"));
        assert!(generated.contains("30313233343536373839616263646566"));
        assert!(generated.contains("function eudAgentTrace("));
        assert!(generated.contains("function eudAgentAssertEq("));
        assert!(generated.contains("function eudAgentFail("));
        assert!(generated.contains("function eudAgentPass("));
        assert!(generated.contains("eudAgentTestSetup();"));
        assert!(generated.contains("eudAgentTestStep(eudAgentTraceTick);"));
    }

    #[test]
    fn terminal_pass_decodes_ordered_symbolized_events() {
        let bytes = terminal_buffer(
            1,
            0,
            0,
            &[
                (1, 0, INTERNAL_BEGIN_EVENT, 0, [0; 4]),
                (2, 8, 2, 1, [42, 42, 0, 0]),
            ],
        );
        let outcome = decode_snapshot(&bytes, &marker(), &input().symbols).unwrap();

        assert_eq!(outcome.status, TraceTestStatus::Passed);
        assert_eq!(outcome.reason, "test_passed");
        assert_eq!(outcome.events.len(), 2);
        assert_eq!(outcome.events[0].event_name.as_deref(), Some("trace.begin"));
        assert_eq!(
            outcome.events[1].event_name.as_deref(),
            Some("counter.ready")
        );
        assert_eq!(outcome.events[1].source.as_deref(), Some("main.eps"));
        assert_eq!(outcome.events[1].line, Some(12));
        assert_eq!(outcome.events[1].values, [42, 42, 0, 0]);
    }

    #[test]
    fn failed_overflowed_and_torn_buffers_fail_closed() {
        let failed = terminal_buffer(2, 0, 9, &[(1, 3, 9, 2, [1, 2, 0, 0])]);
        let outcome = decode_snapshot(&failed, &marker(), &[]).unwrap();
        assert_eq!(outcome.status, TraceTestStatus::Failed);
        assert_eq!(outcome.reason, "assertion_failed:9");

        let overflowed = terminal_buffer(1, 1, 0, &[(1, 3, 2, 1, [0; 4])]);
        let outcome = decode_snapshot(&overflowed, &marker(), &[]).unwrap();
        assert_eq!(outcome.status, TraceTestStatus::Inconclusive);
        assert_eq!(outcome.reason, "trace_buffer_overflow");

        let mut torn = terminal_buffer(1, 0, 0, &[(1, 3, 2, 1, [0; 4])]);
        put_u32(&mut torn, TRACE_HEADER_BYTES + TRACE_RECORD_BYTES, 99);
        assert!(decode_snapshot(&torn, &marker(), &[])
            .unwrap_err()
            .contains("torn or overwritten"));
    }

    #[test]
    #[ignore = "requires a generated live-editor EDS, euddraft, and 32-bit StarCraft"]
    fn live_single_test_build_launch_trace_and_cleanup() {
        let dirs = DataDirs::from_bases(
            Path::new(&std::env::var("APPDATA").unwrap()),
            Path::new(&std::env::var("LOCALAPPDATA").unwrap()),
        );
        dirs.ensure_dirs().unwrap();
        let source_eds = PathBuf::from(std::env::var("EUD_TRACE_EDS").unwrap());
        let euddraft = PathBuf::from(std::env::var("EUD_TRACE_EUDDRAFT").unwrap());
        let starcraft = PathBuf::from(std::env::var("EUD_TRACE_STARCRAFT").unwrap());
        let result = run(
            &dirs,
            &source_eds,
            &euddraft,
            &starcraft,
            TraceTestInput {
                name: "live protocol smoke".to_string(),
                code: "function eudAgentTestSetup() {}\nfunction eudAgentTestStep(tick) {\n    if (tick == 8) {\n        if (eudAgentAssertEq(100, 42, 42)) {\n            eudAgentPass(101);\n        }\n    }\n}"
                    .to_string(),
                symbols: vec![
                    TraceSymbol {
                        event_id: 100,
                        name: "smoke.assert".to_string(),
                        source: Some("eud_agent_trace_test.eps".to_string()),
                        line: Some(4),
                    },
                    TraceSymbol {
                        event_id: 101,
                        name: "smoke.complete".to_string(),
                        source: Some("eud_agent_trace_test.eps".to_string()),
                        line: Some(5),
                    },
                ],
                timeout_ms: 60_000,
            },
            |phase| eprintln!("trace smoke phase={}", phase.as_str()),
        )
        .unwrap();
        eprintln!("{}", serde_json::to_string_pretty(&result).unwrap());
        assert_eq!(result.status, TraceTestStatus::Passed, "{result:?}");
        assert!(result.source_map_unchanged);
        assert_eq!(result.events.len(), 3);
    }

    #[test]
    #[ignore = "requires a generated live-editor EDS, euddraft, and 32-bit StarCraft"]
    fn live_persistent_suite_build_launch_trace_and_cleanup() {
        let dirs = DataDirs::from_bases(
            Path::new(&std::env::var("APPDATA").unwrap()),
            Path::new(&std::env::var("LOCALAPPDATA").unwrap()),
        );
        dirs.ensure_dirs().unwrap();
        let source_eds = PathBuf::from(std::env::var("EUD_TRACE_EDS").unwrap());
        let euddraft = PathBuf::from(std::env::var("EUD_TRACE_EUDDRAFT").unwrap());
        let starcraft = PathBuf::from(std::env::var("EUD_TRACE_STARCRAFT").unwrap());
        let snapshot = EpsSnapshot {
            project: "live-suite".to_string(),
            identity: "live-suite".to_string(),
            files: vec![EpsSnapshotFile {
                path: "tests/protocol.tests".to_string(),
                ftype: "CUIEps".to_string(),
                content: Some(
                    "function eudAgentTestSetup() {}\nfunction eudAgentTestStep(tick) {\n    if (tick == 8) {\n        if (eudAgentAssertEq(100, 42, 42)) {\n            eudAgentPass(101);\n        }\n    }\n}"
                        .to_string(),
                ),
            }],
        };
        let suite_input = TraceSuiteInput {
            tests: None,
            timeout_ms: 60_000,
        };
        let selection = select_persistent_tests(&snapshot, &suite_input).unwrap();
        let result = run_suite(
            &dirs,
            &source_eds,
            &euddraft,
            &starcraft,
            selection,
            suite_input.timeout_ms,
            |phase| eprintln!("persistent trace smoke phase={}", phase.as_str()),
        )
        .unwrap();

        eprintln!("{}", serde_json::to_string_pretty(&result).unwrap());
        assert_eq!(result.status, TraceTestStatus::Passed, "{result:?}");
        assert_eq!(result.passed, 1);
        assert_eq!(result.tests[0].path, "tests/protocol.tests.eps");
        assert_eq!(result.tests[0].source_map_unchanged, Some(true));
        assert!(result.tests[0].summary.contains("3 event(s)"));
        assert!(Path::new(&result.log_dir).join("suite.json").is_file());
    }

    #[test]
    #[ignore = "requires EUD_TRACE_PID pointing at a running instrumented StarCraft"]
    #[cfg(windows)]
    fn live_process_scanner_finds_and_decodes_active_ring_buffer() {
        let pid = std::env::var("EUD_TRACE_PID")
            .unwrap()
            .parse::<u32>()
            .unwrap();
        let process = windows::open_process(pid).unwrap();
        let marker = marker();
        let address = windows::find_active_buffer(process.0, &marker)
            .unwrap()
            .expect("active trace marker");
        let snapshot = windows::read_memory(process.0, address, TRACE_BUFFER_BYTES).unwrap();
        let outcome = decode_snapshot(&snapshot, &marker, &[]).unwrap();
        assert_eq!(outcome.status, TraceTestStatus::Passed);
    }
}
