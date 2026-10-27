"""Generated dependency failures and retries use inert executables only."""
import os
from pathlib import Path
import subprocess

import pytest

from launchkit import compiler
from launchkit.blueprint import starter_blueprint
from launchkit.runtime_selection import RuntimeCandidateTarget
from test_runtime_candidates import Bundle


def rendered(tmp_path, *, candidate=False):
    target = None
    if candidate:
        bundle = Bundle(tmp_path / "inert-external-bundle")
        target = RuntimeCandidateTarget(bundle.tuple_path, bundle.root, bundle.binding)
    return {item.path: item.content for item in compiler.generate_package(starter_blueprint(), runtime_target=target)}


def fake_python(path):
    path.write_text('''#!/bin/sh
printf '%s\\0' "$*" >> "$INSTALL_TEST_LOG"
case "$1:${2:-}" in
  -c:*|-m:venv) exit 0 ;;
  -m:pip)
    if [ "$3" = --version ]; then exit 0; fi
    case "$*" in
      *requirements.application.txt) step=application ;;
      *requirements.txt) step=requirements ;;
      *) echo 'Unexpected inert pip invocation' >&2; exit 99 ;;
    esac ;;
  install_candidate.py:sdk-install) step=sdk ;;
  install_candidate.py:requirements-install) step=selected-requirements ;;
  install_gateway.py:*|app.py:*|setup.py:*) exit 0 ;;
  *) echo 'Unexpected inert Python invocation' >&2; exit 99 ;;
esac
printf '%s\n' "$step" >> .venv/partial-dependencies
if [ "$FAIL_DEPENDENCY" = "$step" ]; then
  echo "INERT ORIGINAL DEPENDENCY ERROR: $step" >&2
  exit 42
fi
exit 0
''')
    path.chmod(0o755)


def exported(tmp_path, *, candidate=False, application=False):
    root = tmp_path / "exported app ' dollar$ semicolon;"
    root.mkdir()
    files = rendered(tmp_path, candidate=candidate)
    (root / "install.sh").write_text(files["install.sh"])
    python = tmp_path / "python 'custom' ; dollar$"
    fake_python(python)
    (root / ".venv/bin").mkdir(parents=True)
    fake_python(root / ".venv/bin/python")
    retained = [root / ".venv/healthy-layer", root / ".env", root / ".mobkit/history",
                root / "data/history", root / "workspace/reference"]
    for path in retained:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"Retained synthetic private input\n")
        path.chmod(0o600)
    before = {path: (path.read_bytes(), path.stat().st_ino, path.stat().st_mode) for path in retained}
    if application:
        (root / "requirements.application.txt").write_text("inert-package==1.0\n")
    environment = {"PATH": "/usr/bin:/bin", "PYTHON_BIN": str(python), "INSTALL_TEST_LOG": str(tmp_path / "calls.log"),
        "FAIL_DEPENDENCY": "none", "OPENAI_API_KEY": "PRIVATE_PROVIDER_MUST_NOT_BE_PRINTED"}
    return root, environment, before, files


def retained_unchanged(before):
    assert all((path.read_bytes(), path.stat().st_ino, path.stat().st_mode) == prior for path, prior in before.items())


@pytest.mark.parametrize("step,application,mode", [("requirements", False, ""),
    ("requirements", True, "--non-interactive"), ("application", True, ""),
    ("application", True, "--non-interactive")])
def test_ordinary_dependency_failure_prints_exact_same_folder_retry_and_preserves_state(tmp_path, step, application, mode):
    root, environment, before, _ = exported(tmp_path, application=application)
    environment["FAIL_DEPENDENCY"] = step
    argv = ["/bin/bash", str(root / "install.sh"), *([mode] if mode else [])]
    failed = subprocess.run(argv, cwd=tmp_path, env=environment, capture_output=True, text=True, timeout=10)
    assert failed.returncode == 42
    assert "INERT ORIGINAL DEPENDENCY ERROR: " + step in failed.stderr
    assert "Application dependency preparation did not finish:" in failed.stderr
    assert "Keep .venv, private settings and application history in place" in failed.stderr
    assert "PRIVATE_PROVIDER_MUST_NOT_BE_PRINTED" not in failed.stderr
    assert "Installation complete" not in failed.stdout
    calls = Path(environment["INSTALL_TEST_LOG"]).read_text().rstrip("\0").split("\0")
    assert not any(command.startswith(("install_gateway.py", "app.py", "setup.py")) for command in calls)
    assert "-m venv .venv" not in calls
    retained_unchanged(before)
    assert not (root / ".venv.previous").exists()
    cd = next(line.strip() for line in failed.stderr.splitlines() if line.startswith("  cd -- "))
    retry = next(line.strip() for line in failed.stderr.splitlines() if line.startswith("  PYTHON_BIN="))
    assert retry.endswith(" bash install.sh" + (" --non-interactive" if mode else ""))
    environment["FAIL_DEPENDENCY"] = "none"
    environment["PYTHON_BIN"] = "/inert-wrong-selection"
    recovered = subprocess.run(["/bin/bash", "-c", cd + "\n" + retry], cwd=tmp_path,
        env=environment, capture_output=True, text=True, timeout=10)
    assert recovered.returncode == 0, recovered.stderr
    assert "Installation complete" in recovered.stdout
    calls = Path(environment["INSTALL_TEST_LOG"]).read_text().rstrip("\0").split("\0")
    assert calls.count("install_gateway.py") == 1 and calls.count("app.py --check") == 1
    assert calls.count("setup.py") == (0 if mode else 1)
    assert "-m venv .venv" not in calls
    retained_unchanged(before)


