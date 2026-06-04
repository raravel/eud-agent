"""Entry point: ``python -m eud_agent``.

  * ``--selfcheck`` -> validate every prerequisite (verify.md "Stage: smoke")
    and exit with code 0 (all good) or non-zero (one line per failed check).
    Never loads the embedding model; never raises on a missing prerequisite.
  * (no flag)       -> would launch the FastAPI server, but ``app.py`` is owned
    by a separate task (EUD-010). Until it exists we fail fast with a clear
    message rather than importing a missing module.
"""

from __future__ import annotations

import argparse
import sys

from .config import Config, run_selfcheck


def _build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        prog="eud_agent",
        description="EUD Editor 3 external agent server.",
    )
    p.add_argument(
        "--selfcheck",
        action="store_true",
        help="validate prerequisites (config, codex, RAG DB, bge-m3, panel) "
        "and exit; does not load the embedding model.",
    )
    p.add_argument("--port", type=int, default=None, help="override server port.")
    p.add_argument(
        "--data-dir",
        default=None,
        help="editor Data\\agent dir (locates agent.cfg).",
    )
    p.add_argument(
        "--codex-cmd", default=None, help="override the codex shim path."
    )
    p.add_argument(
        "--rag-db", default=None, help="override the chromadb store path."
    )
    p.add_argument(
        "--repo-root", default=None, help="override the repo root."
    )
    return p


def _cli_overrides(args: argparse.Namespace) -> dict:
    return {
        "port": args.port,
        "data_dir": args.data_dir,
        "codex_cmd": args.codex_cmd,
        "rag_db": args.rag_db,
        "repo_root": args.repo_root,
    }


def _selfcheck(cfg: Config) -> int:
    rc, messages = run_selfcheck(cfg)
    if rc == 0:
        print("selfcheck: OK (all prerequisites satisfied)")
        print(f"  port={cfg.port} repo_root={cfg.repo_root}")
        print(f"  codex_cmd={cfg.codex_cmd}")
        print(f"  rag_db={cfg.rag_db}")
        return 0
    print("selfcheck: FAILED", file=sys.stderr)
    for msg in messages:
        print(f"  - {msg}", file=sys.stderr)
    return rc


def main(argv: list[str] | None = None) -> int:
    args = _build_parser().parse_args(argv)
    cfg = Config.resolve(cli=_cli_overrides(args))

    if args.selfcheck:
        return _selfcheck(cfg)

    # Server launch path: app.py is out of scope here (EUD-010). Do NOT create
    # it; fail fast with a clear, actionable message.
    print(
        "server app not implemented yet (EUD-010): "
        "run `python -m eud_agent --selfcheck` for prerequisite validation.",
        file=sys.stderr,
    )
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
