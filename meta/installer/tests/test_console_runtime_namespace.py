"""Inert authenticated HTTP peers for the selected native Console method."""
import json

import httpx
import jwt
import pytest

from launchkit.templates_v2.console_auth import AUDIENCE, ISSUER, ConsoleFront


SUBMISSION = {"identity": "lead:main", "content": "Keep the complete proposal: Å.",
              "origin": "browser-forged-origin", "principal": "browser-forged-principal",
              "idempotency_key": "submission-1", "handling_mode": "queue"}
ACCEPTED = {"identity": "lead:main", "interaction_id": "native-interaction-1",
            "session_id": "native-session-1", "input_frame_id": "native-frame-1",
            "cursor": "42", "status": "queued", "conversation_id": "native-conversation-1"}


async def login(client, front):
    assert (await client.get("/login")).status_code == 200
    nonce = client.cookies[front.cookie + "_login"]
    response = await client.post("/login", data={"nonce": nonce, "password": "local-password-123"},
                                 headers={"Origin": front.origin})
    assert response.status_code == 303


def request_body(path, namespace):
    value = SUBMISSION if path == "/console/send" else {
        "jsonrpc": "2.0", "id": "1", "method": namespace + "/console/send", "params": SUBMISSION}
    payload = json.dumps(value, ensure_ascii=False, separators=(", ", ": ")).encode("utf-8") + b"\n "
    if path != "/console/rpc/multipart":
        return payload, "application/json; charset=utf-8"
    boundary = b"forge-inert-namespace-boundary"
    raw = (b"--" + boundary + b'\r\nContent-Disposition: form-data; name="file:upload-1"; filename="sky.bin"'
           b"\r\nContent-Type: application/octet-stream\r\n\r\n\x00\xffnative-upload\r\n--" + boundary
           + b'\r\nContent-Disposition: form-data; name="payload"\r\n\r\n' + payload
           + b"\r\n--" + boundary + b"--\r\n")
    return raw, "multipart/form-data; boundary=" + boundary.decode()


def front_for(tmp_path, selected, delivered, native):
    options = {} if selected is None else {"runtime_namespace": selected}
    return ConsoleFront(tmp_path, 8855, "owner@example.test", "local-password-123", delivered.append,
                        transport=httpx.MockTransport(native), **options)


@pytest.mark.parametrize("selected", [None, "mobkit", "meta"])
@pytest.mark.parametrize("method_namespace", ["mobkit", "meta"])
@pytest.mark.parametrize("path", ["/console/rpc", "/console/rpc/multipart"])
async def test_only_selected_rpc_delivers_and_preserves_native_bytes_and_auth(
        tmp_path, selected, method_namespace, path):
    delivered, observed = [], []
    native_payload = json.dumps({"jsonrpc": "2.0", "id": "1", "result": ACCEPTED},
                                separators=(", ", ": ")).encode() + b"\n"

    def native(request):
        observed.append(request)
        return httpx.Response(200, content=native_payload, headers={"Content-Type": "application/json"})

    front = front_for(tmp_path, selected, delivered, native)
    raw, content_type = request_body(path, method_namespace)
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(
                app=front.application("http://127.0.0.1:61347")), base_url=front.origin) as client:
            await login(client, front)
            current_login = front._logins[client.cookies[front.cookie]]
            response = await client.post(path, content=raw, headers={"Origin": front.origin,
                "Content-Type": content_type, "Authorization": "Bearer browser-forged-token"})
            assert response.status_code == 200 and response.content == native_payload
            assert len(observed) == 1
            forwarded = observed[0]
            assert forwarded.content == raw and forwarded.headers["content-type"] == content_type
            assert forwarded.url.path == path and forwarded.url.host == "127.0.0.1" and forwarded.url.port == 61347
            assert "cookie" not in forwarded.headers and "origin" not in forwarded.headers
            token = forwarded.headers["authorization"].removeprefix("Bearer ")
            assert token != "browser-forged-token"
            claims = jwt.decode(token, front.key, algorithms=["HS256"], issuer=ISSUER, audience=AUDIENCE)
            assert claims["sub"] == "owner@example.test" and claims["sid"] == current_login.login_scope_id
            assert claims["host_scope"] == "native-console"
            if method_namespace != (selected or "mobkit"):
                assert delivered == []
            else:
                assert len(delivered) == 1
                event = delivered[0]
                assert event.principal == "owner@example.test" and event.channel == "host-authenticated-console"
                assert event.content == SUBMISSION["content"] and event.handling_mode == "queue"
                for field in ("identity", "interaction_id", "session_id", "input_frame_id", "cursor", "conversation_id"):
                    assert getattr(event, field) == ACCEPTED[field]
                # The same native interaction remains one application delivery.
                assert (await client.post(path, content=raw, headers={"Origin": front.origin,
                    "Content-Type": content_type})).status_code == 200
                assert len(delivered) == 1
    finally:
        await front.close()


