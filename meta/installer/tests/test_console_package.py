"""Exercise actual generated package closure without starting a runtime."""
import hashlib
import importlib.util
import io
from pathlib import Path
import sys
import zipfile

import pytest

from launchkit.blueprint import starter_blueprint
from launchkit.compiler import build_archive, generate_package
from launchkit.implementation_workspace import ImplementationWorkspace, WorkspaceError


TEMPLATES = Path(__file__).parents[1] / "launchkit/templates_v2"
HOST_FILES = ("console_auth.py", "console_connect.py", "console_binding.py", "console_presentation.py", "console_shell.py",
              "console_setup.mjs", "console_setup.css", "console_shell.mjs", "console_shell.css")
TOOLKIT_FILES = ("LICENSE-APACHE", "LICENSE-MIT", "THIRD_PARTY_NOTICES.md", "package.json",
                 "index.js", "index.d.ts", "model.js", "model.d.ts", "projection.js", "projection.d.ts",
                 "setup-conversation.js", "setup-conversation.d.ts", "setup-markdown.js", "setup-markdown.d.ts",
                 "setup-model.js", "setup-model.d.ts", "view.js", "view.d.ts", "style.css",
                 "vendor/entities/LICENSE", "vendor/entities/package.json", "vendor/entities/decode-codepoint.js",
                 "vendor/entities/decode.js", "vendor/entities/generated/decode-data-html.js",
                 "vendor/entities/generated/decode-data-xml.js", "vendor/entities/internal/bin-trie-flags.js",
                 "vendor/entities/internal/decode-shared.js", "vendor/marked/LICENSE", "vendor/marked/package.json",
                 "vendor/marked/marked.esm.js")
EXPECTED = (*HOST_FILES, *(f"toolkit-ui/{name}" for name in TOOLKIT_FILES))


def test_compiled_console_contains_exact_ui_module_closure_and_notices():
    files = {file.path: file for file in generate_package(starter_blueprint())}
    for name in EXPECTED:
        assert name in files, f"The generated package is missing {name}"
        assert files[name].as_bytes() == (TEMPLATES / name).read_bytes()
        assert files[name].mode == 0o644
    assert {name for name in files if name.startswith("toolkit-ui/")} == {
        f"toolkit-ui/{name}" for name in TOOLKIT_FILES}


def test_exported_console_assets_are_inside_the_normal_checksum_manifest():
    spec = starter_blueprint()
    with zipfile.ZipFile(io.BytesIO(build_archive(spec))) as archive:
        checksums = dict(line.split("  ", 1)[::-1] for line in
                         archive.read(f"{spec.project.slug}/SHA256SUMS").decode().splitlines())
        for name in EXPECTED:
            data = archive.read(f"{spec.project.slug}/{name}")
            assert checksums[name] == hashlib.sha256(data).hexdigest()
            assert data == (TEMPLATES / name).read_bytes()
            assert archive.getinfo(f"{spec.project.slug}/{name}").external_attr >> 16 & 0o777 == 0o644


def test_extracted_package_loads_native_document_composition(tmp_path, monkeypatch):
    spec = starter_blueprint()
    with zipfile.ZipFile(io.BytesIO(build_archive(spec))) as archive:
        archive.extractall(tmp_path)
    package = tmp_path / spec.project.slug
    assert (package / "console_presentation.py").is_file()
    monkeypatch.syspath_prepend(str(package))
    # A fresh module namespace loads the actual extracted helper, not a checkout import.
    shell_loader = importlib.util.spec_from_file_location("console_shell", package / "console_shell.py")
    shell_module = importlib.util.module_from_spec(shell_loader)
    monkeypatch.setitem(sys.modules, "console_shell", shell_module)
    shell_loader.loader.exec_module(shell_module)
    loader = importlib.util.spec_from_file_location("package_console_presentation",
                                                   package / "console_presentation.py")
    module = importlib.util.module_from_spec(loader)
    loader.loader.exec_module(module)
    assets, transform = module.load_console_presentation(package, setup=True)
    native = (b'<!doctype html>\n<html><head>\n'
              b'<meta name="mobkit-base-url" content="">\n'
              b'<link rel="stylesheet" href="/console/assets/console-app.css?v=original">\n'
              b'<script src="/console/assets/console-app.js?v=original" defer></script>\n'
              b'</head><body>\n<div id="root"></div>\n</body></html>\n')
    result = transform(native)
    assert result.count(b'id="root"') == 1
    for line in native.splitlines():
        if not line.startswith((b'</head>', b'</body>')):
            assert line in result
    for name in ("console_shell.mjs", "console_setup.mjs", "toolkit-ui/index.js"):
        assert f"/console/forge/assets/{name}".encode() in result
    for url, (data, media) in assets.items():
        assert url.startswith("/console/forge/assets/")
        assert data == (package / url.removeprefix("/console/forge/assets/")).read_bytes()
        assert media in ("text/css", "application/javascript", "text/javascript")
        assert not url.endswith((".d.ts", ".md", ".json", "LICENSE-MIT", "LICENSE-APACHE"))
    # The same normal extracted package supports base controls without
    # selecting or importing its optional Toolkit/setup distribution.
    base_assets, base_transform = module.load_console_presentation(package)
    assert set(base_assets) == {"/console/forge/assets/console_shell.mjs",
                               "/console/forge/assets/console_shell.css"}
    base = base_transform(native)
    assert b'mountConsoleShell();' in base
    assert b'console_setup' not in base and b'toolkit-ui' not in base


def test_builder_cannot_rewrite_packaged_console_or_acquired_ui(tmp_path):
    workspace = ImplementationWorkspace(tmp_path / "workspace", starter_blueprint())
    workspace.seed()
    for name in EXPECTED:
        target = workspace.project / name
        assert target.is_file(), name
        before = target.read_bytes()
        with pytest.raises(WorkspaceError):
            workspace.write_file(name, "replace protected UI")
        assert target.read_bytes() == before
