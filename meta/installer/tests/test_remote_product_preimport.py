"""Actual stdlib child rejects a poisoned frozen module before importing it.

Only a Python stdlib bootstrap is executed in these controls. The guest source,
SDK and native fixture bytes remain poisoned and cannot be admitted. This module
intentionally does not import the source-fixture blanket subprocess prohibition:
its one explicit child is needed to distinguish a pre-import fence from a check
that runs after the unwanted module already executed.
"""
import json
import os
from pathlib import Path
import subprocess
import sys

import pytest

from launchkit import remote_product_control as control
from launchkit.templates import install_candidate

# Retain actual interpreter/helper facts before the outer application fixtures
# select their inert Linux destination and application-only SDK location.
_ACTUAL_SDK_LOCATION = install_candidate._sdk_location
_HOST_PLATFORM = sys.platform
from test_remote_product_control import control_destination
from test_destination_application_inputs import inputs_destination
from test_destination_application_factory import destination
from test_product_destination_install import installation, destination_source, artifact_custody, product
from test_product_destination_owner import no_destination_execution


@pytest.mark.parametrize("mode,name", [("dependencies", "remote_product_control.py"), ("owner", "deployment.py")])
async def test_stdlib_child_refuses_drift_before_poisoned_control_import(control_destination, mode, name):
    f = control_destination
    marker = f.remote_folder / "unwanted-import.txt"
    module = f.remote_folder / "control-host/launchkit" / name
    module.write_text("from pathlib import Path\nPath(" + repr(str(marker)) + ").write_text('unwanted module executed')\n"
        + "raise RuntimeError('poisoned module executed before verification')\n")
    expected = control._json({"owner": f.remote_owner, "manifest": f.control_bundle.manifest,
        "worker": json.loads((f.remote_folder / "operation.json").read_bytes())["worker"]}).decode()
    arguments = [sys.executable, "-I", "-S", "-B", "-c", control.CONTROL_CHILD_ENTRY,
        str(f.selected.destination), expected, f.remote_operation, mode]
    if mode == "owner":
        arguments.append("-1")  # Never read: this child refuses source before imports.
    outcome = subprocess.run(arguments, check=False, capture_output=True, timeout=10,
        env={"PATH": os.environ.get("PATH", ""), "LC_ALL": "C"})
    assert outcome.returncode != 0
    assert b"The original trusted control input changed before imports." in outcome.stderr
    assert b"poisoned module executed before verification" not in outcome.stderr
    assert not marker.exists()
    assert not (f.selected.destination / ".env").exists()
    assert not (f.remote_folder / "product-install-completed.json").exists()


async def test_owner_uses_authenticated_adapter_literal_after_materialized_source_changes(control_destination, monkeypatch):
    f = control_destination
    marker = f.remote_folder / "adapter-executed.txt"
    module = f.remote_folder / "control-host/launchkit/remote_product_control.py"
    original = control._control_materialized_files
    verified = []

    def drift_after_verification(root, manifest):
        result = original(root, manifest)
        verified.append(result)
        if len(verified) == 1:
            module.write_text("from pathlib import Path\nPath(" + repr(str(marker)) + ").write_text('unwanted exec')\n"
                + "raise RuntimeError('unwanted materialized adapter exec')\n")
        return result

    monkeypatch.setattr(control, "_control_materialized_files", drift_after_verification)
    from launchkit import remote_destination as controller
    with controller._input_descriptor(control._json(f.settings), f.selected.destination) as descriptor:
        with pytest.raises(ValueError, match="original trusted control input changed before actual Install"):
            await control.run_initial_install(f.selected.destination, f.remote_operation, os.dup(descriptor))
    assert len(verified) == 1
    assert not marker.exists() and not (f.selected.destination / ".env").exists()
    assert not (f.remote_folder / "product-install-completed.json").exists()
    assert not getattr(f, "input_owner", None)


