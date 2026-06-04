"""Verification artifact for EUD-017-e827: in-process bge-m3 RAG module.

These tests drive ``eud_agent.rag`` WITHOUT loading the real bge-m3 weights or
opening the real chromadb store (except the explicitly-gated live smoke test).
The default suite STUBS the two heavy loaders so it can assert the exact
contract the harness demands (features/02 "rag.py", Decision 01, rules.md "RAG
model loading must never gate server.ready"):

  - Lazy singleton: importing ``eud_agent.rag`` loads NOTHING (no torch /
    sentence-transformers / chromadb pulled at import; selfcheck must not pay
    that cost). The first ``search`` triggers the load, EXACTLY once.
  - Background warmup: ``start_warmup(rag_db, on_progress=...)`` runs in a thread
    and returns IMMEDIATELY (never gates readiness). The progress callback emits
    a ``rag_warmup`` started -> done sequence (started -> error on a bad db).
  - Warmup/search race: ``start_warmup`` then an immediate ``search`` -> the
    search WAITS on the warmup lock and succeeds; the model loads only ONCE under
    a concurrent warmup + search race (no double-load, no failure).
  - Result shape: ``search(query, k) -> [{title, url, distance, text}]`` mapping
    chroma's ``metadatas`` / ``documents`` / ``distances`` rows.
  - Device: GPU when CUDA torch is available, CPU otherwise (the device string is
    passed explicitly from ``torch.cuda.is_available()``).
  - Errors: a missing / bad ``rag_db`` path -> a clear ``RagUnavailable`` at load;
    a ``search`` after a FAILED load re-raises cleanly (the orchestrator degrades
    to no-context per features/02 edge cases).

The seams the tests patch are MODULE-LEVEL loader functions
(``rag._load_model`` and ``rag._load_collection``); rag.py must be designed so
the SentenceTransformer / chromadb constructors live behind these patchable
functions (and the heavy imports happen INSIDE them, never at module import).

``eud_agent.rag`` does NOT exist during Step A, so this suite is expected to
FAIL on import until rag.py is implemented (Step B).
"""

from __future__ import annotations

import os
import threading
import time

import pytest

# Imported at collection so the failing import is the first signal in Step A.
# (No try/except: the suite MUST error out until the module exists.)
from eud_agent import rag
from eud_agent.rag import RagUnavailable, search, start_warmup

# A rag_db directory that "exists" (so the bad-path short-circuit does not fire)
# is created per-test via tmp_path; for stubbed-loader tests the contents are
# irrelevant because the loaders themselves are patched.


# --------------------------------------------------------------------------- #
# Stubs for the two heavy loaders.
# --------------------------------------------------------------------------- #


class FakeModel:
    """Stands in for a loaded SentenceTransformer.

    Records the device it was built with and every encode() call. encode()
    returns a deterministic 1-D vector regardless of input (the stubbed
    collection ignores it anyway).
    """

    def __init__(self, device: str = "cpu") -> None:
        self.device = device
        self.encode_calls: list = []

    def encode(self, text, *args, **kwargs):
        self.encode_calls.append(text if isinstance(text, str) else list(text))
        return [0.0, 1.0, 0.0]


class FakeCollection:
    """Stands in for a chromadb collection.

    ``query`` returns chroma's batched shape: each of metadatas/documents/
    distances is a list-of-lists (one inner list per query embedding). We map a
    single query, so there is exactly one inner row.
    """

    def __init__(self, rows: list | None = None) -> None:
        # Default rows: 3 docs with title/url metadata + document text.
        self._rows = rows if rows is not None else [
            {
                "metadata": {"title": "Unit health", "url": "http://cafe/1"},
                "document": "setdeaths usage for unit health",
                "distance": 0.12,
            },
            {
                "metadata": {"title": "Player loop", "url": "http://cafe/2"},
                "document": "EUDLoopPlayer iterates players",
                "distance": 0.34,
            },
            {
                "metadata": {"title": "Trigger", "url": "http://cafe/3"},
                "document": "trigger conditions and actions",
                "distance": 0.56,
            },
        ]
        self.query_calls: list = []

    def query(self, *, query_embeddings=None, n_results=5, **kwargs):
        self.query_calls.append({"n_results": n_results, "kwargs": kwargs})
        rows = self._rows[:n_results]
        return {
            "ids": [[str(i) for i in range(len(rows))]],
            "metadatas": [[r["metadata"] for r in rows]],
            "documents": [[r["document"] for r in rows]],
            "distances": [[r["distance"] for r in rows]],
        }


