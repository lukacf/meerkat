"""Local subprocess fixtures only. No sockets, SSH servers or remote commands."""
import asyncio
import base64
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys

import pytest

from launchkit import ssh_browser_access
from launchkit.ssh_browser_access import BrowserAccessError, SshBrowserAccessTransport


ENDPOINT = {'host': 'fixture.example', 'user': 'fixture', 'ssh_port': 2222}
PORT = 18443


def key(seed=0):
    kind = b'ssh-ed25519'
    data = bytes((value + seed) % 256 for value in range(32))
    blob = len(kind).to_bytes(4, 'big') + kind + len(data).to_bytes(4, 'big') + data
    return (base64.b64encode(blob).decode(),
            'SHA256:' + base64.b64encode(hashlib.sha256(blob).digest()).decode().rstrip('='))


def fixture(tmp_path, *, mode='ready', fingerprint=None):
    public, expected = key()
    fingerprint = fingerprint or expected
    trust = tmp_path / 'known_hosts'
    trust.write_text(f'[fixture.example]:2222 ssh-ed25519 {public}\n')
    record = tmp_path / 'child.json'
    release = tmp_path / 'release'
    script = tmp_path / 'fake-ssh.py'
    verification = (f'debug1: Server host key: ssh-ed25519 {fingerprint}\n'
                    "debug1: Host '[fixture.example]:2222' is known and matches the ED25519 host key.\n")
    ready = verification + (f'debug1: Local forwarding listening on 127.0.0.1 port {PORT}.\n'
                             'debug1: Entering interactive session.\n')
    script.write_text('import json,os,stat,sys,time\nfrom pathlib import Path\n'
        'args=sys.argv[1:]\n'
        "pin=next(value.split('=',1)[1] for value in args if value.startswith('UserKnownHostsFile='))\n"
        f"Path({str(record)!r}).write_text(json.dumps({{'pid':os.getpid(),'guardian':os.getppid(),"
        "'argv':args,'env':dict(os.environ),'pin':pin,'pin_bytes':Path(pin).read_text(),"
        "'mode':stat.S_IMODE(os.stat(pin).st_mode),'uid':os.stat(pin).st_uid}))\n"
        + (f'sys.stderr.write({ready!r}); sys.stderr.flush()\n' if mode in {'ready', 'exit-after-ready'} else '')
        + (f"sys.stderr.write('bind [127.0.0.1]:{PORT}: Address already in use\\nfixture-secret'); sys.stderr.flush(); sys.exit(255)\n" if mode == 'conflict' else '')
        + ("sys.stderr.write('fixture-secret' * 10000); sys.stderr.flush()\n" if mode == 'large' else '')
        + ("sys.stderr.write('fixture-secret'); sys.stderr.flush(); sys.exit(255)\n" if mode == 'error' else '')
        + (f'while not Path({str(release)!r}).exists(): time.sleep(.01)\n' if mode == 'exit-after-ready' else 'while True: time.sleep(1)\n'))
    transport = SshBrowserAccessTransport(executable=sys.executable, argv_prefix=(str(script),),
                                          known_hosts_paths=(trust,))
    return transport, expected, record, release, script, trust


async def read_record(path):
    async def poll():
        while True:
            try:
                return json.loads(path.read_text())
            except (FileNotFoundError, ValueError):
                await asyncio.sleep(.01)
    return await asyncio.wait_for(poll(), 30)


def live(pid):
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return False
    return True


async def absent(pid):
    async def poll():
        while live(pid):
            await asyncio.sleep(.01)
    await asyncio.wait_for(poll(), 4)


@pytest.mark.parametrize('port', [0, 22, 1023, 65536, True, '18443', None])
async def test_invalid_port_never_reads_trust_or_spawns(monkeypatch, port):
    monkeypatch.setattr(ssh_browser_access.subprocess, 'Popen', lambda *a, **k: pytest.fail('Unexpected process.'))
    with pytest.raises(BrowserAccessError) as failure:
        await SshBrowserAccessTransport().open(ENDPOINT, expected_fingerprint=key()[1], port=port)
    assert failure.value.code == 'invalid-request'


