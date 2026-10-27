"""TDD tests for identity-first runtime APIs (REQ-41).

Tests that identity-first methods exist on MetaRuntime and
delegate to the correct RPC methods with correct argument shapes.
"""
import asyncio
import json
import os
import subprocess
import sys
import threading

import pytest

from meerkat_meta import IdentityBootstrapMode
from meerkat_meta._transport import PersistentTransport, TransportTermination
from meerkat_meta.builder import Meta
from meerkat_meta.errors import NotConnectedError, RpcError, TransportReaderFailedError
from meerkat_meta.identity_first_models import (
    DispatchInput,
    IdentityBootstrapState,
    IdentityBootstrapStatus,
    ImageBlock,
    TextBlock,
)
from meerkat_meta.runtime import MetaRuntime


def _write_test_gateway(tmp_path, body: str):
    gateway = tmp_path / "test_gateway.py"
    gateway.write_text(f"#!{sys.executable}\n{body}")
    gateway.chmod(0o755)
    return gateway


def _tracking_transport_type(*, env: dict[str, str] | None = None):
    class TrackingPersistentTransport(PersistentTransport):
        instances: list["TrackingPersistentTransport"] = []

        def __init__(self, gateway_bin: str):
            super().__init__(gateway_bin, env=env)
            self.spawned_process: subprocess.Popen[bytes] | None = None
            self.reaped_processes: list[subprocess.Popen[bytes]] = []
            TrackingPersistentTransport.instances.append(self)

        def start(self) -> None:
            super().start()
            if self.spawned_process is None:
                self.spawned_process = self._process

        def stop(self) -> None:
            process = self._process
            super().stop()
            if process is not None:
                self.reaped_processes.append(process)

    return TrackingPersistentTransport


class FakeTransport:
    """Records RPC calls for assertion."""

    def __init__(self, result=None):
        self.calls: list[dict] = []
        self.async_timeouts: list[float | None] = []
        self.request_timeout = 60.0
        self._result = result or {}

    def send_sync(self, request):
        self.calls.append(request)
        return {"jsonrpc": "2.0", "id": request.get("id"), "result": self._result}

    async def send_async(self, request, *, timeout=None):
        self.async_timeouts.append(timeout)
        return self.send_sync(request)

    def is_running(self):
        return True

    def start(self):
        pass

    def stop(self):
        pass

    def set_callback_handler(self, handler):
        pass


def _make_runtime(result=None) -> tuple[MetaRuntime, FakeTransport]:
    transport = FakeTransport(result=result)
    rt = MetaRuntime.__new__(MetaRuntime)
    rt._config = None
    rt._transport = transport
    rt._running = True
    rt._rust_http_base = None
    rt._lifecycle_lock = asyncio.Lock()
    rt._shutdown_task = None
    # Import here to avoid circular issues
    from meerkat_meta.agent_builder import CallbackDispatcher
    rt._dispatcher = CallbackDispatcher()
    return rt, transport


def _bootstrap_status_result(**overrides):
    result = {
        "mode": {"mode": "lazy_with_background_warm", "concurrency": 2},
        "complete": False,
        "ready": False,
        "counts": {"dormant": 1, "warming": 1, "active": 0, "broken": 0},
        "identities": {
            "agent:a": {"state": "dormant"},
            "agent:b": {"state": "warming"},
        },
    }
    result.update(overrides)
    return result


