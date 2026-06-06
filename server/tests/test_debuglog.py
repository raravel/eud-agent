"""DebugLog — persistent JSONL debug trail (chat inputs / tool calls / turn ends).

Contract (``eud_agent.debuglog.DebugLog``):
  * ``DebugLog(data_dir, retention_days=7, now=...)`` ensures ``<data_dir>/logs/``
    exists and deletes ``agent-YYYYMMDD.jsonl`` files dated OLDER than the
    retention window (server-boot cleanup). Foreign filenames are left alone.
  * ``log(event, data)`` appends ONE JSON object per line —
    ``{"ts": <ISO8601>, "event": <event>, "data": <data>}`` — to the current
    day's ``agent-YYYYMMDD.jsonl``. UTF-8 WITHOUT BOM (rules.md encoding
    discipline), ``ensure_ascii=False`` so Korean instruction text stays
    human-readable on disk.
  * Logging is BEST-EFFORT and never raises: an unwritable logs dir or a
    non-JSON-serializable payload must never break the serving flow.

``now`` is injectable so the tests pin the date (no wall-clock coupling).
"""

from __future__ import annotations

import json
from datetime import datetime

from eud_agent.debuglog import DebugLog


def _fixed_now(iso: str):
    dt = datetime.fromisoformat(iso)
    return lambda: dt


# --------------------------------------------------------------------------- #
# Append format
# --------------------------------------------------------------------------- #


def test_log_appends_jsonl_line_with_ts_event_data(tmp_path):
    dlog = DebugLog(tmp_path, now=_fixed_now("2026-06-06T12:00:00"))
    dlog.log("client", {"type": "chat", "text": "hello"})

    path = tmp_path / "logs" / "agent-20260606.jsonl"
    assert path.is_file()
    lines = path.read_text(encoding="utf-8").splitlines()
    assert len(lines) == 1
    entry = json.loads(lines[0])
    assert entry["ts"].startswith("2026-06-06T12:00:00")
    assert entry["event"] == "client"
    assert entry["data"] == {"type": "chat", "text": "hello"}


def test_log_appends_multiple_lines_in_order(tmp_path):
    dlog = DebugLog(tmp_path, now=_fixed_now("2026-06-06T12:00:00"))
    dlog.log("client", {"type": "chat", "text": "one"})
    dlog.log("tool_call", {"tool": "dat_set", "args": {"objId": 0}})

    path = tmp_path / "logs" / "agent-20260606.jsonl"
    entries = [
        json.loads(ln)
        for ln in path.read_text(encoding="utf-8").splitlines()
    ]
    assert [e["event"] for e in entries] == ["client", "tool_call"]


def test_korean_text_is_human_readable_and_bom_free(tmp_path):
    dlog = DebugLog(tmp_path, now=_fixed_now("2026-06-06T12:00:00"))
    dlog.log("client", {"type": "chat", "text": "마린 체력 2배"})

    raw = (tmp_path / "logs" / "agent-20260606.jsonl").read_bytes()
    assert not raw.startswith(b"\xef\xbb\xbf"), "log files must be BOM-free"
    # ensure_ascii=False: the Korean text lands as UTF-8, not \uXXXX escapes.
    assert "마린 체력 2배".encode("utf-8") in raw


# --------------------------------------------------------------------------- #
# Retention cleanup at construction
# --------------------------------------------------------------------------- #


def test_retention_deletes_old_files_keeps_recent_and_foreign(tmp_path):
    logs = tmp_path / "logs"
    logs.mkdir()
    (logs / "agent-20260501.jsonl").write_text("{}\n", encoding="utf-8")  # 36d old
    (logs / "agent-20260604.jsonl").write_text("{}\n", encoding="utf-8")  # 2d old
    (logs / "notes.txt").write_text("keep me", encoding="utf-8")  # foreign

    DebugLog(tmp_path, retention_days=7, now=_fixed_now("2026-06-06T12:00:00"))

    assert not (logs / "agent-20260501.jsonl").exists(), "8+ day file deleted"
    assert (logs / "agent-20260604.jsonl").exists(), "recent file kept"
    assert (logs / "notes.txt").exists(), "foreign filenames untouched"


# --------------------------------------------------------------------------- #
# Best-effort: log() never raises
# --------------------------------------------------------------------------- #


def test_log_never_raises_when_logs_dir_is_a_file(tmp_path):
    # Occupy the logs path with a FILE so mkdir/open fail.
    (tmp_path / "logs").write_text("not a dir", encoding="utf-8")
    dlog = DebugLog(tmp_path, now=_fixed_now("2026-06-06T12:00:00"))
    dlog.log("client", {"type": "chat", "text": "hello"})  # must not raise


def test_log_survives_non_json_serializable_data(tmp_path):
    dlog = DebugLog(tmp_path, now=_fixed_now("2026-06-06T12:00:00"))
    dlog.log("tool_result", {"result": {1, 2, 3}})  # a set is not JSON

    # Either stringified (default=str) or skipped — but NEVER raised. If a line
    # landed, it must be valid JSON.
    path = tmp_path / "logs" / "agent-20260606.jsonl"
    if path.is_file():
        for ln in path.read_text(encoding="utf-8").splitlines():
            json.loads(ln)
