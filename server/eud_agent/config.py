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

Robustness (EUD-009 review advisories 1-3, hardened in EUD-030-75a4):

  * A cfg that EXISTS but fails to parse (BOM'd, malformed JSON, or a non-dict
    top level) no longer falls back silently (advisory 1, makes verify.md
    "agent.cfg schema" true). Resolution still proceeds on defaults so the
    server boots, but the breakage is surfaced:
      - when the cfg was EXPLICITLY pointed at (a direct path, ``EUD_DATA_DIR``,
        or a CLI ``data_dir``) -> ``Config.cfg_error`` carries the distinct
        message ``"agent.cfg present but unparseable at <path>"`` and selfcheck
        exits non-zero;
      - when AUTO-discovered (the ``default_data_dir`` sibling convention, no
        explicit signal) -> a line is appended to ``Config.warnings`` while
        resolution proceeds; selfcheck PRINTS it but still exits 0 when the
        prerequisites are otherwise fine.
  * A non-numeric ``port`` from env/cfg no longer raises an uncaught
    ``ValueError`` (advisory 2): it falls back to ``DEFAULT_PORT`` with a
    ``warnings`` line.
  * HF cache resolution uses the STANDARD HuggingFace env vars (advisory 3):
    ``HF_HUB_CACHE`` (full hub path) > ``HF_HOME``/``hub`` > built-in default;
    the invented ``HF_HOME_HUB`` is gone.

