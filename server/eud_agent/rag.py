"""In-process bge-m3 semantic search over the ECA chromadb_bge store.

The RAG component of the agent server (features/02 "rag.py", Decision 01:
*RAG search runs in-process in the local Python server*). The resident server
amortizes the one-time bge-m3 model load (4.3 GB weights via the HF cache), so
queries on a warm process are sub-second — far better than the per-query reload
of the subprocess-delegation alternative that Decision 01 rejected.

Hard rules this module obeys (rules.md "RAG model loading must never gate
server.ready"; architecture.md "Boot and lifecycle"):

* **Light import.** Importing this module pulls NOTHING heavy: ``torch``,
  ``chromadb`` and ``sentence_transformers`` are imported INSIDE the loader
  functions (``_load_model`` / ``_load_collection`` / ``_cuda_available``), so
  ``config.run_selfcheck`` and ``app`` never pay the multi-GB torch import. Those
  three functions are also the test seams (stubbed in tests/test_rag.py).
* **Lazy singleton + single-load discipline.** The first ``search`` (or the
  background ``start_warmup`` thread) loads the model and opens the collection
  EXACTLY once, guarded by a module lock. Concurrent searches and a warmup race
  serialize on the lock; there is no double-load and no half-loaded state.
* **Warmup never blocks readiness.** ``start_warmup`` spawns a daemon thread and
  returns it immediately; it reports ``rag_warmup`` ``started`` -> ``done`` (or
  ``started`` -> ``error``) through the optional ``on_progress`` callback. The
  server boots and serves the panel regardless of whether the model has loaded.
* **Clean degradation.** A missing/bad ``rag_db`` path or a collection that fails
  to open surfaces as :class:`RagUnavailable`; the failure is remembered, so a
  later ``search`` re-raises ``RagUnavailable`` cleanly (the orchestrator then
  degrades to no-context per features/02 edge cases) — never a partial result,
  never a hang.
* **Read-only DB.** ``PersistentClient`` opens the ECA store in place (the churn
  chromadb causes on open is harmless THERE, outside our repo — Decision 01); we
  only ever ``query`` the collection, never write/add.

GPU is used when CUDA torch is available, CPU otherwise — the device string is
computed from ``_cuda_available()`` and passed EXPLICITLY to ``_load_model`` so
the choice is observable/testable rather than left to library autodetection.
"""

from __future__ import annotations

import os
import threading
from collections.abc import Callable
from typing import Any

# --------------------------------------------------------------------------- #
# Constants (grounded in tech-stack.md / Decision 01).
# --------------------------------------------------------------------------- #

#: HF model id; weights live in the HF hub cache (4.3 GB, pre-fetched).
MODEL_NAME = "BAAI/bge-m3"
#: chromadb collection built by ECA (1024d cosine, 4,974 docs).
COLLECTION_NAME = "eud_docs_bge"
#: Progress stage name surfaced to the panel (architecture.md WS protocol).
WARMUP_STAGE = "rag_warmup"

#: Type of the optional warmup progress callback:
#: ``on_progress(stage: str, state: str, detail: str | None) -> None``.
ProgressCallback = Callable[[str, str, "str | None"], None]


# --------------------------------------------------------------------------- #
# Exceptions.
# --------------------------------------------------------------------------- #


class RagUnavailable(Exception):
    """RAG search cannot be served (bad/missing DB path, failed load).

    The orchestrator catches this and degrades to a no-context codex run
    (features/02 edge cases). It is raised at load time and re-raised by any
    ``search`` issued after a failed load (no half-loaded state).
    """


# --------------------------------------------------------------------------- #
# Module singleton + the lock that serializes loading.
#
# ``_LOCK`` guards every read/write of the singleton fields below. A load
# happens at most once: holders re-check ``_engine`` / ``_load_error`` inside the
# lock (double-checked locking), so a warmup that is mid-load makes a concurrent
# search WAIT on the lock rather than start a second load.
# --------------------------------------------------------------------------- #

_LOCK = threading.Lock()
_engine: _Engine | None = None
# When a load fails we remember the error (resolved rag_db + cause) so later
# searches re-raise RagUnavailable cleanly instead of retrying a broken load.
_load_error: RagUnavailable | None = None
# The rag_db the singleton was loaded (or failed to load) for; a different
# rag_db is only honored after reset() (the server uses one DB per process).
_loaded_db: str | None = None


