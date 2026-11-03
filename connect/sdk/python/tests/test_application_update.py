"""Real private-pipe and SDK composition controls; no native Ready/finish proof."""
import base64
import asyncio
import dataclasses
import hashlib
import io
import json
import os
import sys
import threading
from concurrent.futures import ThreadPoolExecutor
from types import SimpleNamespace

import pytest

from meerkat_toolkit.host_control import ApplicationPreparation, ApplicationUpdateStartup, HostControlError, HostReplyError, HostTransportError, PrivateHostClient, SelectedHostArtifact, ToolkitMobKitRuntime, ToolkitPersistentTransport
from meerkat_toolkit.application_update import ToolkitApplicationUpdateRuntime
from test_host_control import pipes, receive, respond


def selection(path="/selected/update.json", digest="a" * 64):
    return ApplicationUpdateStartup(SelectedHostArtifact(path, digest),
        SelectedHostArtifact("/selected/prior.json", "b" * 64), "/selected/associations")


def test_first_private_selection_then_same_channel_control_and_no_repeat():
    client, reader, writer = pipes()
    try:
        with ThreadPoolExecutor(max_workers=1) as executor:
            pending = executor.submit(client.replace_application, selection())
            request = receive(reader)
            assert request == {"version": 1, "id": 1, "command": {
                "kind": "replace_application", "startup": dataclasses.asdict(selection())}}
            assert not pending.done()
            respond(writer, request, {"kind": "bootstrap_accepted"})
            pending.result(timeout=2)
            opened = executor.submit(client.open_component, "next")
            next_request = receive(reader)
            assert next_request["id"] == 2 and next_request["command"]["kind"] == "open_component"
            respond(writer, next_request, {"kind": "component", "observation": {
                "incarnation": "00000000-0000-0000-0000-000000000001", "component": "next", "revision": 1, "state": "open"}})
            opened.result(timeout=2)
        with pytest.raises(HostControlError): client.replace_application(selection())
    finally:
        client.close(); assert reader.read() == b""; reader.close(); writer.close()


@pytest.mark.parametrize("reply", [
    {"kind": "error", "error": "denied"},
    {"kind": "application_prepared", "manifest": {"path": "/not-finished", "sha256": "d" * 64}},
    {"kind": "bootstrap_accepted", "ready": True},
])
def test_refused_or_wrong_phase_reply_never_becomes_reusable_startup(reply):
    client, reader, writer = pipes()
    try:
        with ThreadPoolExecutor(max_workers=1) as executor:
            pending = executor.submit(client.replace_application, selection())
            request = receive(reader); respond(writer, request, reply)
            with pytest.raises(HostControlError): pending.result(timeout=2)
        with pytest.raises(HostControlError): client.replace_application(selection())
    finally:
        client.close(); assert reader.read() == b""; reader.close(); writer.close()


def test_runtime_carries_identical_bytes_and_native_revision_without_second_inputs(tmp_path, monkeypatch):
    path = tmp_path / "selection.json"; selected = b'{"update_id":"retained-transaction"}\n'; path.write_bytes(selected)
    startup = selection(str(path), hashlib.sha256(selected).hexdigest())
    runtime = ToolkitApplicationUpdateRuntime(SimpleNamespace(gateway_bin="/selected/gateway"), startup)
    params = {"runtime_options": {"declare_spec_update": {"expected_revision": 11}}}
    monkeypatch.setattr(ToolkitMobKitRuntime, "_build_init_params", lambda self: dict(params))
    actual = runtime._build_init_params()
    assert actual == {**params, "prepared_application": {
        "bundle_base64": base64.b64encode(selected).decode("ascii"), "digest": startup.selection.sha256}}
    path.write_bytes(b"changed after selection")
    assert runtime._build_init_params() == actual
    transport = runtime._make_gateway_transport()
    assert isinstance(transport, ToolkitPersistentTransport) and transport._selection == startup
    with pytest.raises(HostControlError, match="already started"): runtime._make_gateway_transport()
    for key in ("mob_config", "application_tool_policies", "prepared_application"):
        monkeypatch.setattr(ToolkitMobKitRuntime, "_build_init_params", lambda self, key=key: {key: {}})
        with pytest.raises(HostControlError, match="conflicts"): runtime._build_init_params()


def test_malformed_pin_and_symlink_refuse_before_a_native_transport_exists(tmp_path):
    for startup in [lambda: selection("relative"), lambda: selection("/bad\0name"),
                    lambda: dataclasses.replace(selection(), association_directory="/bad\0directory")]:
        with pytest.raises(HostControlError): startup()
    source = tmp_path / "source"; source.write_bytes(b"selected")
    with pytest.raises(HostControlError): ToolkitApplicationUpdateRuntime(SimpleNamespace(gateway_bin="/selected/gateway"), selection(str(source)))
    alias = tmp_path / "alias"; alias.symlink_to(source)
    with pytest.raises(OSError): ToolkitApplicationUpdateRuntime(SimpleNamespace(gateway_bin="/selected/gateway"), selection(str(alias), hashlib.sha256(b"selected").hexdigest()))


