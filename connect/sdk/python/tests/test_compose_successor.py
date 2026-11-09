"""Successor persistence/failure controls with a fake engine, not engine acceptance."""
import asyncio
import copy
import json

import pytest

import meerkat_toolkit.compose as module
from meerkat_toolkit.compose import CommandResult, ComposeError, ComposeRuntime
from test_compose import FakeDriver, digest, encoded, fixture_plan


def version(plan, request):
    reply = copy.deepcopy(plan)
    reply["plan"]["request_sha256"] = digest(request)
    reply["plan_sha256"] = digest(encoded(reply["plan"]))
    return reply


class SuccessorDriver(FakeDriver):
    def __init__(self, a, b, workspace):
        super().__init__(a, workspace)
        self.plans = {b'{"v":1}': a, b'{"v":2}': b}
        self.removal_loss = False
        self.hold_stop = False
        self.stop_entered = asyncio.Event()
        self.stop_release = asyncio.Event()

    async def run(self, argv, **kwargs):
        if len(argv) <= 2:
            self.calls.append((list(argv), dict(kwargs["env"])))
            raw = kwargs["stdin"]
            if len(argv) == 1:
                return CommandResult(0, encoded(self.plans[raw])) if raw in self.plans else CommandResult(1)
            assert argv[1] == "--successor"
            request = json.loads(raw)
            return CommandResult(0, encoded({"schema_version": 1, "status": "prepared_successor",
                "transition_sha256": digest(raw),
                "predecessor": self.plans[request["predecessor_request"].encode()],
                "successor": self.plans[request["successor_request"].encode()]}))
        if "up" in argv:
            self.workspace = module.Path(argv[argv.index("--file") + 1]).parent
            self.reply = self.plans[b'{"v":2}'] if self.workspace.name == "next" else self.plans[b'{"v":1}']
        args = argv[5:]
        if args[:2] == ["container", "stop"] and self.hold_stop:
            self.stop_entered.set()
            await self.stop_release.wait()
        result = await super().run(argv, **kwargs)
        if args[:2] == ["container", "rm"] and self.removal_loss:
            self.removal_loss = False
            raise TimeoutError("withheld actual fake removal result")
        return result


async def setup(tmp_path, *, summary=False):
    reply, tools = fixture_plan(tmp_path)
    if summary:
        reply["plan"].update({key: digest(key.encode()) for key in (
            "package_sha256", "manifest_sha256", "manifest_source_sha256", "profile_sha256",
            "configuration_sha256", "policy_sha256")})
        reply["plan"]["host_owner"] = "fixture-owner"
    a, b = version(reply, b'{"v":1}'), version(reply, b'{"v":2}')
    workspace = tmp_path / "workspace"
    driver = SuccessorDriver(a, b, workspace)
    runtime = ComposeRuntime(workspace, tools["planner"]["path"], tools["planner"]["sha256"], runner=driver)
    await runtime.prepare(b'{"v":1}')
    assert (await runtime.start(a["plan_sha256"]))["status"] == "observed"
    proposal = await runtime.prepare_successor(b'{"v":2}', a["plan_sha256"])
    return runtime, driver, a, b, proposal


def effects(driver, start=0):
    return [argv for argv, _ in driver.calls[start:] if "up" in argv or
            argv[5:7] in (["container", "stop"], ["container", "start"],
                           ["container", "rm"], ["network", "rm"], ["volume", "rm"])]


