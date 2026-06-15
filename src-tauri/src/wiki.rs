//! Per-project dat-edit WIKI: a last-value ledger of dat-editor edits the agent
//! applied (via GETDAT/SETDAT) and the user APPROVED.
//!
//! Purpose: later work (including trigger edits) can read the LAST applied
//! unit/weapon/flingy/etc stat values so the agent does not confuse them. SCOPE is
//! the dat-editor tables ONLY (dat/xdat/tbl/req/btn). The original `.scx` map
//! (locations/players via mapsafe/isom) is OUT OF SCOPE — never recorded here.
//!
//! On-disk: `%appdata%\eud-agent\memory\<sanitized-project>\wiki\ledger.json`, written
//! UTF-8 **without BOM** via the shared atomic temp+rename (rules.md). The store reuses
//! [`crate::memory::sanitize_project_name`]/[`crate::memory::write_atomic_bytes`] so the
//! ledger lands beside the project's memory dir with identical write semantics.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::memory::write_atomic_bytes;

const LEDGER_FILE: &str = "ledger.json";
const LEDGER_VERSION: u32 = 1;

/// Approximate token budget for the injected `[wiki facts]` body. Below this the
/// full ledger is injected verbatim; above it, items are SELECTED by relevance up
/// to the budget so a large ledger never blows up per-turn tokens.
///
/// Basis: tokens are approximated as `chars / CHARS_PER_TOKEN` (4), the common
/// rule-of-thumb for English/code (~4 chars per BPE token). It is intentionally
/// crude but deterministic (no tokenizer dependency, no `Date::now`) and trivially
/// tunable here. ~2500 tokens ≈ ~10_000 rendered chars.
pub const WIKI_PROMPT_TOKEN_BUDGET: usize = 2_500;

/// Chars-per-token divisor for the cheap [`approx_tokens`] estimate (see
/// [`WIKI_PROMPT_TOKEN_BUDGET`] for the rationale).
const CHARS_PER_TOKEN: usize = 4;

/// Cheap, deterministic token estimate: rendered char count / [`CHARS_PER_TOKEN`].
fn approx_tokens(text: &str) -> usize {
    text.chars().count() / CHARS_PER_TOKEN
}

/// One-line label prepended to the rendered `[wiki facts]` section: these are the
/// agent's last APPLIED values, which can diverge from the live map after a manual edit.
pub const MAY_DIFFER_LABEL: &str = "NOTE: agent-applied last values; may differ from the live map.";

/// One ledger entry: the last applied + accepted value for a dat-editor property.
///
/// Key (in [`Ledger::entries`]) is `"{table}:{dat}:{objId}:{property}"`. `value` is
/// the LAST APPLIED new value; an upsert overwrites by key (last-value semantics).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LedgerEntry {
    /// Dat family: `dat` | `xdat` | `tbl` | `req` | `btn`.
    pub table: String,
    /// Dat-family name (e.g. `units`, `weapons`); empty for tbl/btn.
    pub dat: String,
    /// Numeric object index.
    #[serde(rename = "objId")]
    pub obj_id: u32,
    /// Property name as used on the bridge wire (e.g. `HP`, `Armor`).
    pub property: String,
    /// The last applied new value (number or string).
    pub value: Value,
    /// For `table=dat` & `dat=units`, the unit name; else `None`.
    #[serde(rename = "itemName", skip_serializing_if = "Option::is_none", default)]
    pub item_name: Option<String>,
    /// Unix seconds taken from the originating journal entry (never `Date::now`).
    #[serde(rename = "appliedAt")]
    pub applied_at: i64,
    /// `false` when written by the accept-hook, `true` when corrected via `wiki_save`.
    #[serde(rename = "editedByUser")]
    pub edited_by_user: bool,
}

impl LedgerEntry {
    /// The ledger key `"{table}:{dat}:{objId}:{property}"` this entry upserts under.
    pub fn key(&self) -> String {
        ledger_key(&self.table, &self.dat, self.obj_id, &self.property)
    }
}

/// The ledger key for a (table, dat, objId, property) tuple.
pub fn ledger_key(table: &str, dat: &str, obj_id: u32, property: &str) -> String {
    format!("{table}:{dat}:{obj_id}:{property}")
}

