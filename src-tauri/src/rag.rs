//! RAG embedding core: in-process [fastembed] bge-m3 dense embeddings + brute-force
//! cosine top-k (Decision 10).
//!
//! This module is the embedding half of the retrieval path. It wraps fastembed's
//! `Bgem3Embedding` (the INT8-quantized `BGEM3Q` ONNX variant — the SAME model
//! `bootstrap::ensure_model` downloads into the HF cache under `models_dir()`),
//! takes the **dense** 1024-d output, and L2-normalizes each vector so cosine
//! similarity reduces to a plain dot product.
//!
//! The full warmup / persisted-index / query-orchestration path is a LATER task;
//! this is the minimal embedding + ranking surface needed for the EUD-107 parity
//! spike (fastembed quantized output vs the Python sentence-transformers
//! full-precision baseline). The parity test lives in [`mod@parity`] below.
//!
//! Errors are the typed [`RagError`]; the public API never panics on caller input.

use std::path::PathBuf;

/// The dense embedding dimensionality of bge-m3 (`outputs[0]` is `[batch, 1024]`).
pub const EMBED_DIM: usize = 1024;

/// Typed errors for the embedding / ranking surface.
#[derive(Debug, thiserror::Error)]
pub enum RagError {
    /// The fastembed model could not be initialized (download / ONNX init failed),
    /// or an `embed` call returned an error from the ONNX session.
    #[error("fastembed bge-m3 embedding failed: {0}")]
    Embed(String),
    /// The model returned a dense vector whose length is not [`EMBED_DIM`] — a
    /// shape mismatch that would corrupt cosine math, so it is rejected up front.
    #[error("expected {EMBED_DIM}-d dense vector, got {0}-d")]
    Dim(usize),
    /// A [`Rag::search`] was issued before the embedding model finished warming up.
    /// Search NEVER blocks waiting for the model — it returns this so the caller can
    /// retry once `rag_warmup` completes (rules.md: warmup must not gate readiness).
    #[error("rag model still warming up")]
    Warming,
    /// The persisted `.bin` index could not be loaded: bad magic/version, truncation,
    /// an I/O error, or a vector whose length is not [`EMBED_DIM`]. Never a panic.
    #[error("rag index load failed: {0}")]
    Index(String),
}

/// A single L2-normalized dense bge-m3 embedding (cosine == dot on these).
///
/// Kept as a heap `Vec<f32>` rather than `[f32; EMBED_DIM]` so the type stays
/// flexible for the later index-load path; every value this module produces is
/// length-checked to [`EMBED_DIM`] and L2-normalized.
pub type Embedding = Vec<f32>;

/// In-process embedder over fastembed's bge-m3 (INT8 `BGEM3Q`) ONNX model.
///
/// Holds the loaded `Bgem3Embedding` session. Constructed via [`Embedder::new`],
/// which triggers the (cached) model download on first use.
pub struct Embedder {
    /// The loaded fastembed bge-m3 session. Embedding requires `&mut self`
    /// (the ONNX session run is `&mut`), so this is owned, not shared.
    inner: fastembed::Bgem3Embedding,
}

impl Embedder {
    /// Construct an embedder, initializing the bge-m3 `BGEM3Q` model.
    ///
    /// When `cache_dir` is `Some`, fastembed's HF cache is pointed there (matching
    /// `bootstrap::ensure_model`, which uses `DataDirs::models_dir()`); `None` uses
    /// fastembed's default cache. The first call with an empty cache downloads the
    /// model (~570MB) — expected, and the reason the parity test is `#[ignore]`d.
    pub fn new(cache_dir: Option<PathBuf>) -> Result<Self, RagError> {
        use fastembed::{Bgem3Embedding, Bgem3InitOptions, Bgem3Model};
        let _model_guard = crate::bootstrap::lock_model_initialization();

        // BGEM3Q == the INT8-quantized bge-m3 ONNX variant — the SAME model
        // `bootstrap::ensure_model` downloads. We compare its quantized output
        // against the Python full-precision baseline in the parity test.
        let mut opts = Bgem3InitOptions::new(Bgem3Model::BGEM3Q);
        if let Some(dir) = cache_dir {
            opts = opts.with_cache_dir(dir);
        }
        let inner = Bgem3Embedding::try_new(opts)
            .map_err(|e| RagError::Embed(format!("model init: {e}")))?;
        Ok(Self { inner })
    }

    /// Embed a batch of texts, returning one L2-normalized [`Embedding`] per input,
    /// in input order. Each vector is the bge-m3 **dense** 1024-d output normalized
    /// so that cosine similarity equals the dot product.
    pub fn embed(&mut self, texts: &[String]) -> Result<Vec<Embedding>, RagError> {
        // `output.dense` is `[batch, 1024]` and is NOT pre-normalized by the model,
        // so we L2-normalize each row here (cosine == dot on normalized vectors).
        let output = self
            .inner
            .embed(texts, None)
            .map_err(|e| RagError::Embed(e.to_string()))?;

        let mut out = Vec::with_capacity(output.dense.len());
        for mut v in output.dense {
            if v.len() != EMBED_DIM {
                return Err(RagError::Dim(v.len()));
            }
            l2_normalize(&mut v);
            out.push(v);
        }
        Ok(out)
    }
}

/// L2-normalize a dense vector in place so its Euclidean norm is 1 (cosine == dot).
///
/// A zero (or denormal) vector is left unchanged rather than producing NaNs — the
/// caller treats an all-zero embedding as orthogonal to everything.
pub fn l2_normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    // Guard against a zero/denormal vector: dividing by ~0 yields inf/NaN, which
    // would poison every cosine score. Leave such a vector as-is (it then dots to 0
    // against any normalized vector — effectively orthogonal).
    if norm > f32::EPSILON {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

/// Cosine similarity of two L2-normalized vectors (== dot product). Inputs are
/// assumed already normalized by [`l2_normalize`] / [`Embedder::embed`].
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// Split a query into lexical search terms for substring matching.
///
/// Maximal runs of ASCII identifier chars (`[A-Za-z0-9_]`) and maximal runs of
/// Hangul are extracted as separate terms; every other char is a separator. This
/// splits an identifier from an attached Korean particle (`chatEvent에` ->
/// `chatevent`) and keeps hex/underscored tokens intact (`0x58D900`, `__addr__`).
/// ASCII terms are lowercased (case-insensitive matching); runs shorter than
/// [`MIN_LEXICAL_TERM`] chars are dropped. Duplicate terms are removed, order kept.
pub fn tokenize_lexical(query: &str) -> Vec<String> {
    #[derive(PartialEq, Clone, Copy)]
    enum Class {
        Ascii,
        Hangul,
        Other,
    }
    fn classify(ch: char) -> Class {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            Class::Ascii
        } else if ('\u{AC00}'..='\u{D7A3}').contains(&ch) // Hangul syllables
            || ('\u{1100}'..='\u{11FF}').contains(&ch) // Hangul Jamo
            || ('\u{3130}'..='\u{318F}').contains(&ch)
        // Hangul Compatibility Jamo
        {
            Class::Hangul
        } else {
            Class::Other
        }
    }
    fn push_term(cur: &mut String, terms: &mut Vec<String>) {
        if cur.chars().count() >= MIN_LEXICAL_TERM {
            let term = cur.to_lowercase();
            if !terms.iter().any(|existing| existing == &term) {
                terms.push(term);
            }
        }
        cur.clear();
    }

    let mut terms: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_class = Class::Other;
    for ch in query.chars() {
        let class = classify(ch);
        // A separator, or a switch between Ascii/Hangul, closes the current run.
        if class == Class::Other || class != cur_class {
            push_term(&mut cur, &mut terms);
        }
        if class != Class::Other {
            cur.push(ch);
            cur_class = class;
        }
    }
    push_term(&mut cur, &mut terms);
    terms
}

