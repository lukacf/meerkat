"""wire_cross_mob between two real gateway processes.

Two ``MetaRuntime`` objects are two ``rpc_gateway`` processes. Each mob has
one member with a socket endpoint (``member_comms_tcp()``), built through the
self-hosted model route with no credential, so the rows need no provider key.
The model is a local chat-completions fake: a user message
``SCRIPTED-SEND peer_id=<id> body=<token>`` makes the member call the comms
``send_message`` tool, and every other request is recorded and answered with
text. A delivery is a recorded request of the OTHER gateway's member that
carries the token as a peer message.
"""

from __future__ import annotations

import asyncio
import json
import os
import re
import socket
import subprocess
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest

from meerkat_meta import MEMBER_STATE_ACTIVE, MEMBER_STATE_BROKEN, Meta
from meerkat_meta.errors import RpcError


def _resolve_gateway_bin() -> str:
    override = os.environ.get("META_GATEWAY_BIN", "").strip()
    if override:
        return override
    repo_root = Path(__file__).resolve().parents[3]
    try:
        result = subprocess.run(
            [str(repo_root / "scripts" / "repo-cargo"), "--print-env", "CARGO_TARGET_DIR"],
            cwd=repo_root,
            check=True,
            capture_output=True,
            text=True,
        )
    except (OSError, subprocess.CalledProcessError):
        return ""
    for line in result.stdout.splitlines():
        if line.startswith("CARGO_TARGET_DIR="):
            target_dir = line.split("=", 1)[1].strip()
            if target_dir:
                return str(Path(target_dir) / "debug" / "rpc_gateway")
    return ""


_GATEWAY_BIN = _resolve_gateway_bin()

pytestmark = pytest.mark.skipif(
    not (_GATEWAY_BIN and os.path.isfile(_GATEWAY_BIN)),
    reason="META_GATEWAY_BIN not set to a built rpc_gateway",
)

_MODEL = "wire-cross-mob-fake"
_TRIGGER = re.compile(r"SCRIPTED-SEND peer_id=([0-9a-f-]+) body=([a-z0-9-]+)")


def _mob_toml(mob_id: str) -> str:
    return f"""\
[mob]
id = "{mob_id}"

[profiles.worker]
model = "{_MODEL}"
provider = "self_hosted"
self_hosted_server_id = "local"
external_addressable = true

[profiles.worker.tools]
comms = true
"""


def _config_toml(base_url: str) -> str:
    return f"""\
[self_hosted]
default_model = "{_MODEL}"

[self_hosted.servers.local]
transport = "openai_compatible"
base_url = "{base_url}"
api_style = "chat_completions"

[self_hosted.models.{_MODEL}]
server = "local"
remote_model = "{_MODEL}"

[realm.global]
default_binding = "local"

[realm.global.backend.local]
provider = "self_hosted"
backend_kind = "self_hosted"
server = "local"

[realm.global.auth.local_auth]
provider = "self_hosted"
auth_method = "none"
source = {{ kind = "platform_default" }}

[realm.global.binding.local]
backend_profile = "local"
auth_profile = "local_auth"
"""


class _FakeModel:
    """A chat-completions endpoint that scripts one tool call per trigger."""

    def __init__(self) -> None:
        self.requests: list[str] = []
        self._sent: set[str] = set()
        self._lock = threading.Lock()
        fake = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *args) -> None:  # quiet
                return

            def do_POST(self) -> None:
                length = int(self.headers.get("Content-Length", "0"))
                body = json.loads(self.rfile.read(length) or b"{}")
                chunks = fake.answer(body)
                if body.get("stream"):
                    self.send_response(200)
                    self.send_header("Content-Type", "text/event-stream")
                    self.end_headers()
                    for chunk in chunks:
                        self.wfile.write(f"data: {json.dumps(chunk)}\n\n".encode())
                    self.wfile.write(b"data: [DONE]\n\n")
                    return
                payload = json.dumps(fake.completion(chunks)).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)

        self._server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.base_url = f"http://127.0.0.1:{self._server.server_address[1]}"
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)
        self._thread.start()

    def close(self) -> None:
        self._server.shutdown()
        self._server.server_close()

    def answer(self, body: dict) -> list[dict]:
        text = json.dumps(body.get("messages", []))
        triggers = _TRIGGER.findall(text)
        with self._lock:
            if triggers and triggers[-1][1] not in self._sent:
                peer_id, token = triggers[-1]
                self._sent.add(token)
                delta = {
                    "role": "assistant",
                    "tool_calls": [
                        {
                            "index": 0,
                            "id": f"call-{token}",
                            "type": "function",
                            "function": {
                                "name": "send_message",
                                "arguments": json.dumps(
                                    {"peer_id": peer_id, "body": token, "handling_mode": "queue"}
                                ),
                            },
                        }
                    ],
                }
                finish = "tool_calls"
            else:
                self.requests.append(text)
                delta = {"role": "assistant", "content": "observed"}
                finish = "stop"
        base = {"id": "fake", "object": "chat.completion.chunk", "created": 1, "model": _MODEL}
        return [
            {**base, "choices": [{"index": 0, "delta": delta, "finish_reason": None}]},
            {**base, "choices": [{"index": 0, "delta": {}, "finish_reason": finish}]},
            {
                **base,
                "choices": [],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
            },
        ]

    @staticmethod
    def completion(chunks: list[dict]) -> dict:
        return {
            "id": "fake",
            "object": "chat.completion",
            "created": 1,
            "model": _MODEL,
            "choices": [
                {
                    "index": 0,
                    "message": chunks[0]["choices"][0]["delta"],
                    "finish_reason": chunks[1]["choices"][0]["finish_reason"],
                }
            ],
            "usage": chunks[2]["usage"],
        }

    def delivered(self, token: str) -> bool:
        """A member was handed ``token`` as a peer message: a recorded request
        that carries it and is not the sender's own trigger for it."""
        with self._lock:
            return any(
                token in text and f"body={token}" not in text for text in self.requests
            )


    def sender_result(self, token: str) -> str | None:
        """What the sending member's model was handed after its
        ``send_message`` call for ``token``: the rest of the request that
        follows the trigger (the tool call and its result)."""
        with self._lock:
            for text in reversed(self.requests):
                if f"body={token}" in text:
                    return text.rsplit(f"body={token}", 1)[-1]
        return None


