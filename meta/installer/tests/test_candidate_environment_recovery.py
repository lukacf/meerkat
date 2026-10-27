"""Private candidate environment errors use inert Python executables only."""
from pathlib import Path
import subprocess

import pytest

from launchkit import compiler
from launchkit.blueprint import starter_blueprint
from launchkit.runtime_selection import RuntimeCandidateTarget
from test_runtime_candidates import Bundle


def rendered(tmp_path, *, candidate=True):
    target = None
    external = None
    if candidate:
        bundle = Bundle(tmp_path / "inert-external-bundle")
        external = bundle.root
        target = RuntimeCandidateTarget(bundle.tuple_path, bundle.root, bundle.binding)
    files = {item.path: item.content for item in compiler.generate_package(
        starter_blueprint(), runtime_target=target)}
    return files, external


def snapshot(paths):
    return {path: (path.read_bytes(), path.stat().st_ino, path.stat().st_mode)
        for path in paths if path.is_file()}


def fake_python(path, kind):
    path.write_text('''#!/bin/sh
printf '%s\\0' "$*" >> "$ENVIRONMENT_TEST_LOG"
case "$1:${2:-}" in
  -c:*)
    if [ "$INERT_PYTHON_KIND" = old-host ]; then
      echo 'INERT ORIGINAL HOST VERSION ERROR' >&2
      exit 43
    fi
''' + ('''    exit 44 ;;
''' if kind == "old-venv" else '''    exit 0 ;;
''') + '''  -m:venv)
    /bin/mkdir -p "$3/bin"
    printf '%s\n' 'Keep newly partial synthetic environment' > "$3/partial-marker"
    echo 'INERT ORIGINAL ENSUREPIP ERROR' >&2
    exit 42 ;;
  -m:pip)
''' + ('''    echo 'INERT ORIGINAL MISSING PIP ERROR' >&2
    exit 45 ;;
''' if kind == "missing-pip" else '''    if [ "$3" = --version ]; then exit 0; fi
    echo 'Unexpected dependency mutation' >&2
    exit 98 ;;
''') + '''  *) echo 'Unexpected downstream inert Python invocation' >&2; exit 99 ;;
esac
''')
    path.chmod(0o755)


def exported(tmp_path, failure):
    root = tmp_path / "saved project ' dollar$ semicolon;"
    root.mkdir()
    files, external = rendered(tmp_path)
    for name in ("install.sh", "start.sh"):
        (root / name).write_text(files[name])
    host = tmp_path / "selected python ' dollar$"
    fake_python(host, "ready")
    environment = {"PATH": "/usr/bin:/bin", "PYTHON_BIN": str(host),
        "INERT_PYTHON_KIND": failure, "ENVIRONMENT_TEST_LOG": str(tmp_path / "calls.log"),
        "OPENAI_API_KEY": "PRIVATE_PROVIDER_MUST_NOT_BE_PRINTED"}
    if failure == "missing-host":
        environment["PYTHON_BIN"] = str(tmp_path / "absent selected python")
    if failure != "create-venv":
        (root / ".venv/bin").mkdir(parents=True)
        (root / ".venv/retained-layer").write_bytes(b"Retained incomplete or healthy environment\n")
        if failure != "partial-venv":
            fake_python(root / ".venv/bin/python", failure)
    retained = [root / ".env", root / ".mobkit/history", root / "data/history",
        root / "workspace/reference"]
    for path in retained:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"Retained synthetic private input\n")
        path.chmod(0o600)
    before = snapshot([*retained, *root.glob(".venv/**/*"), *external.rglob("*")])
    venv_inode = (root / ".venv").stat().st_ino if (root / ".venv").exists() else None
    return root, environment, before, venv_inode, files


def assert_saved_recovery(stderr):
    assert "Keep this installed project folder, .venv, private settings, agent history" in stderr
    assert "accepted external artifact bundle in place" in stderr
    assert "return to this saved installation in Forge" in stderr
    assert "offered update or recovery controls" in stderr
    assert "Do not run this private candidate install.sh manually" in stderr
    assert "bash install.sh" not in stderr and "mv .venv" not in stderr
    assert "unused backup name" not in stderr and "  PYTHON_BIN=" not in stderr
    assert "PRIVATE_PROVIDER_MUST_NOT_BE_PRINTED" not in stderr