class RetainedRemovalDriver(SuccessorDriver):
    """Existing inert engine with one state volume; no actual Docker execution."""
    def __init__(self, a, b, c, workspace):
        super().__init__(a, b, workspace)
        self.plans[b'{"v":3}'] = c
        self.volume = None
        self.volume_creations = 0
        self.state_bytes = b""

    def container(self, service, **kwargs):
        identity = super().container(service, **kwargs)
        if service == "db":
            labels = json.loads(self.reply["plan"]["compose_json"])["volumes"]["fixture-data"]["labels"]
            assert self.volume is None, "Retired state must not be recreated"
            self.volume = {"Name": "fixture-data", "CreatedAt": "2026-10-05T00:00:00Z",
                "Mountpoint": "/engine/fixture-data", "Driver": "local", "Options": {},
                "Labels": dict(labels, **{"com.docker.compose.project": "fixture"})}
            self.volume_creations += 1
            self.state_bytes = b'{"cursor":7,"effects":["retained-A-effect"]}'
            self.containers[identity]["Mounts"].append({"Type": "volume", "Name": "fixture-data",
                "Source": "/engine/fixture-data", "Destination": "/state", "RW": True})
        return identity

    async def run(self, argv, **kwargs):
        args = argv[5:]
        if "up" in argv:
            self.workspace = module.Path(argv[argv.index("--file") + 1]).parent
            if self.workspace.name == "next":
                self.reply = json.loads((self.workspace.parent / "prepared.json").read_bytes())["successor"]
            else:
                self.reply = self.plans[b'{"v":1}']
            return await FakeDriver.run(self, argv, **kwargs)
        if args[:1] == ["volume"] or (args[:1] == ["ps"] and "volume=fixture-data" in args):
            self.calls.append((list(argv), dict(kwargs["env"])))
            if args[:2] == ["volume", "ls"]:
                return CommandResult(0, b"fixture-data\n" if self.volume else b"")
            if args[:2] == ["volume", "inspect"]:
                return CommandResult(0, encoded([self.volume]))
            if args[:1] == ["ps"]:
                users = [identity for identity, item in self.containers.items()
                         if any(m.get("Name") == "fixture-data" for m in item["Mounts"])]
                return CommandResult(0, ("\n".join(users) + ("\n" if users else "")).encode())
            raise AssertionError("No volume effect was authorized")
        return await super().run(argv, **kwargs)


def retained_removal_plans(tmp_path):
    reply, tools = fixture_plan(tmp_path)
    plan = reply["plan"]
    plan["volumes"] = {"data": {"name": "fixture-data", "component": "db", "owner": "host", "format": "1"}}
    document = json.loads(plan["compose_json"])
    creation_labels = {"org.meerkat.original-selection": "a" * 64}
    document["volumes"] = {"fixture-data": {"name": "fixture-data", "labels": creation_labels}}
    document["services"]["db"]["volumes"].append({"type": "volume", "source": "fixture-data",
                                                 "target": "/state", "read_only": False})
    def finish(value, doc, request):
        value["plan"]["compose_json"] = encoded(doc).decode()
        value["plan"]["files"]["compose.json"].update(contents=encoded(doc).decode(), sha256=digest(encoded(doc)))
        return version(value, request)
    a = finish(reply, document, b'{"v":1}')
    b = copy.deepcopy(a)
    b["plan"]["components"] = {"api": b["plan"]["components"]["api"]}
    b["plan"]["components"]["api"].update(dependencies=[], deployment_dependencies=[])
    for field in ("initialization_order", "shutdown_order", "deployment_order"):
        b["plan"][field] = ["api"]
    b["plan"]["request_components"] = []
    b["plan"]["retained_volume_labels"] = {"data": creation_labels}
    document = copy.deepcopy(document)
    document["services"] = {"api": document["services"]["api"]}
    document["volumes"] = {"fixture-data": {"name": "fixture-data", "external": True}}
    b = finish(b, document, b'{"v":2}')
    c = version(b, b'{"v":3}')
    return a, b, c, tools