/// Rank `corpus` by cosine similarity to `query` and return the top-`k` corpus
/// indices, best first. All vectors are assumed L2-normalized (cosine == dot), so
/// ranking is a brute-force dot product per corpus entry (Decision 10). Ties break
/// by lower index for a deterministic order.
pub fn top_k(query: &[f32], corpus: &[Embedding], k: usize) -> Vec<usize> {
    let mut scored: Vec<(usize, f32)> = corpus
        .iter()
        .enumerate()
        .map(|(i, v)| (i, cosine(query, v)))
        .collect();
    // Sort by score desc; on ties prefer the lower index for a deterministic order
    // (matches the Python baseline's stable argsort tie-break).
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    scored.into_iter().take(k).map(|(i, _)| i).collect()
}

// ---------------------------------------------------------------------------------------
// Query path (EUD-109): persisted `.bin` index + lazy-warmup `Rag` over the embedder.
// ---------------------------------------------------------------------------------------

/// Max top-k returned by [`Rag::rank`] / [`Rag::search`] (v1 Korean-query guidance
/// clamped k to 10). A larger requested `k` is clamped down to this.
pub const MAX_TOP_K: usize = 10;

/// Minimum length (in chars) of a lexical query term. Runs shorter than this are
/// dropped by [`tokenize_lexical`] so a bare Korean particle (`에`, `의`) or a
/// single letter does not substring-match nearly every chunk.
const MIN_LEXICAL_TERM: usize = 2;

/// Per-tier ranking multiplier indexed by `IndexEntry.tier_level` (0=Q&A … 3=official).
/// A narrow band near 1.0 so source tier NUDGES but never DOMINATES the cosine signal
/// (bge-m3 cosines cluster ~0.3–0.9). Retunable in code without rebuilding the index
/// (features/17_rag-knowledge-tiering.md, Decision 18). Pinned by `rank_is_tier_weighted`.
const TIER_WEIGHT: [f32; 4] = [1.00, 1.05, 1.10, 1.15];

/// v1 `search_docs` guidance preserved as a single source of truth (surfaced by the
/// future tools layer): the ECA corpus is Korean, so queries should be phrased in
/// Korean while keeping eps/API identifiers as-is, and `k` is clamped to
/// [`MAX_TOP_K`] (10).
pub const SEARCH_DOCS_GUIDANCE: &str = "The ECA reference corpus is written in Korean: \
phrase queries in Korean for the best retrieval, but keep eps/API identifiers (function \
and type names) verbatim as-is. Results are clamped to the top 10 (MAX_TOP_K).";

/// `.bin` index magic — first 4 bytes of the persisted index file.
const INDEX_MAGIC: &[u8; 4] = b"ERAG";
/// `.bin` index format version (bumped on any layout change).
const INDEX_VERSION: u32 = 2;

/// Upper bound on the `Vec::with_capacity` HINT used when loading the index. This is
/// only a pre-allocation cap (NOT a hard entry limit — the loop still reads exactly the
/// header's `count`), so an untrusted/corrupt `count` can't trigger a huge speculative
/// allocation; a wrong count still surfaces as a truncation `RagError::Index`. The real
/// ECA index is ~5k rows, so this leaves ample headroom.
const INDEX_CAP_HINT: usize = 65_536;

/// One in-memory index row: an L2-normalized embedding plus its source text + the
/// `[reference context]` link header the evidence gate cites. The whole index is
/// loaded into RAM at warmup; [`Rag::rank`] brute-force scans it (Decision 10).
#[derive(Debug, Clone, PartialEq)]
pub struct IndexEntry {
    /// Stable corpus id (also the deterministic tie-break key in ranking).
    pub id: u64,
    /// The L2-normalized dense bge-m3 embedding (length [`EMBED_DIM`]).
    pub vector: Embedding,
    /// v2 source-trust tier code (0=Q&A … 3=official) that EUD-157's weighted
    /// [`Rag::rank`] will consume to bias higher-trust chunks. Only the tier CODE is
    /// stored in the index; the per-tier multiplier lives in code, not the index, so
    /// it can be retuned without rebuilding (Decision 18).
    pub tier_level: u8,
    /// The chunk text shown to the model as `[reference context]`.
    pub text: String,
    /// The citation link header (`[title](url)`) the evidence gate requires.
    pub source: String,
}

/// How a ranked chunk matched the query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchKind {
    Semantic,
    Lexical,
}

impl MatchKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Semantic => "semantic",
            Self::Lexical => "lexical",
        }
    }
}

/// A ranked search result. `source` carries the `[reference context]` link header the
/// evidence gate cites.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    /// Stable corpus chunk id.
    pub id: u64,
    /// Source-trust tier code (0=Q&A … 3=official).
    pub tier_level: u8,
    /// Whether lexical or semantic ranking produced this hit.
    pub match_kind: MatchKind,
    /// The matched chunk text.
    pub text: String,
    /// The citation link header (`[title](url)`).
    pub source: String,
    /// Weighted semantic score or lexical occurrence count.
    pub score: f32,
}

