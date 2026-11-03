"""HTTPS front transport controls; no real TLS ingress or provider acceptance."""
import json
import sys
from http.cookies import SimpleCookie
from types import SimpleNamespace

import httpx
import jwt
import pytest

from launchkit.templates_v2 import console_auth
from launchkit.templates_v2.console_auth import ConsoleFront, ISSUER, AUDIENCE
from test_console_login_enrollment import EnrollmentOwner


PASSWORD = "local-password-123"
ORIGIN = "https://forge.example.test"
LANDING_CSP = ("default-src 'none'; script-src 'sha256-" + "A" * 43
               + "='; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'")


async def sign_in(client, front):
    form = await client.get("/login")
    assert form.status_code == 200
    nonce = client.cookies[front.cookie + "_login"]
    result = await client.post("/login", data={"nonce": nonce, "password": PASSWORD},
                               headers={"Origin": front.origin})
    assert result.status_code == 303
    return form, result


@pytest.mark.parametrize("origin", [
    "", "http://forge.example.test", "//forge.example.test", "https://",
    "https://user@forge.example.test", "https://user:password@forge.example.test",
    "https://forge.example.test/", "https://forge.example.test/callback",
    "https://forge.example.test?", "https://forge.example.test#",
    "https://forge.example.test:0", "https://forge.example.test:65536",
    "https://forge.example.test:", "https://forge.example.test:abc",
    "https://forge.example.test\\other", "https://forge.example.test\n",
    "https://forge%2eexample.test", "https://forg\u00e9.example.test",
    "https://.example.test", "https://-forge.example.test", 3, True,
])
def test_invalid_public_origin_refuses_before_key_creation(tmp_path, origin):
    with pytest.raises(ValueError, match="trusted Console origin"):
        ConsoleFront(tmp_path, 8855, "owner", PASSWORD, public_origin=origin)
    assert not (tmp_path / ".mobkit").exists()


@pytest.mark.parametrize("selected,expected", [
    (None, "http://127.0.0.1:8855"),
    (ORIGIN, ORIGIN),
    ("https://FORGE.example.test:443", ORIGIN),
    (ORIGIN + ":8443", ORIGIN + ":8443"),
    ("https://[::1]:8443", "https://[::1]:8443"),
])
async def test_trusted_origin_and_cookie_transport(tmp_path, selected, expected):
    observed = []
    front = ConsoleFront(tmp_path, 8855, "owner", PASSWORD, public_origin=selected,
        transport=httpx.MockTransport(lambda req: observed.append(req) or httpx.Response(200, json={})))
    app = front.application("http://127.0.0.1:61347")

    async def ingress(scope, receive, send):
        # TLS termination must not make request headers the origin authority.
        await app({**scope, "scheme": "http"}, receive, send)

    try:
        assert front.origin == expected and front.local_origin == "http://127.0.0.1:8855"
        assert front.cookie.startswith("__Host-") is (selected is not None)
        assert front.cookie.removeprefix("__Host-").startswith("mobkit_console_")
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=ingress), base_url=expected) as client:
            form, result = await sign_in(client, front)
            assert (await client.get("/console/experience")).status_code == 200
            assert len(observed) == 1
            logout = await client.post("/logout", headers={"Origin": expected})
            assert logout.status_code == 303 and not front.sessions
            # Include both nonce/session creation and deletion. Browser prefix
            # enforcement requires Secure, Path=/ and no Domain on every write.
            cookies = [next(iter(SimpleCookie(value).values()))
                for response in (form, result, logout) for value in response.headers.get_list("set-cookie")]
            assert len(cookies) == 4
            for cookie in cookies:
                assert cookie.key in {front.cookie, front.cookie + "_login"}
                assert cookie["httponly"] and cookie["samesite"] == "strict"
                assert bool(cookie["secure"]) is (selected is not None)
                assert cookie["path"] == "/" and not cookie["domain"]
    finally:
        await front.close()


