"""Migration execution custody against an inert engine and a real SQLite owner.

This qualifies the owner protocol and interruption decisions, not Docker behavior.
"""
import asyncio
import copy
import csv
import json
import re
import sqlite3
import sys
import importlib.util
from pathlib import Path

import pytest

import meerkat_toolkit.compose as module
from meerkat_toolkit.compose import CommandResult, ComposeError, ComposeRuntime
from test_compose import FakeDriver, digest, encoded
from test_compose_successor import RetainedRemovalDriver, retained_removal_plans, version, effects


def reverse_owner(tmp_path):
    """The same finite SQLite owner used by the separately scheduled engine trial."""
    path = Path(__file__).resolve().parents[3] / 'docs/verification/20261006-compose-reverse-migration/sqlite_owner.py'
    spec = importlib.util.spec_from_file_location('reverse_owner_fixture', path)
    owner = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(owner)
    owner.DATABASE = tmp_path / 'owner.sqlite'
    owner.EFFECTS = tmp_path / 'effect-attempts.jsonl'
    owner.COMPONENT, owner.OWNER = 'db', 'host'
    return owner


def migration_plans(tmp_path):
    a, removed, later, tools = retained_removal_plans(tmp_path)
    b = copy.deepcopy(a)
    b['plan']['volumes']['data']['format'] = '2'
    b['plan']['retained_volume_labels'] = removed['plan']['retained_volume_labels']
    b['plan']['components']['db']['image'] = 'example/db-next@sha256:' + 'c' * 64
    document = json.loads(b['plan']['compose_json'])
    document['services']['db']['image'] = b['plan']['components']['db']['image']
    document['volumes'] = {'fixture-data': {'name': 'fixture-data', 'external': True}}
    b['plan']['compose_json'] = encoded(document).decode()
    b['plan']['files']['compose.json'].update(contents=encoded(document).decode(), sha256=digest(encoded(document)))
    b['plan']['state_migration'] = {'protocol_version': 1, 'namespace': 'data', 'component': 'db',
        'owner': 'host', 'from_format': '1', 'to_format': '2', 'operation': 'convert',
        'artifact_digest': 'c' * 64, 'apply': {'executable': 'bin/owner', 'arguments': ['apply']},
        'readback': {'executable': 'bin/owner', 'arguments': ['readback']}, 'timeout_ms': 1000}
    b = version(b, b'{"v":2}')
    # Same-format C intentionally retains static migration capability metadata.
    c = version(b, b'{"v":3}')
    removed['plan']['volumes']['data']['format'] = '2'
    d = version(removed, b'{"v":4}')
    e = version(d, b'{"v":5}')
    return [a, b, c, d, e], tools