class TestIdentityFirstRuntimeAPIs:
    """REQ-41: identity-first methods exist on MetaRuntime."""

    def test_agent_method_exists(self):
        assert hasattr(MetaRuntime, "agent")
        assert callable(getattr(MetaRuntime, "agent"))

    def test_send_method_exists(self):
        assert hasattr(MetaRuntime, "send")
        assert callable(getattr(MetaRuntime, "send"))

    def test_dispatch_method_exists(self):
        assert hasattr(MetaRuntime, "dispatch")
        assert callable(getattr(MetaRuntime, "dispatch"))

    def test_subscribe_method_exists(self):
        assert hasattr(MetaRuntime, "subscribe")
        assert callable(getattr(MetaRuntime, "subscribe"))

    def test_status_method_exists(self):
        assert hasattr(MetaRuntime, "status")
        assert callable(getattr(MetaRuntime, "status"))

    def test_respawn_method_exists(self):
        assert hasattr(MetaRuntime, "respawn")
        assert callable(getattr(MetaRuntime, "respawn"))

    def test_retire_method_exists(self):
        assert hasattr(MetaRuntime, "retire")
        assert callable(getattr(MetaRuntime, "retire"))

    def test_reset_method_exists(self):
        assert hasattr(MetaRuntime, "reset")
        assert callable(getattr(MetaRuntime, "reset"))

    def test_delete_identity_method_exists(self):
        assert hasattr(MetaRuntime, "delete_identity")
        assert callable(getattr(MetaRuntime, "delete_identity"))

    def test_identity_bootstrap_status_method_exists(self):
        assert hasattr(MetaRuntime, "identity_bootstrap_status")
        assert callable(getattr(MetaRuntime, "identity_bootstrap_status"))

    def test_wait_identity_bootstrap_method_exists(self):
        assert hasattr(MetaRuntime, "wait_identity_bootstrap")
        assert callable(getattr(MetaRuntime, "wait_identity_bootstrap"))

    @pytest.mark.asyncio
    async def test_shutdown_keeps_blocking_process_wait_off_event_loop(self):
        rt, transport = _make_runtime()
        stop_started = threading.Event()
        release_stop = threading.Event()

        def blocking_stop():
            stop_started.set()
            if not release_stop.wait(timeout=0.5):
                raise AssertionError("transport.stop blocked the asyncio event loop")

        transport.stop = blocking_stop
        shutdown = asyncio.create_task(rt.shutdown())
        assert await asyncio.to_thread(stop_started.wait, 0.2)

        # This continuation can only run while stop() is blocked if shutdown
        # delegated the subprocess wait to a worker thread.
        await asyncio.sleep(0)
        assert not shutdown.done()
        release_stop.set()
        await shutdown
        assert rt._transport is None

    @pytest.mark.asyncio
    async def test_concurrent_shutdowns_coalesce_and_connect_waits_for_drain(self):
        rt, transport = _make_runtime()
        stop_started = threading.Event()
        release_stop = threading.Event()
        stop_calls = 0

        def blocking_stop():
            nonlocal stop_calls
            stop_calls += 1
            stop_started.set()
            if not release_stop.wait(timeout=0.5):
                raise AssertionError("test did not release transport.stop")

        transport.stop = blocking_stop
        first_shutdown = asyncio.create_task(rt.shutdown())
        assert await asyncio.to_thread(stop_started.wait, 0.2)
        second_shutdown = asyncio.create_task(rt.shutdown())

        with pytest.raises(NotConnectedError) as not_connected:
            await rt._rpc("meta/status", {})
        assert "no transport available" in str(not_connected.value)

        bootstrap_started = asyncio.Event()

        async def fake_bootstrap():
            bootstrap_started.set()
            rt._running = True

        rt._bootstrap = fake_bootstrap
        reconnect = asyncio.create_task(rt.connect())
        await asyncio.sleep(0)
        assert not second_shutdown.done()
        assert not bootstrap_started.is_set()

        release_stop.set()
        await asyncio.gather(first_shutdown, second_shutdown, reconnect)
        assert stop_calls == 1
        assert bootstrap_started.is_set()
        assert rt.is_running

    @pytest.mark.asyncio
    async def test_a_failed_stop_keeps_the_transport_for_the_next_shutdown(self):
        rt, transport = _make_runtime()
        outcomes = [
            RuntimeError(
                "persistent transport: gateway process did not terminate after "
                "bounded cleanup"
            ),
            None,
        ]
        stop_calls = 0

        def stop():
            nonlocal stop_calls
            stop_calls += 1
            outcome = outcomes.pop(0)
            if outcome is not None:
                raise outcome

        transport.stop = stop
        with pytest.raises(RuntimeError, match="did not terminate"):
            await rt.shutdown()

        # Admission stays closed while the runtime keeps the transport whose
        # stop did not succeed.
        assert not rt.is_running
        assert rt._transport is transport
        with pytest.raises(NotConnectedError):
            await rt._rpc("meta/status", {})

        await rt.shutdown()
        assert stop_calls == 2
        assert rt._transport is None
        await rt.shutdown()
        assert stop_calls == 2

    @pytest.mark.asyncio
    async def test_connect_settles_a_retained_transport_before_bootstrapping(self):
        rt, transport = _make_runtime()
        unsettled = RuntimeError(
            "persistent transport: gateway process did not terminate after "
            "bounded cleanup"
        )
        outcomes = [unsettled, unsettled, None]
        stop_calls = 0

        def stop():
            nonlocal stop_calls
            stop_calls += 1
            outcome = outcomes.pop(0)
            if outcome is not None:
                raise outcome

        bootstraps = 0

        async def fake_bootstrap():
            nonlocal bootstraps
            bootstraps += 1
            rt._running = True

        transport.stop = stop
        rt._bootstrap = fake_bootstrap
        with pytest.raises(RuntimeError, match="did not terminate"):
            await rt.shutdown()

        # While the retained stop still fails, connect starts nothing.
        with pytest.raises(RuntimeError, match="did not terminate"):
            await rt.connect()
        assert stop_calls == 2
        assert bootstraps == 0
        assert rt._transport is transport

        # Once that stop succeeds, the same connect bootstraps.
        await rt.connect()
        assert stop_calls == 3
        assert bootstraps == 1
        assert rt._transport is None
        assert rt.is_running

    @pytest.mark.asyncio
    async def test_runtime_reaps_gateway_when_explicit_mode_bypasses_builder_validation(
        self, tmp_path, monkeypatch
    ):
        gateway = _write_test_gateway(
            tmp_path,
            """import json
import sys

for raw_line in sys.stdin:
    request = json.loads(raw_line)
    params = request.get("params", {})
    runtime_options = params.get("runtime_options", {})
    invalid_explicit_mode = (
        "identity_bootstrap_mode" in runtime_options
        and not params.get("has_roster_provider", False)
    )
    if invalid_explicit_mode:
        response = {
            "jsonrpc": "2.0",
            "id": request["id"],
            "error": {
                "code": -32602,
                "message": "identity bootstrap mode requires a roster provider",
            },
        }
    else:
        response = {
            "jsonrpc": "2.0",
            "id": request["id"],
            "result": {"http_base_url": "http://127.0.0.1:1"},
        }
    print(json.dumps(response), flush=True)
""",
        )
        tracking_transport = _tracking_transport_type()
        monkeypatch.setattr(
            "meerkat_meta.runtime.PersistentTransport", tracking_transport
        )

        # Normal MetaBuilder.build() rejects this locally. Constructing the
        # runtime directly keeps the gateway's defensive validation and failed
        # transport reaping covered for callers that bypass the builder.
        config = (
            Meta.builder()
            .gateway(str(gateway))
            .identity_bootstrap_mode(IdentityBootstrapMode.eager_materialize())
            ._config
        )
        runtime = MetaRuntime(config)
        with pytest.raises(RpcError) as rejected:
            await runtime.connect()

        assert rejected.value.code == -32602
        assert len(tracking_transport.instances) == 1
        transport = tracking_transport.instances[0]
        child = transport.spawned_process
        assert child is not None
        assert transport._process is None
        assert transport.reaped_processes == [child]
        assert child.poll() is not None

    @pytest.mark.asyncio
    async def test_connect_retry_waits_for_failed_provider_gateway_to_be_reaped(
        self, tmp_path, monkeypatch
    ):
        gateway = _write_test_gateway(
            tmp_path,
            """import json
import sys

for raw_line in sys.stdin:
    request = json.loads(raw_line)
    callback_id = f"roster:{request['id']}"
    callback = {
        "jsonrpc": "2.0",
        "id": callback_id,
        "method": "callback/roster_provider/roster",
        "params": {"context": {}},
    }
    print(json.dumps(callback), flush=True)
    callback_response = json.loads(sys.stdin.readline())
    if "error" in callback_response:
        response = {
            "jsonrpc": "2.0",
            "id": request["id"],
            "error": {
                "code": -32000,
                "message": callback_response["error"]["message"],
            },
        }
    else:
        response = {
            "jsonrpc": "2.0",
            "id": request["id"],
            "result": {"http_base_url": "http://127.0.0.1:1"},
        }
    print(json.dumps(response), flush=True)
""",
        )

        class FailOnceRoster:
            def __init__(self):
                self.calls = 0

            async def roster(self, context):
                self.calls += 1
                if self.calls == 1:
                    raise RuntimeError("injected roster provider failure")
                return []

        provider = FailOnceRoster()
        tracking_transport = _tracking_transport_type(env=dict(os.environ))
        monkeypatch.setattr(
            "meerkat_meta.runtime.PersistentTransport", tracking_transport
        )
        config = (
            Meta.builder()
            .gateway(str(gateway))
            .roster(provider)
            .identity_bootstrap_mode(IdentityBootstrapMode.lazy_materialize())
            ._config
        )
        runtime = MetaRuntime(config)

        with pytest.raises(RpcError, match="injected roster provider failure"):
            await runtime.connect()

        assert runtime._transport is None
        assert not runtime.is_running
        # The failed gateway exited at stdin EOF; it offered no handshake.
        exited = TransportTermination("exited", 0, None, "none", "not_negotiated")
        assert runtime.last_gateway_termination == exited
        assert len(tracking_transport.instances) == 1
        failed_transport = tracking_transport.instances[0]
        failed_child = failed_transport.spawned_process
        assert failed_child is not None
        assert failed_transport.reaped_processes == [failed_child]
        assert failed_child.poll() is not None

        await runtime.connect()

        assert runtime.is_running
        assert provider.calls == 2
        assert len(tracking_transport.instances) == 2
        live_transport = tracking_transport.instances[1]
        live_child = live_transport.spawned_process
        assert runtime._transport is live_transport
        assert live_child is not None
        assert live_child.poll() is None
        assert failed_transport.reaped_processes == [failed_child]

        await runtime.shutdown()

        assert runtime._transport is None
        assert live_transport.reaped_processes == [live_child]
        assert live_child.poll() is not None
        assert live_transport.last_termination == exited
        assert runtime.last_gateway_termination is live_transport.last_termination


