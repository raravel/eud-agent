"""Verification artifact for EUD-016-af75: server-side codex_client module.

These tests drive ``eud_agent.codex_client`` WITHOUT spawning the real codex
CLI (except the explicitly-gated live smoke test). The default suite mocks
``asyncio.create_subprocess_exec`` with a fake process object so it can assert
the exact contract the rules demand (rules.md "codex invocation (Windows)",
architecture.md / features/02 "codex_client.py"):

  - Resolution: an EMPTY or MISSING codex path fails fast with ``CodexNotFound``
    carrying a helpful, actionable message (never spawn bare "codex").
  - Invocation: ``create_subprocess_exec(resolved, "exec",
    "--skip-git-repo-check", ...)`` with the FULL prompt delivered via stdin
    (written then CLOSED) and NEVER on argv (32,767-char CreateProcess limit;
    closing stdin prevents the EOF-wait hang). ``cwd`` is the repo root.
  - Output: codex stdout is noisy (session banners, token counts) -> extract
    fenced code blocks; multiple blocks join with a blank line; language tags
    are tolerated; ZERO fences -> ``CodexNoCode`` carrying <=500 chars of raw
    output (never apply unfenced noise).
  - Timeout: a process that never returns -> ``CodexTimeout`` within an injected
    small timeout, and the process is KILLED.

``eud_agent.codex_client`` does NOT exist during Step A, so this suite is
expected to FAIL on import until codex_client.py is implemented (Step B).
"""

from __future__ import annotations

import asyncio
import shutil

import pytest

# Imported at collection so the failing import is the first signal in Step A.
# (No try/except: the suite MUST error out until the module exists.)
from eud_agent import codex_client
from eud_agent.codex_client import (
    CodexClient,
    CodexNoCode,
    CodexNotFound,
    CodexTimeout,
)

# --------------------------------------------------------------------------- #
# Fake asyncio subprocess: records argv/cwd, captures stdin, returns canned
# stdout, and (optionally) blocks forever so the timeout path can be exercised.
# --------------------------------------------------------------------------- #


class FakeStdin:
    """Imitates ``proc.stdin``: records every write and whether it was closed."""

    def __init__(self, *, drain_error: BaseException | None = None) -> None:
        self.buffer = bytearray()
        self.closed = False
        self.wait_closed_called = False
        # When set, drain() raises this (simulates a peer that exited without
        # reading stdin -> BrokenPipeError [WinError 109] mid-write).
        self._drain_error = drain_error

    def write(self, data: bytes) -> None:
        assert isinstance(data, (bytes, bytearray)), "stdin must receive bytes"
        assert not self.closed, "write after close"
        self.buffer.extend(data)

    def close(self) -> None:
        self.closed = True

    async def drain(self) -> None:
        if self._drain_error is not None:
            raise self._drain_error

    async def wait_closed(self) -> None:
        self.wait_closed_called = True


class FakeProcess:
    """Stands in for the object returned by create_subprocess_exec.

    ``stdout_bytes`` is returned from communicate(); ``returncode`` is the exit
    code. When ``hang`` is True, communicate() never completes (await sleeps
    forever) so an ``asyncio.wait_for`` wrapper must time out and the client
    must call ``kill()``.
    """

    def __init__(
        self,
        *,
        stdout_bytes: bytes = b"",
        stderr_bytes: bytes = b"",
        returncode: int = 0,
        hang: bool = False,
        stdin_drain_error: BaseException | None = None,
    ) -> None:
        self.stdin = FakeStdin(drain_error=stdin_drain_error)
        self._stdout_bytes = stdout_bytes
        self._stderr_bytes = stderr_bytes
        self.returncode = returncode
        self._hang = hang
        self.killed = False
        self.wait_called = False
        # Order witness: True only if wait() was invoked AFTER kill() (reaping).
        self.reaped_after_kill = False

    async def communicate(self, input=None):  # noqa: A002 - mirror stdlib name
        if self._hang:
            # Never returns on its own; the client's wait_for must cancel us.
            await asyncio.Event().wait()
        return self._stdout_bytes, self._stderr_bytes

    def kill(self) -> None:
        self.killed = True

    async def wait(self) -> int:
        self.wait_called = True
        if self.killed:
            self.reaped_after_kill = True
        return self.returncode


def _install_fake_exec(monkeypatch, proc: FakeProcess) -> dict:
    """Monkeypatch asyncio.create_subprocess_exec to return ``proc``.

    Returns a dict capturing the call: ``argv`` (the positional program+args),
    ``kwargs`` (cwd / stdin / stdout / stderr), so tests can assert the exact
    invocation contract.
    """
    captured: dict = {}

    async def fake_exec(*args, **kwargs):
        captured["argv"] = list(args)
        captured["kwargs"] = kwargs
        return proc

    monkeypatch.setattr(asyncio, "create_subprocess_exec", fake_exec)
    return captured