async def test_public_mode_does_not_admit_unprefixed_cookie_names(tmp_path):
    front = ConsoleFront(tmp_path, 8855, "owner", PASSWORD, public_origin=ORIGIN,
        transport=httpx.MockTransport(lambda req: httpx.Response(200, json={})))
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                     base_url=ORIGIN) as client:
            await client.get("/login")
            nonce = client.cookies[front.cookie + "_login"]
            client.cookies.clear()
            legacy = front.cookie.removeprefix("__Host-")
            refused = await client.post("/login", data={"nonce": nonce, "password": PASSWORD},
                headers={"Origin": ORIGIN, "Cookie": f"{legacy}_login={nonce}"})
            assert refused.status_code == 403 and not front.sessions
            await sign_in(client, front)
            session = client.cookies[front.cookie]
            client.cookies.clear()
            assert (await client.get("/console/experience", headers={"Cookie": f"{legacy}={session}"})).status_code == 401
            assert (await client.get("/console/experience", headers={"Cookie": f"{front.cookie}={session}"})).status_code == 200
    finally:
        await front.close()


async def test_forwarded_headers_never_select_host_or_origin(tmp_path):
    observed = []
    front = ConsoleFront(tmp_path, 8855, "owner", PASSWORD, public_origin=ORIGIN,
        transport=httpx.MockTransport(lambda req: observed.append(req) or httpx.Response(200, json={})))
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                     base_url=ORIGIN) as client:
            await sign_in(client, front)
            spoofed = {"Host": "evil.example", "X-Forwarded-Host": "forge.example.test",
                       "X-Forwarded-Proto": "https", "Forwarded": "host=forge.example.test;proto=https"}
            assert (await client.get("/console/experience", headers=spoofed)).status_code == 400
            for wrong in ("https://evil.example", "http://forge.example.test", front.local_origin):
                assert (await client.post("/host/toolkit/setup/start", json={"review_id": 1},
                    headers={"Origin": wrong, "X-Forwarded-Host": "forge.example.test"})).status_code == 403
            assert not observed
            result = await client.post("/host/toolkit/setup/start", json={"review_id": 1},
                headers={"Origin": ORIGIN, "X-Forwarded-Host": "evil.example", "X-Forwarded-Proto": "http"})
            assert result.status_code == 200 and len(observed) == 1
            assert observed[0].headers["origin"] == ORIGIN
            assert "x-forwarded-host" not in observed[0].headers
    finally:
        await front.close()


async def test_loopback_health_alias_needs_its_read_only_bearer_not_browser_cookie(tmp_path):
    observed = []
    front = ConsoleFront(tmp_path, 8855, "owner", PASSWORD, public_origin=ORIGIN,
        transport=httpx.MockTransport(lambda req: observed.append(req) or httpx.Response(200, json={})))
    try:
        app = front.application("http://127.0.0.1:61347")
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url=ORIGIN) as browser:
            await sign_in(browser, front)
            cookie = front.cookie + "=" + browser.cookies[front.cookie]
        token = console_auth._token(front.key, front.principal, scope="host-health")
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url=front.local_origin) as probe:
            for path in ("/console/experience", "/console/identities"):
                assert (await probe.get(path)).status_code == 401
                assert (await probe.get(path, headers={"Cookie": cookie})).status_code == 401
                assert (await probe.get(path, headers={"Authorization": "Bearer wrong"})).status_code == 401
                assert (await probe.get(path, headers={"Authorization": "Bearer " + token})).status_code == 200
            for path in ("/login", "/console", "/host/toolkit/setup/callback", "/console/timeline"):
                assert (await probe.get(path, headers={"Authorization": "Bearer " + token})).status_code == 400
            assert (await probe.post("/console/experience", headers={"Authorization": "Bearer " + token})).status_code == 400
        assert len(observed) == 2
        for request in observed:
            claims = jwt.decode(request.headers["authorization"].removeprefix("Bearer "),
                                front.key, algorithms=["HS256"], issuer=ISSUER, audience=AUDIENCE)
            assert "sid" not in claims and "cookie" not in request.headers
    finally:
        await front.close()


