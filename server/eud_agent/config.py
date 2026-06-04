"""Configuration resolution and startup self-check for the eud_agent server.

Resolution order (highest precedence first):

    CLI args  >  environment variables  >  agent.cfg JSON  >  built-in defaults

``agent.cfg`` is written into the editor's ``Data\\agent\\`` by
``scripts/install_dropin.ps1``; the drop-in Lua bridge cannot know repo/venv
locations any other way (see architecture.md "Boot and lifecycle"). It is
located either by an explicit path, by ``EUD_DATA_DIR`` (its parent dir), or by
a CLI-provided data dir.

``run_selfcheck`` validates every prerequisite WITHOUT loading the embedding
model (verify.md "Stage: smoke"): each missing prerequisite contributes its own
distinct message and forces a non-zero exit.
"""

from __future__ import annotations

import json
import os
import shutil
import uuid
from dataclasses import dataclass, field
from pathlib import Path

# repo_root: server/eud_agent/config.py -> parents[2] == repo root.
_REPO_ROOT = Path(__file__).resolve().parents[2]

# Built-in defaults (grounded in tech-stack.md / this machine).
DEFAULT_PORT = 8765
DEFAULT_RAG_DB = r"C:\Users\ifthe\proj\eud\ECA\chromadb_bge"
DEFAULT_HF_CACHE = r"C:\Users\ifthe\.cache\huggingface\hub"
BGE_M3_DIRNAME = "models--BAAI--bge-m3"

# Panel static files that must be served from one origin (no CDN).
PANEL_FILES = ("index.html", "app.js", "style.css")

# Env var names (resolution layer 2).
ENV_DATA_DIR = "EUD_DATA_DIR"
ENV_PORT = "EUD_PORT"
ENV_CODEX = "CODEX_CMD"
ENV_RAG_DB = "EUD_RAG_DB"
ENV_REPO_ROOT = "EUD_REPO_ROOT"
ENV_HF_CACHE = "HF_HOME_HUB"


@dataclass
class Config:
    """Resolved server configuration plus the per-session token."""

    data_dir: str
    port: int
    codex_cmd: str
    rag_db: str
    repo_root: str
    hf_cache_dir: str
    token: str = field(default_factory=lambda: str(uuid.uuid4()))

    # ------------------------------------------------------------------ resolve
    @classmethod
    def resolve(
        cls,
        cli: dict | None = None,
        env: dict | None = None,
        agent_cfg_path: str | os.PathLike | None = "__auto__",
    ) -> Config:
        """Build a Config using CLI > env > agent.cfg > defaults.

        ``cli`` is a dict of already-parsed CLI overrides (keys mirror the
        dataclass fields; missing/None keys do not override). ``env`` defaults
        to ``os.environ``. ``agent_cfg_path``:

          * ``"__auto__"`` (default) -> locate agent.cfg via CLI/env data dir.
          * an explicit path        -> load that file if it exists.
          * ``None``                 -> skip agent.cfg entirely.
        """
        cli = {k: v for k, v in (cli or {}).items() if v is not None}
        env = os.environ if env is None else env

        # Locate + load agent.cfg (layer 3).
        cfg_path = cls._locate_agent_cfg(agent_cfg_path, cli, env)
        file_cfg = cls._load_agent_cfg(cfg_path)

        def pick(key: str, env_name: str, default):
            if key in cli:
                return cli[key]
            if env_name and env.get(env_name) not in (None, ""):
                return env[env_name]
            if key in file_cfg and file_cfg[key] not in (None, ""):
                return file_cfg[key]
            return default

        # data_dir: prefer the directory we actually loaded the cfg from.
        data_dir_default = str(cfg_path.parent) if cfg_path else ""
        data_dir = pick("data_dir", ENV_DATA_DIR, data_dir_default)

        port = int(pick("port", ENV_PORT, DEFAULT_PORT))
        repo_root = str(pick("repo_root", ENV_REPO_ROOT, str(_REPO_ROOT)))
        rag_db = str(pick("rag_db", ENV_RAG_DB, DEFAULT_RAG_DB))
        hf_cache_dir = str(pick("hf_cache_dir", ENV_HF_CACHE, DEFAULT_HF_CACHE))

        codex_cmd = cls._resolve_codex(cli, env, file_cfg)

        return cls(
            data_dir=str(data_dir),
            port=port,
            codex_cmd=codex_cmd,
            rag_db=rag_db,
            repo_root=repo_root,
            hf_cache_dir=hf_cache_dir,
        )

    # ----------------------------------------------------------- agent.cfg I/O
    @staticmethod
    def _locate_agent_cfg(
        agent_cfg_path, cli: dict, env: dict
    ) -> Path | None:
        if agent_cfg_path is None:
            return None
        if agent_cfg_path != "__auto__":
            p = Path(agent_cfg_path)
            return p if p.is_file() else None
        # auto: data dir from CLI then env, look for agent.cfg inside it.
        data_dir = cli.get("data_dir") or env.get(ENV_DATA_DIR)
        if not data_dir:
            return None
        p = Path(data_dir) / "agent.cfg"
        return p if p.is_file() else None

    @staticmethod
    def _load_agent_cfg(cfg_path: Path | None) -> dict:
        if not cfg_path or not cfg_path.is_file():
            return {}
        try:
            # File.ReadAllText strips a BOM on the bridge side, but Python must
            # read/write BOM-free; plain utf-8 is correct here (rules.md).
            text = cfg_path.read_text(encoding="utf-8")
            data = json.loads(text)
        except (OSError, ValueError):
            return {}
        return data if isinstance(data, dict) else {}

    # -------------------------------------------------------------- codex
    @staticmethod
    def _resolve_codex(cli: dict, env: dict, file_cfg: dict) -> str:
        """codex shim path: cli > CODEX_CMD env > agent.cfg > shutil.which."""
        if cli.get("codex_cmd"):
            return str(cli["codex_cmd"])
        if env.get(ENV_CODEX):
            return str(env[ENV_CODEX])
        if file_cfg.get("codex_cmd"):
            return str(file_cfg["codex_cmd"])
        # NEVER spawn bare "codex": resolve to the .cmd shim (rules.md). Returns
        # "" when unresolved; the server fails fast on use, selfcheck reports it.
        return shutil.which("codex") or ""

    # -------------------------------------------------------------- token
    @staticmethod
    def new_token() -> str:
        return str(uuid.uuid4())