/// The on-disk / IPC ledger document: `{ version, entries: { key: entry } }`.
///
/// `BTreeMap` keeps keys ordered so the rendered section and serialized JSON are
/// deterministic (stable diffs, stable test output).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ledger {
    pub version: u32,
    pub entries: BTreeMap<String, LedgerEntry>,
}

impl Default for Ledger {
    fn default() -> Self {
        Self {
            version: LEDGER_VERSION,
            entries: BTreeMap::new(),
        }
    }
}

impl Ledger {
    /// True when no entries are recorded (drives the skip-when-empty prompt section).
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Upsert one entry by its key (last-value overwrite).
    pub fn upsert(&mut self, entry: LedgerEntry) {
        self.entries.insert(entry.key(), entry);
    }

    /// Render the `[wiki facts]` prompt section for `query`, or `None` when empty.
    ///
    /// Grouped table/dat -> item (itemName or `#objId`) -> `property = value`, with the
    /// may-differ label on the first line. Skipped (returns `None`) when there are no
    /// entries, mirroring [`crate::memory`]'s optional section.
    ///
    /// Adaptive: when the FULL rendered body fits [`WIKI_PROMPT_TOKEN_BUDGET`] it is
    /// returned verbatim (small maps pay nothing and lose nothing). Over budget, items
    /// are SELECTED query-first up to the budget — see [`Self::select_items`]. No
    /// "omitted/N more" note is appended (the body is for the LLM, where it is noise).
    pub fn render_section(&self, query: &str) -> Option<String> {
        if self.is_empty() {
            return None;
        }

        // The full body is today's output; small ledgers return it unchanged.
        let full = render_items(self.entries.values());
        if approx_tokens(&full) <= WIKI_PROMPT_TOKEN_BUDGET {
            return Some(full);
        }

        // Over budget: select whole items (a unit's properties stay together)
        // query-first, up to the budget, then render with the same grouping.
        let selected = self.select_items(query, WIKI_PROMPT_TOKEN_BUDGET);
        Some(render_items(selected))
    }

    /// Select whole items (table:dat:objId groups) up to `budget` approx tokens,
    /// ordered by relevance to `query` then recency.
    ///
    /// score = number of an item's keywords (dat, itemName, `#objId`, bare objId,
    /// each property name) that appear (case-insensitive substring) in `query`.
    /// Order: score DESC, then `appliedAt` DESC (newest first), then a stable key
    /// for determinism. Whole items are included until the next would exceed the
    /// budget. When NO item scores > 0, this naturally degrades to `appliedAt` DESC.
    fn select_items(&self, query: &str, budget: usize) -> Vec<&LedgerEntry> {
        let query_lc = query.to_lowercase();

        // Group entries by item (table, dat, objId), preserving the per-item set of
        // properties. BTreeMap key gives the stable tie-break ordering.
        let mut items: BTreeMap<(String, String, u32), Vec<&LedgerEntry>> = BTreeMap::new();
        for entry in self.entries.values() {
            items
                .entry((entry.table.clone(), entry.dat.clone(), entry.obj_id))
                .or_default()
                .push(entry);
        }

        // Rank each item: score DESC, then appliedAt DESC, then stable key ASC.
        let mut ranked: Vec<RankedItem> = items
            .into_iter()
            .map(|(key, props)| {
                let score = item_score(&props, &query_lc);
                let applied_at = props.iter().map(|e| e.applied_at).max().unwrap_or_default();
                RankedItem {
                    key,
                    props,
                    score,
                    applied_at,
                }
            })
            .collect();
        ranked.sort_by(|a, b| {
            b.score
                .cmp(&a.score) // score DESC
                .then(b.applied_at.cmp(&a.applied_at)) // appliedAt DESC
                .then(a.key.cmp(&b.key)) // stable key ASC
        });

        // Include whole items until adding the next would exceed the budget.
        let mut chosen: Vec<&LedgerEntry> = Vec::new();
        for item in ranked {
            let mut candidate = chosen.clone();
            candidate.extend(item.props.iter().copied());
            if approx_tokens(&render_items(candidate.iter().copied())) > budget {
                break;
            }
            chosen = candidate;
        }
        chosen
    }
}