@pytest.mark.asyncio
@pytest.mark.parametrize("interrupt_retirement", [False, True])
async def test_removed_state_survives_successors_reopen_and_default_remove(tmp_path, interrupt_retirement):
    a, b, c, tools = retained_removal_plans(tmp_path)
    driver = RetainedRemovalDriver(a, b, c, tmp_path / "workspace")
    runtime = ComposeRuntime(driver.workspace, tools["planner"]["path"], tools["planner"]["sha256"], runner=driver)
    await runtime.prepare(b'{"v":1}')
    started = await runtime.start(a["plan_sha256"])
    origin_volumes = started["observation"]["volumes"]
    origin_state = driver.state_bytes
    origin_creation = copy.deepcopy(driver.volume)
    origin_record = json.loads((runtime.workspace / "record.json").read_bytes())
    proposal = await runtime.prepare_successor(b'{"v":2}', a["plan_sha256"])
    before = await runtime.observe()
    driver.removal_loss = interrupt_retirement
    result = await runtime.adopt_successor(proposal["transition_sha256"], a["plan_sha256"], before["observation_sha256"])
    if interrupt_retirement:
        assert result["status"] == "unconfirmed"
        reopened = ComposeRuntime(runtime.workspace, runtime.planner, runtime.planner_sha256, runner=driver)
        pending = await reopened.observe()
        assert pending["phase"] == "retire" and pending["observation"]["volumes"] == origin_volumes
        at = len(driver.calls)
        with pytest.raises(ComposeError, match="successor_unconfirmed"):
            await reopened.start(a["plan_sha256"])
        with pytest.raises(ComposeError, match="stale_observation"):
            await reopened.reconcile_successor(proposal["transition_sha256"], before["observation_sha256"])
        assert effects(driver, at) == []
        result = await reopened.reconcile_successor(proposal["transition_sha256"], pending["observation_sha256"])
    assert result["status"] == "observed" and result["plan_sha256"] == b["plan_sha256"]
    runtime = ComposeRuntime(runtime.workspace, runtime.planner, runtime.planner_sha256, runner=driver)
    observed = await runtime.observe()
    assert observed["available"] == {"api": True} and observed["volumes"] == origin_volumes
    assert all(c["component"] == "api" for c in observed["containers"].values())
    proposal = await runtime.prepare_successor(b'{"v":3}', b["plan_sha256"])
    observed = await runtime.observe()
    result = await runtime.adopt_successor(proposal["transition_sha256"], b["plan_sha256"], observed["observation_sha256"])
    assert result["status"] == "observed" and result["plan_sha256"] == c["plan_sha256"]
    custody = json.loads((runtime.workspace / "custody.json").read_bytes())
    assert custody["records"][a["plan_sha256"]] == origin_record
    assert len(custody["history"]) == 2 and custody["transition"] is None
    assert all(record["volumes"] == origin_volumes for record in custody["records"].values())
    stopped = await runtime.stop(c["plan_sha256"], result["observation"]["observation_sha256"])
    removed = await runtime.remove(c["plan_sha256"], stopped["observation"]["observation_sha256"], delete_state=set())
    assert removed["observation"]["containers"] == {} and removed["observation"]["volumes"] == origin_volumes
    assert driver.volume == origin_creation and driver.volume_creations == 1 and driver.state_bytes == origin_state
    assert not any(argv[5:6] == ["volume"] for argv in effects(driver))


@pytest.mark.asyncio
async def test_selection_projects_only_verified_owner_state_without_engine_entry(tmp_path):
    runtime, driver, a, b, proposal = await setup(tmp_path, summary=True)
    at = len(driver.calls)
    selected = await runtime.selection(proposal["transition_sha256"])
    assert set(selected) == {"schema_version", "status", "selected", "selected_from_transition_sha256",
                             "pending_transition", "pending_operation", "proposal", "state_namespaces"}
    assert selected["state_namespaces"] == sorted(a["plan"]["volumes"])
    assert selected["status"] == "selected_plan" and selected["pending_operation"] is False
    assert selected["selected"]["status"] == "prepared_summary"
    assert selected["selected"]["plan_sha256"] == a["plan_sha256"]
    assert selected["proposal"]["status"] == "prepared_successor_summary"
    assert selected["proposal"]["successor"]["plan_sha256"] == b["plan_sha256"]
    assert selected["pending_transition"] is None and selected["selected_from_transition_sha256"] is None
    for prepared in (selected["selected"], selected["proposal"]["predecessor"], selected["proposal"]["successor"]):
        assert "plan" not in prepared
        assert all(set(item) == {"sha256", "mode"} for item in prepared["summary"]["files"].values())
        assert "compose_json" not in prepared["summary"] and "engine" not in prepared["summary"]
        assert all("environment" not in item for item in prepared["summary"]["components"].values())
    assert all(len(argv) <= 2 for argv, _ in driver.calls[at:])
    observation = await runtime.observe()
    driver.removal_loss = True
    assert (await runtime.adopt_successor(proposal["transition_sha256"], a["plan_sha256"],
        observation["observation_sha256"]))["status"] == "unconfirmed"
    at = len(driver.calls)
    pending = await runtime.selection(proposal["transition_sha256"])
    assert pending["pending_transition"] == {"transition_sha256": proposal["transition_sha256"],
        "phase": "retire", "predecessor_plan_sha256": a["plan_sha256"], "successor_plan_sha256": b["plan_sha256"]}
    assert all(len(argv) <= 2 for argv, _ in driver.calls[at:])
    observation = await runtime.observe()
    assert (await runtime.reconcile_successor(proposal["transition_sha256"], observation["observation_sha256"]))["status"] == "observed"
    at = len(driver.calls)
    current = await runtime.selection()
    assert current["selected"]["plan_sha256"] == b["plan_sha256"] and current["pending_transition"] is None
    assert current["selected_from_transition_sha256"] == proposal["transition_sha256"]
    assert current["proposal"] is None
    assert current["state_namespaces"] == sorted(b["plan"]["volumes"])
    assert all(len(argv) <= 2 for argv, _ in driver.calls[at:])
    (runtime.workspace / ".successors" / proposal["transition_sha256"] / "prepared.json").write_text("{}")
    with pytest.raises(ComposeError, match="plan_changed"):
        await runtime.selection()


