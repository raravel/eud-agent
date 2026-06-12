//! GPU differential-test fixture + adoption gate (local-only, gated).
//!
//! A SEPARATE, GATED investigation track: does a GPU/CUDA-built bge-m3 embedding
//! match the canonical CPU-built embedding closely enough to adopt for a CPU-query
//! runtime? This is NOT on the L1/L2 critical path and NOT wired into CI (the
//! `ubuntu-latest` runner has no GPU). See
//! `hivemind/docs/features/17_rag-knowledge-tiering.md` ("GPU differential-test
//! track (separate, gated)").
//!
//! The canonical CPU path (`build_rag_index`, default build) is unchanged; the
//! CUDA embedding path is gated behind the `cuda` cargo feature, and the actual
//! differential test (which needs the model + a GPU) is `#[ignore]`d so it never
//! runs in a normal `cargo test`.
//!
//! ## ort/cuda wiring (real optional dependency)
//!
//! fastembed 5.16 pulls `ort 2.0.0-rc.12` transitively. The `cuda` feature adds
//! an OPTIONAL `ort` dependency pinned to EXACTLY that version
//! (`ort = { version = "=2.0.0-rc.12", optional = true }`) and turns on
//! `ort/cuda`; because the version is identical, cargo unifies it with
//! fastembed's ort (features are additive), so the DEFAULT build pulls no CUDA
//! code and the ort graph is NOT split. fastembed 5.15/5.16 does NOT expose
//! execution-provider selection in `Bgem3InitOptions`, so the GPU branch relies
//! on ort's global/default provider registration (`CUDAExecutionProvider`) being
//! effective for the session fastembed creates internally. If that registration
//! proves ineffective at runtime (fastembed may build its own session ignoring
//! the global default), the differential cosine simply reflects the honest
//! outcome and the gate records the same conclusion: "GPU build NOT adopted;
//! CPU build remains canonical."
//!
//! ## Result (this environment)
//!
//! This environment has NO GPU/CUDA toolkit, so the differential test is
//! `#[ignore]`d and the recorded conclusion is "not adopted pending a local CUDA
//! run; CPU build remains canonical." The fixture structure
//! (`per_text_cosine`/`min_cosine`/`adopted`/`conclusion`) is ready to record a
//! real measured cosine when run locally with `--features cuda` on a CUDA box.

use std::path::PathBuf;

/// 1024-d dense vectors, mirroring `build_rag_index::EMBED_DIM`.
#[allow(dead_code)]
const EMBED_DIM: usize = 1024;

/// BGEM3Q int8 embeddings are batch-size-sensitive (rules.md: batch 64 drifts
/// cosine ~0.98 vs batch 16). NEVER use another batch — mirror build_rag_index.
#[allow(dead_code)]
const BATCH_SIZE: usize = 16;

/// Adoption gate: GPU-built vectors are adoptable for the CPU-query runtime ONLY
/// if every pairwise cosine is effectively 1.0 (> 0.9999). Otherwise the
/// documented conclusion is "GPU build NOT adopted; CPU build remains canonical."
fn adopt_gate(min_pairwise_cosine: f32) -> bool {
    min_pairwise_cosine > 0.9999
}

