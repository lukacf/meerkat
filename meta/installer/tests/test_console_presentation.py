"""Fresh source/file composition only; no installed/native host operation."""
import os
from pathlib import Path, PurePosixPath
import posixpath
import re
import shutil

import pytest

from launchkit.templates_v2 import console_presentation as presentation


ROOT = Path(__file__).resolve().parents[1]
TEMPLATES = ROOT / "launchkit/templates_v2"


@pytest.fixture
def installed(tmp_path):
    root = tmp_path / "A folder with spaces"
    root.mkdir()
    for name in presentation.SHELL_FILES:
        shutil.copyfile(TEMPLATES / name, root / name)
    shutil.copytree(TEMPLATES / "toolkit-ui", root / "toolkit-ui")
    return root


def test_base_snapshot_does_not_require_or_read_optional_setup(tmp_path):
    root = tmp_path / "base authenticated app"
    root.mkdir()
    for name in ("console_shell.mjs", "console_shell.css"):
        shutil.copyfile(TEMPLATES / name, root / name)
    # Optional components may be absent or unsafe. The base shell does not
    # select, open or publish them, rather than silently accepting their bytes.
    (root / "toolkit-ui").symlink_to(tmp_path / "unavailable toolkit", target_is_directory=True)
    os.mkfifo(root / "console_setup.mjs")
    assets, transform = presentation.load_console_presentation(root)
    assert set(assets) == {presentation.PREFIX + name for name in
                          ("console_shell.mjs", "console_shell.css")}
    result = transform(b'<html><head></head><body><div id="root"></div></body></html>')
    assert b'mountConsoleShell();' in result
    assert b'console_setup' not in result and b'toolkit-ui' not in result


def test_actual_public_asset_snapshot_and_native_html_composition(installed):
    assets, transform = presentation.load_console_presentation(installed, setup=True)
    expected = [*presentation.SHELL_FILES,
                *("toolkit-ui/" + name for name in presentation.TOOLKIT_FILES)]
    assert len(assets) == 19
    for name in expected:
        content, mime = assets[presentation.PREFIX + name]
        assert content == (TEMPLATES / name).read_bytes()
        assert mime == ("text/css" if name.endswith(".css") else "application/javascript")
    # Preserve all 30 distribution members locally, without publishing type,
    # license or metadata content through the JS/CSS asset surface.
    members = [path for path in (installed / "toolkit-ui").rglob("*") if path.is_file()]
    assert len(members) == 30
    assert set(presentation.TOOLKIT_FILES) == {
        str(path.relative_to(installed / "toolkit-ui"))
        for path in members if path.suffix in {".js", ".css"}
    }
    assert all(not name.endswith((".d.ts", ".md", "package.json", "LICENSE-MIT", "LICENSE-APACHE")) for name in assets)
    native = (ROOT / "tests/fixtures/console_shell/native-console.html").read_bytes()
    result = transform(native)
    assert b'<div id="root"></div>' in result
    assert b'/console/assets/console-app.js?v=250bc27b79ec' in result
    assert b'import("/console/forge/assets/toolkit-ui/index.js")' in result
    assert b'/console/forge/assets/console_shell.css' in result


@pytest.mark.parametrize("name", ["console_shell.mjs", "console_shell.css"])
def test_base_asset_is_required_even_without_optional_setup(installed, name):
    (installed / name).unlink()
    with pytest.raises(ValueError, match="presentation assets are unavailable"):
        presentation.load_console_presentation(installed)