async def test_https_toolkit_landing_and_post_preserve_same_login_without_leaking_query(tmp_path, caplog):
    observed = []
    def native(request):
        observed.append(request)
        if request.method == "GET":
            return httpx.Response(200, text="fixed landing", headers={
                "Content-Type": "text/html", "Content-Security-Policy": LANDING_CSP})
        return httpx.Response(202)
    front = ConsoleFront(tmp_path, 8855, "owner", PASSWORD, public_origin=ORIGIN,
                         transport=httpx.MockTransport(native))
    try:
        app = front.application("http://127.0.0.1:61347")
        path, query = "/host/toolkit/setup/callback", "state=PRIVATE_STATE&code=PRIVATE_CODE"
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url=ORIGIN) as client:
            await sign_in(client, front)
            login = front._logins[client.cookies[front.cookie]]
            landing = await client.get(path + "?" + query, headers={"Origin": "https://provider.example"})
            assert landing.status_code == 200 and landing.headers["referrer-policy"] == "no-referrer"
            assert not observed[-1].url.query
            assert not {"authorization", "cookie", "origin"} & set(observed[-1].headers)
            post = await client.post(path, json={"query": query}, headers={"Origin": ORIGIN})
            assert post.status_code == 202 and json.loads(observed[-1].content) == {"query": query}
            claims = jwt.decode(observed[-1].headers["authorization"].removeprefix("Bearer "),
                                front.key, algorithms=["HS256"], issuer=ISSUER, audience=AUDIENCE)
            assert claims["sid"] == login.login_scope_id and observed[-1].headers["origin"] == ORIGIN
            await client.post("/logout", headers={"Origin": ORIGIN})
            assert (await client.post(path, json={"query": query}, headers={"Origin": ORIGIN})).status_code == 401
            assert len(observed) == 2
        assert "PRIVATE_STATE" not in caplog.text and "PRIVATE_CODE" not in caplog.text
    finally:
        await front.close()


async def test_private_adapters_retain_one_context_and_never_rebind_after_logout(tmp_path, monkeypatch):
    # The factory below explicitly selects no native review. Avoid importing an
    # unselected native SDK merely to exercise the existing host factory seam.
    monkeypatch.setitem(sys.modules, console_auth.__package__ + ".native_controller_review",
                        SimpleNamespace(NativeControllerReview=type("UnusedReview", (), {})))
    enrolled, reviews = [], []
    def factory(context):
        owner = EnrollmentOwner(context)
        owner.release.set()
        enrolled.append(owner)
        return owner
    def review(context):
        reviews.append(context)
        return None
    front = ConsoleFront(tmp_path, 8855, "owner", PASSWORD, public_origin=ORIGIN,
        console_login_factory=factory, controller_review_factory=review,
        transport=httpx.MockTransport(lambda request: httpx.Response(200, json={})))
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                     base_url=ORIGIN) as client:
            await sign_in(client, front)
            first_login = front._logins[client.cookies[front.cookie]]
            await first_login.controller_review_start
            first = enrolled[0].context
            assert first is front._login_context(first_login) is first_login.context
            assert reviews == [first] and reviews[0] is first
            assert first.origin == ORIGIN and first.is_current() is True
            assert (await client.get("/console/experience")).status_code == 200
            assert front._login_context(first_login) is first and len(enrolled) == 1
            assert (await client.post("/logout", headers={"Origin": ORIGIN})).status_code == 303
            assert first.is_current() is False and enrolled[0].revoked
            await sign_in(client, front)
            second_login = front._logins[client.cookies[front.cookie]]
            second = front._login_context(second_login)
            assert second is enrolled[1].context and second is not first
            assert second.login_scope_id != first.login_scope_id
            assert second.is_current() is True and first.is_current() is False
    finally:
        await front.close()


@pytest.mark.parametrize("attribute,new_value", [
    ("origin", "https://replacement.example.test"), ("principal", "replacement-owner"),
])
async def test_captured_login_context_refuses_changed_host_binding(tmp_path, attribute, new_value):
    front = ConsoleFront(tmp_path, 8855, "owner", PASSWORD, public_origin=ORIGIN,
        transport=httpx.MockTransport(lambda request: httpx.Response(200)))
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                     base_url=ORIGIN) as client:
            await sign_in(client, front)
            login = front._logins[client.cookies[front.cookie]]
            context = front._login_context(login)
            assert context.is_current() is True
            setattr(front, attribute, new_value)
            assert context.is_current() is False
            assert front._login_context(login) is context
    finally:
        await front.close()