def _free_loopback_address() -> str:
    """A loopback address with a port that is free now: a FIXED member
    address a gateway can bind again after a restart."""
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        probe.bind(("127.0.0.1", 0))
        return f"127.0.0.1:{probe.getsockname()[1]}"


async def _gateway(
    tmp_path: Path,
    mob_id: str,
    base_url: str,
    *,
    socket_endpoints: bool,
    address: str | None = None,
    persistent: bool = False,
):
    """Start a gateway for ``mob_id``. With ``persistent`` its state lives
    under the mob's directory, so a second call for the same ``mob_id`` is a
    restart on that state."""
    root = tmp_path / mob_id
    root.mkdir(exist_ok=True)
    (root / "mob.toml").write_text(_mob_toml(mob_id))
    (root / "config.toml").write_text(_config_toml(base_url))
    builder = (
        Meta.builder()
        .gateway(_GATEWAY_BIN)
        .mob(str(root / "mob.toml"))
        .meerkat_config(root / "config.toml")
    )
    if socket_endpoints:
        builder = (
            builder.member_comms_tcp(address)
            if address is not None
            else builder.member_comms_tcp()
        )
    if persistent:
        builder = builder.persistent_state(str(root / "state"))
    return await builder.build()


async def _delivered(fake: _FakeModel, token: str, seconds: float = 45.0) -> bool:
    deadline = asyncio.get_running_loop().time() + seconds
    while asyncio.get_running_loop().time() < deadline:
        if fake.delivered(token):
            return True
        await asyncio.sleep(0.2)
    return False


@pytest.mark.asyncio
async def test_wire_cross_mob_between_two_gateway_processes_delivers_both_ways(tmp_path):
    fake = _FakeModel()
    left = right = None
    try:
        left = await _gateway(tmp_path, "xgw-socket-a", fake.base_url, socket_endpoints=True)
        right = await _gateway(tmp_path, "xgw-socket-b", fake.base_url, socket_endpoints=True)
        a, b = left.mob_handle(), right.mob_handle()
        await a.ensure_member("alice", "worker")
        await b.ensure_member("bob", "worker")

        alice = await a.peer_info("alice")
        bob = await b.peer_info("bob")
        for info in (alice, bob):
            assert info["dialable_address"].startswith("tcp://127.0.0.1:"), info
            assert not info["dialable_address"].endswith(":0"), info
            assert info["pubkey_b64"], info

        await a.wire_cross_mob("alice", "bob", b)
        assert bob["comms_name"] in (await a.get_member("alice")).wired_to
        assert alice["comms_name"] in (await b.get_member("bob")).wired_to

        await a.send("alice", f"SCRIPTED-SEND peer_id={bob['peer_id']} body=xgw-a-to-b")
        assert await _delivered(fake, "xgw-a-to-b"), fake.requests[-3:]
        await b.send("bob", f"SCRIPTED-SEND peer_id={alice['peer_id']} body=xgw-b-to-a")
        assert await _delivered(fake, "xgw-b-to-a"), fake.requests[-3:]

        await a.unwire_local(
            "alice",
            bob["comms_name"],
            bob["peer_id"],
            bob["dialable_address"],
            remote_pubkey_b64=bob["pubkey_b64"],
        )
        await b.unwire_local(
            "bob",
            alice["comms_name"],
            alice["peer_id"],
            alice["dialable_address"],
            remote_pubkey_b64=alice["pubkey_b64"],
        )
        assert bob["comms_name"] not in (await a.get_member("alice")).wired_to
        assert alice["comms_name"] not in (await b.get_member("bob")).wired_to
    finally:
        for runtime in (left, right):
            if runtime is not None:
                await runtime.shutdown()
        fake.close()


