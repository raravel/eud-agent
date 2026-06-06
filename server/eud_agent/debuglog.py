"""Persistent JSONL debug trail (chat inputs / tool calls / turn ends).

Purpose-built for post-hoc debugging of live E2E sessions: every inbound WS
client message, every ``/tools/call`` (full args/result, untruncated — the
panel's ``agent_event`` payloads are truncated, this trail is not), and the
turn-end events (``answer``/``plan``/``changeset``/``error``) land as one JSON
object per line under ``<data_dir>/logs/agent-YYYYMMDD.jsonl``.

Design rules:

* **Best-effort, never raises.** A logging failure (unwritable dir, odd
  payload) must never break the serving flow — ``log`` swallows everything.
* **UTF-8 without BOM** (rules.md encoding discipline), ``ensure_ascii=False``
  so Korean instruction text stays human-readable on disk.
* **Day-rotated + retention.** One file per local day; ``agent-YYYYMMDD.jsonl``
  files older than ``retention_days`` are deleted at construction (server
  boot). Foreign filenames in the logs dir are left alone.
* **No streaming deltas.** The wiring only feeds discrete events (the
  reasoning/answer token stream is deliberately excluded) so files stay
  tractable.

``now`` is injectable for tests (defaults to :func:`datetime.now`).
"""

from __future__ import annotations

import json
import re
import threading
from datetime import datetime, timedelta
from pathlib import Path
from typing import Callable

#: Day-rotated log filename shape; group(1) is the YYYYMMDD stamp.
_FILE_RE = re.compile(r"^agent-(\d{8})\.jsonl$")


class DebugLog:
    """Append-only JSONL debug logger rooted at ``<data_dir>/logs/``."""

    def __init__(
        self,
        data_dir: str | Path,
        *,
        retention_days: int = 7,
        now: Callable[[], datetime] | None = None,
    ) -> None:
        self._now = now or datetime.now
        self._dir = Path(data_dir) / "logs"
        # Serialize appends (tool calls run on worker threads via to_thread).
        self._lock = threading.Lock()
        try:
            self._dir.mkdir(parents=True, exist_ok=True)
        except OSError:
            # Best-effort: a failed mkdir just makes log() a silent no-op.
            pass
        self._cleanup(retention_days)

    @property
    def dir(self) -> Path:
        """The logs directory (``<data_dir>/logs``)."""
        return self._dir

    def log(self, event: str, data: dict) -> None:
        """Append ``{"ts", "event", "data"}`` as one line. NEVER raises."""
        try:
            stamp = self._now()
            entry = {
                "ts": stamp.isoformat(timespec="seconds"),
                "event": event,
                "data": data,
            }
            # default=str: a non-JSON value (set, Path, ...) is stringified
            # rather than killing the line — this is a debug trail, not an API.
            line = json.dumps(entry, ensure_ascii=False, default=str)
            path = self._dir / f"agent-{stamp:%Y%m%d}.jsonl"
            with self._lock, open(
                path, "a", encoding="utf-8", newline="\n"
            ) as fh:
                fh.write(line + "\n")
        except Exception:  # noqa: BLE001 - best-effort by contract
            pass

    def _cleanup(self, retention_days: int) -> None:
        """Delete day files dated older than the retention window."""
        try:
            cutoff = (self._now() - timedelta(days=retention_days)).strftime(
                "%Y%m%d"
            )
            for p in self._dir.iterdir():
                m = _FILE_RE.match(p.name)
                if m and m.group(1) < cutoff:
                    p.unlink(missing_ok=True)
        except OSError:
            pass
