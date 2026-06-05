"""The v2 agent engine: ``AgentRunner`` interface + ``CodexSDKRunner`` (features/05
"Engine (single path)").

The v2 brain replaces the v1 single-shot instruct flow with an agentic loop: codex
autonomously calls editor tools (via the eud-tools MCP shim) in real time while the
server stays the policy layer (validation, journal, budgets, plan gating). This
module owns ONLY the codex side: starting/resuming a thread, streaming its events
to the panel, and detecting the ``propose_plan`` turn-ender. All tool logic lives
in ``tools.py``; the journal/changeset live in ``journal.py``; the WS state machine
lives in ``app.py``.

Threading model (documented decision)
-------------------------------------
The official Codex Python SDK (``openai_codex``, pinned 0.1.0b3) is SYNCHRONOUS —
the EUD-053 spike used ``Codex``/``thread.turn(...).stream()`` blocking calls. Rather
than fight FastAPI's event loop with the async client, a turn runs in a worker
thread (``asyncio.to_thread``); the blocking ``.stream()`` loop forwards each event
back onto the WS event loop via ``run_coroutine_threadsafe`` (the loop is captured
when the turn starts). This is the spike-proven shape, kept intact (rules.md /
EUD-053: "do NOT re-research the SDK").

System prompt + per-thread MCP injection
----------------------------------------
The FIRST turn passes ``base_instructions`` (the system prompt: tool catalog +
project state + RAG context + triage rules, built by ``app.py``) and injects the
eud-tools MCP server per-thread via ``thread_start(config={"mcp_servers": {...}})``
(NO global ``codex mcp add`` — the spike proved config injection). The shim is
``sys.executable -m eud_agent.mcp_shim`` with ``EUD_DATA_DIR`` + ``EUD_REQUEST_ID``
in its env (so it locates ``server.ready`` and pins the per-session request id).
The thread id is retained per panel session so follow-up turns
(``thread_resume``) continue the conversation: the FIRST chat starts the thread,
every later chat resumes it (conversation continuity, EUD-064 — the engine routes
start-vs-resume). The retained id is dropped only by ``reset_thread`` (panel
``reset{}``); even a stray ``start_turn`` while a thread is retained REUSES it
rather than discarding the history. ``has_thread`` lets the engine query whether a
thread is live.

codex-environment isolation (EUD-062)
-------------------------------------
A live E2E found that agent codex threads INHERIT the operator's entire personal
codex environment: the global ``~/.codex/config.toml`` MCP servers
(playwright/pencil/node_repl...), enabled plugins, and personal skills. The agent
must run with ONLY what THIS project configures. Isolation is composed into the
spawn at TWO layers (defense-in-depth), driven by :class:`CodexIsolation`:

* **launch-level** ``CodexConfig.config_overrides`` — a tuple the SDK forwards as
  ``codex --config k=v ... app-server`` (verified in the pinned SDK source,
  ``client.py`` ``CodexClient.start``: each override is expanded to
  ``["--config", kv]`` BEFORE the ``app-server`` subcommand). These overrides are
  per-PROCESS (one ``Codex`` client = one ``app-server``). We pass:
  (a) ``mcp_servers={...}`` as a WHOLE-TABLE override (codex ``-c`` whole-table
  replace semantics) so the personal ``mcp_servers`` table is REPLACED, not merged
  — only ``eud-tools`` remains; and (b) ``features.plugins=false`` (the stable
  feature flag) to disable plugins.
* **per-thread** ``thread_start(config={"mcp_servers": {...}})`` — kept from the
  EUD-053 spike. The eud-tools entry here carries an ``EUD_REQUEST_ID`` env value
  pinned at thread creation. With conversation continuity (EUD-064) the codex
  thread now PERSISTS across chats (only the FIRST chat starts it; later chats
  resume), so this pinned id goes STALE from the second chat on — the shim re-reads
  it only once, at thread spawn. The server therefore resolves the LIVE request id
  at tool-call time (the tool endpoint stamps the engine's CURRENT id, ignoring the
  shim-supplied one for an active session); this layer's env id is the legacy
  headless fallback only. The request id stays in this layer because the
  launch-level override is fixed at ``Codex`` construction. Both layers name the
  table ``eud-tools`` so the per-thread layer simply supplies the env for the same
  server the launch-level override admits.

What is NOT isolated, and why:

* **Skills** — there is NO documented blanket skills-disable for ``app-server``.
  ``codex exec --ignore-user-config`` exists but is EXEC-ONLY: probed on this
  machine's CLI, ``codex --ignore-user-config ...`` and
  ``codex app-server --ignore-user-config ...`` both error with "unexpected
  argument", and ``codex app-server --help`` does not list it; the SDK spawns
  ``app-server``, so that flag is unavailable. Per-skill ``[[skills.config]]
  enabled=false`` requires enumerating each skill by path (no wildcard). Personal
  skills therefore remain loadable; this is a documented limitation. The isolation
  settings are INJECTABLE (``CodexSDKRunner(isolation=...)`` /
  :data:`CodexIsolation.extra_overrides`) so a future skills mechanism — or the
  live E2E — can add overrides without touching this module.
* **Dedicated ``CODEX_HOME`` relocation** was REJECTED: a separate codex home would
  diverge ``auth.json`` token rotation from the operator's account (the BYO account
  re-rotates the refresh token), so it is not implemented even though it would
  isolate skills. Documenting the skills limitation honestly is preferred.

propose_plan ends the turn
--------------------------
``propose_plan`` is a flow tool that ends the codex turn for user review. The runner
watches the stream for the ``propose_plan`` MCP tool call and, when the turn
completes after one, returns ``{"kind": "plan", "markdown": ...}`` so ``app.py``
emits ``plan{markdown, revision}``. Otherwise it returns ``{"kind": "answer"}`` (no
mutations) or ``{"kind": "apply"}`` (the turn journaled writes).

cancel semantics
----------------
``cancel()`` interrupts the in-flight ``TurnHandle`` (``handle.interrupt()``); the
journal entries already written PERSIST by design (rules.md: a cancelled turn must
not strand the journal — the user still reviews/rolls back what was applied). A
``CodexSDKRunner`` with no live handle is a no-op cancel.
"""