@pytest.mark.asyncio
async def test_wire_cross_mob_between_two_gateways_without_endpoints_is_refused_plainly(
    tmp_path,
):
    fake = _FakeModel()
    left = right = None
    try:
        left = await _gateway(tmp_path, "xgw-plain-a", fake.base_url, socket_endpoints=False)
        right = await _gateway(tmp_path, "xgw-plain-b", fake.base_url, socket_endpoints=False)
        a, b = left.mob_handle(), right.mob_handle()
        await a.ensure_member("alice", "worker")
        await b.ensure_member("bob", "worker")
        alice = await a.peer_info("alice")
        bob = await b.peer_info("bob")
        # No socket endpoint: the field is absent, never null.
        assert "dialable_address" not in alice and "dialable_address" not in bob

        with pytest.raises(RpcError) as refusal:
            await a.wire_cross_mob("alice", "bob", b)
        message = str(refusal.value)
        assert "is not a live member of a mob in this process" in message, message
        assert "member_comms_address" in message, message
        assert "must be non-zero" not in message, message

        # Nothing was wired on either side.
        assert (await a.get_member("alice")).wired_to == []
        assert (await b.get_member("bob")).wired_to == []
    finally:
        for runtime in (left, right):
            if runtime is not None:
                await runtime.shutdown()
        fake.close()


async def _exchange(fake: _FakeModel, sender, member: str, peer_id: str, token: str) -> bool:
    """``member`` calls ``send_message`` to ``peer_id``; whether the other
    gateway's member was handed ``token``."""
    await sender.send(member, f"SCRIPTED-SEND peer_id={peer_id} body={token}")
    return await _delivered(fake, token)


async def _sender_result(fake: _FakeModel, token: str, seconds: float = 45.0) -> str:
    """The tool result the sending member's model was handed for ``token``."""
    deadline = asyncio.get_running_loop().time() + seconds
    while asyncio.get_running_loop().time() < deadline:
        result = fake.sender_result(token)
        if result is not None:
            return result
        await asyncio.sleep(0.2)
    raise AssertionError(f"the sender's model was never handed a result for {token}")


@pytest.mark.asyncio
async def test_a_wire_between_gateways_with_fixed_member_addresses_survives_a_restart(
    tmp_path,
):
    """Persistent state, one member per gateway, each on a FIXED member
    address: one gateway is shut down and started again on its state, and
    the wire still delivers both ways with no second wire."""
    fake = _FakeModel()
    left = right = None
    right_address = _free_loopback_address()

    async def start_right():
        return await _gateway(
            tmp_path,
            "xgw-fixed-b",
            fake.base_url,
            socket_endpoints=True,
            address=right_address,
            persistent=True,
        )

    try:
        left = await _gateway(
            tmp_path,
            "xgw-fixed-a",
            fake.base_url,
            socket_endpoints=True,
            address=_free_loopback_address(),
            persistent=True,
        )
        right = await start_right()
        a, b = left.mob_handle(), right.mob_handle()
        await a.ensure_member("alice", "worker")
        await b.ensure_member("bob", "worker")
        alice = await a.peer_info("alice")
        bob = await b.peer_info("bob")
        assert bob["dialable_address"] == f"tcp://{right_address}", bob

        await a.wire_cross_mob("alice", "bob", b)
        assert await _exchange(fake, a, "alice", bob["peer_id"], "xgw-fixed-before-a-to-b")
        assert await _exchange(fake, b, "bob", alice["peer_id"], "xgw-fixed-before-b-to-a")

        await right.shutdown()
        right = None
        right = await start_right()
        b = right.mob_handle()

        # The restarted member is the member the other side wired: same
        # endpoint, same key, still active, the edge still there.
        restarted = await b.get_member("bob")
        assert restarted.state == MEMBER_STATE_ACTIVE, restarted
        assert alice["comms_name"] in restarted.wired_to, restarted
        other = await a.get_member("alice")
        assert other.state == MEMBER_STATE_ACTIVE, other
        assert bob["comms_name"] in other.wired_to, other
        assert await b.peer_info("bob") == bob

        # No second wire: both directions deliver.
        assert await _exchange(fake, a, "alice", bob["peer_id"], "xgw-fixed-after-a-to-b")
        assert await _exchange(fake, b, "bob", alice["peer_id"], "xgw-fixed-after-b-to-a")

        # The helper again finds the pair wired as it is and changes nothing.
        await a.wire_cross_mob("alice", "bob", b)
        assert (await a.get_member("alice")).wired_to == [bob["comms_name"]]
        assert (await b.get_member("bob")).wired_to == [alice["comms_name"]]
        assert await _exchange(fake, a, "alice", bob["peer_id"], "xgw-fixed-again-a-to-b")
        assert await _exchange(fake, b, "bob", alice["peer_id"], "xgw-fixed-again-b-to-a")
    finally:
        for runtime in (left, right):
            if runtime is not None:
                await runtime.shutdown()
        fake.close()