class _InertNativeChild:
    """Exercise the real SDK send/response/stop methods without a gateway."""

    def __init__(self, transport, before_wait=lambda: None, *, exit_code=0, exit_after_signals=0,
                 shutdown_result=None):
        self.requests = []
        self.returncode = None
        self.waits = 0
        self.before_wait = before_wait
        self.exit_code = exit_code
        self.exit_after_signals = exit_after_signals
        self.wait_error = OSError("inert child exit is unobserved")
        self.signals = []
        owner = self

        class Input(io.BytesIO):
            def write(self, data):
                written = super().write(data)
                request = json.loads(data)
                owner.requests.append(request)
                transport._read_line(json.dumps({
                    "jsonrpc": "2.0", "id": request["id"],
                    "result": shutdown_result if request["method"] == "meta/shutdown" else {"inert": True},
                }).encode(), process=owner)
                return written

        self.stdin = Input()

    def poll(self):
        return self.returncode

    def wait(self, timeout=None):
        assert self.stdin.closed
        self.before_wait()
        self.waits += 1
        if self.exit_code is None or len(self.signals) < self.exit_after_signals:
            raise self.wait_error
        self.returncode = self.exit_code
        return self.returncode

    def terminate(self):
        self.signals.append("terminate")

    def kill(self):
        self.signals.append("kill")


def test_private_failure_after_acceptance_fences_queued_native_init_worker():
    # The native SDK's actual send_async queues self.send_sync in a worker.
    # Occupy that worker before scheduling init, then fail the real private
    # decoder. An async-caller-only check would have already passed.
    async def scenario():
        client, reader, writer = pipes()
        transport = ToolkitPersistentTransport("/inert/not-spawned", selection(), timeout=0.1)
        child = _InertNativeChild(transport)
        transport._process, transport.host_control = child, client
        release, submitted = threading.Event(), threading.Event()

        class QueuedExecutor(ThreadPoolExecutor):
            calls = 0
            def submit(self, fn, /, *args, **kwargs):
                future = super().submit(fn, *args, **kwargs)
                self.calls += 1
                if self.calls == 2:
                    submitted.set()
                return future

        loop = asyncio.get_running_loop()
        loop.set_default_executor(QueuedExecutor(max_workers=1))
        blocked = loop.run_in_executor(None, release.wait)
        sending = None
        try:
            with ThreadPoolExecutor(max_workers=1) as private:
                accepting = private.submit(client.replace_application, selection())
                first = receive(reader)
                assert first["id"] == 1 and first["command"]["kind"] == "replace_application"
                respond(writer, first, {"kind": "bootstrap_accepted"})
                accepting.result(timeout=2)
                sending = asyncio.create_task(transport.send_async({
                    "jsonrpc": "2.0", "id": "native-init", "method": "meta/init", "params": {},
                }))
                await asyncio.sleep(0)
                assert submitted.is_set() and not sending.done()
                assert child.requests == []

                observing = private.submit(client.open_component, "after-accepted")
                second = receive(reader)
                assert second["id"] == 2 and second["command"]["kind"] == "open_component"
                writer.write(b"{malformed\n")
                with pytest.raises(HostTransportError) as failed:
                    observing.result(timeout=2)
                original = failed.value.__cause__
                assert isinstance(original, json.JSONDecodeError)

            release.set()
            with pytest.raises(HostTransportError) as refused:
                await sending
            assert refused.value.__cause__ is original
            assert child.requests == []
            # Closing preserves the original failure; neither startup nor init
            # is replayed. Non-init cleanup still reaches the same native owner.
            client.close()
            with pytest.raises(HostTransportError) as repeated:
                await transport.send_async({"id": "retry", "method": "meta/init"})
            assert repeated.value.__cause__ is original
            with pytest.raises(HostTransportError) as replay:
                client.replace_application(selection())
            assert isinstance(replay.value.__cause__, HostTransportError)
            assert replay.value.__cause__.__cause__ is original
            assert child.requests == []
            assert transport.send_sync({"id": "cleanup", "method": "meta/shutdown"})["result"] == {"inert": True}
            assert [request["method"] for request in child.requests] == ["meta/shutdown"]
        finally:
            release.set()
            await blocked
            if sending is not None:
                await asyncio.gather(sending, return_exceptions=True)
            await asyncio.to_thread(transport.stop)
            assert child.waits == 1 and transport.host_control is None
            assert reader.read() == b""
            reader.close()
            writer.close()

    asyncio.run(scenario())