from __future__ import annotations

import asyncio
import os
import sys
import threading
from abc import ABC, abstractmethod
from collections.abc import Awaitable, Callable
from dataclasses import dataclass, field

# The plan tool name (mirrors tools.PLAN_TOOL) — detected in the stream.
PLAN_TOOL = "propose_plan"

# The MCP server name the eud-tools shim registers under (both isolation layers
# name the same table key; see the module docstring).
EUD_TOOLS_SERVER = "eud-tools"
# The shim launch command tail (``python -m eud_agent.mcp_shim``).
MCP_SHIM_ARGS = ["-m", "eud_agent.mcp_shim"]


@dataclass(frozen=True)
class CodexIsolation:
    """Knobs for isolating an agent codex thread from the operator's codex env.

    Drives the launch-level ``config_overrides`` the runner composes (see the
    module docstring). Injectable into :class:`CodexSDKRunner` so the live E2E can
    flip settings (e.g. once a skills-disable mechanism is found) without editing
    this module.

    * ``replace_mcp_table`` — emit the WHOLE-TABLE ``mcp_servers={...}`` override so
      the personal MCP table is replaced (only eud-tools survives). Default True.
    * ``disable_plugins`` — emit ``features.plugins=false``. Default True.
    * ``extra_overrides`` — additional raw ``key=tomlvalue`` override strings
      appended verbatim (e.g. a future per-skill disable, or a live-E2E probe).
    """

    replace_mcp_table: bool = True
    disable_plugins: bool = True
    extra_overrides: tuple[str, ...] = field(default_factory=tuple)


# The default isolation applied when a runner is constructed without one.
DEFAULT_ISOLATION = CodexIsolation()