class MigrationDriver(RetainedRemovalDriver):
    def __init__(self, plans, workspace, owner):
        super().__init__(*plans[:3], workspace)
        self.plans = {('{"v":%d}' % (index + 1)).encode(): plan for index, plan in enumerate(plans)}
        self.owner = owner
        with sqlite3.connect(owner) as db:
            db.execute('CREATE TABLE state(format TEXT, cursor INTEGER, effects TEXT, receipt TEXT)')
            db.execute('INSERT INTO state VALUES(?, ?, ?, NULL)', ('1', 7, '["A"]'))
        self.apply_entries = 0
        self.readback_entries = 0
        self.lose_apply = False
        self.hold_apply = False
        self.no_receipt = False
        self.logs = {}
        self.worker_inputs = []
        self.bad_image = False
        self.malformed_readback = False

    def container(self, service, **kwargs):
        if service != 'db' or self.volume is None:
            return super().container(service, **kwargs)
        identity = FakeDriver.container(self, service, **kwargs)
        self.containers[identity]['Mounts'].append({'Type': 'volume', 'Name': 'fixture-data',
            'Source': '/engine/fixture-data', 'Destination': '/state', 'RW': True})
        return identity

    def owner_call(self, role, binding):
        with sqlite3.connect(self.owner) as db:
            fmt, cursor, effects_json, receipt = db.execute('SELECT * FROM state').fetchone()
            if role == 'apply':
                self.apply_entries += 1
                assert fmt == '1' and receipt is None, 'Apply must never run twice'
                saved = {'binding': binding, 'evidence_sha256': digest(encoded([cursor, effects_json]))}
                db.execute('UPDATE state SET format=?,receipt=?', ('2', None if self.no_receipt else json.dumps(saved)))
                return {}
            self.readback_entries += 1
            if receipt is not None:
                return {'protocol_version': 1, 'status': 'committed', 'observed_format': fmt, 'receipt': json.loads(receipt)}
            return {'protocol_version': 1, 'status': 'ready' if fmt == '1' else 'unconfirmed',
                    'observed_format': fmt, 'receipt': None}

    async def run(self, argv, **kwargs):
        args = argv[5:]
        if len(argv) == 2:
            self.calls.append((list(argv), dict(kwargs['env'])))
            raw = kwargs['stdin']
            envelope = json.loads(raw)
            requests = envelope['requests'] if envelope['schema_version'] == 2 else [envelope['predecessor_request'], envelope['successor_request']]
            return CommandResult(0, encoded({'schema_version': 2, 'status': 'prepared_successor',
                'transition_sha256': digest(raw), 'predecessor': self.plans[requests[-2].encode()],
                'successor': self.plans[requests[-1].encode()]}))
        if args[:2] == ['image', 'inspect'] and self.bad_image and 'db-next' in args[-1]:
            return CommandResult(1)
        if args[:2] == ['container', 'create']:
            self.calls.append((list(argv), dict(kwargs['env'])))
            def arg(name):
                return args[args.index(name) + 1]
            labels = dict(args[index + 1].split('=', 1) for index, item in enumerate(args) if item == '--label')
            name = arg('--name')
            role = labels['org.meerkat.migration-role']
            identity = str(self.next_id).zfill(64)
            self.next_id += 1
            mounts = []
            for index, value in enumerate(args):
                if value != '--mount':
                    continue
                fields = {pair.split('=', 1)[0]: pair.split('=', 1)[1] if '=' in pair else True
                          for pair in next(csv.reader([args[index + 1]]))}
                item = {'Type': fields['type'], 'Source': fields['source'], 'Destination': fields['target'],
                        'RW': 'readonly' not in fields}
                if fields['type'] == 'volume':
                    item['Name'] = fields['source']
                    assert 'volume-nocopy' in fields
                mounts.append(item)
            assert arg('--network') == 'none' and arg('--pull') == 'never'
            assert all(not item['RW'] for item in mounts) if role == 'readback' else True
            self.containers[identity] = {'Id': identity, 'Name': '/' + name, 'Image': 'sha256:' + 'd' * 64,
                'Config': {'Labels': labels, 'Image': args[-2], 'Entrypoint': [arg('--entrypoint')],
                    'Cmd': [args[-1]], 'WorkingDir': arg('--workdir'), 'User': arg('--user'),
                    'Env': [args[index + 1] for index, item in enumerate(args) if item == '--env'],
                    'OpenStdin': True, 'Healthcheck': {'Test': ['NONE']}},
                'HostConfig': {'Memory': int(arg('--memory')), 'MemorySwap': int(arg('--memory-swap')),
                    'PidsLimit': int(arg('--pids-limit')), 'NanoCpus': round(float(arg('--cpus')) * 1000000000),
                    'Init': True, 'RestartPolicy': {'Name': 'no'}, 'NetworkMode': 'none',
                    'ReadonlyRootfs': '--read-only' in args, 'CapDrop': ['ALL'], 'SecurityOpt': ['no-new-privileges']},
                'Mounts': mounts, 'NetworkSettings': {'Networks': {}},
                'State': {'Running': False, 'Status': 'created', 'ExitCode': 0}}
            return CommandResult(0, (identity + '\n').encode())
        if args[:1] == ['ps']:
            self.calls.append((list(argv), dict(kwargs['env'])))
            filters = [args[index + 1] for index, item in enumerate(args) if item == '--filter']
            items = self.containers.items()
            for value in filters:
                if value.startswith('name=^/'):
                    pattern = value[len('name='):]
                    items = [(key, item) for key, item in items if re.fullmatch(pattern, item['Name'])]
                elif value.startswith('volume='):
                    items = [(key, item) for key, item in items if any(m.get('Name') == value[7:] for m in item['Mounts'])]
            ids = sorted(key for key, _ in items)
            return CommandResult(0, ('\n'.join(ids) + ('\n' if ids else '')).encode())
        if args[:2] == ['container', 'start'] and '--attach' in args:
            self.calls.append((list(argv), dict(kwargs['env'])))
            item = self.containers[args[-1]]
            role = item['Config']['Labels']['org.meerkat.migration-role']
            binding = json.loads(kwargs['stdin'])
            self.worker_inputs.append((role, binding))
            if role == 'apply' and self.hold_apply:
                item['State'].update(Running=True, Status='running')
                raise TimeoutError('apply still running')
            value = self.owner_call(role, binding)
            self.logs[args[-1]] = encoded(value)
            if role == 'readback' and self.malformed_readback:
                self.malformed_readback = False
                self.logs[args[-1]] = b'invalid owner response'
            item['State'].update(Running=False, Status='exited')
            if role == 'apply' and self.lose_apply:
                lost = self.lose_apply
                self.lose_apply = False
                if lost == 'cancelled':
                    raise asyncio.CancelledError
                raise TimeoutError('actual owner commit, response withheld')
            return CommandResult(0, encoded(value))
        if args[:2] == ['container', 'logs']:
            self.calls.append((list(argv), dict(kwargs['env'])))
            return CommandResult(0, self.logs[args[-1]])
        return await super().run(argv, **kwargs)