@pytest.mark.asyncio
async def test_successor_keeps_old_records_fences_stale_commands_and_reopens(tmp_path):
    runtime, driver, a, b, proposal = await setup(tmp_path)
    before = await runtime.observe()
    source_record = json.loads((runtime.workspace / "record.json").read_bytes())
    result = await runtime.adopt_successor(proposal["transition_sha256"], a["plan_sha256"], before["observation_sha256"])
    assert result["status"] == "observed" and result["plan_sha256"] == b["plan_sha256"]
    assert (runtime.workspace / "request.json").read_bytes() == module.SUCCESSOR_MARKER
    custody = json.loads((runtime.workspace / "custody.json").read_bytes())
    assert custody["records"][a["plan_sha256"]] == source_record
    assert custody["history"][0]["result"] == "observed" and custody["transition"] is None
    at = len(driver.calls)
    for method, args in [(runtime.start, (a["plan_sha256"],)),
                         (runtime.resume, (a["plan_sha256"], before["observation_sha256"])),
                         (runtime.stop, (a["plan_sha256"], before["observation_sha256"]))]:
        with pytest.raises(ComposeError, match="plan_changed"):
            await method(*args)
    assert effects(driver, at) == []
    reopened = ComposeRuntime(runtime.workspace, runtime.planner, runtime.planner_sha256, runner=driver)
    assert all((await reopened.observe())["available"].values())


@pytest.mark.asyncio
async def test_mid_removal_uncertainty_requires_fresh_explicit_reconciliation(tmp_path):
    runtime, driver, a, b, proposal = await setup(tmp_path)
    before = await runtime.observe()
    driver.removal_loss = True
    first = await runtime.adopt_successor(proposal["transition_sha256"], a["plan_sha256"], before["observation_sha256"])
    assert first["status"] == "unconfirmed"
    pending = await runtime.observe()
    assert pending["phase"] == "retire"
    at = len(driver.calls)
    with pytest.raises(ComposeError, match="successor_unconfirmed"):
        await runtime.start(a["plan_sha256"])
    with pytest.raises(ComposeError, match="stale_observation"):
        await runtime.reconcile_successor(proposal["transition_sha256"], before["observation_sha256"])
    assert effects(driver, at) == []
    result = await runtime.reconcile_successor(proposal["transition_sha256"], pending["observation_sha256"])
    assert result["status"] == "observed" and result["plan_sha256"] == b["plan_sha256"]


@pytest.mark.asyncio
async def test_lock_and_cancellation_do_not_transfer_or_restart_automatically(tmp_path):
    runtime, driver, a, _, proposal = await setup(tmp_path)
    before = await runtime.observe()
    driver.hold_stop = True
    task = asyncio.create_task(runtime.adopt_successor(proposal["transition_sha256"], a["plan_sha256"], before["observation_sha256"]))
    await asyncio.wait_for(driver.stop_entered.wait(), 2)
    other = ComposeRuntime(runtime.workspace, runtime.planner, runtime.planner_sha256, runner=driver)
    with pytest.raises(ComposeError, match="operation_in_progress"):
        await other.observe()
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    custody = json.loads((runtime.workspace / "custody.json").read_bytes())
    assert custody["transition"]["result"] == "cancelled_unconfirmed"
    at = len(driver.calls)
    assert (await other.observe())["phase"] == "retire"
    with pytest.raises(ComposeError, match="successor_unconfirmed"):
        await other.resume(a["plan_sha256"], before["observation_sha256"])
    assert effects(driver, at) == []