def test_pending_preparation_allows_native_init_and_keeps_private_pipe_through_stop():
    client, reader, writer = pipes()
    preparation = ApplicationPreparation(SelectedHostArtifact("/selected/plan.json", "d" * 64), timeout=2)
    transport = ToolkitPersistentTransport("/inert/not-spawned", preparation)

    def still_private():
        assert transport.host_control is client
        assert client._command_fd is not None

    child = _InertNativeChild(transport, still_private)
    transport._process, transport.host_control = child, client
    try:
        attempt = client._begin_application_preparation(preparation)
        first = receive(reader)
        assert first["id"] == 1 and first["command"]["kind"] == "prepare_application"
        assert attempt.manifest is None
        # No private reply: native init must run to service preparation callbacks.
        assert transport.send_sync({"id": "native-init", "method": "meta/init"})["result"] == {"inert": True}
        assert attempt.manifest is None
        assert [request["method"] for request in child.requests] == ["meta/init"]
    finally:
        transport.stop()
        assert child.waits == 1 and transport.host_control is None
        assert reader.read() == b""
        reader.close()
        writer.close()


def test_unobserved_native_exit_retains_private_client_until_same_owner_stop(monkeypatch):
    from meerkat_meta._transport import PersistentTransport

    client, reader, writer = pipes()
    transport = ToolkitPersistentTransport("/inert/not-spawned", selection(), timeout=0.1)
    child = _InertNativeChild(transport, exit_code=None)
    transport._process, transport.host_control = child, client
    native_stop, native_errors = PersistentTransport.stop, []

    def observed_native_stop(owner):
        try:
            return native_stop(owner)
        except BaseException as error:
            native_errors.append(error)
            raise

    # Observe the actual native failure object without replacing its cleanup.
    monkeypatch.setattr(PersistentTransport, "stop", observed_native_stop)
    try:
        with ThreadPoolExecutor(max_workers=1) as executor:
            accepting = executor.submit(client.replace_application, selection())
            first = receive(reader)
            assert first["id"] == 1 and first["command"]["kind"] == "replace_application"
            respond(writer, first, {"kind": "bootstrap_accepted"})
            accepting.result(timeout=2)

            with pytest.raises(RuntimeError, match="did not terminate after bounded cleanup") as failed:
                transport.stop()
            assert failed.value is native_errors[0]
            assert failed.value.__cause__ is child.wait_error
            assert child.waits == 3 and child.signals == ["terminate", "kill"]
            assert child.returncode is None and transport._process is child
            # Admission is closed, but this is not evidence of physical exit.
            assert not transport.is_running()
            assert transport.host_control is client and client._reader.is_alive()
            client.assert_available()

            opened = executor.submit(client.open_component, "retained-private-channel")
            second = receive(reader)
            assert second["id"] == 2 and second["command"]["kind"] == "open_component"
            respond(writer, second, {"kind": "component", "observation": {
                "incarnation": "00000000-0000-0000-0000-000000000001",
                "component": "retained-private-channel", "revision": 1, "state": "open"}})
            opened.result(timeout=2)

        # This is the same transport and process owner, not a public runtime
        # rejoin proof. No second child or native init has been introduced.
        child.exit_code = 0
        with pytest.raises(HostControlError, match="escalation=sigkill"):
            transport.stop()
        assert child.waits == 4 and child.returncode == 0
        assert transport._process is None and transport.host_control is None
        assert not client._reader.is_alive() and reader.read() == b""
        assert child.requests == [] and len(native_errors) == 1
        record = transport.last_termination
        assert record.state == "exited" and record.exit_code == 0
        assert record.escalation == "sigkill"
        transport.stop()
        assert transport.last_termination is record and child.waits == 4
    finally:
        child.exit_code = 0
        transport.stop()
        reader.close()
        writer.close()


@pytest.mark.parametrize("exit_code", [2, -9])
def test_observed_nonzero_native_exit_refuses_and_joins_private_close(exit_code):
    client, reader, writer = pipes()
    transport = ToolkitPersistentTransport("/inert/not-spawned", selection(), timeout=0.1)
    child = _InertNativeChild(transport, exit_code=exit_code)
    transport._process, transport.host_control = child, client
    try:
        with pytest.raises(HostControlError, match="without complete settlement"):
            transport.stop()
        assert child.waits == 1 and child.returncode == exit_code
        assert transport._process is None and transport.host_control is None
        assert not client._reader.is_alive() and reader.read() == b""
        assert child.requests == []
        record = transport.last_termination
        assert record.state == "exited"
        assert record.exit_code == (exit_code if exit_code >= 0 else None)
        assert (record.signal is not None) == (exit_code < 0)
        transport.stop()
        assert transport.last_termination is record and child.waits == 1
    finally:
        transport.stop()
        reader.close()
        writer.close()


@pytest.mark.parametrize(("signals", "escalation"), [(1, "sigterm"), (2, "sigkill")])
def test_exit_zero_after_escalation_is_reported_then_acknowledged_without_erasing_record(signals, escalation):
    client, reader, writer = pipes()
    transport = ToolkitPersistentTransport("/inert/not-spawned", selection(), timeout=0.1)
    child = _InertNativeChild(transport, exit_after_signals=signals)
    transport._process, transport.host_control = child, client
    try:
        with pytest.raises(HostControlError, match=f"escalation={escalation}"):
            transport.stop()
        record = transport.last_termination
        assert child.returncode == 0 and record.state == "exited"
        assert record.exit_code == 0 and record.signal is None
        assert record.escalation == escalation and record.native_shutdown == "not_negotiated"
        assert child.waits == signals + 1 and child.signals == ["terminate", "kill"][:signals]
        assert transport.host_control is None and not client._reader.is_alive()
        assert reader.read() == b"" and child.requests == []
        transport.stop()
        assert transport.last_termination is record and child.waits == signals + 1
    finally:
        transport.stop()
        reader.close()
        writer.close()


