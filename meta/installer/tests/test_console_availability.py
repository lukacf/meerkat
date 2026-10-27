"""Transport availability does not establish native application readiness."""
import httpx
import pytest

from launchkit.templates_v2.console_auth import ConsoleFront


ORIGIN = "https://forge.example.test"
PATH = "/host/availability"


def forbidden(*args, **kwargs):
    raise AssertionError("Availability must not touch login state or the native runtime.")


@pytest.mark.parametrize("public_origin", [None, ORIGIN])
@pytest.mark.parametrize("method", ["GET", "HEAD"])
async def test_availability_is_stateless_even_with_expired_browser_state(tmp_path, monkeypatch, public_origin, method):
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123", public_origin=public_origin,
                         transport=httpx.MockTransport(forbidden))
    app = front.application("http://127.0.0.1:61347")
    # Probe traffic must not become login traffic or drive session retirement.
    front.sessions["expired"] = 0
    front.login_tokens["expired"] = 0
    front.failures.append(0)
    monkeypatch.setattr(front, "_live_login", forbidden)
    monkeypatch.setattr(front, "_retire_login", forbidden)
    monkeypatch.setattr(front, "_login_response", forbidden)
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url=front.origin) as client:
            for _ in range(2):
                response = await client.request(method, PATH, headers={
                    "Cookie": f"{front.cookie}=expired", "Authorization": "Bearer ignored",
                })
                assert response.status_code == 204 and response.content == b""
                assert response.headers["cache-control"] == "no-store"
                assert response.headers["referrer-policy"] == "no-referrer"
                assert "set-cookie" not in response.headers
            assert front.sessions == {"expired": 0}
            assert front.login_tokens == {"expired": 0}
            assert front.failures == [0] and not front._logins
    finally:
        front.sessions.clear()
        await front.close()


@pytest.mark.parametrize("method,path,headers,status", [
    ("GET", PATH, {"Host": "evil.example", "X-Forwarded-Host": "forge.example.test"}, 400),
    ("GET", PATH, {"Host": "127.0.0.1:8855"}, 400),
    ("GET", PATH, {"Origin": "https://evil.example"}, 403),
    ("GET", PATH + "?detail=1", {}, 400),
    ("HEAD", PATH + "?auth_token=ignored", {}, 400),
    ("POST", PATH, {"Origin": ORIGIN}, 405),
    ("PUT", PATH, {"Origin": ORIGIN}, 405),
    ("PATCH", PATH, {"Origin": ORIGIN}, 405),
    ("DELETE", PATH, {"Origin": ORIGIN}, 405),
    ("OPTIONS", PATH, {"Origin": ORIGIN}, 405),
])
async def test_availability_refuses_other_authorities_queries_and_methods(tmp_path, method, path, headers, status):
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123", public_origin=ORIGIN,
                         transport=httpx.MockTransport(forbidden))
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=front.application("http://127.0.0.1:61347")),
                                     base_url=ORIGIN) as client:
            response = await client.request(method, path, headers=headers)
            assert response.status_code == status
            assert response.headers["cache-control"] == "no-store"
            assert "set-cookie" not in response.headers
            if status == 405:
                assert response.headers["allow"] == "GET, HEAD"
            assert not front.sessions and not front.login_tokens and not front._logins
    finally:
        await front.close()


@pytest.mark.parametrize("method", ["GET", "HEAD"])
async def test_availability_reports_closing_without_claiming_native_health(tmp_path, method):
    front = ConsoleFront(tmp_path, 8855, "owner", "local-password-123", public_origin=ORIGIN,
                         transport=httpx.MockTransport(forbidden))
    try:
        app = front.application("http://127.0.0.1:61347")
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=app), base_url=ORIGIN) as client:
            # A working front is available even when its native transport would fail.
            assert (await client.request(method, PATH)).status_code == 204
            front._closing = True
            response = await client.request(method, PATH)
            assert response.status_code == 503 and response.content == b""
            assert response.headers["cache-control"] == "no-store"
            assert "set-cookie" not in response.headers
    finally:
        await front.close()