# A codex path that "exists" for construction tests: the python executable is a
# real file on every machine running this suite.
def _real_path() -> str:
    return shutil.which("python") or shutil.which("python3") or __file__


# --------------------------------------------------------------------------- #
# 1. Resolution failure: empty / missing codex path -> CodexNotFound
# --------------------------------------------------------------------------- #


def test_empty_codex_path_raises_not_found(tmp_path):
    """An EMPTY codex_cmd fails fast with a helpful CodexNotFound message
    (config.py returns "" when shutil.which('codex') is unresolved)."""
    with pytest.raises(CodexNotFound) as ei:
        CodexClient("", repo_root=str(tmp_path))
    msg = str(ei.value).lower()
    assert "codex" in msg
    # A helpful message points the operator at the fix (install / CODEX_CMD).
    assert ("which" in msg) or ("codex_cmd" in msg) or ("install" in msg)


def test_missing_codex_path_raises_not_found(tmp_path):
    """A NON-EMPTY but non-existent codex path also fails fast."""
    bogus = str(tmp_path / "no-such-codex.cmd")
    with pytest.raises(CodexNotFound) as ei:
        CodexClient(bogus, repo_root=str(tmp_path))
    assert bogus in str(ei.value)


def test_valid_codex_path_constructs(tmp_path):
    """A path to a real file constructs without raising."""
    client = CodexClient(_real_path(), repo_root=str(tmp_path))
    assert client is not None


# --------------------------------------------------------------------------- #
# 2. stdin delivery: a >40KB prompt arrives COMPLETE via stdin; stdin closed;
#    argv carries exec + --skip-git-repo-check and NOT the prompt.
# --------------------------------------------------------------------------- #


@pytest.mark.asyncio
async def test_large_prompt_delivered_via_stdin_and_argv_clean(monkeypatch, tmp_path):
    big_prompt = "X" * 45_000 + "\n```eps\nputs(1);\n```\n"  # >40KB
    assert len(big_prompt) > 40_000

    proc = FakeProcess(stdout_bytes=b"```eps\nputs(1);\n```\n", returncode=0)
    captured = _install_fake_exec(monkeypatch, proc)

    resolved = _real_path()
    client = CodexClient(resolved, repo_root=str(tmp_path))
    code = await client.generate(big_prompt, timeout=5.0)

    # The extraction still works on the canned output.
    assert "puts(1);" in code

    # --- argv contract ---
    argv = captured["argv"]
    assert argv[0] == resolved, "must spawn the RESOLVED shim, never bare 'codex'"
    assert argv[1] == "exec"
    assert "--skip-git-repo-check" in argv
    # The prompt must NEVER appear on argv (32,767-char CreateProcess limit).
    assert all(big_prompt not in str(a) for a in argv), "prompt leaked onto argv"
    assert not any(len(str(a)) > 1000 for a in argv), "argv carries a huge arg"

    # --- cwd contract ---
    assert str(captured["kwargs"].get("cwd")) == str(tmp_path)

    # --- stdin contract: FULL prompt written then CLOSED ---
    sent = proc.stdin.buffer.decode("utf-8")
    assert sent == big_prompt, "the complete prompt must reach stdin intact"
    assert proc.stdin.closed, "stdin must be CLOSED (prevents the EOF-wait hang)"


@pytest.mark.asyncio
async def test_subprocess_pipes_configured(monkeypatch, tmp_path):
    """stdin/stdout/stderr are all PIPE (explicit stdin is mandatory: an
    inherited console-less stdin makes codex hang until timeout)."""
    proc = FakeProcess(stdout_bytes=b"```\nok\n```", returncode=0)
    captured = _install_fake_exec(monkeypatch, proc)

    client = CodexClient(_real_path(), repo_root=str(tmp_path))
    await client.generate("hi", timeout=5.0)

    kw = captured["kwargs"]
    assert kw.get("stdin") == asyncio.subprocess.PIPE
    assert kw.get("stdout") == asyncio.subprocess.PIPE
    assert kw.get("stderr") == asyncio.subprocess.PIPE


