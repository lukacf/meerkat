"""Real stdio/socket ownership; inert bytes, no MCP or native authority proof."""
import asyncio
import contextlib
import errno
import importlib.util
import io
from pathlib import Path
import sys
from types import SimpleNamespace
import unittest

from test_selected_setup import SelectedFixture, adapter, backend
from test_shared_preparation import transport_call

ENTRY = Path(__file__).resolve().parents[1] / "local_session.py"
spec = importlib.util.spec_from_file_location("shared_local_session_test", ENTRY)
local = importlib.util.module_from_spec(spec)
spec.loader.exec_module(local)


class SharedPeerDiagnostics(unittest.TestCase):
    def test_os_error_keeps_only_phase_class_and_allowlisted_errno(self):
        error = BrokenPipeError(errno.EPIPE, "private body", "/private/credential")
        self.assertEqual(local._shared_peer_failure("stdout_write", error),
            "local_setup_connection_diagnostic phase=stdout_write exception=BrokenPipeError errno=EPIPE")
        error = OSError(987654, "private body", "/private/credential")
        self.assertEqual(local._shared_peer_failure("socket_half_close", error),
            "local_setup_connection_diagnostic phase=socket_half_close exception=OSError errno=other")

    def test_unrecognized_class_phase_and_message_are_not_rendered(self):
        error = type("PrivateCredentialError", (ValueError,), {})("private body")
        self.assertEqual(local._shared_peer_failure("private path", error),
            "local_setup_connection_diagnostic phase=other exception=Other errno=none")
        self.assertEqual(local._shared_peer_failure("arguments", local.LocalSetupError("private body")),
            "local_setup_connection_diagnostic phase=arguments exception=LocalSetupError errno=none")


