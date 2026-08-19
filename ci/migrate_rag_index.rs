//! v1 -> v2 RAG index migration (NO re-embedding).
//!
//! Upgrades the published v1 `rag-index.bin` (no `tier_level` byte) to the v2 layout
//! by re-parsing `ci/corpus/*.jsonl` to derive each chunk's `id` and `tier_level`,
//! joining on `id`, copying every v1 vector BYTE-FOR-BYTE, and stamping the tier byte.
//! Because vectors are preserved verbatim, the DEFAULT_BATCH_SIZE / EUD-107 embedding
//! space is unchanged — a full rebuild is only ever needed on a model change.
//!
//! All-or-nothing join: any v1 id absent from the corpus, OR any corpus id absent from
//! v1, is a HARD ERROR (the guard against id/tier-derivation drift vs build_rag_index.rs).

use std::{
    env,
    fs::{self, File},
    io::{BufRead, BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;

const EMBED_DIM: usize = 1024;
const INDEX_MAGIC: &[u8; 4] = b"ERAG";
const V1_VERSION: u32 = 1;
const V2_VERSION: u32 = 2;

// ---------------------------------------------------------------------------
// Pure corpus-derivation logic — DUPLICATED VERBATIM from
// `ci/build_rag_index.rs` (fnv1a64, chunk_text, tier_level_for_source,
// read_corpus over INPUT_FILES, corpus_docs_from_row, JsonlRow). The migration
// MUST derive ids/tiers byte-identically to the index builder or the all-or-
// nothing join silently fails. These MUST STAY IN SYNC with build_rag_index.rs;
// the byte-for-byte vector-preservation test + the all-or-nothing join are the
// guards against drift. The only intentional difference: this side derives ONLY
// (id, tier_level) per chunk and DROPS the text/source it would build — those
// come from the v1 entry, not re-derived here.
// ---------------------------------------------------------------------------

const INPUT_FILES: [&str; 7] = [
    "articles.jsonl",
    "eud_book.jsonl",
    "cafebook.jsonl",
    "scrmapdocs_en.jsonl",
    "eudplib_api.jsonl",
    "eudplib_examples.jsonl",
    "eud_editor_schema.jsonl",
];
const CHUNK_CHARS: usize = 2000;
const CHUNK_OVERLAP: usize = 200;

#[derive(Debug, Deserialize)]
struct JsonlRow {
    id: Option<String>,
    title: String,
    #[serde(default)]
    url: Option<String>,
    source: String,
    content: String,
    #[serde(default)]
    comments: Option<String>,
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn chunk_text(text: String) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= CHUNK_CHARS {
        return vec![text];
    }

    let step = CHUNK_CHARS - CHUNK_OVERLAP;
    let mut chunks = Vec::new();
    let mut start = 0;

    while start < chars.len() {
        let end = (start + CHUNK_CHARS).min(chars.len());
        chunks.push(chars[start..end].iter().collect());
        if end == chars.len() {
            break;
        }
        start += step;
    }

    chunks
}

/// Derive the v2 source-trust tier code from a corpus row's raw `source` field.
/// Mirrors `build_rag_index.rs::tier_level_for_source` — MUST STAY IN SYNC.
fn tier_level_for_source(source: &str) -> u8 {
    let stem = source.strip_suffix(".jsonl").unwrap_or(source);
    match stem {
        "eud_book" | "cafebook" | "scrmapdocs_en" | "eudplib_api" | "eudplib_examples"
        | "eud_editor_schema" => 3,
        "board_강좌팁" | "board_연구칼럼" => 2,
        "board_유틸리티툴" | "board_Lua자료실" => 1,
        "board_질문답변" => 0,
        _ if stem.starts_with("user_") => 1,
        // Unknown source: conservative neutral default (general).
        _ => 1,
    }
}

/// Read the corpus the SAME way `build_rag_index.rs::read_corpus` does — over the
/// FIXED `INPUT_FILES` list (NOT a glob) — and derive `(id, tier_level)` per chunk.
/// The board_*/user_* names are the per-ROW `source` FIELD inside these files, not
/// separate files; reproducing the published v1 ids depends on this exact read path.
fn read_corpus_id_to_tier(corpus_dir: &Path) -> Result<Vec<(u64, u8)>> {
    let mut out = Vec::new();

    for file_name in INPUT_FILES {
        let path = corpus_dir.join(file_name);
        let file =
            File::open(&path).with_context(|| format!("open JSONL input {}", path.display()))?;
        let reader = BufReader::new(file);

        for (zero_based_line, line) in reader.lines().enumerate() {
            let line_number = zero_based_line + 1;
            let line =
                line.with_context(|| format!("read {} line {line_number}", path.display()))?;
            if line.trim().is_empty() {
                continue;
            }

            let row: JsonlRow = serde_json::from_str(&line)
                .with_context(|| format!("parse {} line {line_number}", path.display()))?;
            out.extend(corpus_docs_id_tier_from_row(row, file_name, line_number));
        }
    }

    Ok(out)
}

/// Derive `(id, tier_level)` per chunk for one row, mirroring
/// `build_rag_index.rs::corpus_docs_from_row`'s chunk_key + tier logic VERBATIM,
/// but dropping the text/source it would build (those come from v1).
fn corpus_docs_id_tier_from_row(
    row: JsonlRow,
    file_name: &str,
    line_number: usize,
) -> Vec<(u64, u8)> {
    let content = row.content.trim();
    let comments = row.comments.as_deref().unwrap_or("").trim();
    if content.is_empty() && comments.is_empty() {
        return Vec::new();
    }

    // All chunks of a row share the row's source tier.
    let tier_level = tier_level_for_source(&row.source);

    let title = row.title.trim();
    let url = row.url.as_deref().unwrap_or("").trim();
    let mut text = format!("제목: {title}\n\n{content}");
    if !comments.is_empty() {
        text.push_str("\n\n[댓글]\n");
        text.push_str(comments);
    }

    let key = row
        .id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(|id| format!("id:{id}"))
        .or_else(|| (!url.is_empty()).then(|| format!("url:{url}")))
        .unwrap_or_else(|| format!("source:{}:{file_name}:{line_number}", row.source));

    let chunks = chunk_text(text);
    chunks
        .into_iter()
        .enumerate()
        .map(|(chunk_index, _chunk_text)| {
            let chunk_key = format!("{key}#{chunk_index}");
            // Stable ids: FNV-1a 64-bit of the deterministic chunk key (matches
            // build_rag_index.rs). The text/source the builder produces here are
            // intentionally discarded — v1 supplies them.
            (fnv1a64(chunk_key.as_bytes()), tier_level)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// v1 reader (the migration owns this — src-tauri/src/rag.rs::load_index now
// REJECTS the v1 layout). v1: NO tier byte.
//   magic b"ERAG" | version u32 = 1 | count u32 |
//   per entry: id u64 | vector f32 x 1024 | text_len u32 + text | source_len u32 + source
// All little-endian, UTF-8 no BOM. Typed anyhow errors on bad magic/version/
// truncation — NEVER panics on a malformed file.
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct V1Entry {
    id: u64,
    vector: Vec<f32>,
    text: String,
    source: String,
}

/// Sequential byte cursor over an in-memory buffer; every read is bounds-checked
/// and returns a typed error rather than panicking on a truncated file.
struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or_else(|| anyhow!("index read overflow"))?;
        if end > self.bytes.len() {
            bail!(
                "truncated index: need {n} bytes at offset {}, have {}",
                self.pos,
                self.bytes.len() - self.pos
            );
        }
        let slice = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn take_u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn take_u64(&mut self) -> Result<u64> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    // Only the test-only v2 reader reads a bare tier byte; gate it to the test
    // build so the bin compile (clippy `-D dead-code`) stays clean.
    #[cfg(test)]
    fn take_u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn take_len_prefixed_string(&mut self, field: &str) -> Result<String> {
        let len = self.take_u32()? as usize;
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec()).with_context(|| format!("{field} is not valid UTF-8"))
    }

    fn take_vector(&mut self) -> Result<Vec<f32>> {
        let bytes = self.take(EMBED_DIM * 4)?;
        let mut vector = Vec::with_capacity(EMBED_DIM);
        for chunk in bytes.chunks_exact(4) {
            vector.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
        }
        Ok(vector)
    }
}

fn read_file_bytes(path: &Path) -> Result<Vec<u8>> {
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .with_context(|| format!("read {}", path.display()))?;
    Ok(bytes)
}

fn read_v1_index(path: &Path) -> Result<Vec<V1Entry>> {
    let bytes = read_file_bytes(path)?;
    let mut cur = Cursor::new(&bytes);

    let magic = cur.take(4)?;
    if magic != INDEX_MAGIC {
        bail!("bad magic {magic:?} (expected {INDEX_MAGIC:?})");
    }
    let version = cur.take_u32()?;
    if version != V1_VERSION {
        bail!("unsupported v1 version {version} (expected {V1_VERSION})");
    }
    let count = cur.take_u32()? as usize;

    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let id = cur.take_u64()?;
        let vector = cur.take_vector()?;
        let text = cur.take_len_prefixed_string("text")?;
        let source = cur.take_len_prefixed_string("source")?;
        entries.push(V1Entry {
            id,
            vector,
            text,
            source,
        });
    }

    Ok(entries)
}

// ---------------------------------------------------------------------------
// v2 entry + join + writer/reader. v2 layout is byte-identical to
// build_rag_index.rs / rag.rs: the single tier byte sits AFTER the full vector
// and BEFORE the text length prefix.
// ---------------------------------------------------------------------------

#[derive(Debug)]
struct V2Entry {
    id: u64,
    vector: Vec<f32>,
    tier_level: u8,
    text: String,
    source: String,
}

/// Join v1 entries against the corpus-derived `(id -> tier)` map, stamping each
/// v1 entry's tier and copying its vector/text/source VERBATIM. All-or-nothing:
/// a v1 id absent from the corpus → hard error; a corpus id never consumed by any
/// v1 entry → hard error (leftover corpus id).
fn build_v2_entries(
    v1_entries: Vec<V1Entry>,
    corpus_id_to_tier: &[(u64, u8)],
) -> Result<Vec<V2Entry>> {
    use std::collections::{HashMap, HashSet};

    let mut tier_by_id: HashMap<u64, u8> = HashMap::with_capacity(corpus_id_to_tier.len());
    for (id, tier) in corpus_id_to_tier {
        tier_by_id.insert(*id, *tier);
    }

    // Track which corpus ids a v1 entry consumed so leftovers can be detected.
    let mut consumed: HashSet<u64> = HashSet::with_capacity(v1_entries.len());

    let mut entries = Vec::with_capacity(v1_entries.len());
    for v1 in v1_entries {
        let tier_level = *tier_by_id.get(&v1.id).ok_or_else(|| {
            anyhow!(
                "v1 id {} ({}) has no corpus match — refusing partial migration",
                v1.id,
                v1.source
            )
        })?;
        consumed.insert(v1.id);
        entries.push(V2Entry {
            id: v1.id,
            vector: v1.vector,
            tier_level,
            text: v1.text,
            source: v1.source,
        });
    }

    // Any corpus id never matched by a v1 entry is a hard error too.
    if let Some((orphan_id, _)) = corpus_id_to_tier
        .iter()
        .find(|(id, _)| !consumed.contains(id))
    {
        bail!("corpus id {orphan_id} has no v1 match — refusing partial migration");
    }

    Ok(entries)
}

fn write_len_prefixed(w: &mut BufWriter<File>, bytes: &[u8], field: &str) -> Result<()> {
    if bytes.len() > u32::MAX as usize {
        bail!("{field} is {} bytes; v2 format length is u32", bytes.len());
    }
    w.write_all(&(bytes.len() as u32).to_le_bytes())?;
    w.write_all(bytes)?;
    Ok(())
}

/// Write the v2 index — byte-identical layout to build_rag_index.rs / rag.rs:
///   magic | version u32 = 2 | count u32 |
///   per entry: id u64 | vector f32 x 1024 | tier_level u8 | text_len u32 + text | source_len u32 + source
/// Little-endian, UTF-8 no BOM.
fn write_v2_index(path: &Path, entries: &[V2Entry]) -> Result<()> {
    if entries.len() > u32::MAX as usize {
        bail!(
            "index has {} entries; v2 format count is u32",
            entries.len()
        );
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("create output directory {}", parent.display()))?;
    }

    let file = File::create(path).with_context(|| format!("create {}", path.display()))?;
    let mut w = BufWriter::new(file);

    w.write_all(INDEX_MAGIC)?;
    w.write_all(&V2_VERSION.to_le_bytes())?;
    w.write_all(&(entries.len() as u32).to_le_bytes())?;

    for entry in entries {
        if entry.vector.len() != EMBED_DIM {
            bail!(
                "entry id {} has {}-d vector, expected {EMBED_DIM}-d",
                entry.id,
                entry.vector.len()
            );
        }

        w.write_all(&entry.id.to_le_bytes())?;
        for value in &entry.vector {
            w.write_all(&value.to_le_bytes())?;
        }
        // v2: one tier byte AFTER the full vector, BEFORE the text length prefix.
        w.write_all(&[entry.tier_level])?;
        write_len_prefixed(&mut w, entry.text.as_bytes(), "text")?;
        write_len_prefixed(&mut w, entry.source.as_bytes(), "source")?;
    }

    w.flush()?;
    Ok(())
}

/// Read the v2 layout back into `Vec<V2Entry>` (for verification / tests). Typed
/// errors on bad magic/version/truncation — NEVER panics on a malformed file.
///
/// Test-only: the migration bin only WRITES v2 (the app's `rag.rs::load_index`
/// is the production v2 reader); this reader exists to verify the written bytes.
#[cfg(test)]
fn read_v2_index(path: &Path) -> Result<Vec<V2Entry>> {
    let bytes = read_file_bytes(path)?;
    let mut cur = Cursor::new(&bytes);

    let magic = cur.take(4)?;
    if magic != INDEX_MAGIC {
        bail!("bad magic {magic:?} (expected {INDEX_MAGIC:?})");
    }
    let version = cur.take_u32()?;
    if version != V2_VERSION {
        bail!("unsupported v2 version {version} (expected {V2_VERSION})");
    }
    let count = cur.take_u32()? as usize;

    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let id = cur.take_u64()?;
        let vector = cur.take_vector()?;
        let tier_level = cur.take_u8()?;
        let text = cur.take_len_prefixed_string("text")?;
        let source = cur.take_len_prefixed_string("source")?;
        entries.push(V2Entry {
            id,
            vector,
            tier_level,
            text,
            source,
        });
    }

    Ok(entries)
}

