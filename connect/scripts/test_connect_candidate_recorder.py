"""Real isolated Recorder process/group controls. No native or provider work."""
from pathlib import Path
import importlib.util
import json
import os
import signal
import subprocess
import sys
import tempfile
import time
import unittest


HERE = Path(__file__).resolve()


def save(path, value):
    temporary = path.with_suffix(".tmp")
    temporary.write_text(json.dumps(value) + "\n")
    temporary.replace(path)


def await_file(path, seconds=10):
    deadline = time.monotonic() + seconds
    while not path.exists():
        if time.monotonic() >= deadline:
            raise AssertionError("fixture did not publish " + str(path))
        time.sleep(0.01)
    return json.loads(path.read_text())


def running(pid):
    """Zombie existence is not live work, and is not a descendant reap claim."""
    result = subprocess.run(["/bin/ps", "-o", "stat=", "-p", str(pid)],
                            capture_output=True, text=True, timeout=5)
    if result.returncode not in (0, 1):
        raise AssertionError("could not observe fixture process state")
    state = result.stdout.strip()
    return bool(state) and not state.startswith("Z")


def worker(root):
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    save(root / "worker.json", {"pid": os.getpid(), "pgid": os.getpgrp()})
    while True:
        time.sleep(1)


def leader(root):
    def term(_signal, _frame):
        print("leader handled TERM after original wait failed", flush=True)
        save(root / "term.json", {"pid": os.getpid()})
        raise SystemExit(0)

    signal.signal(signal.SIGTERM, term)
    descendant = subprocess.Popen([sys.executable, "-B", str(HERE), "--worker", str(root)])
    row = await_file(root / "worker.json")
    assert row["pid"] == descendant.pid and row["pgid"] == os.getpgrp() == os.getpid()
    save(root / "ready.json", {"leader": os.getpid(), "descendant": descendant.pid,
                                "pgid": os.getpgrp()})
    print("leader and descendant ready", flush=True)
    while True:
        time.sleep(1)


