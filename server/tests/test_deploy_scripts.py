"""Verification artifact for EUD-010-e3af: deployment scripts.

Validates the PowerShell 7 deployment scripts under ``scripts/`` per
hivemind/docs/architecture.md ("Boot and lifecycle" agent.cfg schema,
"Repository layout"), hivemind/docs/tech-stack.md ("Active Dependencies",
uv convention), hivemind/docs/verify.md (e2e step 1, what install enables),
and hivemind/docs/rules.md ("Editor integrity" -- file copies are the only
editor touch; "IPC and encoding" -- agent.cfg is UTF-8 WITHOUT BOM).

The scripts under test:

  - scripts/setup_env.ps1        (uv sync server/.venv + bge-m3 cache warn)
  - scripts/install_dropin.ps1   (copy lua + DLLs, write agent.cfg BOM-free)
  - scripts/check_prereqs.ps1    (shared uv/codex/venv-python checks,
                                  dot-sourced by both installers)
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


def _windows_powershell() -> str:
    """Resolve builtin Windows PowerShell 5.1 (always present on Windows)."""
    found = shutil.which("powershell")
    assert found, "powershell (Windows PowerShell 5.1) not found on PATH"
    return found


def _run_script(
    script: Path, *args: str, timeout: int = 120, env: dict | None = None,
    host: str | None = None,
) -> subprocess.CompletedProcess:
    """Run a deployment script via ``<host> -NoProfile -File`` and capture output.

    ``host`` defaults to pwsh (PowerShell 7); pass ``_windows_powershell()`` to
    prove 5.1 compatibility (the bats fall back to it when pwsh is absent).
    An explicit stdin (DEVNULL) is always given so a console-less invocation
    never hangs waiting on input (mirrors the codex stdin rule structurally).
    ``env`` entries overlay ``os.environ`` (e.g. CODEX_CMD to steer the shared
    prerequisite check deterministically).
    """
    cmd = [host or _pwsh(), "-NoProfile", "-NonInteractive", "-File", str(script), *args]
    merged_env = {**os.environ, **env} if env else None
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
        env=merged_env,
    )


def _run_bat(
    bat: Path, stdin_text: str, timeout: int = 300, env: dict | None = None
) -> subprocess.CompletedProcess:
    """Run a double-clickable .bat via ``cmd /d /c`` feeding its prompts.

    The bats read two lines: the editor path prompt (empty line = the ps1
    default) and the final "Press Enter to close..." pause — both must be in
    ``stdin_text`` or the bat blocks until the timeout.
    """
    merged_env = {**os.environ, **env} if env else None
    return subprocess.run(
        ["cmd.exe", "/d", "/c", str(bat)],
        cwd=str(REPO_ROOT),
        input=stdin_text,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=timeout,
        env=merged_env,
    )


def _hermetic_codex_env() -> dict:
    """Point CODEX_CMD at a file that certainly exists (this interpreter).

    The install scripts' shared prerequisite check honors CODEX_CMD before
    PATH (mirroring config.py), so success-path tests stay deterministic on
    machines without a codex install.
    """
    return {"CODEX_CMD": sys.executable}


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

        proc = _run_script(
            INSTALL_DROPIN, "-EditorPath", str(editor_root),
            env=_hermetic_codex_env(),
        )
        assert proc.returncode == 0, (
            f"install_dropin.ps1 exited {proc.returncode}; output:\n{proc.stdout}"
        )
        _assert_install_products_ok(editor_root)


# --- 3. idempotency: install twice ----------------------------------------


def test_install_dropin_idempotent():
    with tempfile.TemporaryDirectory(prefix="eud_fake_editor_") as tmp:
        editor_root = Path(tmp)
        _make_fake_editor(editor_root)

        first = _run_script(
            INSTALL_DROPIN, "-EditorPath", str(editor_root),
            env=_hermetic_codex_env(),
        )
        assert first.returncode == 0, (
            f"first install exited {first.returncode}; output:\n{first.stdout}"
        )
        _assert_install_products_ok(editor_root)

        second = _run_script(
            INSTALL_DROPIN, "-EditorPath", str(editor_root),
            env=_hermetic_codex_env(),
        )
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

        installed = _run_script(
            INSTALL_DROPIN, "-EditorPath", str(editor_root),
            env=_hermetic_codex_env(),
        )
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


# --- 7. shared prerequisite checks (uv/codex/venv python) ------------------


CHECK_PREREQS = SCRIPTS_DIR / "check_prereqs.ps1"


def test_check_prereqs_is_shared_by_both_installers():
    """Both install-path scripts dot-source the single shared check file."""
    assert CHECK_PREREQS.is_file(), f"missing shared check file: {CHECK_PREREQS}"
    for script in (SETUP_ENV, INSTALL_DROPIN):
        text = script.read_text(encoding="utf-8")
        assert "check_prereqs.ps1" in text, (
            f"{script.name} does not reference the shared check file "
            "(check_prereqs.ps1 must be dot-sourced by both installers)"
        )


def test_install_dropin_fails_on_unresolvable_codex():
    """A CODEX_CMD pointing at a nonexistent file must abort the install
    BEFORE anything is copied (resolve order mirrors config.py: env > PATH)."""
    with tempfile.TemporaryDirectory(prefix="eud_fake_editor_") as tmp:
        editor_root = Path(tmp)
        _make_fake_editor(editor_root)
        bogus = str(Path(tmp) / "nope" / "codex.cmd")

        proc = _run_script(
            INSTALL_DROPIN, "-EditorPath", str(editor_root),
            env={"CODEX_CMD": bogus},
        )
        assert proc.returncode != 0, (
            "install_dropin.ps1 must fail when codex is unresolvable; "
            f"output:\n{proc.stdout}"
        )
        assert "codex" in proc.stdout.lower(), (
            "install_dropin.ps1 must name codex as the missing prerequisite; "
            f"output:\n{proc.stdout}"
        )

        # Nothing must have been copied before the prerequisite failure.
        assert not (editor_root / LUA_DEST_REL).exists(), (
            "install copied the lua despite a failed prerequisite check"
        )
        assert not (editor_root / AGENT_CFG_REL).exists(), (
            "install wrote agent.cfg despite a failed prerequisite check"
        )


def test_setup_env_fails_fast_on_unresolvable_codex():
    """setup_env.ps1 must fail on a bad CODEX_CMD BEFORE running uv sync."""
    with tempfile.TemporaryDirectory(prefix="eud_codex_") as tmp:
        bogus = str(Path(tmp) / "nope" / "codex.cmd")

        proc = _run_script(SETUP_ENV, env={"CODEX_CMD": bogus}, timeout=60)
        assert proc.returncode != 0, (
            "setup_env.ps1 must fail when codex is unresolvable; "
            f"output:\n{proc.stdout}"
        )
        assert "codex" in proc.stdout.lower(), (
            "setup_env.ps1 must name codex as the missing prerequisite; "
            f"output:\n{proc.stdout}"
        )
        assert "uv sync" not in proc.stdout, (
            "setup_env.ps1 must fail fast, before starting uv sync; "
            f"output:\n{proc.stdout}"
        )


# --- 8. double-clickable .bat wrappers --------------------------------------


INSTALL_BAT = SCRIPTS_DIR / "install.bat"
UNINSTALL_BAT = SCRIPTS_DIR / "uninstall.bat"


def test_install_bat_full_install_against_fake_editor():
    """install.bat = setup_env + install_dropin, prompted editor path.

    stdin feeds the editor-path prompt and the final Enter-to-close pause.
    """
    with tempfile.TemporaryDirectory(prefix="eud_fake_editor_") as tmp:
        editor_root = Path(tmp)
        _make_fake_editor(editor_root)

        proc = _run_bat(
            INSTALL_BAT, f"{editor_root}\n\n",
            env=_hermetic_codex_env(), timeout=600,
        )
        assert proc.returncode == 0, (
            f"install.bat exited {proc.returncode}; output:\n{proc.stdout}"
        )
        _assert_install_products_ok(editor_root)


def test_install_bat_propagates_failure_and_still_pauses():
    """A failing step must surface as a non-zero bat exit code, and the final
    Enter-to-close pause must still be reached (the window never auto-closes
    on failure). A bogus CODEX_CMD makes setup_env fail fast."""
    with tempfile.TemporaryDirectory(prefix="eud_codex_") as tmp:
        bogus = str(Path(tmp) / "nope" / "codex.cmd")

        proc = _run_bat(
            INSTALL_BAT, "\n\n", env={"CODEX_CMD": bogus}, timeout=120,
        )
        assert proc.returncode != 0, (
            "install.bat must propagate the setup_env failure as a non-zero "
            f"exit code; output:\n{proc.stdout}"
        )
        assert "Press Enter to close" in proc.stdout, (
            "install.bat must still reach the Enter-to-close pause on "
            f"failure; output:\n{proc.stdout}"
        )


def test_uninstall_bat_against_fake_editor():
    with tempfile.TemporaryDirectory(prefix="eud_fake_editor_") as tmp:
        editor_root = Path(tmp)
        _make_fake_editor(editor_root)

        installed = _run_script(
            INSTALL_DROPIN, "-EditorPath", str(editor_root),
            env=_hermetic_codex_env(),
        )
        assert installed.returncode == 0, (
            f"install (pre-uninstall) exited {installed.returncode}; "
            f"output:\n{installed.stdout}"
        )

        proc = _run_bat(UNINSTALL_BAT, f"{editor_root}\n\n")
        assert proc.returncode == 0, (
            f"uninstall.bat exited {proc.returncode}; output:\n{proc.stdout}"
        )
        assert not (editor_root / LUA_DEST_REL).exists(), (
            "uninstall.bat did not remove the drop-in lua"
        )
        assert not (editor_root / AGENT_CFG_REL).exists(), (
            "uninstall.bat did not remove agent.cfg"
        )
        for dll in WEBVIEW2_DLLS:
            assert (editor_root / dll).is_file(), (
                f"uninstall.bat removed a DLL (default must keep them): {dll}"
            )


# --- 9. Windows PowerShell 5.1 compatibility ---------------------------------
# The bats fall back to builtin powershell.exe when pwsh is absent, so the
# deploy scripts must run under 5.1 too (they declare #Requires -Version 5.1
# and stay ASCII-only: 5.1 reads BOM-less sources as ANSI/CP949).


def test_install_uninstall_under_windows_powershell_51():
    ps51 = _windows_powershell()
    with tempfile.TemporaryDirectory(prefix="eud_fake_editor_") as tmp:
        editor_root = Path(tmp)
        _make_fake_editor(editor_root)

        installed = _run_script(
            INSTALL_DROPIN, "-EditorPath", str(editor_root),
            env=_hermetic_codex_env(), host=ps51,
        )
        assert installed.returncode == 0, (
            f"install under powershell 5.1 exited {installed.returncode}; "
            f"output:\n{installed.stdout}"
        )
        _assert_install_products_ok(editor_root)

        removed = _run_script(
            UNINSTALL_DROPIN, "-EditorPath", str(editor_root), host=ps51,
        )
        assert removed.returncode == 0, (
            f"uninstall under powershell 5.1 exited {removed.returncode}; "
            f"output:\n{removed.stdout}"
        )
        assert not (editor_root / LUA_DEST_REL).exists()
        assert not (editor_root / AGENT_CFG_REL).exists()


def test_setup_env_prereq_check_under_windows_powershell_51():
    """The shared check_prereqs.ps1 must parse and fail fast under 5.1."""
    with tempfile.TemporaryDirectory(prefix="eud_codex_") as tmp:
        bogus = str(Path(tmp) / "nope" / "codex.cmd")
        proc = _run_script(
            SETUP_ENV, env={"CODEX_CMD": bogus}, timeout=60,
            host=_windows_powershell(),
        )
        assert proc.returncode != 0, (
            "setup_env under powershell 5.1 must fail on a bad CODEX_CMD; "
            f"output:\n{proc.stdout}"
        )
        assert "codex" in proc.stdout.lower(), (
            f"failure must name codex; output:\n{proc.stdout}"
        )
        assert "uv sync" not in proc.stdout, (
            f"must fail before uv sync; output:\n{proc.stdout}"
        )


# --- 10. release packaging ----------------------------------------------------


PACKAGE_RELEASE = SCRIPTS_DIR / "package_release.ps1"
README_RELEASE = SCRIPTS_DIR / "README.release.md"


def _make_fake_rag_db(root: Path) -> Path:
    """Minimal chromadb-store shape: a folder holding chroma.sqlite3."""
    rag = root / "chromadb_bge"
    rag.mkdir(parents=True, exist_ok=True)
    (rag / "chroma.sqlite3").write_bytes(b"SQLite format 3\x00 fake")
    return rag


def test_install_dropin_writes_rag_db_when_bundled():
    """-RagDb (or the auto-detected release bundle) lands in agent.cfg as
    'rag_db'; without it the key is omitted (server default applies)."""
    with tempfile.TemporaryDirectory(prefix="eud_fake_editor_") as tmp:
        editor_root = Path(tmp) / "editor"
        editor_root.mkdir()
        _make_fake_editor(editor_root)
        rag = _make_fake_rag_db(Path(tmp))

        proc = _run_script(
            INSTALL_DROPIN, "-EditorPath", str(editor_root),
            "-RagDb", str(rag), env=_hermetic_codex_env(),
        )
        assert proc.returncode == 0, (
            f"install with -RagDb exited {proc.returncode}; output:\n{proc.stdout}"
        )
        cfg = json.loads((editor_root / AGENT_CFG_REL).read_text(encoding="utf-8"))
        assert cfg.get("rag_db") == str(rag), (
            f"agent.cfg rag_db mismatch: {cfg.get('rag_db')!r} != {str(rag)!r}"
        )

        # Re-install WITHOUT -RagDb: no bundled rag\ in the dev repo, so the
        # key must be absent (the server falls back to its built-in default).
        proc2 = _run_script(
            INSTALL_DROPIN, "-EditorPath", str(editor_root),
            env=_hermetic_codex_env(),
        )
        assert proc2.returncode == 0, (
            f"re-install exited {proc2.returncode}; output:\n{proc2.stdout}"
        )
        cfg2 = json.loads((editor_root / AGENT_CFG_REL).read_text(encoding="utf-8"))
        assert "rag_db" not in cfg2, (
            f"agent.cfg must omit rag_db without a bundle; got {cfg2.get('rag_db')!r}"
        )


def test_package_release_zip_structure():
    """package_release.ps1 produces a zip mirroring the repo layout with the
    runtime-minimal set: no tests/.venv/hivemind, bundled rag, root README."""
    import zipfile

    assert PACKAGE_RELEASE.is_file(), f"missing script: {PACKAGE_RELEASE}"
    assert README_RELEASE.is_file(), f"missing README template: {README_RELEASE}"

    dist_index = REPO_ROOT / "panel" / "dist" / "index.html"
    if not dist_index.is_file():
        import pytest

        pytest.skip("panel/dist absent (gitignored); build the panel first")

    with tempfile.TemporaryDirectory(prefix="eud_pkg_") as tmp:
        rag = _make_fake_rag_db(Path(tmp))
        out_dir = Path(tmp) / "out"

        proc = _run_script(
            PACKAGE_RELEASE,
            "-OutDir", str(out_dir),
            "-RagDb", str(rag),
            "-SkipPanelBuild",
            timeout=300,
        )
        assert proc.returncode == 0, (
            f"package_release.ps1 exited {proc.returncode}; output:\n{proc.stdout}"
        )

        zips = list(out_dir.glob("eud-agent-*.zip"))
        assert len(zips) == 1, f"expected exactly one zip, got {zips}"

        names = set(zipfile.ZipFile(zips[0]).namelist())
        required = (
            "eud-agent/README.md",
            "eud-agent/bridge/ZZZ_10_agent_bridge.lua",
            "eud-agent/server/pyproject.toml",
            "eud-agent/server/uv.lock",
            "eud-agent/server/eud_agent/__main__.py",
            "eud-agent/panel/dist/index.html",
            "eud-agent/vendor/webview2/WebView2Loader.dll",
            "eud-agent/rag/chromadb_bge/chroma.sqlite3",
            "eud-agent/scripts/install.bat",
            "eud-agent/scripts/uninstall.bat",
            "eud-agent/scripts/setup_env.ps1",
            "eud-agent/scripts/install_dropin.ps1",
            "eud-agent/scripts/uninstall_dropin.ps1",
            "eud-agent/scripts/check_prereqs.ps1",
        )
        missing = [n for n in required if n not in names]
        assert not missing, f"zip is missing required entries: {missing}"

        leaked = [
            n for n in names
            if "/tests/" in n or "/.venv/" in n or "/hivemind/" in n
            or "__pycache__" in n or "/spikes/" in n or "/node_modules/" in n
        ]
        assert not leaked, f"zip leaked non-runtime files: {leaked[:10]}"


# --- 11. dev_run: launches the resident server (EUD-018 wired the app) -------


def test_dev_run_launches_server():
    """dev_run.ps1 launches the resident server (was the EUD-010 stub).

    EUD-018 wired ``python -m eud_agent`` (no flag) to actually serve, so
    dev_run now starts a long-lived server. We prove it LAUNCHES (writes
    ``server.ready`` into the dev data dir once its own socket accepts) and then
    terminate it — the entry must not hang BEFORE serving. A fresh worktree
    without server/.venv is skipped (the script Fails fast there, by design).
    """
    import time

    assert DEV_RUN.is_file(), f"missing script: {DEV_RUN}"

    venv_python = REPO_ROOT / "server" / ".venv" / "Scripts" / "python.exe"
    if not venv_python.is_file():
        import pytest

        pytest.skip("server/.venv absent (gitignored worktree); run setup_env.ps1")

    with tempfile.TemporaryDirectory() as td:
        data_dir = Path(td) / "devdata"
        data_dir.mkdir(parents=True, exist_ok=True)
        ready_path = data_dir / "server.ready"

        cmd = [
            _pwsh(), "-NoProfile", "-NonInteractive", "-File", str(DEV_RUN),
            "-DataDir", str(data_dir), "-Port", "0",
        ]
        proc = subprocess.Popen(
            cmd,
            cwd=str(REPO_ROOT),
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
        try:
            # server.ready appears once the server's own socket accepts — proof
            # the entry launched the server and did not hang at startup.
            deadline = time.monotonic() + 60.0
            launched = False
            while time.monotonic() < deadline:
                if ready_path.is_file():
                    launched = True
                    break
                if proc.poll() is not None:
                    break  # the process exited early; fall through to diagnose
                time.sleep(0.2)
            assert launched, (
                "dev_run.ps1 never wrote server.ready (server did not launch); "
                f"exit={proc.poll()}"
            )
            ready = json.loads(ready_path.read_text(encoding="utf-8"))
            assert ready["port"] > 0 and ready["pid"] > 0
        finally:
            # pwsh spawns python as a child; kill the whole tree so the resident
            # server (still warming the model) cannot be orphaned (Windows).
            if os.name == "nt":
                subprocess.run(
                    ["taskkill", "/F", "/T", "/PID", str(proc.pid)],
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                    check=False,
                )
            else:  # pragma: no cover - tests run on Windows
                proc.terminate()
            try:
                proc.wait(timeout=15)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=15)


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