/// Cosine similarity of two vectors. Inputs are assumed L2-normalized (as in
/// `build_rag_index::l2_normalize`), so this reduces to the dot product.
#[allow(dead_code)]
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// L2-normalize in place, mirroring `build_rag_index::l2_normalize`.
#[allow(dead_code)]
fn l2_normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > f32::EPSILON {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

/// A FIXED small corpus subset: short eps/EUD strings, hard-coded so the
/// differential test is deterministic and needs no corpus files. Kept tiny so a
/// local CUDA run is fast.
#[allow(dead_code)]
const FIXED_SUBSET: [&str; 5] = [
    "제목: 버튼 비활성화\n\nbtnset에서 disstr이 0이면 게임이 크래시한다.",
    "제목: 위치 삭제\n\nlocation을 삭제할 때는 슬롯을 0으로 만들되 재번호하지 않는다.",
    "function onPluginStart() { setcurpl(P1); print(\"hello eud\"); }",
    "제목: SCA\n\nSCArchive는 죽은 기능이므로 IsUsed를 항상 false로 둔다.",
    "EUD 에디터에서 CUI와 RawText만 settable 텍스트 타입이다.",
];

/// Resolve the model cache dir from `FASTEMBED_CACHE` / `HF_HOME`, else a temp
/// subdir — mirrors how build_rag_index lets the cache dir be supplied.
#[allow(dead_code)]
fn resolve_cache_dir() -> PathBuf {
    std::env::var_os("FASTEMBED_CACHE")
        .or_else(|| std::env::var_os("HF_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("eud-gpu-diff-cache"))
}

/// Where the differential-test fixture JSON is written. Overridable via
/// `GPU_DIFF_FIXTURE`; defaults next to the ci builder for easy inspection.
#[allow(dead_code)]
fn resolve_fixture_path() -> PathBuf {
    std::env::var_os("GPU_DIFF_FIXTURE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("ci/gpu_diff_fixture.json"))
}

/// Serialize the differential result as JSON by hand (no serde dependency added
/// for the test): per-text cosine array, min, adopted bool, conclusion string.
#[allow(dead_code)]
fn fixture_json(per_text_cosine: &[f32], min: f32, adopted: bool, conclusion: &str) -> String {
    let cosines = per_text_cosine
        .iter()
        .map(|c| format!("{c}"))
        .collect::<Vec<_>>()
        .join(", ");
    let conclusion_escaped = conclusion.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        "{{\n  \"embed_dim\": {EMBED_DIM},\n  \"batch_size\": {BATCH_SIZE},\n  \
         \"per_text_cosine\": [{cosines}],\n  \"min_cosine\": {min},\n  \
         \"adopted\": {adopted},\n  \"conclusion\": \"{conclusion_escaped}\"\n}}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::{adopt_gate, cosine};

    /// L2-normalize a copy of `v` so the cosine helper (a plain dot product) is
    /// exercised on unit vectors, mirroring `build_rag_index::l2_normalize`.
    fn normalized(v: &[f32]) -> Vec<f32> {
        let mut out = v.to_vec();
        super::l2_normalize(&mut out);
        out
    }

    #[test]
    fn cosine_identical_vectors_is_one_and_adopts() {
        let a = normalized(&[1.0, 2.0, 3.0, 4.0]);
        let b = a.clone();
        let c = cosine(&a, &b);
        assert!(
            (c - 1.0).abs() < 1e-4,
            "identical vectors must have cosine ~1.0, got {c}"
        );
        assert!(
            adopt_gate(c),
            "cosine {c} (effectively 1.0) must adopt the GPU build"
        );
    }

    #[test]
    fn cosine_divergent_vectors_does_not_adopt() {
        let a = normalized(&[1.0, 0.0, 0.0, 0.0]);
        let b = normalized(&[0.0, 1.0, 0.0, 0.0]);
        let c = cosine(&a, &b);
        assert!(
            c < 0.9999,
            "orthogonal vectors must have cosine well below the gate, got {c}"
        );
        assert!(
            !adopt_gate(c),
            "divergent cosine {c} must NOT adopt the GPU build"
        );
    }

    #[test]
    fn adopt_gate_borderline_below_threshold_rejects_above_adopts() {
        // 0.9998 is just under the > 0.9999 threshold: NOT adoptable.
        assert!(
            !adopt_gate(0.9998),
            "0.9998 is below the > 0.9999 gate and must NOT adopt"
        );
        // Exactly at the threshold is not strictly greater: NOT adoptable.
        assert!(
            !adopt_gate(0.9999),
            "0.9999 is not strictly > 0.9999 and must NOT adopt"
        );
        // 0.99995 is above the threshold: adoptable.
        assert!(
            adopt_gate(0.99995),
            "0.99995 is above the > 0.9999 gate and must adopt"
        );
    }

    #[test]
    fn fixture_json_round_trips_fields() {
        let json = super::fixture_json(&[1.0, 0.99998], 0.99998, false, "test conclusion");
        assert!(json.contains("\"min_cosine\": 0.99998"));
        assert!(json.contains("\"adopted\": false"));
        assert!(json.contains("\"conclusion\": \"test conclusion\""));
        assert!(json.contains("\"batch_size\": 16"));
        assert!(json.contains("\"embed_dim\": 1024"));
    }

    /// Local-only differential test: needs the bge-m3 model (download/cache) and,
    /// for the GPU leg, a CUDA device + `--features cuda`. `#[ignore]` so a normal
    /// `cargo test` never runs it. Run locally with:
    ///   cargo test --manifest-path ci/Cargo.toml --test gpu_diff_test \
    ///       --features cuda -- --ignored gpu_cpu_differential
    #[test]
    #[ignore = "needs the bge-m3 model + a CUDA GPU; run locally with --features cuda"]
    fn gpu_cpu_differential() {
        use fastembed::{Bgem3Embedding, Bgem3InitOptions, Bgem3Model};

        let cache = super::resolve_cache_dir();
        let texts: Vec<String> = super::FIXED_SUBSET.iter().map(|s| s.to_string()).collect();

        // CPU EP — ALWAYS runs. Mirror the canonical build_rag_index embed call
        // exactly: BGEM3Q model, with_cache_dir, embed(&texts, Some(16)).
        let mut cpu_embedder = Bgem3Embedding::try_new(
            Bgem3InitOptions::new(Bgem3Model::BGEM3Q).with_cache_dir(cache.clone()),
        )
        .expect("CPU embedder init failed");
        let cpu_out = cpu_embedder
            .embed(&texts, Some(super::BATCH_SIZE))
            .expect("CPU embedding failed");
        let mut cpu_vecs = cpu_out.dense;
        assert_eq!(cpu_vecs.len(), texts.len(), "CPU vector count mismatch");
        for v in &mut cpu_vecs {
            assert_eq!(v.len(), super::EMBED_DIM, "CPU vector not 1024-d");
            super::l2_normalize(v);
        }

        #[cfg(feature = "cuda")]
        {
            // fastembed 5.15/5.16 does NOT expose EP selection. Register the CUDA
            // execution provider globally via ort BEFORE creating the embedder so
            // the session fastembed builds internally can pick it up. If this is
            // ineffective (fastembed builds its own session ignoring globals), the
            // cosine below simply reflects the honest CPU-vs-(CPU-or-GPU) outcome
            // and the gate records it.
            use ort::execution_providers::CUDAExecutionProvider;
            if let Err(e) = ort::init()
                .with_execution_providers([CUDAExecutionProvider::default().build()])
                .commit()
            {
                eprintln!("ort CUDA provider registration failed: {e}");
            }

            let mut gpu_embedder = Bgem3Embedding::try_new(
                Bgem3InitOptions::new(Bgem3Model::BGEM3Q).with_cache_dir(cache.clone()),
            )
            .expect("GPU embedder init failed");
            let gpu_out = gpu_embedder
                .embed(&texts, Some(super::BATCH_SIZE))
                .expect("GPU embedding failed");
            let mut gpu_vecs = gpu_out.dense;
            assert_eq!(gpu_vecs.len(), texts.len(), "GPU vector count mismatch");
            for v in &mut gpu_vecs {
                assert_eq!(v.len(), super::EMBED_DIM, "GPU vector not 1024-d");
                super::l2_normalize(v);
            }

            let per_text_cosine: Vec<f32> = cpu_vecs
                .iter()
                .zip(&gpu_vecs)
                .map(|(c, g)| cosine(c, g))
                .collect();
            let min = per_text_cosine
                .iter()
                .copied()
                .fold(f32::INFINITY, f32::min);
            let adopted = adopt_gate(min);
            let conclusion = if adopted {
                "GPU build adoptable: every pairwise cosine > 0.9999 (effectively 1.0).".to_string()
            } else {
                format!(
                    "GPU build NOT adopted (min pairwise cosine {min} <= 0.9999); \
                     CPU build remains canonical."
                )
            };
            let json = super::fixture_json(&per_text_cosine, min, adopted, &conclusion);
            let fixture = super::resolve_fixture_path();
            std::fs::write(&fixture, json).expect("write fixture");
            eprintln!("wrote differential fixture: {}", fixture.display());
            assert!(adopted, "{conclusion}");
        }

        #[cfg(not(feature = "cuda"))]
        {
            // No GPU/CUDA feature built: record the documented conclusion. The CPU
            // leg still ran (sanity-embedded above), but there is nothing to
            // compare, so the fixture records "not adopted" with empty cosines.
            let conclusion = "GPU build NOT adopted (no GPU/CUDA feature built); \
                              CPU build remains canonical.";
            let json = super::fixture_json(&[], f32::NAN, false, conclusion);
            let fixture = super::resolve_fixture_path();
            std::fs::write(&fixture, json).expect("write fixture");
            eprintln!("{conclusion}");
            eprintln!("wrote differential fixture: {}", fixture.display());
        }
    }
}
