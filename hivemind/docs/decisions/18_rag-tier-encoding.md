# Decision 18: RAG tier stored as u8 level code, weight resolved in code

- Date: 2026-06-12
- Status: Accepted
- Context: While planning RAG knowledge tiering (L2), the v2 index must carry a per-chunk source-trust signal so `rank()` can down-weight unverified Q&A posts relative to official manuals. The fork: store a raw `f32` weight per entry, or store a compact `u8` tier-level code and resolve the multiplier in code at query time.
- Considered:
  - u8 tier-level code — Pros: weight re-tuning is a code-only change (no index republish), policy/data separation. Cons: rank() needs a level→weight lookup table. Recommendation: ★★★.
  - f32 weight stored directly — Pros: rank() just multiplies the stored value. Cons: weight policy is baked into data; re-tuning forces a full index rebuild + republish. Recommendation: ★☆☆.
- Chosen: u8 tier-level code
- Rationale: The tier *level* of a source (official / lecture / general / Q&A) is stable, but the exact multiplier that keeps tier from overpowering the cosine signal is an empirical value that will be tuned by measurement. Baking only the stable level into the index and keeping the multiplier as a code constant (`TIER_WEIGHT[level]`) lets us re-tune without re-embedding or republishing the index asset.
- Impact: features/17_rag-knowledge-tiering.md (v2 entry layout, rank formula), src-tauri/src/rag.rs (IndexEntry.tier_level + load/write + rank), ci/build_rag_index.rs (tier derivation + v2 write), the v1→v2 migration path.