/// One item (table:dat:objId group) ranked for over-budget selection in
/// [`Ledger::select_items`]: its grouping key, its properties, and the relevance
/// `score` + `applied_at` recency the ordering sorts on.
struct RankedItem<'a> {
    key: (String, String, u32),
    props: Vec<&'a LedgerEntry>,
    score: usize,
    applied_at: i64,
}

/// The keywords an item exposes for query matching: dat name, itemName (if any),
/// `#objId`, bare objId, and each property name — all lowercased.
fn item_keywords(props: &[&LedgerEntry]) -> Vec<String> {
    let mut keywords = Vec::new();
    if let Some(first) = props.first() {
        if !first.dat.trim().is_empty() {
            keywords.push(first.dat.to_lowercase());
        }
        keywords.push(format!("#{}", first.obj_id));
        keywords.push(first.obj_id.to_string());
        if let Some(name) = first
            .item_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            keywords.push(name.to_lowercase());
        }
    }
    for entry in props {
        keywords.push(entry.property.to_lowercase());
    }
    keywords
}

/// Count how many of an item's keywords appear as a substring of the lowercased query.
fn item_score(props: &[&LedgerEntry], query_lc: &str) -> usize {
    item_keywords(props)
        .iter()
        .filter(|kw| query_lc.contains(kw.as_str()))
        .count()
}

/// Render the given entries as the `[wiki facts]` body, grouped table/dat -> item
/// label -> `property = value`, with the may-differ label on the first line. This is
/// the single rendering path shared by the full and selected branches of
/// [`Ledger::render_section`]; BTreeMap iteration keeps output order stable.
fn render_items<'a, I>(entries: I) -> String
where
    I: IntoIterator<Item = &'a LedgerEntry>,
{
    let mut groups: RenderGroups = BTreeMap::new();
    for entry in entries {
        let item_label = entry
            .item_name
            .clone()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| format!("#{}", entry.obj_id));
        groups
            .entry((entry.table.clone(), entry.dat.clone()))
            .or_default()
            .entry((entry.obj_id, item_label))
            .or_default()
            .push(format!(
                "{} = {}",
                entry.property,
                render_value(&entry.value)
            ));
    }

    let mut lines = vec!["[wiki facts]".to_string(), MAY_DIFFER_LABEL.to_string()];
    for ((table, dat), items) in &groups {
        let header = if dat.is_empty() {
            format!("## {table}")
        } else {
            format!("## {table} {dat}")
        };
        lines.push(header);
        for ((_obj_id, item_label), props) in items {
            lines.push(format!("- {item_label}"));
            for prop in props {
                lines.push(format!("  - {prop}"));
            }
        }
    }

    lines.join("\n")
}

/// `(table, dat)` -> `(objId, itemLabel)` -> ordered `"property = value"` lines, the
/// grouping shape [`Ledger::render_section`] folds the ledger into for stable output.
type RenderGroups = BTreeMap<(String, String), BTreeMap<(u32, String), Vec<String>>>;