/// Serialize `entries` to the at-rest `.bin` format (used by tests now; the CI index
/// builder later). Layout (little-endian):
///
/// ```text
/// magic b"ERAG" [4] | version u32 = 2 | count u32 |
///   count records: id u64 | vector EMBED_DIM*f32 (4096 bytes) |
///   tier_level u8 (1 byte) | text_len u32 + text utf8 | source_len u32 + source utf8
/// ```
///
/// The single `tier_level` byte sits AFTER the full vector and BEFORE the text length.
/// A vector whose length is not [`EMBED_DIM`] is rejected as [`RagError::Index`] before
/// any bytes are written.
pub fn write_index(path: &std::path::Path, entries: &[IndexEntry]) -> Result<(), RagError> {
    use std::io::Write;

    let file = std::fs::File::create(path)
        .map_err(|e| RagError::Index(format!("create {}: {e}", path.display())))?;
    let mut w = std::io::BufWriter::new(file);

    let write = |w: &mut std::io::BufWriter<std::fs::File>, buf: &[u8]| -> Result<(), RagError> {
        w.write_all(buf)
            .map_err(|e| RagError::Index(format!("write: {e}")))
    };

    write(&mut w, INDEX_MAGIC)?;
    write(&mut w, &INDEX_VERSION.to_le_bytes())?;
    write(&mut w, &(entries.len() as u32).to_le_bytes())?;

    for entry in entries {
        if entry.vector.len() != EMBED_DIM {
            return Err(RagError::Index(format!(
                "entry id {} has {}-d vector, expected {EMBED_DIM}-d",
                entry.id,
                entry.vector.len()
            )));
        }
        write(&mut w, &entry.id.to_le_bytes())?;
        for f in &entry.vector {
            write(&mut w, &f.to_le_bytes())?;
        }
        // v2: one tier byte AFTER the full vector, BEFORE the text length prefix.
        write(&mut w, &[entry.tier_level])?;
        let text = entry.text.as_bytes();
        write(&mut w, &(text.len() as u32).to_le_bytes())?;
        write(&mut w, text)?;
        let source = entry.source.as_bytes();
        write(&mut w, &(source.len() as u32).to_le_bytes())?;
        write(&mut w, source)?;
    }

    w.flush()
        .map_err(|e| RagError::Index(format!("flush: {e}")))?;
    Ok(())
}

/// Load the `.bin` index fully into memory. Rejects bad magic/version, truncation, an
/// I/O error, and any vector whose length is not [`EMBED_DIM`] — all as
/// [`RagError::Index`]. NEVER panics on a malformed file. The `version != INDEX_VERSION`
/// guard rejects the v1 layout (no `tier_level` byte) as [`RagError::Index`].
pub fn load_index(path: &std::path::Path) -> Result<Vec<IndexEntry>, RagError> {
    let bytes = std::fs::read(path)
        .map_err(|e| RagError::Index(format!("read {}: {e}", path.display())))?;

    let mut cur = Cursor::new(&bytes);

    let magic = cur.take(4)?;
    if magic != INDEX_MAGIC {
        return Err(RagError::Index(format!("bad magic {magic:?}")));
    }
    let version = cur.take_u32()?;
    if version != INDEX_VERSION {
        return Err(RagError::Index(format!(
            "unsupported version {version} (expected {INDEX_VERSION})"
        )));
    }
    let count = cur.take_u32()? as usize;

    // Clamp only the pre-allocation HINT (the loop still reads exactly `count` records,
    // so a wrong/corrupt count still fails as a truncation error rather than over-allocating).
    let mut entries = Vec::with_capacity(count.min(INDEX_CAP_HINT));
    for _ in 0..count {
        let id = cur.take_u64()?;
        let mut vector = Vec::with_capacity(EMBED_DIM);
        let raw = cur.take(EMBED_DIM * 4)?;
        for chunk in raw.chunks_exact(4) {
            vector.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
        }
        // The reader guarantees EMBED_DIM*4 bytes; this is a belt-and-suspenders check
        // against the spec's invariant (a vector must be exactly EMBED_DIM-d).
        if vector.len() != EMBED_DIM {
            return Err(RagError::Index(format!(
                "entry id {id} has {}-d vector, expected {EMBED_DIM}-d",
                vector.len()
            )));
        }
        // v2: one tier byte AFTER the vector, BEFORE the text length prefix.
        let tier_level = cur.take_u8()?;
        let text_len = cur.take_u32()? as usize;
        let text = String::from_utf8(cur.take(text_len)?.to_vec())
            .map_err(|e| RagError::Index(format!("entry id {id} text not utf8: {e}")))?;
        let source_len = cur.take_u32()? as usize;
        let source = String::from_utf8(cur.take(source_len)?.to_vec())
            .map_err(|e| RagError::Index(format!("entry id {id} source not utf8: {e}")))?;
        entries.push(IndexEntry {
            id,
            vector,
            tier_level,
            text,
            source,
        });
    }

    Ok(entries)
}

/// Minimal forward byte cursor that maps any short read to [`RagError::Index`] (so a
/// truncated index file is a typed error, never a panic / out-of-bounds slice).
struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// Borrow the next `n` bytes, advancing the cursor; errors if fewer than `n` remain.
    fn take(&mut self, n: usize) -> Result<&'a [u8], RagError> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or_else(|| RagError::Index("length overflow reading index".to_string()))?;
        if end > self.buf.len() {
            return Err(RagError::Index(format!(
                "truncated index: need {n} bytes at offset {} but only {} remain",
                self.pos,
                self.buf.len().saturating_sub(self.pos)
            )));
        }
        let out = &self.buf[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    fn take_u8(&mut self) -> Result<u8, RagError> {
        Ok(self.take(1)?[0])
    }

    fn take_u32(&mut self) -> Result<u32, RagError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn take_u64(&mut self) -> Result<u64, RagError> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }
}

/// The RAG query state: an in-memory index + a lazily-initialized embedder.
///
/// Construction NEVER loads the model, so the UI is usable before warmup finishes
/// (rules.md: RAG model loading must NEVER gate app readiness). The embedder lives
/// behind a [`Mutex`] because [`Embedder::embed`] needs `&mut self`.
pub struct Rag {
    /// The full in-memory index, brute-force scanned by [`Self::rank`].
    index: Vec<IndexEntry>,
    /// The lazily-initialized embedder. `None` until a successful [`Self::warmup`].
    embedder: std::sync::Mutex<Option<Embedder>>,
    /// fastembed HF cache dir passed to [`Embedder::new`] at warmup (`None` = default).
    cache_dir: Option<std::path::PathBuf>,
}

impl Rag {
    /// Build from an already-loaded index. Does NOT init the model (`is_ready()` is
    /// false until [`Self::warmup`] succeeds).
    pub fn new(index: Vec<IndexEntry>, cache_dir: Option<std::path::PathBuf>) -> Self {
        Self {
            index,
            embedder: std::sync::Mutex::new(None),
            cache_dir,
        }
    }

