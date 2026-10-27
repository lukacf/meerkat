"""Offline authenticated static/HTML composition, not browser/native acceptance."""
from __future__ import annotations

import asyncio
import html
from pathlib import Path
import re
import shutil

import httpx
import pytest

from launchkit.templates_v2 import console_auth
from launchkit.templates_v2.console_auth import ConsoleFront
from launchkit.templates_v2.console_presentation import load_console_presentation


ASSET = "/console/forge/assets/toolkit/setup-conversation.js"
CSS_ASSET = "/console/forge/assets/toolkit/style.css"
NATIVE_HTML = (b'<!doctype html><html><body><div id="root"></div>'
               b'<script src="/console/assets/console-app.js" defer></script>'
               b'</body></html>')
NATIVE_IIFE = b'(()=>{window.nativeConsoleMount="root";})();'
PUBLIC_JS = b'export const publicSetupView = "qualified-fixture";'
PUBLIC_CSS = b'.toolkit-view{display:block}'
FILE_LIMIT = 16 * 1024 * 1024


async def sign_in(client, front):
    response = await client.get("/login")
    nonce = re.search(r'name="nonce" value="([^"]+)"', response.text)
    assert nonce is not None
    response = await client.post("/login", data={"nonce": html.unescape(nonce[1]),
        "password": "private-console-fixture"}, headers={"Origin": front.origin})
    assert response.status_code == 303


def make_front(tmp_path, *, native=None, **options):
    requests = []
    def upstream(request):
        requests.append(request)
        return native(request) if native else httpx.Response(200, content=NATIVE_HTML,
            headers={"Content-Type": "text/html; charset=utf-8"})
    front = ConsoleFront(tmp_path, 8855, "reader", "private-console-fixture",
        transport=httpx.MockTransport(upstream), **options)
    application = front.application("http://127.0.0.1:61347")
    return front, application, requests


def assert_public_headers(response):
    assert response.headers["cache-control"] == "no-store"
    assert response.headers["x-content-type-options"] == "nosniff"


async def test_console_assets_default_preserves_native_console_bytes(tmp_path):
    front, application, native = make_front(tmp_path)
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=application),
                                    base_url=front.origin) as client:
            await sign_in(client, front)
            response = await client.get("/console")
            assert response.status_code == 200 and response.content == NATIVE_HTML
            assert [request.url.path for request in native] == ["/console"]
    finally:
        await front.close()


async def test_base_shell_and_plain_logout_keep_human_login_boundary_without_setup(tmp_path):
    templates = Path(__file__).resolve().parents[1] / "launchkit/templates_v2"
    for name in ("console_shell.mjs", "console_shell.css"):
        shutil.copyfile(templates / name, tmp_path / name)
    assets, transform = load_console_presentation(tmp_path)
    document = NATIVE_HTML.replace(b'<html>', b'<html><head></head>')
    front, application, native = make_front(tmp_path, console_assets=assets, document_transform=transform,
        native=lambda request: httpx.Response(200, content=document, headers={"Content-Type": "text/html"}))
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=application),
                                    base_url=front.origin) as client:
            assert (await client.get("/console")).status_code == 303
            assert (await client.get("/console/forge/assets/console_shell.mjs")).status_code == 401
            assert native == []
            await sign_in(client, front)
            response = await client.get("/console")
            assert response.status_code == 200 and b'mountConsoleShell();' in response.content
            assert b'console_setup' not in response.content and b'toolkit-ui' not in response.content
            assert response.content.count(b'<div id="root"></div>') == 1
            assert b'<script src="/console/assets/console-app.js" defer></script>' in response.content
            for path, (payload, _) in assets.items():
                assert (await client.get(path)).content == payload
            assert front.connect_factory is None and front.frontend_management is None
            assert (await client.post("/logout", headers={"Origin": "http://other.invalid"})).status_code == 403
            assert (await client.get("/console")).status_code == 200
            logout = await client.post("/logout", headers={"Origin": front.origin})
            assert logout.status_code == 303 and logout.headers["location"] == "/login"
            assert (await client.get("/console")).status_code == 303
            assert (await client.get("/console/forge/assets/console_shell.mjs")).status_code == 401
            assert [request.url.path for request in native] == ["/console", "/console"]
    finally:
        await front.close()


async def test_console_assets_unconfigured_prefix_never_falls_through_to_native(tmp_path):
    front, application, native = make_front(tmp_path)
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=application),
                                    base_url=front.origin) as client:
            await sign_in(client, front)
            response = await client.get(ASSET)
            assert response.status_code == 404
            assert native == []
    finally:
        await front.close()