@pytest.mark.parametrize("selected", [None, "mobkit", "meta"])
async def test_normal_console_send_keeps_native_admission_under_selected_namespace(tmp_path, selected):
    delivered, observed = [], []
    native_payload = json.dumps(ACCEPTED).encode()

    def native(request):
        observed.append(request)
        return httpx.Response(200, content=native_payload)

    front = front_for(tmp_path, selected, delivered, native)
    raw, content_type = request_body("/console/send", "meta")
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(
                app=front.application("http://127.0.0.1:61347")), base_url=front.origin) as client:
            await login(client, front)
            response = await client.post("/console/send", content=raw,
                headers={"Origin": front.origin, "Content-Type": content_type})
            assert response.status_code == 200 and response.content == native_payload
            assert len(observed) == 1 and observed[0].content == raw
            assert len(delivered) == 1 and delivered[0].principal == "owner@example.test"
    finally:
        await front.close()


@pytest.mark.parametrize("selected", [None, "META", "meta/console/send", "", "meerkat_meta", ["meta"], 1, True])
def test_invalid_constructor_namespace_fails_before_private_key_creation(tmp_path, selected):
    with pytest.raises(ValueError, match="runtime namespace"):
        ConsoleFront(tmp_path, 8855, "owner", "local-password-123", runtime_namespace=selected)
    assert not (tmp_path / ".mobkit").exists()


async def test_meta_namespace_preserves_login_and_csrf_admission_guards(tmp_path):
    delivered, observed = [], []
    front = front_for(tmp_path, "meta", delivered,
                      lambda request: observed.append(request) or httpx.Response(200, json={"result": ACCEPTED}))
    raw, content_type = request_body("/console/rpc", "meta")
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(
                app=front.application("http://127.0.0.1:61347")), base_url=front.origin) as client:
            headers = {"Origin": front.origin, "Content-Type": content_type}
            assert (await client.post("/console/rpc", content=raw, headers=headers)).status_code == 401
            await login(client, front)
            for origin in (None, "https://foreign.example", "null"):
                headers = {"Content-Type": content_type}
                if origin is not None:
                    headers["Origin"] = origin
                assert (await client.post("/console/rpc", content=raw, headers=headers)).status_code == 403
            assert observed == [] and delivered == []
    finally:
        await front.close()


@pytest.mark.parametrize("outcome", ["error", "http-error", "missing-evidence", "foreign-identity"])
async def test_selected_meta_does_not_invent_native_admission(tmp_path, outcome):
    accepted = dict(ACCEPTED)
    status = 200
    if outcome == "missing-evidence":
        accepted.pop("input_frame_id")
    elif outcome == "foreign-identity":
        accepted["identity"] = "foreign:main"
    elif outcome == "http-error":
        status = 403
    envelope = {"error": {"code": -32601, "message": "Native method rejected."}} if outcome == "error" else {"result": accepted}
    native_payload = json.dumps(envelope).encode()
    delivered = []
    front = front_for(tmp_path, "meta", delivered,
                      lambda request: httpx.Response(status, content=native_payload))
    raw, content_type = request_body("/console/rpc", "meta")
    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(
                app=front.application("http://127.0.0.1:61347")), base_url=front.origin) as client:
            await login(client, front)
            response = await client.post("/console/rpc", content=raw,
                headers={"Origin": front.origin, "Content-Type": content_type})
            expected = 503 if outcome in ("missing-evidence", "foreign-identity") else status
            assert response.status_code == expected and delivered == []
            if expected != 503:
                assert response.content == native_payload
    finally:
        await front.close()