    /// Load the `.bin` index from disk (no model init). Propagates a malformed file as
    /// [`RagError::Index`].
    pub fn from_index_file(
        path: &std::path::Path,
        cache_dir: Option<std::path::PathBuf>,
    ) -> Result<Self, RagError> {
        Ok(Self::new(load_index(path)?, cache_dir))
    }

    /// Number of indexed docs.
    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// True when the index is empty.
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// Look up one exact indexed chunk by its stable id.
    pub(crate) fn document(&self, id: u64) -> Option<&IndexEntry> {
        self.index.iter().find(|entry| entry.id == id)
    }

    /// True once the embedding model is loaded (after a successful [`Self::warmup`]).
    pub fn is_ready(&self) -> bool {
        self.embedder.lock().map(|g| g.is_some()).unwrap_or(false)
    }

    /// Initialize the embedder (blocking ONNX load ~570MB on first run). Emits
    /// `rag_warmup` progress via `emitter` on the PANEL's gate contract: the
    /// terminal detail is `"done"` on success (moves the gate to ready / unlocks
    /// send) or `"error: …"` on failure (fails the gate OPEN to unavailable); any
    /// other detail (e.g. the initial "loading …") keeps it on loading. Idempotent:
    /// if already ready, emits `"done"` and returns early. The CALLER runs this on a
    /// background thread (e.g. `tokio::task::spawn_blocking`) — it is NOT called on
    /// construction, so readiness never gates app start.
    pub fn warmup(&self, emitter: &dyn crate::bootstrap::ProgressEmitter) -> Result<(), RagError> {
        // Poisoned mutex shouldn't happen (the guard holds only an Option), but treat it
        // as an embed failure rather than panicking.
        let mut guard = self
            .embedder
            .lock()
            .map_err(|_| RagError::Embed("embedder mutex poisoned".to_string()))?;
        if guard.is_some() {
            emitter.emit("rag_warmup", 100, "done");
            return Ok(());
        }
        emitter.emit("rag_warmup", 0, "loading bge-m3 model");
        let embedder = match Embedder::new(self.cache_dir.clone()) {
            Ok(embedder) => embedder,
            Err(error) => {
                // Signal failure on the SAME contract the panel reads: a
                // `rag_warmup` detail starting with "error" fails the send gate
                // OPEN (unavailable) instead of leaving it stuck on "loading"
                // forever when the model never loads.
                emitter.emit("rag_warmup", 100, &format!("error: {error}"));
                return Err(error);
            }
        };
        *guard = Some(embedder);
        emitter.emit("rag_warmup", 100, "done");
        Ok(())
    }

    /// Brute-force tier-weighted ranking: `cosine(query_vec, each entry) *
    /// TIER_WEIGHT[tier_level]`, top-k by score desc (tie: lower `id`). `k` is clamped to
    /// [`MAX_TOP_K`]. Assumes `query_vec` is L2-normalized. No model needed — testable.
    /// Empty index -> empty Vec. The tier index is looked up DEFENSIVELY so a corrupt
    /// `tier_level` >= 4 falls back to a 1.0 weight instead of panicking (rules.md).
    pub fn rank(&self, query_vec: &[f32], k: usize) -> Vec<Hit> {
        let mut scored: Vec<(&IndexEntry, f32)> = self
            .index
            .iter()
            .map(|e| {
                let w = TIER_WEIGHT
                    .get(e.tier_level as usize)
                    .copied()
                    .unwrap_or(1.0);
                (e, cosine(query_vec, &e.vector) * w)
            })
            .collect();
        // Score desc; deterministic tie-break by lower id (matches `top_k`'s intent).
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.id.cmp(&b.0.id))
        });
        scored
            .into_iter()
            .take(k.min(MAX_TOP_K))
            .map(|(e, score)| Hit {
                id: e.id,
                tier_level: e.tier_level,
                match_kind: MatchKind::Semantic,
                text: e.text.clone(),
                source: e.source.clone(),
                score,
            })
            .collect()
    }

    /// Case-insensitive substring ("lexical") ranking over the in-memory index.
    ///
    /// Complements the dense-embedding [`Self::rank`] so exact identifiers and
    /// Korean terms (e.g. `chatEvent`, `0x58D900`, `버튼셋`) are found even when
    /// their dense-vector cosine does not reach the top-k. Needs NO embedding model,
    /// so it works during warmup. The query is split by [`tokenize_lexical`]; an
    /// entry matches when any term is a substring of its (lowercased) text. Ranked
    /// by distinct terms matched (desc), then total occurrences (desc), then `id`
    /// (asc, deterministic). `Hit::score` carries the occurrence count — a lexical
    /// relevance signal, NOT a cosine (hybrid places these ahead of cosine hits, so
    /// the two score scales never need to be comparable). `k` is clamped to
    /// [`MAX_TOP_K`]; no terms / empty index -> empty Vec.
    pub fn lexical_rank(&self, query: &str, k: usize) -> Vec<Hit> {
        let terms = tokenize_lexical(query);
        if terms.is_empty() {
            return Vec::new();
        }
        let mut scored: Vec<(&IndexEntry, usize, usize)> = Vec::new();
        for entry in &self.index {
            let haystack = entry.text.to_lowercase();
            let mut distinct = 0usize;
            let mut occurrences = 0usize;
            for term in &terms {
                let count = haystack.matches(term.as_str()).count();
                if count > 0 {
                    distinct += 1;
                    occurrences += count;
                }
            }
            if distinct > 0 {
                scored.push((entry, distinct, occurrences));
            }
        }
        // More distinct terms first, then more occurrences, then lower id (stable).
        scored.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.cmp(&a.2)).then(a.0.id.cmp(&b.0.id)));
        scored
            .into_iter()
            .take(k.min(MAX_TOP_K))
            .map(|(entry, _distinct, occurrences)| Hit {
                id: entry.id,
                tier_level: entry.tier_level,
                match_kind: MatchKind::Lexical,
                text: entry.text.clone(),
                source: entry.source.clone(),
                score: occurrences as f32,
            })
            .collect()
    }

    /// Hybrid retrieval: [`Self::lexical_rank`] first, then dense [`Self::search`]
    /// fills the remaining slots, deduped by stable chunk id, clamped to [`MAX_TOP_K`].
    ///
    /// This is what `search_docs` uses so a bare identifier query (`chatEvent`) is
    /// found by substring match even though its dense cosine misses the top-k, while
    /// conceptual Korean queries still get semantic hits. The lexical pass needs no
    /// model, so hits still surface while the embedder is warming up (the semantic
    /// pass then degrades to empty via `unwrap_or_default` rather than erroring).
    pub fn search_hybrid(&self, query: &str, k: usize) -> Vec<Hit> {
        let k = k.min(MAX_TOP_K);
        let mut out: Vec<Hit> = Vec::with_capacity(k);
        let mut seen: std::collections::HashSet<u64> = std::collections::HashSet::new();

        // Lexical hits take the leading slots (exact-match wins for identifiers).
        for hit in self.lexical_rank(query, k) {
            if out.len() >= k {
                break;
            }
            if seen.insert(hit.id) {
                out.push(hit);
            }
        }
        // Semantic hits fill what remains; a warming/failed embedder yields none.
        for hit in self.search(query, k).unwrap_or_default() {
            if out.len() >= k {
                break;
            }
            if seen.insert(hit.id) {
                out.push(hit);
            }
        }
        out
    }

    /// Embed the query with the model then [`Self::rank`]. Returns
    /// [`RagError::Warming`] if the model is not ready yet (never blocks waiting for
    /// it). Zero hits / empty index -> empty Vec.
    ///
    /// Uses `try_lock` (NEVER `lock`) so a query arriving WHILE [`Self::warmup`] holds
    /// the embedder lock across the ~570MB blocking load returns `Warming` immediately
    /// instead of blocking on it — search must never gate on model loading (rules.md).
    pub fn search(&self, query: &str, k: usize) -> Result<Vec<Hit>, RagError> {
        let mut guard = match self.embedder.try_lock() {
            Ok(g) => g,
            // The lock is held by an in-flight warmup: do NOT block — report Warming so
            // the caller retries once `rag_warmup` completes.
            Err(std::sync::TryLockError::WouldBlock) => return Err(RagError::Warming),
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err(RagError::Embed("embedder mutex poisoned".to_string()))
            }
        };
        let embedder = guard.as_mut().ok_or(RagError::Warming)?;
        let mut embedded = embedder.embed(&[query.to_string()])?;
        // `embed` returns one vector per input, already L2-normalized.
        let query_vec = embedded
            .pop()
            .ok_or_else(|| RagError::Embed("embed returned no vectors".to_string()))?;
        // Drop the embedder lock before ranking (rank only reads the index).
        drop(guard);
        Ok(self.rank(&query_vec, k))
    }

    /// Test-only: acquire the embedder lock to SIMULATE an in-flight warmup, so a test
    /// can prove `search` returns `Warming` (via `try_lock`) rather than blocking while
    /// the lock is held. Not part of the public API.
    #[cfg(test)]
    fn lock_embedder_for_test(&self) -> std::sync::MutexGuard<'_, Option<Embedder>> {
        self.embedder.lock().unwrap()
    }
}