@pytest.mark.parametrize('fingerprint,timeout', [
    ('SHA256:bad', 15), (None, 15), ('MD5:bad', 15), (key()[1], 0),
    (key()[1], True), (key()[1], float('nan')), (key()[1], float('inf')),
    (key()[1], 61), (key()[1], 10**500),
])
async def test_invalid_request_is_authored_and_never_spawns(monkeypatch, fingerprint, timeout):
    monkeypatch.setattr(ssh_browser_access.subprocess, 'Popen', lambda *a, **k: pytest.fail('Unexpected process.'))
    with pytest.raises(BrowserAccessError) as failure:
        await SshBrowserAccessTransport().open(ENDPOINT, expected_fingerprint=fingerprint, port=PORT, timeout=timeout)
    assert failure.value.code == 'invalid-request'


@pytest.mark.parametrize('endpoint', [None, {}, {**ENDPOINT, 'host': '-oProxyCommand=bad'},
                                    {**ENDPOINT, 'user': 'bad;user'}, {**ENDPOINT, 'extra': 'secret'}])
async def test_invalid_endpoint_does_not_spawn(monkeypatch, endpoint):
    monkeypatch.setattr(ssh_browser_access.subprocess, 'Popen', lambda *a, **k: pytest.fail('Unexpected process.'))
    with pytest.raises(BrowserAccessError) as failure:
        await SshBrowserAccessTransport().open(endpoint, expected_fingerprint=key()[1], port=PORT)
    assert failure.value.code == 'invalid-endpoint'
    assert 'secret' not in str(failure.value)


async def test_actual_owned_fixture_exact_forwarding_pin_env_and_cleanup(tmp_path, monkeypatch):
    monkeypatch.setenv('OPENAI_API_KEY', 'fixture-provider-secret')
    monkeypatch.setenv('AWS_SESSION_TOKEN', 'fixture-cloud-secret')
    monkeypatch.setenv('SSH_AUTH_SOCK', '/fixture/auth.sock')
    transport, expected, path, _, _, trust = fixture(tmp_path)
    original = trust.read_bytes()
    handle = await transport.open(ENDPOINT, expected_fingerprint=expected, port=PORT, timeout=3)
    try:
        observed = await read_record(path)
        argv = observed['argv']
        assert argv[:5] == ['-F', '/dev/null', '-v', '-N', '-T']
        assert argv[-6:] == ['-p', '2222', '-l', 'fixture', '--', 'fixture.example']
        assert argv.count('-L') == 1 and argv[argv.index('-L')+1] == f'127.0.0.1:{PORT}:127.0.0.1:{PORT}'
        assert not set(argv) & {'-R', '-D', '-g', '-M'}
        for option in ('GlobalKnownHostsFile=/dev/null', 'StrictHostKeyChecking=yes', 'BatchMode=yes',
                       'UpdateHostKeys=no', 'ForwardAgent=no', 'PermitLocalCommand=no', 'ControlMaster=no',
                       'ControlPath=none', 'ClearAllForwardings=no', 'GatewayPorts=no',
                       'ExitOnForwardFailure=yes', 'ConnectTimeout=10', 'FingerprintHash=sha256'):
            assert argv[argv.index(option)-1] == '-o'
        assert observed['mode'] == 0o600 and observed['uid'] == os.getuid()
        assert observed['pin_bytes'].encode() == original
        assert Path(observed['pin']).exists()
        assert observed['env']['SSH_AUTH_SOCK'] == '/fixture/auth.sock'
        assert set(observed['env']) <= {'HOME', 'USER', 'PATH', 'SSH_AUTH_SOCK', 'LANG', 'LC_CTYPE', '__CF_USER_TEXT_ENCODING'}
        assert await handle.observe() is True
    finally:
        assert await handle.close() is True
    assert await handle.close() is True
    assert await handle.observe() is False
    await absent(observed['pid'])
    await absent(observed['guardian'])
    assert not Path(observed['pin']).exists() and trust.read_bytes() == original
    assert SshBrowserAccessTransport()._executable == '/usr/bin/ssh'