/// Read v1, derive the corpus `(id -> tier)` map, join all-or-nothing, write v2.
fn migrate_v1_to_v2(v1_path: &Path, corpus_dir: &Path, out_path: &Path) -> Result<()> {
    let v1_entries = read_v1_index(v1_path)?;
    let corpus_id_to_tier = read_corpus_id_to_tier(corpus_dir)?;
    let v2_entries = build_v2_entries(v1_entries, &corpus_id_to_tier)?;
    write_v2_index(out_path, &v2_entries)?;
    Ok(())
}

#[derive(Debug)]
struct Args {
    in_path: PathBuf,
    corpus_dir: PathBuf,
    out_path: PathBuf,
}

fn parse_args() -> Result<Args> {
    let mut in_path = None;
    let mut corpus_dir = None;
    let mut out_path = None;

    let mut args = env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_string_lossy().as_ref() {
            "--in" => {
                in_path = Some(PathBuf::from(
                    args.next()
                        .ok_or_else(|| anyhow!("--in requires the v1 index path"))?,
                ));
            }
            "--corpus" => {
                corpus_dir =
                    Some(PathBuf::from(args.next().ok_or_else(|| {
                        anyhow!("--corpus requires a directory path")
                    })?));
            }
            "--out" => {
                out_path =
                    Some(PathBuf::from(args.next().ok_or_else(|| {
                        anyhow!("--out requires an output file path")
                    })?));
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            other => bail!("unknown argument {other:?}; run with --help for usage"),
        }
    }

    Ok(Args {
        in_path: in_path.ok_or_else(|| anyhow!("--in <v1 rag-index.bin> is required"))?,
        corpus_dir: corpus_dir.unwrap_or_else(|| PathBuf::from("ci/corpus")),
        out_path: out_path.ok_or_else(|| anyhow!("--out <v2 rag-index.bin> is required"))?,
    })
}