class _Engine:
    """The loaded pair (embedding model + chromadb collection)."""

    __slots__ = ("model", "collection")

    def __init__(self, model: Any, collection: Any) -> None:
        self.model = model
        self.collection = collection


# --------------------------------------------------------------------------- #
# Heavy loaders + device probe — THE TEST SEAMS.
#
# Each performs its heavy import INSIDE the function body so importing rag.py
# stays light. tests/test_rag.py monkeypatches these three names with stubs.
# --------------------------------------------------------------------------- #


def _cuda_available() -> bool:
    """Return True when a CUDA-capable torch build sees a device.

    The torch import is deferred here; any failure (CPU-only build, no driver)
    degrades to CPU rather than raising.
    """
    try:
        import torch  # heavy: deferred

        return bool(torch.cuda.is_available())
    except Exception:  # noqa: BLE001 - any torch/driver issue -> CPU fallback
        return False


def _load_model(device: str) -> Any:
    """Load the bge-m3 ``SentenceTransformer`` onto ``device`` ("cuda"/"cpu").

    Heavy import deferred. Weights come from the HF hub cache (no network on a
    warmed machine); ``device`` is passed explicitly (see module docstring).
    """
    from sentence_transformers import SentenceTransformer  # heavy: deferred

    return SentenceTransformer(MODEL_NAME, device=device)


def _load_collection(rag_db: str) -> Any:
    """Open the ECA chromadb store at ``rag_db`` and return the bge collection.

    Heavy import deferred. The store is opened in place and READ ONLY by
    convention (Decision 01); we only ever ``query`` it.
    """
    import chromadb  # heavy: deferred

    client = chromadb.PersistentClient(path=rag_db)
    return client.get_collection(COLLECTION_NAME)


# --------------------------------------------------------------------------- #
# Loading (single-load discipline) — all under _LOCK.
# --------------------------------------------------------------------------- #


def _ensure_loaded(rag_db: str) -> _Engine:
    """Return the loaded engine, loading it once under the lock if needed.

    Contract:
      * A nonexistent ``rag_db`` directory short-circuits to ``RagUnavailable``
        BEFORE any heavy loader runs (no chromadb import for a missing path).
      * A previously-failed load re-raises the remembered ``RagUnavailable``
        (clean, deterministic; the orchestrator degrades to no-context).
      * The model + collection load EXACTLY once; concurrent callers block on
        ``_LOCK`` and observe the already-built engine.
    """
    global _engine, _load_error, _loaded_db

    # Cheap path/precondition check OUTSIDE the lock first: a missing directory
    # never needs the heavy loaders and must fail fast (tests pin this).
    if not os.path.isdir(rag_db):
        raise RagUnavailable(
            f"RAG DB directory not found: {rag_db} "
            "(set rag_db / EUD_RAG_DB to the ECA chromadb_bge path)."
        )

    with _LOCK:
        # Re-check inside the lock (double-checked locking): a warmup that was
        # mid-load while we waited may have finished or failed.
        if _engine is not None and _loaded_db == rag_db:
            return _engine
        if _load_error is not None and _loaded_db == rag_db:
            # A prior load for this DB failed; re-raise the same clean error.
            raise _load_error

        # Either nothing is loaded yet, or the caller asked for a different DB
        # (only reachable after reset() in normal use). Perform the single load.
        try:
            device = "cuda" if _cuda_available() else "cpu"
            model = _load_model(device)
            collection = _load_collection(rag_db)
        except RagUnavailable as exc:
            _load_error = exc
            _loaded_db = rag_db
            raise
        except Exception as exc:  # noqa: BLE001 - wrap ANY loader failure
            err = RagUnavailable(
                f"failed to load RAG (db={rag_db}): {exc}"
            )
            _load_error = err
            _loaded_db = rag_db
            raise err from exc

        _engine = _Engine(model, collection)
        _load_error = None
        _loaded_db = rag_db
        return _engine


# --------------------------------------------------------------------------- #
# Public API.
# --------------------------------------------------------------------------- #


def is_loaded() -> bool:
    """True when the model+collection singleton is loaded and ready.

    Used by the warmup non-blocking check and by tests. A failed load reports
    False (there is no usable engine).
    """
    return _engine is not None


def reset() -> None:
    """Drop the singleton and any remembered load error.

    Mainly a test hook (tests/test_rag.py resets between cases); also lets the
    server retry after a transient load failure if it ever needs to.
    """
    global _engine, _load_error, _loaded_db
    with _LOCK:
        _engine = None
        _load_error = None
        _loaded_db = None