async def test_actual_trust_refusal_precedes_forwarding_process(tmp_path):
    transport, expected, record, _, _, trust = fixture(tmp_path)
    trust.write_text(f'[fixture.example]:2222 ssh-ed25519 {key(1)[0]}\n')
    with pytest.raises(BrowserAccessError) as failure:
        await transport.open(ENDPOINT, expected_fingerprint=expected, port=PORT, timeout=2)
    assert failure.value.code == 'host-key-unavailable' and not record.exists()


@pytest.mark.parametrize('mode,code', [('conflict', 'port-unavailable'), ('error', 'connection-unavailable'),
                                     ('large', 'output-limit'), ('waiting', 'timeout')])
async def test_actual_startup_failures_clean_only_owned_process_and_redact(tmp_path, mode, code):
    transport, expected, path, _, _, _ = fixture(tmp_path, mode=mode)
    unrelated = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(30)'])
    try:
        with pytest.raises(BrowserAccessError) as failure:
            await transport.open(ENDPOINT, expected_fingerprint=expected, port=PORT,
                                 timeout=.3 if mode == 'waiting' else 3)
        assert failure.value.code == code
        assert 'fixture-secret' not in str(failure.value) and 'fixture.example' not in str(failure.value)
        assert unrelated.poll() is None
        observed = await read_record(path)
        await absent(observed['pid'])
        await absent(observed['guardian'])
        assert not Path(observed['pin']).exists()
    finally:
        unrelated.terminate()
        unrelated.wait(timeout=3)


async def test_actual_verified_key_mismatch_cleans_before_typed_refusal(tmp_path):
    transport, expected, path, _, _, _ = fixture(tmp_path, fingerprint=key(1)[1])
    with pytest.raises(BrowserAccessError) as failure:
        await transport.open(ENDPOINT, expected_fingerprint=expected, port=PORT, timeout=3)
    assert failure.value.code == 'host-key-unavailable'
    observed = await read_record(path)
    await absent(observed['pid'])
    assert not Path(observed['pin']).exists()


async def test_actual_startup_cancellation_reaps_forwarder_and_guardian(tmp_path):
    transport, expected, path, _, _, _ = fixture(tmp_path, mode='waiting')
    operation = asyncio.create_task(transport.open(ENDPOINT, expected_fingerprint=expected, port=PORT))
    observed = await read_record(path)
    operation.cancel()
    with pytest.raises(asyncio.CancelledError):
        await operation
    await absent(observed['pid'])
    await absent(observed['guardian'])
    assert not Path(observed['pin']).exists()


async def test_actual_exit_observation_retains_cleanup_evidence_for_close(tmp_path):
    transport, expected, path, release, _, _ = fixture(tmp_path, mode='exit-after-ready')
    handle = await transport.open(ENDPOINT, expected_fingerprint=expected, port=PORT, timeout=3)
    observed = await read_record(path)
    release.touch()
    await absent(observed['pid'])
    assert await handle.observe() is False
    assert await handle.close() is True
    assert not Path(observed['pin']).exists()


