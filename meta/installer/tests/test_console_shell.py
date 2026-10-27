"""Pure composition of captured native HTML and selected local assets."""

import hashlib
import importlib.util
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "console_shell", ROOT / "launchkit/templates_v2/console_shell.py")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)
HTML = ROOT / "tests/fixtures/console_shell/native-console.html"
URLS = {name: f"/console/forge/assets/{file}" for name, file in {
    "shell_js": "console_shell.mjs", "shell_css": "console_shell.css",
    "setup_js": "console_setup.mjs", "setup_css": "console_setup.css",
    "toolkit_ui_js": "toolkit-ui/index.js", "toolkit_ui_css": "toolkit-ui/style.css",
}.items()}


def test_base_shell_keeps_native_console_without_importing_optional_setup():
    result = MODULE.render_console_shell(HTML.read_bytes(),
        **{name: URLS[name] for name in ("shell_js", "shell_css")})
    assert result.count(b'<div id="root"></div>') == 1
    assert b'/console/assets/console-app.js?v=250bc27b79ec' in result
    assert b'mountConsoleShell();' in result
    assert b'console_setup' not in result and b'toolkit-ui' not in result


def test_actual_native_document_keeps_root_iife_meta_and_resource_order():
    native = HTML.read_bytes()
    assert hashlib.sha256(native).hexdigest() == "6ba5c7a5f6f1786a7fcc2e01060b5adc5cc4a561e016c0238ccdbe77d65d4a21"
    result = MODULE.render_console_shell(native, **URLS).decode()
    # Every original line remains literal and ordered, including the actual IIFE.
    offset = 0
    for line in native.decode().splitlines():
        found = result.index(line, offset)
        offset = found + len(line)
    assert result.count('<div id="root"></div>') == 1
    assert result.count('console-app.js?v=250bc27b79ec') == 1
    assert result.index('console-app.css?') < result.index('toolkit-ui/style.css') < result.index('console_setup.css') < result.index('console_shell.css')
    assert 'import("/console/forge/assets/toolkit-ui/index.js")' in result
    assert 'mountConsoleShell({ loadSetup: async () => {' in result
    assert 'import { mountSetupConversation } from' not in result
    assert 'iframe' not in result
    assert 'Session' not in result


@pytest.mark.parametrize("missing", ["setup_js", "setup_css", "toolkit_ui_js", "toolkit_ui_css"])
def test_partial_optional_setup_selection_is_refused(missing):
    selected = {name: value for name, value in URLS.items() if name != missing}
    with pytest.raises(ValueError, match="complete selected asset closure"):
        MODULE.render_console_shell(HTML.read_bytes(), **selected)


@pytest.mark.parametrize("url", [
    "https://example.invalid/ui.js", "//example.invalid/ui.js", "/console/assets/ui.js",
    "/console/forge/assets/../private.js", "/console/forge/assets/a/./b.js",
    "/console/forge/assets/a//b.js", "/console/forge/assets/a%2fb.js",
    "/console/forge/assets/ui.js?chosen=browser", "/console/forge/assets/ui.js#selection",
    '/console/forge/assets/ui.js"</script>',
])
def test_rejects_unselected_or_ambiguous_resource_urls(url):
    with pytest.raises(ValueError):
        MODULE.render_console_shell(HTML.read_bytes(), **{**URLS, "toolkit_ui_js": url})


@pytest.mark.parametrize("change", [
    lambda doc: doc.replace(b'<div id="root"></div>', b''),
    lambda doc: doc.replace(b'<div id="root"></div>', b'<div id="root"></div>' * 2),
    lambda doc: doc.replace(b'</body>', b''),
])
def test_unexpected_native_document_fails_without_substituting_a_second_app(change):
    with pytest.raises(ValueError):
        MODULE.render_console_shell(change(HTML.read_bytes()), **URLS)


def test_shell_cannot_be_injected_twice():
    result = MODULE.render_console_shell(HTML.read_bytes(), **URLS)
    with pytest.raises(ValueError):
        MODULE.render_console_shell(result, **URLS)