async def setup(tmp_path):
    plans, tools = migration_plans(tmp_path)
    driver = MigrationDriver(plans, tmp_path / 'workspace', tmp_path / 'state.db')
    runtime = ComposeRuntime(driver.workspace, tools['planner']['path'], tools['planner']['sha256'], runner=driver)
    await runtime.prepare(b'{"v":1}')
    await runtime.start(plans[0]['plan_sha256'])
    proposal = await runtime.prepare_successor(b'{"v":2}', plans[0]['plan_sha256'])
    return runtime, driver, plans, proposal


@pytest.mark.asyncio
@pytest.mark.parametrize('lost', [False, True, 'cancelled'])
async def test_owner_receipt_switches_once_then_dormant_history_keeps_logical_format(tmp_path, lost):
    runtime, driver, plans, proposal = await setup(tmp_path)
    a, b, c, d, e = plans
    physical = copy.deepcopy(driver.volume)
    before = await runtime.observe()
    driver.lose_apply = lost
    if lost == 'cancelled':
        with pytest.raises(asyncio.CancelledError):
            await runtime.adopt_successor(proposal['transition_sha256'], a['plan_sha256'], before['observation_sha256'])
        result = {'status': 'unconfirmed'}
    else:
        result = await runtime.adopt_successor(proposal['transition_sha256'], a['plan_sha256'], before['observation_sha256'])
    if lost:
        assert result['status'] == 'unconfirmed'
        runtime = ComposeRuntime(runtime.workspace, runtime.planner, runtime.planner_sha256, runner=driver)
        pending = await runtime.observe()
        assert pending['phase'] == 'migrate' and pending['observation']['plan_sha256'] == a['plan_sha256']
        assert len(pending['observation']['migration_workers']) == 1
        result = await runtime.reconcile_successor(proposal['transition_sha256'], pending['observation_sha256'])
    assert result['status'] == 'observed' and result['plan_sha256'] == b['plan_sha256']
    assert driver.apply_entries == 1 and driver.volume == physical
    for index, (old, new) in enumerate(zip(plans[1:], plans[2:]), start=3):
        runtime = ComposeRuntime(runtime.workspace, runtime.planner, runtime.planner_sha256, runner=driver)
        proposal = await runtime.prepare_successor(('{"v":%d}' % index).encode(), old['plan_sha256'])
        raw = json.loads((runtime.workspace / '.successors' / proposal['transition_sha256'] / 'request.json').read_bytes())
        assert raw['schema_version'] == 2 and len(raw['requests']) == index
        before = await runtime.observe()
        result = await runtime.adopt_successor(proposal['transition_sha256'], old['plan_sha256'], before['observation_sha256'])
        assert result['status'] == 'observed' and result['plan_sha256'] == new['plan_sha256']
        assert driver.apply_entries == 1 and driver.volume == physical
    custody = json.loads((runtime.workspace / 'custody.json').read_bytes())
    assert custody['schema_version'] == 3 and len(custody['history']) == 4
    assert sum('migration' in item for item in custody['history']) == 1
    with sqlite3.connect(driver.owner) as db:
        fmt, cursor, effects_json, receipt = db.execute('SELECT * FROM state').fetchone()
    assert (fmt, cursor, effects_json) == ('2', 7, '["A"]')
    binding = json.loads(receipt)['binding']
    assert binding['physical_volume_observation_sha256'] == digest(encoded(before['volumes']['data']))
    assert custody['history'][0]['migration']['receipt']['receipt']['binding'] == binding
    assert plans[-1]['plan']['volumes']['data']['format'] == '2'
    assert plans[-1]['plan']['retained_volume_labels'] == plans[1]['plan']['retained_volume_labels']


