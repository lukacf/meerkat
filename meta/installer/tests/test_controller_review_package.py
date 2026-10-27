"""Exercise the real protected package closure, without native activation."""
import hashlib
import importlib.abc
import importlib.util
import io
import sys
from types import ModuleType
import zipfile

import pytest

from launchkit.blueprint import starter_blueprint
from launchkit.compiler import TEMPLATES_V2, build_archive, generate_package
from launchkit.implementation_workspace import ImplementationWorkspace, WorkspaceError


REVIEW_FILES = ("native_controller_review.py", "controller_review.mjs", "controller_review.css")
NATIVE_PACKAGES = ("meerkat_toolkit", "meerkat_mobkit")
PREFIX = "/console/forge/assets/"


def test_review_source_closure_is_exact_inside_normal_zip_manifest():
    spec = starter_blueprint()
    files = {item.path: item for item in generate_package(spec)}
    for name in REVIEW_FILES:
        assert name in files, f"The generated package is missing {name}"
        assert files[name].as_bytes() == (TEMPLATES_V2 / name).read_bytes()
        assert files[name].mode == 0o644
    assert "connect_installer_caller.py" not in files
    assert "connect_policy_input.py" not in files
    with zipfile.ZipFile(io.BytesIO(build_archive(spec))) as archive:
        rows = archive.read(f"{spec.project.slug}/SHA256SUMS").decode().splitlines()
        checksums = dict(line.split("  ", 1)[::-1] for line in rows)
        assert len(checksums) == len(rows)
        for name in REVIEW_FILES:
            data = archive.read(f"{spec.project.slug}/{name}")
            assert data == (TEMPLATES_V2 / name).read_bytes()
            assert checksums[name] == hashlib.sha256(data).hexdigest()
            assert archive.getinfo(f"{spec.project.slug}/{name}").external_attr >> 16 & 0o777 == 0o644


class DenyNativeImport(importlib.abc.MetaPathFinder):
    def find_spec(self, fullname, path=None, target=None):
        if fullname.split(".", 1)[0] in NATIVE_PACKAGES:
            pytest.fail("Pure extracted presentation must not import a native SDK or activate review.")


def test_extracted_optional_review_assets_compose_while_default_remains_dormant(tmp_path, monkeypatch):
    spec = starter_blueprint()
    with zipfile.ZipFile(io.BytesIO(build_archive(spec))) as archive:
        archive.extractall(tmp_path)
    package = tmp_path / spec.project.slug
    # A maintained full-suite process can already have SDK/review imports from
    # unrelated tests. Isolate this pure extraction check, then let monkeypatch
    # restore those exact prior modules at teardown. Fresh imports still refuse.
    for name in tuple(sys.modules):
        if name.split(".", 1)[0] in NATIVE_PACKAGES or name == "native_controller_review":
            monkeypatch.delitem(sys.modules, name)
    assert not any(name.split(".", 1)[0] in NATIVE_PACKAGES for name in sys.modules)
    monkeypatch.setattr(sys, "meta_path", [DenyNativeImport(), *sys.meta_path])
    monkeypatch.syspath_prepend(str(package))
    # Load the actual package composition helper and its actual extracted
    # sibling. Neither checkout modules nor a dummy asset map prove closure.
    shell_loader = importlib.util.spec_from_file_location("console_shell", package / "console_shell.py")
    shell = importlib.util.module_from_spec(shell_loader)
    monkeypatch.setitem(sys.modules, "console_shell", shell)
    shell_loader.loader.exec_module(shell)
    loader = importlib.util.spec_from_file_location("extracted_review_presentation", package / "console_presentation.py")
    presentation = importlib.util.module_from_spec(loader)
    loader.loader.exec_module(presentation)
    native = (b'<!doctype html>\n<html><head>\n'
        b'<meta name="mobkit-base-url" content="">\n'
        b'<link rel="stylesheet" href="/console/assets/console-app.css?v=original">\n'
        b'<script src="/console/assets/console-app.js?v=original" defer></script>\n'
        b'</head><body>\n<div id="root"></div>\n</body></html>\n')
    optional_assets, optional_transform = presentation.load_console_presentation(package, controller_review=True)
    assert set(optional_assets) == {PREFIX + name for name in (
        "console_shell.mjs", "console_shell.css", "controller_review.mjs", "controller_review.css")}
    for name, mime in (("controller_review.mjs", "application/javascript"), ("controller_review.css", "text/css")):
        assert optional_assets[PREFIX + name] == ((package / name).read_bytes(), mime)
    optional = optional_transform(native)
    assert optional.count(b'id="root"') == 1
    for line in native.splitlines():
        if not line.startswith((b'</head>', b'</body>')):
            assert line in optional
    assert (PREFIX + "controller_review.mjs").encode() in optional
    assert (PREFIX + "controller_review.css").encode() in optional
    base_assets, base_transform = presentation.load_console_presentation(package)
    assert set(base_assets) == {PREFIX + "console_shell.mjs", PREFIX + "console_shell.css"}
    assert b"controller_review" not in base_transform(native)
    assert b"toolkit-ui" not in base_transform(native)
    assert not any(name.split(".", 1)[0] in NATIVE_PACKAGES for name in sys.modules)
    assert "native_controller_review" not in sys.modules


def test_extracted_presentation_restores_preexisting_native_modules(tmp_path, monkeypatch):
    retained = {name: ModuleType(name) for name in (
        "meerkat_toolkit", "meerkat_toolkit.host_control", "meerkat_mobkit",
        "meerkat_mobkit.runtime", "native_controller_review")}
    for name, module in retained.items():
        monkeypatch.setitem(sys.modules, name, module)
    with pytest.MonkeyPatch.context() as extraction_patch:
        test_extracted_optional_review_assets_compose_while_default_remains_dormant(
            tmp_path, extraction_patch)
        assert not any(name in sys.modules for name in retained)
    assert all(sys.modules[name] is module for name, module in retained.items())


def test_builder_cannot_replace_any_packaged_review_file(tmp_path):
    workspace = ImplementationWorkspace(tmp_path / "workspace", starter_blueprint())
    workspace.seed()
    digest = workspace.digest()
    for name in REVIEW_FILES:
        target = workspace.project / name
        assert target.is_file(), f"The protected seed is missing {name}"
        before = target.read_bytes()
        with pytest.raises(WorkspaceError, match="read-only"):
            workspace.write_file(name, "replace protected reviewer")
        assert target.read_bytes() == before
        assert workspace.digest() == digest