def _toml_inline(value) -> str:
    """Serialize ``value`` as an inline-TOML literal for a ``-c key=<value>`` arg.

    Handles only the shapes the isolation config needs (str / bool / int / list /
    dict), producing inline tables (``{ k = v }``) and arrays (``[ a, b ]``) that
    round-trip through ``tomllib``. The codex ``-c`` parser treats the value as
    TOML, so an inline table for the whole ``mcp_servers`` map yields a whole-table
    override. ``codex_bin`` paths on Windows contain backslashes, so strings are
    emitted as TOML *basic* strings with the two TOML escapes that matter here
    (``\\`` and ``"``).
    """
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, int):
        return str(value)
    if isinstance(value, str):
        escaped = value.replace("\\", "\\\\").replace('"', '\\"')
        return f'"{escaped}"'
    if isinstance(value, (list, tuple)):
        return "[" + ", ".join(_toml_inline(v) for v in value) + "]"
    if isinstance(value, dict):
        items = ", ".join(
            f"{_toml_key(k)} = {_toml_inline(v)}" for k, v in value.items()
        )
        return "{" + items + "}"
    raise TypeError(f"unsupported TOML value: {type(value).__name__}")


def _toml_key(key: str) -> str:
    """A TOML key: bare when it is a simple identifier, else a quoted key.

    ``eud-tools`` contains a dash, which is NOT a bare-key char, so it must be
    quoted (``"eud-tools"``) for the value to parse.
    """
    if key and all(c.isalnum() or c == "_" for c in key):
        return key
    escaped = key.replace("\\", "\\\\").replace('"', '\\"')
    return f'"{escaped}"'

# Default model for the codex thread (None -> SDK/account default). Pinned to None
# so the BYO account's configured default is used (the spike left it default).
DEFAULT_MODEL: str | None = None

SendCallback = Callable[[dict], Awaitable[None]]


class AgentRunner(ABC):
    """One agentic conversation session (one panel WS connection).

    ``start_turn`` runs the FIRST turn (system prompt + per-thread MCP injection);
    ``resume_turn`` continues the retained thread (plan feedback/approval, follow-up
    chat). Both stream ``agent_event``/``answer``/``plan`` through the injected
    ``send`` callback and RETURN a small dict describing how the turn ended
    (``{"kind": "answer"|"apply"|"plan", "markdown"?: str}``) so the WS state machine
    can route to ``answer``/``changeset``/``plan``. ``cancel`` interrupts the
    in-flight turn without stranding the journal.

    Conversation continuity (EUD-064): ``has_thread`` reports whether a codex
    thread is already retained (the engine starts the FIRST chat then resumes every
    later one); ``reset_thread`` drops the retained id so the next chat starts a
    fresh conversation (panel ``reset{}``).
    """

    @abstractmethod
    async def start_turn(
        self, text: str, *, request_id: str, system_prompt: str
    ) -> dict:
        ...

    @abstractmethod
    async def resume_turn(self, text: str, *, request_id: str) -> dict:
        ...

    @abstractmethod
    def has_thread(self) -> bool:
        ...

    @abstractmethod
    def reset_thread(self) -> None:
        ...

    @abstractmethod
    def cancel(self) -> None:
        ...


# --------------------------------------------------------------------------- #
# CodexSDKRunner.
# --------------------------------------------------------------------------- #