class TestSendMethod:
    """send() accepts str or list[ContentBlock]."""

    @pytest.mark.asyncio
    async def test_send_string_content(self):
        rt, transport = _make_runtime(result={"accepted": True})
        await rt.send("triage:main", "Hello")
        assert len(transport.calls) == 1
        params = transport.calls[0]["params"]
        assert params["identity"] == "triage:main"
        assert params["content"] == "Hello"

    @pytest.mark.asyncio
    async def test_send_block_content(self):
        rt, transport = _make_runtime(result={"accepted": True})
        blocks = [TextBlock(text="Hi")]
        await rt.send("agent:alpha", blocks)
        params = transport.calls[0]["params"]
        assert params["identity"] == "agent:alpha"
        assert params["content"] == [{"type": "text", "text": "Hi"}]

    @pytest.mark.asyncio
    async def test_send_image_block_content_uses_strict_source_shape(self):
        rt, transport = _make_runtime(result={"accepted": True})
        blocks = [ImageBlock(media_type="image/png", data="abc")]
        await rt.send("agent:alpha", blocks)
        params = transport.calls[0]["params"]
        assert params["content"] == [
            {
                "type": "image",
                "media_type": "image/png",
                "source": "inline",
                "data": "abc",
            }
        ]


class TestDispatchMethod:
    """dispatch() accepts DispatchInput."""

    @pytest.mark.asyncio
    async def test_dispatch_sends_correct_params(self):
        rt, transport = _make_runtime(result={"accepted": True})
        di = DispatchInput(content="test", origin="scheduler", correlation_id="c1")
        await rt.dispatch("gate:main", di)
        params = transport.calls[0]["params"]
        assert params["identity"] == "gate:main"
        assert params["dispatch_input"]["content"] == "test"
        assert params["dispatch_input"]["origin"] == "scheduler"
        assert params["dispatch_input"]["correlation_id"] == "c1"

    @pytest.mark.asyncio
    @pytest.mark.parametrize("through_handle", [False, True])
    @pytest.mark.parametrize("with_pair", [False, True])
    async def test_dispatch_text_preserves_caller_idempotency_pair(
        self, through_handle, with_pair
    ):
        rt, transport = _make_runtime(result={"accepted": True})
        pair = (
            {"correlation_id": "lab-event-1", "idempotency_key": "lab:event-1"}
            if with_pair else {}
        )
        if through_handle:
            await rt.agent("gate:main").dispatch_text(
                "Lab closed.\nKeep both paragraphs.", origin="connector", **pair
            )
        else:
            await rt.dispatch_text(
                "gate:main", "Lab closed.\nKeep both paragraphs.",
                origin="connector", **pair,
            )
        assert len(transport.calls) == 1
        assert transport.calls[0]["method"] == "meta/dispatch"
        assert transport.calls[0]["params"] == {
            "identity": "gate:main",
            "dispatch_input": {
                "content": "Lab closed.\nKeep both paragraphs.",
                "origin": "connector",
                **pair,
            },
        }


