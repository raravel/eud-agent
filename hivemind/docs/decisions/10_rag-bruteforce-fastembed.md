# Decision 10: RAG = in-memory brute-force cosine + fastembed bge-m3

- Date: 2026-06-08
- Status: Accepted
- Context: The ECA RAG corpus is `BAAI/bge-m3`, 1024d cosine, **4,974 documents**
  (chromadb_bge, 121MB on disk; raw vectors ~20MB). The Python path used
  `sentence-transformers` + chromadb in-process. Rust must replace both.
- Considered:
  - In-memory brute-force cosine over a prebuilt vector file (Recommended) —
    Pros: zero ANN dependency; 4,974×1024 is sub-millisecond per query; leanest
    binary. Cons: O(N) scan (negligible at this scale); needs revisiting if the
    corpus grows ~100x. ★★★.
  - usearch HNSW + sqlite metadata — Pros: sublinear ANN, scales. Cons: extra
    native dependency, unnecessary at this scale. ★★☆.
  - LanceDB embedded — Pros: vectors + metadata + filters in one store. Cons:
    heavy Arrow/datafusion dependency tree, fights the lean-binary goal. ★☆☆.
- Chosen: `fastembed` 5.15 (bge-m3 ONNX via `ort`) embeds the query at runtime;
  the corpus vectors + chunk text + `source:` link metadata ship as a prebuilt
  index; search is in-memory brute-force cosine.
- Rationale: The corpus is tiny; an ANN index is over-engineering and adds weight
  the single-binary goal cannot afford.
- CRITICAL — embedding parity: the index MUST be built in CI with the SAME
  fastembed pipeline used at query time, NOT a raw export of the chromadb vectors.
  bge-m3's `sentence-transformers` output (pooling/normalization) and fastembed's
  quantized `BGEM3Q` ONNX output must occupy the same space or retrieval breaks. A
  spike validates top-k agreement against the Python pipeline before adoption.
- Impact: tech-stack.md, rules.md, feature 12.