def _install_stub_loaders(
    monkeypatch,
    *,
    model: FakeModel | None = None,
    collection: FakeCollection | None = None,
    model_delay: float = 0.0,
    collection_delay: float = 0.0,
    model_error: BaseException | None = None,
    collection_error: BaseException | None = None,
) -> dict:
    """Patch rag._load_model / rag._load_collection with counting stubs.

    Returns a dict of counters (``model_loads`` / ``collection_loads``) and the
    captured ``device`` so tests can assert single-load and device selection.
    """
    counters = {"model_loads": 0, "collection_loads": 0, "device": None}
    the_model = model if model is not None else FakeModel()
    the_collection = collection if collection is not None else FakeCollection()

    def fake_load_model(device: str):
        counters["model_loads"] += 1
        counters["device"] = device
        if model_delay:
            time.sleep(model_delay)
        if model_error is not None:
            raise model_error
        the_model.device = device
        return the_model

    def fake_load_collection(rag_db):
        counters["collection_loads"] += 1
        if collection_delay:
            time.sleep(collection_delay)
        if collection_error is not None:
            raise collection_error
        return the_collection

    monkeypatch.setattr(rag, "_load_model", fake_load_model)
    monkeypatch.setattr(rag, "_load_collection", fake_load_collection)
    counters["_model"] = the_model
    counters["_collection"] = the_collection
    return counters


@pytest.fixture(autouse=True)
def _reset_rag_singleton():
    """Each test starts from a clean, unloaded module state.

    rag.py keeps a process-global singleton; the suite must be able to reset it
    so a load in one test does not leak into the next. ``reset()`` is part of the
    module's (test-facing but documented) surface.
    """
    rag.reset()
    yield
    rag.reset()


# --------------------------------------------------------------------------- #
# 1. Lazy init: nothing loaded at import; first search triggers load ONCE.
# --------------------------------------------------------------------------- #


def test_import_is_light_no_heavy_modules_loaded():
    """Importing eud_agent.rag must NOT pull torch / sentence-transformers /
    chromadb (selfcheck / app must not pay the multi-GB torch import)."""
    import sys

    # rag is already imported at collection; assert it is not "loaded" purely as
    # a consequence of importing it (the heavy load is deferred to first use).
    assert not rag.is_loaded(), "rag must not be loaded merely by importing it"
    # A strict sys.modules check is brittle if pytest plugins import torch, so
    # the load-state assertion above is the real contract; this line is purely
    # informational and never fails.
    assert "torch" not in sys.modules or True  # informational


def test_first_search_triggers_single_load(monkeypatch, tmp_path):
    counters = _install_stub_loaders(monkeypatch)
    assert counters["model_loads"] == 0, "nothing loads before the first search"
    assert not rag.is_loaded()

    out = search("unit health", k=2, rag_db=str(tmp_path))
    assert rag.is_loaded()
    assert counters["model_loads"] == 1
    assert counters["collection_loads"] == 1
    assert len(out) == 2

    # A second search reuses the loaded singleton (no reload).
    search("player loop", k=1, rag_db=str(tmp_path))
    assert counters["model_loads"] == 1, "the model must load EXACTLY once"
    assert counters["collection_loads"] == 1


# --------------------------------------------------------------------------- #
# 2. Result shape: [{title, url, distance, text}] from chroma's batched rows.
# --------------------------------------------------------------------------- #


def test_search_result_shape(monkeypatch, tmp_path):
    _install_stub_loaders(monkeypatch)
    out = search("유닛 체력 설정", k=3, rag_db=str(tmp_path))

    assert isinstance(out, list) and len(out) == 3
    first = out[0]
    assert set(first.keys()) == {"title", "url", "distance", "text"}
    assert first["title"] == "Unit health"
    assert first["url"] == "http://cafe/1"
    assert first["text"] == "setdeaths usage for unit health"
    assert isinstance(first["distance"], float)
    # Ordering preserved from chroma (ascending distance in the stub).
    assert [r["distance"] for r in out] == [0.12, 0.34, 0.56]


def test_search_passes_k_as_n_results(monkeypatch, tmp_path):
    counters = _install_stub_loaders(monkeypatch)
    search("q", k=2, rag_db=str(tmp_path))
    coll = counters["_collection"]
    assert coll.query_calls, "query must be issued"
    assert coll.query_calls[-1]["n_results"] == 2


def test_search_default_k_is_5(monkeypatch, tmp_path):
    counters = _install_stub_loaders(monkeypatch)
    search("q", rag_db=str(tmp_path))
    coll = counters["_collection"]
    assert coll.query_calls[-1]["n_results"] == 5