@pytest.mark.parametrize(("answer", "expected"), [
    ({"shutdown": True, "runtime_cleanup_completed": True}, "acknowledged"),
    ({"runtime_cleanup_completed": False, "runtime_cleanup_retained_until_exit": True}, "retained_until_exit"),
    ({"runtime_cleanup_completed": False}, "refused"),
])
def test_negotiated_shutdown_answer_is_retained_without_resending(answer, expected):
    client, reader, writer = pipes()
    transport = ToolkitPersistentTransport("/inert/not-spawned", selection(), timeout=0.1)
    child = _InertNativeChild(transport, shutdown_result=answer)
    transport._process, transport.host_control = child, client
    transport._supports_shutdown_handshake = True
    try:
        if expected == "refused":
            with pytest.raises(RuntimeError, match="gateway shutdown failed after bounded cleanup"):
                transport.stop()
        else:
            transport.stop()
        record = transport.last_termination
        assert record.state == "exited" and record.exit_code == 0
        assert record.native_shutdown == expected and record.escalation == "none"
        assert transport.host_control is None and not client._reader.is_alive()
        assert reader.read() == b""
        transport.stop()
        assert transport.last_termination is record and child.waits == 1
        assert [request["method"] for request in child.requests] == ["meta/shutdown"]
    finally:
        transport.stop()
        reader.close()
        writer.close()


def test_ordinary_transport_reuse_does_not_inherit_previous_private_join(monkeypatch):
    from meerkat_meta._transport import PersistentTransport

    # This low-level fixture keeps the real private pipes and Toolkit start
    # boundary, with an inert native construction and no authority selection.
    client, reader, writer = pipes()
    transport = ToolkitPersistentTransport("/inert/not-spawned", object(), timeout=0.1)
    first = _InertNativeChild(transport)
    second = _InertNativeChild(transport, exit_code=2)
    transport._process, transport.host_control = first, client
    starts = []

    def native_start(owner):
        starts.append(owner)
        owner._process = second
        owner._last_termination = None

    try:
        transport.stop()
        assert first.returncode == 0 and transport.host_control is None
        with monkeypatch.context() as scoped:
            scoped.setattr(PersistentTransport, "start", native_start)
            scoped.setattr(PrivateHostClient, "bootstrap", lambda *_args: None)
            transport.start()
            next_client = transport.host_control
            assert next_client is not None and next_client is not client
            transport.start()
            assert starts == [transport] and transport.host_control is next_client
        with pytest.raises(HostControlError, match="exit=2"):
            transport.stop()
        assert second.returncode == 2 and transport.host_control is None
        assert not next_client._reader.is_alive()
        record = transport.last_termination
        transport.stop()
        assert transport.last_termination is record and second.waits == 1
    finally:
        transport.stop()
        reader.close()
        writer.close()


@pytest.mark.parametrize(("native_error_type", "private_error_type"), [
    (RuntimeError, RuntimeError),
    (asyncio.CancelledError, RuntimeError),
    (RuntimeError, asyncio.CancelledError),
    (asyncio.CancelledError, asyncio.CancelledError),
])
def test_stop_preserves_both_exact_failures_and_rejoins_same_client(
    monkeypatch, native_error_type, private_error_type,
):
    from meerkat_meta._transport import PersistentTransport

    client, reader, writer = pipes()
    transport = ToolkitPersistentTransport("/inert/not-spawned", selection(), timeout=0.1)
    child = _InertNativeChild(transport)
    transport._process, transport.host_control = child, client
    native_stop, private_close = PersistentTransport.stop, client.close
    native_error = native_error_type("native cleanup failure after observed exit")
    private_error = private_error_type("private close join failure")

    def failed_native_stop(owner):
        native_stop(owner)
        raise native_error

    def failed_private_close():
        private_close()
        raise private_error

    try:
        with monkeypatch.context() as scoped:
            scoped.setattr(PersistentTransport, "stop", failed_native_stop)
            scoped.setattr(client, "close", failed_private_close)
            with pytest.raises(BaseExceptionGroup) as failed:
                transport.stop()
            assert failed.value.exceptions == (native_error, private_error)
            assert failed.value.__cause__ is native_error
            assert isinstance(failed.value, ExceptionGroup) == (
                native_error_type is RuntimeError and private_error_type is RuntimeError
            )
        assert child.returncode == 0 and transport._process is None
        assert transport.host_control is client
        assert not client._reader.is_alive() and reader.read() == b""
        # With no child left, repeat through the same owner joins private close.
        transport.stop()
        assert transport.host_control is None and child.waits == 1
    finally:
        transport.stop()
        reader.close()
        writer.close()