#[cfg(test)]
mod parity {
    //! EUD-107 embedding parity spike.
    //!
    //! Embeds the committed fixture corpus + Korean queries with fastembed's
    //! INT8-quantized bge-m3 and compares the brute-force cosine top-5 against the
    //! Python `sentence-transformers` full-precision baseline (`baseline_top5`).
    //!
    //! `#[ignore]`d because the first run downloads the ~570MB model. Run with:
    //! `cargo test -p eud-agent rag::parity -- --ignored`.

    use super::*;
    use serde::Deserialize;

    /// One corpus entry from the fixture (only `text` is used for embedding; the
    /// other fields mirror the RAG chunk shape).
    #[derive(Debug, Deserialize)]
    struct CorpusItem {
        #[allow(dead_code)]
        id: u64,
        text: String,
        #[allow(dead_code)]
        title: String,
        #[allow(dead_code)]
        source: String,
    }

    /// The committed parity fixture (`tests/fixtures/rag_parity.json`).
    #[derive(Debug, Deserialize)]
    struct Fixture {
        #[allow(dead_code)]
        model: String,
        #[allow(dead_code)]
        normalized: bool,
        dim: usize,
        top_k: usize,
        corpus: Vec<CorpusItem>,
        queries: Vec<String>,
        /// Per-query Python sentence-transformers bge-m3 top-5 corpus indices.
        baseline_top5: Vec<Vec<usize>>,
    }

    fn load_fixture() -> Fixture {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/rag_parity.json"
        );
        let raw =
            std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read fixture {path}: {e}"));
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse fixture {path}: {e}"))
    }

    /// Overlap of two top-k index lists = size of the set intersection.
    fn overlap(a: &[usize], b: &[usize]) -> usize {
        a.iter().filter(|i| b.contains(i)).count()
    }

    /// Count of positions where both lists agree on the same index (strict order
    /// agreement of the rank), for the informational report line.
    fn order_agreement(a: &[usize], b: &[usize]) -> usize {
        a.iter().zip(b.iter()).filter(|(x, y)| x == y).count()
    }

    #[test]
    #[ignore = "downloads ~570MB bge-m3 model; run with --ignored (EUD-107 spike)"]
    fn top5_overlap_matches_python_baseline() {
        let fx = load_fixture();
        assert_eq!(fx.dim, EMBED_DIM, "fixture dim must match bge-m3 dense dim");
        let k = fx.top_k;
        assert_eq!(k, 5, "this spike compares top-5");
        assert_eq!(
            fx.queries.len(),
            fx.baseline_top5.len(),
            "one baseline row per query"
        );

        let mut embedder = Embedder::new(None).expect("init bge-m3 embedder");

        // Embed the whole corpus and all queries with fastembed (dense, normalized).
        let corpus_texts: Vec<String> = fx.corpus.iter().map(|c| c.text.clone()).collect();
        let corpus_emb = embedder.embed(&corpus_texts).expect("embed corpus");
        let query_emb = embedder.embed(&fx.queries).expect("embed queries");

        let mut overlaps: Vec<usize> = Vec::with_capacity(fx.queries.len());
        let mut pass_count = 0usize;

        for (q, qv) in query_emb.iter().enumerate() {
            let rust_top5 = top_k(qv, &corpus_emb, k);
            let baseline = &fx.baseline_top5[q];
            let ov = overlap(&rust_top5, baseline);
            let order = order_agreement(&rust_top5, baseline);
            overlaps.push(ov);
            if ov >= 4 {
                pass_count += 1;
            }
            eprintln!(
                "q{q}: {ov}/{k} overlap (order-agree {order}/{k})  rust={rust_top5:?}  python={baseline:?}"
            );
        }

        let mean = overlaps.iter().sum::<usize>() as f64 / overlaps.len() as f64;
        eprintln!(
            "EUD-107 parity: mean overlap {:.2}/{k}  ({pass_count}/{} queries >= 4/5)",
            mean,
            fx.queries.len()
        );

        // Criterion: top-5 overlap >= 4/5 for at least 8 of the 10 queries.
        assert!(
            pass_count >= 8,
            "embedding parity below threshold: only {pass_count}/{} queries had >=4/5 \
             top-5 overlap with the Python baseline (mean {mean:.2}/{k}). This is a \
             legitimate quantization-parity finding, not a test bug — see per-query \
             lines above.",
            fx.queries.len()
        );
    }
}

