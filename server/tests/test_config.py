"""Verification artifact for EUD-009-6aed: config resolution + selfcheck.

Covers:
  1. Config precedence: CLI > env > agent.cfg > defaults
     (tmp_path agent.cfg fixtures + monkeypatch for env).
  2. Session token generation uniqueness (uuid4).
  3. selfcheck failure modes: each induced prerequisite failure
     (bad rag_db path, unresolvable codex, missing panel files,
     missing HF cache) produces its OWN distinct message and a
     non-zero exit.

These tests import ``eud_agent.config`` and ``eud_agent.__main__`` which do
not exist during Step A — the suite is expected to FAIL until config.py /
__main__.py are implemented (Step B).
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from eud_agent import config as cfgmod
from eud_agent.config import Config

# Defaults from the harness contract (features/02_python-server.md).
DEFAULT_PORT = 8765
DEFAULT_RAG_DB = r"C:\Users\ifthe\proj\eud\ECA\chromadb_bge"


def _write_agent_cfg(tmp_path: Path, data: dict) -> Path:
    """Write a Data\\agent\\agent.cfg under tmp_path; return its path."""
    agent_dir = tmp_path / "agent"
    agent_dir.mkdir(parents=True, exist_ok=True)
    cfg_path = agent_dir / "agent.cfg"
    cfg_path.write_text(json.dumps(data), encoding="utf-8")
    return cfg_path


def _write_raw_agent_cfg(tmp_path: Path, raw: bytes) -> Path:
    """Write raw (possibly malformed/BOM'd) agent.cfg bytes; return its path."""
    agent_dir = tmp_path / "agent"
    agent_dir.mkdir(parents=True, exist_ok=True)
    cfg_path = agent_dir / "agent.cfg"
    cfg_path.write_bytes(raw)
    return cfg_path


# --------------------------------------------------------------- precedence


def test_defaults_when_nothing_provided(monkeypatch):
    """With no CLI, no env, no agent.cfg, defaults apply."""
    for var in ("EUD_DATA_DIR", "EUD_PORT", "CODEX_CMD", "EUD_RAG_DB", "EUD_REPO_ROOT"):
        monkeypatch.delenv(var, raising=False)
    cfg = Config.resolve(cli={}, agent_cfg_path=None)
    assert cfg.port == DEFAULT_PORT
    assert Path(cfg.rag_db) == Path(DEFAULT_RAG_DB)


def test_agent_cfg_overrides_defaults(monkeypatch, tmp_path):
    for var in ("EUD_DATA_DIR", "EUD_PORT", "CODEX_CMD", "EUD_RAG_DB", "EUD_REPO_ROOT"):
        monkeypatch.delenv(var, raising=False)
    cfg_path = _write_agent_cfg(
        tmp_path,
        {"port": 9001, "repo_root": str(tmp_path / "repo")},
    )
    cfg = Config.resolve(cli={}, agent_cfg_path=cfg_path)
    assert cfg.port == 9001
    assert Path(cfg.repo_root) == (tmp_path / "repo")


def test_env_overrides_agent_cfg(monkeypatch, tmp_path):
    cfg_path = _write_agent_cfg(tmp_path, {"port": 9001})
    monkeypatch.setenv("EUD_PORT", "9100")
    cfg = Config.resolve(cli={}, agent_cfg_path=cfg_path)
    assert cfg.port == 9100


def test_cli_overrides_env_and_agent_cfg(monkeypatch, tmp_path):
    cfg_path = _write_agent_cfg(tmp_path, {"port": 9001})
    monkeypatch.setenv("EUD_PORT", "9100")
    cfg = Config.resolve(cli={"port": 9200}, agent_cfg_path=cfg_path)
    assert cfg.port == 9200


def test_cli_overrides_codex_cmd(monkeypatch, tmp_path):
    monkeypatch.setenv("CODEX_CMD", r"C:\env\codex.cmd")
    cfg = Config.resolve(
        cli={"codex_cmd": r"C:\cli\codex.cmd"}, agent_cfg_path=None
    )
    assert cfg.codex_cmd == r"C:\cli\codex.cmd"


def test_env_codex_cmd_overrides_agent_cfg(monkeypatch, tmp_path):
    cfg_path = _write_agent_cfg(tmp_path, {"codex_cmd": r"C:\cfg\codex.cmd"})
    monkeypatch.setenv("CODEX_CMD", r"C:\env\codex.cmd")
    cfg = Config.resolve(cli={}, agent_cfg_path=cfg_path)
    assert cfg.codex_cmd == r"C:\env\codex.cmd"


def test_data_dir_locates_agent_cfg_via_env(monkeypatch, tmp_path):
    """When EUD_DATA_DIR points at a dir holding agent.cfg, it is loaded."""
    for var in ("EUD_PORT", "CODEX_CMD", "EUD_RAG_DB", "EUD_REPO_ROOT"):
        monkeypatch.delenv(var, raising=False)
    cfg_path = _write_agent_cfg(tmp_path, {"port": 9333})
    data_dir = cfg_path.parent
    monkeypatch.setenv("EUD_DATA_DIR", str(data_dir))
    cfg = Config.resolve(cli={})  # no explicit path; located via EUD_DATA_DIR
    assert cfg.port == 9333
    assert Path(cfg.data_dir) == data_dir


# --------------------------------------------------------------- token


def test_token_generated_and_unique():
    a = Config.resolve(cli={}, agent_cfg_path=None)
    b = Config.resolve(cli={}, agent_cfg_path=None)
    assert a.token and isinstance(a.token, str)
    assert a.token != b.token


def test_token_uuid4_shape():
    cfg = Config.resolve(cli={}, agent_cfg_path=None)
    # uuid4 string form: 8-4-4-4-12 hex with dashes
    parts = cfg.token.split("-")
    assert len(parts) == 5
    assert [len(p) for p in parts] == [8, 4, 4, 4, 12]


# --------------------------------------------------------------- selfcheck

# Each check has a stable, distinct substring in its failure message so the
# panel/operator can tell which prerequisite is missing.
CODEX_MARK = "codex"
RAG_MARK = "RAG DB"
HF_MARK = "bge-m3"
PANEL_MARK = "panel"


def _good_config(tmp_path: Path) -> Config:
    """A Config whose paths all point at satisfiable prerequisites we fake."""
    repo = tmp_path / "repo"
    panel = repo / "panel"
    panel.mkdir(parents=True)
    for name in ("index.html", "app.js", "style.css"):
        (panel / name).write_text("// ok", encoding="utf-8")

    rag = tmp_path / "ragdb"
    rag.mkdir()
    (rag / "chroma.sqlite3").write_text("", encoding="utf-8")

    hf = tmp_path / "hfcache" / "models--BAAI--bge-m3"
    hf.mkdir(parents=True)

    codex = tmp_path / "codex.cmd"
    codex.write_text("", encoding="utf-8")

    return Config(
        data_dir=str(tmp_path / "agent"),
        port=DEFAULT_PORT,
        codex_cmd=str(codex),
        rag_db=str(rag),
        repo_root=str(repo),
        hf_cache_dir=str(tmp_path / "hfcache"),
        token="tok",
    )


def test_selfcheck_all_good_returns_zero(tmp_path):
    cfg = _good_config(tmp_path)
    rc, messages = cfgmod.run_selfcheck(cfg)
    assert rc == 0, f"expected ok, got failures: {messages}"


def test_selfcheck_bad_rag_db_distinct_message(tmp_path):
    cfg = _good_config(tmp_path)
    cfg.rag_db = str(tmp_path / "nope-rag")
    rc, messages = cfgmod.run_selfcheck(cfg)
    assert rc != 0
    joined = "\n".join(messages)
    assert RAG_MARK in joined
    assert CODEX_MARK not in joined
    assert PANEL_MARK not in joined
    assert HF_MARK not in joined


def test_selfcheck_unresolvable_codex_distinct_message(tmp_path):
    cfg = _good_config(tmp_path)
    cfg.codex_cmd = ""  # unresolved
    rc, messages = cfgmod.run_selfcheck(cfg)
    assert rc != 0
    joined = "\n".join(messages)
    assert CODEX_MARK in joined
    assert RAG_MARK not in joined
    assert PANEL_MARK not in joined
    assert HF_MARK not in joined


def test_selfcheck_missing_panel_distinct_message(tmp_path):
    cfg = _good_config(tmp_path)
    # remove one required panel file
    (Path(cfg.repo_root) / "panel" / "app.js").unlink()
    rc, messages = cfgmod.run_selfcheck(cfg)
    assert rc != 0
    joined = "\n".join(messages)
    assert PANEL_MARK in joined
    assert RAG_MARK not in joined
    assert CODEX_MARK not in joined
    assert HF_MARK not in joined


def test_selfcheck_missing_hf_cache_distinct_message(tmp_path):
    cfg = _good_config(tmp_path)
    cfg.hf_cache_dir = str(tmp_path / "empty-hf")
    Path(cfg.hf_cache_dir).mkdir()
    rc, messages = cfgmod.run_selfcheck(cfg)
    assert rc != 0
    joined = "\n".join(messages)
    assert HF_MARK in joined
    assert RAG_MARK not in joined
    assert CODEX_MARK not in joined
    assert PANEL_MARK not in joined


def test_selfcheck_reports_all_failures_together(tmp_path):
    """Every missing prerequisite is reported (not just the first)."""
    cfg = _good_config(tmp_path)
    cfg.codex_cmd = ""
    cfg.rag_db = str(tmp_path / "nope-rag")
    rc, messages = cfgmod.run_selfcheck(cfg)
    assert rc != 0
    joined = "\n".join(messages)
    assert CODEX_MARK in joined and RAG_MARK in joined


# ----------------------------------------------- config robustness (EUD-030-75a4)
#
# Design (warnings mechanism):
#   * ``Config`` carries a ``warnings: list[str]`` of NON-fatal diagnostics
#     gathered during resolve (an auto-discovered cfg that won't parse, an
#     invalid port). Resolution always proceeds on defaults so the server boots.
#   * ``Config`` carries a ``cfg_error: str | None`` set ONLY when a cfg that was
#     EXPLICITLY pointed at (a direct path, or env EUD_DATA_DIR / CLI data_dir)
#     exists but fails to parse. This is the distinct, FATAL diagnostic.
#   * ``run_selfcheck`` appends ``cfg.warnings`` to its messages WITHOUT forcing a
#     non-zero exit, but appends ``cfg.cfg_error`` AND forces non-zero exit (so an
#     operator who pointed at a broken cfg learns why it was ignored).
#
# Auto-discovery seam: ``resolve`` takes ``default_data_dir`` (the sibling/default
# fallback). A broken cfg found there (no explicit env/CLI signal) is a WARNING.

CFG_UNPARSEABLE_MARK = "agent.cfg present but unparseable"
PORT_MARK = "port"

# Variants that EXIST as files but must not parse into a usable dict.
_BOM_CFG = '{"port": 9001}'.encode("utf-8-sig")  # leading UTF-8 BOM (EF BB BF)
_MALFORMED_CFG = b'{"port": 9001'  # truncated JSON
_NON_DICT_CFG = b'[1, 2, 3]'  # valid JSON, but not an object


def _all_env_clean(monkeypatch):
    for var in (
        "EUD_DATA_DIR", "EUD_PORT", "CODEX_CMD",
        "EUD_RAG_DB", "EUD_REPO_ROOT", "HF_HUB_CACHE", "HF_HOME", "HF_HOME_HUB",
    ):
        monkeypatch.delenv(var, raising=False)


@pytest.mark.parametrize(
    "raw",
    [_BOM_CFG, _MALFORMED_CFG, _NON_DICT_CFG],
    ids=["bom", "malformed", "non-dict"],
)
def test_explicit_unparseable_cfg_falls_back_but_flags_error(
    monkeypatch, tmp_path, raw
):
    """An EXPLICIT cfg that exists but won't parse: resolve on defaults, set
    cfg_error (distinct), and selfcheck exits non-zero with its own message."""
    _all_env_clean(monkeypatch)
    cfg_path = _write_raw_agent_cfg(tmp_path, raw)
    cfg = Config.resolve(cli={}, agent_cfg_path=cfg_path)

    # Resolution proceeds on defaults (server must boot).
    assert cfg.port == DEFAULT_PORT
    # Distinct diagnostic recorded, pointing at the path.
    assert cfg.cfg_error is not None
    assert CFG_UNPARSEABLE_MARK in cfg.cfg_error
    assert str(cfg_path) in cfg.cfg_error

    rc, messages = cfgmod.run_selfcheck(cfg)
    joined = "\n".join(messages)
    assert rc != 0, "explicit unparseable cfg must force a non-zero selfcheck"
    assert CFG_UNPARSEABLE_MARK in joined


@pytest.mark.parametrize(
    "raw",
    [_BOM_CFG, _MALFORMED_CFG, _NON_DICT_CFG],
    ids=["bom", "malformed", "non-dict"],
)
def test_unparseable_cfg_via_env_data_dir_flags_error(monkeypatch, tmp_path, raw):
    """Env EUD_DATA_DIR pointing at a dir with a broken cfg counts as EXPLICIT:
    cfg_error set, selfcheck non-zero."""
    _all_env_clean(monkeypatch)
    cfg_path = _write_raw_agent_cfg(tmp_path, raw)
    monkeypatch.setenv("EUD_DATA_DIR", str(cfg_path.parent))
    cfg = Config.resolve(cli={})  # auto-locate via EUD_DATA_DIR

    assert cfg.port == DEFAULT_PORT
    assert cfg.cfg_error is not None
    assert CFG_UNPARSEABLE_MARK in cfg.cfg_error

    rc, messages = cfgmod.run_selfcheck(cfg)
    assert rc != 0
    assert CFG_UNPARSEABLE_MARK in "\n".join(messages)


def test_auto_discovered_broken_cfg_warns_but_selfcheck_ok(monkeypatch, tmp_path):
    """A broken cfg found by AUTO-discovery (no explicit env/CLI signal) is a
    WARNING: resolution proceeds, and selfcheck (otherwise good) still exits 0
    but surfaces the warning line."""
    _all_env_clean(monkeypatch)
    # Broken cfg sits under the auto/sibling default data dir, not pointed at.
    _write_raw_agent_cfg(tmp_path, _MALFORMED_CFG)
    good = _good_config(tmp_path / "good")  # satisfiable prerequisites
    cfg = Config.resolve(
        cli={},
        agent_cfg_path="__auto__",
        default_data_dir=str(tmp_path / "agent"),
    )

    # Auto-discovered breakage is a warning, NOT a fatal cfg_error.
    assert cfg.cfg_error is None
    assert any(CFG_UNPARSEABLE_MARK in w for w in cfg.warnings)
    assert cfg.port == DEFAULT_PORT

    # Borrow the satisfiable prereq paths so only the warning would matter.
    cfg.codex_cmd = good.codex_cmd
    cfg.rag_db = good.rag_db
    cfg.repo_root = good.repo_root
    cfg.hf_cache_dir = good.hf_cache_dir

    rc, messages = cfgmod.run_selfcheck(cfg)
    joined = "\n".join(messages)
    assert rc == 0, f"auto-discovered warning must not fail selfcheck: {messages}"
    assert CFG_UNPARSEABLE_MARK in joined, "the warning must still be surfaced"


def test_bad_port_via_env_falls_back_with_diagnostic(monkeypatch, tmp_path):
    """A non-numeric EUD_PORT must NOT raise: default port + a warning."""
    _all_env_clean(monkeypatch)
    monkeypatch.setenv("EUD_PORT", "not-a-number")
    cfg = Config.resolve(cli={}, agent_cfg_path=None)  # must not raise ValueError
    assert cfg.port == DEFAULT_PORT
    assert any(PORT_MARK in w for w in cfg.warnings)
    assert any("not-a-number" in w for w in cfg.warnings)

    rc, messages = cfgmod.run_selfcheck(cfg)
    assert PORT_MARK in "\n".join(messages)


def test_bad_port_via_cfg_falls_back_with_diagnostic(monkeypatch, tmp_path):
    """A non-numeric port from agent.cfg must NOT raise: default port + warning."""
    _all_env_clean(monkeypatch)
    cfg_path = _write_agent_cfg(tmp_path, {"port": "not-a-number"})
    cfg = Config.resolve(cli={}, agent_cfg_path=cfg_path)  # must not raise
    assert cfg.port == DEFAULT_PORT
    assert any(PORT_MARK in w for w in cfg.warnings)

    rc, messages = cfgmod.run_selfcheck(cfg)
    assert PORT_MARK in "\n".join(messages)


# ----------------------------------------------------- HF cache resolution


def test_hf_hub_cache_env_is_full_hub_path(monkeypatch, tmp_path):
    """HF_HUB_CACHE (highest precedence) is used verbatim as the hub path."""
    _all_env_clean(monkeypatch)
    hub = tmp_path / "explicit-hub"
    monkeypatch.setenv("HF_HUB_CACHE", str(hub))
    cfg = Config.resolve(cli={}, agent_cfg_path=None)
    assert Path(cfg.hf_cache_dir) == hub


def test_hf_home_env_uses_hub_subdir(monkeypatch, tmp_path):
    """HF_HOME (no HF_HUB_CACHE) -> <HF_HOME>/hub."""
    _all_env_clean(monkeypatch)
    home = tmp_path / "hfhome"
    monkeypatch.setenv("HF_HOME", str(home))
    cfg = Config.resolve(cli={}, agent_cfg_path=None)
    assert Path(cfg.hf_cache_dir) == home / "hub"


def test_hf_hub_cache_takes_precedence_over_hf_home(monkeypatch, tmp_path):
    _all_env_clean(monkeypatch)
    monkeypatch.setenv("HF_HUB_CACHE", str(tmp_path / "hub"))
    monkeypatch.setenv("HF_HOME", str(tmp_path / "home"))
    cfg = Config.resolve(cli={}, agent_cfg_path=None)
    assert Path(cfg.hf_cache_dir) == tmp_path / "hub"


def test_hf_defaults_when_no_env(monkeypatch, tmp_path):
    _all_env_clean(monkeypatch)
    cfg = Config.resolve(cli={}, agent_cfg_path=None)
    assert Path(cfg.hf_cache_dir) == Path(cfgmod.DEFAULT_HF_CACHE)


def test_hf_home_hub_legacy_env_is_ignored(monkeypatch, tmp_path):
    """The invented HF_HOME_HUB is no longer honored: it must not win."""
    _all_env_clean(monkeypatch)
    monkeypatch.setenv("HF_HOME_HUB", str(tmp_path / "legacy"))
    cfg = Config.resolve(cli={}, agent_cfg_path=None)
    assert Path(cfg.hf_cache_dir) != tmp_path / "legacy"
    assert Path(cfg.hf_cache_dir) == Path(cfgmod.DEFAULT_HF_CACHE)


# ------------------------------------------------------------- entrypoint (subproc)


def test_main_selfcheck_exit_code_matches(tmp_path):
    """`python -m eud_agent --selfcheck` returns selfcheck's exit code.

    Smoke that the argparse entrypoint wires --selfcheck to run_selfcheck and
    propagates the non-zero exit for a doctored (broken) config via env.
    """
    import subprocess
    import sys

    # Force a broken codex resolution by pointing CODEX_CMD at a nonexistent file
    # and an empty data dir so agent.cfg is absent. We only assert non-zero exit
    # and that some specific marker appears in stderr/stdout.
    env = {
        **_clean_env(),
        "CODEX_CMD": str(tmp_path / "definitely-not-codex.cmd"),
        "EUD_RAG_DB": str(tmp_path / "no-rag"),
        "EUD_REPO_ROOT": str(tmp_path / "no-repo"),
    }
    proc = subprocess.run(
        [sys.executable, "-m", "eud_agent", "--selfcheck"],
        capture_output=True,
        text=True,
        env=env,
        cwd=str(_server_dir()),
    )
    assert proc.returncode != 0
    out = proc.stdout + proc.stderr
    assert RAG_MARK in out or PANEL_MARK in out or CODEX_MARK in out


def test_main_without_selfcheck_launches_server(monkeypatch):
    """Without --selfcheck the entrypoint launches the resident server.

    EUD-018 wired the no-flag path to ``_serve`` (was an "app not implemented"
    stub). We patch ``_serve`` so the test stays fast (no real bind / model
    warmup / hang) and assert main() routes to it with the resolved Config.
    """
    from eud_agent import __main__ as entry

    called = {}

    def fake_serve(cfg) -> int:
        called["cfg"] = cfg
        return 0

    monkeypatch.setattr(entry, "_serve", fake_serve)
    rc = entry.main([])
    assert rc == 0
    assert "cfg" in called, "main() did not route the no-flag path to _serve"


def _server_dir() -> Path:
    # tests/ -> server/
    return Path(__file__).resolve().parents[1]


def _clean_env() -> dict:
    import os

    env = dict(os.environ)
    for var in ("EUD_DATA_DIR", "EUD_PORT", "EUD_RAG_DB", "EUD_REPO_ROOT"):
        env.pop(var, None)
    return env


if __name__ == "__main__":
    raise SystemExit(pytest.main([__file__, "-q"]))
