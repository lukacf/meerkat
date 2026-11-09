"""Actual bounded listener parser over synthetic proc files, not native proof.

The existing source predicates, fd-held directory checks, ancestry traversal,
socket parser, deadlines and repeated observations execute after integration.
Kernel proc/cgroup observations are explicit inert fixtures. No native runtime,
SDK, service manager, SSH, tunnel, HTTP or application transaction is executed.
"""
from copy import deepcopy
import errno
import os
import time

import pytest

from launchkit import remote_destination as controller
from test_remote_destination import listener_fixture


def process(proc, pid, *, parent, group=None, session=None, ticks='100', cgroup=None):
    root = proc / str(pid)
    for directory in ('fd', 'ns'):
        (root / directory).mkdir(parents=True, exist_ok=True)
    namespace = root / 'ns/net'
    if not namespace.is_symlink():
        namespace.symlink_to('net:[123]')
    fields = ['S', str(parent), str(group or pid), str(session or pid)] + ['0'] * 15 + [ticks] + ['0'] * 3
    (root / 'stat').write_text(f'{pid} (private-process-canary) ' + ' '.join(fields))
    (root / 'cgroup').write_text('0::' + (cgroup or '/shared-user-group') + '\n')
    return root


@pytest.fixture
def product_listener(listener_fixture):
    base, proc, members, owner, operations, state, row = listener_fixture
    process(proc, 41, parent=43, ticks='90')
    process(proc, 42, parent=41)
    anchor = {'kind': 'interactive-handle', 'pid': 41, 'parent_pid': 43,
        'lifecycle_id': '11111111-1111-4111-8111-111111111111'}
    runtime = {'runtime_pid': 42, 'source_digest': 'a' * 64, 'metadata_sha256': 'b' * 64,
        'startup_attempt_id': '22222222-2222-4222-8222-222222222222'}
    return base, proc, members, owner, anchor, runtime, state, row


def observe(f, require_current=lambda: None):
    _, _, _, owner, anchor, runtime, _, _ = f
    return controller._product_listener_proof(owner, runtime, anchor, 9912,
        time.monotonic() + 4, require_current)


@pytest.mark.parametrize('address', ['0100007F', '00000000'])
def test_product_listener_joins_child_session_instead_of_shared_worker_group(product_listener, address):
    f = product_listener
    _, proc, _, _, anchor, runtime, _, row = f
    (proc / 'self/net/tcp').write_text('header\n' + row(address=address))
    # The app root and its helper deliberately belong to distinct groups.
    process(proc, 45, parent=42, group=45, session=45, ticks='110')
    (proc / '42/fd/7').unlink()
    (proc / '45/fd/9').symlink_to('socket:[9876]')
    first = observe(f)
    assert isinstance(first, str) and len(first) == 64 and observe(f) == first
    changed = deepcopy(runtime)
    changed['metadata_sha256'] = 'c' * 64
    assert controller._product_listener_proof(f[3], changed, anchor, 9912,
        time.monotonic() + 4, lambda: None) != first


@pytest.mark.parametrize('problem', ['foreign-holder', 'sibling-runtime', 'shared-worker-group',
    'runtime-namespace', 'worker-parent', 'child-before-parent'])
def test_product_listener_refuses_foreign_process_authority_even_in_shared_user_group(product_listener, problem):
    f = product_listener
    _, proc, _, _, _, _, _, _ = f
    if problem == 'foreign-holder':
        other = process(proc, 44, parent=43)
        (other / 'fd/8').symlink_to('socket:[9876]')
    elif problem == 'sibling-runtime':
        process(proc, 42, parent=43)
    elif problem == 'shared-worker-group':
        process(proc, 42, parent=41, group=41, session=41)
    elif problem == 'runtime-namespace':
        (proc / '42/ns/net').unlink()
        (proc / '42/ns/net').symlink_to('net:[999]')
    elif problem == 'worker-parent':
        process(proc, 41, parent=99, ticks='90')
    else:
        process(proc, 42, parent=41, ticks='80')
    with pytest.raises((ValueError, OSError)):
        observe(f)


@pytest.mark.parametrize('boundary', ['birth', 'socket', 'namespace', 'original-owner'])
def test_product_listener_rechecks_kernel_and_original_owner_after_first_snapshot(product_listener, monkeypatch, boundary):
    f = product_listener
    _, proc, _, _, _, _, _, row = f
    original = controller._product_listener_snapshot
    calls = []
    def snapshot(*args):
        value = original(*args)
        calls.append(1)
        if len(calls) == 1:
            if boundary == 'birth':
                process(proc, 42, parent=41, ticks='101')
            elif boundary == 'socket':
                (proc / 'self/net/tcp').write_text('header\n' + row(inode=8888))
            elif boundary == 'namespace':
                (proc / '42/ns/net').unlink()
                (proc / '42/ns/net').symlink_to('net:[999]')
        return value
    monkeypatch.setattr(controller, '_product_listener_snapshot', snapshot)
    def current():
        if calls and boundary == 'original-owner':
            raise ValueError('The original application handle changed.')
    with pytest.raises((ValueError, OSError)):
        observe(f, current)


def test_product_background_listener_requires_ancestry_as_well_as_actual_service_cgroup(product_listener):
    f = product_listener
    _, proc, members, _, anchor, _, state, _ = f
    group = state['ControlGroup']
    process(proc, 41, parent=43, ticks='90', cgroup=group)
    process(proc, 42, parent=41, cgroup=group)
    members.write_text('41\n42\n')
    anchor.update(kind='background-manager', control_group=group,
        instance_id='33333333-3333-4333-8333-333333333333', service={'unit_sha256': 'd' * 64})
    token = observe(f)
    assert isinstance(token, str) and len(token) == 64
    # Even the exact unit cgroup cannot grant a foreign sibling socket custody.
    other = process(proc, 44, parent=43, cgroup=group)
    (other / 'fd/8').symlink_to('socket:[9876]')
    members.write_text('41\n42\n44\n')
    with pytest.raises((ValueError, OSError)):
        observe(f)


def test_product_listener_refuses_unavailable_observation_budget(product_listener):
    f = product_listener
    with pytest.raises(ValueError):
        controller._product_listener_proof(f[3], f[5], f[4], 9912, time.monotonic() - 1,
            lambda: None)


def test_unrelated_same_user_processes_do_not_consume_retained_directory_budget(product_listener, monkeypatch):
    f = product_listener
    proc = f[1]
    for pid in range(1000, 1512):
        process(proc, pid, parent=43)
    opened, peak = set(), []
    original_open, original_close = os.open, os.close
    def bounded_open(path, flags, *args, **kwargs):
        if flags & os.O_DIRECTORY and len(opened) >= 32:
            raise OSError(errno.EMFILE, 'Synthetic directory descriptor budget exhausted.')
        descriptor = original_open(path, flags, *args, **kwargs)
        if flags & os.O_DIRECTORY:
            opened.add(descriptor)
            peak.append(len(opened))
        return descriptor
    def close(descriptor):
        opened.discard(descriptor)
        return original_close(descriptor)
    monkeypatch.setattr(os, 'open', bounded_open)
    monkeypatch.setattr(os, 'close', close)
    token = observe(f)
    assert isinstance(token, str) and len(token) == 64
    assert max(peak) < 16 and not opened
