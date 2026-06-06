"""Verification artifact for EUD-078: the per-map-project memory store.

These tests drive ``eud_agent.memory`` (the :class:`ProjectMemory` store and its
module-level helpers): path resolution + Windows-name sanitization, four-file
atomic UTF-8-without-BOM IO with an 8 KB write cap, the append-only
``episodes.jsonl``, the ``meta.json`` LIST-hash staleness signal, and the
``[project memory]`` section renderer (heading order, empty-file omission,
staleness suffix, recent-episode lines, the 40 000-char truncation contract, and
the disabled/unreadable ``(no project memory)`` degradation).

``eud_agent.memory`` does NOT exist during Step A, so this suite is expected to
FAIL on import (ImportError / collection error) until memory.py is implemented
(Step B). The public API the implementation must satisfy is defined entirely by
this file.

Contract (features/07 "Project Memory" + architecture/rules encoding discipline):

  * store dir ``<data-dir>/harness/<sanitized-project-name>/`` (sibling of
    ``journal/``); empty project name disables the store (explicit state);
  * the four markdown files (``resources``/``structure``/``conventions``/
    ``lessons``) round-trip UTF-8 WITHOUT BOM, atomic temp+replace, absent reads
    as ``""``, writes over the 8 192-byte UTF-8 cap are rejected (explicit
    result, not an exception that breaks the flow);
  * ``episodes.jsonl`` appends one JSON object per line; append failures are
    swallowed; read-last-N returns the newest N;
  * ``meta.json`` carries ``{version, list_hash, list_hash_ts}``; the helper
    sha256s a LIST reply and the store compares it for staleness;
  * the ``[project memory]`` section renders in the order resources/structure/
    conventions/lessons (empty omitted), a ``## recent episodes`` block of the
    last 10, the staleness suffix on the structure heading when the LIST hash
    drifts, the 40 000-char cap (episodes dropped FIRST, then lessons tail-
    truncated, ``memory section truncated`` marker appended), and renders
    ``[project memory]\n(no project memory)`` when disabled/unreadable.

Public API expected (constructed/used below):

  ProjectMemory(data_dir, project_name)
    .enabled                              -> bool (False when name sanitizes empty)
    .store_dir                            -> Path | None
    .read(name)                           -> str ("" when absent/disabled)
    .write(name, content)                 -> WriteResult{ok, reason}
    .append_episode(episode: dict)        -> bool (swallows + logs failures)
    .read_episodes(limit)                 -> list[dict] (newest last)
    .read_meta()                          -> dict
    .write_meta(meta: dict)               -> None
    .update_list_hash(list_reply)         -> None  (refresh stored hash + ts)
    .is_stale(list_reply)                 -> bool  (stored hash != current hash)
    .render_section(list_reply=None)      -> str   ("[project memory]\n...")

  module-level:
    sanitize_project_name(name)           -> str
    list_hash(list_reply)                 -> str (sha256 hex)
    MEMORY_FILES                          -> ("resources","structure",
                                             "conventions","lessons")
    CONTENT_CAP_BYTES = 8192
    SECTION_CAP_CHARS = 40000
    STALE_SUFFIX = "(may be outdated ...)"
    NO_MEMORY = "(no project memory)"
"""

from __future__ import annotations

import json

import pytest

# Imported at collection so the failing import is the first signal in Step A.
from eud_agent.memory import (
    CONTENT_CAP_BYTES,
    MEMORY_FILES,
    NO_MEMORY,
    SECTION_CAP_CHARS,
    STALE_SUFFIX,
    ProjectMemory,
    list_hash,
    sanitize_project_name,
)

# --------------------------------------------------------------------------- #
# Fixtures / helpers.
# --------------------------------------------------------------------------- #


def make_store(tmp_path, project_name="My Map"):
    """A ProjectMemory rooted at a tmp data dir for a named project."""
    return ProjectMemory(data_dir=str(tmp_path), project_name=project_name)


