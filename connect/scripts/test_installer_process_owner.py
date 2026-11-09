"""Signal handoff and direct-child join controls for the existing installer."""
from __future__ import annotations

import importlib.util
from pathlib import Path
import signal
import subprocess
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).with_name("install-library-consumer.py")
SPEC = importlib.util.spec_from_file_location("installer_process_owner", SCRIPT)
installer = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(installer)


class Child:
    pid = 417

    def __init__(self, waits):
        self.returncode = None
        self.waits = list(waits)
        self.wait_calls = []

    def wait(self, timeout=None):
        self.wait_calls.append(timeout)
        result = self.waits.pop(0) if self.waits else self.returncode
        if isinstance(result, BaseException):
            raise result
        self.returncode = result
        return result


class InstallerProcessOwnerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.original = {sig: signal.getsignal(sig) for sig in (signal.SIGINT, signal.SIGTERM)}
        self.addCleanup(self.restore)

    def restore(self):
        for sig, handler in self.original.items():
            signal.signal(sig, handler)

    def invoke(self):
        return installer.run(["inert-owned-child"], self.root, {}, "step", timeout=9)

    def assert_restored(self):
        self.assertEqual({sig: signal.getsignal(sig) for sig in self.original}, self.original)

    def test_spawn_signal_is_deferred_until_child_is_owned(self):
        for sig in (signal.SIGINT, signal.SIGTERM):
            with self.subTest(signal=sig):
                # Separate paths preserve exclusive log creation in each call.
                self.root = Path(self.temp.name) / str(int(sig))
                self.root.mkdir()
                child = Child([0])
                def spawn(*args, **kwargs):
                    signal.getsignal(sig)(sig, None)
                    return child
                with patch.object(installer.subprocess, "Popen", side_effect=spawn), \
                        patch.object(installer.os, "killpg") as kill:
                    with self.assertRaises(KeyboardInterrupt):
                        self.invoke()
                    self.assertEqual(kill.call_args_list, [unittest.mock.call(child.pid, signal.SIGTERM)])
                    self.assertEqual(child.wait_calls, [2, None])
                    self.assertEqual(child.returncode, 0)
                self.assert_restored()

    def test_nonzero_reaped_child_is_never_signaled(self):
        child = Child([7])
        with patch.object(installer.subprocess, "Popen", return_value=child), \
                patch.object(installer.os, "killpg") as kill:
            with self.assertRaisesRegex(installer.InstallError, "installer step failed"):
                self.invoke()
            kill.assert_not_called()
            self.assertEqual(child.wait_calls, [0.1, None])
            self.assertEqual(child.returncode, 7)
        self.assert_restored()

    def test_successful_term_join_is_not_followed_by_kill(self):
        child = Child([KeyboardInterrupt(), 0])
        with patch.object(installer.subprocess, "Popen", return_value=child), \
                patch.object(installer.os, "killpg") as kill:
            with self.assertRaises(KeyboardInterrupt):
                self.invoke()
            self.assertEqual(kill.call_args_list, [unittest.mock.call(child.pid, signal.SIGTERM)])
            self.assertEqual(child.wait_calls, [0.1, 2, None])
            self.assertEqual(child.returncode, 0)
        self.assert_restored()

    def test_term_timeout_escalates_then_joins_same_child(self):
        child = Child([KeyboardInterrupt(), subprocess.TimeoutExpired("owned", 2), -9])
        with patch.object(installer.subprocess, "Popen", return_value=child), \
                patch.object(installer.os, "killpg") as kill:
            with self.assertRaises(KeyboardInterrupt):
                self.invoke()
            self.assertEqual(kill.call_args_list, [unittest.mock.call(child.pid, signal.SIGTERM),
                                                   unittest.mock.call(child.pid, signal.SIGKILL)])
            self.assertEqual(child.wait_calls, [0.1, 2, None])
            self.assertEqual(child.returncode, -9)
        self.assert_restored()

    def test_signal_error_still_waits_for_owned_child(self):
        child = Child([KeyboardInterrupt(), 0])
        with patch.object(installer.subprocess, "Popen", return_value=child), \
                patch.object(installer.os, "killpg", side_effect=PermissionError):
            with self.assertRaises(PermissionError):
                self.invoke()
            self.assertEqual(child.wait_calls, [0.1, None])
            self.assertEqual(child.returncode, 0)
        self.assert_restored()

    def test_repeated_signals_during_cleanup_do_not_change_handlers_or_abandon_join(self):
        child = Child([0])
        actual_signal = signal.signal
        def install_handler(sig, handler):
            # There must be no transition to SIG_IGN or caller handlers while
            # an actual child is owned and unreaped. The old cleanup transition
            # delivers a second TERM here and abandons its child.
            if child.returncode is None and spawn_done:
                self.assertTrue(callable(handler) and handler not in self.original.values())
            return actual_signal(sig, handler)
        spawn_done = False
        def spawn(*args, **kwargs):
            nonlocal spawn_done
            spawn_done = True
            signal.getsignal(signal.SIGTERM)(signal.SIGTERM, None)
            return child
        def term(pid, sig):
            self.assertEqual((pid, sig), (child.pid, signal.SIGTERM))
            signal.getsignal(signal.SIGTERM)(signal.SIGTERM, None)
            signal.getsignal(signal.SIGINT)(signal.SIGINT, None)
        with patch.object(installer.subprocess, "Popen", side_effect=spawn), \
                patch.object(installer.signal, "signal", side_effect=install_handler), \
                patch.object(installer.os, "killpg", side_effect=term):
            with self.assertRaises(KeyboardInterrupt):
                self.invoke()
        self.assertEqual(child.wait_calls, [2, None])
        self.assertEqual(child.returncode, 0)
        self.assert_restored()

    def test_signal_with_successful_wait_refuses_next_step_without_signaling_reaped_child(self):
        child = Child([])
        def wait(timeout=None):
            child.wait_calls.append(timeout)
            child.returncode = 0
            signal.getsignal(signal.SIGTERM)(signal.SIGTERM, None)
            return 0
        child.wait = wait
        with patch.object(installer.subprocess, "Popen", return_value=child), \
                patch.object(installer.os, "killpg") as kill:
            with self.assertRaises(KeyboardInterrupt):
                self.invoke()
            kill.assert_not_called()
        self.assertEqual(child.wait_calls, [0.1, None])
        self.assert_restored()

    def test_signal_during_post_join_restore_is_not_lost(self):
        child = Child([0])
        actual_signal = signal.signal
        injected = False
        def restore(sig, handler):
            nonlocal injected
            if not injected and child.returncode == 0 and handler == self.original[sig]:
                injected = True
                signal.getsignal(signal.SIGTERM)(signal.SIGTERM, None)
            return actual_signal(sig, handler)
        with patch.object(installer.subprocess, "Popen", return_value=child), \
                patch.object(installer.signal, "signal", side_effect=restore), \
                patch.object(installer.os, "killpg") as kill:
            with self.assertRaises(KeyboardInterrupt):
                self.invoke()
            kill.assert_not_called()
        self.assertTrue(injected)
        self.assertEqual(child.wait_calls, [0.1])
        self.assert_restored()

    def test_polling_uses_one_original_deadline(self):
        child = Child([subprocess.TimeoutExpired("owned", 0.1), 0])
        with patch.object(installer.subprocess, "Popen", return_value=child), \
                patch.object(installer.time, "monotonic", side_effect=[10, 10, 20]), \
                patch.object(installer.os, "killpg") as kill:
            with self.assertRaises(subprocess.TimeoutExpired) as caught:
                self.invoke()
            self.assertEqual(caught.exception.timeout, 9)
            self.assertEqual(kill.call_args_list, [unittest.mock.call(child.pid, signal.SIGTERM)])
        self.assertEqual(child.wait_calls, [0.1, 2, None])
        self.assert_restored()

    def test_spawn_failure_restores_handlers_without_signaling(self):
        with patch.object(installer.subprocess, "Popen", side_effect=OSError("spawn refused")), \
                patch.object(installer.os, "killpg") as kill:
            with self.assertRaises(OSError):
                self.invoke()
            kill.assert_not_called()
        self.assert_restored()


if __name__ == "__main__":
    unittest.main()