@pytest.mark.asyncio
@pytest.mark.parametrize('mode', ['running', 'missing', 'no_receipt'])
async def test_attempted_apply_never_reenters_or_activates_from_absence_or_admission(tmp_path, mode):
    runtime, driver, plans, proposal = await setup(tmp_path)
    driver.hold_apply = mode == 'running'
    driver.lose_apply = mode == 'missing'
    driver.no_receipt = mode == 'no_receipt'
    before = await runtime.observe()
    result = await runtime.adopt_successor(proposal['transition_sha256'], plans[0]['plan_sha256'], before['observation_sha256'])
    assert result['status'] == 'unconfirmed'
    if mode == 'missing':
        driver.containers = {key: item for key, item in driver.containers.items()
                             if item['Config']['Labels'].get('org.meerkat.migration-role') != 'apply'}
    runtime = ComposeRuntime(runtime.workspace, runtime.planner, runtime.planner_sha256, runner=driver)
    pending = await runtime.observe()
    assert pending['phase'] == 'migrate' and pending['observation']['containers'] == {}
    at = len(driver.calls)
    with pytest.raises(ComposeError, match='stale_observation'):
        await runtime.reconcile_successor(proposal['transition_sha256'], before['observation_sha256'])
    assert effects(driver, at) == []
    entries = driver.apply_entries
    starts = sum(argv[5:7] == ['container', 'start'] for argv, _ in driver.calls)
    result = await runtime.reconcile_successor(proposal['transition_sha256'], pending['observation_sha256'])
    assert result['status'] == 'unconfirmed' and driver.apply_entries == entries
    assert not any('up' in argv for argv, _ in driver.calls[at:])
    if mode in ('running', 'missing'):
        assert sum(argv[5:7] == ['container', 'start'] for argv, _ in driver.calls) == starts