def test_every_actual_relative_js_import_has_a_selected_public_asset(installed):
    assets, _ = presentation.load_console_presentation(installed, setup=True)
    observed = set()
    for url, (content, mime) in assets.items():
        if mime != "application/javascript":
            continue
        for name in re.findall(r'\b(?:from|import)\s*["\']([^"\']+)["\']', content.decode()):
            assert name.startswith(("./", "../")), "The acquired UI must not depend on an unbound module origin."
            target = posixpath.normpath(str(PurePosixPath(url).parent / name))
            assert target in assets
            observed.add(target)
    assert presentation.PREFIX + "toolkit-ui/view.js" in observed
    assert presentation.PREFIX + "toolkit-ui/projection.js" in observed
    assert presentation.PREFIX + "toolkit-ui/setup-model.js" in observed
    assert presentation.PREFIX + "toolkit-ui/setup-markdown.js" in observed
    assert presentation.PREFIX + "toolkit-ui/vendor/marked/marked.esm.js" in observed
    assert presentation.PREFIX + "toolkit-ui/vendor/entities/decode.js" in observed


def test_frozen_snapshot_does_not_read_files_during_requests_or_follow_later_edits(installed):
    assets, transform = presentation.load_console_presentation(installed, setup=True)
    before = assets[presentation.PREFIX + "console_setup.mjs"]
    (installed / "console_setup.mjs").write_bytes(b"changed after snapshot")
    shutil.rmtree(installed)
    assert assets[presentation.PREFIX + "console_setup.mjs"] == before
    with pytest.raises(TypeError):
        assets[presentation.PREFIX + "console_setup.mjs"] = (b"changed", "application/javascript")
    assert b'data-forge-console-shell' in transform(b'<html><head></head><body><div id="root"></div></body></html>')


@pytest.mark.parametrize("missing", ["console_setup.css", "toolkit-ui/projection.js", "toolkit-ui/style.css"])
def test_missing_closure_member_refuses_instead_of_returning_partial_map(installed, missing):
    (installed / missing).unlink()
    with pytest.raises(ValueError, match="presentation assets are unavailable"):
        presentation.load_console_presentation(installed, setup=True)


@pytest.mark.parametrize("linked", ["console_setup.mjs", "toolkit-ui/index.js", "toolkit-ui"])
def test_file_or_directory_symlink_is_never_read(installed, linked, tmp_path):
    path = installed / linked
    outside = tmp_path / "outside"
    if path.is_dir():
        shutil.copytree(path, outside); shutil.rmtree(path)
    else:
        outside.write_bytes(b"outside canary"); path.unlink()
    path.symlink_to(outside, target_is_directory=outside.is_dir())
    with pytest.raises(ValueError, match="presentation assets are unavailable"):
        presentation.load_console_presentation(installed, setup=True)


def test_selected_root_symlink_is_refused(installed, tmp_path):
    link = tmp_path / "linked installed root"
    link.symlink_to(installed, target_is_directory=True)
    with pytest.raises(ValueError):
        presentation.load_console_presentation(link)


@pytest.mark.parametrize("kind", ["fifo", "directory", "hardlink"])
def test_nonregular_or_multiple_link_source_is_refused_without_blocking(installed, kind, tmp_path):
    path = installed / "console_setup.css"
    path.unlink()
    if kind == "fifo":
        os.mkfifo(path)
    elif kind == "directory":
        path.mkdir()
    else:
        outside = tmp_path / "other source"; outside.write_bytes(b"linked canary"); os.link(outside, path)
    with pytest.raises(ValueError):
        presentation.load_console_presentation(installed, setup=True)


def test_exact_source_size_limit_and_one_byte_over(installed):
    path = installed / "console_setup.css"
    with path.open("wb") as file:
        file.truncate(presentation.MAX_ASSET_BYTES)
    assets, _ = presentation.load_console_presentation(installed, setup=True)
    assert len(assets[presentation.PREFIX + "console_setup.css"][0]) == 16 * 1024 * 1024
    with path.open("ab") as file:
        file.write(b"x")
    with pytest.raises(ValueError):
        presentation.load_console_presentation(installed, setup=True)