@pytest.mark.parametrize("failing_owner", ["native", "private"])
@pytest.mark.parametrize("error_type", [RuntimeError, asyncio.CancelledError])
def test_single_stop_failure_keeps_original_object(monkeypatch, failing_owner, error_type):
    from meerkat_meta._transport import PersistentTransport

    client, reader, writer = pipes()
    transport = ToolkitPersistentTransport("/inert/not-spawned", selection(), timeout=0.1)
    child = _InertNativeChild(transport)
    transport._process, transport.host_control = child, client
    native_stop, private_close = PersistentTransport.stop, client.close
    original = error_type("one owner failed after observed exit")

    def observed_native_stop(owner):
        native_stop(owner)
        if failing_owner == "native":
            raise original

    def observed_private_close():
        private_close()
        if failing_owner == "private":
            raise original

    try:
        with monkeypatch.context() as scoped:
            scoped.setattr(PersistentTransport, "stop", observed_native_stop)
            scoped.setattr(client, "close", observed_private_close)
            with pytest.raises(error_type) as failed:
                transport.stop()
            assert failed.value is original
        assert child.returncode == 0 and transport._process is None
        assert transport.host_control is (client if failing_owner == "private" else None)
        assert not client._reader.is_alive() and reader.read() == b""
        # This control joins the same low-level owner. The separate public
        # runtime control below exercises D's returned cleanup handle.
        transport.stop()
        assert transport.host_control is None and child.waits == 1
    finally:
        transport.stop()
        reader.close()
        writer.close()


def _refusing_startup_runtime(tmp_path):
    """Real SDK process/private pipes; the child refuses before native init."""
    from meerkat_meta.builder import MetaBuilderConfig

    log = tmp_path / "child.jsonl"
    child = tmp_path / "inert-gateway"
    child.write_text(f"#!{sys.executable}\n" + f"LOG = {str(log)!r}\n" + """
import json, os, sys, threading
args = sys.argv
read_fd = int(args[args.index('--host-control-read-fd') + 1])
write_fd = int(args[args.index('--host-control-write-fd') + 1])
log_fd = os.open(LOG, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
def record(value):
    os.write(log_fd, json.dumps(value).encode() + b'\\n')
record({'kind': 'spawn'})
def private():
    with os.fdopen(read_fd, 'rb', buffering=0) as reader, os.fdopen(write_fd, 'wb', buffering=0) as writer:
        for line in reader:
            request = json.loads(line)
            record({'kind': 'private', 'request': request})
            writer.write(json.dumps({'version': 1, 'id': request['id'],
                'result': {'kind': 'error', 'error': 'denied'}}).encode() + b'\\n')
thread = threading.Thread(target=private, daemon=True)
thread.start()
for line in sys.stdin.buffer:
    record({'kind': 'native', 'request': json.loads(line)})
record({'kind': 'exit', 'code': 0})
# Native EOF ends this inert fixture independently of private-client close.
os._exit(0)
""")
    child.chmod(0o700)
    source = tmp_path / "update.json"
    selected = b'{"update_id":"inert-startup-control"}\n'
    source.write_bytes(selected)
    runtime = ToolkitApplicationUpdateRuntime(
        MetaBuilderConfig(gateway_bin=str(child)),
        selection(str(source), hashlib.sha256(selected).hexdigest()),
    )
    return runtime, log


def test_runtime_preserves_private_refusal_when_local_close_rejects(tmp_path, monkeypatch):
    from meerkat_meta.runtime import MetaRuntime

    async def scenario():
        runtime, log = _refusing_startup_runtime(tmp_path)
        select, close = PrivateHostClient.replace_application, PrivateHostClient.close
        cleanup = MetaRuntime._cleanup_failed_bootstrap
        private_error = OSError("first private close did not join")
        startup_errors, clients, close_calls, cleanup_entries = [], [], [], []

        def observed_selection(client, startup):
            clients.append(client)
            try:
                return select(client, startup)
            except BaseException as error:
                startup_errors.append(error)
                raise

        def close_once_failed(client):
            close_calls.append(client)
            if len(close_calls) == 1:
                raise private_error
            return close(client)

        async def observed_cleanup(owner, transport, startup_error):
            # Observe, then always enter the real cleanup owner. Assertions
            # below cannot prevent the original native child from being joined.
            cleanup_entries.append((transport.host_control, transport._process, transport._process.returncode, startup_error))
            return await cleanup(owner, transport, startup_error)

        with monkeypatch.context() as scoped:
            scoped.setattr(PrivateHostClient, "replace_application", observed_selection)
            scoped.setattr(PrivateHostClient, "close", close_once_failed)
            scoped.setattr(MetaRuntime, "_cleanup_failed_bootstrap", observed_cleanup)
            try:
                with pytest.raises(HostReplyError) as failed:
                    await runtime.connect()
                assert len(startup_errors) == 1 and failed.value is startup_errors[0]
                assert failed.value.code.value == "denied"
                assert len(clients) == 1 and close_calls == [clients[0], clients[0]]
                assert len(cleanup_entries) == 1 and cleanup_entries[0][0] is clients[0]
                assert cleanup_entries[0][2] is None
                assert cleanup_entries[0][1].returncode == 0
                assert cleanup_entries[0][3] is startup_errors[0]
                assert not clients[0]._reader.is_alive()
                with pytest.raises(HostControlError, match="already started"):
                    await runtime.connect()
                rows = [json.loads(line) for line in log.read_text().splitlines()]
                assert [row["kind"] for row in rows] == ["spawn", "private", "exit"]
                request = rows[1]["request"]
                assert request["id"] == 1 and request["command"]["kind"] == "replace_application"
                assert close_calls == [clients[0], clients[0]]
            finally:
                await runtime.shutdown()

    asyncio.run(scenario())