def case(root, mode):
    # Every injected interruption is delivered inside this separate test
    # process. Neither Recorder nor the test changes the parent test handlers.
    spec = importlib.util.spec_from_file_location("candidate_recorder", HERE.with_name("connect-candidate.py"))
    candidate = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(candidate)
    recorder = candidate.Recorder(root)
    recorder.TERM_GRACE_SECONDS = 0.25
    recorder.CLEANUP_WAIT_SECONDS = 0.02
    original_handlers = {sig: signal.getsignal(sig) for sig in (signal.SIGINT, signal.SIGTERM)}
    real_popen = subprocess.Popen
    real_killpg = os.killpg
    real_write = candidate.write_new
    owned = []
    signals = []
    repeated = False
    kill_refused = False
    wait_failure = RuntimeError("actual wait observation failed")
    first_error = None
    original_trace = sys.gettrace()
    post_return_seen = False
    group_case = mode in ("interrupt", "entry_interrupt", "wait_error", "record_error", "signal_error",
                         "kill_error", "timeout", "post_return")
    command = ([sys.executable, "-B", str(HERE), "--leader", str(root)] if group_case else
               [sys.executable, "-B", "-c", "print('ordinary output', flush=True); raise SystemExit(" +
                ("7" if mode == "nonzero" else "0") + ")"])

    def spawn(*args, **kwargs):
        child = real_popen(*args, **kwargs)
        owned.append(child)
        original_wait = child.wait
        injected = False

        def wait(timeout=None):
            nonlocal injected, first_error
            if not injected and mode == "timeout":
                injected = True
                await_file(root / "ready.json")
            if not injected and mode not in ("success", "nonzero", "timeout", "post_return"):
                injected = True
                if group_case:
                    await_file(root / "ready.json")
                else:
                    assert original_wait(timeout=timeout) == 0
                if mode == "wait_error":
                    first_error = wait_failure
                    raise wait_failure
                try:
                    os.kill(os.getpid(), signal.SIGINT)
                except KeyboardInterrupt as error:
                    first_error = error
                    raise
                raise AssertionError("isolated SIGINT was not delivered")
            return original_wait(timeout=timeout)

        child.wait = wait
        return child

    def killpg(pgid, sig):
        nonlocal repeated, kill_refused
        assert owned and pgid == owned[0].pid
        signals.append(int(sig))
        if mode == "signal_error" and sig == signal.SIGTERM:
            raise PermissionError("injected TERM delivery refusal")
        if mode == "kill_error" and sig == signal.SIGKILL and not kill_refused:
            kill_refused = True
            raise PermissionError("injected first KILL delivery refusal")
        result = real_killpg(pgid, sig)
        if mode in ("interrupt", "wait_error") and sig == signal.SIGTERM and not repeated:
            repeated = True
            os.kill(os.getpid(), signal.SIGINT)
        return result

    def write_new(path, value):
        if mode == "record_error" and path == root / "records/command.json":
            raise OSError("injected record destination failure")
        return real_write(path, value)

    stop_calls = 0
    if mode == "entry_interrupt" and hasattr(recorder, "_stop_owned"):
        original_stop = recorder._stop_owned

        def stop_owned(process, state):
            nonlocal stop_calls
            stop_calls += 1
            if stop_calls == 1:
                # Interrupt at the owner-finalization call boundary, before
                # its internal retry loop exists. Ordinary controls do not
                # depend on this private seam and can exercise the old owner.
                os.kill(os.getpid(), signal.SIGINT)
            return original_stop(process, state)

        recorder._stop_owned = stop_owned

    def post_return_trace(frame, event, _argument):
        nonlocal post_return_seen, first_error
        if (event == "line" and frame.f_code is candidate.Recorder.run.__code__
                and not post_return_seen and frame.f_locals.get("process") is not None):
            # First actual line event after handle assignment, independent of
            # source line numbers or a private cleanup helper. The old separate
            # try regions leak here; the common guard owns this exact handle.
            child = frame.f_locals["process"]
            assert owned == [child] and child.returncode is None
            row = await_file(root / "ready.json")
            assert row["leader"] == row["pgid"] == child.pid
            post_return_seen = True
            first_error = KeyboardInterrupt("owned handle bound before first wait")
            raise first_error
        return post_return_trace

    caught = None
    try:
        candidate.subprocess.Popen = spawn
        candidate.os.killpg = killpg
        candidate.write_new = write_new
        try:
            if mode == "post_return":
                sys.settrace(post_return_trace)
            recorder.run("command", command, cwd=root,
                         env={"PYTHONNOUSERSITE": "1", "PYTHONDONTWRITEBYTECODE": "1"},
                         seconds=0.5 if mode == "timeout" else 10)
        except BaseException as error:
            caught = error
        finally:
            sys.settrace(original_trace)
            candidate.subprocess.Popen = real_popen
            candidate.os.killpg = real_killpg
            candidate.write_new = real_write

        assert len(owned) == 1 and owned[0].returncode is not None
        with unittest.TestCase().assertRaises(ChildProcessError):
            os.waitpid(owned[0].pid, os.WNOHANG)
        assert original_handlers == {sig: signal.getsignal(sig) for sig in original_handlers}
        assert sys.gettrace() is original_trace
        if group_case:
            row = await_file(root / "ready.json")
            assert row["leader"] == owned[0].pid == row["pgid"]
            # SIGKILL delivery can precede scheduling of death. Bound this
            # observation, while allowing a dead orphan zombie on CI hosts.
            deadline = time.monotonic() + 5
            while running(row["descendant"]) and time.monotonic() < deadline:
                time.sleep(0.02)
            assert not running(row["descendant"])
            assert int(signal.SIGKILL) in signals
        record_path = root / "records/command.json"
        record = json.loads(record_path.read_text()) if record_path.exists() else None
        if mode == "success":
            assert caught is None
        elif mode in ("nonzero", "timeout"):
            assert isinstance(caught, ValueError)
        else:
            assert caught is first_error
        if record is not None:
            assert record["log"] == candidate.binding(root / "records/command.log")
            assert record["argv"] == command and record["cwd"] == str(root)
            assert record["env"]["PYTHONNOUSERSITE"] == "1"
            assert record["env"]["PYTHONDONTWRITEBYTECODE"] == "1"
        if mode in ("success", "nonzero"):
            assert set(record) == {"argv", "cwd", "env", "exit_code", "timed_out", "elapsed_seconds", "log"}
            assert record["exit_code"] == (0 if mode == "success" else 7)
            assert record["timed_out"] is False and signals == []
        elif mode == "timeout":
            assert record["timed_out"] is True and record["exit_code"] == 0
            assert record["cleanup"]["direct_child_reaped"] is True
        elif mode == "record_error":
            assert record is None and "command record also failed: OSError" in caught.__notes__
        else:
            assert record["exit_code"] is None and record["process_exit_code"] == owned[0].returncode
            assert record["failure"] == type(first_error).__name__
            assert record["cleanup"]["direct_child_reaped"] is True
            assert record["cleanup"]["descendant_reap_observed"] is False
        if mode in ("interrupt", "wait_error"):
            assert repeated and record["cleanup"]["errors"]["KeyboardInterrupt"] >= 1
            assert record["process_exit_code"] == 0  # leader exited; its child still needed KILL
        if mode == "entry_interrupt":
            assert stop_calls == 2 and caught is first_error
            assert "cleanup also failed: KeyboardInterrupt" in caught.__notes__
        if mode == "post_return":
            assert post_return_seen and caught is first_error
            assert record["process_exit_code"] == 0
        if mode == "kill_error":
            assert kill_refused and signals.count(int(signal.SIGKILL)) == 2
            assert record["cleanup"]["errors"]["PermissionError"] == 1
            assert caught is first_error
        if mode == "exit_race":
            assert record["process_exit_code"] == 0 and record["exit_code"] is None
            assert record["cleanup"]["group_after_grace"] == "absent"
        if group_case and mode != "signal_error":
            assert "leader handled TERM" in (root / "records/command.log").read_text()
        save(root / "observed.json", {"passed": True, "mode": mode,
            "original_exception_preserved": first_error is not None and caught is first_error,
            "direct_child_reaped": True, "signals": signals,
            "descendant_live": False if group_case else None,
            "descendant_reap_claimed": False,
            "handlers_unchanged": True, "trace_restored": sys.gettrace() is original_trace,
            "post_return_boundary_observed": post_return_seen})
    finally:
        # Emergency fixture cleanup remains distinct from the Recorder oracle.
        # The original child's session is the only signal target retained here.
        for child in owned:
            try:
                real_killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            child.wait = lambda timeout=None, child=child: subprocess.Popen.wait(child, timeout=timeout)
            child.wait(timeout=5)


