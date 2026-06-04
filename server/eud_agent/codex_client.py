"""codex exec client: turn a composed prompt into epScript code.

This module is the only place the agent spawns the user's BYO ``codex`` CLI.
Every rule here is a measured Windows behavior, not a style choice
(rules.md "codex invocation (Windows)", architecture.md / features/02
"codex_client.py"):

* The caller passes the ALREADY-RESOLVED codex path (config.py resolves it via
  ``shutil.which("codex")`` / ``CODEX_CMD`` to the ``.cmd`` shim). We never spawn
  bare ``"codex"``; we re-validate the path exists and fail fast with
  ``CodexNotFound`` when it is empty or missing.
* Invocation is ``create_subprocess_exec(resolved, "exec",
  "--skip-git-repo-check", stdin=PIPE, stdout=PIPE, stderr=PIPE,
  cwd=repo_root)``. The FULL prompt goes to stdin (written, then CLOSED): argv
  has a 32,767-char CreateProcess limit (RAG context exceeds it) and closing
  stdin prevents codex's EOF-wait hang. Every subprocess gets an explicit stdin.
* codex stdout is noisy (session banners, token counts). We extract fenced code
  blocks only; zero fences -> ``CodexNoCode`` carrying <=500 chars of raw output.
  We NEVER return unfenced stdout.
* Timeout defaults to 600s (injectable); on timeout we kill the process and
  raise ``CodexTimeout``.

The verified eps-conventions system prompt is COPIED below from
``runner_legacy.py`` (the frozen ECA runner draft) — that module is a read-only
reference and is deliberately NOT imported (import-then-extend rule / it self-
reconfigures stdio and parses argv at import-adjacent scope).
"""

from __future__ import annotations

import asyncio
import os
import re
from pathlib import Path

# --------------------------------------------------------------------------- #
# Exceptions (module-level, distinct).
# --------------------------------------------------------------------------- #


class CodexNotFound(Exception):
    """The resolved codex path is empty or does not point at a real file."""


class CodexTimeout(Exception):
    """codex did not finish within the timeout; the process was killed."""


class CodexNoCode(Exception):
    """codex stdout contained no fenced code block (only banner noise)."""


# --------------------------------------------------------------------------- #
# System prompt — COPIED verbatim from runner_legacy.py lines 49-54.
#
# PROVENANCE: server/eud_agent/runner_legacy.py is the FROZEN verified ECA
# codex-runner draft (import-then-extend rule, rules.md). Its ``SYSTEM`` constant
# is the verified eps-conventions instruction (code-only output, player loops,
# variable declarations). We copy the text here rather than import the module.
# Keep this string in sync only if the frozen reference is ever re-verified.
# --------------------------------------------------------------------------- #

SYSTEM_PROMPT = (
    "너는 스타크래프트 EUD 맵 제작용 epScript(eps) 코드를 작성하는 어시스턴트다. "
    "아래 [참고자료]는 네이버 카페/공식 매뉴얼에서 검색한 eps/eud3 지식이다. "
    "사용자 요청을 만족하는 epScript 코드만 출력해라. 설명/마크다운 없이 코드만. "
    "플레이어 루프·변수 선언 등 eps 관례를 지켜라."
)

# Fence matcher (markdown semantics): an opening ``` (optionally language-tagged)
# at the end of its line, the body, then a CLOSING ``` that MUST start its own
# line (re.M anchors ^/$). Anchoring the close at line start prevents an inline
# ``` inside a string literal from prematurely terminating a block (A2). DOTALL
# so the body may span lines; the leading \r? tolerates CRLF before the close.
_FENCE = re.compile(r"```[^\n]*\n(.*?)\r?\n```[ \t]*$", re.S | re.M)

_RAW_SNIPPET_LIMIT = 500


# --------------------------------------------------------------------------- #
# Fence extraction.
# --------------------------------------------------------------------------- #


def extract_code(text: str | None) -> str:
    """Extract fenced code from codex stdout.

    Multiple blocks are joined with a blank line. Language tags on the fence
    (```eps) are tolerated and stripped. Surrounding banner/token-count noise is
    discarded. With ZERO fences we raise ``CodexNoCode`` carrying the first
    ``_RAW_SNIPPET_LIMIT`` chars of the raw output (never return unfenced noise).
    """
    raw = text or ""
    # Normalize CRLF -> LF first: codex on Windows can emit CRLF; an interior \r
    # surviving extraction would flow into SET bodies (A3). Do it before matching
    # so the captured body is already LF-only.
    raw = raw.replace("\r\n", "\n").replace("\r", "\n")
    blocks = [b.strip() for b in _FENCE.findall(raw)]
    blocks = [b for b in blocks if b]
    if not blocks:
        snippet = raw[:_RAW_SNIPPET_LIMIT]
        raise CodexNoCode(
            "codex produced no fenced code block; raw output (truncated):\n"
            + snippet
        )
    return "\n\n".join(blocks)


# --------------------------------------------------------------------------- #
# Prompt composer.
# --------------------------------------------------------------------------- #


