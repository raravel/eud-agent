"""Per-instruct orchestration state machine (features/02 "orchestrator.py").

The orchestrator is the server-side brain of a single panel request. It drives
the pipeline ``rag (optional) -> codex -> lsp -> diff -> done`` for an
``instruct`` and the ``set``/``neweps`` apply path, emitting the WS protocol
events the panel consumes (architecture.md "WebSocket protocol") through an
async ``send`` callback. The app layer wires ``send`` to a broadcaster so two
connected clients both receive every event.

Design constraints honored here (rules.md "Server and panel", features/02):

  * **One in-flight instruct.** A single ``asyncio.Lock`` (non-blocking acquire)
    gates ``instruct``; a second concurrent request emits ``error {message:
    "busy"}`` and touches neither rag nor codex. apply/status/list are NOT gated
    by that lock (they are cheap bridge round-trips).
  * **Never block the event loop.** Every synchronous collaborator call
    (``bridge_io`` file IPC, ``rag.search``) runs in a thread via
    ``asyncio.to_thread``. ``codex_client.generate`` is already a coroutine.
  * **RAG is optional + degrades.** With ``use_context`` we call ``rag.search``;
    ``RagUnavailable`` degrades to a no-context codex run WITH a progress note
    (features/02 edge case) rather than failing the instruct.
  * **LSP is advisory + optional.** ``lsp_gate`` may not be installed; we import
    it lazily inside ``try/except ImportError`` and, on absence (or any failure),
    emit ``progress {stage: "lsp", detail: "skipped"}`` with ``diagnostics=[]``.
    Diagnostics annotate, never block (rules.md).
  * **Diff for SET targets.** For a ``set``-style instruct we fetch the current
    content via ``bridge_io.get`` and produce a unified diff
    (``difflib.unified_diff``) labelled with the target name.
  * **Busy/timeout translation.** apply forwards an ``on_busy`` callback to
    ``bridge_io.set``/``neweps``; a ``BridgeBusy`` (poll timeout while the editor
    compiles) surfaces ``progress {stage: "waiting_build"}`` then
    ``error {message: "editor busy"}``. A ``BridgeError`` surfaces
    ``error {message: <bridge message>}``.
"""

from __future__ import annotations

import asyncio
import difflib
from typing import Any

from . import codex_client, rag
from .bridge_io import BridgeBusy, BridgeError

# WS protocol constant: the panel renders generated code as epScript.
_LANG = "eps"
# RAG retrieval depth (rag.search default; kept explicit here).
_RAG_K = 5