fn print_usage() {
    eprintln!(
        "usage: migrate_rag_index --in <v1.bin> [--corpus <dir>] --out <v2.bin>\n\
         defaults: --corpus ci/corpus\n\
         Upgrades a v1 rag-index.bin to v2 WITHOUT re-embedding (vectors copied byte-for-byte)."
    );
}

fn main() -> Result<()> {
    let args = parse_args()?;
    migrate_v1_to_v2(&args.in_path, &args.corpus_dir, &args.out_path)?;
    eprintln!(
        "migrated: {} + {} -> {}",
        args.in_path.display(),
        args.corpus_dir.display(),
        args.out_path.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs::{self, File};
    use std::io::Write;
    use std::path::{Path, PathBuf};

    /// Per-test temp dir under the OS temp root; removed on drop. Avoids leaking
    /// synthesized corpus/index files between runs.
    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(tag: &str) -> Self {
            let mut path = std::env::temp_dir();
            let nonce = format!(
                "eud-migrate-{tag}-{}-{:?}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            );
            path.push(nonce);
            fs::create_dir_all(&path).expect("create temp dir");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    /// Create the corpus dir with ALL `INPUT_FILES` present as empty files, mirroring
    /// the real corpus (the migration reads the FIXED INPUT_FILES list, not a glob, and
    /// errors hard on a missing one — exactly like `build_rag_index.rs::read_corpus`).
    /// Tests then overwrite only the file(s) they populate; the rest stay empty (their
    /// blank lines are skipped), so each test still exercises the real INPUT_FILES read.
    fn init_corpus_dir(tmp: &TempDir) -> PathBuf {
        let corpus_dir = tmp.path().join("corpus");
        fs::create_dir_all(&corpus_dir).expect("create corpus dir");
        for file_name in super::INPUT_FILES {
            File::create(corpus_dir.join(file_name)).expect("create empty input file");
        }
        corpus_dir
    }

    /// Write `.jsonl` rows into `dir/<file_name>` (overwrites). The row's `source`
    /// field drives the derived tier; `id` (when set) drives the chunk key.
    fn write_corpus_file(dir: &Path, file_name: &str, rows: &[&str]) {
        let path = dir.join(file_name);
        let mut f = File::create(&path).expect("create corpus file");
        for row in rows {
            f.write_all(row.as_bytes()).expect("write row");
            f.write_all(b"\n").expect("write newline");
        }
    }

    /// Hand-write a v1-format `.bin` (NO tier byte) from raw (id, vector, text, source)
    /// tuples, mirroring the v1 layout the migration's v1 reader must accept:
    ///   magic b"ERAG" | version u32 = 1 | count u32 |
    ///   per entry: id u64 | vector f32 x 1024 | text_len u32 + text | source_len u32 + source
    /// All little-endian, UTF-8 no BOM.
    fn write_v1_index(path: &Path, entries: &[(u64, Vec<f32>, &str, &str)]) {
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(b"ERAG");
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        for (id, vector, text, source) in entries {
            buf.extend_from_slice(&id.to_le_bytes());
            assert_eq!(vector.len(), super::EMBED_DIM, "v1 vector must be 1024-d");
            for value in vector {
                buf.extend_from_slice(&value.to_le_bytes());
            }
            buf.extend_from_slice(&(text.len() as u32).to_le_bytes());
            buf.extend_from_slice(text.as_bytes());
            buf.extend_from_slice(&(source.len() as u32).to_le_bytes());
            buf.extend_from_slice(source.as_bytes());
        }
        fs::write(path, buf).expect("write v1 index");
    }

    /// A deterministic, non-trivial 1024-d vector so the byte-for-byte preservation
    /// assert is meaningful (not all-zero / all-equal). `seed` decorrelates entries.
    fn synth_vector(seed: u64) -> Vec<f32> {
        (0..super::EMBED_DIM)
            .map(|i| {
                let x = (seed.wrapping_mul(2_654_435_761).wrapping_add(i as u64)) % 100_000;
                (x as f32) / 100_000.0 - 0.5
            })
            .collect()
    }

    /// TEST 1 — vector preservation + tier stamping.
    ///
    /// Synthesize a tiny corpus faithful to the REAL data shape (board name lives in the
    /// per-row `source` FIELD inside the physical INPUT_FILES, not a separate file) and a
    /// hand-written v1 `.bin` whose ids match the corpus chunks; run the migration; assert
    /// (a) every v2 vector is byte-identical to its v1 source, and (b) each entry's
    /// `tier_level` equals the source-tier mapping.
    #[test]
    fn migration_preserves_vectors_byte_for_byte_and_stamps_tier() {
        let tmp = TempDir::new("preserve");
        let corpus_dir = init_corpus_dir(&tmp);

        // tier-3 row: a physical cafebook.jsonl row whose source is "cafebook.jsonl".
        write_corpus_file(
            &corpus_dir,
            "cafebook.jsonl",
            &[
                r#"{"id":"a1","title":"오피셜","url":"https://x/1","source":"cafebook.jsonl","content":"official content"}"#,
            ],
        );
        // tier-0 row: a Q&A chunk is encoded as a row INSIDE articles.jsonl whose
        // source FIELD is "board_질문답변.jsonl" (the real corpus shape).
        write_corpus_file(
            &corpus_dir,
            "articles.jsonl",
            &[
                r#"{"id":"q1","title":"질문","url":"https://x/2","source":"board_질문답변.jsonl","content":"qna content"}"#,
            ],
        );

        // Derive each chunk's id the SAME way the migration must (single chunk each ->
        // chunk_index 0).
        let id_official = super::fnv1a64(b"id:a1#0");
        let id_qna = super::fnv1a64(b"id:q1#0");

        let vec_official = synth_vector(7);
        let vec_qna = synth_vector(42);

        let v1_path = tmp.path().join("rag-index.v1.bin");
        write_v1_index(
            &v1_path,
            &[
                (
                    id_official,
                    vec_official.clone(),
                    "official content",
                    "[오피셜](https://x/1)",
                ),
                (
                    id_qna,
                    vec_qna.clone(),
                    "qna content",
                    "[질문](https://x/2)",
                ),
            ],
        );

        let out_path = tmp.path().join("rag-index.v2.bin");

        super::migrate_v1_to_v2(&v1_path, &corpus_dir, &out_path)
            .expect("migration should succeed when every id joins");

        let v2_entries = super::read_v2_index(&out_path).expect("read migrated v2 index");

        // (a) byte-for-byte vector preservation: compare the LE byte serialization.
        let want_bytes =
            |v: &[f32]| -> Vec<u8> { v.iter().flat_map(|f| f.to_le_bytes()).collect() };
        for entry in &v2_entries {
            let got = want_bytes(&entry.vector);
            let expected = if entry.id == id_official {
                want_bytes(&vec_official)
            } else if entry.id == id_qna {
                want_bytes(&vec_qna)
            } else {
                panic!("unexpected id {} in v2 output", entry.id);
            };
            assert_eq!(
                got, expected,
                "v2 vector must be byte-identical to v1 source"
            );
        }

        // (b) tier stamping matches the source-tier mapping.
        let official = v2_entries
            .iter()
            .find(|e| e.id == id_official)
            .expect("official entry present");
        let qna = v2_entries
            .iter()
            .find(|e| e.id == id_qna)
            .expect("qna entry present");
        assert_eq!(
            official.tier_level,
            super::tier_level_for_source("cafebook.jsonl"),
            "official source must map to its tier"
        );
        assert_eq!(official.tier_level, 3, "cafebook is tier 3");
        assert_eq!(
            qna.tier_level,
            super::tier_level_for_source("board_질문답변.jsonl"),
            "qna source must map to its tier"
        );
        assert_eq!(qna.tier_level, 0, "board_질문답변 is tier 0");
    }

    /// TEST 2a — a v1 id with NO corpus match is a HARD ERROR.
    #[test]
    fn migration_errors_when_v1_id_has_no_corpus_match() {
        let tmp = TempDir::new("v1-orphan");
        let corpus_dir = init_corpus_dir(&tmp);

        // Corpus has exactly one chunk (id:a1#0), in a physical INPUT_FILES file.
        write_corpus_file(
            &corpus_dir,
            "cafebook.jsonl",
            &[
                r#"{"id":"a1","title":"t","url":"https://x/1","source":"cafebook.jsonl","content":"c"}"#,
            ],
        );
        let id_matched = super::fnv1a64(b"id:a1#0");
        // A second v1 id that no corpus chunk derives -> orphan.
        let id_orphan = super::fnv1a64(b"id:NOPE#0");

        let v1_path = tmp.path().join("rag-index.v1.bin");
        write_v1_index(
            &v1_path,
            &[
                (id_matched, synth_vector(1), "c", "[t](https://x/1)"),
                (
                    id_orphan,
                    synth_vector(2),
                    "orphan",
                    "[orphan](https://x/9)",
                ),
            ],
        );
        let out_path = tmp.path().join("rag-index.v2.bin");

        let result = super::migrate_v1_to_v2(&v1_path, &corpus_dir, &out_path);
        assert!(
            result.is_err(),
            "a v1 id absent from the corpus must be a hard error"
        );
    }

    /// TEST 2b — a corpus id with NO v1 match is a HARD ERROR.
    #[test]
    fn migration_errors_when_corpus_id_has_no_v1_match() {
        let tmp = TempDir::new("corpus-orphan");
        let corpus_dir = init_corpus_dir(&tmp);

        // Corpus has TWO chunks (both rows in a physical INPUT_FILES file) ...
        write_corpus_file(
            &corpus_dir,
            "cafebook.jsonl",
            &[
                r#"{"id":"a1","title":"t1","url":"https://x/1","source":"cafebook.jsonl","content":"c1"}"#,
                r#"{"id":"a2","title":"t2","url":"https://x/2","source":"cafebook.jsonl","content":"c2"}"#,
            ],
        );
        let id_a1 = super::fnv1a64(b"id:a1#0");
        // ... but v1 only carries one of them: a2 is a corpus orphan.

        let v1_path = tmp.path().join("rag-index.v1.bin");
        write_v1_index(
            &v1_path,
            &[(id_a1, synth_vector(3), "c1", "[t1](https://x/1)")],
        );
        let out_path = tmp.path().join("rag-index.v2.bin");

        let result = super::migrate_v1_to_v2(&v1_path, &corpus_dir, &out_path);
        assert!(
            result.is_err(),
            "a corpus id absent from v1 must be a hard error"
        );
    }
}
