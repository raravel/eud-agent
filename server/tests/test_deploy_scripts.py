"""Verification artifact for EUD-010-e3af: deployment scripts.

Validates the PowerShell 7 deployment scripts under ``scripts/`` per
hivemind/docs/architecture.md ("Boot and lifecycle" agent.cfg schema,
"Repository layout"), hivemind/docs/tech-stack.md ("Active Dependencies",
uv convention), hivemind/docs/verify.md (e2e step 1, what install enables),
and hivemind/docs/rules.md ("Editor integrity" -- file copies are the only
editor touch; "IPC and encoding" -- agent.cfg is UTF-8 WITHOUT BOM).

The four scripts under test:

  - scripts/setup_env.ps1        (uv sync server/.venv + bge-m3 cache warn)
  - scripts/install_dropin.ps1   (copy lua + DLLs, write agent.cfg BOM-free)
  - scripts/dev_run.ps1          (run the server standalone for panel work)
  - scripts/uninstall_dropin.ps1 (remove lua + agent.cfg + runtime files)

These tests drive the *real* install/uninstall scripts via ``pwsh`` against a
TEMP fake editor layout, so nothing in the real editor folder is ever touched
(rules.md "Editor integrity"). The fake layout mirrors the real editor's
validation surface: an ``EUD Editor 3.exe`` marker at the root plus a
``Data/Lua/TriggerEditor`` directory (both discovered from the real editor at
implementation time).

This file is pytest-compatible (plain ``test_*`` functions with asserts)
AND standalone-runnable with system Python::

    python server/tests/test_deploy_scripts.py

Only the stdlib + subprocess are used (the project venv may be absent in a
worktree, since server/.venv is gitignored).
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

# repo_root: server/tests/test_deploy_scripts.py -> parents[2] == repo root
REPO_ROOT = Path(__file__).resolve().parents[2]

SCRIPTS_DIR = REPO_ROOT / "scripts"
SETUP_ENV = SCRIPTS_DIR / "setup_env.ps1"
INSTALL_DROPIN = SCRIPTS_DIR / "install_dropin.ps1"
DEV_RUN = SCRIPTS_DIR / "dev_run.ps1"
UNINSTALL_DROPIN = SCRIPTS_DIR / "uninstall_dropin.ps1"

REQUIRED_SCRIPTS = (SETUP_ENV, INSTALL_DROPIN, DEV_RUN, UNINSTALL_DROPIN)

# Editor validation surface (discovered from the real editor folder
# C:\Users\ifthe\proj\eud\EUD.Editor.3.0.19.6.0 at implementation time).
EDITOR_EXE_NAME = "EUD Editor 3.exe"
TRIGGER_EDITOR_REL = Path("Data") / "Lua" / "TriggerEditor"

# Drop-in install destinations relative to the editor root.
LUA_NAME = "ZZZ_10_agent_bridge.lua"
LUA_DEST_REL = TRIGGER_EDITOR_REL / LUA_NAME
AGENT_DIR_REL = Path("Data") / "agent"
AGENT_CFG_REL = AGENT_DIR_REL / "agent.cfg"

# Vendored WebView2 DLLs that install copies next to the editor exe.
WEBVIEW2_DLLS = (
    "Microsoft.Web.WebView2.Core.dll",
    "Microsoft.Web.WebView2.Wpf.dll",
    "WebView2Loader.dll",
)

UTF8_BOM = b"\xef\xbb\xbf"

# Expected default port baked into agent.cfg per architecture.md.
EXPECTED_PORT = 8765


# --- pwsh resolution ------------------------------------------------------


def _pwsh() -> str:
    """Resolve the PowerShell 7 executable, or raise a clear error."""
    found = shutil.which("pwsh")
    assert found, (
        "pwsh (PowerShell 7) not found on PATH; required to run the "
        "deployment scripts"
    )
    return found


def _run_script(
    script: Path, *args: str, timeout: int = 120
) -> subprocess.CompletedProcess:
    """Run a deployment script via ``pwsh -NoProfile -File`` and capture output.

    An explicit stdin (DEVNULL) is always given so a console-less invocation
    never hangs waiting on input (mirrors the codex stdin rule structurally).
    """
    cmd = [_pwsh(), "-NoProfile", "-NonInteractive", "-File", str(script), *args]
    return subprocess.run(
        cmd,
        cwd=str(REPO_ROOT),
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=timeout,
    )


# --- fake editor layout ---------------------------------------------------


def _make_fake_editor(root: Path) -> None:
    """Create a TEMP fake editor layout matching the install validation surface.

    A real editor exposes both an ``EUD Editor 3.exe`` at the root and a
    ``Data\\Lua\\TriggerEditor`` directory; the install script validates on at
    least one of these before copying. We provide both so the install proceeds.
    """
    (root / TRIGGER_EDITOR_REL).mkdir(parents=True, exist_ok=True)
    (root / EDITOR_EXE_NAME).write_bytes(b"MZ fake editor exe marker")


def _assert_install_products_ok(editor_root: Path) -> None:
    """Assert install copied the lua + DLLs and wrote a valid BOM-free agent.cfg."""
    # lua copied
    lua_dest = editor_root / LUA_DEST_REL
    assert lua_dest.is_file(), f"lua not copied to {lua_dest}"
    assert lua_dest.stat().st_size > 0, f"copied lua is empty: {lua_dest}"

    # DLLs copied next to the editor exe
    for dll in WEBVIEW2_DLLS:
        dst = editor_root / dll
        assert dst.is_file(), f"webview2 DLL not copied next to exe: {dst}"
        assert dst.stat().st_size > 0, f"copied DLL is empty: {dst}"

    # agent.cfg exists, BOM-free, valid JSON with the documented schema
    cfg_path = editor_root / AGENT_CFG_REL
    assert cfg_path.is_file(), f"agent.cfg not written: {cfg_path}"

    raw = cfg_path.read_bytes()
    assert not raw.startswith(UTF8_BOM), (
        "agent.cfg starts with a UTF-8 BOM (forbidden; the drop-in lua parses "
        "the first line and a BOM corrupts it)"
    )

    cfg = json.loads(raw.decode("utf-8"))
    assert set(cfg) >= {"python_exe", "repo_root", "port"}, (
        f"agent.cfg missing required keys; got {sorted(cfg)}"
    )

    python_exe = cfg["python_exe"]
    repo_root = cfg["repo_root"]
    port = cfg["port"]

    assert isinstance(python_exe, str) and python_exe, (
        "python_exe must be a non-empty string"
    )
    assert isinstance(repo_root, str) and repo_root, (
        "repo_root must be a non-empty string"
    )
    assert isinstance(port, int) and not isinstance(port, bool), (
        f"port must be an int, got {type(port).__name__}: {port!r}"
    )
    assert port == EXPECTED_PORT, f"port must default to {EXPECTED_PORT}, got {port}"

    assert os.path.isabs(python_exe), f"python_exe must be absolute: {python_exe}"
    assert os.path.isabs(repo_root), f"repo_root must be absolute: {repo_root}"
    assert Path(python_exe).exists(), (
        f"python_exe in agent.cfg does not exist: {python_exe} "
        "(expected server/.venv/Scripts/python.exe)"
    )
    assert Path(repo_root).exists(), (
        f"repo_root in agent.cfg does not exist: {repo_root}"
    )


# --- 1. scripts exist -----------------------------------------------------


def test_deploy_scripts_exist():
    missing = [
        str(p.relative_to(REPO_ROOT)) for p in REQUIRED_SCRIPTS if not p.is_file()
    ]
    assert not missing, f"missing deployment scripts: {missing}"


# --- 2. install against a fake editor -------------------------------------


def test_install_dropin_against_fake_editor():
    with tempfile.TemporaryDirectory(prefix="eud_fake_editor_") as tmp:
        editor_root = Path(tmp)
        _make_fake_editor(editor_root)

        proc = _run_script(INSTALL_DROPIN, "-EditorPath", str(editor_root))
        assert proc.returncode == 0, (
            f"install_dropin.ps1 exited {proc.returncode}; output:\n{proc.stdout}"
        )
        _assert_install_products_ok(editor_root)


# --- 3. idempotency: install twice ----------------------------------------


def test_install_dropin_idempotent():
    with tempfile.TemporaryDirectory(prefix="eud_fake_editor_") as tmp:
        editor_root = Path(tmp)
        _make_fake_editor(editor_root)

        first = _run_script(INSTALL_DROPIN, "-EditorPath", str(editor_root))
        assert first.returncode == 0, (
            f"first install exited {first.returncode}; output:\n{first.stdout}"
        )
        _assert_install_products_ok(editor_root)

        second = _run_script(INSTALL_DROPIN, "-EditorPath", str(editor_root))
        assert second.returncode == 0, (
            f"second (idempotent) install exited {second.returncode}; "
            f"output:\n{second.stdout}"
        )
        _assert_install_products_ok(editor_root)


# --- 4. wrong editor path refuses + copies nothing ------------------------


def test_install_dropin_refuses_wrong_editor_path():
    with tempfile.TemporaryDirectory(prefix="eud_not_editor_") as tmp:
        not_editor = Path(tmp)  # empty: no exe, no Data\Lua\TriggerEditor

        proc = _run_script(INSTALL_DROPIN, "-EditorPath", str(not_editor))
        assert proc.returncode != 0, (
            "install_dropin.ps1 must refuse a non-editor path with a non-zero "
            f"exit; output:\n{proc.stdout}"
        )
        out = proc.stdout.lower()
        assert "editor" in out, (
            "install_dropin.ps1 must print a clear error mentioning the editor "
            f"path; output:\n{proc.stdout}"
        )

        # Nothing must have been copied into the wrong directory.
        leaked = [p.name for p in not_editor.rglob("*") if p.is_file()]
        assert not leaked, (
            f"install_dropin.ps1 copied files into a non-editor path: {leaked}"
        )


# --- 5. uninstall removes lua + agent.cfg, keeps DLLs ---------------------


def test_uninstall_dropin_removes_lua_and_cfg_keeps_dlls():
    with tempfile.TemporaryDirectory(prefix="eud_fake_editor_") as tmp:
        editor_root = Path(tmp)
        _make_fake_editor(editor_root)

        installed = _run_script(INSTALL_DROPIN, "-EditorPath", str(editor_root))
        assert installed.returncode == 0, (
            f"install (pre-uninstall) exited {installed.returncode}; "
            f"output:\n{installed.stdout}"
        )
        _assert_install_products_ok(editor_root)

        proc = _run_script(UNINSTALL_DROPIN, "-EditorPath", str(editor_root))
        assert proc.returncode == 0, (
            f"uninstall_dropin.ps1 exited {proc.returncode}; output:\n{proc.stdout}"
        )

        # lua + agent.cfg gone
        assert not (editor_root / LUA_DEST_REL).exists(), (
            "uninstall did not remove the drop-in lua"
        )
        assert not (editor_root / AGENT_CFG_REL).exists(), (
            "uninstall did not remove agent.cfg"
        )

        # DLLs still present (default: leave DLLs unless -RemoveDlls)
        for dll in WEBVIEW2_DLLS:
            assert (editor_root / dll).is_file(), (
                f"uninstall removed a DLL without -RemoveDlls: {dll}"
            )


# --- 6. setup_env: script + uv resolvable (venv-product when present) -----


def test_setup_env_exists_and_venv_product():
    assert SETUP_ENV.is_file(), f"missing script: {SETUP_ENV}"

    # uv must be resolvable (the venv toolchain per tech-stack.md).
    assert shutil.which("uv"), (
        "uv not found on PATH; setup_env.ps1 relies on 'uv sync' for the venv"
    )

    venv_python = REPO_ROOT / "server" / ".venv" / "Scripts" / "python.exe"
    if venv_python.is_file():
        assert venv_python.stat().st_size > 0, (
            f"venv python is empty: {venv_python}"
        )
    else:
        # A fresh worktree has no server/.venv (gitignored). Skip the
        # venv-product assertion with a printed note; this passes on the main
        # repo where the venv has been synced.
        print(
            "NOTE: server/.venv/Scripts/python.exe absent (gitignored worktree); "
            "skipping venv-product assertion -- run setup_env.ps1 to create it"
        )


# --- 7. dev_run: surfaces the 'app not implemented' state without hanging --


def test_dev_run_surfaces_not_implemented_without_hanging():
    assert DEV_RUN.is_file(), f"missing script: {DEV_RUN}"

    # The FastAPI app module does not exist yet: the entry prints
    # "server app not implemented yet" and exits non-zero. dev_run must
    # surface that honestly (not mask it) and must not hang. A 60s timeout
    # guards against a hang; on timeout we fail rather than leak a process.
    try:
        proc = _run_script(DEV_RUN, timeout=60)
    except subprocess.TimeoutExpired as exc:
        raise AssertionError(
            "dev_run.ps1 hung (>60s): it must surface the server entry's "
            f"exit immediately, not block. captured:\n{exc.output or ''}"
        ) from exc

    # Either the venv is missing (worktree) or the app is not implemented yet:
    # in both honest cases dev_run exits non-zero. It must NOT exit 0 while the
    # app entry is unimplemented.
    assert proc.returncode != 0, (
        "dev_run.ps1 exited 0 while the server app is not implemented yet; "
        f"it must surface the non-zero state. output:\n{proc.stdout}"
    )


# --- standalone runner ----------------------------------------------------


def _all_test_functions():
    module = sys.modules[__name__]
    return [
        (name, obj)
        for name, obj in sorted(vars(module).items())
        if name.startswith("test_") and callable(obj)
    ]


def main() -> int:
    failures = 0
    tests = _all_test_functions()
    for name, fn in tests:
        try:
            fn()
        except AssertionError as exc:
            failures += 1
            print(f"FAIL {name}: {exc}")
        except Exception as exc:  # unexpected (e.g. timeout, missing tool)
            failures += 1
            print(f"ERROR {name}: {type(exc).__name__}: {exc}")
        else:
            print(f"PASS {name}")
    total = len(tests)
    print(f"\n{total - failures}/{total} checks passed")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