class Orchestrator:
    """Drives one instruct/apply request and emits WS events via ``send``.

    Parameters
    ----------
    bridge:
        A ``bridge_io.BridgeIO`` instance (the only file-IPC writer). Its
        ``get``/``set``/``neweps``/``status``/``list_files`` calls are synchronous
        and run in a thread executor.
    codex:
        A ``codex_client.CodexClient`` (or compatible) with an async
        ``generate(prompt, *, timeout=None) -> str``.
    rag_db:
        Path to the ECA chromadb store, forwarded to ``rag.search``.
    send:
        Async callback ``send(event: dict) -> None`` — the WS broadcaster. Every
        protocol event is delivered through it.
    """

    def __init__(self, bridge, codex, *, rag_db: str, send) -> None:
        self._bridge = bridge
        self._codex = codex
        self._rag_db = rag_db
        self._send = send
        # Non-blocking gate: only ONE instruct may be in flight at a time.
        self._instruct_lock = asyncio.Lock()

    # ------------------------------------------------------------------ events
    async def _emit(self, event: dict) -> None:
        await self._send(event)

    async def _progress(self, stage: str, detail: str | None = None) -> None:
        event: dict[str, Any] = {"type": "progress", "stage": stage}
        if detail is not None:
            event["detail"] = detail
        await self._emit(event)

    async def _error(self, message: str) -> None:
        await self._emit({"type": "error", "message": message})

    # --------------------------------------------------------------- instruct
    async def instruct(
        self, instruction: str, target: str, *, use_context: bool
    ) -> None:
        """Run ``rag -> codex -> lsp -> diff -> done`` for one instruction.

        ONE in-flight at a time: a second concurrent call emits ``error
        {message: "busy"}`` and returns without running rag/codex.
        """
        if self._instruct_lock.locked():
            await self._error("busy")
            return
        async with self._instruct_lock:
            await self._run_instruct(instruction, target, use_context)

    async def _run_instruct(
        self, instruction: str, target: str, use_context: bool
    ) -> None:
        # --- codex availability gate (features/02 "codex absent: ... instruct
        # returns error event"). create_app keeps the server up with codex=None
        # when the shim is unresolved; an instruct must surface a clean error
        # rather than crash the WS loop on None.generate. ---
        if self._codex is None:
            await self._error(
                "codex not available — install codex or set CODEX_CMD"
            )
            return

        # --- rag (optional) ---
        context_chunks: list[str] = []
        if use_context:
            context_chunks = await self._rag_stage(instruction)

        # --- diff prep: fetch current content for a SET-style target ---
        # We fetch before codex so the bridge round-trip overlaps nothing risky
        # and the current code can also feed the prompt's [현재 코드] section.
        try:
            current = await asyncio.to_thread(self._bridge.get, target)
        except BridgeBusy:
            await self._progress("waiting_build")
            await self._error("editor busy")
            return
        except BridgeError as exc:
            await self._error(str(exc))
            return

        # --- codex ---
        await self._progress("codex")
        prompt = codex_client.build_prompt(
            instruction, context_chunks=context_chunks, current_code=current
        )
        try:
            code = await self._codex.generate(prompt)
        except (
            codex_client.CodexNotFound,
            codex_client.CodexTimeout,
            codex_client.CodexNoCode,
        ) as exc:
            await self._error(str(exc))
            return

        # --- lsp (advisory, optional) ---
        diagnostics = await self._lsp_stage(code)

        # --- diff + done ---
        diff = self._unified_diff(current, code, target)
        await self._emit(
            {
                "type": "code",
                "code": code,
                "lang": _LANG,
                "diff": diff,
                "diagnostics": diagnostics,
            }
        )

    async def _rag_stage(self, instruction: str) -> list[str]:
        """Run the RAG search; degrade to no-context on RagUnavailable.

        Emits ``progress {stage: "rag"}`` (with a degrade detail on failure) and
        returns the context text chunks (empty on degrade).
        """
        await self._progress("rag")
        try:
            results = await asyncio.to_thread(
                rag.search, instruction, _RAG_K, rag_db=self._rag_db
            )
        except rag.RagUnavailable as exc:
            # features/02 edge case: degrade to no-context with a progress note.
            await self._progress("rag", f"unavailable, no context: {exc}")
            return []
        return [r.get("text", "") for r in results if r.get("text")]

    async def _lsp_stage(self, code: str) -> list[dict]:
        """Advisory diagnostics; absence/any failure -> skipped + []."""
        try:
            from . import lsp_gate  # lazy: the package may not be installed
        except ImportError:
            await self._progress("lsp", "skipped")
            return []
        await self._progress("lsp")
        try:
            diagnostics = await asyncio.to_thread(lsp_gate.diagnose, code)
        except Exception:  # noqa: BLE001 - advisory only, never block the flow
            await self._progress("lsp", "skipped")
            return []
        return diagnostics or []

    @staticmethod
    def _unified_diff(current: str, new: str, target: str) -> str:
        """Unified diff of ``current`` -> ``new`` labelled with the target name."""
        diff = difflib.unified_diff(
            current.splitlines(keepends=True),
            new.splitlines(keepends=True),
            fromfile=f"a/{target}",
            tofile=f"b/{target}",
        )
        return "".join(diff)

    # ------------------------------------------------------------------ apply
    async def apply(self, *, mode: str, target: str, code: str) -> None:
        """Apply generated code via the bridge (``set`` or ``neweps``).

        On success emits ``applied {target}``. A ``BridgeBusy`` surfaces a
        ``waiting_build`` progress note (from the bridge's ``on_busy`` callback)
        then ``error {message: "editor busy"}``; a ``BridgeError`` surfaces
        ``error {message: <bridge message>}``.
        """
        # The bridge fires this (synchronously, from the executor thread) the
        # first time a build is detected mid-poll. Schedule the WS note back on
        # the event loop without blocking the worker thread.
        loop = asyncio.get_running_loop()

        def on_busy() -> None:
            asyncio.run_coroutine_threadsafe(
                self._progress("waiting_build"), loop
            )

        try:
            if mode == "neweps":
                await asyncio.to_thread(
                    self._bridge.neweps, target, code, on_busy=on_busy
                )
            else:
                await asyncio.to_thread(
                    self._bridge.set, target, code, on_busy=on_busy
                )
        except BridgeBusy:
            await self._error("editor busy")
            return
        except BridgeError as exc:
            await self._error(str(exc))
            return
        await self._emit({"type": "applied", "target": target})

    # ----------------------------------------------------------- status / list
    async def status(self) -> None:
        """Emit ``status {compiling, project}`` from the bridge STATUS reply."""
        try:
            reply = await asyncio.to_thread(self._bridge.status)
        except BridgeBusy:
            await self._error("editor busy")
            return
        except BridgeError as exc:
            await self._error(str(exc))
            return
        compiling, project = _parse_status(reply)
        await self._emit(
            {"type": "status", "compiling": compiling, "project": project}
        )

    async def list_files(self) -> None:
        """Emit ``list {files}`` from the bridge LIST reply (empty = zero files)."""
        try:
            files = await asyncio.to_thread(self._bridge.list_files)
        except BridgeBusy:
            await self._error("editor busy")
            return
        except BridgeError as exc:
            await self._error(str(exc))
            return
        await self._emit({"type": "list", "files": files})


def _parse_status(reply: str) -> tuple[bool, str]:
    """Parse the bridge STATUS reply (``compiling=.. / project=.. / version=..``).

    Tolerant: unknown/missing keys degrade to ``(False, "")``. ``compiling`` is
    true only for the literal ``true`` (case-insensitive).
    """
    compiling = False
    project = ""
    for line in reply.splitlines():
        key, sep, value = line.partition("=")
        if not sep:
            continue
        key = key.strip().lower()
        value = value.strip()
        if key == "compiling":
            compiling = value.lower() == "true"
        elif key == "project":
            project = value
    return compiling, project