@pytest.mark.asyncio
async def test_early_exit_with_large_prompt_yields_no_code_not_broken_pipe(
    monkeypatch, tmp_path
):
    """B1: a codex shim that EXITS without reading stdin + a large (300KB) prompt
    breaks the pipe mid-write (BrokenPipeError [WinError 109]). generate() must
    swallow it, fall through to communicate(), and surface the process's real
    stderr as a clean CodexNoCode — NEVER let BrokenPipeError leak out.
    """
    big_prompt = "Y" * 300_000  # the documented RAG-context size
    proc = FakeProcess(
        stdout_bytes=b"",  # the early-exiting shim produced no fenced code
        stderr_bytes=b"error: unknown option '--frobnicate'\nUsage: codex exec",
        returncode=2,
        stdin_drain_error=BrokenPipeError(109, "The pipe has been ended"),
    )
    _install_fake_exec(monkeypatch, proc)

    client = CodexClient(_real_path(), repo_root=str(tmp_path))
    with pytest.raises(CodexNoCode) as ei:
        await client.generate(big_prompt, timeout=5.0)

    # The stderr tail is surfaced so the operator sees WHY codex produced nothing.
    assert "unknown option" in str(ei.value)


# --------------------------------------------------------------------------- #
# 3. Fence extraction: single, multiple, language tags, banner noise.
# --------------------------------------------------------------------------- #


def test_extract_single_block():
    text = "preamble\n```\nputs(1);\n```\ntrailer"
    assert codex_client.extract_code(text) == "puts(1);"


def test_extract_multiple_blocks_joined_with_blank_line():
    text = "```\nA();\n```\nchatter\n```\nB();\n```"
    assert codex_client.extract_code(text) == "A();\n\nB();"


def test_extract_language_tagged_fences():
    text = "intro\n```eps\nfunction f() {}\n```\nend"
    assert codex_client.extract_code(text) == "function f() {}"


def test_extract_with_surrounding_banner_noise():
    """Codex stdout includes session banners + token counts around the fence."""
    text = (
        "[2026-06-04T12:00:00] codex exec session started\n"
        "model: gpt-5-codex   tokens: 1234 in / 567 out\n"
        "Here is the code you asked for:\n"
        "```eps\n"
        "foreach(p : EUDLoopPlayer()) {\n"
        "    setdeaths(p, SetTo, 1, \"Terran Marine\");\n"
        "}\n"
        "```\n"
        "[done] 8.1s  tokens used: 1801\n"
    )
    out = codex_client.extract_code(text)
    assert out.startswith("foreach(p : EUDLoopPlayer())")
    assert "setdeaths" in out
    assert "session started" not in out
    assert "tokens used" not in out


def test_extract_block_with_inline_backticks_in_string_not_truncated():
    """A2: a line whose body contains an INLINE ``` (inside a string literal)
    must NOT close the block — the closing fence is only valid at line start.

    The old non-greedy regex stopped at the first inner backtick run and silently
    truncated the code; this asserts the FULL body survives.
    """
    text = (
        "here you go:\n"
        "```eps\n"
        'const fence = "```";  // looks like a fence but is in a string\n'
        "puts(fence);\n"
        "doMore();\n"
        "```\n"
        "[done]\n"
    )
    out = codex_client.extract_code(text)
    # The whole body is present, including the line with the inline ```.
    assert 'const fence = "```";' in out
    assert "puts(fence);" in out
    assert out.rstrip().endswith("doMore();")
    # And it is a single joined block (no spurious split / truncation).
    assert "\n\n" not in out


def test_extract_normalizes_crlf_to_lf():
    """A3: codex on Windows can emit CRLF; the extracted body must be LF-only
    (an interior \\r would otherwise flow into SET bodies)."""
    text = "intro\r\n```eps\r\nline1();\r\nline2();\r\n```\r\ntrailer\r\n"
    out = codex_client.extract_code(text)
    assert "\r" not in out, "interior CR must be normalized away"
    assert out == "line1();\nline2();"


# --------------------------------------------------------------------------- #
# 4. No-fence -> CodexNoCode carrying <=500 chars of the raw output.
# --------------------------------------------------------------------------- #


def test_no_fence_raises_no_code_with_truncated_raw():
    raw = "just some unfenced banner noise " * 50  # >500 chars, no fences
    assert len(raw) > 500
    with pytest.raises(CodexNoCode) as ei:
        codex_client.extract_code(raw)
    carried = str(ei.value)
    # The raw output is surfaced so the operator sees what codex actually said,
    # but truncated to <=500 chars (never spam / never apply the noise).
    assert "banner noise" in carried
    # The carried raw snippet must not exceed 500 chars of the original.
    assert raw[:500] in carried
    assert raw[:501] not in carried