@pytest.mark.parametrize("interrupted", [False, True], ids=["refusal", "cancelled-startup"])
def test_failed_update_startup_rejoins_private_close_only_through_native_cleanup(tmp_path, monkeypatch, interrupted):
    from meerkat_meta.errors import GatewayStartupCleanupError
    from meerkat_meta._transport import TransportTermination

    async def scenario():
        runtime, log = _refusing_startup_runtime(tmp_path)
        select, close = PrivateHostClient.replace_application, PrivateHostClient.close
        factory = runtime._make_gateway_transport
        private_error = OSError("private close remains unsettled after native exit")
        interruption = asyncio.CancelledError("interrupted private startup")
        refusals, clients, close_calls, factories = [], [], [], []
        returned_cleanup = None

        def observed_factory():
            factories.append(True)
            return factory()

        def observed_selection(client, startup):
            clients.append(client)
            try:
                return select(client, startup)
            except BaseException as error:
                refusals.append(error)
                if interrupted:
                    raise interruption from error
                raise

        def failed_private_close(client):
            close_calls.append((client, runtime.last_gateway_termination))
            # Local startup close happens first, before the native stop. The
            # second call must fail after physical exit to exercise D's
            # retained composed cleanup, not merely a pre-exit failure.
            if len(close_calls) <= 2:
                raise private_error
            return close(client)

        with monkeypatch.context() as scoped:
            scoped.setattr(runtime, "_make_gateway_transport", observed_factory)
            scoped.setattr(PrivateHostClient, "replace_application", observed_selection)
            scoped.setattr(PrivateHostClient, "close", failed_private_close)
            try:
                if interrupted:
                    with pytest.raises(asyncio.CancelledError) as failed:
                        await runtime.connect()
                    assert failed.value is interruption
                    returned_cleanup = failed.value.gateway_cleanup
                    assert failed.value.__cause__ is refusals[0]
                else:
                    with pytest.raises(GatewayStartupCleanupError) as failed:
                        await runtime.connect()
                    returned_cleanup = failed.value.cleanup
                    assert failed.value.startup_error is refusals[0]
                    assert failed.value.__cause__ is refusals[0]
                    assert failed.value.cleanup_error is private_error
                assert len(refusals) == 1 and isinstance(refusals[0], HostReplyError)
                assert refusals[0].code.value == "denied"
                assert len(clients) == 1 and len(close_calls) == 2
                assert close_calls[0] == (clients[0], None)
                record = returned_cleanup.last_termination
                assert record == TransportTermination("exited", 0, None, "none", "not_negotiated")
                assert close_calls[1] == (clients[0], record)
                assert factories == [True]

                # The regression oracle uses only D's returned cleanup, not a
                # captured transport or a new runtime to finish the old join.
                await returned_cleanup.shutdown()
                assert close_calls == [(clients[0], None), (clients[0], record), (clients[0], record)]
                assert returned_cleanup.last_termination is record
                assert not clients[0]._reader.is_alive()
                await returned_cleanup.shutdown()
                assert len(close_calls) == 3 and factories == [True]
                assert returned_cleanup.last_termination is record
                with pytest.raises(HostControlError, match="already started"):
                    await runtime.connect()
                assert factories == [True, True]
                rows = [json.loads(line) for line in log.read_text().splitlines()]
                assert [row["kind"] for row in rows] == ["spawn", "private", "exit"]
                assert rows[1]["request"]["id"] == 1
                assert rows[1]["request"]["command"]["kind"] == "replace_application"
                assert rows[2]["code"] == 0
            finally:
                # If an assertion fails before handoff, the already-created
                # public runtime still owns its child. No replacement/reaper.
                scoped.setattr(PrivateHostClient, "close", close)
                if returned_cleanup is not None:
                    await returned_cleanup.shutdown()
                await runtime.shutdown()

    asyncio.run(scenario())


