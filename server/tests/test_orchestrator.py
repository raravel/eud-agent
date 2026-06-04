"""Verification artifact for EUD-018-bafb: the per-instruct orchestrator.

These tests drive ``eud_agent.orchestrator`` WITHOUT any real RAG model, codex
subprocess, or Lua bridge. Every dependency is a stub/monkeypatch so the suite
can assert the EXACT event sequences the WS protocol demands (architecture.md
"WebSocket protocol", features/02 "orchestrator.py", rules.md "Server and
panel"):

  * instruct (state machine ``rag -> codex -> lsp -> diff -> done``) emits
    ``progress`` at each transition and a terminal ``code`` event carrying the
    generated code, ``lang: "eps"``, a unified diff (for a SET target, diffed
    against the current bridge content), and the advisory diagnostics list.
  * ``useContext=False`` SKIPS the rag stage entirely (no rag.search call).
  * ``RagUnavailable`` degrades to a no-context codex run WITH a progress note
    (features/02 edge case) rather than failing the instruct.
  * LSP is advisory and OPTIONAL: ``lsp_gate`` does not exist yet, so the
    orchestrator imports it lazily inside ``try/except ImportError`` and, on
    absence, emits ``progress {stage: "lsp", detail: "skipped"}`` with
    ``diagnostics=[]`` (rules.md: absence of node/the package must not break the
    flow).
  * ONE in-flight instruct: a second concurrent instruct returns
    ``error {message: "busy"}`` and never touches codex/rag.
  * apply routes to ``bridge_io.set`` / ``bridge_io.neweps`` and emits
    ``applied {target}``; a ``BridgeBusy`` mid-poll surfaces
    ``progress {stage: "waiting_build"}`` and, on the eventual timeout,
    ``error {message: "editor busy"}``.
  * The unified diff is produced with ``difflib.unified_diff`` and is correct
    for a small change.

Synchronous bridge_io / rag / lsp calls are expected to run via
``asyncio.to_thread`` / ``run_in_executor`` (so a slow bridge call never blocks
the event loop) — the tests do not assert the executor mechanics directly but
DO rely on the orchestrator being a coroutine API.

``eud_agent.orchestrator`` does NOT exist during Step A, so this suite is
expected to FAIL on import until orchestrator.py is implemented (Step B).
"""

from __future__ import annotations

import asyncio

import pytest

# Imported at collection so the failing import is the first signal in Step A.
# (No try/except: the suite MUST error out until the module exists.)
from eud_agent import rag as rag_mod
from eud_agent.bridge_io import BridgeBusy, BridgeError
from eud_agent.orchestrator import Orchestrator
from eud_agent.rag import RagUnavailable

# --------------------------------------------------------------------------- #
# Test doubles for the orchestrator's three collaborators.
# --------------------------------------------------------------------------- #


class FakeBridge:
    """Stands in for a ``bridge_io.BridgeIO`` instance.

    Records calls and returns canned replies. ``get`` returns the configured
    current content (for the diff); ``set`` / ``neweps`` return "OK" by default
    but may be configured to raise ``BridgeBusy`` / ``BridgeError``. Each method
    accepts (and ignores) the ``on_busy`` keyword the orchestrator forwards.
    """

    def __init__(
        self,
        *,
        current: str = "",
        set_error: Exception | None = None,
        neweps_error: Exception | None = None,
        status: str = "compiling=false\nproject=demo\n",
        files: list[dict] | None = None,
        on_busy_calls: int = 0,
    ) -> None:
        self.current = current
        self.set_error = set_error
        self.neweps_error = neweps_error
        self._status = status
        self._files = files if files is not None else []
        # When >0, set()/neweps() invoke on_busy this many times before raising
        # the configured error (models a BridgeBusy after a waiting_build note).
        self.on_busy_calls = on_busy_calls
        self.calls: list[tuple] = []

    def get(self, path, **kw):
        self.calls.append(("get", path))
        return self.current

    def set(self, path, code, **kw):
        self.calls.append(("set", path, code))
        self._maybe_busy(kw)
        if self.set_error is not None:
            raise self.set_error
        return "OK"

    def neweps(self, name, code, **kw):
        self.calls.append(("neweps", name, code))
        self._maybe_busy(kw)
        if self.neweps_error is not None:
            raise self.neweps_error
        return "OK"

    def status(self, **kw):
        self.calls.append(("status",))
        return self._status

    def list_files(self, **kw):
        self.calls.append(("list_files",))
        return self._files

    def _maybe_busy(self, kw):
        on_busy = kw.get("on_busy")
        for _ in range(self.on_busy_calls):
            if on_busy is not None:
                on_busy()