def accepted_inert_pip(marker, control_path):
    """Explicit inert bootstrap input; no installed pip/SDK payload is selected."""
    import hashlib
    import io
    import zipfile
    output = io.BytesIO()
    body = ("from pathlib import Path\nimport sys\n"
        + "def main(arguments):\n"
        + "    assert sys.flags.no_site == 1\n"
        + "    assert sys.prefix == " + repr(str(control_path / ".venv")) + "\n"
        + "    assert {'--isolated','--no-index','--no-deps','--no-compile','--require-hashes','--only-binary=:all:'} <= set(arguments)\n"
        + "    Path(" + repr(str(marker)) + ").write_text('accepted inert pip executed')\n"
        + "    return 0\n")
    with zipfile.ZipFile(output, "w") as archive:
        for name in ("pip/__init__.py", "pip/_internal/__init__.py", "pip/_internal/cli/__init__.py"):
            archive.writestr(name, b"")
        archive.writestr("pip/_internal/cli/main.py", body.encode())
    data = output.getvalue()
    return data, {"name": "pip-inert-py3-none-any.whl", "size": len(data), "sha256": hashlib.sha256(data).hexdigest()}


async def test_actual_dependency_helper_nested_pip_never_executes_site_startup(control_destination, monkeypatch):
    f = control_destination
    monkeypatch.setattr(install_candidate, "_sdk_location", _ACTUAL_SDK_LOCATION)
    control_path = f.remote_folder / "control-host"
    venv = control_path / ".venv"
    site = venv / ("lib/python" + str(sys.version_info.major) + "." + str(sys.version_info.minor) + "/site-packages")
    site.mkdir(mode=0o700, parents=True)
    (venv / "bin").mkdir(mode=0o700)
    (venv / "bin/python").symlink_to(Path(sys.executable).resolve())
    (venv / "pyvenv.cfg").write_text("home = " + str(Path(sys.executable).resolve().parent)
        + "\ninclude-system-site-packages = false\nversion = " + ".".join(map(str, sys.version_info[:3])) + "\n")
    pth_marker, site_marker = f.remote_folder / "pth-executed.txt", f.remote_folder / "site-executed.txt"
    (site / "unwanted.pth").write_text("import pathlib; pathlib.Path(" + repr(str(pth_marker)) + ").write_text('unwanted site startup')\n")
    (site / "sitecustomize.py").write_text("from pathlib import Path\nPath(" + repr(str(site_marker)) + ").write_text('unwanted sitecustomize')\n")
    pip_marker = f.remote_folder / "accepted-pip.txt"
    bootstrap = accepted_inert_pip(pip_marker, control_path)
    dependencies, files = f.control_bundle.manifest["dependencies"], f.control_bundle.manifest["files"]
    refs = {name: {"path": str(control_path / name), **files[name]}
        for name in (dependencies["requirements"], *dependencies["wheels"])}
    # Actual typed artifact readers, held-byte staging and execute_command seam
    # run. The accepted pip body is inert, so installed readback must refuse.
    with pytest.raises(FileNotFoundError):
        control.install_control_dependencies(control_path, refs, site_packages=site, pip_bootstrap=bootstrap)
    assert pip_marker.read_text() == "accepted inert pip executed"
    assert not pth_marker.exists() and not site_marker.exists()
    assert not (f.selected.destination / ".env").exists()
    assert not (f.remote_folder / "product-install-completed.json").exists()