@pytest.mark.asyncio
async def test_migrator_preflight_refuses_before_retirement_and_wrong_admission_is_inert(tmp_path):
    runtime, driver, plans, proposal = await setup(tmp_path)
    before = await runtime.observe()
    at = len(driver.calls)
    with pytest.raises(ComposeError, match='plan_changed'):
        await runtime.adopt_successor(proposal['transition_sha256'], plans[1]['plan_sha256'], before['observation_sha256'])
    assert effects(driver, at) == []
    driver.bad_image = True
    with pytest.raises(ComposeError, match='engine_observation_unavailable'):
        await runtime.adopt_successor(proposal['transition_sha256'], plans[0]['plan_sha256'], before['observation_sha256'])
    assert effects(driver, at) == [] and driver.apply_entries == 0
    assert (runtime.workspace / 'request.json').read_bytes() == b'{"v":1}'


# Existing pure-planner fixture requires an explicit executable, never a build.
from test_compose_proposal import EXAMPLE, planner


@pytest.mark.asyncio
async def test_actual_planner_runtime_preparation_is_inert_and_binds_migration_profile(tmp_path, planner):
    request = json.loads(EXAMPLE.read_bytes())
    profile = json.loads(request['profile_json'])
    for name in ('docker', 'compose'):
        tool = tmp_path / name
        tool.write_bytes(b'Never executed by this test')
        profile['engine'][name] = {'path': str(tool), 'sha256': digest(tool.read_bytes())}
    request['profile_json'] = encoded(profile).decode()
    request['profile_sha256'] = digest(encoded(profile))
    class PlannerOnly(module.ProcessRunner):
        def __init__(self):
            self.calls = []
        async def run(self, argv, **kwargs):
            assert argv[0] == str(planner[0]) and len(argv) in (1, 2), 'No engine calls during preparation'
            self.calls.append(list(argv))
            return await super().run(argv, **kwargs)
    runner = PlannerOnly()
    runtime = ComposeRuntime(tmp_path / 'canonical', *planner, runner=runner)
    a = await runtime.prepare(encoded(request))
    successor = copy.deepcopy(request)
    package = json.loads(successor['package_json'])
    package['identity']['version'] = '2.0.0'
    manifest = json.loads(successor['manifest_json'])
    manifest['package'] = package['identity']
    manifest['package_content_digest'] = digest(encoded(package))
    database = next(item for item in manifest['components'] if item['id'] == 'database')
    database['state'][0].update(format='2', migrations=[{'from': '1', 'operation': 'convert-data',
        'artifact_digest': database['source']['content_digest'], 'reversible': False}])
    successor['package_json'] = encoded(package).decode()
    successor['manifest_json'] = encoded(manifest).decode()
    profile['state_migration'] = {'protocol_version': 1, 'namespace': 'data', 'operation': 'convert-data',
        'apply': {'executable': 'usr/local/bin/migrate', 'arguments': ['apply']},
        'readback': {'executable': 'usr/local/bin/migrate', 'arguments': ['readback']}, 'timeout_ms': 30000}
    successor['profile_json'] = encoded(profile).decode()
    successor['profile_sha256'] = digest(encoded(profile))
    proposal = await runtime.prepare_successor(encoded(successor), a['plan_sha256'])
    assert proposal['schema_version'] == 2 and proposal['predecessor'] == a
    assert proposal['successor']['plan']['state_migration']['from_format'] == '1'
    selected = await runtime.selection(proposal['transition_sha256'])
    assert selected['selected']['plan_sha256'] == a['plan_sha256']
    assert selected['proposal']['schema_version'] == 2 and selected['pending_transition'] is None
    fresh = ComposeRuntime(tmp_path / 'fresh-format', *planner, runner=runner)
    ordinary = await fresh.prepare(encoded(successor))
    assert ordinary['schema_version'] == 1
    assert ordinary['plan']['components']['database']['entrypoint'] == ['usr/local/bin/database']
    assert not (fresh.workspace / 'custody.json').exists()
    stale = copy.deepcopy(successor)
    stale['profile_sha256'] = request['profile_sha256']
    with pytest.raises(ComposeError, match='successor_planning_failed'):
        await runtime.prepare_successor(encoded(stale), a['plan_sha256'])
    assert all(len(argv) <= 2 for argv in runner.calls)