#[cfg(test)]
mod query {
    //! EUD-109 query-path contract (verify-first, STEP A).
    //!
    //! These tests pin the persisted-index + lazy-warmup query surface that STEP B
    //! will implement (`MAX_TOP_K`, `SEARCH_DOCS_GUIDANCE`, `RagError::{Warming,
    //! Index}`, `IndexEntry`, `Hit`, `write_index`/`load_index`, and the `Rag`
    //! state with `new`/`from_index_file`/`len`/`is_empty`/`is_ready`/`warmup`/
    //! `rank`/`search`). They currently FAIL TO COMPILE because that API does not
    //! exist yet — that is the intended failing artifact.
    //!
    //! No test here touches the real ~570MB model: construction never loads it, and
    //! `search_before_warmup_is_warming` proves a pre-warmup query returns
    //! `RagError::Warming` instead of blocking on a download (the real-model path is
    //! covered by the `#[ignore]`d `mod parity` test).

    use super::*;

    /// Build an `EMBED_DIM`-length vector with the given `(index, value)` pairs set
    /// (all other components zero), L2-normalized so cosine == dot. Avoids writing
    /// 1024 literals per fixture vector.
    fn vec_with(pairs: &[(usize, f32)]) -> Embedding {
        let mut v = vec![0.0f32; EMBED_DIM];
        for &(i, val) in pairs {
            v[i] = val;
        }
        l2_normalize(&mut v);
        v
    }