def build_prompt(
    instruction: str,
    context_chunks: list[str] | None = None,
    current_code: str | None = None,
) -> str:
    """Compose the full codex prompt.

    Layout (sections the orchestrator relies on):

        <SYSTEM_PROMPT>

        [참고자료]
        <context chunks, blank-line separated; "(없음)" when empty>

        [현재 코드]            # only when current_code is provided
        <current_code>

        [요청]
        <instruction>

        [epScript 코드]

    ``context_chunks`` may be empty/None and ``current_code`` may be None; both
    degrade cleanly (the legacy runner used the same [참고자료]/[요청]/[epScript
    코드] framing).
    """
    chunks = [c for c in (context_chunks or []) if c and c.strip()]
    context = "\n\n".join(chunks) if chunks else "(없음)"

    parts = [
        SYSTEM_PROMPT,
        "",
        "[참고자료]",
        context,
    ]
    if current_code is not None and current_code.strip():
        parts += ["", "[현재 코드]", current_code]
    parts += ["", "[요청]", instruction, "", "[epScript 코드]"]
    return "\n".join(parts)


# --------------------------------------------------------------------------- #
# Client.
# --------------------------------------------------------------------------- #


class CodexClient:
    """Spawns ``codex exec`` with the prompt on stdin and extracts the code.

    ``codex_cmd`` is the resolved shim path (config.py output). It is validated
    at construction: empty or non-existent -> ``CodexNotFound`` (fail fast, never
    spawn bare ``"codex"``). ``repo_root`` becomes the subprocess ``cwd``
    (``--skip-git-repo-check`` is always passed regardless).
    """

    DEFAULT_TIMEOUT = 600.0

    def __init__(self, codex_cmd: str, repo_root: str | os.PathLike) -> None:
        if not codex_cmd:
            raise CodexNotFound(
                "codex path is empty: shutil.which('codex') did not resolve. "
                "Install codex or set CODEX_CMD / agent.cfg codex_cmd to the "
                "codex.cmd shim path."
            )
        if not Path(codex_cmd).is_file():
            raise CodexNotFound(
                f"codex path does not exist: {codex_cmd} "
                "(set CODEX_CMD to the real codex.cmd shim)."
            )
        self.codex_cmd = str(codex_cmd)
        self.repo_root = str(repo_root)

    async def generate(self, prompt: str, *, timeout: float | None = None) -> str:
        """Run ``codex exec`` with ``prompt`` on stdin; return extracted code.

        Raises ``CodexTimeout`` (process killed) on timeout, ``CodexNoCode`` when
        stdout has no fenced block.
        """
        if timeout is None:
            timeout = self.DEFAULT_TIMEOUT

        proc = await asyncio.create_subprocess_exec(
            self.codex_cmd,
            "exec",
            "--skip-git-repo-check",
            stdin=asyncio.subprocess.PIPE,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            cwd=self.repo_root,
        )

        # Full prompt via stdin, then CLOSE it (argv 32,767-char limit; closing
        # stdin prevents codex's EOF-wait hang). We write+close explicitly rather
        # than via communicate(input=...) so the close is unconditional and
        # observable, then drain the pipes with communicate().
        #
        # B1: a .cmd that exits WITHOUT reading stdin (e.g. an arg/usage error)
        # with a large prompt (the 300KB RAG-context case) breaks the pipe mid-
        # write -> BrokenPipeError / ConnectionResetError [WinError 109]. Swallow
        # those and FALL THROUGH to communicate(): the process's real exit and
        # stderr then surface as a clean CodexNoCode (with stderr tail) instead of
        # a raw OS exception. (drain() only waits on the asyncio high-water mark,
        # so this write path is deadlock-safe — only the early-exit case needs the
        # guard.)
        prompt_bytes = (prompt or "").encode("utf-8")
        if proc.stdin is not None:
            try:
                proc.stdin.write(prompt_bytes)
                await proc.stdin.drain()
                proc.stdin.close()
            except (BrokenPipeError, ConnectionResetError):
                pass

        try:
            stdout_b, stderr_b = await asyncio.wait_for(
                proc.communicate(), timeout=timeout
            )
        except TimeoutError as exc:
            # A1: reap the killed process (await wait) so the proactor transport
            # is closed and no un-reaped ResourceWarning leaks.
            await self._kill_and_reap(proc)
            raise CodexTimeout(
                f"codex exec timed out after {timeout:.0f}s (process killed)."
            ) from exc

        stdout = (stdout_b or b"").decode("utf-8", errors="replace")
        stderr = (stderr_b or b"").decode("utf-8", errors="replace")

        try:
            return extract_code(stdout)
        except CodexNoCode as exc:
            # Surface stderr tail too: a non-zero exit usually explains the empty
            # output better than the (banner-only) stdout snippet.
            tail = stderr.strip()[-_RAW_SNIPPET_LIMIT:]
            if tail:
                raise CodexNoCode(f"{exc}\n--- stderr (tail) ---\n{tail}") from exc
            raise

    @staticmethod
    async def _kill_and_reap(proc) -> None:
        try:
            proc.kill()
        except ProcessLookupError:
            pass
        # Reap: await the process so its transport is closed (prevents un-reaped
        # ResourceWarnings on the proactor loop). Tolerate a process that already
        # exited or a wait() that errors after kill.
        try:
            await proc.wait()
        except (ProcessLookupError, ChildProcessError):
            pass