BOM note: ``agent.cfg`` is read with plain ``utf-8`` (rules.md forbids
``utf-8-sig`` for IPC files). A BOM'd file therefore fails ``json.loads`` and is
handled as the unparseable case above — intentional, not a bug.
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
# Standard HuggingFace cache env vars (advisory 3): HF_HUB_CACHE is the full hub
# path; HF_HOME's hub lives at <HF_HOME>/hub. The invented HF_HOME_HUB is gone.
ENV_HF_HUB_CACHE = "HF_HUB_CACHE"
ENV_HF_HOME = "HF_HOME"


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
    # Non-fatal diagnostics gathered during resolve (auto-discovered broken cfg,
    # non-numeric port). Resolution proceeds on defaults; selfcheck prints these
    # but does not fail on them.
    warnings: list[str] = field(default_factory=list)
    # Set only when an EXPLICITLY pointed-at cfg exists but won't parse; selfcheck
    # includes it AND forces a non-zero exit (EUD-009 advisory 1).
    cfg_error: str | None = None

    # ------------------------------------------------------------------ resolve
    @classmethod
    def resolve(
        cls,
        cli: dict | None = None,
        env: dict | None = None,
        agent_cfg_path: str | os.PathLike | None = "__auto__",
        default_data_dir: str | os.PathLike | None = None,
    ) -> Config:
        """Build a Config using CLI > env > agent.cfg > defaults.

        ``cli`` is a dict of already-parsed CLI overrides (keys mirror the
        dataclass fields; missing/None keys do not override). ``env`` defaults
        to ``os.environ``. ``agent_cfg_path``:

          * ``"__auto__"`` (default) -> locate agent.cfg via CLI/env data dir,
            then (if neither is set) under ``default_data_dir``.
          * an explicit path        -> load that file if it exists.
          * ``None``                 -> skip agent.cfg entirely.

        ``default_data_dir`` is the sibling/default fallback used only by the
        ``"__auto__"`` path when no CLI/env data dir is given. A cfg found there
        is treated as AUTO-discovered (broken -> warning, not a fatal error);
        a cfg pointed at directly or via CLI/env is EXPLICIT (broken ->
        ``cfg_error`` + non-zero selfcheck). See module docstring (advisory 1).
        """
        cli = {k: v for k, v in (cli or {}).items() if v is not None}
        env = os.environ if env is None else env

        warnings: list[str] = []
        cfg_error: str | None = None

        # Locate + load agent.cfg (layer 3). ``explicit`` records whether the
        # cfg location came from a direct/CLI/env signal vs. auto-discovery.
        cfg_path, explicit = cls._locate_agent_cfg(
            agent_cfg_path, cli, env, default_data_dir
        )
        file_cfg, parse_failed = cls._load_agent_cfg(cfg_path)
        if parse_failed:
            msg = f"agent.cfg present but unparseable at {cfg_path}"
            if explicit:
                cfg_error = msg
            else:
                warnings.append(msg)

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

        # port: guard the int() coercion (advisory 2). A non-numeric value from
        # env/cfg falls back to the default with a warning instead of raising.
        raw_port = pick("port", ENV_PORT, DEFAULT_PORT)
        try:
            port = int(raw_port)
        except (TypeError, ValueError):
            warnings.append(
                f"port: invalid value {raw_port!r}; "
                f"falling back to default {DEFAULT_PORT}."
            )
            port = DEFAULT_PORT

        repo_root = str(pick("repo_root", ENV_REPO_ROOT, str(_REPO_ROOT)))
        rag_db = str(pick("rag_db", ENV_RAG_DB, DEFAULT_RAG_DB))
        hf_cache_dir = cls._resolve_hf_cache(env)

        codex_cmd = cls._resolve_codex(cli, env, file_cfg)

        return cls(
            data_dir=str(data_dir),
            port=port,
            codex_cmd=codex_cmd,
            rag_db=rag_db,
            repo_root=repo_root,
            hf_cache_dir=hf_cache_dir,
            warnings=warnings,
            cfg_error=cfg_error,
        )

    # ----------------------------------------------------------- agent.cfg I/O
    @staticmethod
    def _locate_agent_cfg(
        agent_cfg_path, cli: dict, env: dict, default_data_dir=None
    ) -> tuple[Path | None, bool]:
        """Return ``(cfg_path_or_None, explicit)``.

        ``explicit`` is True when the location came from a direct path arg, a
        CLI ``data_dir``, or ``EUD_DATA_DIR``; False when it was auto-discovered
        under ``default_data_dir``.
        """
        if agent_cfg_path is None:
            return None, False
        if agent_cfg_path != "__auto__":
            p = Path(agent_cfg_path)
            return (p if p.is_file() else None), True
        # auto: data dir from CLI then env (explicit), else the default (auto).
        data_dir = cli.get("data_dir") or env.get(ENV_DATA_DIR)
        if data_dir:
            p = Path(data_dir) / "agent.cfg"
            return (p if p.is_file() else None), True
        if default_data_dir:
            p = Path(default_data_dir) / "agent.cfg"
            return (p if p.is_file() else None), False
        return None, False

    @staticmethod
    def _load_agent_cfg(cfg_path: Path | None) -> tuple[dict, bool]:
        """Return ``(cfg_dict, parse_failed)``.

        ``parse_failed`` is True when the file EXISTS but cannot be turned into
        a usable dict (read error, malformed JSON incl. a leading BOM, or a
        non-dict top level). The caller decides whether that is a fatal
        ``cfg_error`` (explicit cfg) or a ``warnings`` line (auto-discovered).
        """
        if not cfg_path or not cfg_path.is_file():
            return {}, False
        try:
            # File.ReadAllText strips a BOM on the bridge side, but Python must
            # read/write BOM-free; plain utf-8 is correct here (rules.md). A
            # BOM'd file therefore fails json.loads -> parse_failed=True.
            text = cfg_path.read_text(encoding="utf-8")
            data = json.loads(text)
        except (OSError, ValueError):
            return {}, True
        if not isinstance(data, dict):
            return {}, True
        return data, False

    # -------------------------------------------------------------- HF cache
    @staticmethod
    def _resolve_hf_cache(env: dict) -> str:
        """Standard HuggingFace hub-cache resolution (EUD-009 advisory 3).

        ``HF_HUB_CACHE`` (the full hub path) > ``HF_HOME``/``hub`` > the
        built-in default. The invented ``HF_HOME_HUB`` is no longer honored.
        """
        if env.get(ENV_HF_HUB_CACHE):
            return str(env[ENV_HF_HUB_CACHE])
        if env.get(ENV_HF_HOME):
            return str(Path(env[ENV_HF_HOME]) / "hub")
        return DEFAULT_HF_CACHE

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
    (each tagged so the operator can tell which prerequisite is missing).

    Config diagnostics from ``resolve`` (EUD-009 advisories 1-2) are folded in:
      * ``cfg.cfg_error`` (an EXPLICITLY pointed-at cfg that won't parse) is a
        FAILED check -> distinct message + non-zero exit.
      * ``cfg.warnings`` (auto-discovered broken cfg, non-numeric port) are
        PRINTED but do NOT fail the check; selfcheck still exits 0 when the real
        prerequisites are fine.
    """
    failures: list[str] = []

    # 0) config diagnostics gathered during resolve.
    if cfg.cfg_error:
        failures.append(cfg.cfg_error)

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

    # Non-fatal warnings are surfaced in the messages but never change the exit
    # code (the operator sees them; the server still boots).
    messages = failures + list(cfg.warnings)
    return (1 if failures else 0), messages