    /// Unique temp file path for a round-trip (no `tempfile` dev-dep). Caller removes.
    fn unique_temp_file(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("eud-agent-rag-test-{tag}-{nanos}.bin"))
    }

    /// Three entries with distinct ids/text/source/tier and distinct (separable)
    /// vectors. The `tier_level` values are spread across the L1/L2 source-tier range
    /// (and a 0) so the v2 layout/round-trip tests exercise a non-trivial tier byte.
    fn sample_entries() -> Vec<IndexEntry> {
        vec![
            IndexEntry {
                id: 1,
                vector: vec_with(&[(0, 1.0)]),
                tier_level: 1,
                text: "trigger location idiom".to_string(),
                source: "[ECA chunk 1](https://cafe/edac/1)".to_string(),
            },
            IndexEntry {
                id: 2,
                vector: vec_with(&[(1, 1.0)]),
                tier_level: 2,
                text: "button set disstr rule".to_string(),
                source: "[ECA chunk 2](https://cafe/edac/2)".to_string(),
            },
            IndexEntry {
                id: 3,
                vector: vec_with(&[(2, 1.0)]),
                tier_level: 0,
                text: "eps print idiom".to_string(),
                source: "[ECA chunk 3](https://cafe/edac/3)".to_string(),
            },
        ]
    }

    #[test]
    fn bin_roundtrip() {
        let entries = sample_entries();
        let path = unique_temp_file("roundtrip");

        write_index(&path, &entries).expect("write_index");
        let loaded = load_index(&path).expect("load_index");
        std::fs::remove_file(&path).ok();

        assert_eq!(loaded.len(), entries.len());
        for (got, want) in loaded.iter().zip(entries.iter()) {
            assert_eq!(got.id, want.id);
            assert_eq!(got.text, want.text);
            assert_eq!(got.source, want.source);
            // The v2 tier byte must survive write->load (it weights ranking in EUD-157).
            assert_eq!(
                got.tier_level, want.tier_level,
                "tier_level must round-trip exactly"
            );
            // Vectors must survive bit-for-bit (little-endian f32 records).
            assert_eq!(got.vector, want.vector, "vector must round-trip exactly");
        }
    }

    #[test]
    fn load_index_rejects_truncated() {
        let path = unique_temp_file("truncated");
        // Wrong magic + a few garbage bytes: no valid header/record can be parsed.
        std::fs::write(&path, b"XXXX\x01\x00").unwrap();

        let err = load_index(&path);
        std::fs::remove_file(&path).ok();

        assert!(
            matches!(err, Err(RagError::Index(_))),
            "a bad/truncated .bin must be RagError::Index, got {err:?}"
        );
    }

    /// Golden/differential v2 layout test: hand-build the EXACT expected byte vector
    /// from first principles (NEVER from `write_index`) and assert `write_index`
    /// reproduces it byte-for-byte. This independent reference IS the v2 wire-format
    /// parity contract that the CI index builder (EUD-156) must later match
    /// byte-for-byte — if this and the builder ever diverge, the published
    /// `rag-index.bin` becomes unloadable. Authoritative v2 layout (little-endian):
    ///   header: magic b"ERAG" | version u32 = 2 | count u32
    ///   entry:  id u64 | vector f32 x EMBED_DIM | tier_level u8 (1 byte)
    ///           | text(len u32 + utf8) | source(len u32 + utf8)
    /// The single `tier_level` byte sits AFTER the full vector and BEFORE the text
    /// length prefix.
    #[test]
    fn v2_byte_layout_is_exact() {
        let entries = sample_entries();
        let path = unique_temp_file("v2-layout");

        // Build the expected bytes BY HAND (do NOT call write_index to produce these).
        let mut expected: Vec<u8> = Vec::new();
        expected.extend_from_slice(b"ERAG");
        expected.extend_from_slice(&2u32.to_le_bytes()); // INDEX_VERSION = 2
        expected.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        for e in &entries {
            expected.extend_from_slice(&e.id.to_le_bytes());
            for f in &e.vector {
                expected.extend_from_slice(&f.to_le_bytes());
            }
            // tier_level: exactly one byte, AFTER the vector, BEFORE the text len.
            expected.push(e.tier_level);
            let text = e.text.as_bytes();
            expected.extend_from_slice(&(text.len() as u32).to_le_bytes());
            expected.extend_from_slice(text);
            let source = e.source.as_bytes();
            expected.extend_from_slice(&(source.len() as u32).to_le_bytes());
            expected.extend_from_slice(source);
        }

        write_index(&path, &entries).expect("write_index");
        let actual = std::fs::read(&path).expect("read back written index");
        std::fs::remove_file(&path).ok();

        assert_eq!(
            actual, expected,
            "write_index must emit the exact hand-built v2 wire layout"
        );
    }

    /// A v1-format file (well-formed header, version=1) must be REJECTED as a typed
    /// `RagError::Index` (never a panic) now that `INDEX_VERSION` is 2 — the existing
    /// `version != INDEX_VERSION` guard does this once the constant is bumped.
    #[test]
    fn load_index_rejects_v1() {
        let path = unique_temp_file("v1-reject");
        // Minimal but well-formed v1 header: magic + version=1 + count=0 (no records).
        let mut v1: Vec<u8> = Vec::new();
        v1.extend_from_slice(b"ERAG");
        v1.extend_from_slice(&1u32.to_le_bytes());
        v1.extend_from_slice(&0u32.to_le_bytes());
        std::fs::write(&path, &v1).unwrap();

        let err = load_index(&path);
        std::fs::remove_file(&path).ok();

        assert!(
            matches!(err, Err(RagError::Index(_))),
            "a v1 index must surface as RagError::Index (typed, no panic), got {err:?}"
        );
    }

    #[test]
    fn rank_orders_by_cosine() {
        let entries = sample_entries();
        let rag = Rag::new(entries, None);

        // Query points mostly at entry id=2's axis (index 1), a bit at id=1 (index 0),
        // and not at all toward id=3 (index 2). Expected order: 2, then 1, then 3.
        let q = vec_with(&[(1, 0.9), (0, 0.3)]);
        let hits = rag.rank(&q, 5);

        assert_eq!(hits.len(), 3);
        assert_eq!(hits[0].text, "button set disstr rule", "nearest is id=2");
        assert_eq!(hits[0].source, "[ECA chunk 2](https://cafe/edac/2)");
        assert_eq!(hits[0].id, 2);
        assert_eq!(hits[0].tier_level, 2);
        assert_eq!(hits[0].match_kind, MatchKind::Semantic);
        assert_eq!(hits[1].text, "trigger location idiom", "second is id=1");
        assert_eq!(
            hits[2].text, "eps print idiom",
            "third (orthogonal) is id=3"
        );

        // Scores descend and the top score matches the WEIGHTED cosine to id=2
        // (id=2 is tier 2, so its weight is TIER_WEIGHT[2]).
        assert!(hits[0].score >= hits[1].score && hits[1].score >= hits[2].score);
        let expected_top = cosine(&q, &vec_with(&[(1, 1.0)])) * super::TIER_WEIGHT[2];
        assert!(
            (hits[0].score - expected_top).abs() < 1e-5,
            "top score {} should match weighted cosine {}",
            hits[0].score,
            expected_top
        );
        // id=3 is orthogonal to the query -> ~0 cosine (0 * TIER_WEIGHT[0] still ~0).
        assert!(hits[2].score.abs() < 1e-5, "orthogonal entry scores ~0");
    }

    /// EUD-157 verify-first (STEP A): pin the tier-weighted ranking contract.
    ///
    /// `score = cosine(query, entry) * TIER_WEIGHT[entry.tier_level]`, where
    /// [`super::TIER_WEIGHT`] is a narrow band near 1.0 so a higher tier NUDGES a
    /// near-tie but can NEVER flip a large cosine gap. This references
    /// `super::TIER_WEIGHT` (which does not exist until STEP B), so it FAILS TO
    /// COMPILE — that IS the verify-first gate failure.
    #[test]
    fn rank_is_tier_weighted() {
        // --- Documented case 1: tier NUDGES a near-tie. ---
        // Two entries with nearly-equal cosine to the query but different tiers:
        // a tier-0 entry (weight TIER_WEIGHT[0]) almost perfectly aligned, and a
        // tier-3 entry (weight TIER_WEIGHT[3]) very slightly less aligned. The
        // tier-3 weight must overcome the tiny cosine deficit and rank first.
        let low_tier_high_cos = IndexEntry {
            id: 10,
            // Almost on the query axis (index 0) with a tiny off-axis component.
            vector: vec_with(&[(0, 1.0), (1, 0.02)]),
            tier_level: 0,
            text: "low tier, near-perfect cosine".to_string(),
            source: "[low](https://cafe/edac/10)".to_string(),
        };
        let high_tier_near_cos = IndexEntry {
            id: 11,
            // Slightly LESS aligned to the query axis than id=10.
            vector: vec_with(&[(0, 1.0), (1, 0.05)]),
            tier_level: 3,
            text: "high tier, slightly lower cosine".to_string(),
            source: "[high](https://cafe/edac/11)".to_string(),
        };
        let q = vec_with(&[(0, 1.0)]);

        // Sanity: the raw cosines are a near-tie, with the LOW-tier entry strictly
        // higher (so any reordering is attributable to the tier weight, not cosine).
        let cos_low = cosine(&q, &low_tier_high_cos.vector);
        let cos_high = cosine(&q, &high_tier_near_cos.vector);
        assert!(
            cos_low > cos_high,
            "fixture sanity: low-tier entry must have the higher raw cosine ({cos_low} vs {cos_high})"
        );

        let rag = Rag::new(
            vec![low_tier_high_cos.clone(), high_tier_near_cos.clone()],
            None,
        );
        let hits = rag.rank(&q, 5);
        assert_eq!(hits.len(), 2);
        assert_eq!(
            hits[0].text, "high tier, slightly lower cosine",
            "tier weight must nudge the higher-tier entry past a near-tie"
        );

        // The top hit's score is the WEIGHTED score of the tier-3 entry.
        let expected_high = cos_high * super::TIER_WEIGHT[3];
        assert!(
            (hits[0].score - expected_high).abs() < 1e-5,
            "top score {} should equal cosine * TIER_WEIGHT[3] ({expected_high})",
            hits[0].score
        );

        // --- Documented case 2: tier NEVER overpowers a large cosine gap. ---
        // A tier-0 entry strongly aligned to the query vs a tier-3 entry only
        // weakly aligned. The narrow weight band cannot flip the large cosine gap,
        // so the high-cosine low-tier entry still ranks first.
        let strong_low_tier = IndexEntry {
            id: 20,
            vector: vec_with(&[(0, 1.0)]),
            tier_level: 0,
            text: "strong cosine, low tier".to_string(),
            source: "[strong](https://cafe/edac/20)".to_string(),
        };
        let weak_high_tier = IndexEntry {
            id: 21,
            // Mostly off-axis: a much smaller cosine to the query.
            vector: vec_with(&[(0, 0.3), (1, 1.0)]),
            tier_level: 3,
            text: "weak cosine, high tier".to_string(),
            source: "[weak](https://cafe/edac/21)".to_string(),
        };

        // Sanity: even after the BEST possible tier boost, the weak entry's weighted
        // score stays below the strong entry's — the gap is large enough that the
        // band near 1.0 cannot flip it.
        let cos_strong = cosine(&q, &strong_low_tier.vector);
        let cos_weak = cosine(&q, &weak_high_tier.vector);
        assert!(
            cos_strong * super::TIER_WEIGHT[0] > cos_weak * super::TIER_WEIGHT[3],
            "fixture sanity: weighted strong-cosine entry must outrank weighted weak-cosine entry"
        );

        let rag2 = Rag::new(vec![strong_low_tier, weak_high_tier], None);
        let hits2 = rag2.rank(&q, 5);
        assert_eq!(hits2.len(), 2);
        assert_eq!(
            hits2[0].text, "strong cosine, low tier",
            "tier weight must NOT flip a large cosine gap"
        );
    }

    #[test]
    fn rank_clamps_k() {
        let q = vec_with(&[(0, 1.0)]);

        // Small index: k larger than the corpus returns every entry (3).
        let small = Rag::new(sample_entries(), None);
        assert_eq!(small.rank(&q, 100).len(), 3);

        // Large index (> MAX_TOP_K): k is clamped to MAX_TOP_K.
        let big_entries: Vec<IndexEntry> = (0..(MAX_TOP_K + 5) as u64)
            .map(|i| IndexEntry {
                id: i,
                vector: vec_with(&[((i as usize) % EMBED_DIM, 1.0)]),
                tier_level: 0,
                text: format!("doc {i}"),
                source: format!("[doc {i}](https://cafe/edac/{i})"),
            })
            .collect();
        let big = Rag::new(big_entries, None);
        assert_eq!(big.rank(&q, 100).len(), MAX_TOP_K);
    }

    #[test]
    fn empty_index_returns_empty() {
        let rag = Rag::new(vec![], None);
        let q = vec_with(&[(0, 1.0)]);

        assert!(rag.is_empty());
        assert_eq!(rag.len(), 0);
        assert!(rag.rank(&q, 5).is_empty());
    }

    #[test]
    fn search_before_warmup_is_warming() {
        // Construction must NOT load the model, so the index is ready but the embedder
        // is not — a query before warmup returns Warming instead of blocking.
        let rag = Rag::new(sample_entries(), None);

        assert!(!rag.is_ready(), "construction must not load the model");
        let res = rag.search("위치 트리거", 5); // a Korean query
        assert!(
            matches!(res, Err(RagError::Warming)),
            "search before warmup must be RagError::Warming, got {res:?}"
        );
    }

    #[test]
    fn search_docs_guidance_mentions_korean() {
        // The v1 guidance is preserved as a single source of truth and clamps to MAX_TOP_K.
        // (Compile-time pin so clippy doesn't flag a constant runtime assertion.)
        const _: () = assert!(MAX_TOP_K == 10, "v1 clamped top-k to 10");
        assert!(
            SEARCH_DOCS_GUIDANCE.contains("Korean") || SEARCH_DOCS_GUIDANCE.contains("한"),
            "guidance must surface the Korean-query advice"
        );
    }

    #[test]
    fn search_during_warmup_does_not_block() {
        // Simulate an in-flight warmup by holding the embedder lock on THIS thread, then
        // call `search` on the SAME thread. With `try_lock`, search returns Warming
        // immediately (WouldBlock) instead of deadlocking on the held lock — proving it
        // never gates on model loading. No model, no sleeps, fully deterministic.
        let rag = Rag::new(sample_entries(), None);
        let _held = rag.lock_embedder_for_test(); // lock held for the rest of the test

        let res = rag.search("위치 트리거", 5);
        assert!(
            matches!(res, Err(RagError::Warming)),
            "search during warmup must be RagError::Warming (non-blocking), got {res:?}"
        );
    }

    /// One index entry with arbitrary text on a fixed (shared) vector — the vector is
    /// irrelevant to the lexical/hybrid tests, which never touch the embedder.
    fn entry_with_text(id: u64, text: &str) -> IndexEntry {
        IndexEntry {
            id,
            vector: vec_with(&[(0, 1.0)]),
            tier_level: 1,
            text: text.to_string(),
            source: format!("[doc {id}](https://cafe/edac/{id})"),
        }
    }

    #[test]
    fn tokenize_lexical_splits_and_filters() {
        // A Korean particle glued to an identifier must not defeat the substring match.
        assert_eq!(
            tokenize_lexical("chatEvent에 대해"),
            vec!["chatevent".to_string(), "대해".to_string()]
        );
        // Single-char runs (particles, stray letters) are dropped.
        assert!(tokenize_lexical("a 의 b").is_empty());
        // Hex + underscored identifiers survive intact (and lowercase ASCII).
        assert_eq!(
            tokenize_lexical("0x58D900 __addr__"),
            vec!["0x58d900".to_string(), "__addr__".to_string()]
        );
        // Duplicate terms are removed, first-seen order kept.
        assert_eq!(
            tokenize_lexical("chatEvent chatevent"),
            vec!["chatevent".to_string()]
        );
    }

    #[test]
    fn lexical_rank_finds_identifier_substring() {
        let entries = vec![
            entry_with_text(
                1,
                "제목: 미궁\n\n[chatEvent]\n__addr__: 0x58D900 chatEvent 사용",
            ),
            entry_with_text(2, "제목: 무관\n\n버튼셋 이야기"),
        ];
        let rag = Rag::new(entries, None);

        let hits = rag.lexical_rank("chatEvent", 5);
        assert_eq!(hits.len(), 1, "only the chatEvent chunk matches");
        assert!(hits[0].text.contains("chatEvent"));
        // score is the occurrence count: "chatevent" appears twice in entry 1.
        assert_eq!(hits[0].score, 2.0);
        assert_eq!(hits[0].id, 1);
        assert_eq!(hits[0].match_kind, MatchKind::Lexical);

        // A query with no usable terms (all sub-min-length) returns nothing.
        assert!(rag.lexical_rank("a 의", 5).is_empty());
    }

    #[test]
    fn search_hybrid_surfaces_lexical_before_warmup() {
        // Acceptance mirror: search_docs("chatEvent") must return >= 1 hit even though
        // the embedder is not warmed. The semantic pass returns Warming -> empty, but
        // the lexical pass still finds the chatEvent chunk (no model needed).
        let entries = vec![
            entry_with_text(1, "제목: 미궁 퍼즐맵\n\n[chatEvent] __addr__: 0x58D900"),
            entry_with_text(2, "제목: 다른 글\n\n관계 없는 내용"),
        ];
        let rag = Rag::new(entries, None);
        assert!(!rag.is_ready(), "construction must not load the model");

        let hits = rag.search_hybrid("chatEvent", 5);
        assert!(
            hits.iter().any(|hit| hit.text.contains("chatEvent")),
            "hybrid must surface the chatEvent chunk via lexical match, got {hits:?}"
        );
    }
}