class CodexSDKRunner(AgentRunner):
    """``AgentRunner`` over the official Codex Python SDK (EUD-053 shapes).

    Constructed with the ``tool_layer`` (unused directly here — the MCP shim
    forwards to the FastAPI tool endpoints — but kept for parity with the
    interface/factory and any future direct introspection), the ``send`` callback,
    a ``build_system_prompt`` builder (called by ``app.py``, not here), the resolved
    ``codex_bin`` (rules.md: never bare ``codex``), and the editor ``data_dir`` (so
    the shim can locate ``server.ready``).
    """

    def __init__(
        self,
        *,
        tool_layer,
        send: SendCallback,
        build_system_prompt,
        codex_bin: str,
        data_dir: str | os.PathLike,
        model: str | None = DEFAULT_MODEL,
        isolation: CodexIsolation = DEFAULT_ISOLATION,
    ) -> None:
        self.tool_layer = tool_layer
        self._send = send
        self._build_system_prompt = build_system_prompt
        self._codex_bin = codex_bin
        self._data_dir = str(data_dir)
        self._model = model
        self._isolation = isolation
        # SDK objects are created lazily on the first turn (so importing this
        # module — and constructing the runner in create_app — never spawns codex).
        self._codex = None
        self._thread = None
        self._thread_id: str | None = None
        # The live TurnHandle (set while a turn streams) + a lock so cancel reads
        # it safely from the WS loop thread while the worker thread runs the turn.
        self._handle = None
        self._handle_lock = threading.Lock()

    # ------------------------------------------------------------- public API
    async def start_turn(
        self, text: str, *, request_id: str, system_prompt: str
    ) -> dict:
        loop = asyncio.get_running_loop()
        return await asyncio.to_thread(
            self._run_turn_blocking,
            text,
            request_id=request_id,
            system_prompt=system_prompt,
            resume=False,
            loop=loop,
        )

    async def resume_turn(self, text: str, *, request_id: str) -> dict:
        loop = asyncio.get_running_loop()
        return await asyncio.to_thread(
            self._run_turn_blocking,
            text,
            request_id=request_id,
            system_prompt=None,
            resume=True,
            loop=loop,
        )

    def cancel(self) -> None:
        with self._handle_lock:
            handle = self._handle
        if handle is None:
            return
        try:
            handle.interrupt()
        except Exception:  # noqa: BLE001 - cancel is best-effort; never raise
            pass

    def has_thread(self) -> bool:
        """Whether a codex thread is retained (continuity, EUD-064)."""
        return self._thread_id is not None

    def reset_thread(self) -> None:
        """Drop the retained thread so the next turn starts a fresh conversation.

        Panel ``reset{}`` (EUD-064). Only the thread id is cleared — the lazily
        built ``Codex`` client (and its isolated ``app-server`` process) is kept so
        the next ``thread_start`` reuses it without a respawn.
        """
        self._thread_id = None
        self._thread = None

    # ------------------------------------------------------------- internals
    def _ensure_codex(self):
        from openai_codex import Codex

        if self._codex is None:
            self._codex = Codex(config=self._codex_config())
        return self._codex

    def _codex_config(self):
        """The launch-level ``CodexConfig`` carrying the isolation overrides.

        Composes the per-PROCESS ``config_overrides`` (EUD-062): a whole-table
        ``mcp_servers`` replacement (only eud-tools) + ``features.plugins=false``,
        plus any injected ``extra_overrides``. ``launch_args_override`` is left
        None so the SDK builds the normal ``codex --config ... app-server``
        invocation (``--ignore-user-config`` is exec-only and rejected by
        app-server — see the module docstring).
        """
        from openai_codex import CodexConfig

        return CodexConfig(
            codex_bin=self._codex_bin,
            config_overrides=self._isolation_overrides(),
        )

    def _isolation_overrides(self) -> tuple[str, ...]:
        """Build the ``--config k=v`` override tuple from the isolation knobs."""
        iso = self._isolation
        overrides: list[str] = []
        if iso.replace_mcp_table:
            # Whole-table override: REPLACES the operator's personal mcp_servers
            # table (codex -c whole-table semantics) so only eud-tools survives.
            # No per-request env here — that lives in the per-thread config layer.
            table = {
                EUD_TOOLS_SERVER: {
                    "command": sys.executable,
                    "args": list(MCP_SHIM_ARGS),
                }
            }
            overrides.append(f"mcp_servers={_toml_inline(table)}")
        if iso.disable_plugins:
            overrides.append("features.plugins=false")
        overrides.extend(iso.extra_overrides)
        return tuple(overrides)

    def _thread_config(self, request_id: str) -> dict:
        """Per-thread MCP injection of the eud-tools shim (EUD-053 spike shape).

        Carries the LIVE ``EUD_REQUEST_ID`` (per chat-session) — see the module
        docstring for why this stays in the thread layer, not the launch override.
        """
        return {
            "mcp_servers": {
                EUD_TOOLS_SERVER: {
                    "command": sys.executable,
                    "args": list(MCP_SHIM_ARGS),
                    "env": {
                        "EUD_DATA_DIR": self._data_dir,
                        "EUD_REQUEST_ID": request_id,
                    },
                }
            }
        }

    def _run_turn_blocking(
        self,
        text: str,
        *,
        request_id: str,
        system_prompt: str | None,
        resume: bool,
        loop: asyncio.AbstractEventLoop,
    ) -> dict:
        """Run ONE codex turn (blocking, in a worker thread) and stream events.

        Starts (or resumes) the thread, then consumes ``turn.stream()`` forwarding
        each event to the WS loop as ``agent_event``. Returns the turn-end dict.

        Continuity (EUD-064): a RETAINED thread is always resumed — even on a
        ``resume=False`` call — so a stray ``start_turn`` cannot discard the
        conversation history; a fresh ``thread_start`` happens ONLY when no thread
        is retained (the first chat, or after ``reset_thread``). ``resume`` from the
        engine still signals "no system prompt"; the retention check is what guards
        the history. ``base_instructions`` therefore apply only to the first thread.
        """
        codex = self._ensure_codex()

        if self._thread_id is not None:
            thread = codex.thread_resume(self._thread_id)
        else:
            kwargs: dict = {"config": self._thread_config(request_id)}
            if system_prompt:
                kwargs["base_instructions"] = system_prompt
            if self._model:
                kwargs["model"] = self._model
            thread = codex.thread_start(**kwargs)
            self._thread_id = thread.id
        self._thread = thread

        handle = thread.turn(text)
        with self._handle_lock:
            self._handle = handle

        plan_markdown: str | None = None
        saw_mutation = False
        answer_parts: list[str] = []
        try:
            for event in handle.stream():
                kind, detail, info = _classify_event(event)
                self._emit_threadsafe(
                    loop, {"type": "agent_event", "kind": kind, "detail": detail}
                )
                if info.get("plan_markdown") is not None:
                    plan_markdown = info["plan_markdown"]
                if info.get("mutation"):
                    saw_mutation = True
                if info.get("answer_text"):
                    answer_parts.append(info["answer_text"])
        finally:
            with self._handle_lock:
                self._handle = None

        if plan_markdown is not None:
            return {"kind": "plan", "markdown": plan_markdown}
        answer = " ".join(p for p in answer_parts if p).strip()
        if answer:
            self._emit_threadsafe(loop, {"type": "answer", "text": answer})
        if saw_mutation:
            return {"kind": "apply"}
        return {"kind": "answer"}

    def _emit_threadsafe(
        self, loop: asyncio.AbstractEventLoop, event: dict
    ) -> None:
        """Schedule ``send(event)`` on the WS event loop from the worker thread."""
        try:
            fut = asyncio.run_coroutine_threadsafe(self._send(event), loop)
            fut.result(timeout=10.0)
        except Exception:  # noqa: BLE001 - a dead WS must not break the turn loop
            pass