/// Render a JSON value for the prompt/UI: strings unquoted, everything else compact.
fn render_value(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// File-backed dat-edit ledger for a single project's wiki dir.
///
/// Construct with [`WikiStore::load`]; an absent/corrupt file yields an empty ledger.
/// A `None` wiki dir (empty project name) makes every op a no-op.
#[derive(Debug, Clone)]
pub struct WikiStore {
    wiki_dir: Option<PathBuf>,
    ledger: Ledger,
}

impl WikiStore {
    /// Load the ledger from `<wiki_dir>/ledger.json`.
    ///
    /// A missing/unreadable/malformed file yields an empty ledger (never errors). A
    /// `None` `wiki_dir` (empty project name) yields a disabled, in-memory-only store.
    pub fn load(wiki_dir: Option<PathBuf>) -> Self {
        let ledger = wiki_dir.as_deref().map(read_ledger).unwrap_or_default();
        Self { wiki_dir, ledger }
    }

    /// True when a usable wiki dir backs this store (non-empty project name).
    pub fn enabled(&self) -> bool {
        self.wiki_dir.is_some()
    }

    /// Borrow the in-memory ledger (the source for IPC + prompt rendering).
    pub fn ledger(&self) -> &Ledger {
        &self.ledger
    }

    /// Upsert one entry in memory (last-value overwrite); does not persist.
    pub fn upsert(&mut self, entry: LedgerEntry) {
        self.ledger.upsert(entry);
    }

    /// Atomically persist the ledger as UTF-8 **without BOM** (temp + rename).
    ///
    /// No-op (returns `Ok`) when the store is disabled. Reuses the memory layer's
    /// [`write_atomic_bytes`] so the write semantics match exactly.
    pub fn save(&self) -> anyhow::Result<()> {
        let Some(dir) = self.wiki_dir.as_deref() else {
            return Ok(());
        };
        let bytes = serde_json::to_vec_pretty(&self.ledger)?;
        write_atomic_bytes(&dir.join(LEDGER_FILE), &bytes)?;
        Ok(())
    }

    /// Render the `[wiki facts]` prompt section for `query`, or `None` when
    /// empty/disabled. Query-aware: see [`Ledger::render_section`].
    pub fn render_section(&self, query: &str) -> Option<String> {
        self.ledger.render_section(query)
    }
}

/// Which properties of an accepted changeset are recorded to the ledger.
///
/// `All` records every dat property (accept-all); `Ids` records only the dat
/// properties whose changeset item id OR per-property journal entry id is listed
/// (per-property accept granularity — EUD changeset decision contract).
#[derive(Debug, Clone)]
pub enum AcceptedScope {
    All,
    Ids(Vec<String>),
}

impl AcceptedScope {
    fn includes(&self, item_id: &str, property_id: &str) -> bool {
        match self {
            Self::All => true,
            Self::Ids(ids) => ids.iter().any(|id| id == item_id || id == property_id),
        }
    }
}

/// Build the ledger entries for the ACCEPTED dat property changes of a changeset.
///
/// Only `JournalTarget::Dat`-derived items (`dat_ref.is_some()`) are recorded — file,
/// settings, plugin, and map items are skipped (wiki scope is dat-editor tables only).
/// Each accepted [`crate::journal::PropertyChange`] becomes one [`LedgerEntry`] with
/// `value = new`. `applied_at` is taken from the matching journal entry's `ts` (never
/// `Date::now`); `item_name` is `chk::unit_name(objId)` for `table=dat`/`dat=units`.
pub fn accepted_ledger_entries(
    changeset: &crate::journal::Changeset,
    journal: &crate::journal::Journal,
    scope: &AcceptedScope,
) -> Vec<LedgerEntry> {
    let mut entries = Vec::new();
    for item in &changeset.items {
        let Some(dat_ref) = &item.dat_ref else {
            continue; // not a dat-family edit — out of wiki scope.
        };
        let table = dat_table_slug(dat_ref.table).to_string();
        let is_units =
            matches!(dat_ref.table, crate::journal::DatTable::Dat) && dat_ref.dat == "units";

        for property in &item.properties {
            if !scope.includes(&item.id, &property.id) {
                continue;
            }
            // A reset-to-default records `after = Value::Null` (no meaningful last
            // applied value). Skip it: the contract says `value` is the last APPLIED
            // new value (number or string), and a null entry would fail the panel's
            // `isLedgerEntry` guard and silently drop the whole wiki push event.
            if property.new.is_null() {
                continue;
            }
            let applied_at = journal
                .entries
                .iter()
                .find(|entry| entry.id == property.id)
                .map(|entry| entry.ts as i64)
                .unwrap_or_default();
            let item_name = is_units
                .then(|| crate::chk::unit_name(u16::try_from(dat_ref.obj_id).unwrap_or(u16::MAX)));
            entries.push(LedgerEntry {
                table: table.clone(),
                dat: dat_ref.dat.clone(),
                obj_id: dat_ref.obj_id,
                property: property.property.clone(),
                value: property.new.clone(),
                item_name,
                applied_at,
                edited_by_user: false,
            });
        }
    }
    entries
}

/// Lowercase family slug used for the ledger `table` field and the rendered header.
fn dat_table_slug(table: crate::journal::DatTable) -> &'static str {
    match table {
        crate::journal::DatTable::Dat => "dat",
        crate::journal::DatTable::Xdat => "xdat",
        crate::journal::DatTable::Tbl => "tbl",
        crate::journal::DatTable::Req => "req",
        crate::journal::DatTable::Btn => "btn",
    }
}