@pytest.mark.asyncio
async def test_generate_no_fence_raises_no_code(monkeypatch, tmp_path):
    """generate() surfaces CodexNoCode when the process emits no fenced block."""
    proc = FakeProcess(stdout_bytes=b"only a banner, no code here", returncode=0)
    _install_fake_exec(monkeypatch, proc)
    client = CodexClient(_real_path(), repo_root=str(tmp_path))
    with pytest.raises(CodexNoCode):
        await client.generate("do something", timeout=5.0)


# --------------------------------------------------------------------------- #
# 5. Timeout: a hanging process -> CodexTimeout within the injected timeout,
#    and the process is KILLED.
# --------------------------------------------------------------------------- #


@pytest.mark.asyncio
async def test_timeout_raises_and_kills(monkeypatch, tmp_path):
    proc = FakeProcess(hang=True)
    _install_fake_exec(monkeypatch, proc)
    client = CodexClient(_real_path(), repo_root=str(tmp_path))

    loop = asyncio.get_running_loop()
    t0 = loop.time()
    with pytest.raises(CodexTimeout):
        await client.generate("hang please", timeout=0.3)
    elapsed = loop.time() - t0

    assert elapsed < 3.0, "must time out near the injected 0.3s, not the 600s default"
    assert proc.killed, "the hanging process must be KILLED on timeout"
    # A1: the killed process must be REAPED (await wait) so the proactor transport
    # is closed (no un-reaped ResourceWarning). wait() must run AFTER kill().
    assert proc.wait_called, "the process must be awaited (reaped) after kill"
    assert proc.reaped_after_kill, "wait() must be called AFTER kill(), not before"


# --------------------------------------------------------------------------- #
# 6. build_prompt composer: embeds the system prompt, instruction, context,
#    and current code; clean and deterministic.
# --------------------------------------------------------------------------- #


def test_build_prompt_includes_system_instruction_context_code():
    prompt = codex_client.build_prompt(
        instruction="make all players' marine deaths 1",
        context_chunks=["chunk-A: EUDLoopPlayer usage", "chunk-B: setdeaths"],
        current_code="// existing\nputs(0);\n",
    )
    assert isinstance(prompt, str)
    # The verified eps-conventions system prompt is embedded.
    assert codex_client.SYSTEM_PROMPT in prompt
    assert "make all players' marine deaths 1" in prompt
    assert "chunk-A: EUDLoopPlayer usage" in prompt
    assert "chunk-B: setdeaths" in prompt
    assert "puts(0);" in prompt


def test_build_prompt_handles_no_context_and_no_current_code():
    """The composer is robust to empty context and absent current code."""
    prompt = codex_client.build_prompt(
        instruction="add a hello trigger",
        context_chunks=[],
        current_code=None,
    )
    assert codex_client.SYSTEM_PROMPT in prompt
    assert "add a hello trigger" in prompt


def test_system_prompt_is_nonempty_eps_text():
    """SYSTEM_PROMPT is the copied (not imported) verified eps-conventions text."""
    assert isinstance(codex_client.SYSTEM_PROMPT, str)
    assert codex_client.SYSTEM_PROMPT.strip()
    assert "epScript" in codex_client.SYSTEM_PROMPT


# --------------------------------------------------------------------------- #
# Module-surface sanity (exception types are distinct, importable names exist).
# --------------------------------------------------------------------------- #


def test_exception_hierarchy_distinct():
    for exc in (CodexNotFound, CodexTimeout, CodexNoCode):
        assert issubclass(exc, Exception)
    assert len({CodexNotFound, CodexTimeout, CodexNoCode}) == 3
    for name in (
        "CodexClient",
        "CodexNotFound",
        "CodexTimeout",
        "CodexNoCode",
        "extract_code",
        "build_prompt",
        "SYSTEM_PROMPT",
    ):
        assert hasattr(codex_client, name), f"missing public name: {name}"


# --------------------------------------------------------------------------- #
# 7. LIVE smoke (opt-in): real codex round-trip; skipped by default.
# --------------------------------------------------------------------------- #


@pytest.mark.skipif(
    not (
        __import__("os").environ.get("EUD_CODEX_LIVE") == "1"
        and shutil.which("codex")
    ),
    reason="live codex smoke: set EUD_CODEX_LIVE=1 and have codex on PATH",
)
@pytest.mark.asyncio
async def test_live_codex_round_trip(tmp_path):
    resolved = shutil.which("codex")
    client = CodexClient(resolved, repo_root=str(tmp_path))
    prompt = (
        "Reply with ONLY a single fenced code block containing the one line: "
        "puts(42); — no prose, no explanation."
    )
    code = await client.generate(prompt, timeout=120.0)
    assert code.strip(), "live codex returned an empty extraction"


if __name__ == "__main__":
    raise SystemExit(pytest.main([__file__, "-q"]))