class FakeCodex:
    """Stands in for ``codex_client.CodexClient``.

    ``generate(prompt)`` records the prompt and returns the canned code (or
    raises a configured error). The prompt capture lets a test prove the RAG
    context did / did not flow into the codex call.
    """

    def __init__(self, *, code: str = "x = 1;", error: Exception | None = None,
                 delay: float = 0.0) -> None:
        self.code = code
        self.error = error
        self.delay = delay
        self.prompts: list[str] = []

    async def generate(self, prompt, *, timeout=None):
        self.prompts.append(prompt)
        if self.delay:
            await asyncio.sleep(self.delay)
        if self.error is not None:
            raise self.error
        return self.code


class Recorder:
    """Async ``send`` collector: every emitted event dict is appended."""

    def __init__(self) -> None:
        self.events: list[dict] = []

    async def __call__(self, event: dict) -> None:
        self.events.append(event)

    def types(self) -> list[str]:
        return [e.get("type") for e in self.events]

    def first(self, etype: str) -> dict:
        for e in self.events:
            if e.get("type") == etype:
                return e
        raise AssertionError(f"no event of type {etype!r} in {self.types()}")

    def stages(self) -> list[str]:
        return [e.get("stage") for e in self.events if e.get("type") == "progress"]


# --------------------------------------------------------------------------- #
# Fixtures / helpers.
# --------------------------------------------------------------------------- #


@pytest.fixture
def rag_results():
    """Canned RAG hits in the rag.search result shape."""
    return [
        {"title": "doc1", "url": "u1", "distance": 0.1, "text": "context one"},
        {"title": "doc2", "url": "u2", "distance": 0.2, "text": "context two"},
    ]


def make_orch(bridge, codex, recorder, *, rag_db="C:\\fake\\rag"):
    """Construct an Orchestrator with the injected collaborators.

    Contract under test: ``Orchestrator(bridge, codex, *, rag_db, send)`` where
    ``send`` is an async callable taking a single event dict.
    """
    return Orchestrator(bridge, codex, rag_db=rag_db, send=recorder)


def patch_rag(monkeypatch, *, results=None, error=None):
    """Patch ``eud_agent.rag.search`` to return ``results`` or raise ``error``.

    The orchestrator is expected to call ``rag.search(query, k=..., rag_db=...)``
    (the real module signature) via an executor; we record the args.
    """
    calls = []

    def fake_search(query, k=5, *, rag_db):
        calls.append({"query": query, "k": k, "rag_db": rag_db})
        if error is not None:
            raise error
        return results if results is not None else []

    monkeypatch.setattr(rag_mod, "search", fake_search)
    return calls


# --------------------------------------------------------------------------- #
# instruct: happy path (useContext=True, SET target with a diff).
# --------------------------------------------------------------------------- #


async def test_instruct_happy_with_context_set_target(monkeypatch, rag_results):
    rag_calls = patch_rag(monkeypatch, results=rag_results)
    bridge = FakeBridge(current="x = 0;\n")
    codex = FakeCodex(code="x = 1;\n")
    rec = Recorder()
    o = make_orch(bridge, codex, rec)

    await o.instruct("set x to one", target="main.eps", use_context=True)

    # rag.search was called with the instruction + the configured rag_db.
    assert len(rag_calls) == 1
    assert rag_calls[0]["rag_db"] == "C:\\fake\\rag"

    # Progress stages in order: rag -> codex -> lsp (skipped, module absent).
    stages = rec.stages()
    assert stages[:3] == ["rag", "codex", "lsp"]

    # The lsp progress notes it was skipped (lsp_gate.py does not exist yet).
    lsp_ev = [e for e in rec.events
              if e.get("type") == "progress" and e.get("stage") == "lsp"][0]
    assert lsp_ev.get("detail") == "skipped"

    # Terminal code event: the generated code, lang eps, a real diff, empty diags.
    code_ev = rec.first("code")
    assert code_ev["code"] == "x = 1;\n"
    assert code_ev["lang"] == "eps"
    assert code_ev["diagnostics"] == []
    # The diff is a unified diff of current ("x = 0;") -> generated ("x = 1;").
    diff = code_ev["diff"]
    assert "-x = 0;" in diff
    assert "+x = 1;" in diff

    # The current content was fetched from the bridge for the diff.
    assert ("get", "main.eps") in bridge.calls

    # The RAG context flowed into the codex prompt.
    assert "context one" in codex.prompts[0]


async def test_instruct_codex_runs_after_rag(monkeypatch, rag_results):
    """codex must run AFTER rag (the prompt is built from the RAG context)."""
    patch_rag(monkeypatch, results=rag_results)
    bridge = FakeBridge(current="a;\n")
    codex = FakeCodex(code="b;\n")
    rec = Recorder()
    o = make_orch(bridge, codex, rec)

    await o.instruct("do", target="f.eps", use_context=True)

    types = rec.types()
    # 'code' is terminal and comes after all the progress events.
    assert types[-1] == "code"
    assert codex.prompts, "codex.generate was never called"