fn read_ledger(dir: &Path) -> Ledger {
    let path = dir.join(LEDGER_FILE);
    let Ok(bytes) = fs::read(&path) else {
        return Ledger::default();
    };
    // `write_atomic_bytes` never writes a BOM, but strip one defensively so a
    // hand-edited file still parses (mirrors config.rs load_config).
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes);
    serde_json::from_slice(bytes).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("eud-agent-wiki-test-{tag}-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn entry(
        table: &str,
        dat: &str,
        obj_id: u32,
        property: &str,
        value: Value,
        item_name: Option<&str>,
    ) -> LedgerEntry {
        LedgerEntry {
            table: table.to_string(),
            dat: dat.to_string(),
            obj_id,
            property: property.to_string(),
            value,
            item_name: item_name.map(str::to_string),
            applied_at: 1_718_000_000,
            edited_by_user: false,
        }
    }

    #[test]
    fn upsert_overwrites_by_key_keeping_last_value() {
        let mut ledger = Ledger::default();
        ledger.upsert(entry(
            "dat",
            "units",
            0,
            "HP",
            json!(40),
            Some("Terran Marine"),
        ));
        ledger.upsert(entry(
            "dat",
            "units",
            0,
            "HP",
            json!(80),
            Some("Terran Marine"),
        ));
        // A different property on the same object is a distinct key.
        ledger.upsert(entry(
            "dat",
            "units",
            0,
            "Armor",
            json!(1),
            Some("Terran Marine"),
        ));

        assert_eq!(ledger.entries.len(), 2);
        assert_eq!(
            ledger.entries["dat:units:0:HP"].value,
            json!(80),
            "later upsert wins"
        );
        assert_eq!(ledger.entries["dat:units:0:Armor"].value, json!(1));
    }

    #[test]
    fn distinct_dat_files_and_tables_never_merge() {
        // units #5 and weapons #5 share an objId but are distinct keys.
        let a = entry("dat", "units", 5, "HP", json!(100), Some("Marine"));
        let b = entry("dat", "weapons", 5, "Damage", json!(6), None);
        assert_ne!(a.key(), b.key());
        assert_eq!(a.key(), "dat:units:5:HP");
        assert_eq!(b.key(), "dat:weapons:5:Damage");
    }

    #[test]
    fn render_section_groups_and_labels_units_by_name_others_by_objid() {
        let mut ledger = Ledger::default();
        ledger.upsert(entry(
            "dat",
            "units",
            0,
            "HP",
            json!(80),
            Some("Terran Marine"),
        ));
        ledger.upsert(entry(
            "dat",
            "units",
            0,
            "Armor",
            json!(1),
            Some("Terran Marine"),
        ));
        ledger.upsert(entry("dat", "weapons", 5, "Damage", json!(6), None));

        let section = ledger.render_section("").expect("non-empty ledger renders");
        assert!(section.starts_with("[wiki facts]\n"));
        assert!(section.contains(MAY_DIFFER_LABEL));
        assert!(section.contains("## dat units"));
        assert!(section.contains("- Terran Marine"));
        assert!(section.contains("  - HP = 80"));
        assert!(section.contains("  - Armor = 1"));
        assert!(section.contains("## dat weapons"));
        // No itemName -> labelled by #objId.
        assert!(section.contains("- #5"));
        assert!(section.contains("  - Damage = 6"));
    }

    #[test]
    fn render_value_unquotes_strings() {
        let mut ledger = Ledger::default();
        ledger.upsert(entry("tbl", "", 11, "String", json!("boss name"), None));
        let section = ledger.render_section("").unwrap();
        assert!(section.contains("## tbl"));
        assert!(
            section.contains("  - String = boss name"),
            "string values render unquoted: {section}"
        );
    }

    #[test]
    fn empty_ledger_renders_no_section() {
        assert_eq!(Ledger::default().render_section(""), None);
        let store = WikiStore::load(None);
        assert!(!store.enabled());
        assert_eq!(store.render_section(""), None);
    }

    /// Build a ledger whose full rendered body clearly EXCEEDS the token budget by
    /// adding many filler weapons items, plus a single named units item ("Zealot")
    /// the relevance tests can target. `applied_at` ascends with `i` so recency
    /// (appliedAt DESC) is well-defined and deterministic.
    fn over_budget_ledger() -> Ledger {
        let mut ledger = Ledger::default();
        // Each filler item is one weapons entry (~30 rendered chars). The budget is
        // ~10_000 chars, so 600 items (~18_000 chars) is comfortably over.
        for i in 0..600u32 {
            ledger.upsert(LedgerEntry {
                table: "dat".to_string(),
                dat: "weapons".to_string(),
                obj_id: i,
                property: "Damage".to_string(),
                value: json!(i),
                item_name: None,
                applied_at: 1_700_000_000 + i as i64,
                edited_by_user: false,
            });
        }
        // A distinctively named, two-property units item to match queries against.
        ledger.upsert(LedgerEntry {
            table: "dat".to_string(),
            dat: "units".to_string(),
            obj_id: 65,
            property: "HP".to_string(),
            value: json!(160),
            item_name: Some("Protoss Zealot".to_string()),
            applied_at: 1_700_000_500,
            edited_by_user: false,
        });
        ledger.upsert(LedgerEntry {
            table: "dat".to_string(),
            dat: "units".to_string(),
            obj_id: 65,
            property: "Shields".to_string(),
            value: json!(60),
            item_name: Some("Protoss Zealot".to_string()),
            applied_at: 1_700_000_500,
            edited_by_user: false,
        });
        ledger
    }

    #[test]
    fn small_ledger_renders_full_regardless_of_query() {
        let mut ledger = Ledger::default();
        ledger.upsert(entry("dat", "units", 0, "HP", json!(80), Some("Marine")));
        ledger.upsert(entry("dat", "weapons", 5, "Damage", json!(6), None));

        // A small ledger is well under budget: same body for any query, and
        // byte-equal to the legacy full render (rendered via the shared path).
        let full = render_items(ledger.entries.values());
        assert!(approx_tokens(&full) <= WIKI_PROMPT_TOKEN_BUDGET);
        assert_eq!(ledger.render_section("").as_deref(), Some(full.as_str()));
        assert_eq!(
            ledger.render_section("anything at all").as_deref(),
            Some(full.as_str())
        );
    }

    #[test]
    fn over_budget_includes_query_match_and_excludes_unrelated_keeping_item_whole() {
        let ledger = over_budget_ledger();
        let full = render_items(ledger.entries.values());
        assert!(
            approx_tokens(&full) > WIKI_PROMPT_TOKEN_BUDGET,
            "fixture must exceed budget"
        );

        // Query names the Zealot units item; an unrelated filler weapon (#0) must
        // be excludable while the matched item is kept whole (both properties).
        let section = ledger
            .render_section("how much shields does the protoss zealot have")
            .expect("over-budget ledger still renders");

        assert!(approx_tokens(&section) <= WIKI_PROMPT_TOKEN_BUDGET);
        assert!(
            section.contains("- Protoss Zealot"),
            "matched item included"
        );
        // Whole-item inclusion: both of the unit's properties survive together.
        assert!(section.contains("  - HP = 160"));
        assert!(section.contains("  - Shields = 60"));
        // The selection is a strict subset of the full body (something dropped).
        assert!(section.len() < full.len(), "an unrelated item was excluded");
    }

    #[test]
    fn over_budget_no_query_match_falls_back_to_recency_within_budget() {
        let ledger = over_budget_ledger();

        // A query that matches no keyword -> appliedAt DESC fallback. The newest
        // filler item (#599, applied_at 1_700_000_599) must appear; the budget holds.
        let section = ledger
            .render_section("zzz nonexistent gibberish term")
            .expect("over-budget ledger still renders");

        assert!(approx_tokens(&section) <= WIKI_PROMPT_TOKEN_BUDGET);
        assert!(
            section.contains("- #599"),
            "newest-applied item is selected first: {}",
            &section[..section.len().min(200)]
        );
    }

    #[test]
    fn render_section_is_deterministic_for_same_inputs() {
        let ledger = over_budget_ledger();
        let q = "buff the protoss zealot hp";
        let a = ledger.render_section(q);
        let b = ledger.render_section(q);
        assert_eq!(a, b, "same inputs must yield byte-identical output");
    }

    #[test]
    fn save_then_load_round_trips_without_bom() {
        let base = unique_temp_dir("round-trip");
        let wiki_dir = base.join("wiki");

        let mut store = WikiStore::load(Some(wiki_dir.clone()));
        store.upsert(entry(
            "dat",
            "units",
            7,
            "HP",
            json!(80),
            Some("Terran Marine"),
        ));
        store.save().unwrap();

        let bytes = fs::read(wiki_dir.join(LEDGER_FILE)).unwrap();
        assert!(
            !bytes.starts_with(&[0xEF, 0xBB, 0xBF]),
            "ledger.json must not have a BOM"
        );

        let reloaded = WikiStore::load(Some(wiki_dir));
        assert_eq!(reloaded.ledger().version, LEDGER_VERSION);
        assert_eq!(reloaded.ledger().entries.len(), 1);
        assert_eq!(reloaded.ledger().entries["dat:units:7:HP"].value, json!(80));

        fs::remove_dir_all(base).ok();
    }

    #[test]
    fn disabled_store_save_is_a_noop() {
        let mut store = WikiStore::load(None);
        store.upsert(entry("dat", "units", 0, "HP", json!(40), None));
        // No wiki dir -> save writes nothing and never errors.
        store.save().unwrap();
        assert_eq!(store.ledger().entries.len(), 1);
    }

    fn dat_changeset() -> (crate::journal::Changeset, crate::journal::Journal) {
        use crate::journal::{
            Changeset, ChangesetItem, ChangesetItemKind, DatRef, DatTable, Journal, JournalEntry,
            JournalTarget, PropertyChange, Snapshot, WriteTool,
        };

        let journal = Journal {
            request_id: "req-1".to_string(),
            entries: vec![
                JournalEntry {
                    id: "e-hp".to_string(),
                    seq: 1,
                    tool: WriteTool::DatSet,
                    target: JournalTarget::Dat {
                        table: DatTable::Dat,
                        dat: "units".to_string(),
                        obj_id: 0,
                        property: "HP".to_string(),
                    },
                    before: Snapshot::DatValue {
                        value: json!(40),
                        was_default: false,
                    },
                    after: Snapshot::DatValue {
                        value: json!(80),
                        was_default: false,
                    },
                    ts: 1_718_000_111,
                },
                JournalEntry {
                    id: "e-dmg".to_string(),
                    seq: 2,
                    tool: WriteTool::DatSet,
                    target: JournalTarget::Dat {
                        table: DatTable::Dat,
                        dat: "weapons".to_string(),
                        obj_id: 5,
                        property: "Damage".to_string(),
                    },
                    before: Snapshot::DatValue {
                        value: json!(5),
                        was_default: false,
                    },
                    after: Snapshot::DatValue {
                        value: json!(6),
                        was_default: false,
                    },
                    ts: 1_718_000_222,
                },
            ],
        };

        let changeset = Changeset {
            request_id: "req-1".to_string(),
            items: vec![
                ChangesetItem {
                    id: "dat:Dat:units:0".to_string(),
                    kind: ChangesetItemKind::Dat,
                    path: None,
                    dat_ref: Some(DatRef {
                        table: DatTable::Dat,
                        dat: "units".to_string(),
                        obj_id: 0,
                    }),
                    properties: vec![PropertyChange {
                        property: "HP".to_string(),
                        old: json!(40),
                        new: json!(80),
                        id: "e-hp".to_string(),
                        seq: 1,
                    }],
                    diff: None,
                },
                ChangesetItem {
                    id: "dat:Dat:weapons:5".to_string(),
                    kind: ChangesetItemKind::Dat,
                    path: None,
                    dat_ref: Some(DatRef {
                        table: DatTable::Dat,
                        dat: "weapons".to_string(),
                        obj_id: 5,
                    }),
                    properties: vec![PropertyChange {
                        property: "Damage".to_string(),
                        old: json!(5),
                        new: json!(6),
                        id: "e-dmg".to_string(),
                        seq: 2,
                    }],
                    diff: None,
                },
                // A file item must be ignored (out of wiki scope).
                ChangesetItem {
                    id: "f-1".to_string(),
                    kind: ChangesetItemKind::Modified,
                    path: Some("scripts/main.eps".to_string()),
                    dat_ref: None,
                    properties: Vec::new(),
                    diff: Some("--- a\n+++ b\n".to_string()),
                },
            ],
        };

        (changeset, journal)
    }

    #[test]
    fn accepted_all_records_every_dat_property_with_journal_ts_and_unit_name() {
        let (changeset, journal) = dat_changeset();
        let entries = accepted_ledger_entries(&changeset, &journal, &AcceptedScope::All);

        assert_eq!(entries.len(), 2, "two dat items, file item skipped");
        let hp = entries
            .iter()
            .find(|e| e.key() == "dat:units:0:HP")
            .unwrap();
        assert_eq!(hp.value, json!(80));
        assert_eq!(hp.applied_at, 1_718_000_111, "ts from the journal entry");
        assert_eq!(hp.item_name.as_deref(), Some("Terran Marine"));
        assert!(!hp.edited_by_user);

        let dmg = entries
            .iter()
            .find(|e| e.key() == "dat:weapons:5:Damage")
            .unwrap();
        assert_eq!(dmg.value, json!(6));
        assert_eq!(dmg.item_name, None, "non-units dat has no item name");
    }

    #[test]
    fn accepted_skips_reset_to_default_null_valued_property() {
        use crate::journal::{
            Changeset, ChangesetItem, ChangesetItemKind, DatRef, DatTable, Journal, JournalEntry,
            JournalTarget, PropertyChange, Snapshot, WriteTool,
        };

        // A dat_reset records after = Value::Null, which surfaces as a null
        // PropertyChange.new. The ledger must NOT record it (a null-valued entry
        // would fail the panel's isLedgerEntry guard and drop the wiki event).
        let journal = Journal {
            request_id: "req-reset".to_string(),
            entries: vec![JournalEntry {
                id: "e-reset".to_string(),
                seq: 1,
                tool: WriteTool::DatSet,
                target: JournalTarget::Dat {
                    table: DatTable::Dat,
                    dat: "units".to_string(),
                    obj_id: 0,
                    property: "HP".to_string(),
                },
                before: Snapshot::DatValue {
                    value: json!(80),
                    was_default: false,
                },
                after: Snapshot::DatValue {
                    value: Value::Null,
                    was_default: true,
                },
                ts: 1_718_000_333,
            }],
        };
        let changeset = Changeset {
            request_id: "req-reset".to_string(),
            items: vec![ChangesetItem {
                id: "dat:Dat:units:0".to_string(),
                kind: ChangesetItemKind::Dat,
                path: None,
                dat_ref: Some(DatRef {
                    table: DatTable::Dat,
                    dat: "units".to_string(),
                    obj_id: 0,
                }),
                properties: vec![PropertyChange {
                    property: "HP".to_string(),
                    old: json!(80),
                    new: Value::Null,
                    id: "e-reset".to_string(),
                    seq: 1,
                }],
                diff: None,
            }],
        };

        let entries = accepted_ledger_entries(&changeset, &journal, &AcceptedScope::All);
        assert!(
            entries.is_empty(),
            "a reset-to-default (null new value) records no ledger entry"
        );
    }

    #[test]
    fn accepted_ids_record_only_listed_items_or_property_ids() {
        let (changeset, journal) = dat_changeset();

        // Accept only the units item (by its changeset item id).
        let by_item = accepted_ledger_entries(
            &changeset,
            &journal,
            &AcceptedScope::Ids(vec!["dat:Dat:units:0".to_string()]),
        );
        assert_eq!(by_item.len(), 1);
        assert_eq!(by_item[0].key(), "dat:units:0:HP");

        // Accept only the weapons property (by its per-property journal entry id).
        let by_property = accepted_ledger_entries(
            &changeset,
            &journal,
            &AcceptedScope::Ids(vec!["e-dmg".to_string()]),
        );
        assert_eq!(by_property.len(), 1);
        assert_eq!(by_property[0].key(), "dat:weapons:5:Damage");
    }

    #[test]
    fn load_of_missing_or_malformed_file_yields_empty_ledger() {
        let base = unique_temp_dir("malformed");
        let wiki_dir = base.join("wiki");
        fs::create_dir_all(&wiki_dir).unwrap();
        fs::write(wiki_dir.join(LEDGER_FILE), b"not json").unwrap();

        let store = WikiStore::load(Some(wiki_dir));
        assert!(store.ledger().is_empty());

        fs::remove_dir_all(base).ok();
    }
}