@pytest.mark.asyncio
async def test_malformed_readback_is_removed_before_fresh_explicit_reconciliation(tmp_path):
    runtime, driver, plans, proposal = await setup(tmp_path)
    driver.malformed_readback = True
    before = await runtime.observe()
    result = await runtime.adopt_successor(proposal['transition_sha256'], plans[0]['plan_sha256'], before['observation_sha256'])
    assert result['status'] == 'unconfirmed' and driver.apply_entries == 0
    custody = json.loads((runtime.workspace / 'custody.json').read_bytes())
    first = custody['transition']['migration']['readbacks'][0]
    assert first['removed'] and first['id'] not in driver.containers
    runtime = ComposeRuntime(runtime.workspace, runtime.planner, runtime.planner_sha256, runner=driver)
    pending = await runtime.observe()
    result = await runtime.reconcile_successor(proposal['transition_sha256'], pending['observation_sha256'])
    assert result['status'] == 'observed' and driver.apply_entries == 1
    assert driver.readback_entries == 3 and not any(
        item['Config']['Labels'].get('org.meerkat.migration') for item in driver.containers.values())


@pytest.mark.asyncio
@pytest.mark.parametrize('payload', [None, b'{"protocol_version":1,"attempt_id":"bound"}\n'])
async def test_docker_process_boundary_supplies_bytes_to_real_child(tmp_path, payload):
    # A harmless stdin echo process replaces Docker; no daemon or socket is used.
    tool = tmp_path / 'echo-input'
    tool.write_text('#!' + sys.executable + '\nimport sys\nsys.stdout.buffer.write(sys.stdin.buffer.read() or b"empty")\n')
    tool.chmod(0o700)
    workspace = tmp_path / 'workspace'
    workspace.mkdir()
    pin = {'path': str(tool), 'sha256': digest(tool.read_bytes())}
    runtime = ComposeRuntime(workspace, tool, pin['sha256'])
    plan = {'engine': {'docker': pin, 'compose': pin, 'endpoint': 'unix:///never-contacted'}}
    kwargs = {} if payload is None else {'stdin': payload}
    result = await runtime._docker(plan, ['inert-fixture'], **kwargs)
    assert result.returncode == 0 and result.stdout == (payload or b'empty') and result.stderr == b''


@pytest.mark.asyncio
async def test_matching_worker_without_retained_create_intent_is_not_owned(tmp_path):
    runtime, driver, plans, proposal = await setup(tmp_path)
    driver.hold_apply = True
    before = await runtime.observe()
    result = await runtime.adopt_successor(proposal['transition_sha256'], plans[0]['plan_sha256'], before['observation_sha256'])
    assert result['status'] == 'unconfirmed'
    # Reuse the exact validated worker shape to model a same-name object already
    # present when a fresh worker reference was saved, before creation intent.
    custody_path = runtime.workspace / 'custody.json'
    custody = json.loads(custody_path.read_bytes())
    ref = custody['transition']['migration']['apply']
    identity = ref['id']
    driver.containers[identity]['State'].update(Running=False, Status='created')
    ref.update(id=None, create_attempted=False, start_attempted=False, quiescent=False)
    custody_path.write_bytes(encoded(custody))
    retained = custody_path.read_bytes()
    containers = copy.deepcopy(driver.containers)
    runtime = ComposeRuntime(runtime.workspace, runtime.planner, runtime.planner_sha256, runner=driver)
    at = len(driver.calls)
    with pytest.raises(ComposeError, match='migration_worker_unowned'):
        await runtime.observe()
    assert effects(driver, at) == [] and driver.containers == containers and driver.apply_entries == 0
    assert custody_path.read_bytes() == retained and custody['active'] == plans[0]['plan_sha256']


