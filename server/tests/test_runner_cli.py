"""Verification artifact for EUD-020-4840: headless runner_cli.

``eud_agent.runner_cli`` is the headless retention of the verified ECA codex
runner (``runner_legacy.py``), refactored ONTO the shared server modules
(features/02 "runner_cli.py"). It keeps the legacy job-queue contract while
routing all heavy lifting through ``rag`` / ``codex_client`` / the ``bridge_io``
file-IPC conventions:

  * Jobs live at ``<data_dir>\\jobs\\<id>.json`` with the legacy schema
    ``{"instruction", "target", "context"}`` (``context`` defaults True).
  * Each processed job produces ``<data_dir>\\inbox\\agent_<id>.cmd`` whose first
    line is ``SET <target>`` and whose body (from the 2nd line) is the generated
    code, written UTF-8 **without** a BOM. The ``agent_*`` inbox namespace is
    CONTRACTUAL (architecture.md IPC naming): the legacy runner owns it and the
    server's ``bridge_io`` (``srv-*``) must never collide with it.
  * After processing, the job file is consumed (``jobs\\<id>.json`` ->
    ``jobs\\<id>.done``).
  * ``--once`` processes the queue once and exits 0; ``--mock`` fakes codex (no
    subprocess); ``--no-context`` skips RAG.

The tests drive the module WITHOUT a real RAG model, codex subprocess, panel, or
editor — they stub the same seams the rest of the suite established
(``rag.search``, ``codex_client.CodexClient``). Module-sharing is asserted
structurally so the legacy fence/prompt duplication does not creep back in.

``eud_agent.runner_cli`` does NOT exist during Step A, so this suite is expected
to FAIL on import until runner_cli.py is implemented (Step B).
"""

from __future__ import annotations

import json
import re
from pathlib import Path

import pytest

# Imported at collection so the failing import is the first signal in Step A.
# (No try/except: the suite MUST error out until the module exists.)
from eud_agent import codex_client as codex_mod
from eud_agent import rag as rag_mod
from eud_agent import runner_cli

# Source location of runner_cli (structural / no-duplication assertions read it).
_RUNNER_CLI_SRC = (
    Path(__file__).resolve().parents[1] / "eud_agent" / "runner_cli.py"
)


# --------------------------------------------------------------------------- #
# Test doubles for the shared collaborators the runner routes through.
# --------------------------------------------------------------------------- #


class FakeCodexClient:
    """Recording codex stand-in returned by the patched factory seam.

    ``generate(prompt)`` records the prompt (so a test can prove RAG context did
    / did not flow into it) and returns canned code. It is handed back by the
    ``runner_cli._make_codex_client`` factory under test (no subprocess, no path
    validation), which is the seam the runner routes BOTH modes through.
    """

    instances: list[FakeCodexClient] = []

    def __init__(self) -> None:
        self.prompts: list[str] = []
        FakeCodexClient.instances.append(self)

    async def generate(self, prompt, *, timeout=None):
        self.prompts.append(prompt)
        return "x = 1;\n"


@pytest.fixture(autouse=True)
def _reset_fake_codex():
    FakeCodexClient.instances = []
    yield
    FakeCodexClient.instances = []


@pytest.fixture
def data_dir(tmp_path) -> Path:
    """A temp editor ``Data\\agent`` dir with an empty jobs queue."""
    d = tmp_path / "agent"
    (d / "jobs").mkdir(parents=True)
    (d / "inbox").mkdir()
    (d / "outbox").mkdir()
    return d


def write_job(data_dir: Path, job_id: str, **fields) -> Path:
    """Write a legacy-schema job JSON and return its path."""
    job = {"instruction": "make a marine", "target": "main.eps", "context": True}
    job.update(fields)
    p = data_dir / "jobs" / f"{job_id}.json"
    p.write_text(json.dumps(job), encoding="utf-8")
    return p


def patch_rag(monkeypatch, *, results=None):
    """Patch ``eud_agent.rag.search`` and record its calls.

    Returns the calls list; an empty list after a run proves rag was skipped.
    """
    calls = []

    def fake_search(query, k=5, *, rag_db):
        calls.append({"query": query, "k": k, "rag_db": rag_db})
        return results if results is not None else []

    monkeypatch.setattr(rag_mod, "search", fake_search)
    return calls


def patch_codex(monkeypatch):
    """Patch the ``runner_cli._make_codex_client`` factory seam.

    Returns a fresh :class:`FakeCodexClient` regardless of mode, so the runner
    never spawns a subprocess and the recorded prompt is observable. (The runner
    owns the mock/real branch inside the factory; the tests replace the whole
    factory, which is why no codex path is needed here.)
    """

    def fake_factory(cfg, *, mock):
        return FakeCodexClient()

    monkeypatch.setattr(runner_cli, "_make_codex_client", fake_factory)


