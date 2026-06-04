"""Entry point: ``python -m eud_agent``.

  * ``--selfcheck`` -> validate every prerequisite (verify.md "Stage: smoke")
    and exit with code 0 (all good) or non-zero (one line per failed check).
    Never loads the embedding model; never raises on a missing prerequisite.
  * (no flag)       -> launch the resident FastAPI server. We bind ``127.0.0.1``
    on the configured port (falling back to an OS-assigned ephemeral port when
    taken — ``app.resolve_bound_socket``), record the ACTUAL resolved port on the
    Config (so the Origin check + ``server.ready`` advertise the real port), and
    hand the pre-bound socket to uvicorn. The heartbeat watcher gets a handle to
    the uvicorn ``Server`` so a stale heartbeat self-terminates the process.
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


def _serve(cfg: Config) -> int:
    """Bind 127.0.0.1 (cfg port, fallback ephemeral) and run uvicorn.

    Imports of ``app`` / ``uvicorn`` are deferred to here so ``--selfcheck`` stays
    light (it must not pull FastAPI/uvicorn). The pre-bound socket's port becomes
    ``cfg.port`` BEFORE ``create_app`` so the Origin check and the ``server.ready``
    writer both advertise the actual resolved port (architecture.md port policy).
    """
    import uvicorn

    from .app import create_app, resolve_bound_socket

    # Pre-bind: cfg.port, falling back to an OS-assigned ephemeral port if taken.
    sock = resolve_bound_socket(cfg.port)
    cfg.port = sock.getsockname()[1]  # the single source of truth for the port

    app = create_app(cfg, start_lifecycle=True)

    config = uvicorn.Config(app, log_level="info")
    server = uvicorn.Server(config)
    # Let the heartbeat watcher self-terminate by flipping should_exit.
    app.state.shutdown_state["server"] = server

    server.run(sockets=[sock])
    return 0


def main(argv: list[str] | None = None) -> int:
    args = _build_parser().parse_args(argv)
    cfg = Config.resolve(cli=_cli_overrides(args))

    if args.selfcheck:
        return _selfcheck(cfg)

    return _serve(cfg)


if __name__ == "__main__":
    raise SystemExit(main())