@pytest.mark.parametrize("interruption", ["cancel", "timeout"])
def test_update_observer_interruption_rejoins_pending_post_exit_private_close(tmp_path, monkeypatch, interruption):
    from meerkat_meta._transport import TransportTermination

    async def scenario():
        runtime, log = _refusing_startup_runtime(tmp_path)
        select, close = PrivateHostClient.replace_application, PrivateHostClient.close
        factory, stop = runtime._make_gateway_transport, ToolkitPersistentTransport.stop
        local_error = OSError("local startup close did not join")
        private_error = OSError("pending post-exit private close did not join")
        entered, release = threading.Event(), threading.Event()
        refusals, clients, closes, factories, stops, observed, deadlines = [], [], [], [], [], [], []
        connect_task = pending_join = None
        selected_transport = None

        def observed_factory():
            nonlocal selected_transport
            factories.append(True)
            selected_transport = factory()
            return selected_transport

        def observed_selection(client, startup):
            clients.append(client)
            try:
                return select(client, startup)
            except BaseException as error:
                refusals.append(error)
                raise

        def observed_stop(transport):
            if transport is selected_transport:
                stops.append(True)
            return stop(transport)

        def pending_private_close(client):
            if selected_transport is None or selected_transport.host_control is not client:
                return close(client)
            closes.append((client, runtime.last_gateway_termination))
            if len(closes) == 1:
                # Keep the client after the earlier, synchronous startup close.
                raise local_error
            if len(closes) == 2:
                # The barrier is in Toolkit's real composed stop, after native
                # has observed the original child exit. No stop is replaced.
                entered.set()
                if not release.wait(10):
                    raise AssertionError("the test did not release private close")
                raise private_error
            return close(client)

        async def observed_connect():
            try:
                await runtime.connect()
            except BaseException as error:
                # Retain the first actual exception, not a later re-await of
                # an already cancelled Task, which can manufacture a new one.
                observed.append(error)
                raise

        async def timed_connect():
            async with asyncio.timeout(None) as deadline:
                deadlines.append(deadline)
                await observed_connect()

        with monkeypatch.context() as scoped:
            scoped.setattr(runtime, "_make_gateway_transport", observed_factory)
            scoped.setattr(PrivateHostClient, "replace_application", observed_selection)
            scoped.setattr(PrivateHostClient, "close", pending_private_close)
            scoped.setattr(ToolkitPersistentTransport, "stop", observed_stop)
            try:
                connect_task = asyncio.create_task(timed_connect() if interruption == "timeout" else observed_connect())
                assert await asyncio.to_thread(entered.wait, 10), "post-exit private close was not reached"
                assert len(refusals) == len(clients) == 1
                assert selected_transport is not None and selected_transport.host_control is clients[0]
                assert isinstance(refusals[0], HostReplyError) and refusals[0].code.value == "denied"
                record = runtime.last_gateway_termination
                assert record == TransportTermination("exited", 0, None, "none", "not_negotiated")
                assert closes == [(clients[0], None), (clients[0], record)]
                assert factories == stops == [True]
                assert not connect_task.done() and not observed and not release.is_set()

                if interruption == "cancel":
                    assert connect_task.cancel()
                    with pytest.raises(asyncio.CancelledError):
                        await connect_task
                    error = observed[0]
                else:
                    # Expire only after the exact barrier, without timing the
                    # child startup or sleeping to guess a cleanup interval.
                    assert len(deadlines) == 1
                    deadlines[0].reschedule(asyncio.get_running_loop().time())
                    with pytest.raises(TimeoutError) as failed:
                        await connect_task
                    error = failed.value.__cause__
                    assert error is observed[0]
                assert len(observed) == 1 and type(error) is asyncio.CancelledError
                assert error.__context__ is refusals[0]
                cleanup = error.gateway_cleanup
                assert cleanup.last_termination is record
                assert len(closes) == 2 and stops == [True] and not release.is_set()

                # Queue the real cleanup call before releasing the same pending
                # stop. FIFO call_soon ordering lets shutdown reach its shield
                # before the worker finishes; no timing delay is the oracle.
                pending_join = asyncio.create_task(cleanup.shutdown())
                asyncio.get_running_loop().call_soon(release.set)
                with pytest.raises(OSError) as failed:
                    await pending_join
                assert failed.value is private_error
                assert len(closes) == 2 and stops == factories == [True]
                assert cleanup.last_termination is record

                # Only the returned same-runtime capability retries the failed
                # private join. It neither reconnects nor constructs a child.
                await cleanup.shutdown()
                assert closes == [(clients[0], None), (clients[0], record), (clients[0], record)]
                assert stops == [True, True] and factories == [True]
                assert cleanup.last_termination is record and not clients[0]._reader.is_alive()
                await cleanup.shutdown()
                assert len(closes) == 3 and stops == [True, True] and factories == [True]
                assert cleanup.last_termination is record
                rows = [json.loads(line) for line in log.read_text().splitlines()]
                assert [row["kind"] for row in rows] == ["spawn", "private", "exit"]
                assert rows[1]["request"]["id"] == 1
                assert rows[1]["request"]["command"]["kind"] == "replace_application"
                assert rows[2]["code"] == 0
            finally:
                # Release any entered worker even after a failed assertion.
                # The existing runtime joins it; there is no captured transport
                # cleanup, replacement child, or second process reaper.
                release.set()
                scoped.setattr(PrivateHostClient, "close", close)
                for pending in (connect_task, pending_join):
                    if pending is not None:
                        try:
                            await pending
                        except BaseException:
                            pass
                await runtime.shutdown()

    asyncio.run(scenario())