class TestLifecycleMethods:
    """respawn, retire, reset, delete_identity delegate to RPC."""

    @pytest.mark.asyncio
    async def test_respawn_rpc(self):
        rt, transport = _make_runtime(result={"accepted": True})
        await rt.respawn("agent:1")
        assert transport.calls[0]["method"] == "meta/respawn"
        assert transport.calls[0]["params"]["identity"] == "agent:1"

    @pytest.mark.asyncio
    async def test_retire_rpc(self):
        rt, transport = _make_runtime(result={"accepted": True})
        await rt.retire("agent:1")
        assert transport.calls[0]["method"] == "meta/retire"
        assert transport.calls[0]["params"]["identity"] == "agent:1"

    @pytest.mark.asyncio
    async def test_activate_member_instruction_rpc(self):
        receipt = {"record": {"session_id": "s"}, "disposition": "applied"}
        rt, transport = _make_runtime(result={"identity": "agent:1", "receipt": receipt})
        activation = {
            "revision": {
                "namespace": "host",
                "key": "persona",
                "revision_id": "r2",
                "content_sha256": "0" * 64,
            },
            "activation_id": "a2",
            "expectation": {"kind": "absent"},
            "body": "Use your granted tools.",
        }
        result = await rt.activate_member_instruction("agent:1", activation)
        assert transport.calls[0]["method"] == "meta/member_activate_instruction"
        assert transport.calls[0]["params"] == {"identity": "agent:1", "activation": activation}
        assert result["receipt"]["disposition"] == "applied"
        # Default: the transport's own request timeout.
        assert transport.async_timeouts[0] is None
        # A mid-turn call can block for a whole turn; the caller can widen
        # this call's reply timeout without touching the transport.
        await rt.activate_member_instruction("agent:1", activation, transport_timeout=600.0)
        assert transport.async_timeouts[1] == 600.0

    @pytest.mark.asyncio
    async def test_member_instruction_activations_rpc(self):
        rt, transport = _make_runtime(result={"identity": "agent:1", "page": {"records": []}})
        await rt.member_instruction_activations("agent:1", namespace="host", key="persona")
        assert transport.calls[0]["method"] == "meta/member_instruction_activations"
        assert transport.calls[0]["params"] == {
            "identity": "agent:1",
            "namespace": "host",
            "key": "persona",
        }

    @pytest.mark.asyncio
    async def test_reset_rpc(self):
        rt, transport = _make_runtime(result={"accepted": True})
        await rt.reset("agent:1")
        assert transport.calls[0]["method"] == "meta/reset"
        assert transport.calls[0]["params"]["identity"] == "agent:1"

    @pytest.mark.asyncio
    async def test_delete_identity_rpc(self):
        rt, transport = _make_runtime(result={"accepted": True})
        await rt.delete_identity("agent:1")
        assert transport.calls[0]["method"] == "meta/delete_identity"
        assert transport.calls[0]["params"]["identity"] == "agent:1"