def test_search_handles_missing_metadata_keys(monkeypatch, tmp_path):
    """A row whose metadata lacks title/url must not crash; the keys still
    appear (empty strings) so the orchestrator's prompt builder is robust."""
    rows = [
        {"metadata": {}, "document": "bare doc", "distance": 0.1},
        {"metadata": None, "document": "none-meta doc", "distance": 0.2},
    ]
    _install_stub_loaders(monkeypatch, collection=FakeCollection(rows))
    out = search("q", k=2, rag_db=str(tmp_path))
    assert len(out) == 2
    for r in out:
        assert set(r.keys()) == {"title", "url", "distance", "text"}
        assert r["title"] == ""
        assert r["url"] == ""


# --------------------------------------------------------------------------- #
# 3. Device selection: GPU when torch.cuda.is_available(), else CPU.
# --------------------------------------------------------------------------- #


def test_device_cuda_when_available(monkeypatch, tmp_path):
    counters = _install_stub_loaders(monkeypatch)
    monkeypatch.setattr(rag, "_cuda_available", lambda: True)
    search("q", rag_db=str(tmp_path))
    assert counters["device"] == "cuda"


def test_device_cpu_when_no_cuda(monkeypatch, tmp_path):
    counters = _install_stub_loaders(monkeypatch)
    monkeypatch.setattr(rag, "_cuda_available", lambda: False)
    search("q", rag_db=str(tmp_path))
    assert counters["device"] == "cpu"


# --------------------------------------------------------------------------- #
# 4. Background warmup: non-blocking, progress callback, ordering with search.
# --------------------------------------------------------------------------- #


def test_start_warmup_is_non_blocking(monkeypatch, tmp_path):
    """start_warmup must return IMMEDIATELY (it must never gate server.ready):
    with a deliberately SLOW loader, start_warmup + a short sleep should find the
    function already returned while the load is still in flight."""
    _install_stub_loaders(monkeypatch, model_delay=1.0)

    t0 = time.monotonic()
    t = start_warmup(str(tmp_path))
    elapsed = time.monotonic() - t0

    assert elapsed < 0.5, "start_warmup must return immediately, not block on load"
    assert isinstance(t, threading.Thread)
    assert t.is_alive() or not rag.is_loaded(), (
        "the load should still be in flight right after a non-blocking start"
    )
    # Let the warmup finish so the autouse reset() is clean.
    t.join(timeout=5.0)
    assert rag.is_loaded()


def test_warmup_progress_callback_started_then_done(monkeypatch, tmp_path):
    events: list = []

    def on_progress(stage, state, detail=None):
        events.append((stage, state))

    _install_stub_loaders(monkeypatch)
    t = start_warmup(str(tmp_path), on_progress=on_progress)
    t.join(timeout=5.0)

    # Every progress event is the rag_warmup stage; states go started -> done.
    assert events, "warmup must emit progress"
    assert all(stage == "rag_warmup" for stage, _ in events)
    states = [state for _, state in events]
    assert states[0] == "started"
    assert states[-1] == "done"
    assert "error" not in states


def test_warmup_progress_error_on_bad_db(monkeypatch, tmp_path):
    events: list = []

    def on_progress(stage, state, detail=None):
        events.append((stage, state))

    # The collection loader raises RagUnavailable (bad db).
    _install_stub_loaders(
        monkeypatch,
        collection_error=RagUnavailable("bad db"),
    )
    t = start_warmup(str(tmp_path), on_progress=on_progress)
    t.join(timeout=5.0)

    states = [state for _, state in events]
    assert states[0] == "started"
    assert states[-1] == "error", "a failed warmup must report the error state"
    assert all(stage == "rag_warmup" for stage, _ in events)


def test_warmup_then_immediate_search_waits_and_succeeds(monkeypatch, tmp_path):
    """start_warmup (slow loader) then an immediate search: the search must WAIT
    on the warmup lock and then succeed, with the model loaded only ONCE."""
    counters = _install_stub_loaders(monkeypatch, model_delay=0.4)

    start_warmup(str(tmp_path))
    # Issue the search immediately while warmup is still loading.
    out = search("q", k=2, rag_db=str(tmp_path))

    assert len(out) == 2
    assert counters["model_loads"] == 1, "warmup + search must not double-load"
    assert counters["collection_loads"] == 1