async def test_console_assets_copies_trusted_map_and_serves_explicit_js_css_types(tmp_path):
    assets = {ASSET: (PUBLIC_JS, "application/javascript"), CSS_ASSET: (PUBLIC_CSS, "text/css"),
              "/console/forge/assets/alias.js": (PUBLIC_JS, "text/javascript")}
    original = dict(assets)
    front, application, native = make_front(tmp_path, console_assets=assets)
    assets[ASSET] = (b"MUTATED_SOURCE_MAP_CANARY", "application/javascript")
    assets.clear()
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=application),
                                    base_url=front.origin) as client:
            await sign_in(client, front)
            for path, (content, content_type) in original.items():
                response = await client.get(path)
                assert response.status_code == 200 and response.content == content
                assert response.headers["content-type"].split(";", 1)[0] == content_type
                assert_public_headers(response)
            assert native == []
    finally:
        await front.close()


async def test_console_assets_requires_human_login_not_health_or_native_bearer(tmp_path):
    transformed = []
    front, application, native = make_front(tmp_path, console_assets={ASSET: (PUBLIC_JS, "application/javascript")},
        document_transform=lambda value: transformed.append(value) or value)
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=application),
                                    base_url=front.origin) as client:
            for bearer in (None, "host-health", "native-console"):
                headers = {} if bearer is None else {"Authorization": "Bearer " +
                    console_auth._token(front.key, front.principal, scope=bearer)}
                assert (await client.get(ASSET, headers=headers)).status_code == 401
                assert (await client.get("/console", headers=headers)).status_code == 303
            assert transformed == [] and native == []
    finally:
        await front.close()


async def test_console_assets_closed_paths_methods_queries_never_reach_native(tmp_path):
    front, application, native = make_front(tmp_path, console_assets={ASSET: (PUBLIC_JS, "application/javascript")})
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=application),
                                    base_url=front.origin) as client:
            await sign_in(client, front)
            assert (await client.get("/console/forge/assets/unknown.js")).status_code == 404
            assert (await client.post(ASSET, content=b"untrusted", headers={"Origin": front.origin})).status_code == 405
            assert (await client.get(ASSET + "?path=/private&session_id=browser")).status_code == 400
            for path in ["/console/forge/assets/%2e%2e/private.js",
                         "/console/forge/assets//toolkit/setup-conversation.js",
                         "/console/forge/assets/toolkit%5csetup-conversation.js"]:
                assert (await client.get(path)).status_code in (400, 404)
            assert native == []
    finally:
        await front.close()


@pytest.mark.parametrize("assets", [
    [], {ASSET: [PUBLIC_JS, "application/javascript"]}, {ASSET: (bytearray(PUBLIC_JS), "application/javascript")},
    {ASSET: ("not bytes", "application/javascript")}, {ASSET: (PUBLIC_JS, "text/html")},
    {ASSET: (PUBLIC_JS, "application/javascript; charset=utf-8")},
    {ASSET: (PUBLIC_JS, "application/javascript", "extra")},
    {ASSET: (b"x" * (FILE_LIMIT + 1), "application/javascript")},
    {"/arbitrary/setup.js": (PUBLIC_JS, "application/javascript")},
    {"/console/forge/assets/": (PUBLIC_JS, "application/javascript")},
    {"/console/forge/assets/../private.js": (PUBLIC_JS, "application/javascript")},
    {"/console/forge/assets/./setup.js": (PUBLIC_JS, "application/javascript")},
    {"/console/forge/assets//setup.js": (PUBLIC_JS, "application/javascript")},
    {"/console/forge/assets/setup.js?selector=browser": (PUBLIC_JS, "application/javascript")},
    {"/console/forge/assets/setup.js#fragment": (PUBLIC_JS, "application/javascript")},
    {"/console/forge/assets/%2e%2e/private.js": (PUBLIC_JS, "application/javascript")},
    {"/console/forge/assets/a\\setup.js": (PUBLIC_JS, "application/javascript")},
    {"/console/forge/assets/a\x00setup.js": (PUBLIC_JS, "application/javascript")},
    {1: (PUBLIC_JS, "application/javascript")},
], ids=["not-mapping", "mutable-tuple", "mutable-bytes", "text-payload", "html-mime", "unselected-mime-parameters",
        "extra-tuple-field", "oversize", "outside-prefix", "empty-name", "parent-segment", "dot-segment",
        "duplicate-slash", "query", "fragment", "encoded-traversal", "backslash", "nul", "nontext-path"])
def test_console_assets_rejects_invalid_trusted_mapping_at_construction(tmp_path, assets):
    with pytest.raises(ValueError):
        ConsoleFront(tmp_path, 8855, "reader", "private-console-fixture", console_assets=assets)


