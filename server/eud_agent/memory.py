"""Per-map-project memory store (features/07 "Project Memory").

Codex forgets the user's map project between requests, so this store gives every
project a small, durable memory that is INJECTED into every prompt and that codex
updates autonomously via the ``memory_write`` tool. The store owns paths, name
sanitization, atomic UTF-8 IO, the append-only episode log, the LIST-hash
staleness signal, and the rendered ``[project memory]`` prompt section.

Storage layout
--------------
State lives under ``<data-dir>/harness/<sanitized-project-name>/`` (a sibling of
``journal/``), created on demand on the first write:

* ``resources.md`` / ``structure.md`` / ``conventions.md`` / ``lessons.md`` —
  the four codex/panel-editable markdown files (full-file replace semantics);
* ``episodes.jsonl`` — append-only request history, one JSON object per line;
* ``meta.json`` — ``{"version": 1, "list_hash": "<sha256>", "list_hash_ts":
  "<ISO8601>"}`` for structure-staleness detection.

All files are UTF-8 **without BOM** (rules.md "IPC and encoding"), markdown/meta
writes are atomic (temp + :func:`os.replace`, mirroring :mod:`eud_agent.journal`).

Sanitization + disabled state
-----------------------------
The project name comes from bridge STATUS. Characters invalid in Windows file
names (``<>:"/\\|?*`` and control chars) become ``_`` and trailing dots/spaces
are stripped. A name that sanitizes to EMPTY (no project open) DISABLES the
store: reads return ``""``, writes/appends return an explicit non-ok result
(never raising), and the rendered section degrades to ``(no project memory)`` —
the same best-effort contract as RAG.

Write cap
---------
Each markdown file is capped at :data:`CONTENT_CAP_BYTES` (8 192) UTF-8 BYTES
(multi-byte text counts as bytes, not characters). An over-cap write is REJECTED
with an explicit :class:`WriteResult` telling codex to condense — the prior file
content is left intact (no partial/corrupting write).

Section rendering + truncation
------------------------------
:meth:`ProjectMemory.render_section` builds the ``[project memory]`` block: a
static instruction block, the four files each under a ``## <name>`` heading (empty
files omitted) in the order resources/structure/conventions/lessons, then a
``## recent episodes`` block (last 10, one line each, rejected/partial decisions
marked). The ``structure`` heading carries
:data:`STALE_SUFFIX` when the stored ``list_hash`` differs from the current LIST
reply. The whole section is capped at :data:`SECTION_CAP_CHARS` (40 000): on
overflow the episodes block is dropped FIRST, then ``lessons.md`` is
tail-truncated (head kept), and a ``memory section truncated`` marker is appended.
"""

from __future__ import annotations

import hashlib
import json
import logging
import os
import re
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path

_log = logging.getLogger(__name__)

#: The four codex/panel-editable markdown files, in render order.
MEMORY_FILES: tuple[str, ...] = ("resources", "structure", "conventions", "lessons")

#: Per-file write cap, in UTF-8 bytes (over-budget writes are rejected).
CONTENT_CAP_BYTES = 8192

#: Rendered ``[project memory]`` section cap, in characters.
SECTION_CAP_CHARS = 40000

#: Suffix appended to the ``## structure`` heading when the LIST hash drifted.
STALE_SUFFIX = "(may be outdated — project files changed since last memory update)"

#: Rendered body when the store is disabled or unreadable.
NO_MEMORY = "(no project memory)"

#: Marker appended after section-cap truncation.
_TRUNCATED_MARKER = "memory section truncated"

#: Episodes injected into a rendered section (the WS payload uses its own limit).
_RENDER_EPISODE_LIMIT = 10

#: Instruction-head length for an episode line.
_EPISODE_HEAD_CHARS = 80

#: Characters invalid in a Windows file name (control chars handled separately).
_INVALID_CHARS_RE = re.compile(r'[<>:"/\\|?*\x00-\x1f]')

#: Decisions that are corrections — marked explicitly so codex heeds them.
_CORRECTION_DECISIONS = {"rejected", "partial"}

