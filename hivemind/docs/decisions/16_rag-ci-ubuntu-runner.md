# Decision NN: Migrate build-rag-index CI to ubuntu-latest

- Date: 2026-06-10
- Status: Accepted
- Context: The `build-rag-index` workflow (feature 16) runs on `windows-latest` and takes
  ~2.5-3.5h per run. Two cost drivers: (1) the fastembed bge-m3 model (~570MB) is
  re-downloaded every run (only cargo is cached via Swatinem/rust-cache), and (2) CPU-only
  embedding of ~8445 chunks on a 4-vCPU runner. A local Ryzen 7800X3D build takes ~36 min.
  A runner-strategy fork was hit while planning the optimization.
- Considered:
  - ubuntu-latest — Pros: Linux minutes billed 1x (Windows 2x), less OS overhead. Cons:
    port the manifest PowerShell step to bash, drop the `.exe` suffix on the builder path,
    confirm ort/fastembed build on Linux. Recommendation: ★★★ — best cost/effort ratio;
    combined with a model cache it removes the download waste.
  - windows-latest (keep) — Pros: minimal-risk, shell/paths unchanged. Cons: embedding
    compute still slow + 2x billing; model cache only saves the download. Recommendation: ★☆☆.
  - Larger/GPU runner — Pros: max embedding speedup (near local). Cons: paid runners (cost),
    GPU adds ort CUDA setup complexity. Recommendation: ★☆☆ — only if budget allows.
- Chosen: ubuntu-latest
- Rationale: Standard runners cannot reach local speed, so the realistic win is cost (Linux
  1x billing) plus eliminating the repeated 570MB download via an actions/cache step; ORT
  thread/batch tuning applies on top regardless of OS. GPU/large runners were declined to
  avoid paid infra.
- Impact: .github/workflows/build-rag-index.yml (runner, bash manifest step, builder path,
  model-cache step), ci/build_rag_index.rs (embedding batch/thread tuning),
  features/16_rag-corpus-pipeline.md (Embed/CI note updated by the task harness sync).
