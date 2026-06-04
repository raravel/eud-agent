"""Headless jobs-queue runner — the retention of the verified ECA codex runner.

This is the headless-retention half of the agent (features/02 "runner_cli.py"):
it keeps the verified job-queue flow of the frozen ECA runner draft but
refactors every heavy step ONTO the shared server modules so the panel path and
the headless path share one implementation of prompt-building, fence extraction,
RAG search, and the file-IPC conventions. It exists so codex / RAG can be
exercised WITHOUT the panel or the editor.

Module wiring (architecture.md component graph ``cli -> rag / codex / bridge``):

  * ``config.Config.resolve`` resolves ``data_dir`` / ``rag_db`` / ``codex_cmd`` /
    ``repo_root`` (CLI ``data_dir`` wins per the resolution order).
  * ``rag.search`` provides the (optional) context chunks; ``RagUnavailable``
    degrades to a no-context run with a printed note (features/02 edge case).
  * ``codex_client.build_prompt`` composes the prompt and
    ``codex_client.CodexClient.generate`` runs the real codex subprocess.
    ``--mock`` still builds the prompt and goes through the CodexClient seam
    (so the RAG context remains observable) but, for the real client, swaps in a
    canned fenced reply routed through ``codex_client.extract_code`` — the SAME
    fence handling the real path uses (no local fence/prompt duplication).
  * the produced command file follows the bridge file-IPC convention: a SET
    command written to ``<data_dir>\\inbox\\agent_<id>.cmd`` as
    ``SET <target>\\n<code>``, UTF-8 **without** a BOM (rules.md "IPC and
    encoding"). The ``agent_*`` namespace is CONTRACTUAL — it is the legacy
    runner's namespace; the server's ``bridge_io`` owns ``srv-*`` and the two
    never collide (architecture.md "File IPC protocol").

Job queue (legacy schema, mirrored from the frozen ECA runner draft — that
reference module is read-only and is deliberately NOT imported here):

    <data_dir>\\jobs\\<id>.json   {"instruction": str, "target": str,
                                   "context": bool (default True)}

Each processed job writes ``inbox\\agent_<id>.cmd`` and renames the job
``jobs\\<id>.json`` -> ``jobs\\<id>.done``.

CLI: ``--once`` processes the current queue once and exits 0; without it the
runner polls every second. ``--mock`` fakes codex (no subprocess); ``--no-context``
skips RAG. ``--data-dir`` (legacy alias ``--agent-dir``) overrides the editor
``Data\\agent`` directory.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import sys
import time
from pathlib import Path

from . import codex_client, rag
from .config import Config
from .rag import RagUnavailable

# Canned codex reply for --mock: a single fenced block so it flows through the
# SAME codex_client.extract_code path the real codex output does (the fence
# semantics live there, never duplicated here).
_MOCK_CODEX_OUTPUT = (
    "```eps\n"
    "// [mock] codex disabled — dummy epScript\n"
    "function afterTriggerExec() {\n"
    "    foreach(p : EUDLoopPlayer()) {\n"
    '        setdeaths(p, SetTo, 1, "Terran Marine");\n'
    "    }\n"
    "}\n"
    "```"
)

#: RAG poll budget passed to ``rag.search`` (matches the legacy runner's n=5).
_RAG_K = 5

#: Idle poll interval for the resident (non-``--once``) loop.
_POLL_INTERVAL = 1.0


# --------------------------------------------------------------------------- #
# Job processing.
# --------------------------------------------------------------------------- #


def _retrieve_context(instruction: str, rag_db: str) -> list[str]:
    """Return RAG context chunks for ``instruction`` (empty list on degrade).

    Routes through the shared in-process ``rag.search``; a ``RagUnavailable``
    (bad/missing DB path, failed load) degrades to no context with a printed
    note rather than failing the job (features/02 edge case).
    """
    try:
        hits = rag.search(instruction, _RAG_K, rag_db=rag_db)
    except RagUnavailable as exc:
        print(f"[rag] unavailable, continuing without context: {exc}",
              file=sys.stderr)
        return []
    return [h.get("text", "") for h in hits if h.get("text")]


class _MockCodexClient:
    """In-process codex stand-in for ``--mock`` (no subprocess, no path check).

    Mirrors the ``CodexClient.generate(prompt)`` interface: it RECEIVES the built
    prompt (so the RAG context stays observable to a caller that wants to inspect
    it) and returns a canned fenced reply routed through the shared
    ``codex_client.extract_code`` — the same fence handling the real path uses.
    Because it never validates a codex path, ``--mock`` works codex-less by
    construction (no ``CodexNotFound``).
    """

    async def generate(self, prompt: str, *, timeout: float | None = None) -> str:
        return codex_client.extract_code(_MOCK_CODEX_OUTPUT)


def _make_codex_client(cfg: Config, *, mock: bool):
    """Factory seam returning the codex client for the run mode.

    ``mock`` -> a :class:`_MockCodexClient` (canned output, no subprocess, no
    path validation); otherwise the real ``codex_client.CodexClient`` bound to
    the resolved shim path and repo root. Keeping construction behind one
    function lets tests patch the seam wholesale and keeps ``_generate_code``
    free of mode branching.
    """
    if mock:
        return _MockCodexClient()
    return codex_client.CodexClient(cfg.codex_cmd, repo_root=cfg.repo_root)


def _generate_code(
    cfg: Config, instruction: str, context_chunks: list[str], *, mock: bool
) -> str:
    """Build the prompt (shared composer) and return the generated eps code.

    The prompt is always composed via ``codex_client.build_prompt`` and handed to
    whatever client the factory returns; the client's ``generate`` runs the real
    codex subprocess or, under ``--mock``, returns the canned reply (both via the
    shared ``extract_code`` fence handling). The runner is otherwise synchronous,
    so ``generate`` is driven with ``asyncio.run``.
    """
    prompt = codex_client.build_prompt(
        instruction, context_chunks, current_code=None
    )
    client = _make_codex_client(cfg, mock=mock)
    return asyncio.run(client.generate(prompt))


def _write_cmd(inbox: Path, job_id: str, target: str, code: str) -> Path:
    """Write ``inbox\\agent_<id>.cmd`` as ``SET <target>\\n<code>``, UTF-8 no BOM.

    Bytes are written directly (``encode("utf-8")`` + ``write_bytes``, the same
    idiom as ``bridge_io._write_cmd``) so no BOM is emitted and ``\\n`` is never
    translated to ``\\r\\n`` — the bridge's first-line command parser keys off
    ``\\n`` (rules.md "IPC and encoding").
    """
    inbox.mkdir(parents=True, exist_ok=True)
    command_text = f"SET {target}\n{code}"
    path = inbox / f"agent_{job_id}.cmd"
    path.write_bytes(command_text.encode("utf-8"))
    return path


def process_job(
    cfg: Config, jobpath: Path, *, mock: bool, use_context: bool
) -> Path:
    """Process one job file end to end; return the produced ``.cmd`` path.

    Reads the legacy-schema job JSON, runs the (optional) RAG + codex pipeline,
    writes the ``agent_<id>.cmd`` SET command, then consumes the job by renaming
    ``<id>.json`` -> ``<id>.done`` (``os.replace``).
    """
    job_id = jobpath.stem
    job = json.loads(jobpath.read_text(encoding="utf-8"))

    instruction = (job.get("instruction") or "").strip()
    target = (job.get("target") or "").strip()
    # Effective context = the job's own flag AND the runner-level switch.
    want_ctx = use_context and bool(job.get("context", True))
    print(
        f"[job {job_id}] instruction={instruction!r} target={target!r} "
        f"ctx={want_ctx}",
        file=sys.stderr,
    )

    chunks = _retrieve_context(instruction, cfg.rag_db) if want_ctx else []
    code = _generate_code(cfg, instruction, chunks, mock=mock)

    inbox = Path(cfg.data_dir) / "inbox"
    cmd_path = _write_cmd(inbox, job_id, target, code)

    os.replace(jobpath, jobpath.with_suffix(".done"))
    print(
        f"[job {job_id}] applied -> {cmd_path}  (code {len(code)}B)",
        file=sys.stderr,
    )
    return cmd_path


def _drain_queue(cfg: Config, *, mock: bool, use_context: bool) -> None:
    """Process every ``*.json`` currently in ``<data_dir>\\jobs`` once.

    A failing job logs its error and is skipped (it stays a ``.json`` so a later
    pass can retry) rather than aborting the whole drain.
    """
    jobs_dir = Path(cfg.data_dir) / "jobs"
    jobs_dir.mkdir(parents=True, exist_ok=True)
    for name in sorted(os.listdir(jobs_dir)):
        if not name.endswith(".json"):
            continue
        jobpath = jobs_dir / name
        try:
            process_job(cfg, jobpath, mock=mock, use_context=use_context)
        except Exception as exc:  # noqa: BLE001 - one bad job must not abort the drain
            print(f"[job {name}] ERROR: {exc}", file=sys.stderr)


# --------------------------------------------------------------------------- #
# CLI.
# --------------------------------------------------------------------------- #


def _build_parser() -> argparse.ArgumentParser:
    ap = argparse.ArgumentParser(
        prog="eud_agent.runner_cli",
        description="Headless jobs-queue runner (RAG -> codex -> SET via inbox).",
    )
    # --data-dir is the canonical flag; --agent-dir is the legacy alias (same
    # dest) so existing invocations keep working.
    ap.add_argument(
        "--data-dir",
        "--agent-dir",
        dest="data_dir",
        default=None,
        help="editor Data\\agent directory (CLI wins over env/agent.cfg).",
    )
    ap.add_argument(
        "--once",
        action="store_true",
        help="process the current queue once, then exit.",
    )
    ap.add_argument(
        "--mock",
        action="store_true",
        help="fake codex output (no subprocess).",
    )
    ap.add_argument(
        "--no-context",
        action="store_true",
        help="skip RAG context retrieval.",
    )
    return ap


def main(argv: list[str] | None = None) -> int:
    """Entry point: resolve config, then drain (``--once``) or poll the queue.

    Returns 0 on a clean ``--once`` drain. The resident (non-``--once``) loop
    runs until interrupted (KeyboardInterrupt -> 0).
    """
    args = _build_parser().parse_args(argv)

    cli = {"data_dir": args.data_dir} if args.data_dir else None
    cfg = Config.resolve(cli=cli)

    print(
        f"[runner] data_dir={cfg.data_dir} mock={args.mock} once={args.once}",
        file=sys.stderr,
    )

    use_context = not args.no_context
    if args.once:
        _drain_queue(cfg, mock=args.mock, use_context=use_context)
        return 0

    try:
        while True:
            _drain_queue(cfg, mock=args.mock, use_context=use_context)
            time.sleep(_POLL_INTERVAL)
    except KeyboardInterrupt:
        return 0


if __name__ == "__main__":
    raise SystemExit(main())