class SharedStdio(SelectedFixture, unittest.IsolatedAsyncioTestCase):
    async def test_joined_peer_renewal_preserves_exact_work_and_fences_old_handles(self):
        from test_shared_preparation import transport_check
        controller = self.controller()
        host = adapter.ControllerConnections(controller)
        selected = asyncio.Future()
        stopped = asyncio.Event()
        streams = []
        async def peer_transport(peer, reader, writer):
            while raw := await reader.readline():
                await transport_call(peer, "/api/state", {})
                writer.write(raw.upper()); await writer.drain()
        module = SimpleNamespace(ControllerConnections=adapter.ControllerConnections,
            socket_stdio=peer_transport, backend=backend, _joined=adapter._joined)
        async def ready(paths): selected.set_result(paths)
        connections = asyncio.create_task(local.shared_connections(module, controller, 1,
            stopped.wait, ready, _owner=host))
        try:
            paths = await asyncio.wait_for(selected, 3)
            predecessor = host._connections[0]
            old_read, old_write = await asyncio.open_unix_connection(paths[0])
            streams.append(old_write)
            old_write.write(b"old\n"); await old_write.drain()
            self.assertEqual(await asyncio.wait_for(old_read.readline(), 3), b"OLD\n")
            request = self.stage("node")
            checked = await transport_check(host, predecessor,
                {**request, "setup_acquisition": "old"})
            installed = await transport_call(predecessor, "/api/selected_install",
                {**self.selection(checked), "setup_acquisition": "old"})
            job = installed["job"]["id"]
            command = {"operation": "renew_peer", "peer": 0}
            with self.assertRaises(backend.SetupError): host.peer_control(command)
            self.assertEqual(len(host._connections), 1)
            self.assertEqual(host.peer_control({"operation": "close_peer", "peer": 0})["state"], "pending")
            with self.assertRaises(backend.SetupError): host.peer_control(command)
            await asyncio.wait_for(asyncio.shield(predecessor._transport_owner.close_task), 3)
            renewed = host.peer_control(command)
            self.assertEqual(renewed["predecessor"], 0)
            self.assertEqual(renewed["peer"], 1)
            self.assertNotEqual(renewed["path"], paths[0])
            successor = host._connections[1]
            self.assertIs(successor.controller, predecessor.controller)
            with self.assertRaises(backend.SetupError): host.peer_control(command)
            with self.assertRaises(backend.SetupError): await transport_call(predecessor, "/api/state", {})
            with self.assertRaises((ConnectionRefusedError, FileNotFoundError)):
                await asyncio.open_unix_connection(paths[0])
            new_read, new_write = await asyncio.open_unix_connection(renewed["path"])
            streams.append(new_write)
            self.assertEqual(host.peer_control({"operation": "close_peer", "peer": 0})["state"], "joined")
            new_write.write(b"successor\n"); await new_write.drain()
            self.assertEqual(await asyncio.wait_for(new_read.readline(), 3), b"SUCCESSOR\n")
            self.assertEqual((await transport_call(successor, "/api/job",
                {"job_id": job, "setup_acquisition": "old"}))["job"]["id"], job)
            completion = {"operation": "observe_completion", "peer": 0,
                "validation_id": checked["validation_id"], "preparation": "f" * 64,
                "setup_acquisition": "old"}
            self.assertEqual(host.owner_control(completion)["completion"]["job"]["id"], job)
            # A new successor acquisition is never disclosed to old resource handles.
            await transport_check(host, successor, {**request, "setup_acquisition": "new"})
            with self.assertRaises(backend.SetupError):
                host.owner_control({**completion, "setup_acquisition": "new"})
            host.owner_control({**completion, "operation": "leave_preparation", "last": False})
            with self.assertRaises(backend.SetupError):
                await transport_call(successor, "/api/selected_install",
                    {**self.selection(checked), "setup_acquisition": "old"})
            self.assertEqual(len(controller.jobs), 1)
            self.assertFalse(controller.closing)
        finally:
            stopped.set()
            await asyncio.wait_for(connections, 3)
            for writer in streams:
                writer.close(); await writer.wait_closed()
        self.assertTrue(all(peer._transport_owner.joined() for peer in host._connections))
        self.assertFalse(Path(paths[0]).parent.exists())

    async def test_peer_renewal_lifetime_bound_refuses_without_losing_cleanup(self):
        controller = self.controller()
        host = adapter.ControllerConnections(controller)
        selected, stopped = asyncio.Future(), asyncio.Event()
        module = SimpleNamespace(ControllerConnections=adapter.ControllerConnections,
            backend=backend, _joined=adapter._joined)
        async def ready(paths): selected.set_result(paths)
        connections = asyncio.create_task(local.shared_connections(module, controller, 1,
            stopped.wait, ready, _owner=host))
        try:
            paths = await asyncio.wait_for(selected, 3)
            for index in range(8):
                host.peer_control({"operation": "close_peer", "peer": index})
                await asyncio.wait_for(asyncio.shield(host._connections[index]._transport_owner.close_task), 3)
                if index < 7:
                    self.assertEqual(host.peer_control({"operation": "renew_peer", "peer": index})["peer"], index + 1)
                else:
                    with self.assertRaisesRegex(backend.SetupError, "connection_limit"):
                        host.peer_control({"operation": "renew_peer", "peer": index})
            self.assertEqual(len(host._connections), 8)
            self.assertNotIn(7, host._successors)
            self.assertFalse(controller.closing)
        finally:
            stopped.set()
            await asyncio.wait_for(connections, 3)
        self.assertTrue(all(peer._transport_owner.joined() for peer in host._connections))
        self.assertFalse(Path(paths[0]).parent.exists())

    async def test_failed_renewal_socket_setup_joins_owner_and_removes_owned_paths(self):
        from unittest.mock import patch
        for target, method in ((local.os, "chmod"), (local.socket.socket, "listen")):
            with self.subTest(method=method):
                controller = self.controller()
                host = adapter.ControllerConnections(controller)
                selected = []
                module = SimpleNamespace(ControllerConnections=adapter.ControllerConnections,
                    backend=backend, _joined=adapter._joined)
                async def ready(paths): selected.extend(paths)
                async def stopped():
                    host.peer_control({"operation": "close_peer", "peer": 0})
                    await asyncio.wait_for(asyncio.shield(host._connections[0]._transport_owner.close_task), 3)
                    with patch.object(target, method, side_effect=OSError("renewal listener setup failed")):
                        host.peer_control({"operation": "renew_peer", "peer": 0})
                with self.assertRaisesRegex(OSError, "renewal listener setup failed"):
                    await local.shared_connections(module, controller, 1, stopped, ready, _owner=host)
                self.assertEqual(len(host._connections), 2)
                self.assertFalse(host._successors)
                self.assertTrue(controller.closing)
                self.assertIsNone(host._peer_factory)
                self.assertTrue(all(peer._transport_owner.joined() for peer in host._connections))
                self.assertFalse(Path(selected[0]).parent.exists())

    async def test_distinct_real_stdio_peers_join_one_owner_and_close_independently(self):
        controller = self.controller()
        peers, processes = [], []
        selected = asyncio.Future()
        stop = asyncio.Event()
        async def peer_transport(peer, reader, writer):
            peers.append(peer)
            while raw := await reader.readline():
                await transport_call(peer, "/api/state", {})
                writer.write(raw.upper())
                await writer.drain()
            writer.write(b"joined\n")
            await writer.drain()
        module = SimpleNamespace(ControllerConnections=adapter.ControllerConnections,
                                 socket_stdio=peer_transport, backend=backend, _joined=adapter._joined)
        async def ready(paths):
            selected.set_result(paths)
        owner = asyncio.create_task(local.shared_connections(module, controller, 2, stop.wait, ready))
        paths = []
        try:
            paths = await asyncio.wait_for(selected, 3)
            self.assertEqual(len(set(paths)), 2)
            for path in paths:
                process = await asyncio.create_subprocess_exec(sys.executable, "-I", "-B", str(ENTRY),
                    "--shared-peer", path, stdin=asyncio.subprocess.PIPE,
                    stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE,
                    env={"PATH": "/usr/bin:/bin", "PYTHONDONTWRITEBYTECODE": "1"})
                processes.append(process)
            first, second = processes
            first.stdin.write(b"first\n")
            second.stdin.write(b"second\n")
            await asyncio.gather(first.stdin.drain(), second.stdin.drain())
            self.assertEqual(await asyncio.wait_for(first.stdout.readline(), 3), b"FIRST\n")
            self.assertEqual(await asyncio.wait_for(second.stdout.readline(), 3), b"SECOND\n")
            self.assertIsNot(peers[0], peers[1])
            self.assertIs(peers[0].controller, peers[1].controller)
            duplicate_read, duplicate_write = await asyncio.open_unix_connection(paths[0])
            self.assertEqual(await asyncio.wait_for(duplicate_read.read(), 3), b"")
            duplicate_write.close()
            await duplicate_write.wait_closed()
            self.assertEqual(len(peers), 2)
            first.stdin.close()
            self.assertEqual(await asyncio.wait_for(first.stdout.read(), 3), b"joined\n")
            self.assertEqual(await asyncio.wait_for(first.wait(), 3), 0)
            self.assertFalse(controller.closing)
            self.assertFalse(owner.done())
            second.stdin.write(b"still live\n")
            await second.stdin.drain()
            self.assertEqual(await asyncio.wait_for(second.stdout.readline(), 3), b"STILL LIVE\n")
            second.stdin.close()
            self.assertEqual(await asyncio.wait_for(second.stdout.read(), 3), b"joined\n")
            self.assertEqual(await asyncio.wait_for(second.wait(), 3), 0)
            self.assertFalse(controller.closing)
            stop.set()
            await asyncio.wait_for(owner, 3)
            self.assertTrue(controller.closing)
            self.assertTrue(all(not thread.is_alive() for thread in controller.threads))
            self.assertTrue(all(not Path(path).exists() for path in paths))
            self.assertFalse(Path(paths[0]).parent.exists())
            stale = await asyncio.create_subprocess_exec(sys.executable, "-I", "-B", str(ENTRY),
                "--shared-peer", paths[0], stdin=asyncio.subprocess.DEVNULL,
                stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
            processes.append(stale)
            stdout, stderr = await asyncio.wait_for(stale.communicate(), 3)
            self.assertEqual(stale.returncode, 2)
            self.assertEqual(stdout, b"")
            self.assertEqual(stderr, b"local_setup_connection_unavailable\n"
                b"local_setup_connection_diagnostic phase=path_validation exception=FileNotFoundError errno=ENOENT\n")
        finally:
            stop.set()
            await asyncio.wait_for(owner, 3)
            for process in processes:
                if process.stdin is not None:
                    process.stdin.close()
                await asyncio.wait_for(process.wait(), 3)
                if process.stderr is not None:
                    await process.stderr.read()

    async def test_inherited_owner_frames_bind_sequence_and_exact_peer_ticket(self):
        controller = self.controller()
        owner = adapter.ControllerConnections(controller)
        first, second = owner.open(), owner.open()
        class Output:
            def __init__(self): self.data = bytearray()
            def write(self, data): self.data.extend(data)
            async def drain(self): await asyncio.sleep(0)
        try:
            pending = await transport_call(first, "/api/selected_check", self.stage("node"))
            command = {"operation": "inspect_selection", "peer": 0, "ticket": pending["ticket"]}
            reader, output = asyncio.StreamReader(limit=4096), Output()
            reader.feed_data(local.encoded({"id": 1, "command": command}))
            reader.feed_data(local.encoded({"id": 2, "command": {**command, "peer": 1}}))
            reader.feed_eof()
            await asyncio.wait_for(local.shared_owner_requests(adapter, owner, reader, output), 3)
            results = [local.decode(line) for line in output.data.splitlines()]
            self.assertEqual([item["id"] for item in results], [1, 2])
            self.assertIn("selection", results[0]["result"])
            self.assertEqual(results[1], {"id": 2, "error": "shared_selection_unavailable"})
            self.assertFalse(owner._views[first]["validations"])
            self.assertFalse(owner._views[second]["validations"])
            for raw in (local.encoded({"id": 2, "command": command}),
                        local.encoded({"id": True, "command": command}),
                        b"x" * 4097 + b"\n", b'{"id":1,"id":1,"command":{}}\n'):
                reader, output = asyncio.StreamReader(limit=4096), Output()
                reader.feed_data(raw); reader.feed_eof()
                with self.assertRaises((local.LocalSetupError, ValueError)):
                    await asyncio.wait_for(local.shared_owner_requests(adapter, owner, reader, output), 3)
                self.assertEqual(output.data, b"")
        finally:
            await owner.aclose()

    async def test_host_close_fences_live_peers_and_joins_before_removing_endpoints(self):
        controller = self.controller()
        selected = asyncio.Future()
        entered = asyncio.Event()
        stop = asyncio.Event()
        joined = []
        async def peer_transport(peer, reader, writer):
            entered.set()
            try:
                await reader.read()
            finally:
                self.assertTrue(controller.closing)
                self.assertTrue(peer._admission_closed)
                joined.append(True)
        module = SimpleNamespace(ControllerConnections=adapter.ControllerConnections,
                                 socket_stdio=peer_transport, backend=backend, _joined=adapter._joined)
        async def ready(paths):
            selected.set_result(paths)
        owner = asyncio.create_task(local.shared_connections(module, controller, 2, stop.wait, ready))
        writer = None
        try:
            paths = await asyncio.wait_for(selected, 3)
            reader, writer = await asyncio.open_unix_connection(paths[0])
            await asyncio.wait_for(entered.wait(), 3)
            stop.set()
            await asyncio.wait_for(owner, 3)
            self.assertEqual(joined, [True])
            self.assertEqual(await asyncio.wait_for(reader.read(), 3), b"")
            self.assertTrue(all(not Path(path).exists() for path in paths))
        finally:
            stop.set()
            await asyncio.wait_for(owner, 3)
            if writer is not None:
                writer.close()
                await writer.wait_closed()

    async def test_peer_retirement_ack_waits_for_a_drain_and_socket_while_b_worker_survives(self):
        import threading
        from unittest.mock import patch
        from test_shared_preparation import transport_check
        controller = self.controller()
        host = adapter.ControllerConnections(controller)
        selected = asyncio.Future()
        stopped = asyncio.Event()
        a_entered, a_release = threading.Event(), threading.Event()
        socket_finishing, socket_release = asyncio.Event(), asyncio.Event()
        replies = asyncio.Queue()
        reader = asyncio.StreamReader(limit=256 * 1024)
        class Output:
            def write(self, raw): replies.put_nowait(local.decode(raw))
            async def drain(self): await asyncio.sleep(0)
        sequence = 0
        async def command(value):
            nonlocal sequence
            sequence += 1
            reader.feed_data(local.encoded({"id": sequence, "command": value}))
            reply = await asyncio.wait_for(replies.get(), 3)
            self.assertEqual(reply["id"], sequence)
            return reply
        async def peer_transport(peer, source, sink):
            try:
                while await source.readline():
                    try:
                        await transport_call(peer, "/api/state", {})
                    except backend.SetupError as error:
                        if peer is host._connections[0] and str(error) == "host_closing":
                            return
                        raise
                    sink.write(b"ok\n")
                    await sink.drain()
            finally:
                if peer is host._connections[0]:
                    socket_finishing.set()
                    await socket_release.wait()
        module = SimpleNamespace(ControllerConnections=adapter.ControllerConnections,
            socket_stdio=peer_transport, backend=backend, _joined=adapter._joined)
        async def ready(paths): selected.set_result(paths)
        async def stop(): await stopped.wait()
        connections = asyncio.create_task(local.shared_connections(module, controller, 2,
            stop, ready, _owner=host))
        requests = asyncio.create_task(local.shared_owner_requests(adapter, host, reader, Output()))
        streams, installs = [], []
        original = host.dispatch
        def hold_a(peer, route, arguments, entry):
            if peer is host._connections[0] and route == "/api/state":
                a_entered.set()
                if not a_release.wait(10):
                    raise AssertionError("test releases exact A dispatch")
            return original(peer, route, arguments, entry)
        try:
            paths = await asyncio.wait_for(selected, 3)
            a, b = host._connections
            checked = await transport_check(host, b, self.stage("node"))
            controller.runner.block = threading.Event()
            install = asyncio.create_task(transport_call(b, "/api/selected_install", self.selection(checked)))
            installs.append(install)
            self.assertTrue(await asyncio.to_thread(controller.runner.entered.wait, 3))
            job = controller.validations[checked["validation_id"]]["job_id"]
            a_read, a_write = await asyncio.open_unix_connection(paths[0])
            b_read, b_write = await asyncio.open_unix_connection(paths[1])
            streams.extend((a_write, b_write))
            self.assertEqual((await command({"operation": "observe_peer", "peer": 0}))["result"]["state"], "not_requested")
            with patch.object(host, "dispatch", side_effect=hold_a):
                a_write.write(b"hold\n"); await a_write.drain()
                self.assertTrue(await asyncio.to_thread(a_entered.wait, 3))
                # Retain an actual late entry before peer fencing. It cannot
                # be consumed or reissued after close_peer returns pending.
                late = host.owner_control({"operation": "prepare_entry", "peer": 0,
                    "connection": "1", "tool": "setup_state", "arguments": {}, "preparation": None})["entry"]
                first = await command({"operation": "close_peer", "peer": 0})
                self.assertEqual(first["result"], {"scope": "host_shared_peer_retirement", "peer": 0, "state": "pending"})
                retained = a._transport_owner.close_task
                # Discarding that first reply changes no close owner. Readback
                # and a repeated request name the same still-pending task.
                del first
                for operation in ("observe_peer", "close_peer"):
                    self.assertEqual((await command({"operation": operation, "peer": 0}))["result"]["state"], "pending")
                    self.assertIs(a._transport_owner.close_task, retained)
                self.assertFalse(a._close_task.done())
                with self.assertRaisesRegex(backend.SetupError, "host_closing"):
                    await a.dispatch("/api/state", {}, _entry=late)
                with self.assertRaises(backend.SetupError):
                    host.owner_control({"operation": "prepare_entry", "peer": 0,
                        "connection": "1", "tool": "setup_state", "arguments": {}, "preparation": None})
                host.owner_control({"operation": "close_entry", "peer": 0, "entry": late})
                host.owner_control({"operation": "finish_entry", "peer": 0, "entry": late})
                b_write.write(b"live\n"); await b_write.drain()
                self.assertEqual(await asyncio.wait_for(b_read.readline(), 3), b"ok\n")
                self.assertEqual((await transport_call(b, "/api/job", {"job_id": job}))["job"]["status"], "running")
                self.assertFalse(controller.closing)
                self.assertFalse(controller._job_controls[job]["requested"].is_set())
                self.assertTrue(controller._job_threads[job].is_alive())
                a_release.set()
                await asyncio.wait_for(socket_finishing.wait(), 3)
                await asyncio.wait_for(a.aclose(), 3)
                self.assertTrue(a._close_task.done())
                self.assertFalse(retained.done(), "adapter drain alone is not socket/serve completion")
                self.assertEqual((await command({"operation": "observe_peer", "peer": 0}))["result"]["state"], "pending")
                socket_release.set()
                await asyncio.wait_for(asyncio.shield(retained), 3)
                for operation in ("observe_peer", "close_peer"):
                    self.assertEqual((await command({"operation": operation, "peer": 0}))["result"]["state"], "joined")
                    self.assertIs(a._transport_owner.close_task, retained)
                self.assertEqual(await asyncio.wait_for(a_read.read(), 3), b"")
                self.assertFalse(a._transport_owner.tasks)
                self.assertFalse(a._transport_owner.writers)
                self.assertFalse(install.done())
                self.assertTrue(controller._job_threads[job].is_alive())
                self.assertFalse(controller._job_controls[job]["requested"].is_set())
                b_write.write(b"still live\n"); await b_write.drain()
                self.assertEqual(await asyncio.wait_for(b_read.readline(), 3), b"ok\n")
            controller.runner.block.set()
            self.assertEqual((await asyncio.wait_for(install, 3))["job"]["status"], "succeeded")
            self.assertEqual(len(controller.jobs), 1)
        finally:
            a_release.set(); socket_release.set()
            if controller.runner.block is not None: controller.runner.block.set()
            await asyncio.gather(*installs, return_exceptions=True)
            reader.feed_eof()
            await asyncio.wait_for(requests, 3)
            stopped.set()
            await asyncio.wait_for(connections, 3)
            for writer in streams:
                writer.close(); await writer.wait_closed()
        self.assertFalse(host._job_waits)
        self.assertTrue(all(not thread.is_alive() for thread in controller.threads))

    async def test_failed_second_listener_registration_still_joins_owner_and_cleans_paths(self):
        from unittest.mock import patch
        controller = self.controller()
        owner = adapter.ControllerConnections(controller)
        module = SimpleNamespace(ControllerConnections=adapter.ControllerConnections,
            backend=backend, _joined=adapter._joined)
        loop = asyncio.get_running_loop()
        add_reader, mkdtemp = loop.add_reader, local.tempfile.mkdtemp
        calls, directories = [], []
        def register(fd, *args):
            calls.append(fd)
            if len(calls) == 2:
                raise RuntimeError("controlled reader registration failure")
            return add_reader(fd, *args)
        def directory(*args, **kwargs):
            path = mkdtemp(*args, **kwargs); directories.append(Path(path)); return path
        async def ready(_): self.fail("incomplete listener set cannot be published")
        async def stopped(): self.fail("failed construction does not enter serving")
        with patch.object(loop, "add_reader", side_effect=register), \
                patch.object(local.tempfile, "mkdtemp", side_effect=directory):
            with self.assertRaisesRegex(RuntimeError, "controlled reader registration failure"):
                await local.shared_connections(module, controller, 2, stopped, ready, _owner=owner)
        self.assertEqual(len(calls), 2)
        self.assertEqual(len(owner._connections), 2)
        first, second = (peer._transport_owner for peer in owner._connections)
        self.assertEqual(first.listener.fileno(), -1)
        self.assertIsNone(second.listener)
        self.assertTrue(all(transport.retirement() == "joined" for transport in (first, second)))
        self.assertTrue(controller.closing)
        self.assertTrue(all(not thread.is_alive() for thread in controller.threads))
        self.assertTrue(directories and all(not path.exists() for path in directories))

    async def test_peer_close_joins_accepted_socket_setup_before_acknowledging(self):
        from unittest.mock import patch
        controller = self.controller()
        owner = adapter.ControllerConnections(controller)
        selected = asyncio.Future()
        stopped, opening, release = asyncio.Event(), asyncio.Event(), asyncio.Event()
        entered = []
        async def peer_transport(peer, reader, writer):
            entered.append(peer)
            while raw := await reader.readline():
                await transport_call(peer, "/api/state", {})
                writer.write(raw.upper()); await writer.drain()
        module = SimpleNamespace(ControllerConnections=adapter.ControllerConnections,
            socket_stdio=peer_transport, backend=backend, _joined=adapter._joined)
        async def ready(paths): selected.set_result(paths)
        connections = asyncio.create_task(local.shared_connections(module, controller, 2,
            stopped.wait, ready, _owner=owner))
        original = asyncio.open_unix_connection
        streams = []
        try:
            paths = await asyncio.wait_for(selected, 3)
            a, b = owner._connections
            async def held_open(*args, **kwargs):
                connection = kwargs.get("sock")
                if connection is not None and connection.getsockname() == paths[0]:
                    opening.set()
                    await release.wait()
                return await original(*args, **kwargs)
            with patch.object(local.asyncio, "open_unix_connection", side_effect=held_open):
                a_reader, a_writer = await original(paths[0])
                streams.append(a_writer)
                await asyncio.wait_for(opening.wait(), 3)
                transport = a._transport_owner
                self.assertEqual(len(transport.openings), 1)
                self.assertFalse(transport.writers)
                self.assertEqual(owner.peer_control({"operation": "close_peer", "peer": 0})["state"], "pending")
                retained = transport.close_task
                self.assertEqual(transport.listener.fileno(), -1)
                transport.accept_ready()  # Already-queued reader callback after fence.
                for operation in ("close_peer", "observe_peer"):
                    self.assertEqual(owner.peer_control({"operation": operation, "peer": 0})["state"], "pending")
                    self.assertIs(transport.close_task, retained)
                with self.assertRaises(OSError):
                    await original(paths[0])
                b_reader, b_writer = await original(paths[1])
                streams.append(b_writer)
                b_writer.write(b"b lives\n"); await b_writer.drain()
                self.assertEqual(await asyncio.wait_for(b_reader.readline(), 3), b"B LIVES\n")
                self.assertEqual(entered, [b])
                self.assertFalse(b._admission_closed)
                self.assertFalse(controller.closing)
                release.set()
                await asyncio.wait_for(asyncio.shield(retained), 3)
                self.assertEqual(owner.peer_control({"operation": "observe_peer", "peer": 0})["state"], "joined")
                self.assertFalse(transport.openings or transport.tasks or transport.writers)
                self.assertEqual(await asyncio.wait_for(a_reader.read(), 3), b"")
                self.assertEqual(entered, [b], "late A setup never enters the adapter")
                b_writer.write(b"still live\n"); await b_writer.drain()
                self.assertEqual(await asyncio.wait_for(b_reader.readline(), 3), b"STILL LIVE\n")
        finally:
            release.set(); stopped.set()
            await asyncio.wait_for(connections, 3)
            for writer in streams:
                writer.close(); await writer.wait_closed()
        self.assertTrue(all(not thread.is_alive() for thread in controller.threads))

    async def test_private_peer_close_rejects_malformed_requests_without_fencing_b(self):
        controller = self.controller()
        owner = adapter.ControllerConnections(controller)
        a, b = owner.open(), owner.open()
        module = SimpleNamespace(backend=backend, _joined=adapter._joined)
        transports = [local._SharedPeerTransport(module, peer) for peer in (a, b)]
        class Output:
            def __init__(self): self.data = bytearray()
            def write(self, raw): self.data.extend(raw)
            async def drain(self): await asyncio.sleep(0)
        requests = [
            {"operation": "close_peer", "peer": True},
            {"operation": "close_peer", "peer": -1},
            {"operation": "close_peer", "peer": 2},
            {"operation": "close_peer", "peer": 0, "entry": "invented"},
            {"operation": "observe_peer"},
        ]
        reader, output = asyncio.StreamReader(limit=4096), Output()
        try:
            for index, command in enumerate(requests, 1):
                reader.feed_data(local.encoded({"id": index, "command": command}))
            reader.feed_eof()
            await asyncio.wait_for(local.shared_owner_requests(adapter, owner, reader, output), 3)
            self.assertEqual([local.decode(line) for line in output.data.splitlines()],
                [{"id": index, "error": "shared_selection_unavailable"} for index in range(1, len(requests) + 1)])
            self.assertFalse(a._admission_closed)
            self.assertFalse(b._admission_closed)
            self.assertTrue(all(transport.close_task is None for transport in transports))
            first = owner.peer_control({"operation": "close_peer", "peer": 0})
            self.assertEqual(first["state"], "pending")
            task = transports[0].close_task
            await asyncio.wait_for(asyncio.shield(task), 3)
            self.assertEqual(owner.peer_control({"operation": "observe_peer", "peer": 0})["state"], "joined")
            self.assertIs(transports[0].close_task, task)
            self.assertFalse(b._admission_closed)
            self.assertFalse(controller.closing)
            self.assertEqual(owner.peer_control({"operation": "observe_peer", "peer": 1})["state"], "not_requested")
        finally:
            await asyncio.gather(*(transport.aclose() for transport in transports))
            await owner.aclose()

    async def test_failed_peer_close_is_reported_by_peer_and_category_never_text(self):
        from unittest.mock import patch
        controller = self.controller()
        selected, refused = asyncio.Future(), asyncio.Event()
        module = SimpleNamespace(ControllerConnections=adapter.ControllerConnections,
            backend=backend, _joined=adapter._joined)
        async def ready(paths): selected.set_result(paths)
        async def open_refused(**_):
            refused.set()
            raise BrokenPipeError(errno.EPIPE, "private body", "/private/credential")
        async def stopped():
            # Only the second peer's accepted socket fails to open as a stream;
            # the host then closes both peers.
            client = local.socket.socket(local.socket.AF_UNIX, local.socket.SOCK_STREAM)
            try:
                client.connect(selected.result()[1])
                await asyncio.wait_for(refused.wait(), 3)
                for _ in range(3):
                    await asyncio.sleep(0)
            finally:
                client.close()
        stderr = io.StringIO()
        with patch.object(local.asyncio, "open_unix_connection", open_refused), contextlib.redirect_stderr(stderr):
            with self.assertRaisesRegex(local.LocalSetupError, "local_setup_connection_failed"):
                await asyncio.wait_for(local.shared_connections(module, controller, 2, stopped, ready), 3)
        # Each failed peer close is named by its peer and closed categories,
        # never by its exception text or paths.
        self.assertEqual(stderr.getvalue(), "local_setup_connection_diagnostic phase=peer_close peer=1 "
            "exception=BrokenPipeError errno=EPIPE\n")
        self.assertTrue(controller.closing)
        self.assertFalse(Path(selected.result()[0]).parent.exists())

    async def test_cancelled_serve_before_first_poll_still_joins_socket_and_failed_drain_is_not_joined(self):
        controller = self.controller()
        owner = adapter.ControllerConnections(controller)
        peer = owner.open()
        entered = []
        async def serve(*_): entered.append(True)
        module = SimpleNamespace(backend=backend, _joined=adapter._joined, socket_stdio=serve)
        transport = local._SharedPeerTransport(module, peer)
        closed, release = asyncio.Event(), asyncio.Event()
        class Writer:
            def close(self): closed.set()
            async def wait_closed(self): await release.wait()
        try:
            transport.start(None, Writer())
            pending = next(iter(transport.tasks))
            pending.cancel()  # Before its first poll: coroutine finally cannot run.
            self.assertEqual(transport.retirement(True), "pending")
            await asyncio.wait_for(closed.wait(), 3)
            self.assertFalse(entered)
            self.assertEqual(transport.retirement(), "pending")
            release.set()
            await asyncio.wait_for(asyncio.shield(transport.close_task), 3)
            self.assertEqual(transport.retirement(), "joined")
            self.assertFalse(transport.tasks)
            self.assertFalse(transport.writers)
            other = owner.open()
            failed = local._SharedPeerTransport(module, other)
            async def failure(): raise RuntimeError("controlled drain failure")
            other.wait_workers = failure
            self.assertEqual(failed.retirement(True), "pending")
            with self.assertRaisesRegex(local.LocalSetupError, "local_setup_connection_failed"):
                await asyncio.wait_for(asyncio.shield(failed.close_task), 3)
            self.assertEqual(failed.retirement(), "failed")
            self.assertEqual(failed.retirement(True), "failed")
            with self.assertRaises(RuntimeError): await other.aclose()
        finally:
            release.set()
            await transport.aclose()
            # The failed peer owner remains honestly failed; global close still
            # joins Controller and is expected to retain that same error.
            try: await owner.aclose()
            except RuntimeError as error: self.assertEqual(str(error), "controlled drain failure")
        self.assertTrue(all(not thread.is_alive() for thread in controller.threads))



if __name__ == "__main__":
    unittest.main()