# --------------------------------------------------------------------------- #
# Module-level constants are the contract the renderer/cap depend on.
# --------------------------------------------------------------------------- #


def test_module_constants():
    assert MEMORY_FILES == ("resources", "structure", "conventions", "lessons")
    assert CONTENT_CAP_BYTES == 8192
    assert SECTION_CAP_CHARS == 40000
    # the EXACT staleness suffix the spec mandates on the structure heading.
    assert STALE_SUFFIX == (
        "(may be outdated — project files changed since last memory update)"
    )
    assert NO_MEMORY == "(no project memory)"


# --------------------------------------------------------------------------- #
# Sanitization: Windows-invalid chars + trailing dots/spaces -> '_'; empty name
# disables the store.
# --------------------------------------------------------------------------- #


def test_sanitize_replaces_windows_invalid_chars():
    # every char in <>:"/\|?* becomes '_'.
    assert sanitize_project_name('a<b>c:d"e/f\\g|h?i*j') == "a_b_c_d_e_f_g_h_i_j"


def test_sanitize_replaces_control_chars():
    assert sanitize_project_name("a\tb\nc\x00d") == "a_b_c_d"


def test_sanitize_strips_trailing_dots_and_spaces():
    assert sanitize_project_name("My Map. ") == "My Map"
    assert sanitize_project_name("name...") == "name"
    assert sanitize_project_name("name   ") == "name"


def test_sanitize_keeps_inner_dots_and_spaces():
    assert sanitize_project_name("My v1.2 Map") == "My v1.2 Map"


def test_sanitize_empty_or_whitespace_returns_empty():
    assert sanitize_project_name("") == ""
    assert sanitize_project_name("   ") == ""
    # a name that is only invalid chars + trailing strip can collapse to empty.
    assert sanitize_project_name("...") == ""


def test_empty_project_name_disables_store(tmp_path):
    store = make_store(tmp_path, project_name="")
    assert store.enabled is False
    assert store.store_dir is None
    # reads are defined and return "": no crash, no disk access.
    assert store.read("resources") == ""
    # writes are explicitly rejected (not raised) when disabled.
    res = store.write("resources", "x")
    assert res.ok is False
    assert res.reason


def test_whitespace_project_name_disables_store(tmp_path):
    assert make_store(tmp_path, project_name="   ").enabled is False


# --------------------------------------------------------------------------- #
# Path resolution: <data-dir>/harness/<sanitized>/ ; sibling of journal/.
# --------------------------------------------------------------------------- #


def test_store_dir_path_layout(tmp_path):
    store = make_store(tmp_path, project_name='Bad:Name?')
    assert store.enabled is True
    expected = tmp_path / "harness" / "Bad_Name_"
    assert store.store_dir == expected


def test_store_dir_created_on_demand_not_before_write(tmp_path):
    store = make_store(tmp_path, project_name="Fresh")
    # constructing the store must not eagerly create the dir.
    assert not (tmp_path / "harness" / "Fresh").exists()
    # a read of an absent file does not create it either.
    assert store.read("resources") == ""
    assert not (tmp_path / "harness" / "Fresh").exists()
    # the first write materializes the dir.
    assert store.write("resources", "hello").ok is True
    assert (tmp_path / "harness" / "Fresh").is_dir()


# --------------------------------------------------------------------------- #
# Four-file IO: atomic round-trip, BOM-free bytes, absent reads "".
# --------------------------------------------------------------------------- #


@pytest.mark.parametrize("name", ["resources", "structure", "conventions", "lessons"])
def test_each_file_round_trips(tmp_path, name):
    store = make_store(tmp_path)
    assert store.read(name) == ""  # absent reads ""
    body = f"# {name}\nsome durable fact.\n"
    assert store.write(name, body).ok is True
    assert store.read(name) == body