def run_once(monkeypatch, data_dir: Path, *extra_args) -> int:
    """Invoke the runner once over ``data_dir`` and return its exit code.

    Contract under test: ``runner_cli.main(argv)`` mirrors the legacy
    ``argparse`` CLI (``--once`` / ``--mock`` / ``--no-context`` /
    ``--agent-dir``/``--data-dir``) and returns an int exit code.
    """
    argv = ["--once", "--data-dir", str(data_dir), *extra_args]
    return _invoke_main(monkeypatch, argv)


def _invoke_main(monkeypatch, argv: list[str]) -> int:
    """Call ``runner_cli.main`` tolerating either ``--data-dir`` or the legacy
    ``--agent-dir`` flag name (whichever the implementation settled on).

    Normalizes a ``SystemExit`` (argparse-style exit) and a ``None`` return
    (treated as exit 0) into a plain int so the assertions stay simple.
    """
    try:
        rv = runner_cli.main(argv)
    except SystemExit as exc:
        # argparse may not know --data-dir; retry with --agent-dir.
        if "--data-dir" in argv:
            alt = ["--agent-dir" if a == "--data-dir" else a for a in argv]
            try:
                rv = runner_cli.main(alt)
            except SystemExit as exc2:
                return int(exc2.code or 0)
        else:
            return int(exc.code or 0)
    return 0 if rv is None else int(rv)


def read_cmd(inbox: Path) -> tuple[str, bytes]:
    """Return ``(name, raw_bytes)`` of the single produced .cmd in ``inbox``."""
    cmds = sorted(inbox.glob("*.cmd"))
    assert len(cmds) == 1, f"expected exactly one .cmd, found {cmds}"
    return cmds[0].name, cmds[0].read_bytes()


# --------------------------------------------------------------------------- #
# 1) --once --mock produces a well-formed agent_<id>.cmd; job consumed; exit 0.
# --------------------------------------------------------------------------- #


def test_once_mock_produces_wellformed_set_cmd(monkeypatch, data_dir):
    patch_rag(monkeypatch)
    patch_codex(monkeypatch)
    write_job(data_dir, "job1", target="main.eps", context=False)

    code = run_once(monkeypatch, data_dir, "--mock", "--no-context")
    assert code == 0, "a clean --once run must exit 0"

    name, raw = read_cmd(data_dir / "inbox")

    # UTF-8 without BOM (rules.md: a BOM corrupts the bridge's first-line parse).
    assert not raw.startswith(b"\xef\xbb\xbf"), ".cmd must not carry a UTF-8 BOM"
    text = raw.decode("utf-8")

    # First line is `SET <target>`; the body is whatever follows the first \n.
    first_line, sep, body = text.partition("\n")
    assert sep, "the .cmd must have a body on the 2nd line"
    assert first_line == "SET main.eps"
    assert body.strip(), "the .cmd body (generated code) must be non-empty"

    # The job was consumed: <id>.json gone, <id>.done present.
    assert not (data_dir / "jobs" / "job1.json").exists()
    assert (data_dir / "jobs" / "job1.done").exists()


# --------------------------------------------------------------------------- #
# 2) Namespace: produced .cmd is agent_*, NEVER srv-* (contractual, EUD-020).
# --------------------------------------------------------------------------- #


def test_cmd_uses_agent_namespace_never_srv(monkeypatch, data_dir):
    patch_rag(monkeypatch)
    patch_codex(monkeypatch)
    write_job(data_dir, "jobX")

    run_once(monkeypatch, data_dir, "--mock", "--no-context")

    name, _ = read_cmd(data_dir / "inbox")
    assert re.fullmatch(r"agent_.+\.cmd", name), (
        f"runner must own the legacy agent_* namespace, got {name!r}"
    )
    assert not name.startswith("srv-"), (
        "runner_cli must NEVER emit the server's srv-* namespace (architecture.md)"
    )
    # The job id is carried in the .cmd name (legacy agent_<id>.cmd convention).
    assert "jobX" in name


# --------------------------------------------------------------------------- #
# 3a) --no-context SKIPS rag (rag.search must not be called).
# --------------------------------------------------------------------------- #


def test_no_context_skips_rag(monkeypatch, data_dir):
    rag_calls = patch_rag(monkeypatch, results=[{"text": "should not appear"}])
    patch_codex(monkeypatch)
    write_job(data_dir, "jc", instruction="no ctx", context=True)

    run_once(monkeypatch, data_dir, "--mock", "--no-context")

    assert rag_calls == [], "rag.search must NOT be called under --no-context"


