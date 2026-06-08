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
