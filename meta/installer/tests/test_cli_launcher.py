"""The meta-forge launcher: prepare once, keep the caller's directory, never import from it.

The script runs against a fake .venv/bin/python and a fake start.sh that log
what they receive; one row runs the real interpreter to show that a launchkit
package in the caller's directory is never imported.
"""
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys

import pytest

INSTALLER = Path(__file__).resolve().parents[1]
LAUNCHER = INSTALLER / "meta-forge"
CREDENTIALS = {"OPENAI_API_KEY": "launcher-test-value", "GITHUB_TOKEN": "launcher-test-value",
               "RKAT_GEMINI_API_KEY": "launcher-test-value", "RKAT_COPILOT_GITHUB_TOKEN": "launcher-test-value",
               "RKAT_GH_TOKEN": "launcher-test-value"}

FAKE_PYTHON = """#!{python}
import json, os, sys
with open(os.environ["LAUNCH_LOG"], "a") as log:
    log.write(json.dumps({{"argv": sys.argv[1:], "cwd": os.getcwd(), "pythonpath": os.environ.get("PYTHONPATH"),
        "credentials": sorted(name for name in os.environ if name in {names})}}) + "\\n")
sys.exit(int(os.environ.get("FAKE_EXIT", "0")))
"""

FAKE_START = """#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")" && pwd)"
printf 'start.sh %s\\n' "$*" >> "$LAUNCH_LOG"
for name in OPENAI_API_KEY GITHUB_TOKEN RKAT_GEMINI_API_KEY RKAT_COPILOT_GITHUB_TOKEN RKAT_GH_TOKEN; do
  if [ -n "${!name+x}" ]; then printf 'leaked %s\\n' "$name" >> "$LAUNCH_LOG"; fi
done
echo "preparing on stdout"
if [ -n "${FAIL_PREPARE:-}" ]; then exit 7; fi
mkdir -p "$root/.venv/bin"
cp "$root/fake-python" "$root/.venv/bin/python"
chmod 755 "$root/.venv/bin/python"
"""


@pytest.fixture
def forge_folder(tmp_path):
    root = tmp_path / "Forge folder"
    root.mkdir()
    shutil.copyfile(LAUNCHER, root / "meta-forge")
    (root / "meta-forge").chmod(0o755)
    (root / "fake-python").write_text(FAKE_PYTHON.format(python=sys.executable, names=sorted(CREDENTIALS)))
    (root / "start.sh").write_text(FAKE_START)
    caller = tmp_path / "caller"
    caller.mkdir()
    return root, caller, tmp_path / "launch.log"


def launch(root, caller, log, *args, prepared=True, **extra):
    if prepared:
        (root / ".venv/bin").mkdir(parents=True, exist_ok=True)
        shutil.copyfile(root / "fake-python", root / ".venv/bin/python")
        (root / ".venv/bin/python").chmod(0o755)
    environment = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "LAUNCH_LOG": str(log), **CREDENTIALS, **extra}
    return subprocess.run([str(root / "meta-forge"), *args], cwd=caller, env=environment, capture_output=True,
                          text=True, timeout=60)


def entries(log):
    return [line for line in log.read_text().splitlines()] if log.exists() else []


def test_the_launcher_is_executable_in_the_checkout():
    # Git records the executable bit; the checkout's umask decides the group and other bits.
    assert LAUNCHER.stat().st_mode & 0o100 and os.access(LAUNCHER, os.X_OK)
    assert LAUNCHER.read_text().startswith("#!/usr/bin/env bash\n")


def test_it_runs_the_private_python_without_importing_from_the_caller(forge_folder):
    root, caller, log = forge_folder
    result = launch(root, caller, log, "setup", "list", "--json", PYTHONPATH="/caller/extra")
    assert result.returncode == 0, result.stderr
    (record,) = [json.loads(line) for line in entries(log)]
    assert record["argv"] == ["-B", "-P", "-m", "launchkit.cli", "setup", "list", "--json"]
    assert record["cwd"] == str(caller)
    # The caller's PYTHONPATH is dropped: the command imports only from its own folder and environment.
    assert record["pythonpath"] == str(root)
    # The command itself receives the caller's environment; it never forwards credentials (test_cli_secrets).
    assert record["credentials"] == sorted(CREDENTIALS)


def test_a_missing_environment_is_prepared_by_start_sh_without_credentials(forge_folder):
    root, caller, log = forge_folder
    result = launch(root, caller, log, "version", "--json", prepared=False)
    assert result.returncode == 0, result.stderr
    lines = entries(log)
    assert lines[0] == "start.sh --prepare-only"
    assert not any(line.startswith("leaked") for line in lines)
    assert json.loads(lines[-1])["argv"][-2:] == ["version", "--json"]
    # Preparation messages never mix into the command's JSON on stdout.
    assert result.stdout == "" and "preparing on stdout" in result.stderr
    again = launch(root, caller, log, "version", prepared=False)
    assert again.returncode == 0 and entries(log).count("start.sh --prepare-only") == 1