# --------------------------------------------------------------------------- #
# Event classification (spike-proven shapes; tolerant of model variance).
# --------------------------------------------------------------------------- #


def _classify_event(event) -> tuple[str, str, dict]:
    """Map a streamed SDK ``Notification`` to ``(kind, detail, info)``.

    ``kind`` is the panel-facing ``agent_event`` kind (thinking/tool_call/
    tool_result/turn_done/...); ``detail`` is a short string; ``info`` carries
    side signals the turn loop consumes: ``plan_markdown`` (a propose_plan tool
    call ended the turn), ``mutation`` (a write tool ran), ``answer_text`` (an
    agent message chunk). All access is defensive (``getattr``) so a model/SDK
    shape change degrades to a generic event rather than crashing the turn.
    """
    method = getattr(event, "method", "") or ""
    info: dict = {}

    if method == "item/completed":
        root = _item_root(event)
        rtype = getattr(root, "type", None)
        if rtype == "mcpToolCall":
            tool = getattr(root, "tool", "") or ""
            if tool == PLAN_TOOL or tool.endswith(f"__{PLAN_TOOL}"):
                info["plan_markdown"] = _plan_markdown_from(root)
            elif _is_mutation_tool(tool):
                info["mutation"] = True
            return ("tool_result", tool, info)
        if rtype == "agentMessage":
            text = getattr(root, "text", "") or ""
            info["answer_text"] = text
            return ("answer", "", info)
        return ("item_completed", str(rtype or ""), info)

    if method == "item/started":
        root = _item_root(event)
        rtype = getattr(root, "type", None)
        if rtype == "mcpToolCall":
            return ("tool_call", getattr(root, "tool", "") or "", info)
        return ("item_started", str(rtype or ""), info)

    if method == "turn/started":
        return ("thinking", "", info)
    if method == "turn/completed":
        return ("turn_done", "", info)
    if method.endswith("reasoning/summaryTextDelta") or method.endswith(
        "reasoning/textDelta"
    ):
        # Reasoning-text chunk (EUD-063): ReasoningSummaryTextDelta /
        # ReasoningTextDelta carry the text in payload.delta. Forward it as the
        # ``reasoning`` kind so the panel can render the model's thinking.
        return ("reasoning", _delta_text(event), info)
    if method.endswith("agentMessage/delta"):
        # Answer-text chunk (EUD-063): AgentMessageDelta carries the streamed
        # answer in payload.delta — forward it instead of dropping it.
        return ("delta", _delta_text(event), info)
    if method.endswith("tokenUsage/updated"):
        return ("token_usage", "", info)
    return ("event", method, info)