#: Static instruction block (tells codex what is worth recording, and how).
_INSTRUCTION_BLOCK = (
    "Record only durable, project-specific facts via the memory_write tool: "
    "resource allocations (switches, death counters, locations, EUD addresses), "
    "file roles, naming/trigger conventions, and user corrections. Never record "
    "transient or code-derivable detail. Each file is a full replacement; rewrite "
    "it faithfully from what you see below."
)


def sanitize_project_name(name: str) -> str:
    """Sanitize a bridge project name into a Windows-safe directory name.

    Replaces characters invalid in Windows file names (``<>:"/\\|?*`` and control
    chars) with ``_`` and strips trailing dots/spaces. A name that is empty,
    whitespace-only, or collapses to empty after stripping returns ``""`` (which
    DISABLES the store for the session).
    """
    if not name:
        return ""
    cleaned = _INVALID_CHARS_RE.sub("_", name)
    # Strip trailing dots/spaces (invalid as a Windows dir name); keep inner ones.
    cleaned = cleaned.rstrip(". ")
    return cleaned


def list_hash(list_reply: str) -> str:
    """Return the sha256 hex digest of a bridge LIST reply (staleness signal)."""
    return hashlib.sha256(list_reply.encode("utf-8")).hexdigest()


@dataclass
class WriteResult:
    """Outcome of a :meth:`ProjectMemory.write`.

    ``ok`` is true on a successful write; otherwise ``reason`` carries a short,
    user/codex-facing explanation (disabled store, unknown file, over-budget).
    """

    ok: bool
    reason: str = ""