class TestStatusMethod:
    """status() returns IdentityStatus."""

    @pytest.mark.asyncio
    async def test_status_rpc_and_result(self):
        raw_status = {
            "identity": "triage:main",
            "state": "active",
            "agent_runtime_id": "rt-1",
            "session_id": "s-1",
            "profile": "assistant",
            "addressability": "addressable",
            "display_name": "Triage",
            "labels": {"env": "prod"},
            "generation": 0,
            "intent_managed": False,
            "wake_refusal": None,
        }
        rt, transport = _make_runtime(result=raw_status)
        from meerkat_meta.identity_first_models import IdentityStatus
        result = await rt.status("triage:main")
        assert isinstance(result, IdentityStatus)
        assert result.identity == "triage:main"
        assert result.state == "active"
        assert result.labels == {"env": "prod"}


class TestIdentityBootstrapMethods:
    @pytest.mark.asyncio
    async def test_status_uses_distinct_rpc_and_returns_typed_model(self):
        rt, transport = _make_runtime(result=_bootstrap_status_result())

        result = await rt.identity_bootstrap_status()

        assert isinstance(result, IdentityBootstrapStatus)
        assert transport.calls[0]["method"] == "meta/status_identity_bootstrap"
        assert transport.calls[0]["params"] == {}
        assert result.identities["agent:b"].state is IdentityBootstrapState.WARMING

    @pytest.mark.asyncio
    async def test_wait_defaults_to_materialized_target(self):
        rt, transport = _make_runtime(
            result=_bootstrap_status_result(
                complete=True,
                timed_out=False,
                target="materialized",
            )
        )

        result = await rt.wait_identity_bootstrap()

        assert isinstance(result, IdentityBootstrapStatus)
        assert transport.calls[0]["method"] == "meta/wait_identity_bootstrap"
        assert transport.calls[0]["params"] == {
            "target": "materialized",
            "timeout_ms": 60_000,
        }
        assert transport.async_timeouts == [65.0]
        assert result.target == "materialized"

    @pytest.mark.asyncio
    async def test_wait_forwards_startup_ready_target_and_timeout(self):
        rt, transport = _make_runtime(
            result=_bootstrap_status_result(
                timed_out=True,
                target="startup_ready",
                startup_ready=False,
            )
        )

        result = await rt.wait_identity_bootstrap(
            target="startup_ready", timeout=2.5
        )

        assert transport.calls[0]["params"] == {
            "target": "startup_ready",
            "timeout_ms": 2500,
        }
        assert transport.async_timeouts == [7.5]
        assert result.timed_out is True
        assert result.startup_ready is False

    @pytest.mark.asyncio
    async def test_wait_uses_per_request_transport_timeout_beyond_default(self):
        rt, transport = _make_runtime(
            result=_bootstrap_status_result(
                complete=True,
                ready=True,
                target="materialized",
            )
        )

        await rt.wait_identity_bootstrap(timeout=120)

        assert transport.calls[0]["params"]["timeout_ms"] == 120_000
        assert transport.async_timeouts == [125.0]

    @pytest.mark.asyncio
    async def test_wait_rejects_invalid_target_before_rpc(self):
        rt, transport = _make_runtime(result=_bootstrap_status_result())

        with pytest.raises(ValueError, match="target"):
            await rt.wait_identity_bootstrap(target="kickoff")

        assert transport.calls == []

    @pytest.mark.asyncio
    @pytest.mark.parametrize(
        "timeout",
        [-0.1, True, "1", float("nan"), float("inf")],
    )
    async def test_wait_rejects_invalid_timeout_before_rpc(self, timeout):
        rt, transport = _make_runtime(result=_bootstrap_status_result())

        with pytest.raises(ValueError, match="timeout"):
            await rt.wait_identity_bootstrap(timeout=timeout)

        assert transport.calls == []