def _delta_text(event) -> str:
    """The ``delta`` text on a streamed delta notification, defensively.

    The reasoning/answer delta notifications expose ``delta: str`` directly on the
    notification payload (``event.payload.delta``). A missing field degrades to an
    empty string so a model/SDK shape change never crashes the turn loop.
    """
    payload = getattr(event, "payload", None)
    return getattr(payload, "delta", "") or ""


def _item_root(event):
    """The item payload root (``event.payload.item.root``), defensively."""
    payload = getattr(event, "payload", None)
    item = getattr(payload, "item", None)
    return getattr(item, "root", item)


def _plan_markdown_from(root) -> str:
    """Extract the markdown a propose_plan call carried.

    Prefer the tool RESULT (the server returns ``{markdown}``); fall back to the
    call ARGS (``{"markdown": ...}``). Both are accessed defensively.
    """
    for attr in ("result", "output"):
        val = getattr(root, attr, None)
        md = _dig_markdown(val)
        if md:
            return md
    args = getattr(root, "arguments", None) or getattr(root, "args", None)
    md = _dig_markdown(args)
    return md or ""


def _dig_markdown(val) -> str:
    """Pull a ``markdown`` field out of a dict / JSON-string / object."""
    if val is None:
        return ""
    if isinstance(val, dict):
        return str(val.get("markdown") or "")
    if isinstance(val, str):
        s = val.strip()
        if s.startswith("{"):
            import json

            try:
                d = json.loads(s)
            except ValueError:
                return ""
            if isinstance(d, dict):
                return str(d.get("markdown") or "")
        return ""
    md = getattr(val, "markdown", None)
    return str(md) if md else ""


# Mutation tool names (mirror tools.WRITE_TOOLS minus build_run, which the journal
# skips but still counts as a mutation for the apply-kind signal). Imported lazily
# to avoid a hard cycle at module import.
def _is_mutation_tool(tool: str) -> bool:
    from .tools import WRITE_TOOLS

    bare = tool.rsplit("__", 1)[-1]  # strip any "server__tool" namespacing
    return bare in WRITE_TOOLS