class ReverseMigrationDriver(MigrationDriver):
    """Existing inert engine, with the trial's real SQLite owner and emit ledger."""
    def __init__(self, plans, workspace, owner):
        super().__init__(plans, workspace, owner.DATABASE)
        owner.DATABASE.unlink()  # Remove only the base fixture's unused seed.
        self.domain = owner
        self.old_receipt = None

    def container(self, service, **kwargs):
        identity = super().container(service, **kwargs)
        if service == 'db':
            self.domain.application(int(self.reply['plan']['volumes']['data']['format']))
        return identity

    def owner_call(self, role, binding):
        if role == 'apply':
            self.apply_entries += 1
            return self.domain.apply(binding)
        self.readback_entries += 1
        if self.old_receipt is not None:
            return {'protocol_version': 1, 'status': 'committed',
                    'observed_format': binding['to_format'], 'receipt': self.old_receipt}
        return self.domain.readback(binding)


async def reverse_setup(tmp_path):
    plans, tools = migration_plans(tmp_path)
    a, b = plans[:2]
    b['plan']['state_migration']['operation'] = 'sqlite-v1-to-v2'
    b = version(b, b'{"v":2}')
    c = copy.deepcopy(b)
    c['plan']['volumes']['data']['format'] = '1'
    c['plan']['components']['db']['image'] = a['plan']['components']['db']['image']
    document = json.loads(c['plan']['compose_json'])
    document['services']['db']['image'] = a['plan']['components']['db']['image']
    c['plan']['compose_json'] = encoded(document).decode()
    c['plan']['files']['compose.json'].update(contents=encoded(document).decode(), sha256=digest(encoded(document)))
    c['plan']['state_migration'].update(from_format='2', to_format='1', operation='sqlite-v2-to-v1')
    c = version(c, b'{"v":3}')
    owner = reverse_owner(tmp_path)
    driver = ReverseMigrationDriver([a, b, c], tmp_path / 'workspace', owner)
    runtime = ComposeRuntime(driver.workspace, tools['planner']['path'], tools['planner']['sha256'], runner=driver)
    await runtime.prepare(b'{"v":1}')
    await runtime.start(a['plan_sha256'])
    forward = await runtime.prepare_successor(b'{"v":2}', a['plan_sha256'])
    before = await runtime.observe()
    assert (await runtime.adopt_successor(forward['transition_sha256'], a['plan_sha256'],
                                         before['observation_sha256']))['status'] == 'observed'
    assert owner.snapshot()['cursor'] == 2 and len(owner.EFFECTS.read_bytes().splitlines()) == 2
    return runtime, driver, (a, b, c), forward