def test_write_is_utf8_without_bom(tmp_path):
    store = make_store(tmp_path)
    # multi-byte content (Korean) so the encoding is actually exercised.
    store.write("lessons", "한글 교훈\n")
    path = store.store_dir / "lessons.md"
    raw = path.read_bytes()
    assert not raw.startswith(b"\xef\xbb\xbf")  # no BOM
    assert raw.decode("utf-8") == "한글 교훈\n"


def test_write_overwrites_full_file(tmp_path):
    store = make_store(tmp_path)
    store.write("resources", "first\n")
    store.write("resources", "second\n")  # full replacement semantics
    assert store.read("resources") == "second\n"


def test_write_leaves_no_temp_file(tmp_path):
    store = make_store(tmp_path)
    store.write("resources", "x")
    leftovers = [p.name for p in store.store_dir.iterdir()
                 if p.name.endswith(".tmp")]
    assert leftovers == []


def test_unknown_file_name_rejected(tmp_path):
    store = make_store(tmp_path)
    res = store.write("notafile", "x")
    assert res.ok is False
    assert res.reason


# --------------------------------------------------------------------------- #
# 8 KB write cap (UTF-8 BYTES, not chars): over-cap is rejected explicitly.
# --------------------------------------------------------------------------- #


def test_write_at_cap_accepted(tmp_path):
    store = make_store(tmp_path)
    content = "a" * CONTENT_CAP_BYTES  # exactly 8192 ASCII bytes
    res = store.write("resources", content)
    assert res.ok is True
    assert store.read("resources") == content


def test_write_over_cap_rejected_and_not_written(tmp_path):
    store = make_store(tmp_path)
    store.write("resources", "kept\n")  # pre-existing content
    content = "a" * (CONTENT_CAP_BYTES + 1)
    res = store.write("resources", content)
    assert res.ok is False
    assert res.reason  # explains the over-budget rejection (codex should condense)
    # the prior content must survive a rejected write.
    assert store.read("resources") == "kept\n"