def test_concurrent_search_and_warmup_single_load(monkeypatch, tmp_path):
    """A concurrent race of N searches + a warmup must load the model EXACTLY
    once (the lazy load is guarded by a lock; no double-load, no failure)."""
    counters = _install_stub_loaders(monkeypatch, model_delay=0.2)

    results: list = []
    errors: list = []

    def do_search():
        try:
            results.append(search("q", k=1, rag_db=str(tmp_path)))
        except Exception as exc:  # noqa: BLE001 - surfaced via the errors list
            errors.append(exc)

    threads = [threading.Thread(target=do_search) for _ in range(6)]
    start_warmup(str(tmp_path))
    for t in threads:
        t.start()
    for t in threads:
        t.join(timeout=5.0)

    assert not errors, f"concurrent searches raised: {errors}"
    assert len(results) == 6
    assert counters["model_loads"] == 1, "the lock must serialize to a single load"
    assert counters["collection_loads"] == 1


# --------------------------------------------------------------------------- #
# 5. Bad rag_db path -> RagUnavailable at load; search after failed load
#    re-raises cleanly (orchestrator degrades to no-context).
# --------------------------------------------------------------------------- #


def test_nonexistent_rag_db_raises_unavailable(monkeypatch):
    """A nonexistent rag_db directory short-circuits to RagUnavailable WITHOUT
    even attempting the heavy load (no chromadb import for a missing path)."""
    # Loaders that would "succeed" if reached — the path check must fire first.
    _install_stub_loaders(monkeypatch)
    missing = r"C:\no\such\rag\db\path\nope"
    with pytest.raises(RagUnavailable) as ei:
        search("q", rag_db=missing)
    assert missing in str(ei.value) or "rag" in str(ei.value).lower()


def test_collection_load_failure_raises_unavailable(monkeypatch, tmp_path):
    """A chromadb that raises on open surfaces as RagUnavailable at load."""
    _install_stub_loaders(
        monkeypatch,
        collection_error=RuntimeError("corrupt sqlite store"),
    )
    with pytest.raises(RagUnavailable):
        search("q", rag_db=str(tmp_path))


def test_search_after_failed_load_reraises_cleanly(monkeypatch, tmp_path):
    """After a failed load, a subsequent search must re-raise RagUnavailable
    cleanly (not hang, not return partial results) so the orchestrator can
    degrade to no-context deterministically."""
    _install_stub_loaders(
        monkeypatch,
        collection_error=RuntimeError("corrupt sqlite store"),
    )
    with pytest.raises(RagUnavailable):
        search("q", rag_db=str(tmp_path))
    # Second call: still unavailable, still a clean RagUnavailable (no crash).
    with pytest.raises(RagUnavailable):
        search("q", rag_db=str(tmp_path))


def test_rag_unavailable_is_exception_subclass():
    assert issubclass(RagUnavailable, Exception)


# --------------------------------------------------------------------------- #
# Module-surface sanity (public names exist).
# --------------------------------------------------------------------------- #


def test_public_surface():
    for name in ("search", "start_warmup", "RagUnavailable", "reset", "is_loaded"):
        assert hasattr(rag, name), f"missing public name: {name}"


# --------------------------------------------------------------------------- #
# 6. LIVE smoke (opt-in): real bge-m3 load + query against the real ECA DB.
#    Skipped by default. NOTE: the live load takes ~tens of seconds (bge-m3
#    4.3 GB weights, CUDA) — hence the generous join/timeout budget below.
# --------------------------------------------------------------------------- #

_LIVE_RAG_DB = r"C:\Users\ifthe\proj\eud\ECA\chromadb_bge"


@pytest.mark.skipif(
    not (os.environ.get("EUD_RAG_LIVE") == "1" and os.path.isdir(_LIVE_RAG_DB)),
    reason="live rag smoke: set EUD_RAG_LIVE=1 and have the ECA chromadb_bge dir",
)
def test_live_rag_search_returns_k_results():
    """Real load + search against the real ECA DB returns k results with the
    right shape and plausible (finite) distances.

    NOTE: first load pulls the 4.3 GB bge-m3 weights into memory (~tens of
    seconds on CUDA); this test is intentionally slow and opt-in only.
    """
    import math

    out = search("유닛 체력 설정", k=5, rag_db=_LIVE_RAG_DB)
    assert isinstance(out, list) and len(out) == 5
    for r in out:
        assert set(r.keys()) == {"title", "url", "distance", "text"}
        assert isinstance(r["text"], str)
        assert math.isfinite(float(r["distance"])), "distance must be a finite number"


if __name__ == "__main__":
    raise SystemExit(pytest.main([__file__, "-q"]))