def test_runtime_preserves_client_construction_error_through_local_fd_cleanup(tmp_path, monkeypatch):
    from meerkat_meta.runtime import MetaRuntime
    import meerkat_toolkit.host_control as module

    async def scenario():
        runtime, log = _refusing_startup_runtime(tmp_path)
        startup_error = OSError("private client construction refused")
        close_error = OSError("local descriptor close outcome was uncertain")
        close, cleanup = os.close, MetaRuntime._cleanup_failed_bootstrap
        local_fds, attempts, cleanup_entries = [], [], []
        local_identity = {}

        def refuse_client(command_fd, response_fd, **_kwargs):
            local_fds.extend((*runtime._transport._inherited_fds, command_fd, response_fd))
            for fd in local_fds:
                observed = os.fstat(fd)
                local_identity[fd] = (observed.st_dev, observed.st_ino)
            raise startup_error

        def failed_local_close(fd):
            observed = os.fstat(fd)
            is_local = local_identity.get(fd) == (observed.st_dev, observed.st_ino)
            if is_local:
                attempts.append(fd)
            close(fd)
            if is_local and fd == local_fds[2]:
                raise close_error

        async def observed_cleanup(owner, transport, startup_error):
            cleanup_entries.append((transport.host_control, transport._process, transport._process.returncode, startup_error))
            return await cleanup(owner, transport, startup_error)

        with monkeypatch.context() as scoped:
            scoped.setattr(module, "PrivateHostClient", refuse_client)
            scoped.setattr(os, "close", failed_local_close)
            scoped.setattr(MetaRuntime, "_cleanup_failed_bootstrap", observed_cleanup)
            try:
                with pytest.raises(OSError) as failed:
                    await runtime.connect()
                assert failed.value is startup_error
                assert len(local_fds) == 4 and sorted(attempts) == sorted(local_fds)
                for fd, identity in local_identity.items():
                    try:
                        observed = os.fstat(fd)
                    except OSError:
                        continue
                    assert (observed.st_dev, observed.st_ino) != identity
                assert len(cleanup_entries) == 1 and cleanup_entries[0][0] is None
                assert cleanup_entries[0][2] is None
                assert cleanup_entries[0][1].returncode == 0
                assert cleanup_entries[0][3] is startup_error
                rows = [json.loads(line) for line in log.read_text().splitlines()]
                assert rows == [{"kind": "spawn"}, {"kind": "exit", "code": 0}]
            finally:
                try:
                    await runtime.shutdown()
                finally:
                    # A failing baseline can abandon a still-local pipe end.
                    # Clean only that observed fixture pipe, never a descriptor
                    # number reused for another object after it was closed.
                    for fd, identity in local_identity.items():
                        try:
                            observed = os.fstat(fd)
                        except OSError:
                            continue
                        if (observed.st_dev, observed.st_ino) == identity:
                            close(fd)

    asyncio.run(scenario())


@pytest.mark.parametrize("failure_stage", ["native_start", "local_close"])
def test_direct_start_preserves_failure_and_leaves_stop_to_same_caller(monkeypatch, failure_stage):
    from meerkat_meta._transport import PersistentTransport

    transport = ToolkitPersistentTransport("/inert/not-spawned", selection())
    child = _InertNativeChild(transport)
    startup_error = OSError("startup failed after retaining the child")
    stop_error = OSError("inline native stop would replace startup failure")
    stop_calls = []
    close, failed_fd = os.close, []

    def failed_start(owner):
        owner._process = child
        if failure_stage == "native_start":
            raise startup_error
        failed_fd.extend(owner._inherited_fds[1:])

    def failed_close(fd):
        close(fd)
        if fd in failed_fd:
            raise startup_error

    def failed_stop(owner):
        stop_calls.append(owner)
        raise stop_error

    try:
        with monkeypatch.context() as scoped:
            scoped.setattr(PersistentTransport, "start", failed_start)
            scoped.setattr(PersistentTransport, "stop", failed_stop)
            scoped.setattr(os, "close", failed_close)
            with pytest.raises(OSError) as failed:
                transport.start()
            assert failed.value is startup_error
            assert stop_calls == [] and transport._process is child
            assert child.returncode is None
            assert (transport.host_control is None) == (failure_stage == "native_start")
    finally:
        # This is the explicit low-level caller obligation, not a public
        # failed-build cleanup handle or a second process owner.
        transport.stop()
    assert child.waits == 1 and child.returncode == 0 and transport._process is None
