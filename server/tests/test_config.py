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