@pytest.mark.parametrize('crash', [False, True])
async def test_actual_host_crash_eof_reaps_forwarder_without_adopting_pid(tmp_path, crash):
    _, expected, path, _, script, trust = fixture(tmp_path)
    owner = tmp_path / 'owner.py'
    installer = str(Path(__file__).resolve().parents[1])
    owner.write_text('import asyncio,os,sys\n'
        f'sys.path.insert(0,{installer!r})\n'
        'from launchkit.ssh_browser_access import SshBrowserAccessTransport\n'
        'async def main():\n'
        f'    transport=SshBrowserAccessTransport(executable={sys.executable!r},argv_prefix=({str(script)!r},),known_hosts_paths=({str(trust)!r},))\n'
        f'    handle=await transport.open({ENDPOINT!r},expected_fingerprint={expected!r},port={PORT},timeout=3)\n'
        + ('    os.kill(os.getpid(),9)\n' if crash else '    os._exit(0)\n') +
        'asyncio.run(main())\n')
    process = subprocess.Popen([sys.executable, str(owner)], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    observed = await read_record(path)
    result = await asyncio.to_thread(process.communicate, timeout=5)
    assert process.returncode == (-signal.SIGKILL if crash else 0), result[1].decode()
    await absent(observed['pid'])
    await absent(observed['guardian'])
    assert not Path(observed['pin']).exists()


async def test_actual_repeated_startup_cancellation_settles_owned_cleanup(tmp_path, monkeypatch):
    transport, expected, path, _, _, _ = fixture(tmp_path, mode='waiting')
    original = ssh_browser_access._Handle._close
    entered = asyncio.Event()
    release = asyncio.Event()
    async def delayed(handle):
        entered.set()
        await release.wait()
        return await original(handle)
    monkeypatch.setattr(ssh_browser_access._Handle, '_close', delayed)
    operation = asyncio.create_task(transport.open(ENDPOINT, expected_fingerprint=expected, port=PORT))
    observed = await read_record(path)
    operation.cancel()
    await entered.wait()
    operation.cancel()
    await asyncio.sleep(0)
    assert not operation.done()
    release.set()
    with pytest.raises(asyncio.CancelledError):
        await operation
    await absent(observed['pid'])
    await absent(observed['guardian'])
    assert not Path(observed['pin']).exists()


async def test_actual_cancelled_startup_with_unconfirmed_cleanup_carries_custody(tmp_path, monkeypatch):
    transport, expected, path, _, _, _ = fixture(tmp_path, mode='waiting')
    original = ssh_browser_access._Handle._close
    async def refused(handle):
        return False
    monkeypatch.setattr(ssh_browser_access._Handle, '_close', refused)
    operation = asyncio.create_task(transport.open(ENDPOINT, expected_fingerprint=expected, port=PORT))
    observed = await read_record(path)
    operation.cancel()
    with pytest.raises(BrowserAccessError) as failure:
        await operation
    assert failure.value.code == 'cleanup-unavailable'
    assert failure.value.handle._process.pid == observed['guardian']
    monkeypatch.setattr(ssh_browser_access._Handle, '_close', original)
    assert await failure.value.handle.close() is True
    await absent(observed['pid'])
    await absent(observed['guardian'])


async def test_cancelled_inner_cleanup_does_not_spin_or_claim_success():
    class Handle:
        async def _close(self):
            raise asyncio.CancelledError()
    assert await asyncio.wait_for(ssh_browser_access._settle_cleanup(Handle()), .5) == (False, True)


async def test_guardian_creation_error_never_exposes_private_exception(tmp_path, monkeypatch):
    transport, expected, _, _, _, _ = fixture(tmp_path)
    async def pin(*args):
        return b'public fixture pin'
    monkeypatch.setattr(transport._ssh, '_pin', pin)
    def unavailable(*args, **kwargs):
        raise OSError('fixture-provider-secret private-user-path')
    monkeypatch.setattr(ssh_browser_access.subprocess, 'Popen', unavailable)
    with pytest.raises(BrowserAccessError) as failure:
        await transport.open(ENDPOINT, expected_fingerprint=expected, port=PORT)
    assert failure.value.code == 'connection-unavailable'
    assert 'fixture-provider-secret' not in str(failure.value)
    assert 'private-user-path' not in str(failure.value)


async def test_unconfirmed_startup_cleanup_retains_private_exact_handle(tmp_path, monkeypatch):
    transport, expected, path, _, _, _ = fixture(tmp_path, mode='error')
    original = ssh_browser_access._Handle._close
    held = []
    async def refusal(handle):
        held.append(handle)
        return False
    monkeypatch.setattr(ssh_browser_access._Handle, '_close', refusal)
    try:
        with pytest.raises(BrowserAccessError) as failure:
            await transport.open(ENDPOINT, expected_fingerprint=expected, port=PORT, timeout=3)
        assert failure.value.code == 'cleanup-unavailable'
        assert failure.value.handle is held[0]
        assert 'handle' not in str(failure.value)
    finally:
        monkeypatch.setattr(ssh_browser_access._Handle, '_close', original)
        if held:
            async def close_until_confirmed():
                # An initial guardian frame can honestly say cleanup is still
                # pending. Keep its exact handle until eventual confirmation.
                while not await held[0].close():
                    await asyncio.sleep(.01)
            await asyncio.wait_for(close_until_confirmed(), ssh_browser_access.CLEANUP_TIMEOUT + 6)
    observed = await read_record(path)
    await absent(observed['pid'])
    await absent(observed['guardian'])


@pytest.mark.parametrize('phase,deadline,expected_preparation', [
    ('arrival', 99.0, []),
    ('pin-write', 105.0, ['directory', 'pin-write']),
])
def test_actual_guardian_expired_deadline_never_admits_forwarder(
        tmp_path, phase, deadline, expected_preparation):
    """Execute the embedded guardian, with only its child launcher made inert."""
    report = tmp_path / 'guardian-report.json'
    wrapper = tmp_path / 'guardian-deadline.py'
    wrapper.write_text(
        'import json,os,subprocess,tempfile,time\n'
        'from pathlib import Path\n'
        'clock=[100.0]\n'
        'preparation=[]\n'
        'admissions=[]\n'
        'time.monotonic=lambda: clock[0]\n'
        'original_directory=tempfile.TemporaryDirectory\n'
        'original_fdopen=os.fdopen\n'
        'def private_directory(*args,**kwargs):\n'
        f'    kwargs["dir"]={str(tmp_path)!r}\n'
        '    directory=original_directory(*args,**kwargs)\n'
        '    preparation.append("directory")\n'
        '    return directory\n'
        'class PinWrite:\n'
        '    def __init__(self,channel): self.channel=channel\n'
        '    def __enter__(self): return self\n'
        '    def __exit__(self,*args): return self.channel.__exit__(*args)\n'
        '    def write(self,pin):\n'
        '        count=self.channel.write(pin)\n'
        '        preparation.append("pin-write")\n'
        f'        if {phase!r}=="pin-write": clock[0]=106.0\n'
        '        return count\n'
        'def pin_fdopen(fd,mode,*args,**kwargs):\n'
        '    channel=original_fdopen(fd,mode,*args,**kwargs)\n'
        '    return PinWrite(channel) if mode=="wb" else channel\n'
        'def refuse_child(*args,**kwargs):\n'
        '    admissions.append("attempted")\n'
        '    raise RuntimeError("Inert fixture refuses all forwarding children.")\n'
        'tempfile.TemporaryDirectory=private_directory\n'
        'os.fdopen=pin_fdopen\n'
        'subprocess.Popen=refuse_child\n'
        'try:\n'
        f'    exec(compile({ssh_browser_access.GUARDIAN!r},"<actual-guardian>","exec"))\n'
        'finally:\n'
        f'    Path({str(report)!r}).write_text(json.dumps(dict(preparation=preparation,admissions=admissions)))\n')
    request = dict(argv=['inert-forwarder', 'UserKnownHostsFile=@PIN@'],
                   pin=base64.b64encode(b'public fixture pin\n').decode(),
                   port=PORT, timeout=5.0, deadline=deadline)
    # subprocess.run kills and waits for this exact wrapper on timeout. Its
    # patched launcher cannot create any descendant, socket or SSH process.
    result = subprocess.run([sys.executable, '-I', str(wrapper)],
                            input=(json.dumps(request) + '\n').encode(),
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            timeout=3, check=False)
    assert result.returncode == 0, result.stderr.decode()
    frames = [json.loads(line) for line in result.stdout.splitlines()]
    assert frames == [dict(event='error', cleaned=True, code='timeout')]
    observed = json.loads(report.read_text())
    assert observed['admissions'] == []
    assert observed['preparation'] == expected_preparation
    assert list(tmp_path.glob('mobkit-browser-pin-*')) == []