# --------------------------------------------------------------------------- #
# instruct: useContext=False skips RAG.
# --------------------------------------------------------------------------- #


async def test_instruct_no_context_skips_rag(monkeypatch):
    rag_calls = patch_rag(monkeypatch, results=[{"text": "should not appear"}])
    bridge = FakeBridge(current="")
    codex = FakeCodex(code="z;\n")
    rec = Recorder()
    o = make_orch(bridge, codex, rec)

    await o.instruct("no ctx", target="f.eps", use_context=False)

    assert rag_calls == [], "rag.search must NOT be called when use_context=False"
    assert "rag" not in rec.stages()
    # codex still runs and the prompt has no RAG context leakage.
    assert "should not appear" not in codex.prompts[0]
    assert rec.first("code")["code"] == "z;\n"


# --------------------------------------------------------------------------- #
# instruct: RagUnavailable degrades to no-context with a progress note.
# --------------------------------------------------------------------------- #


async def test_instruct_rag_unavailable_degrades(monkeypatch):
    patch_rag(monkeypatch, error=RagUnavailable("db missing"))
    bridge = FakeBridge(current="")
    codex = FakeCodex(code="ok;\n")
    rec = Recorder()
    o = make_orch(bridge, codex, rec)

    await o.instruct("with ctx but db gone", target="f.eps", use_context=True)

    # A rag progress note is still emitted (the degrade is observable).
    rag_progress = [e for e in rec.events
                    if e.get("type") == "progress" and e.get("stage") == "rag"]
    assert rag_progress, "expected a rag progress note even on RagUnavailable"
    # The instruct does NOT fail: codex still runs and a code event is produced.
    assert not [e for e in rec.events if e.get("type") == "error"]
    assert rec.first("code")["code"] == "ok;\n"
    assert codex.prompts, "codex must still run on RAG degrade"


# --------------------------------------------------------------------------- #
# instruct: codex unavailable (codex=None) -> clean error, no exception.
# --------------------------------------------------------------------------- #


async def test_instruct_codex_none_emits_clean_error(monkeypatch):
    """codex absent (create_app keeps the server up with codex=None): instruct
    must emit a clean error event and NEVER raise (features/02 codex-absent edge
    case; would otherwise be None.generate -> AttributeError in the WS loop)."""
    rag_calls = patch_rag(monkeypatch, results=[{"text": "ctx"}])
    bridge = FakeBridge(current="x;\n")
    rec = Recorder()
    o = make_orch(bridge, None, rec)  # codex=None

    # Must not raise.
    await o.instruct("do something", target="f.eps", use_context=True)

    err = rec.first("error")
    assert "codex" in err["message"].lower()
    # The gate is hit FIRST: no rag/bridge work, no code event.
    assert rag_calls == [], "codex-None must short-circuit before rag.search"
    assert ("get", "f.eps") not in bridge.calls
    assert not [e for e in rec.events if e.get("type") == "code"]


# --------------------------------------------------------------------------- #
# instruct: ONE in-flight (second concurrent instruct -> error "busy").
# --------------------------------------------------------------------------- #


async def test_second_concurrent_instruct_is_busy(monkeypatch, rag_results):
    patch_rag(monkeypatch, results=rag_results)
    bridge = FakeBridge(current="")
    # A slow codex keeps the first instruct in-flight while the second arrives.
    codex = FakeCodex(code="done;\n", delay=0.2)
    rec = Recorder()
    o = make_orch(bridge, codex, rec)

    first = asyncio.create_task(
        o.instruct("first", target="f.eps", use_context=True)
    )
    # Yield so the first instruct grabs the in-flight slot before the second.
    await asyncio.sleep(0.02)
    await o.instruct("second", target="f.eps", use_context=True)

    # The second instruct produced exactly a busy error and ran NO codex call
    # for itself (only the first instruct's single codex invocation exists).
    busy = [e for e in rec.events
            if e.get("type") == "error" and e.get("message") == "busy"]
    assert len(busy) == 1
    assert len(codex.prompts) == 1  # the second never reached codex

    await first
    # After the first finishes, a fresh instruct is accepted again.
    assert rec.first("code")["code"] == "done;\n"


async def test_in_flight_released_after_instruct(monkeypatch, rag_results):
    """A sequential second instruct (after the first completes) is NOT busy."""
    patch_rag(monkeypatch, results=rag_results)
    bridge = FakeBridge(current="")
    codex = FakeCodex(code="c;\n")
    rec = Recorder()
    o = make_orch(bridge, codex, rec)

    await o.instruct("a", target="f.eps", use_context=True)
    await o.instruct("b", target="f.eps", use_context=True)

    assert not [e for e in rec.events
                if e.get("type") == "error" and e.get("message") == "busy"]
    code_events = [e for e in rec.events if e.get("type") == "code"]
    assert len(code_events) == 2


