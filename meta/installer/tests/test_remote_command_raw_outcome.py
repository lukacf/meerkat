"""Local child-process controls for the existing generated command owner.

These exercise raw stream custody and bounded teardown, not native policy,
remote service, SDK cancellation or same-runtime application integration.
"""
import importlib.util
import os
from pathlib import Path
import signal
import subprocess
import sys
import time
from types import SimpleNamespace

import pytest

from launchkit import deployment_deadlines


def load_command_owner():
    path = Path(__file__).parents[1] / 'launchkit/templates_v2/remote_operations.py'
    # The shared renderer, as the compiler exports it; command() never reads a stop budget.
    source = deployment_deadlines.render_destination_shutdown_budget(path.read_text())
    spec = importlib.util.spec_from_file_location('raw_remote_command_owner', path)
    module = importlib.util.module_from_spec(spec)
    exec(compile(source, str(path), 'exec'), module.__dict__)
    return module


def child(code):
    return [sys.executable, '-I', '-c', code]


@pytest.mark.parametrize('exit_code', [0, 23])
def test_raw_outcome_retains_exact_separate_binary_streams_and_actual_exit(tmp_path, capsys, exit_code):
    module = load_command_owner()
    stdout, stderr = b'\xff\x00receipt\r\n', b'\xfeprivate error\n\x00'
    code = f'import os,sys;os.write(1,{stdout!r});os.write(2,{stderr!r});sys.exit({exit_code})'
    outcome = module.command(child(code), root=tmp_path, timeout=2, raw_outcome=True)
    assert outcome == {'returncode': exit_code, 'stdout': stdout, 'stderr': stderr}
    assert capsys.readouterr() == ('', '')


def test_raw_outcome_reports_actual_signal_exit_and_private_output_creation(tmp_path):
    module = load_command_owner()
    artifact = tmp_path / 'native-output-fixture'
    code = f'import os,signal,pathlib;pathlib.Path({str(artifact)!r}).write_text("fixture");os.kill(os.getpid(),signal.SIGTERM)'
    outcome = module.command(child(code), root=tmp_path, timeout=2, raw_outcome=True)
    assert outcome == {'returncode': -signal.SIGTERM, 'stdout': b'', 'stderr': b''}
    assert artifact.stat().st_mode & 0o777 == 0o600


def test_raw_output_drains_both_full_pipes_with_independent_bounds(tmp_path):
    module = load_command_owner()
    # Each stream is valid but together exceeds one stream's allowance. Reading
    # stdout until EOF before reading stderr would deadlock this actual child.
    size = 600 * 1024
    code = f'import os;os.write(1,b"a"*{size});os.write(2,b"b"*{size})'
    outcome = module.command(child(code), root=tmp_path, timeout=3, raw_outcome=True)
    assert outcome == {'returncode': 0, 'stdout': b'a' * size, 'stderr': b'b' * size}


def test_default_merged_decoded_behavior_and_failure_policy_are_unchanged(tmp_path, capsys):
    module = load_command_owner()
    code = 'import os,sys;os.write(1,b"stdout\\n");os.write(2,b"private-stderr\\xff\\n");sys.exit(23)'
    arguments = child(code)
    with pytest.raises(module.DestinationError) as caught:
        module.command(arguments, root=tmp_path, timeout=2)
    assert 'private-stderr' not in str(caught.value)
    assert module.command(arguments, root=tmp_path, timeout=2, allow_failure=True) == (23, 'stdout\nprivate-stderr\ufffd\n')
    assert module.command(child('print("default-ok")'), root=tmp_path, timeout=2) == (0, 'default-ok\n')
    assert capsys.readouterr() == ('', '')


@pytest.mark.parametrize('options', [
    {'raw_outcome': 'yes'}, {'raw_outcome': True, 'terminal': True},
    {'raw_outcome': True, 'timeout': 0}, {'raw_outcome': True, 'timeout': -1},
    {'raw_outcome': True, 'timeout': float('inf')}, {'raw_outcome': True, 'timeout': float('nan')},
    {'raw_outcome': True, 'timeout': True},
])
def test_invalid_raw_options_never_enter_process_owner(tmp_path, monkeypatch, options):
    module = load_command_owner()
    entered = []
    monkeypatch.setattr(module.subprocess, 'Popen', lambda *args, **kwargs: entered.append((args, kwargs)))
    with pytest.raises(module.DestinationError):
        module.command(['inert-invalid-option'], root=tmp_path, **options)
    assert entered == []