# ===================================================================== selfcheck


def run_selfcheck(cfg: Config) -> tuple[int, list[str]]:
    """Validate every server prerequisite without loading the embedding model.

    Returns ``(exit_code, messages)``: ``exit_code`` is 0 when all checks pass,
    non-zero otherwise; ``messages`` holds one distinct line per FAILED check
    (each tagged so the operator can tell which prerequisite is missing). On
    success ``messages`` is empty.
    """
    failures: list[str] = []

    # 1) codex shim resolution (shutil.which / override) -> a real file.
    if not cfg.codex_cmd:
        failures.append(
            "codex: shim not resolved (shutil.which('codex') failed); "
            "install codex or set CODEX_CMD to the codex.cmd path."
        )
    elif not Path(cfg.codex_cmd).is_file():
        failures.append(
            f"codex: resolved path does not exist: {cfg.codex_cmd}"
        )

    # 2) RAG DB path exists and looks like a chromadb sqlite store (no heavy
    #    chromadb import: a path/sqlite-file existence check is sufficient).
    rag = Path(cfg.rag_db)
    if not rag.is_dir():
        failures.append(f"RAG DB: directory not found: {cfg.rag_db}")
    elif not (rag / "chroma.sqlite3").is_file():
        failures.append(
            f"RAG DB: chroma.sqlite3 missing in {cfg.rag_db} "
            "(not a chromadb persistent store?)"
        )

    # 3) bge-m3 weights present in the HF hub cache (no model load).
    hf = Path(cfg.hf_cache_dir) / BGE_M3_DIRNAME
    if not hf.is_dir():
        failures.append(
            f"bge-m3: weights not found in HF cache: {hf} "
            "(first query would download ~4.3 GB)."
        )

    # 4) panel static files present, relative to repo_root.
    panel_dir = Path(cfg.repo_root) / "panel"
    missing_panel = [n for n in PANEL_FILES if not (panel_dir / n).is_file()]
    if missing_panel:
        failures.append(
            f"panel: missing static file(s) {missing_panel} under {panel_dir}"
        )

    return (1 if failures else 0), failures