@pytest.mark.asyncio
async def test_journal_without_marker_is_not_transfer_and_requires_fresh_admission(tmp_path, monkeypatch):
    runtime, driver, a, _, proposal = await setup(tmp_path)
    before = await runtime.observe()
    original = module._write
    def interrupted(path, data, **kwargs):
        if path == runtime.workspace / "request.json" and data == module.SUCCESSOR_MARKER:
            raise OSError("fixture interruption before marker replacement")
        return original(path, data, **kwargs)
    monkeypatch.setattr(module, "_write", interrupted)
    at = len(driver.calls)
    with pytest.raises(OSError):
        await runtime.adopt_successor(proposal["transition_sha256"], a["plan_sha256"], before["observation_sha256"])
    assert effects(driver, at) == []
    assert (runtime.workspace / "request.json").read_bytes() == b'{"v":1}'
    monkeypatch.setattr(module, "_write", original)
    await runtime.stop(a["plan_sha256"], before["observation_sha256"])
    with pytest.raises(ComposeError, match="stale_observation"):
        await runtime.adopt_successor(proposal["transition_sha256"], a["plan_sha256"], before["observation_sha256"])
    observed = await runtime.observe()
    result = await runtime.adopt_successor(proposal["transition_sha256"], a["plan_sha256"], observed["observation_sha256"])
    assert result["status"] == "observed"
    assert len(list((runtime.workspace / ".successors").glob("unadmitted-*.json"))) == 1


@pytest.mark.asyncio
async def test_attempted_unseen_creation_refuses_recreation_on_fresh_observation(tmp_path):
    runtime, driver, a, _, proposal = await setup(tmp_path)
    before = await runtime.observe()
    original = driver.run
    lost = False
    async def run(argv, **kwargs):
        nonlocal lost
        if "up" in argv and "next/compose.json" in argv[argv.index("--file") + 1] and not lost:
            lost = True
            raise TimeoutError("no owner-visible result or container identity")
        return await original(argv, **kwargs)
    driver.run = run
    result = await runtime.adopt_successor(proposal["transition_sha256"], a["plan_sha256"], before["observation_sha256"])
    assert result["status"] == "unconfirmed"
    custody = json.loads((runtime.workspace / "custody.json").read_bytes())
    assert list(custody["transition"]["attempted_components"]) == ["db"]
    observed = await runtime.observe()
    at = len(driver.calls)
    result = await runtime.reconcile_successor(proposal["transition_sha256"], observed["observation_sha256"])
    assert result["status"] == "unconfirmed" and effects(driver, at) == []


@pytest.mark.asyncio
async def test_old_ack_and_independently_prepared_root_confer_no_adoption(tmp_path):
    runtime, driver, a, _, proposal = await setup(tmp_path)
    before = await runtime.observe()
    at = len(driver.calls)
    with pytest.raises(ComposeError):
        await runtime.adopt_successor(a["plan_sha256"], a["plan_sha256"], before["observation_sha256"])
    assert effects(driver, at) == []
    second = ComposeRuntime(tmp_path / "other", runtime.planner, runtime.planner_sha256, runner=driver)
    await second.prepare(b'{"v":1}')
    # No first-start custody exists. Matching resource names are not adoption evidence.
    await second.prepare_successor(b'{"v":2}', a["plan_sha256"])
    at = len(driver.calls)
    with pytest.raises(ComposeError):
        await second.adopt_successor(proposal["transition_sha256"], a["plan_sha256"], before["observation_sha256"])
    assert effects(driver, at) == []


@pytest.mark.asyncio
async def test_unavailable_successor_dependency_never_enters_dependent(tmp_path):
    runtime, driver, a, _, proposal = await setup(tmp_path)
    before = await runtime.observe()
    original = driver.run

    async def run(argv, **kwargs):
        if "up" in argv and "next/compose.json" in argv[argv.index("--file") + 1]:
            driver.calls.append((list(argv), dict(kwargs["env"])))
            assert argv[-1] == "db", "Unavailable prerequisite admitted dependent"
            return CommandResult(1)
        return await original(argv, **kwargs)

    driver.run = run
    result = await runtime.adopt_successor(proposal["transition_sha256"], a["plan_sha256"], before["observation_sha256"])
    assert result["status"] == "unconfirmed"
    custody = json.loads((runtime.workspace / "custody.json").read_bytes())
    assert list(custody["transition"]["attempted_components"]) == ["db"]