# --------------------------------------------------------------------------- #
# 3b) With context: stubbed RAG flows into the codex prompt (mock captures it).
# --------------------------------------------------------------------------- #


def test_context_flows_into_prompt(monkeypatch, data_dir):
    rag_calls = patch_rag(
        monkeypatch,
        results=[
            {"title": "d1", "url": "u1", "distance": 0.1, "text": "CONTEXT-MARKER"}
        ],
    )
    patch_codex(monkeypatch)
    write_job(data_dir, "jctx", instruction="do it", target="t.eps", context=True)

    # --mock fakes codex output, but the prompt the mock receives must still
    # carry the RAG context (the runner builds the prompt before mocking output).
    run_once(monkeypatch, data_dir, "--mock")

    assert len(rag_calls) == 1, "rag.search must run exactly once with context"
    assert rag_calls[0]["query"] == "do it"

    assert FakeCodexClient.instances, "the runner did not build a CodexClient"
    prompts = [p for inst in FakeCodexClient.instances for p in inst.prompts]
    assert prompts, "codex.generate was never called"
    assert any("CONTEXT-MARKER" in p for p in prompts), (
        "the RAG context did not flow into the codex prompt"
    )


# --------------------------------------------------------------------------- #
# 4) Module sharing: runner_cli CALLS the shared codex_client helpers (call-level,
#    not source-text) and does NOT re-define the legacy fence/prompt duplicates.
# --------------------------------------------------------------------------- #


def test_runner_calls_shared_build_prompt(monkeypatch, data_dir):
    """The runner must compose the prompt via ``codex_client.build_prompt`` (not
    its own inline prompt string) — proven by a recording wrapper that the runner
    actually invokes for a job (features/02: refactored ONTO the shared modules)."""
    patch_rag(monkeypatch)
    patch_codex(monkeypatch)

    calls = []
    real_build = codex_mod.build_prompt

    def recording_build_prompt(instruction, context_chunks=None, current_code=None):
        calls.append(
            {
                "instruction": instruction,
                "context_chunks": context_chunks,
                "current_code": current_code,
            }
        )
        return real_build(instruction, context_chunks, current_code=current_code)

    monkeypatch.setattr(codex_mod, "build_prompt", recording_build_prompt)
    write_job(data_dir, "jbp", instruction="build it", context=False)

    run_once(monkeypatch, data_dir, "--mock", "--no-context")

    assert len(calls) == 1, "runner must call codex_client.build_prompt once/job"
    assert calls[0]["instruction"] == "build it"
    # current_code is None for the headless runner (it always issues a fresh SET).
    assert calls[0]["current_code"] is None


def test_runner_calls_shared_extract_code(monkeypatch, data_dir):
    """Fence handling must go through ``codex_client.extract_code`` — the runner's
    --mock path routes its canned reply through it (no local fence parsing)."""
    # Real (non-factory-patched) codex so the runner's OWN mock client runs and
    # exercises extract_code; rag is skipped so no DB is needed.
    patch_rag(monkeypatch)

    calls = []
    real_extract = codex_mod.extract_code

    def recording_extract_code(text):
        calls.append(text)
        return real_extract(text)

    monkeypatch.setattr(codex_mod, "extract_code", recording_extract_code)
    write_job(data_dir, "jec", instruction="x", context=False)

    code = run_once(monkeypatch, data_dir, "--mock", "--no-context")
    assert code == 0

    assert calls, "runner must route codex output through codex_client.extract_code"
    # What it extracted is what landed in the .cmd body (the shared helper's
    # output flows straight into the SET command).
    _, raw = read_cmd(data_dir / "inbox")
    body = raw.decode("utf-8").partition("\n")[2]
    assert body.strip(), "extracted code must be non-empty"


def test_runner_has_no_local_fence_duplicate():
    """Secondary structural guard: the runner must not re-declare ``_FENCE`` (the
    fence regex lives ONLY in codex_client)."""
    src = _RUNNER_CLI_SRC.read_text(encoding="utf-8")
    assert "_FENCE" not in src, (
        "runner_cli must not re-declare _FENCE (use codex_client.extract_code)"
    )


# --------------------------------------------------------------------------- #
# 5) runner_legacy stays untouched: runner_cli must NOT import it (import-then-
#    extend / the legacy module self-reconfigures stdio + parses argv).
# --------------------------------------------------------------------------- #


def test_runner_cli_does_not_import_runner_legacy():
    src = _RUNNER_CLI_SRC.read_text(encoding="utf-8")
    assert "runner_legacy" not in src, (
        "runner_cli must not import the frozen runner_legacy reference"
    )