@pytest.mark.parametrize('stream', [1, 2])
def test_raw_stream_overflow_kills_and_reaps_its_actual_child(tmp_path, stream):
    module = load_command_owner()
    pidfile = tmp_path / 'owned.pid'
    code = f'import os,pathlib;pathlib.Path({str(pidfile)!r}).write_text(str(os.getpid()))\nwhile True: os.write({stream},b"x"*8192)\n'
    started = time.monotonic()
    with pytest.raises(module.DestinationError, match='output bound'):
        module.command(child(code), root=tmp_path, timeout=2, raw_outcome=True)
    assert time.monotonic() - started < 4
    with pytest.raises(ProcessLookupError):
        os.kill(int(pidfile.read_text()), 0)


def test_raw_command_deadline_remains_finite_after_both_streams_close(tmp_path):
    module = load_command_owner()
    pidfile = tmp_path / 'owned.pid'
    code = f'import os,time,pathlib;pathlib.Path({str(pidfile)!r}).write_text(str(os.getpid()));os.close(1);os.close(2);time.sleep(30)'
    started = time.monotonic()
    with pytest.raises(subprocess.TimeoutExpired):
        module.command(child(code), root=tmp_path, timeout=0.25, raw_outcome=True)
    assert time.monotonic() - started < 3
    with pytest.raises(ProcessLookupError):
        os.kill(int(pidfile.read_text()), 0)


def test_raw_stderr_inheritance_times_out_and_signals_only_owned_unreaped_group(tmp_path, monkeypatch):
    module = load_command_owner()
    pidfile = tmp_path / 'owned-descendant.pid'
    leader = ('import os,pathlib,subprocess,sys\n'
        'child=subprocess.Popen([sys.executable,"-I","-c","import time;time.sleep(30)"],stdout=subprocess.DEVNULL)\n'
        f'pathlib.Path({str(pidfile)!r}).write_text(str(child.pid))\n'
        'os._exit(0)\n')
    unrelated = subprocess.Popen(child('import time;time.sleep(30)'), start_new_session=True,
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    original_signal, signals = module.signal_group, []
    def signal_owned(process, selected):
        signals.append((process.pid, selected, process.returncode))
        original_signal(process, selected)
    monkeypatch.setattr(module, 'signal_group', signal_owned)
    try:
        with pytest.raises(TimeoutError, match='time limit'):
            module.command(child(leader), root=tmp_path, timeout=0.4, raw_outcome=True)
        assert len(signals) == 1 and signals[0][1:] == (signal.SIGKILL, None)
        assert signals[0][0] != os.getpgid(unrelated.pid) and unrelated.poll() is None
        with pytest.raises(ProcessLookupError):
            os.kill(signals[0][0], 0)
    finally:
        unrelated.kill()
        unrelated.wait(timeout=2)


def test_raw_cleanup_failed_reap_remains_failure_and_closes_both_streams(tmp_path, monkeypatch):
    module = load_command_owner()
    waits, closed, signals = [], [], []
    process = SimpleNamespace(pid=7777777, returncode=None,
        stdout=SimpleNamespace(close=lambda: closed.append('stdout')),
        stderr=SimpleNamespace(close=lambda: closed.append('stderr')))
    class Selector:
        def __enter__(self): return self
        def __exit__(self, *args): pass
        def register(self, *args): pass
        def get_map(self): return {}
    def wait(timeout):
        waits.append(timeout)
        raise subprocess.TimeoutExpired('inert-owned-child', timeout)
    process.wait = wait
    monkeypatch.setattr(module.subprocess, 'Popen', lambda *args, **kwargs: process)
    monkeypatch.setattr(module.selectors, 'DefaultSelector', Selector)
    monkeypatch.setattr(module, 'signal_group', lambda owner, selected: signals.append((owner.pid, selected)))
    with pytest.raises(subprocess.TimeoutExpired):
        module.command(['inert-owned-child'], root=tmp_path, timeout=0.1, raw_outcome=True)
    assert len(waits) == 2 and waits[-1] == module.FORCE_REAP_TIMEOUT
    assert signals == [(process.pid, signal.SIGKILL)] and closed == ['stdout', 'stderr']