def search(
    query: str,
    k: int = 5,
    *,
    rag_db: str,
) -> list[dict[str, Any]]:
    """Embed ``query`` and return the top-``k`` matches from the ECA store.

    Loads the model + collection on first use (lazy singleton; subsequent calls
    reuse it). If a background ``start_warmup`` is mid-load, this WAITS on the
    module lock and then reuses that single load — never a second load.

    Returns a list of ``{"title", "url", "distance", "text"}`` dicts mapping
    chroma's batched query response (we issue a single query, so we read row 0 of
    ``metadatas`` / ``documents`` / ``distances``). Missing/None metadata fields
    degrade to empty strings so the orchestrator's prompt builder stays robust.

    Raises :class:`RagUnavailable` when the DB path is bad or the load failed
    (the orchestrator degrades to no-context per features/02 edge cases).
    """
    engine = _ensure_loaded(rag_db)

    embedding = engine.model.encode(query)
    # chromadb wants a LIST of query embeddings; ndarray/list both tolerated by
    # the client, but list() keeps the stubbed-collection contract explicit.
    res = engine.collection.query(
        query_embeddings=[_as_list(embedding)],
        n_results=k,
    )
    return _map_results(res)


def start_warmup(
    rag_db: str,
    on_progress: ProgressCallback | None = None,
) -> threading.Thread:
    """Start loading the model+collection in a background daemon thread.

    Returns the STARTED thread immediately so it never gates ``server.ready``
    (architecture.md / Decision 01). Progress is reported through ``on_progress``
    as ``("rag_warmup", "started", None)`` then ``("rag_warmup", "done", None)``
    on success, or ``("rag_warmup", "error", <detail>)`` on failure. A failed
    warmup leaves the module in a state where a later ``search`` re-raises
    ``RagUnavailable`` cleanly (the load error is remembered).
    """

    def _run() -> None:
        _emit(on_progress, "started")
        try:
            _ensure_loaded(rag_db)
        except RagUnavailable as exc:
            _emit(on_progress, "error", str(exc))
        except Exception as exc:  # noqa: BLE001 - warmup must never crash boot
            _emit(on_progress, "error", str(exc))
        else:
            _emit(on_progress, "done")

    thread = threading.Thread(
        target=_run, name="rag-warmup", daemon=True
    )
    thread.start()
    return thread


# --------------------------------------------------------------------------- #
# Helpers.
# --------------------------------------------------------------------------- #


def _emit(cb: ProgressCallback | None, state: str, detail: str | None = None) -> None:
    """Invoke the progress callback, tolerating a None callback or a raising one.

    A misbehaving panel callback must never break the warmup thread.
    """
    if cb is None:
        return
    try:
        cb(WARMUP_STAGE, state, detail)
    except Exception:  # noqa: BLE001 - progress reporting is best-effort
        pass


def _as_list(embedding: Any) -> list:
    """Coerce a model embedding (ndarray/list/tensor) to a plain list.

    Avoids importing numpy/torch here: ``tolist`` is honored when present, else
    we fall back to ``list(...)``.
    """
    tolist = getattr(embedding, "tolist", None)
    if callable(tolist):
        return tolist()
    return list(embedding)


def _map_results(res: dict) -> list[dict[str, Any]]:
    """Map chroma's batched query response (row 0) to the result shape.

    chroma returns ``metadatas`` / ``documents`` / ``distances`` as
    list-of-lists (one inner list per query embedding); we issued one query, so
    we read index 0. Missing/None metadata fields degrade to empty strings.
    """
    metadatas = _row0(res.get("metadatas"))
    documents = _row0(res.get("documents"))
    distances = _row0(res.get("distances"))

    out: list[dict[str, Any]] = []
    # strict=False: the three batched fields are derived from the same query and
    # are normally equal length; if chroma ever returns a short field we stop at
    # the shortest rather than raise (degrade, never crash a search).
    for meta, doc, dist in zip(metadatas, documents, distances, strict=False):
        meta = meta or {}
        out.append(
            {
                "title": str(meta.get("title", "") or ""),
                "url": str(meta.get("url", "") or ""),
                "distance": float(dist),
                "text": doc or "",
            }
        )
    return out


def _row0(batched: Any) -> list:
    """Return the first inner row of a chroma batched field, or ``[]``."""
    if not batched:
        return []
    first = batched[0]
    return list(first) if first else []