@unittest.skipUnless(os.name == "posix", "Recorder selects POSIX process groups")
class RecorderOwnershipTests(unittest.TestCase):
    def run_case(self, mode):
        with tempfile.TemporaryDirectory(prefix="recorder-control-") as directory:
            root = Path(directory)
            with (root / "harness.log").open("wb") as log:
                harness = subprocess.Popen([sys.executable, "-B", str(HERE), "--case", str(root), mode],
                                           stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
                try:
                    code = harness.wait(timeout=30)
                finally:
                    if harness.poll() is None:
                        ready = root / "ready.json"
                        if ready.exists():
                            row = json.loads(ready.read_text())
                            try:
                                os.killpg(row["pgid"], signal.SIGKILL)
                            except ProcessLookupError:
                                pass
                        os.killpg(harness.pid, signal.SIGKILL)
                        harness.wait(timeout=5)
            self.assertEqual(code, 0, (root / "harness.log").read_text())
            observed = json.loads((root / "observed.json").read_text())
            self.assertTrue(observed["passed"])
            self.assertTrue(observed["direct_child_reaped"])
            self.assertTrue(observed["handlers_unchanged"])

    def test_interruption_reaps_leader_and_escalates_remaining_group_despite_second_interrupt(self):
        self.run_case("interrupt")

    def test_unexpected_wait_error_survives_cleanup_interruption(self):
        self.run_case("wait_error")

    def test_second_interruption_before_cleanup_entry_cannot_abandon_the_owner(self):
        self.run_case("entry_interrupt")

    def test_interrupt_after_returned_handle_assignment_is_owned_before_first_wait(self):
        self.run_case("post_return")

    def test_original_interruption_survives_record_write_failure(self):
        self.run_case("record_error")

    def test_group_signal_failure_preserves_original_exception_and_still_escalates(self):
        self.run_case("signal_error")

    def test_failed_kill_is_retried_without_abandoning_remaining_live_descendant(self):
        self.run_case("kill_error")

    def test_exit_zero_race_remains_failed_after_original_interruption(self):
        self.run_case("exit_race")

    def test_ordinary_success_and_nonzero_records_remain_compatible(self):
        for mode in ("success", "nonzero"):
            with self.subTest(mode=mode):
                self.run_case(mode)

    def test_timeout_retains_failure_and_cleans_remaining_group(self):
        self.run_case("timeout")


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1].startswith("--"):
        if sys.argv[1] == "--worker":
            worker(Path(sys.argv[2]))
        elif sys.argv[1] == "--leader":
            leader(Path(sys.argv[2]))
        elif sys.argv[1] == "--case":
            case(Path(sys.argv[2]), sys.argv[3])
        else:
            raise SystemExit("unknown fixture role")
    else:
        unittest.main()