def test_path_replaced_during_read_is_not_adopted(installed, monkeypatch):
    original = os.read
    replaced = False

    def read(descriptor, size):
        nonlocal replaced
        result = original(descriptor, size)
        if result and not replaced:
            replacement = installed / "replacement"
            replacement.write_bytes(b"different payload")
            replacement.replace(installed / "console_shell.mjs")
            replaced = True
        return result

    monkeypatch.setattr(os, "read", read)
    with pytest.raises(ValueError, match="changed while being read"):
        presentation.load_console_presentation(installed, setup=True)


def test_all_owned_descriptors_close_on_missing_asset(installed, monkeypatch):
    original_open, original_close = os.open, os.close
    opened, closed = [], []

    def open_file(*args, **kwargs):
        descriptor = original_open(*args, **kwargs); opened.append(descriptor); return descriptor

    def close_file(descriptor):
        closed.append(descriptor); return original_close(descriptor)

    (installed / "toolkit-ui/view.js").unlink()
    monkeypatch.setattr(os, "open", open_file); monkeypatch.setattr(os, "close", close_file)
    with pytest.raises(ValueError):
        presentation.load_console_presentation(installed, setup=True)
    assert sorted(opened) == sorted(closed)


@pytest.mark.parametrize("linked", ["vendor", "vendor/entities/generated", "vendor/entities/internal"])
def test_nested_directory_symlink_refuses_complete_snapshot_and_closes_descriptors(installed, linked, tmp_path, monkeypatch):
    path = installed / "toolkit-ui" / linked
    outside = tmp_path / "outside vendor closure"
    shutil.copytree(path, outside)
    shutil.rmtree(path)
    path.symlink_to(outside, target_is_directory=True)
    original_open, original_close = os.open, os.close
    opened, closed = [], []

    def open_file(*args, **kwargs):
        descriptor = original_open(*args, **kwargs); opened.append(descriptor); return descriptor

    def close_file(descriptor):
        closed.append(descriptor); return original_close(descriptor)

    monkeypatch.setattr(os, "open", open_file); monkeypatch.setattr(os, "close", close_file)
    try:
        with pytest.raises(ValueError, match="presentation assets are unavailable"):
            presentation.load_console_presentation(installed, setup=True)
    finally:
        assert sorted(opened) == sorted(closed)


@pytest.mark.parametrize("ancestor,leaf", [
    ("vendor/entities", "vendor/entities/decode.js"),
    ("vendor/entities/generated", "vendor/entities/generated/decode-data-html.js"),
])
def test_nested_directory_replaced_during_read_refuses_same_leaf_and_closes_descriptors(installed, ancestor, leaf, tmp_path, monkeypatch):
    source = installed / "toolkit-ui" / leaf
    before = presentation._stamp(source.stat())
    original_open, original_close, original_read = os.open, os.close, os.read
    opened, closed = [], []
    replaced = False

    def open_file(*args, **kwargs):
        descriptor = original_open(*args, **kwargs); opened.append(descriptor); return descriptor

    def close_file(descriptor):
        closed.append(descriptor); return original_close(descriptor)

    def read(descriptor, size):
        nonlocal replaced
        result = original_read(descriptor, size)
        if result and not replaced and presentation._stamp(os.fstat(descriptor)) == before:
            directory = installed / "toolkit-ui" / ancestor
            retained = tmp_path / "retained vendor directory"
            directory.rename(retained)
            directory.symlink_to(retained, target_is_directory=True)
            # The leaf is identical. A basename-only no-follow check would
            # silently follow the newly selected ancestor to these same bytes.
            assert presentation._stamp(source.stat()) == before
            replaced = True
        return result

    monkeypatch.setattr(os, "open", open_file); monkeypatch.setattr(os, "close", close_file)
    monkeypatch.setattr(os, "read", read)
    try:
        with pytest.raises(ValueError, match="changed while being read"):
            presentation.load_console_presentation(installed, setup=True)
    finally:
        assert replaced
        assert sorted(opened) == sorted(closed)