@pytest.mark.asyncio
async def test_a_wire_between_gateways_with_kernel_picked_ports_is_lost_at_a_restart_and_recovered(
    tmp_path,
):
    """A pinned limit. Persistent state with ``member_comms_tcp()``'s default
    address (a port the kernel picks): after a restart the member binds
    another port, is restored broken, and the wire is dead. The way back,
    with nothing but what each gateway answers after the restart: respawn
    the member, unwire both sides, wire again."""
    fake = _FakeModel()
    left = right = None

    async def start_right():
        return await _gateway(
            tmp_path, "xgw-moved-b", fake.base_url, socket_endpoints=True, persistent=True
        )

    try:
        left = await _gateway(
            tmp_path, "xgw-moved-a", fake.base_url, socket_endpoints=True, persistent=True
        )
        right = await start_right()
        a, b = left.mob_handle(), right.mob_handle()
        await a.ensure_member("alice", "worker")
        await b.ensure_member("bob", "worker")
        alice = await a.peer_info("alice")
        bob = await b.peer_info("bob")
        await a.wire_cross_mob("alice", "bob", b)
        assert await _exchange(fake, a, "alice", bob["peer_id"], "xgw-moved-before-a-to-b")

        await right.shutdown()
        right = None
        right = await start_right()
        b = right.mob_handle()

        # The member is broken, not only the edge: its new port is not the
        # endpoint its mob recorded.
        broken = await b.get_member("bob")
        assert broken.state == MEMBER_STATE_BROKEN, broken
        status = await b.member_status("bob")
        assert "respawn the member" in (status.error or ""), status
        with pytest.raises(RpcError):
            await b.peer_info("bob")
        # The other side still holds the edge and sends to the old port.
        assert bob["comms_name"] in (await a.get_member("alice")).wired_to
        await a.send(
            "alice", f"SCRIPTED-SEND peer_id={bob['peer_id']} body=xgw-moved-dead-a-to-b"
        )
        dead = await _sender_result(fake, "xgw-moved-dead-a-to-b")
        assert "peer_message_sent" not in dead, dead[-600:]
        assert not fake.delivered("xgw-moved-dead-a-to-b")

        # 1. Respawn the broken member: active again on a new port. (A
        # respawn starts the member on a fresh session.)
        await b.respawn_member("bob")
        assert (await b.get_member("bob")).state == MEMBER_STATE_ACTIVE
        moved = await b.peer_info("bob")
        assert moved["dialable_address"] != bob["dialable_address"], moved

        # 2. The helper alone does not replace the edge: the other side is
        # wired to the old endpoint, and a wire is never replaced. The
        # refusal says so in plain words, and nothing changes.
        with pytest.raises(RpcError) as refusal:
            await a.wire_cross_mob("alice", "bob", b)
        message = str(refusal.value)
        assert "already wired" in message and "unwire" in message, message
        assert "Running -> Running" not in message, message
        assert (await a.get_member("alice")).wired_to == [bob["comms_name"]]
        assert (await b.get_member("bob")).wired_to == [alice["comms_name"]]

        # 3. Unwire both sides, each by the other member's CURRENT peer info.
        await a.unwire_local(
            "alice",
            moved["comms_name"],
            moved["peer_id"],
            moved["dialable_address"],
            remote_pubkey_b64=moved["pubkey_b64"],
        )
        await b.unwire_local(
            "bob",
            alice["comms_name"],
            alice["peer_id"],
            alice["dialable_address"],
            remote_pubkey_b64=alice["pubkey_b64"],
        )
        assert (await a.get_member("alice")).wired_to == []
        assert (await b.get_member("bob")).wired_to == []

        # 4. Wire again: both directions deliver.
        await a.wire_cross_mob("alice", "bob", b)
        assert await _exchange(fake, a, "alice", moved["peer_id"], "xgw-moved-after-a-to-b")
        assert await _exchange(fake, b, "bob", alice["peer_id"], "xgw-moved-after-b-to-a")
    finally:
        for runtime in (left, right):
            if runtime is not None:
                await runtime.shutdown()
        fake.close()