class ProjectMemory:
    """Per-project memory store rooted at ``<data-dir>/harness/<project>/``.

    Constructed with the editor ``data_dir`` and the (raw, unsanitized) project
    ``project_name``. When the sanitized name is empty the store is DISABLED:
    every operation degrades gracefully (reads ``""``, writes/appends non-ok,
    section ``(no project memory)``) and never touches disk.
    """

    def __init__(self, *, data_dir: str | os.PathLike, project_name: str):
        self.data_dir = Path(data_dir)
        self.project_name = project_name
        self._sanitized = sanitize_project_name(project_name)

    # ------------------------------------------------------------- state
    @property
    def enabled(self) -> bool:
        """True when a non-empty project name yields a usable store."""
        return bool(self._sanitized)

    @property
    def store_dir(self) -> Path | None:
        """The store directory, or ``None`` when the store is disabled."""
        if not self.enabled:
            return None
        return self.data_dir / "harness" / self._sanitized

    # ------------------------------------------------------------- file IO
    def _file_path(self, name: str) -> Path | None:
        store = self.store_dir
        if store is None:
            return None
        return store / f"{name}.md"

    def read(self, name: str) -> str:
        """Return the content of a markdown file, or ``""`` when absent/disabled.

        A read NEVER creates the store dir and NEVER raises for an absent file.
        """
        path = self._file_path(name)
        if path is None or not path.is_file():
            return ""
        return path.read_text(encoding="utf-8")

    def write(self, name: str, content: str) -> WriteResult:
        """Atomically write a markdown file (full replacement); return the outcome.

        Rejected (no disk write, prior content intact) when the store is disabled,
        ``name`` is not one of :data:`MEMORY_FILES`, or ``content`` exceeds
        :data:`CONTENT_CAP_BYTES` UTF-8 bytes. UTF-8 without BOM, atomic temp +
        :func:`os.replace` (same discipline as :mod:`eud_agent.journal`).
        """
        store = self.store_dir
        if store is None:
            return WriteResult(False, "no project is open; memory is disabled")
        if name not in MEMORY_FILES:
            return WriteResult(
                False, f"unknown memory file {name!r}; expected one of "
                f"{', '.join(MEMORY_FILES)}"
            )
        encoded = content.encode("utf-8")
        if len(encoded) > CONTENT_CAP_BYTES:
            return WriteResult(
                False,
                f"content is {len(encoded)} bytes, over the "
                f"{CONTENT_CAP_BYTES}-byte budget; condense it.",
            )
        store.mkdir(parents=True, exist_ok=True)
        path = store / f"{name}.md"
        tmp = path.with_suffix(".md.tmp")
        # Bytes so there is no BOM and no newline translation (rules.md).
        tmp.write_bytes(encoded)
        os.replace(tmp, path)
        return WriteResult(True)

    # ------------------------------------------------------------- episodes
    def _episodes_path(self) -> Path | None:
        store = self.store_dir
        if store is None:
            return None
        return store / "episodes.jsonl"

    def append_episode(self, episode: dict) -> bool:
        """Append one JSON object as a line to ``episodes.jsonl``.

        Best-effort: a disabled store or any IO failure is logged and SWALLOWED
        (returns ``False``) so memory can never break the request flow. Returns
        ``True`` on a successful append.
        """
        path = self._episodes_path()
        if path is None:
            return False
        try:
            self.store_dir.mkdir(parents=True, exist_ok=True)
            line = json.dumps(episode, ensure_ascii=False)
            # UTF-8 without BOM; explicit "\n" so no platform newline translation.
            with open(path, "a", encoding="utf-8", newline="\n") as fh:
                fh.write(line + "\n")
            return True
        except Exception:  # noqa: BLE001 - best-effort by contract
            _log.warning("episode append failed for %s", self._sanitized,
                         exc_info=True)
            return False

    def read_episodes(self, limit: int) -> list[dict]:
        """Return the last ``limit`` episodes (newest LAST); ``[]`` when absent.

        Malformed lines are skipped (best-effort). A disabled/absent store yields
        an empty list.
        """
        path = self._episodes_path()
        if path is None or not path.is_file():
            return []
        episodes: list[dict] = []
        for raw in path.read_text(encoding="utf-8").splitlines():
            raw = raw.strip()
            if not raw:
                continue
            try:
                episodes.append(json.loads(raw))
            except json.JSONDecodeError:
                continue
        return episodes[-limit:] if limit >= 0 else episodes

    # ------------------------------------------------------------- meta
    def _meta_path(self) -> Path | None:
        store = self.store_dir
        if store is None:
            return None
        return store / "meta.json"

    def read_meta(self) -> dict:
        """Return ``meta.json`` as a dict, or ``{}`` when absent/disabled."""
        path = self._meta_path()
        if path is None or not path.is_file():
            return {}
        try:
            return json.loads(path.read_text(encoding="utf-8"))
        except json.JSONDecodeError:
            return {}

    def write_meta(self, meta: dict) -> None:
        """Atomically write ``meta.json`` (UTF-8 no BOM); no-op when disabled."""
        store = self.store_dir
        if store is None:
            return
        store.mkdir(parents=True, exist_ok=True)
        path = store / "meta.json"
        tmp = path.with_suffix(".json.tmp")
        tmp.write_bytes(
            json.dumps(meta, ensure_ascii=False, indent=2).encode("utf-8")
        )
        os.replace(tmp, path)

    def update_list_hash(self, list_reply: str) -> None:
        """Record the current LIST reply's hash + ISO timestamp in ``meta.json``.

        Called whenever ``structure`` is (re)written so :meth:`is_stale` can detect
        later project-file drift.
        """
        meta = self.read_meta()
        meta["version"] = 1
        meta["list_hash"] = list_hash(list_reply)
        meta["list_hash_ts"] = datetime.now().isoformat(timespec="seconds")
        self.write_meta(meta)

    def is_stale(self, list_reply: str) -> bool:
        """True when the stored ``list_hash`` differs from the current LIST reply.

        A store with NO recorded hash is treated as stale (the structure summary
        may predate any file-list snapshot).
        """
        stored = self.read_meta().get("list_hash")
        if not stored:
            return True
        return stored != list_hash(list_reply)

    # ------------------------------------------------------------- render
    def render_section(self, list_reply: str | None = None) -> str:
        """Build the ``[project memory]`` prompt section.

        ``list_reply`` (the current bridge LIST text) enables the structure
        staleness suffix; omit it when no LIST is available (no suffix is added).
        A disabled store, or any read failure, degrades to
        ``[project memory]\\n(no project memory)``.
        """
        if not self.enabled:
            return f"[project memory]\n{NO_MEMORY}"
        try:
            return self._render_enabled(list_reply)
        except Exception:  # noqa: BLE001 - best-effort: never block the turn
            _log.warning("memory render failed for %s", self._sanitized,
                         exc_info=True)
            return f"[project memory]\n{NO_MEMORY}"

    def _render_enabled(self, list_reply: str | None) -> str:
        # File blocks (empty files omitted). Read via self.read so a degrading
        # read failure is observable and routes to the (no project memory) path.
        files: dict[str, str] = {name: self.read(name) for name in MEMORY_FILES}
        stale = list_reply is not None and self.is_stale(list_reply)

        file_blocks: list[str] = []
        for name in MEMORY_FILES:
            body = files[name].strip()
            if not body:
                continue
            heading = f"## {name}"
            if name == "structure" and stale:
                heading = f"{heading} {STALE_SUFFIX}"
            file_blocks.append(f"{heading}\n{body}")

        episode_block = self._render_episodes()

        # Assemble at full size first, then apply the documented truncation order.
        body_parts = [_INSTRUCTION_BLOCK, *file_blocks]
        if episode_block:
            body_parts.append(episode_block)
        section = "[project memory]\n" + "\n\n".join(body_parts)
        if len(section) <= SECTION_CAP_CHARS:
            return section

        # Over cap: drop episodes FIRST.
        body_parts = [_INSTRUCTION_BLOCK, *file_blocks]
        section = "[project memory]\n" + "\n\n".join(
            body_parts + [_TRUNCATED_MARKER]
        )
        if len(section) <= SECTION_CAP_CHARS:
            return section

        # Still over cap: tail-truncate lessons.md (keep its head), re-render.
        return self._render_with_truncated_lessons(files, stale)

    def _render_with_truncated_lessons(
        self, files: dict[str, str], stale: bool
    ) -> str:
        """Render with ``lessons`` tail-truncated to fit the section cap.

        Episodes are already dropped. The non-lessons blocks + the instruction
        block + the truncation marker form a fixed overhead; lessons' head is kept
        up to the remaining budget.
        """
        fixed_blocks: list[str] = [_INSTRUCTION_BLOCK]
        for name in MEMORY_FILES:
            if name == "lessons":
                continue
            body = files[name].strip()
            if not body:
                continue
            heading = f"## {name}"
            if name == "structure" and stale:
                heading = f"{heading} {STALE_SUFFIX}"
            fixed_blocks.append(f"{heading}\n{body}")

        lessons_body = files["lessons"].strip()
        # Build the frame WITHOUT lessons to measure the fixed overhead, then size
        # the lessons head to whatever budget remains.
        frame = "[project memory]\n" + "\n\n".join(
            fixed_blocks + ["## lessons\n", _TRUNCATED_MARKER]
        )
        budget = SECTION_CAP_CHARS - len(frame)
        head = lessons_body[: max(budget, 0)]
        blocks = list(fixed_blocks)
        if head:
            blocks.append(f"## lessons\n{head}")
        section = "[project memory]\n" + "\n\n".join(blocks + [_TRUNCATED_MARKER])
        # Defensive final clamp (the head sizing is an estimate around joins).
        if len(section) > SECTION_CAP_CHARS:
            section = section[:SECTION_CAP_CHARS]
        return section

    def _render_episodes(self) -> str:
        """Render the ``## recent episodes`` block, or ``""`` when none.

        One line per episode (newest last): ``<ts> <kind> <instruction-head> ->
        <decision>``; rejected/partial decisions are tagged so codex treats them
        as corrections.
        """
        episodes = self.read_episodes(_RENDER_EPISODE_LIMIT)
        if not episodes:
            return ""
        lines = ["## recent episodes"]
        for ep in episodes:
            lines.append(_episode_line(ep))
        return "\n".join(lines)


def _episode_line(ep: dict) -> str:
    """Format one episode as ``<ts> <kind> <instruction-head> -> <decision>``."""
    ts = str(ep.get("ts", ""))
    kind = str(ep.get("kind", ""))
    instruction = str(ep.get("instruction", "")).replace("\n", " ")
    head = instruction[:_EPISODE_HEAD_CHARS]
    decision = str(ep.get("decision", ""))
    if decision in _CORRECTION_DECISIONS:
        decision = f"{decision} (correction)"
    return f"{ts} {kind} {head} -> {decision}"