@pytest.mark.parametrize("step", ["sdk", "selected-requirements", "application"])
def test_candidate_failure_keeps_saved_forge_recovery_owner_and_stops_downstream(tmp_path, step):
    root, environment, before, files = exported(tmp_path, candidate=True, application=True)
    environment["FAIL_DEPENDENCY"] = step
    failed = subprocess.run(["/bin/bash", str(root / "install.sh"), "--non-interactive"],
        cwd=tmp_path, env=environment, capture_output=True, text=True, timeout=10)
    assert failed.returncode == 42
    assert "INERT ORIGINAL DEPENDENCY ERROR: " + step in failed.stderr
    assert "accepted external artifact bundle" in failed.stderr
    assert "return to this saved installation in Forge" in failed.stderr
    assert "offered update or recovery controls" in failed.stderr
    assert "Do not run this private candidate install.sh manually" in failed.stderr
    assert "  PYTHON_BIN=" not in failed.stderr and "  cd -- " not in failed.stderr
    assert "PRIVATE_PROVIDER_MUST_NOT_BE_PRINTED" not in failed.stderr
    calls = Path(environment["INSTALL_TEST_LOG"]).read_text().rstrip("\0").split("\0")
    assert not any(command.startswith(("install_gateway.py", "app.py", "setup.py")) for command in calls)
    assert not any("-r requirements.txt" in command for command in calls)
    if step == "sdk":
        assert "install_candidate.py requirements-install" not in calls
    retained_unchanged(before)
    assert "It cannot create a fresh installation" in files["README.md"]
    assert "Do not extract this ZIP into\na new folder or run `bash install.sh`" in files["README.md"]
    assert "use the printed retry command" not in files["README.md"]


@pytest.mark.parametrize("candidate,application", [(False, False), (False, True), (True, False), (True, True)])
def test_generated_dependency_success_keeps_original_command_order_and_optional_selection(tmp_path, candidate, application):
    root, environment, before, files = exported(tmp_path, candidate=candidate, application=application)
    success = subprocess.run(["/bin/bash", str(root / "install.sh"), "--non-interactive"],
        cwd=tmp_path, env=environment, capture_output=True, text=True, timeout=10)
    assert success.returncode == 0, success.stderr
    calls = Path(environment["INSTALL_TEST_LOG"]).read_text().rstrip("\0").split("\0")
    first = (["install_candidate.py sdk-install", "install_candidate.py requirements-install"] if candidate else
        ["-m pip install --disable-pip-version-check -r requirements.txt"])
    if application:
        first += ["-m pip install --disable-pip-version-check -r requirements.application.txt"]
    assert calls[1].startswith("-c from pathlib import Path\n")
    assert "require_project_directory(Path.cwd())" in calls[1]
    assert calls[:1] + calls[2:] == ["-c import sys; sys.exit(0 if sys.version_info >= (3, 11) else \"Python 3.11 or newer is required\")",
        "-c import sys; sys.exit(0 if sys.version_info >= (3, 11) else 1)", "-m pip --version",
        *first, "install_gateway.py", "app.py --check"]
    assert "setup.py" not in calls and "dependency preparation did not finish" not in success.stderr
    assert files["install.sh"].count(".venv/bin/python install_gateway.py") == 1
    retained_unchanged(before)


@pytest.mark.parametrize("changed", ["gateway", "dependency", "candidate-recovery"])
def test_protected_dependency_render_boundary_changes_are_refused(tmp_path, monkeypatch, changed):
    original = compiler.TEMPLATES

    class Templates:
        def __truediv__(self, name):
            if name != "install.sh":
                return original / name
            path = tmp_path / "changed-install.sh"
            source = (original / name).read_text()
            if changed == "gateway":
                source = source.replace(".venv/bin/python install_gateway.py", "changed gateway")
            elif changed == "dependency":
                source = source.replace('prepare_application_dependency_step "installing pinned dependencies"', "changed dependency")
            else:
                source = source.replace("show_dependency_recovery() {", "changed_recovery() {")
            path.write_text(source)
            return path

    monkeypatch.setattr(compiler, "TEMPLATES", Templates())
    with pytest.raises(ValueError, match="protected.*dependency"):
        rendered(tmp_path, candidate=changed == "candidate-recovery")