async def test_verified_dependency_dispatch_selects_real_ensurepip_only_after_created_venv_prefix(control_destination, monkeypatch):
    # This executes the actual verified child dispatch tail in the test process,
    # after the source/ABI/birth fence covered by other controls. The canonical
    # ensurepip body remains real; only its distro guard is supplied explicitly.
    # No pip child or selected dependency installation may run in this control.
    import ensurepip
    import sysconfig
    f = control_destination
    monkeypatch.setattr(sys, "platform", _HOST_PLATFORM)
    control_path = f.remote_folder / "control-host"
    venv = control_path / ".venv"
    venv.mkdir(mode=0o700)
    (venv / "pyvenv.cfg").write_text("inert original created-venv specimen\n")
    original_bootstrap, original_run = ensurepip.bootstrap, ensurepip._run_pip
    environment, path = dict(os.environ), list(sys.path)
    prefixes, selected = [], []

    def distro_guard(*args, **kwargs):
        # Delegate to the real stdlib canonical selector. A system-prefix call
        # must fail before its _run_pip hook, just as Debian/Ubuntu requires.
        if sys.prefix == sys.base_prefix:
            raise SystemExit("inert distro system-interpreter ensurepip refusal")
        prefixes.append((sys.prefix, sys.exec_prefix))
        assert sys.prefix == str(venv) and (venv / "pyvenv.cfg").is_file()
        return original_bootstrap(*args, **kwargs)

    def receive_original_selection(actual_control, refs, *, site_packages, pip_bootstrap):
        assert actual_control == control_path and site_packages.is_relative_to(venv)
        assert set(refs) == {f.control_bundle.manifest["dependencies"]["requirements"],
            *f.control_bundle.manifest["dependencies"]["wheels"]}
        data, pin = pip_bootstrap
        assert len(data) == pin["size"] and len(data) > 0
        selected.append(pip_bootstrap)

    monkeypatch.setattr(ensurepip, "bootstrap", distro_guard)
    monkeypatch.setattr(control, "install_control_dependencies", receive_original_selection)
    monkeypatch.setattr(sys, "prefix", sys.base_prefix)
    monkeypatch.setattr(sys, "exec_prefix", sys.base_exec_prefix)
    with pytest.raises(SystemExit, match="distro system-interpreter"):
        control._stdlib_pip_bootstrap()
    assert ensurepip._run_pip is original_run and dict(os.environ) == environment
    # Execute the exact existing dispatch source. It sets the real venv scheme
    # and prefix before calling the canonical selector, with no control-root
    # insertion into stdlib lookup. The original installer callback is inert.
    tail = control.CONTROL_CHILD_ENTRY[control.CONTROL_CHILD_ENTRY.index("venv=control/'.venv'"):]
    namespace = {"control": control_path, "mode": "dependencies", "manifest": f.control_bundle.manifest,
        "adapter": vars(control), "sys": sys, "sysconfig": sysconfig, "Path": Path}
    exec(compile(tail, "/__forge_frozen__/actual-control-dependency-dispatch.py", "exec"), namespace)
    assert prefixes == [(str(venv), str(venv))] and len(selected) == 1
    assert ensurepip._run_pip is original_run and dict(os.environ) == environment
    assert sys.path == path and str(control_path) not in sys.path
    assert not (f.selected.destination / ".env").exists()
    assert not (f.remote_folder / "product-install-completed.json").exists()


INERT_PIP = (b"inert pip bootstrap, never executed", {"name": "pip-inert-py3-none-any.whl", "size": 1,
    "sha256": "0" * 64})


def inert_control_installer(monkeypatch):
    """Record the site the installer would receive; nothing is installed or executed."""
    calls = []
    def installer(*args, **kwargs):
        calls.append(kwargs.get("site_packages"))
        raise AssertionError("inert installer: no installation may start")
    monkeypatch.setattr(install_candidate, "install_selected_dependency_wheels", installer)
    monkeypatch.setattr(control, "_control_dependency_executor",
        lambda control_path, bootstrap, current: lambda *args, **kwargs: pytest.fail("Inert executor ran."))
    return calls


def control_dependency_refs(control_path):
    # The refusal comes first: these pins are never opened.
    return {"dependencies/requirements.txt": {"path": str(control_path / "dependencies/requirements.txt"),
        "size": 1, "sha256": "0" * 64}}


@pytest.mark.parametrize("site", ["missing", "none", "default-scheme", "outside", "venv-root", "parent-escape"])
def test_control_dependencies_install_only_into_the_named_control_venv(tmp_path, monkeypatch, site):
    # The interpreter's default scheme can name the system site (Debian's
    # dist-packages), so the control site has no default: only a site inside
    # the created control venv is a target, refused before any installation.
    import sysconfig
    calls = inert_control_installer(monkeypatch)
    control_path = tmp_path / "control-host"
    venv = control_path / ".venv"
    refs = control_dependency_refs(control_path)
    selected = {"none": None, "default-scheme": Path(sysconfig.get_path("purelib")),
        "outside": tmp_path / "elsewhere/site-packages", "venv-root": venv,
        "parent-escape": venv / "lib/../../elsewhere/site-packages"}
    if site == "missing":
        with pytest.raises(TypeError, match="site_packages"):
            control.install_control_dependencies(control_path, refs, pip_bootstrap=INERT_PIP)
    else:
        with pytest.raises(ValueError, match="only into the created control environment"):
            control.install_control_dependencies(control_path, refs, site_packages=selected[site],
                pip_bootstrap=INERT_PIP)
    assert calls == []


def test_control_dependencies_keep_the_named_control_venv_site(tmp_path, monkeypatch):
    calls = inert_control_installer(monkeypatch)
    control_path = tmp_path / "control-host"
    site = control_path / ".venv/lib/python3.12/site-packages"
    with pytest.raises(AssertionError, match="inert installer"):
        control.install_control_dependencies(control_path, control_dependency_refs(control_path),
            site_packages=site, pip_bootstrap=INERT_PIP)
    assert calls == [site]