@pytest.mark.asyncio
@pytest.mark.parametrize('lost', [False, True, 'cancelled'])
async def test_reverse_migration_preserves_new_effects_and_resumes_old_application(tmp_path, lost):
    runtime, driver, (a, b, c), forward = await reverse_setup(tmp_path)
    original_volume = copy.deepcopy(driver.volume)
    prior_receipt = driver.domain.snapshot()['receipts'][0]
    # Even this inert planner returning old A cannot bypass its missing reverse
    # owner declaration. The actual planner also rejects repeated raw history.
    old = await runtime.prepare_successor(b'{"v":1}', b['plan_sha256'])
    before = await runtime.observe()
    at = len(driver.calls)
    with pytest.raises(ComposeError, match='migration_contract_changed'):
        await runtime.adopt_successor(old['transition_sha256'], b['plan_sha256'], before['observation_sha256'])
    assert effects(driver, at) == []
    proposal = await runtime.prepare_successor(b'{"v":3}', b['plan_sha256'])
    at = len(driver.calls)
    with pytest.raises(ComposeError, match='plan_changed'):
        await runtime.adopt_successor(proposal['transition_sha256'], a['plan_sha256'], before['observation_sha256'])
    assert effects(driver, at) == []
    driver.lose_apply = lost
    if lost == 'cancelled':
        with pytest.raises(asyncio.CancelledError):
            await runtime.adopt_successor(proposal['transition_sha256'], b['plan_sha256'], before['observation_sha256'])
    else:
        result = await runtime.adopt_successor(proposal['transition_sha256'], b['plan_sha256'], before['observation_sha256'])
        assert result['status'] == ('unconfirmed' if lost else 'observed')
    if lost:
        assert driver.domain.snapshot()['cursor'] == 2
        assert len(driver.domain.EFFECTS.read_bytes().splitlines()) == 2
        runtime = ComposeRuntime(runtime.workspace, runtime.planner, runtime.planner_sha256, runner=driver)
        pending = await runtime.observe()
        assert pending['observation']['plan_sha256'] == b['plan_sha256']
        assert pending['phase'] == 'migrate'
        result = await runtime.reconcile_successor(proposal['transition_sha256'], pending['observation_sha256'])
        assert result['status'] == 'observed'
    state = driver.domain.snapshot()
    attempts = [json.loads(line) for line in driver.domain.EFFECTS.read_bytes().splitlines()]
    assert state['format'] == 1 and state['cursor'] == 3 and state['accepted'] == ['event-1', 'event-2', 'event-3']
    assert attempts == [{'event_id': 'event-1', 'application_format': 1},
                        {'event_id': 'event-2', 'application_format': 2},
                        {'event_id': 'event-3', 'application_format': 1}]
    assert state['receipts'][0] == prior_receipt and len(state['receipts']) == 2
    assert state['receipts'][1]['binding']['transition_sha256'] == proposal['transition_sha256']
    assert driver.apply_entries == 2 and driver.volume == original_volume and driver.volume_creations == 1
    custody = json.loads((runtime.workspace / 'custody.json').read_bytes())
    assert [entry['id'] for entry in custody['history']] == [forward['transition_sha256'], proposal['transition_sha256']]
    assert [entry['migration']['receipt']['receipt'] for entry in custody['history']] == state['receipts']
    at = len(driver.calls)
    with pytest.raises(ComposeError, match='no_pending_successor'):
        await runtime.reconcile_successor(proposal['transition_sha256'], '0' * 64)
    assert effects(driver, at) == [] and driver.domain.snapshot() == state


@pytest.mark.asyncio
@pytest.mark.parametrize('mode', ['old_receipt', 'missing_apply'])
async def test_reverse_migration_refuses_old_receipt_or_missing_attempted_worker(tmp_path, mode):
    runtime, driver, (_, b, _), _ = await reverse_setup(tmp_path)
    proposal = await runtime.prepare_successor(b'{"v":3}', b['plan_sha256'])
    before = await runtime.observe()
    driver.lose_apply = True
    assert (await runtime.adopt_successor(proposal['transition_sha256'], b['plan_sha256'],
                                         before['observation_sha256']))['status'] == 'unconfirmed'
    if mode == 'old_receipt':
        driver.old_receipt = driver.domain.snapshot()['receipts'][0]
    else:
        driver.containers = {key: item for key, item in driver.containers.items()
                             if item['Config']['Labels'].get('org.meerkat.migration-role') != 'apply'}
    runtime = ComposeRuntime(runtime.workspace, runtime.planner, runtime.planner_sha256, runner=driver)
    pending = await runtime.observe()
    result = await runtime.reconcile_successor(proposal['transition_sha256'], pending['observation_sha256'])
    assert result['status'] == 'unconfirmed' and result['plan_sha256'] == b['plan_sha256']
    assert driver.apply_entries == 2 and driver.domain.snapshot()['cursor'] == 2
    assert len(driver.domain.EFFECTS.read_bytes().splitlines()) == 2
    if mode == 'old_receipt':
        driver.old_receipt = None
        pending = await runtime.observe()
        assert (await runtime.reconcile_successor(proposal['transition_sha256'], pending['observation_sha256']))['status'] == 'observed'
        assert driver.apply_entries == 2 and driver.domain.snapshot()['cursor'] == 3