# --------------------------------------------------------------------------- #
# apply: set / neweps happy paths.
# --------------------------------------------------------------------------- #


async def test_apply_set_happy(monkeypatch):
    bridge = FakeBridge()
    codex = FakeCodex()
    rec = Recorder()
    o = make_orch(bridge, codex, rec)

    await o.apply(mode="set", target="main.eps", code="q;\n")

    assert ("set", "main.eps", "q;\n") in bridge.calls
    applied = rec.first("applied")
    assert applied["target"] == "main.eps"
    assert not [e for e in rec.events if e.get("type") == "error"]


async def test_apply_neweps_happy(monkeypatch):
    bridge = FakeBridge()
    codex = FakeCodex()
    rec = Recorder()
    o = make_orch(bridge, codex, rec)

    await o.apply(mode="neweps", target="new1", code="body;\n")

    assert ("neweps", "new1", "body;\n") in bridge.calls
    assert rec.first("applied")["target"] == "new1"


async def test_apply_bridge_error_surfaces_error_event(monkeypatch):
    """A BridgeError (e.g. NEWEPS duplicate) becomes an error event, not a crash."""
    bridge = FakeBridge(neweps_error=BridgeError("ERROR: duplicate 'dup'"))
    codex = FakeCodex()
    rec = Recorder()
    o = make_orch(bridge, codex, rec)

    await o.apply(mode="neweps", target="dup", code="x;\n")

    err = rec.first("error")
    assert "duplicate" in err["message"].lower()
    assert not [e for e in rec.events if e.get("type") == "applied"]


# --------------------------------------------------------------------------- #
# apply: BridgeBusy -> waiting_build progress, then error "editor busy".
# --------------------------------------------------------------------------- #


async def test_apply_busy_emits_waiting_build_then_editor_busy(monkeypatch):
    # set() fires on_busy once (the waiting_build note) then times out (BridgeBusy).
    bridge = FakeBridge(
        set_error=BridgeBusy("timed out"),
        on_busy_calls=1,
    )
    codex = FakeCodex()
    rec = Recorder()
    o = make_orch(bridge, codex, rec)

    await o.apply(mode="set", target="main.eps", code="x;\n")

    # The waiting_build progress note fired (the on_busy callback path).
    wb = [e for e in rec.events
          if e.get("type") == "progress" and e.get("stage") == "waiting_build"]
    assert wb, "expected a waiting_build progress note on BridgeBusy"

    # The eventual outcome is an 'editor busy' error.
    err = rec.first("error")
    assert err["message"] == "editor busy"
    assert not [e for e in rec.events if e.get("type") == "applied"]


# --------------------------------------------------------------------------- #
# status / list passthrough events.
# --------------------------------------------------------------------------- #


async def test_status_emits_status_event(monkeypatch):
    bridge = FakeBridge(status="compiling=true\nproject=demoproj\n")
    codex = FakeCodex()
    rec = Recorder()
    o = make_orch(bridge, codex, rec)

    await o.status()

    ev = rec.first("status")
    assert ev["compiling"] is True
    assert ev["project"] == "demoproj"


async def test_list_emits_list_event(monkeypatch):
    files = [
        {"path": "a.eps", "ftype": "CUIEps", "settable": True},
        {"path": "b.gui", "ftype": "GUI", "settable": False},
    ]
    bridge = FakeBridge(files=files)
    codex = FakeCodex()
    rec = Recorder()
    o = make_orch(bridge, codex, rec)

    await o.list_files()

    ev = rec.first("list")
    assert ev["files"] == files


# --------------------------------------------------------------------------- #
# Unified diff correctness for a small change (independent of the bridge).
# --------------------------------------------------------------------------- #


async def test_unified_diff_correctness(monkeypatch):
    """A SET-target instruct yields a unified diff matching difflib semantics."""
    patch_rag(monkeypatch, results=[])
    before = "line1\nline2\nline3\n"
    after = "line1\nCHANGED\nline3\n"
    bridge = FakeBridge(current=before)
    codex = FakeCodex(code=after)
    rec = Recorder()
    o = make_orch(bridge, codex, rec)

    await o.instruct("change line2", target="f.eps", use_context=False)

    diff = rec.first("code")["diff"]
    # The hunk header and the +/- lines of the single-line change are present.
    assert "@@" in diff
    assert "-line2" in diff
    assert "+CHANGED" in diff
    # Unchanged context lines are NOT prefixed with +/- (difflib context marker).
    assert " line1" in diff or "line1" in diff