def test_a_failed_preparation_stops_before_the_command(forge_folder):
    root, caller, log = forge_folder
    result = launch(root, caller, log, "setup", "list", prepared=False, FAIL_PREPARE="1")
    assert result.returncode == 7
    assert entries(log) == ["start.sh --prepare-only"]


def test_the_exit_code_is_the_commands(forge_folder):
    root, caller, log = forge_folder
    assert launch(root, caller, log, "plan", "accept", "last", FAKE_EXIT="84").returncode == 84


def test_a_launchkit_in_the_callers_directory_is_never_imported(tmp_path):
    planted = tmp_path / "caller" / "launchkit" / "cli"
    planted.mkdir(parents=True)
    (planted.parent / "__init__.py").write_text("raise SystemExit('planted launchkit imported')\n")
    (planted / "__init__.py").write_text("raise SystemExit('planted launchkit imported')\n")
    environment = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "PYTHONPATH": str(INSTALLER),
                   "META_FORGE_PORT": "9"}
    result = subprocess.run([sys.executable, "-B", "-P", "-m", "launchkit.cli", "--help"], cwd=planted.parents[1],
                            env=environment, capture_output=True, text=True, timeout=60)
    assert result.returncode == 0, result.stderr
    assert "planted" not in result.stdout + result.stderr and "meta-forge" in result.stdout


@pytest.mark.parametrize("caller_path", [":/nonexistent", ".", "lib", "planted folder"])
def test_the_launcher_never_imports_from_the_callers_directory_or_pythonpath(tmp_path, caller_path):
    """The real launcher and interpreter; a module shadowing the standard library waits in the caller's
    directory, in a folder relative to it and in an absolute folder on the caller's PYTHONPATH."""
    forge = tmp_path / "forge"
    (forge / ".venv" / "bin").mkdir(parents=True)
    shutil.copyfile(LAUNCHER, forge / "meta-forge")
    (forge / "meta-forge").chmod(0o755)
    (forge / "launchkit").symlink_to(INSTALLER / "launchkit")
    python = forge / ".venv" / "bin" / "python"
    python.write_text(f"#!/bin/sh\nexec {shlex.quote(sys.executable)} \"$@\"\n")
    python.chmod(0o755)
    caller, planted = tmp_path / "caller", tmp_path / "planted folder"
    for folder in (caller, caller / "lib", planted):
        folder.mkdir(parents=True, exist_ok=True)
        (folder / "argparse.py").write_text("import sys\nsys.stderr.write('planted module imported\\n')\n"
                                            "raise SystemExit(99)\n")
    value = str(planted) if caller_path == "planted folder" else caller_path
    result = subprocess.run([str(forge / "meta-forge"), "--help"], cwd=caller, capture_output=True, text=True,
                            env={"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": str(tmp_path),
                                 "PYTHONPATH": value}, timeout=60)
    assert result.returncode == 0, result.stderr
    assert "planted" not in result.stdout + result.stderr and "usage: meta-forge" in result.stdout


@pytest.mark.parametrize("layout", ["Forge:lib", "Projects:2026/Forge:lib"])
def test_a_forge_folder_whose_path_holds_a_colon_is_refused(tmp_path, layout):
    """macOS Finder writes a "/" typed in a folder name as ":". PYTHONPATH would split the Forge folder's path at
    each ":" and look the parts after one up in the caller's directory, where a planted launchkit waits. The refusal
    never repeats the path: a folder name may hold anything, control characters included."""
    forge = tmp_path / layout
    (forge / ".venv" / "bin").mkdir(parents=True)
    shutil.copyfile(LAUNCHER, forge / "meta-forge")
    (forge / "meta-forge").chmod(0o755)
    (forge / "launchkit").symlink_to(INSTALLER / "launchkit")
    python = forge / ".venv" / "bin" / "python"
    python.write_text(f"#!/bin/sh\nexec {shlex.quote(sys.executable)} \"$@\"\n")
    python.chmod(0o755)
    caller = tmp_path / "caller"
    for entry in str(forge).split(":")[1:]:
        planted = caller / entry / "launchkit"
        planted.mkdir(parents=True)
        (planted / "__init__.py").write_text("import sys\nsys.stderr.write('planted launchkit imported\\n')\n"
                                             "raise SystemExit(99)\n")
    result = subprocess.run([str(forge / "meta-forge"), "--help"], cwd=caller, capture_output=True, text=True,
                            env={"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": str(tmp_path)}, timeout=60)
    assert result.returncode == 86, (result.returncode, result.stderr)
    output = result.stdout + result.stderr
    assert "planted" not in output and result.stdout == ""
    assert "contains ':'" in result.stderr and "Move or rename the folder" in result.stderr
    for path in (forge, forge.resolve()):
        named = [str(path), *(part for part in path.parts if ":" in part)]
        assert len(named) == 1 + layout.count(":")
        assert not [text for text in named if text in output], (named, result.stderr)