@pytest.mark.parametrize("failure", ["missing-host", "old-host", "create-venv",
    "partial-venv", "old-venv", "missing-pip"])
@pytest.mark.parametrize("mode", ["", "--non-interactive"])
def test_candidate_environment_failure_keeps_saved_owner_and_stops_before_mutation(tmp_path, failure, mode):
    root, environment, before, venv_inode, files = exported(tmp_path, failure)
    result = subprocess.run(["/bin/bash", str(root / "install.sh"), *([mode] if mode else [])],
        cwd=tmp_path, env=environment, capture_output=True, text=True, timeout=10)
    assert result.returncode == (43 if failure == "old-host" else 1)
    assert_saved_recovery(result.stderr)
    assert "Installation complete" not in result.stdout
    assert snapshot(before) == before
    if venv_inode is not None:
        assert (root / ".venv").stat().st_ino == venv_inode
    assert not (root / ".venv.previous").exists()
    log = Path(environment["ENVIRONMENT_TEST_LOG"])
    calls = log.read_text().rstrip("\0").split("\0") if log.exists() else []
    version_check = '-c import sys; sys.exit(0 if sys.version_info >= (3, 11) else "Python 3.11 or newer is required")'
    venv_check = '-c import sys; sys.exit(0 if sys.version_info >= (3, 11) else 1)'
    expected = [] if failure == "missing-host" else [version_check]
    if failure == "create-venv":
        expected += ["-m venv .venv"]
        assert "INERT ORIGINAL ENSUREPIP ERROR" in result.stderr
        assert (root / ".venv/partial-marker").read_text().strip() == "Keep newly partial synthetic environment"
    elif failure in ("old-venv", "missing-pip"):
        expected += [venv_check]
        if failure == "missing-pip":
            expected += ["-m pip --version"]
            assert "Pip is unavailable in this application's .venv." in result.stderr
            assert "INERT ORIGINAL MISSING PIP ERROR" not in result.stderr
    elif failure == "old-host":
        assert "INERT ORIGINAL HOST VERSION ERROR" in result.stderr
    if failure not in {"missing-host", "old-host"}:
        directory_check = calls.pop(1)
        assert directory_check.startswith("-c from pathlib import Path\n")
        assert "require_project_directory(Path.cwd())" in directory_check
    assert calls == expected
    assert "It cannot create a fresh installation" in files["README.md"]
    assert "Do not extract this ZIP into\na new folder or run `bash install.sh`" in files["README.md"]


@pytest.mark.parametrize("missing", ["absent", "dangling-link"])
def test_candidate_start_without_interpreter_preserves_saved_state_and_never_calls_python(tmp_path, missing):
    root, environment, before, venv_inode, _ = exported(tmp_path, "partial-venv")
    if missing == "dangling-link":
        (root / ".venv/bin/python").symlink_to(root / "absent-python")
    result = subprocess.run(["/bin/bash", str(root / "start.sh"), "--doctor"],
        cwd=tmp_path, env=environment, capture_output=True, text=True, timeout=10)
    assert result.returncode == 1 and "interpreter is unavailable" in result.stderr
    assert_saved_recovery(result.stderr)
    assert snapshot(before) == before and (root / ".venv").stat().st_ino == venv_inode
    assert not Path(environment["ENVIRONMENT_TEST_LOG"]).exists()
    if missing == "dangling-link":
        assert (root / ".venv/bin/python").is_symlink()


@pytest.mark.parametrize("boundary", ["selected-host", "version-check", "environment-function",
    "environment-end", "startup"])
def test_changed_candidate_environment_render_boundaries_are_refused(tmp_path, monkeypatch, boundary):
    original = compiler.TEMPLATES

    class Templates:
        def __truediv__(self, name):
            if name != ("start.sh" if boundary == "startup" else "install.sh"):
                return original / name
            text = (original / name).read_text()
            anchors = {"selected-host": "Install Python 3.10 or newer, then run this installer again.",
                "version-check": "sys.version_info >= (3, 10)",
                "environment-function": "show_environment_recovery() {",
                "environment-end": 'install_mode="${1:-}"', "startup": "Run bash install.sh first."}
            path = tmp_path / name
            path.write_text(text.replace(anchors[boundary], "Changed protected input"))
            return path

    monkeypatch.setattr(compiler, "TEMPLATES", Templates())
    with pytest.raises(ValueError, match="protected candidate .*recovery boundary"):
        rendered(tmp_path)