def test_console_assets_rejects_noncallable_transform_at_construction(tmp_path):
    with pytest.raises(ValueError):
        ConsoleFront(tmp_path, 8855, "reader", "private-console-fixture", document_transform=object())


@pytest.mark.parametrize("path", ["/console", "/console/"])
async def test_console_assets_transforms_native_html_as_bytes_preserving_native_mount(tmp_path, path):
    seen = []
    def transform(*args):
        assert len(args) == 1 and type(args[0]) is bytes
        seen.append(args[0])
        return args[0].replace(b"</body>", b'<aside id="forge-connect-drawer"></aside></body>')
    front, application, native = make_front(tmp_path, document_transform=transform)
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=application),
                                    base_url=front.origin) as client:
            await sign_in(client, front)
            response = await client.get(path)
            assert response.status_code == 200
            assert b'<div id="root"></div>' in response.content
            assert b'<script src="/console/assets/console-app.js" defer></script>' in response.content
            assert b'<aside id="forge-connect-drawer"></aside>' in response.content
            assert seen == [NATIVE_HTML]
            assert [request.url.path for request in native] == [path]
            assert_public_headers(response)
    finally:
        await front.close()


@pytest.mark.parametrize("method,path,status,mime,content", [
    ("GET", "/console", 200, "application/json", b'{"native":"canonical"}'),
    ("GET", "/console", 503, "text/html", b"Native unavailable HTML"),
    ("GET", "/console", 201, "text/html", b"Native non200 HTML"),
    ("POST", "/console", 200, "text/html", NATIVE_HTML),
    ("GET", "/console/assets/console-app.js?v=qualified", 200, "application/javascript", NATIVE_IIFE),
    ("GET", "/console/assets/console-app.css", 200, "text/css", b"#root{display:block}"),
])
async def test_console_assets_does_not_transform_other_native_responses(tmp_path, method, path, status, mime, content):
    transformed = []
    front, application, native = make_front(tmp_path,
        native=lambda request: httpx.Response(status, content=content, headers={"Content-Type": mime}),
        document_transform=lambda value: transformed.append(value) or b"INCORRECT_TRANSFORM_CANARY")
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=application),
                                    base_url=front.origin) as client:
            await sign_in(client, front)
            response = await client.request(method, path, headers={"Origin": front.origin} if method == "POST" else {})
            assert response.status_code == status and response.content == content
            assert transformed == [] and len(native) == 1
    finally:
        await front.close()


@pytest.mark.parametrize("kind", ["error", "nonbytes", "oversize"])
async def test_console_assets_transform_failure_is_fixed_generic_502(tmp_path, kind):
    def transform(_):
        if kind == "error":
            raise RuntimeError("PRIVATE_TRANSFORM_EXCEPTION_CANARY")
        if kind == "nonbytes":
            return "PRIVATE_TRANSFORM_RESULT_CANARY"
        return b"x" * (FILE_LIMIT + 1)
    front, application, _ = make_front(tmp_path, document_transform=transform)
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=application),
                                    base_url=front.origin) as client:
            await sign_in(client, front)
            response = await client.get("/console")
            assert response.status_code == 502
            assert "PRIVATE_TRANSFORM" not in response.text
            assert response.text == "The native Console is unavailable."
            assert_public_headers(response)
    finally:
        await front.close()


@pytest.mark.parametrize("retirement", ["logout", "expiry"])
async def test_console_assets_held_html_body_rechecks_login_before_transform_or_reply(tmp_path, retirement):
    entered, release = asyncio.Event(), asyncio.Event()
    closed, transformed = [], []
    class HeldHTML(httpx.AsyncByteStream):
        async def __aiter__(self):
            entered.set()
            await release.wait()
            yield NATIVE_HTML
        async def aclose(self):
            closed.append(True)
    front, application, _ = make_front(tmp_path,
        native=lambda request: httpx.Response(200, stream=HeldHTML(), headers={"Content-Type": "text/html"}),
        document_transform=lambda value: transformed.append(value) or value)
    pending = None
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=application),
                                    base_url=front.origin) as client:
            await sign_in(client, front)
            session = client.cookies[front.cookie]
            pending = asyncio.create_task(client.get("/console"))
            await asyncio.wait_for(entered.wait(), 30)
            if retirement == "logout":
                assert (await client.post("/logout", headers={"Origin": front.origin})).status_code == 303
            else:
                front._logins[session].expires = 0
                front.sessions[session] = 0
            release.set()
            response = await asyncio.wait_for(pending, 1)
            assert response.status_code == 401
            assert transformed == [] and NATIVE_HTML not in response.content
            assert closed == [True]
    finally:
        release.set()
        if pending is not None:
            await asyncio.gather(pending, return_exceptions=True)
        await front.close()