class TestAgentMethod:
    """agent() returns an AgentHandle."""

    @pytest.mark.asyncio
    async def test_agent_returns_handle(self):
        rt, transport = _make_runtime()
        from meerkat_meta.runtime import IdentityAgentHandle
        handle = rt.agent("triage:main")
        assert isinstance(handle, IdentityAgentHandle)
        assert handle._identity == "triage:main"


class TestSubscribeMethod:
    """subscribe() delegates to RPC."""

    @pytest.mark.asyncio
    async def test_subscribe_rpc(self):
        rt, transport = _make_runtime(result={"stream_id": "stream-1"})
        result = await rt.subscribe("agent:1")
        assert transport.calls[0]["method"] == "meta/subscribe"
        assert transport.calls[0]["params"]["identity"] == "agent:1"


class TestGatewayChildLifetime:
    """An ended gateway child is reported and never silently replaced (#111)."""

    @staticmethod
    def _counting_transport_type():
        class CountingTransport(PersistentTransport):
            instances: list["CountingTransport"] = []

            def __init__(self, gateway_bin: str):
                super().__init__(gateway_bin)
                self.children: list[subprocess.Popen[bytes]] = []
                CountingTransport.instances.append(self)

            def start(self) -> None:
                super().start()
                if self._process is not None and (
                    not self.children or self.children[-1] is not self._process
                ):
                    self.children.append(self._process)

        return CountingTransport

    @pytest.mark.asyncio
    async def test_an_ended_gateway_is_reported_and_not_replaced(self, tmp_path, monkeypatch):
        gateway = _write_test_gateway(
            tmp_path,
            """import json
import sys

for raw_line in sys.stdin:
    request = json.loads(raw_line)
    if request.get("method") == "meta/init":
        print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": {}}), flush=True)
    else:
        sys.exit(7)
""",
        )
        counting = self._counting_transport_type()
        monkeypatch.setattr("meerkat_meta.runtime.PersistentTransport", counting)
        runtime = MetaRuntime(Meta.builder().gateway(str(gateway))._config)
        await runtime.connect()
        assert runtime.is_running
        transport = counting.instances[0]
        try:
            with pytest.raises(TransportReaderFailedError):
                await runtime._rpc("meta/status", {})
            assert not runtime.is_running, "runtime still reports a running gateway"

            # Neither async nor sync admission starts an uninitialized child.
            with pytest.raises(TransportReaderFailedError):
                await runtime._rpc("meta/status", {})
            with pytest.raises(TransportReaderFailedError):
                await asyncio.to_thread(runtime._rpc_sync, "meta/status", {})
            assert len(transport.children) == 1, "a request started a replacement gateway"

            ended = await runtime.wait_gateway_ended()
            assert ended is not None and ended.exit_code == 7
        finally:
            await runtime.shutdown()

        # Shutdown detaches admission but keeps the observation.
        with pytest.raises(NotConnectedError):
            await runtime._rpc("meta/status", {})
        assert runtime.gateway_ended is ended
        assert len(transport.children) == 1

    @pytest.mark.asyncio
    async def test_a_childless_runtime_runs_without_a_gateway_record(self):
        class Builder:
            async def build_agent(self, options):
                return None

        runtime = MetaRuntime(Meta.builder().session_service(Builder())._config)
        await runtime.connect()
        try:
            assert runtime.is_running
            assert runtime.gateway_ended is None
        finally:
            await runtime.shutdown()