def test_cap_counts_utf8_bytes_not_chars(tmp_path):
    store = make_store(tmp_path)
    # each Korean char is 3 UTF-8 bytes; 2731 chars = 8193 bytes > cap, but
    # 2731 < 8192 chars (so a char-based cap would wrongly accept it).
    char_count = (CONTENT_CAP_BYTES // 3) + 1  # 2731
    content = "가" * char_count
    assert len(content) < CONTENT_CAP_BYTES
    assert len(content.encode("utf-8")) > CONTENT_CAP_BYTES
    res = store.write("conventions", content)
    assert res.ok is False


# --------------------------------------------------------------------------- #
# Episodes: append one JSONL line, read last N, append failures swallowed.
# --------------------------------------------------------------------------- #


def _episode(i, decision="answer", kind="answer"):
    return {
        "ts": f"2026-06-06T10:0{i}:00",
        "request_id": f"req-{i}",
        "instruction": f"instruction number {i}",
        "kind": kind,
        "tools": ["dat_set"],
        "files": ["stats.eps"],
        "decision": decision,
    }


def test_append_episode_writes_one_jsonl_line(tmp_path):
    store = make_store(tmp_path)
    assert store.append_episode(_episode(1)) is True
    path = store.store_dir / "episodes.jsonl"
    lines = path.read_text(encoding="utf-8").splitlines()
    assert len(lines) == 1
    assert json.loads(lines[0])["request_id"] == "req-1"


def test_episodes_file_is_utf8_no_bom(tmp_path):
    store = make_store(tmp_path)
    store.append_episode({**_episode(1), "instruction": "한글 지시"})
    raw = (store.store_dir / "episodes.jsonl").read_bytes()
    assert not raw.startswith(b"\xef\xbb\xbf")
    assert "한글 지시" in raw.decode("utf-8")


def test_read_episodes_returns_last_n_newest_last(tmp_path):
    store = make_store(tmp_path)
    for i in range(5):
        store.append_episode(_episode(i))
    last3 = store.read_episodes(3)
    assert [e["request_id"] for e in last3] == ["req-2", "req-3", "req-4"]


def test_read_episodes_when_absent_returns_empty(tmp_path):
    store = make_store(tmp_path)
    assert store.read_episodes(10) == []


def test_append_episode_swallows_failure(tmp_path, monkeypatch):
    """An append that fails (e.g. unwritable) must NOT raise; it returns False so
    memory never breaks the request flow."""
    store = make_store(tmp_path)
    store.append_episode(_episode(0))  # materialize the dir

    import builtins

    real_open = builtins.open

    def boom(path, *a, **k):
        if "episodes.jsonl" in str(path):
            raise OSError("disk full")
        return real_open(path, *a, **k)

    monkeypatch.setattr(builtins, "open", boom)
    # must not raise.
    assert store.append_episode(_episode(1)) is False


def test_append_episode_disabled_store_noop(tmp_path):
    store = make_store(tmp_path, project_name="")
    assert store.append_episode(_episode(1)) is False


# --------------------------------------------------------------------------- #
# meta.json + LIST-hash staleness.
# --------------------------------------------------------------------------- #


def test_list_hash_is_sha256_hex_and_stable():
    h1 = list_hash("a\tCUIEps\nb\tRawText\n")
    h2 = list_hash("a\tCUIEps\nb\tRawText\n")
    assert h1 == h2
    assert len(h1) == 64 and all(c in "0123456789abcdef" for c in h1)
    assert list_hash("different") != h1


def test_write_meta_round_trip_version_and_hash(tmp_path):
    store = make_store(tmp_path)
    store.write_meta({"version": 1, "list_hash": "abc", "list_hash_ts": "T"})
    meta = store.read_meta()
    assert meta["version"] == 1
    assert meta["list_hash"] == "abc"
    assert meta["list_hash_ts"] == "T"


def test_meta_json_utf8_no_bom(tmp_path):
    store = make_store(tmp_path)
    store.write_meta({"version": 1, "list_hash": "abc", "list_hash_ts": "T"})
    raw = (store.store_dir / "meta.json").read_bytes()
    assert not raw.startswith(b"\xef\xbb\xbf")
    assert json.loads(raw.decode("utf-8"))["version"] == 1


def test_read_meta_absent_returns_empty(tmp_path):
    store = make_store(tmp_path)
    assert store.read_meta() == {}


def test_update_list_hash_then_not_stale(tmp_path):
    store = make_store(tmp_path)
    reply = "stats.eps\tCUIEps\nmain.eps\tCUIEps\n"
    store.update_list_hash(reply)
    assert store.is_stale(reply) is False
    meta = store.read_meta()
    assert meta["list_hash"] == list_hash(reply)
    assert meta.get("list_hash_ts")  # an ISO timestamp was recorded


def test_is_stale_when_list_changed(tmp_path):
    store = make_store(tmp_path)
    store.update_list_hash("old list\n")
    assert store.is_stale("a NEW list with more files\n") is True


def test_is_stale_when_no_meta_recorded(tmp_path):
    # never updated -> no stored hash -> treat as stale (structure may be old).
    store = make_store(tmp_path)
    assert store.is_stale("any list\n") is True


# --------------------------------------------------------------------------- #
# Section renderer: heading + order + empty omission.
# --------------------------------------------------------------------------- #


def test_render_section_header_and_instruction_block(tmp_path):
    store = make_store(tmp_path)
    section = store.render_section()
    assert section.startswith("[project memory]")
    # the static instruction block tells codex to record durable facts via the
    # memory tool (we assert the load-bearing keywords, not exact prose).
    low = section.lower()
    assert "memory_write" in low
    assert "durable" in low


def test_render_section_order_and_headings(tmp_path):
    store = make_store(tmp_path)
    store.write("resources", "switch 1 = quest flag\n")
    store.write("structure", "stats.eps: RPG stat system\n")
    store.write("conventions", "snake_case triggers\n")
    store.write("lessons", "never reuse death counter 7\n")
    section = store.render_section()
    # each file under its own '## <name>' heading, in spec order.
    i_res = section.index("## resources")
    i_str = section.index("## structure")
    i_con = section.index("## conventions")
    i_les = section.index("## lessons")
    assert i_res < i_str < i_con < i_les
    assert "switch 1 = quest flag" in section
    assert "stats.eps: RPG stat system" in section


def test_render_section_omits_empty_files(tmp_path):
    store = make_store(tmp_path)
    store.write("resources", "only this one\n")
    # structure/conventions/lessons stay empty.
    section = store.render_section()
    assert "## resources" in section
    assert "## structure" not in section
    assert "## conventions" not in section
    assert "## lessons" not in section


# --------------------------------------------------------------------------- #
# Staleness suffix on the structure heading.
# --------------------------------------------------------------------------- #


def test_structure_heading_carries_stale_suffix_when_drifted(tmp_path):
    store = make_store(tmp_path)
    store.write("structure", "stats.eps: stats\n")
    store.update_list_hash("old list\n")
    # current LIST differs from the stored hash -> suffix on the structure heading.
    section = store.render_section(list_reply="a DIFFERENT list now\n")
    assert f"## structure {STALE_SUFFIX}" in section


def test_structure_heading_no_suffix_when_fresh(tmp_path):
    store = make_store(tmp_path)
    store.write("structure", "stats.eps: stats\n")
    reply = "stats.eps\tCUIEps\n"
    store.update_list_hash(reply)
    section = store.render_section(list_reply=reply)
    assert "## structure" in section
    assert STALE_SUFFIX not in section


def test_no_stale_suffix_without_list_reply(tmp_path):
    # rendering without a current LIST reply cannot judge staleness -> no suffix.
    store = make_store(tmp_path)
    store.write("structure", "stats.eps: stats\n")
    store.update_list_hash("old\n")
    section = store.render_section()  # list_reply omitted
    assert STALE_SUFFIX not in section


# --------------------------------------------------------------------------- #
# Recent episodes block: last 10, one line each, rejected/partial marked.
# --------------------------------------------------------------------------- #


def test_render_recent_episodes_last_10_one_line_each(tmp_path):
    store = make_store(tmp_path)
    for i in range(15):
        store.append_episode(_episode(i % 10))  # 15 episodes total
    section = store.render_section()
    assert "## recent episodes" in section
    block = section.split("## recent episodes", 1)[1]
    ep_lines = [ln for ln in block.splitlines() if ln.strip()]
    # exactly the last 10 episodes rendered.
    assert len(ep_lines) == 10


def test_render_episode_line_shape(tmp_path):
    store = make_store(tmp_path)
    store.append_episode({
        "ts": "2026-06-06T10:00:00",
        "request_id": "req-x",
        "instruction": "make the marine stronger",
        "kind": "changeset",
        "tools": ["dat_set"],
        "files": ["units"],
        "decision": "accepted",
    })
    section = store.render_section()
    block = section.split("## recent episodes", 1)[1]
    line = next(ln for ln in block.splitlines() if "marine" in ln)
    # '<ts> <kind> <instruction-head> -> <decision>'
    assert "2026-06-06T10:00:00" in line
    assert "changeset" in line
    assert "make the marine stronger" in line
    assert "->" in line
    assert "accepted" in line


def test_render_rejected_and_partial_decisions_marked(tmp_path):
    store = make_store(tmp_path)
    store.append_episode({**_episode(1), "instruction": "bad edit",
                          "decision": "rejected"})
    store.append_episode({**_episode(2), "instruction": "half edit",
                          "decision": "partial"})
    section = store.render_section()
    block = section.split("## recent episodes", 1)[1]
    rej = next(ln for ln in block.splitlines() if "bad edit" in ln)
    par = next(ln for ln in block.splitlines() if "half edit" in ln)
    # rejected/partial are explicitly visible so codex treats them as corrections.
    assert "rejected" in rej.lower()
    assert "partial" in par.lower()


def test_render_no_episodes_omits_block(tmp_path):
    store = make_store(tmp_path)
    store.write("resources", "x\n")
    section = store.render_section()
    assert "## recent episodes" not in section


# --------------------------------------------------------------------------- #
# 40 000-char cap + documented truncation order: episodes FIRST, then lessons
# tail-truncated, then a 'memory section truncated' marker.
# --------------------------------------------------------------------------- #


def test_render_under_cap_keeps_everything(tmp_path):
    store = make_store(tmp_path)
    store.write("resources", "small\n")
    store.append_episode(_episode(1))
    section = store.render_section()
    assert len(section) <= SECTION_CAP_CHARS
    assert "memory section truncated" not in section
    assert "## recent episodes" in section


def _write_raw(store, name, body):
    """Write a file body straight to disk, bypassing the 8 KB write cap.

    The per-file write() cap (8192 B) means four files can sum to at most ~32 KB
    — under the 40 000-char section cap — so the section-cap truncation paths are
    only reachable from over-cap content already on disk (e.g. a file edited
    outside the tool, or pre-cap legacy content). We materialize that state
    directly to exercise the renderer's truncation contract.
    """
    store.store_dir.mkdir(parents=True, exist_ok=True)
    (store.store_dir / f"{name}.md").write_text(body, encoding="utf-8")


def test_truncation_drops_episodes_first(tmp_path):
    """When the section would exceed the cap, the episodes block is dropped before
    any file content is touched."""
    store = make_store(tmp_path)
    # over-cap file bodies (on disk) so files alone exceed the section cap.
    big = "x" * 11000
    for name in MEMORY_FILES:
        _write_raw(store, name, big)
    for i in range(20):
        store.append_episode(_episode(i % 10))
    section = store.render_section()
    assert len(section) <= SECTION_CAP_CHARS
    # episodes dropped first; the four files' headings survive.
    assert "## recent episodes" not in section
    assert "## resources" in section
    assert "memory section truncated" in section


def test_truncation_tail_truncates_lessons_after_episodes(tmp_path):
    """If dropping episodes is not enough, lessons.md is tail-truncated (its TAIL
    removed, head kept) and the truncation marker appended."""
    store = make_store(tmp_path)
    big = "y" * 11000
    for name in MEMORY_FILES:
        _write_raw(store, name, big)
    # make lessons distinguishable head/tail so we can assert the TAIL went.
    lessons = "LESSON_HEAD\n" + ("z" * 11000) + "\nLESSON_TAIL"
    _write_raw(store, "lessons", lessons)
    for i in range(20):
        store.append_episode(_episode(i % 10))
    section = store.render_section()
    assert len(section) <= SECTION_CAP_CHARS
    assert "## recent episodes" not in section
    assert "memory section truncated" in section
    # head of lessons survives; the tail marker is gone.
    assert "LESSON_HEAD" in section
    assert "LESSON_TAIL" not in section


# --------------------------------------------------------------------------- #
# Degradation: disabled / unreadable store renders '(no project memory)'.
# --------------------------------------------------------------------------- #


def test_disabled_store_renders_no_memory(tmp_path):
    store = make_store(tmp_path, project_name="")
    section = store.render_section()
    assert section == f"[project memory]\n{NO_MEMORY}"


def test_unreadable_store_renders_no_memory(tmp_path, monkeypatch):
    """A store dir that cannot be read degrades to '(no project memory)' rather
    than raising (same degradation contract as RAG)."""
    store = make_store(tmp_path, project_name="Broken")
    store.write("resources", "x\n")

    # force every file read to raise so rendering must degrade.
    def boom(self, name):
        raise OSError("unreadable")

    monkeypatch.setattr(ProjectMemory, "read", boom, raising=True)
    section = store.render_section()
    assert section == f"[project memory]\n{NO_MEMORY}"
