---
task_id: EUD-017-e827
completed_at: 2026-06-04T19:48:14
duration_minutes: 40
coding_retries: 0
verify_retries: 0
review_rounds: 0
verification_required: true
verification_passed: true
blocking_issues: false
review_scores:
  correctness: 9
  spec_compliance: 10
  safety: 8
  clarity: 10
tokens:
  estimated: true
  input: 3060
  output: 4910
cost_usd: 0.42
profile: quality
models:
  executor: claude-opus-4-7
  reviewer: claude-opus-4-7
---

## Summary
`server/eud_agent/rag.py` (Decision 01, in-process): lazy double-checked-locked singleton over patchable seams (_load_model/_load_collection/_cuda_available — heavy imports deferred inside; module import stays light, 0.04s collection), start_warmup daemon thread reporting rag_warmup started→done/error via a defensive callback, search(query, k=5, rag_db) → [{title,url,distance,text}] with None-tolerant row-0 mapping, explicit cuda/cpu device, RagUnavailable with memoized failure + reset() escape hatch, read-only chroma access (get_collection + query only).

## Changes
- `server/eud_agent/rag.py` — new (363 lines)
- `server/tests/test_rag.py` — 19 tests (18 stubbed + 1 live-gated via EUD_RAG_LIVE=1): lazy single-load, 6-thread race, warmup ordering/non-blocking, progress sequence, bad-db short-circuit, shape mapping

## Verification
- Two-phase gate: Step A RED (collection ImportError) confirmed by orchestrator before Step B; GREEN after — 168 passed + 2 skipped + ruff clean, re-run independently by orchestrator.
- LIVE smoke run independently by orchestrator: real ECA DB + bge-m3 CUDA load, query "유닛 체력 설정" → 5 topically-relevant results, finite cosine distances (~0.32-0.33); load+first-search 12.8s, warm search 0.015s (the resident-process amortization Decision 01 targeted).
- Ready-not-gated proven: start_warmup returns immediately (test with slow stub loader).
- Scope-drift gate: 2 paths, both declared (scope narrowed from the dishonest server/tests/** by orchestrator pre-spawn — that broad scope had deferred EUD-030/EUD-031 in scheduling).

## Review
Verdict PASS (9/10/8/10), no blocking findings. Reviewer verified: double-checked locking sound (assignment only after loaders return; re-check inside lock); warmup holds the lock for the whole load (correct trade-off — no double-load window); progress states complete for the WS protocol; stub faithfully models chromadb 1.5.9 QueryResult; race tests deterministic (invariant-based, not timing-based); telemetry default is a NO-OP in pinned 1.5.9 (Posthog.capture is pass — verified). Advisories: zip(strict=False) silently truncates on malformed responses (deliberate degrade; a debug log would help); is_loaded() reads outside the lock (benign, advisory uses only).

## Harness Sync
- no-op (skip condition): rag.py already in features/02 ## Implementation; test excluded; no manifest. Contract-drift clean.

## Notes
- CARRY-FORWARD for the app/orchestrator tasks: (1) transient load failures poison RAG until `rag.reset()` — spec-compliant memoization, but if transient recovery is ever wanted the app must wire a reset() caller; (2) optionally pass `Settings(anonymized_telemetry=False)` to PersistentClient for version-proof no-phone-home (no-op today under the 1.5.9 pin).
- Concurrency note: single CUDA model serializes concurrent encodes (GIL + GPU) — fine for the one-in-flight-instruct orchestrator model.
- Raw harness-reported subagent tokens ≈ 204,964 (68,860 + 82,283 + 53,821).
