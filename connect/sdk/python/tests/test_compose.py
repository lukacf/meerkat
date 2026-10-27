"""Execution-boundary tests. The fake driver is not Docker runtime acceptance."""
from __future__ import annotations

import asyncio
import copy
import hashlib
import json
from pathlib import Path
import sys

import pytest

from meerkat_toolkit.compose import (CommandResult, ComposeError, ComposeRuntime,
                                     ProcessRunner)


def encoded(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def digest(value):
    return hashlib.sha256(value).hexdigest()


def fixture_plan(tmp_path):
    tools = {}
    for name in ("planner", "docker", "compose"):
        path = tmp_path / name
        path.write_bytes(name.encode())
        tools[name] = {"path": str(path), "sha256": digest(name.encode())}
    labels = {"org.meerkat.manifest": "a" * 64, "org.meerkat.profile": "b" * 64}
    components, services = {}, {}
    for name, lifetime, dependencies in (("db", "deployment", []),
                                          ("api", "deployment", ["db"]),
                                          ("tool", "request", ["db"])):
        components[name] = {"service": name, "lifetime": lifetime,
            "image": "example/" + name + "@sha256:" + "c" * 64,
            "image_id": "sha256:" + "d" * 64, "platform": "linux/arm64", "user": "1000:1000",
            "environment": {"MODE": "fixed"}, "readiness": "running", "stop_timeout_ms": 1000,
            "dependencies": dependencies, "deployment_dependencies": dependencies}
        services[name] = {"image": components[name]["image"], "entrypoint": ["bin/server"],
            "command": ["literal"], "working_dir": "/", "environment": {"MODE": "fixed"},
            "user": "1000:1000", "mem_limit": 67108864, "memswap_limit": 67108864,
            "pids_limit": 16, "cpus": "0.100", "labels": labels,
            "networks": {"default": {"aliases": [name]}},
            "healthcheck": {"disable": True}, "volumes": [{"type": "bind", "source": "./inputs/configuration",
                  "target": "/configuration", "read_only": True}]}
    compose = {"services": services, "networks": {"default": {"name": "fixture-network"}}, "volumes": {}}
    compose_text = encoded(compose).decode()
    plan = {"project": "fixture", "engine": {"id": "engine-1", "endpoint": "unix:///fixture.sock",
            "docker": tools["docker"], "compose": tools["compose"]}, "components": components,
            "compose_json": compose_text, "files": {}, "deployment_order": ["db", "api"],
            "initialization_order": ["db", "api", "tool"], "shutdown_order": ["tool", "api", "db"],
            "request_components": ["tool"], "volumes": {}}
    for path, contents, mode in (("compose.json", compose_text, 0o600),
                                  ("inputs/configuration", "{}", 0o444), ("inputs/policy", "{}", 0o444)):
        plan["files"][path] = {"contents": contents, "sha256": digest(contents.encode()), "mode": mode}
    return {"schema_version": 1, "status": "prepared", "plan_sha256": digest(encoded(plan)), "plan": plan}, tools


class FakeDriver(ProcessRunner):
    def __init__(self, reply, workspace):
        self.reply = reply
        self.workspace = workspace
        self.calls = []
        self.containers = {}
        self.network = None
        self.next_id = 1
        self.fail_up = False
        self.started = asyncio.Event()
        self.wait_stream = False

    def container(self, service, *, name=None):
        plan = self.reply["plan"]
        component = plan["components"][service]
        expected = json.loads(plan["compose_json"])["services"][service]
        identity = str(self.next_id).zfill(64)
        self.next_id += 1
        labels = dict(expected["labels"], **{"com.docker.compose.project": "fixture",
                      "com.docker.compose.service": service, "com.docker.compose.oneoff": "False"})
        item = {"Id": identity, "Name": "/" + (name or "fixture-" + service + "-1"),
                "Image": component["image_id"],
                "Config": {"Labels": labels, "Image": component["image"],
                "Entrypoint": expected["entrypoint"], "Cmd": expected["command"],
                "WorkingDir": "/", "User": component["user"], "Env": ["MODE=fixed"],
                "Healthcheck": {"Test": ["NONE"]}},
                "HostConfig": {"Memory": 67108864, "MemorySwap": 67108864,
                "PidsLimit": 16, "NanoCpus": 100000000, "Init": True, "RestartPolicy": {"Name": "no"}},
                "Mounts": [{"Type": "bind", "Source": str(self.workspace / "inputs/configuration"),
                            "Destination": "/configuration", "RW": False}],
                "NetworkSettings": {"Networks": {"fixture-network": {"NetworkID": "e" * 64,
                    "Aliases": [name] if name else ["fixture-" + service + "-1", service, service]}}},
                "State": {"Running": True, "Status": "running", "ExitCode": 0}}
        self.network = {"Id": "e" * 64, "Name": "fixture-network", "Internal": True,
                        "Driver": "bridge", "Labels": {"com.docker.compose.project": "fixture"}, "Containers": {}}
        self.containers[identity] = item
        return identity

    async def run(self, argv, *, cwd, env, **kwargs):
        self.calls.append((list(argv), dict(env)))
        if len(argv) == 1:
            return CommandResult(0, encoded(self.reply))
        args = argv[5:]
        if args[0] == "compose":
            args = args[7:]
            if args[0] == "up":
                for service in args:
                    if service in self.reply["plan"]["components"]:
                        self.container(service)
                return CommandResult(1 if self.fail_up else 0)
            if args[0] == "start":
                for item in self.containers.values():
                    if item["Config"]["Labels"]["com.docker.compose.service"] in args[1:]:
                        item["State"].update(Running=True, Status="running")
                return CommandResult(0)
        if args[:1] == ["info"]:
            return CommandResult(0, encoded("engine-1"))
        if args[:2] == ["image", "inspect"]:
            return CommandResult(0, encoded([{"Id": "sha256:" + "d" * 64, "Os": "linux", "Architecture": "arm64"}]))
        if args[0] == "ps":
            return CommandResult(0, ("\n".join(self.containers) + ("\n" if self.containers else "")).encode())
        if args[:2] == ["container", "inspect"]:
            found = []
            for target in args[2:]:
                matches = [item for identity, item in self.containers.items()
                           if identity == target or item["Name"] == "/" + target]
                if len(matches) != 1:
                    return CommandResult(1)
                found.extend(matches)
            return CommandResult(0, encoded(found))
        if args[:2] == ["container", "start"]:
            self.containers[args[-1]]["State"].update(Running=True, Status="running")
            return CommandResult(0)
        if args[:2] == ["container", "stop"]:
            self.containers[args[-1]]["State"].update(Running=False, Status="exited")
            return CommandResult(0)
        if args[:2] == ["container", "rm"]:
            del self.containers[args[-1]]
            return CommandResult(0)
        if args[:2] == ["network", "ls"]:
            return CommandResult(0, ((self.network["Id"] + "\n") if self.network else "").encode())
        if args[:2] == ["network", "inspect"]:
            return CommandResult(0, encoded([self.network]))
        if args[:2] == ["network", "rm"]:
            self.network = None
            return CommandResult(0)
        if args[:2] == ["volume", "ls"]:
            return CommandResult(0)
        raise AssertionError(args)

    async def stream(self, argv, *, stdout, **kwargs):
        self.calls.append((list(argv), dict(kwargs["env"])))
        name = argv[argv.index("--name") + 1]
        identity = self.container(argv[-1], name=name)
        await stdout(b'{"opaque":"protocol"}\n')
        self.started.set()
        if self.wait_stream:
            await asyncio.Event().wait()
        self.containers[identity]["State"].update(Running=False, Status="exited")
        return 0


def fixture_runtime(tmp_path):
    reply, tools = fixture_plan(tmp_path)
    workspace = tmp_path / "workspace"
    driver = FakeDriver(reply, workspace)
    runtime = ComposeRuntime(workspace, tools["planner"]["path"], tools["planner"]["sha256"], runner=driver)
    return runtime, driver, reply


@pytest.mark.parametrize("start_period", ["omitted", 0, 5000000])
def test_health_start_period_retained_inspection_accepts_exact_duration(tmp_path, start_period):
    fixture = json.loads((Path(__file__).parent / "fixtures/compose-health-start-period.json").read_text())
    runtime, driver, _ = fixture_runtime(tmp_path)
    raw, plan = fixture["inspection"], fixture["plan"]
    assert "StartPeriod" not in raw["Config"]["Healthcheck"]
    if start_period != "omitted":
        raw["Config"]["Healthcheck"]["StartPeriod"] = start_period
    if start_period == 5000000:
        compose = json.loads(plan["compose_json"])
        compose["services"]["support"]["healthcheck"]["start_period"] = "5ms"
        plan["compose_json"] = encoded(compose).decode()
    observed = runtime._container(plan, raw, {}, fixture["image_environments"], fixture["image_descriptors"])
    assert observed["id"] == raw["Id"] and observed["component"] == "support"
    assert observed["health"] == raw["State"]["Health"]["Status"] == "unhealthy"
    assert driver.calls == []


@pytest.mark.parametrize("expected_ms, actual", [
    (5, "omitted"), (5, 0), (0, 1), (0, -1), (0, None), (0, False),
    (0, True), (0, 0.0), (0, "0"), (0, []), (0, {}), (5, 5000000.0),
])
def test_health_start_period_retained_inspection_refuses_mismatch_and_invalid_type(tmp_path, expected_ms, actual):
    fixture = json.loads((Path(__file__).parent / "fixtures/compose-health-start-period.json").read_text())
    runtime, driver, _ = fixture_runtime(tmp_path)
    raw, plan = fixture["inspection"], fixture["plan"]
    compose = json.loads(plan["compose_json"])
    compose["services"]["support"]["healthcheck"]["start_period"] = f"{expected_ms}ms"
    plan["compose_json"] = encoded(compose).decode()
    if actual != "omitted":
        raw["Config"]["Healthcheck"]["StartPeriod"] = actual
    with pytest.raises(ComposeError, match="^resource_mismatch$"):
        runtime._container(plan, raw, {}, fixture["image_environments"], fixture["image_descriptors"])
    assert driver.calls == []


@pytest.mark.parametrize("field", ["Test", "Retries", "Interval", "Timeout"])
def test_health_zero_start_period_does_not_admit_other_missing_fields(tmp_path, field):
    fixture = json.loads((Path(__file__).parent / "fixtures/compose-health-start-period.json").read_text())
    runtime, driver, _ = fixture_runtime(tmp_path)
    del fixture["inspection"]["Config"]["Healthcheck"][field]
    with pytest.raises(ComposeError, match="^resource_mismatch$"):
        runtime._container(fixture["plan"], fixture["inspection"], {},
                           fixture["image_environments"], fixture["image_descriptors"])
    assert driver.calls == []


@pytest.mark.asyncio
async def test_prepare_retains_inputs_no_engine_and_reopen_replay(tmp_path):
    runtime, driver, reply = fixture_runtime(tmp_path)
    assert await runtime.prepare(b'{"exact":"request"}') == reply
    assert len(driver.calls) == 1
    assert (runtime.workspace / "request.json").read_bytes() == b'{"exact":"request"}'
    assert (runtime.workspace / "inputs/configuration").stat().st_mode & 0o777 == 0o444
    first = await runtime.start(reply["plan_sha256"])
    assert first["status"] == "observed"
    assert first["observation"]["available"] == {"db": True, "api": True}
    effects = sum("up" in call[0] for call in driver.calls)
    reopened = ComposeRuntime(runtime.workspace, runtime.planner, runtime.planner_sha256, runner=driver)
    assert (await reopened.start(reply["plan_sha256"]))["observation"] == first["observation"]
    assert sum("up" in call[0] for call in driver.calls) == effects
    for argv, env in driver.calls[1:]:
        if len(argv) > 1:
            assert argv[1:5] == ["--host", "unix:///fixture.sock", "--config", str(runtime.workspace / ".docker")]
            assert "DOCKER_HOST" not in env and "DOCKER_CONTEXT" not in env


@pytest.mark.asyncio
async def test_stale_stop_and_changed_input_refuse_without_effect(tmp_path):
    runtime, driver, reply = fixture_runtime(tmp_path)
    await runtime.prepare(b"{}")
    observed = (await runtime.start(reply["plan_sha256"]))["observation"]
    driver.containers[next(iter(driver.containers))]["State"].update(Running=False, Status="exited")
    with pytest.raises(ComposeError, match="stale_observation"):
        await runtime.stop(reply["plan_sha256"], observed["observation_sha256"])
    assert not any("stop" in call[0] for call in driver.calls)
    config = runtime.workspace / "inputs/configuration"
    config.chmod(0o600)
    config.write_text("changed")
    with pytest.raises(ComposeError, match="inputs_changed"):
        await runtime.observe()


@pytest.mark.asyncio
async def test_stop_reverse_order_restart_and_remove_are_explicit(tmp_path):
    runtime, driver, reply = fixture_runtime(tmp_path)
    await runtime.prepare(b"{}")
    first = (await runtime.start(reply["plan_sha256"]))["observation"]
    stopped = await runtime.stop(reply["plan_sha256"], first["observation_sha256"])
    stops = [call[0][-1] for call in driver.calls if "stop" in call[0]]
    assert [first["containers"][x]["component"] for x in stops] == ["api", "db"]
    assert not any("rm" in call[0] for call in driver.calls)
    assert not any(stopped["observation"]["available"].values())
    restarted = await runtime.start(reply["plan_sha256"])
    assert set(restarted["observation"]["containers"]) == set(first["containers"])
    stopped = await runtime.stop(reply["plan_sha256"], restarted["observation"]["observation_sha256"])
    removed = await runtime.remove(reply["plan_sha256"], stopped["observation"]["observation_sha256"], delete_state=set())
    assert removed["observation"]["containers"] == {}
    assert removed["observation"]["network"] is None


@pytest.mark.asyncio
async def test_failed_start_retains_partial_and_never_repeats(tmp_path):
    runtime, driver, reply = fixture_runtime(tmp_path)
    await runtime.prepare(b"{}")
    driver.fail_up = True
    result = await runtime.start(reply["plan_sha256"])
    assert result["status"] == "unconfirmed"
    effects = sum("up" in call[0] for call in driver.calls)
    with pytest.raises(ComposeError, match="operation_unconfirmed"):
        await runtime.start(reply["plan_sha256"])
    assert sum("up" in call[0] for call in driver.calls) == effects
    assert (await runtime.observe())["containers"]


@pytest.mark.asyncio
async def test_replacement_foreign_resource_and_engine_refuse(tmp_path):
    runtime, driver, reply = fixture_runtime(tmp_path)
    await runtime.prepare(b"{}")
    await runtime.start(reply["plan_sha256"])
    old = next(iter(driver.containers))
    del driver.containers[old]
    driver.container("db")
    with pytest.raises(ComposeError, match="resource_replaced"):
        await runtime.observe()


@pytest.mark.asyncio
async def test_request_dependency_refusal_and_cancellation_only_stops_request(tmp_path):
    runtime, driver, reply = fixture_runtime(tmp_path)
    await runtime.prepare(b"{}")
    first = await runtime.start(reply["plan_sha256"])
    async def source():
        yield b"opaque input"
    output = []
    async def sink(data):
        output.append(data)
    db = next(x for x in driver.containers.values() if x["Config"]["Labels"]["com.docker.compose.service"] == "db")
    db["State"].update(Running=False, Status="exited")
    with pytest.raises(ComposeError, match="dependency_unavailable"):
        await runtime.run_stdio("tool", reply["plan_sha256"], stdin=source(), stdout=sink, stderr=sink)
    db["State"].update(Running=True, Status="running")
    driver.wait_stream = True
    task = asyncio.create_task(runtime.run_stdio("tool", reply["plan_sha256"], stdin=source(), stdout=sink, stderr=sink))
    await driver.started.wait()
    with pytest.raises(ComposeError, match="operation_in_progress"):
        await runtime.stop(reply["plan_sha256"], first["observation"]["observation_sha256"])
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    stops = [call[0][-1] for call in driver.calls if "stop" in call[0]]
    assert len(stops) == 1
    assert driver.containers[stops[0]]["Config"]["Labels"]["com.docker.compose.service"] == "tool"
    assert output == [b'{"opaque":"protocol"}\n']
    assert (await runtime.observe())["available"] == {"db": True, "api": True}


@pytest.mark.asyncio
async def test_malformed_inspection_and_tool_tamper_are_fixed_refusals(tmp_path):
    runtime, driver, reply = fixture_runtime(tmp_path)
    await runtime.prepare(b"{}")
    Path(reply["plan"]["engine"]["docker"]["path"]).write_text("changed")
    with pytest.raises(ComposeError, match="tool_binding_changed"):
        await runtime.start(reply["plan_sha256"])
    assert not driver.containers


@pytest.mark.asyncio
async def test_real_process_dual_stream_limits_deadline_and_cancellation(tmp_path):
    runner = ProcessRunner()
    env = {"PATH": "/usr/bin:/bin"}
    reply = await runner.run([sys.executable, "-c", "import sys;sys.stdout.write('out');sys.stdout.close();sys.stderr.write('err')"], cwd=tmp_path, env=env)
    assert reply == CommandResult(0, b"out", b"err")
    with pytest.raises(ComposeError, match="command_output_limit"):
        await runner.run([sys.executable, "-c", "import sys;sys.stderr.write('x'*10000)"], cwd=tmp_path, env=env, limit=100)
    with pytest.raises(TimeoutError):
        await runner.run([sys.executable, "-c", "import time;time.sleep(10)"], cwd=tmp_path, env=env, timeout=0.05)


@pytest.mark.asyncio
async def test_explicit_resume_completes_partial_graph_without_replacing_database(tmp_path):
    runtime, driver, reply = fixture_runtime(tmp_path)
    original_run = driver.run
    fail_database = True
    async def partial(argv, **kwargs):
        nonlocal fail_database
        result = await original_run(argv, **kwargs)
        if "up" in argv and argv[-1] == "db" and fail_database:
            fail_database = False
            next(iter(driver.containers.values()))["State"].update(Running=False, Status="exited")
            return CommandResult(1)
        return result
    driver.run = partial
    await runtime.prepare(b"{}")
    failed = await runtime.start(reply["plan_sha256"])
    assert failed["status"] == "unconfirmed"
    assert failed["observation"]["available"] == {"db": False, "api": False}
    database_id = next(iter(driver.containers))
    assert len(driver.containers) == 1
    with pytest.raises(ComposeError, match="operation_unconfirmed"):
        await runtime.start(reply["plan_sha256"])
    observed = await runtime.observe()
    resumed = await runtime.resume(reply["plan_sha256"], observed["observation_sha256"])
    assert resumed["status"] == "observed"
    assert resumed["observation"]["available"] == {"db": True, "api": True}
    assert database_id in resumed["observation"]["containers"]
    assert sum("up" in call[0] and call[0][-1] == "db" for call in driver.calls) == 1
    # Previously observed missing containers are not recreated by resume.
    del driver.containers[database_id]
    observed = await runtime.observe()
    with pytest.raises(ComposeError, match="resources_disappeared"):
        await runtime.resume(reply["plan_sha256"], observed["observation_sha256"])


@pytest.mark.asyncio
async def test_actual_volume_observations_and_explicit_delete_disposition(tmp_path):
    runtime, driver, reply = fixture_runtime(tmp_path)
    plan = reply["plan"]
    plan["volumes"] = {"data": {"name": "fixture-data", "component": "db", "owner": "host", "format": 1}}
    document = json.loads(plan["compose_json"])
    document["volumes"] = {"fixture-data": {"name": "fixture-data"}}
    plan["compose_json"] = encoded(document).decode()
    plan["files"]["compose.json"].update(contents=plan["compose_json"], sha256=digest(encoded(document)))
    present = False
    volume = {"Name": "fixture-data", "CreatedAt": "2026-10-04T00:00:00Z", "Mountpoint": "/engine/fixture-data",
              "Driver": "local", "Labels": {"com.docker.compose.project": "fixture"}}
    original_run = driver.run
    async def run(argv, **kwargs):
        nonlocal present
        args = argv[5:]
        if "up" in argv:
            present = True
        if args[:2] == ["volume", "ls"]:
            return CommandResult(0, b"fixture-data\n" if present else b"")
        if args[:2] == ["volume", "inspect"]:
            return CommandResult(0, encoded([volume]))
        if args[:2] == ["volume", "rm"]:
            assert args[2] == "fixture-data"
            present = False
            return CommandResult(0)
        if args[:1] == ["ps"] and "volume=fixture-data" in args:
            return CommandResult(0)
        return await original_run(argv, **kwargs)
    driver.run = run
    await runtime.prepare(b"{}")
    first = await runtime.start(reply["plan_sha256"])
    stopped = await runtime.stop(reply["plan_sha256"], first["observation"]["observation_sha256"])
    assert present
    assert stopped["observation"]["volumes"]["data"]["created_at"] == volume["CreatedAt"]
    with pytest.raises(ComposeError, match="invalid_state_disposition"):
        await runtime.remove(reply["plan_sha256"], stopped["observation"]["observation_sha256"], delete_state={"other"})
    removed = await runtime.remove(reply["plan_sha256"], stopped["observation"]["observation_sha256"], delete_state={"data"})
    assert removed["status"] == "observed" and not present
    assert removed["observation"]["volumes"] == {}


@pytest.mark.asyncio
async def test_wrong_engine_malformed_health_and_foreign_members_refuse(tmp_path):
    runtime, driver, reply = fixture_runtime(tmp_path)
    await runtime.prepare(b"{}")
    await runtime.start(reply["plan_sha256"])
    item = next(iter(driver.containers.values()))
    item["State"]["Running"] = "yes"
    with pytest.raises(ComposeError, match="engine_observation_invalid"):
        await runtime.observe()
    item["State"]["Running"] = True
    item["Config"]["Healthcheck"] = {"Test": ["CMD", "different-check"]}
    with pytest.raises(ComposeError, match="resource_mismatch"):
        await runtime.observe()
    item["Config"]["Healthcheck"] = {"Test": ["NONE"]}
    driver.network["Containers"] = {"f" * 64: {}}
    with pytest.raises(ComposeError, match="foreign_network_member"):
        await runtime.observe()
    original_run = driver.run
    async def wrong_engine(argv, **kwargs):
        if "info" in argv:
            return CommandResult(0, encoded("different-engine"))
        return await original_run(argv, **kwargs)
    driver.run = wrong_engine
    with pytest.raises(ComposeError, match="engine_changed"):
        await runtime.observe()


@pytest.mark.asyncio
async def test_same_workspace_race_and_lost_start_reply_keep_one_attempt(tmp_path):
    runtime, driver, reply = fixture_runtime(tmp_path)
    await runtime.prepare(b"{}")
    entered, gate = asyncio.Event(), asyncio.Event()
    original_run = driver.run
    async def block(argv, **kwargs):
        result = await original_run(argv, **kwargs)
        if "up" in argv:
            entered.set()
            await gate.wait()
        return result
    driver.run = block
    first = asyncio.create_task(runtime.start(reply["plan_sha256"]))
    await entered.wait()
    with pytest.raises(ComposeError, match="operation_in_progress"):
        await runtime.start(reply["plan_sha256"])
    first.cancel()
    with pytest.raises(asyncio.CancelledError):
        await first
    assert (await runtime.observe())["pending_operation"] is True
    assert len(driver.containers) == 1
    assert sum("up" in call[0] for call in driver.calls) == 1


@pytest.mark.asyncio
async def test_symlinked_input_parent_is_refused_before_engine_entry(tmp_path):
    runtime, driver, reply = fixture_runtime(tmp_path)
    await runtime.prepare(b"{}")
    (runtime.workspace / "inputs").rename(runtime.workspace / "retained-inputs")
    (runtime.workspace / "inputs").symlink_to(runtime.workspace / "retained-inputs")
    with pytest.raises(ComposeError, match="invalid_retained_path"):
        await runtime.start(reply["plan_sha256"])
    assert not any("info" in call[0] for call in driver.calls)


@pytest.mark.asyncio
async def test_stream_callback_failure_still_joins_owned_request(tmp_path):
    runtime, driver, reply = fixture_runtime(tmp_path)
    await runtime.prepare(b"{}")
    await runtime.start(reply["plan_sha256"])
    async def source():
        yield b"request"
    async def failed_sink(data):
        raise RuntimeError("caller sink closed")
    result = await runtime.run_stdio("tool", reply["plan_sha256"], stdin=source(), stdout=failed_sink, stderr=failed_sink)
    assert result["status"] == "unconfirmed"
    assert result["request"]["stopped"] is True
    assert result["request"]["returncode"] is None
    assert all(item["State"]["Running"] for item in driver.containers.values()
               if item["Config"]["Labels"]["com.docker.compose.service"] in {"db", "api"})


@pytest.mark.asyncio
async def test_exact_image_environment_and_network_id_join(tmp_path):
    runtime, driver, reply = fixture_runtime(tmp_path)
    await runtime.prepare(b"{}")
    await runtime.start(reply["plan_sha256"])
    original_run = driver.run
    async def with_image_env(argv, **kwargs):
        result = await original_run(argv, **kwargs)
        if argv[5:7] == ["image", "inspect"]:
            value = json.loads(result.stdout)
            value[0]["Config"] = {"Env": ["IMAGE_DEFAULT=retained", "MODE=old"]}
            return CommandResult(0, encoded(value))
        return result
    driver.run = with_image_env
    for item in driver.containers.values():
        item["Config"]["Env"] += ["IMAGE_DEFAULT=retained"]
    assert all((await runtime.observe())["available"].values())
    item = next(iter(driver.containers.values()))
    item["Config"]["Env"].append("LD_PRELOAD=/unselected.so")
    with pytest.raises(ComposeError, match="resource_mismatch"):
        await runtime.observe()
    item["Config"]["Env"].pop()
    async def wrong_network(argv, **kwargs):
        result = await with_image_env(argv, **kwargs)
        if argv[5:7] == ["network", "inspect"]:
            value = json.loads(result.stdout)
            value[0]["Id"] = "f" * 64
            return CommandResult(0, encoded(value))
        return result
    driver.run = wrong_network
    with pytest.raises(ComposeError, match="engine_observation_invalid"):
        await runtime.observe()


@pytest.mark.asyncio
@pytest.mark.parametrize("action", ["start", "stdio"])
async def test_unbound_image_volume_refuses_before_any_creation(tmp_path, action):
    runtime, driver, reply = fixture_runtime(tmp_path)
    await runtime.prepare(b"{}")
    if action == "stdio":
        await runtime.start(reply["plan_sha256"])
    original_run = driver.run
    async def image_volume(argv, **kwargs):
        result = await original_run(argv, **kwargs)
        if argv[5:7] == ["image", "inspect"] and argv[-1] == reply["plan"]["components"]["tool"]["image"]:
            value = json.loads(result.stdout)
            value[0]["Config"] = {"Volumes": {"/unselected-state": {}}}
            return CommandResult(0, encoded(value))
        return result
    driver.run = image_volume
    before = len(driver.calls)
    async def source():
        yield b"request"
    async def sink(data):
        raise AssertionError("Unselected image state entered stream")
    with pytest.raises(ComposeError, match="^image_volume_unbound$"):
        if action == "start":
            await runtime.start(reply["plan_sha256"])
        else:
            await runtime.run_stdio("tool", reply["plan_sha256"], stdin=source(), stdout=sink, stderr=sink)
    assert all("compose" not in call[0][5:] for call in driver.calls[before:])
    assert not driver.started.is_set()


@pytest.mark.asyncio
async def test_image_volume_invalid_shapes_and_paths_fail_closed(tmp_path):
    runtime, driver, reply = fixture_runtime(tmp_path)
    await runtime.prepare(b"{}")
    original_run = driver.run
    volume_metadata = None
    async def image_volume(argv, **kwargs):
        result = await original_run(argv, **kwargs)
        if argv[5:7] == ["image", "inspect"]:
            value = json.loads(result.stdout)
            value[0]["Config"] = {"Volumes": volume_metadata}
            return CommandResult(0, encoded(value))
        return result
    driver.run = image_volume
    for volume_metadata in ([], False, "", {"relative": {}}, {"/": {}},
                            {"/data/": {}}, {"/a/../data": {}}, {"/a//data": {}},
                            {"/a\\data": {}}, {"/a\n": {}}, {"/configuration": None},
                            {"/configuration": {"option": "unsupported"}}):
        with pytest.raises(ComposeError, match="^engine_observation_invalid$"):
            await runtime.start(reply["plan_sha256"])
    assert not driver.containers
    assert all("compose" not in call[0][5:] for call in driver.calls)


@pytest.mark.asyncio
@pytest.mark.parametrize("kind", ["bind", "volume"])
async def test_image_volume_explicit_mount_coverage_passes_preflight(tmp_path, kind):
    runtime, driver, reply = fixture_runtime(tmp_path)
    plan = reply["plan"]
    document = json.loads(plan["compose_json"])
    if kind == "volume":
        document["services"]["db"]["volumes"] = [{"type": "volume", "source": "fixture-data",
                                                  "target": "/configuration", "read_only": False}]
        document["volumes"] = {"fixture-data": {"name": "fixture-data"}}
        plan["volumes"] = {"data": {"name": "fixture-data", "component": "db", "owner": "host", "format": 1}}
        plan["compose_json"] = encoded(document).decode()
        plan["files"]["compose.json"].update(contents=plan["compose_json"], sha256=digest(encoded(document)))
    original_run = driver.run
    async def image_volume(argv, **kwargs):
        result = await original_run(argv, **kwargs)
        if argv[5:7] == ["image", "inspect"] and argv[-1] == plan["components"]["db"]["image"]:
            value = json.loads(result.stdout)
            value[0]["Config"] = {"Volumes": {"/configuration": {}}}
            return CommandResult(0, encoded(value))
        return result
    driver.run = image_volume
    await runtime.prepare(b"{}")
    assert (await runtime.observe())["containers"] == {}


def docker29_identity_fixture(tmp_path):
    # Exact identity fields from acceptance/image-identity-readback.json. Other
    # fields remain the test driver's synthetic config, not a real engine run.
    runtime, driver, reply = fixture_runtime(tmp_path)
    source = "48b0309ca019d89d40f670aa1bc06e426dc0931948452e8491e3d65087abc07d"
    image_id = "sha256:1832327faf048390adc33852575d37c7ba155e064a339e78b9bd81983a8c7a00"
    descriptor = {"mediaType": "application/vnd.oci.image.manifest.v1+json", "digest": image_id,
                  "platform": {"architecture": "arm64", "os": "linux", "variant": "v8"}}
    expected = reply["plan"]["components"]["db"]
    expected.update(image="alpine@sha256:" + source, image_id=image_id)
    document = json.loads(reply["plan"]["compose_json"])
    document["services"]["db"]["image"] = expected["image"]
    reply["plan"]["compose_json"] = encoded(document).decode()
    reply["plan"]["files"]["compose.json"].update(contents=encoded(document).decode(), sha256=digest(encoded(document)))
    original_run, original_container = driver.run, driver.container
    async def inspect(argv, **kwargs):
        result = await original_run(argv, **kwargs)
        if argv[5:7] == ["image", "inspect"] and argv[-1] == expected["image"]:
            return CommandResult(0, encoded([{"Id": image_id, "Os": "linux", "Architecture": "arm64",
                                             "Descriptor": descriptor}]))
        return result
    def container(service, **kwargs):
        identity = original_container(service, **kwargs)
        if service == "db":
            driver.containers[identity].update(Image="sha256:" + source,
                                               ImageManifestDescriptor=copy.deepcopy(descriptor))
        return identity
    driver.run, driver.container = inspect, container
    return runtime, driver, reply, descriptor


@pytest.mark.asyncio
async def test_actual_docker29_identity_fields_join_source_and_platform(tmp_path):
    runtime, driver, reply, descriptor = docker29_identity_fixture(tmp_path)
    await runtime.prepare(b"{}")
    result = await runtime.start(reply["plan_sha256"])
    assert result["status"] == "observed"
    db = next(v for v in result["observation"]["containers"].values() if v["component"] == "db")
    assert db["image_id"] == descriptor["digest"]
    assert db["container_image_id"] != db["image_id"]
    assert db["image_manifest"] == descriptor


@pytest.mark.asyncio
async def test_docker29_identity_conflicts_refuse_before_further_effects(tmp_path):
    runtime, driver, reply, descriptor = docker29_identity_fixture(tmp_path)
    await runtime.prepare(b"{}")
    await runtime.start(reply["plan_sha256"])
    identity = next(i for i, v in driver.containers.items() if v["Config"]["Labels"]["com.docker.compose.service"] == "db")
    original = copy.deepcopy(driver.containers[identity])
    cases = [
        {"Image": "sha256:" + "f" * 64},
        {"ImageManifestDescriptor": None},
        {"ImageManifestDescriptor": {}},
        {"ImageManifestDescriptor": dict(descriptor, digest="sha256:" + "f" * 64)},
        {"ImageManifestDescriptor": dict(descriptor, mediaType="application/vnd.docker.distribution.manifest.v2+json")},
        {"ImageManifestDescriptor": dict(descriptor, platform=None)},
        {"ImageManifestDescriptor": dict(descriptor, platform={"os": "linux", "architecture": "amd64"})},
        {"ImageManifestDescriptor": dict(descriptor, platform={"os": "linux", "architecture": "arm64", "variant": "v7"})},
        # Exact platform ID does not excuse a contradictory optional descriptor.
        {"Image": descriptor["digest"], "ImageManifestDescriptor": dict(descriptor, digest="sha256:" + "f" * 64)},
    ]
    for mutation in cases:
        driver.containers[identity] = dict(copy.deepcopy(original), **mutation)
        before = len(driver.calls)
        with pytest.raises(ComposeError, match="^(resource_mismatch|engine_observation_invalid)$"):
            await runtime.start(reply["plan_sha256"])
        assert all("compose" not in call[0][5:] for call in driver.calls[before:])
    driver.containers[identity] = copy.deepcopy(original)
    driver.containers[identity]["Config"]["Image"] = "alpine@sha256:" + "f" * 64
    with pytest.raises(ComposeError, match="^resource_mismatch$"):
        await runtime.observe()
    driver.containers[identity] = copy.deepcopy(original)
    image_descriptor = copy.deepcopy(descriptor)
    for mutation in ({}, dict(image_descriptor, digest="sha256:" + "f" * 64),
                     dict(image_descriptor, platform={"os": "linux", "architecture": "amd64"})):
        descriptor.clear()
        descriptor.update(mutation)
        with pytest.raises(ComposeError, match="^(resource_mismatch|engine_observation_invalid)$"):
            await runtime.observe()


@pytest.mark.asyncio
async def test_local_image_exact_id_has_no_registry_alias_escape(tmp_path):
    runtime, driver, reply = fixture_runtime(tmp_path)
    expected = reply["plan"]["components"]["db"]
    expected["image"] = expected["image_id"]
    document = json.loads(reply["plan"]["compose_json"])
    document["services"]["db"]["image"] = expected["image"]
    reply["plan"]["compose_json"] = encoded(document).decode()
    reply["plan"]["files"]["compose.json"].update(contents=encoded(document).decode(), sha256=digest(encoded(document)))
    # Docker's local single-manifest image readback may omit descriptor platform.
    descriptor = {"mediaType": "application/vnd.docker.distribution.manifest.v2+json",
                  "digest": expected["image_id"]}
    original_run, original_container = driver.run, driver.container
    async def inspect(argv, **kwargs):
        result = await original_run(argv, **kwargs)
        if argv[5:7] == ["image", "inspect"] and argv[-1] == expected["image"]:
            value = json.loads(result.stdout)
            value[0]["Descriptor"] = descriptor
            return CommandResult(0, encoded(value))
        return result
    def container(service, **kwargs):
        identity = original_container(service, **kwargs)
        if service == "db":
            driver.containers[identity]["ImageManifestDescriptor"] = copy.deepcopy(descriptor)
        return identity
    driver.run, driver.container = inspect, container
    await runtime.prepare(b"{}")
    assert (await runtime.start(reply["plan_sha256"]))["status"] == "observed"
    db = next(v for v in driver.containers.values() if v["Config"]["Labels"]["com.docker.compose.service"] == "db")
    db["Image"] = "sha256:" + "f" * 64
    with pytest.raises(ComposeError, match="^resource_mismatch$"):
        await runtime.observe()


@pytest.mark.asyncio
async def test_alias_presence_and_cross_service_claims_refuse_effects(tmp_path):
    runtime, driver, reply = fixture_runtime(tmp_path)
    await runtime.prepare(b"{}")
    initial = (await runtime.start(reply["plan_sha256"]))["observation"]
    async def source():
        yield b"request"
    async def sink(data):
        pass
    db = next(v for v in driver.containers.values() if v["Config"]["Labels"]["com.docker.compose.service"] == "db")
    endpoint = db["NetworkSettings"]["Networks"]["fixture-network"]
    original = endpoint["Aliases"]
    endpoint["Aliases"] = ["fixture-db-1"]
    before = len(driver.calls)
    for call in (lambda: runtime.start(reply["plan_sha256"]),
                 lambda: runtime.resume(reply["plan_sha256"], initial["observation_sha256"]),
                 lambda: runtime.run_stdio("tool", reply["plan_sha256"], stdin=source(), stdout=sink, stderr=sink)):
        with pytest.raises(ComposeError, match="^resource_mismatch$"):
            await call()
    assert all("compose" not in call[0][5:] for call in driver.calls[before:])
    endpoint["Aliases"] = original
    await runtime.run_stdio("tool", reply["plan_sha256"], stdin=source(), stdout=sink, stderr=sink)
    for role in ("api", "tool"):
        item = next(v for v in driver.containers.values() if v["Config"]["Labels"]["com.docker.compose.service"] == role)
        aliases = item["NetworkSettings"]["Networks"]["fixture-network"]["Aliases"]
        aliases.append("DB.")
        with pytest.raises(ComposeError, match="^resource_mismatch$"):
            await runtime.observe()
        aliases.pop()


@pytest.mark.asyncio
async def test_alias_facts_are_normalized_and_protect_stale_actions(tmp_path):
    runtime, driver, reply = fixture_runtime(tmp_path)
    await runtime.prepare(b"{}")
    initial = (await runtime.start(reply["plan_sha256"]))["observation"]
    db = next(v for v in driver.containers.values() if v["Config"]["Labels"]["com.docker.compose.service"] == "db")
    endpoint = db["NetworkSettings"]["Networks"]["fixture-network"]
    endpoint["Aliases"] = ["DB.", "fixture-db-1", "db"]
    assert (await runtime.observe())["observation_sha256"] == initial["observation_sha256"]
    endpoint["Aliases"].append("additional-alias")
    assert (await runtime.observe())["observation_sha256"] != initial["observation_sha256"]
    with pytest.raises(ComposeError, match="^stale_observation$"):
        await runtime.stop(reply["plan_sha256"], initial["observation_sha256"])
    assert not any("stop" in call[0] for call in driver.calls)


@pytest.mark.asyncio
async def test_alias_shape_bounds_and_stopped_oneoff_semantics(tmp_path):
    runtime, driver, reply = fixture_runtime(tmp_path)
    await runtime.prepare(b"{}")
    await runtime.start(reply["plan_sha256"])
    db = next(v for v in driver.containers.values() if v["Config"]["Labels"]["com.docker.compose.service"] == "db")
    endpoint = db["NetworkSettings"]["Networks"]["fixture-network"]
    original = endpoint["Aliases"]
    for value in ("db", {}, [None], [3], [""], ["db\n"], ["x" * 254], ["db"] * 257,
                  ["db.."], ["/db"], ["-db"], ["db-"], ["db_name"], [".db"], ["\u212a"]):
        endpoint["Aliases"] = value
        with pytest.raises(ComposeError, match="^engine_observation_invalid$"):
            await runtime.observe()
    endpoint["Aliases"] = original
    async def source():
        yield b"request"
    async def sink(data):
        pass
    assert (await runtime.run_stdio("tool", reply["plan_sha256"], stdin=source(), stdout=sink, stderr=sink))["status"] == "observed"
    for item in driver.containers.values():
        item["State"].update(Running=False, Status="exited")
        item["NetworkSettings"]["Networks"]["fixture-network"]["Aliases"] = None
    stopped = await runtime.observe()
    assert not any(stopped["available"].values())
    assert all(item["aliases"] == [] for item in stopped["containers"].values())
    assert (await runtime.remove(reply["plan_sha256"], stopped["observation_sha256"], delete_state=set()))["status"] == "observed"


@pytest.mark.asyncio
async def test_host_bind_prefix_keeps_identity_and_explicit_mixed_paths(tmp_path):
    runtime, driver, reply = fixture_runtime(tmp_path)
    local = str(runtime.workspace.parent.resolve())
    reply["plan"]["engine"]["bind_source_prefix"] = {"local_root": local, "engine_root": "/engine/shared"}
    reply["plan_sha256"] = digest(encoded(reply["plan"]))
    await runtime.prepare(b"{}")
    first = (await runtime.start(reply["plan_sha256"]))["observation"]
    db = next(v for v in driver.containers.values() if v["Config"]["Labels"]["com.docker.compose.service"] == "db")
    host_source = db["Mounts"][0]["Source"]
    db["Mounts"][0]["Source"] = "/engine/shared" + host_source[len(local):]
    current = await runtime.observe()
    mounts = {v["component"]: v["mounts"][0] for v in current["containers"].values()}
    assert mounts["db"]["source"] == db["Mounts"][0]["Source"]
    assert mounts["db"]["host_source"] == host_source
    assert mounts["api"]["source"] == mounts["api"]["host_source"] == host_source
    assert current["observation_sha256"] != first["observation_sha256"]
    with pytest.raises(ComposeError, match="^stale_observation$"):
        await runtime.stop(reply["plan_sha256"], first["observation_sha256"])
    assert not any("stop" in call[0] for call in driver.calls)
    assert (await runtime.stop(reply["plan_sha256"], current["observation_sha256"]))["status"] == "observed"


def test_host_bind_prefix_refuses_unmapped_siblings_suffixes_and_null(tmp_path):
    runtime, driver, reply = fixture_runtime(tmp_path)
    raw = driver.containers[driver.container("db")]
    host_source = raw["Mounts"][0]["Source"]
    local = str(runtime.workspace.parent.resolve())
    prefix = {"local_root": local, "engine_root": "/engine/shared"}
    mapped = "/engine/shared" + host_source[len(local):]
    plan = reply["plan"]
    for setting in (None, {}):
        if setting is None:
            plan["engine"]["bind_source_prefix"] = None
        else:
            plan["engine"].pop("bind_source_prefix", None)
        raw["Mounts"][0]["Source"] = host_source
        assert runtime._container(plan, raw, {}, {"db": {}})["mounts"][0]["host_source"] == host_source
        raw["Mounts"][0]["Source"] = mapped
        with pytest.raises(ComposeError, match="^resource_mismatch$"):
            runtime._container(plan, raw, {}, {"db": {}})
    plan["engine"]["bind_source_prefix"] = prefix
    for bad in (mapped + "-other", mapped.replace("/engine/shared/", "/engine/shared-other/"),
                "/other" + host_source, mapped + "/../configuration"):
        raw["Mounts"][0]["Source"] = bad
        with pytest.raises(ComposeError, match="^resource_mismatch$"):
            runtime._container(plan, raw, {}, {"db": {}})
    # A sibling of the declared local root is not a descendant of that root.
    plan["engine"]["bind_source_prefix"] = {"local_root": local + "-other", "engine_root": "/engine/shared"}
    raw["Mounts"][0]["Source"] = mapped
    with pytest.raises(ComposeError, match="^resource_mismatch$"):
        runtime._container(plan, raw, {}, {"db": {}})
    # A textual prefix ending within a path component cannot translate it.
    partial_root = str(runtime.workspace)[:-1]
    plan["engine"]["bind_source_prefix"] = {"local_root": partial_root, "engine_root": "/engine/shared"}
    raw["Mounts"][0]["Source"] = "/engine/shared" + host_source[len(partial_root):]
    with pytest.raises(ComposeError, match="^resource_mismatch$"):
        runtime._container(plan, raw, {}, {"db": {}})


def test_host_bind_prefix_defensively_rejects_invalid_declarations(tmp_path):
    runtime, driver, reply = fixture_runtime(tmp_path)
    raw = driver.containers[driver.container("db")]
    valid = {"local_root": "/local", "engine_root": "/engine"}
    invalid = [False, [], {}, {"local_root": "/local"}, dict(valid, extra="no")]
    for root in (None, 1, "/", "relative", "/local/", "/local//nested", "/local/../nested",
                 "/local/./nested", "/local\\nested", "/local\n", "/local\x7f", "/\u212a", "/" + "x" * 257):
        invalid.append(dict(valid, local_root=root))
        invalid.append(dict(valid, engine_root=root))
    invalid += [{"local_root": "/same", "engine_root": "/same"},
                {"local_root": "/parent", "engine_root": "/parent/child"},
                {"local_root": "/parent/child", "engine_root": "/parent"}]
    for prefix in invalid:
        reply["plan"]["engine"]["bind_source_prefix"] = prefix
        with pytest.raises(ComposeError, match="^invalid_bind_source_prefix$"):
            runtime._container(reply["plan"], raw, {}, {"db": {}})


@pytest.mark.asyncio
async def test_changed_host_bind_prefix_invalidates_retained_plan_before_engine(tmp_path):
    runtime, driver, reply = fixture_runtime(tmp_path)
    reply["plan"]["engine"]["bind_source_prefix"] = {"local_root": "/local", "engine_root": "/engine"}
    reply["plan_sha256"] = digest(encoded(reply["plan"]))
    await runtime.prepare(b"{}")
    first = (await runtime.start(reply["plan_sha256"]))["observation"]
    original_digest = reply["plan_sha256"]
    reply["plan"]["engine"]["bind_source_prefix"]["engine_root"] = "/another-engine"
    reply["plan_sha256"] = digest(encoded(reply["plan"]))
    before = len(driver.calls)
    with pytest.raises(ComposeError, match="^plan_changed$"):
        await runtime.stop(original_digest, first["observation_sha256"])
    assert all(len(call[0]) == 1 for call in driver.calls[before:])


@pytest.mark.asyncio
async def test_repeated_cancellation_joins_request_cleanup_before_unlock(tmp_path):
    runtime, driver, reply = fixture_runtime(tmp_path)
    await runtime.prepare(b"{}")
    await runtime.start(reply["plan_sha256"])
    driver.wait_stream = True
    cleanup_entered, cleanup_release = asyncio.Event(), asyncio.Event()
    original_run = driver.run
    async def block_cleanup(argv, **kwargs):
        if argv[5:7] == ["container", "stop"]:
            cleanup_entered.set()
            await cleanup_release.wait()
        return await original_run(argv, **kwargs)
    driver.run = block_cleanup
    async def source():
        yield b"request"
    async def sink(data):
        pass
    task = asyncio.create_task(runtime.run_stdio("tool", reply["plan_sha256"], stdin=source(), stdout=sink, stderr=sink))
    await driver.started.wait()
    task.cancel()
    await cleanup_entered.wait()
    for _ in range(3):
        task.cancel()
        await asyncio.sleep(0)
        assert not task.done()
        with pytest.raises(ComposeError, match="operation_in_progress"):
            await runtime.observe()
    cleanup_release.set()
    with pytest.raises(asyncio.CancelledError):
        await task
    observed = await runtime.observe()
    assert all(observed["available"].values())
    assert not any(c["running"] for c in observed["containers"].values() if c["kind"] == "request")


@pytest.mark.asyncio
async def test_cli_pipe_backpressure_and_large_process_overflow_are_bounded(tmp_path):
    import os
    from meerkat_toolkit.compose import _PipeSink
    read_fd, write_fd = os.pipe()
    sink = _PipeSink(write_fd)
    ticks = []
    async def tick():
        await asyncio.sleep(0.02)
        ticks.append(True)
    ticker = asyncio.create_task(tick())
    try:
        with pytest.raises(TimeoutError):
            async with asyncio.timeout(0.05):
                await sink.write(b"x" * (4 * 1024 * 1024))
        await ticker
        assert ticks == [True]
    finally:
        sink.close()
        os.close(read_fd)
        os.close(write_fd)
    runner = ProcessRunner()
    with pytest.raises(ComposeError, match="command_output_limit"):
        async with asyncio.timeout(3):
            await runner.run([sys.executable, "-c", "import sys;sys.stdout.buffer.write(b'x'*16000000);sys.stderr.buffer.write(b'y'*16000000)"],
                             cwd=tmp_path, env={"PATH": "/usr/bin:/bin"}, limit=100)


@pytest.mark.asyncio
@pytest.mark.parametrize("method", ["run", "stream"])
async def test_reaped_successful_process_is_never_signalled(tmp_path, monkeypatch, method):
    import meerkat_toolkit.compose as module
    signals = []
    monkeypatch.setattr(module.os, "killpg", lambda pid, sig: signals.append((pid, sig)))
    runner = ProcessRunner()
    command = [sys.executable, "-c", "print('completed')"]
    if method == "run":
        result = await runner.run(command, cwd=tmp_path, env={"PATH": "/usr/bin:/bin"})
        assert result.returncode == 0 and result.stdout == b"completed\n"
    else:
        chunks = []
        async def source():
            if False:
                yield b""
        async def sink(data):
            chunks.append(data)
        result = await runner.stream(command, cwd=tmp_path, env={"PATH": "/usr/bin:/bin"},
                                     stdin=source(), stdout=sink, stderr=sink, timeout=2)
        assert result == 0 and b"".join(chunks) == b"completed\n"
    assert signals == []


@pytest.mark.asyncio
@pytest.mark.parametrize("method", ["run", "stream"])
async def test_live_child_cancellation_signals_only_owned_unreaped_group(tmp_path, monkeypatch, method):
    import signal
    import meerkat_toolkit.compose as module
    created = asyncio.Event()
    children, signals = [], []
    original_create = module.asyncio.create_subprocess_exec
    original_kill = module.os.killpg
    async def create(*args, **kwargs):
        child = await original_create(*args, **kwargs)
        children.append(child)
        created.set()
        return child
    def kill(pid, sig):
        assert len(children) == 1 and children[0].pid == pid
        assert children[0].returncode is None
        signals.append((pid, sig))
        original_kill(pid, sig)
    monkeypatch.setattr(module.asyncio, "create_subprocess_exec", create)
    monkeypatch.setattr(module.os, "killpg", kill)
    runner = ProcessRunner()
    command = [sys.executable, "-c", "import time;time.sleep(20)"]
    if method == "run":
        operation = runner.run(command, cwd=tmp_path, env={"PATH": "/usr/bin:/bin"})
    else:
        async def source():
            if False:
                yield b""
        async def sink(data):
            pass
        operation = runner.stream(command, cwd=tmp_path, env={"PATH": "/usr/bin:/bin"},
                                  stdin=source(), stdout=sink, stderr=sink, timeout=20)
    task = asyncio.create_task(operation)
    await created.wait()
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert signals == [(children[0].pid, signal.SIGKILL)]
    assert children[0].returncode == -signal.SIGKILL


@pytest.mark.asyncio
@pytest.mark.parametrize("failure", ["limit", "iterator", "iterator_after_bytes"])
async def test_stream_input_failure_cannot_be_reported_as_success(tmp_path, failure):
    async def source():
        if failure == "limit":
            yield b"12345"
        else:
            if failure == "iterator_after_bytes":
                yield b"12"
            raise RuntimeError("synthetic source failure")
            yield b""
    async def sink(data):
        pass
    expected_error = ComposeError if failure == "limit" else RuntimeError
    expected_message = "stream_input_limit" if failure == "limit" else "synthetic source failure"
    with pytest.raises(expected_error, match=expected_message):
        async with asyncio.timeout(2):
            await ProcessRunner().stream(
                [sys.executable, "-c", "import sys; data=sys.stdin.buffer.read();sys.stdout.write(str(len(data)))"],
                cwd=tmp_path, env={"PATH": "/usr/bin:/bin"}, stdin=source(),
                stdout=sink, stderr=sink, timeout=1, limit=4)


@pytest.mark.asyncio
async def test_stream_server_exit_does_not_wait_for_open_input(tmp_path):
    input_cancelled = asyncio.Event()
    async def source():
        try:
            await asyncio.Event().wait()
            yield b"unreachable"
        finally:
            input_cancelled.set()
    output = []
    async def sink(data):
        output.append(data)
    async with asyncio.timeout(2):
        result = await ProcessRunner().stream([sys.executable, "-c", "print('done')"],
            cwd=tmp_path, env={"PATH": "/usr/bin:/bin"}, stdin=source(),
            stdout=sink, stderr=sink, timeout=1)
    assert result == 0 and b"".join(output) == b"done\n"
    assert input_cancelled.is_set()


@pytest.mark.asyncio
@pytest.mark.parametrize("extra_components", [0, 24])
async def test_request_cleanup_cost_is_scoped_and_does_not_claim_graph_freshness(tmp_path, extra_components):
    runtime, driver, reply = fixture_runtime(tmp_path)
    plan = reply["plan"]
    document = json.loads(plan["compose_json"])
    for index in range(extra_components):
        name = "extra" + str(index)
        plan["components"][name] = dict(copy.deepcopy(plan["components"]["db"]), service=name)
        document["services"][name] = copy.deepcopy(document["services"]["db"])
        document["services"][name]["networks"]["default"]["aliases"] = [name]
        plan["deployment_order"].append(name)
        plan["initialization_order"].append(name)
        plan["shutdown_order"].insert(0, name)
    plan["compose_json"] = encoded(document).decode()
    plan["files"]["compose.json"].update(contents=plan["compose_json"], sha256=digest(encoded(document)))
    reply["plan_sha256"] = digest(encoded(plan))
    await runtime.prepare(b"{}")
    await runtime.start(reply["plan_sha256"])
    before_record = json.loads((runtime.workspace / "record.json").read_bytes())
    original_stream = driver.stream
    cleanup_offset = None
    async def stream(*args, **kwargs):
        nonlocal cleanup_offset
        result = await original_stream(*args, **kwargs)
        # A changed dependency is not a reason to skip stopping the owned request,
        # and must not be returned as a freshly inspected graph after cleanup.
        db = next(v for v in driver.containers.values() if v["Name"] == "/fixture-db-1")
        db["Config"]["Env"].append("UNSELECTED=value")
        cleanup_offset = len(driver.calls)
        return result
    driver.stream = stream
    async def source():
        yield b"request"
    async def sink(data):
        pass
    result = await runtime.run_stdio("tool", reply["plan_sha256"], stdin=source(), stdout=sink, stderr=sink)
    calls = [entry[0][5:] for entry in driver.calls[cleanup_offset:]]
    assert result["status"] == "observed"
    assert result["observation"] is None
    assert result["request"]["stopped"] is True
    assert [call[:2] for call in calls] == [["info", "--format"], ["image", "inspect"], ["container", "inspect"]]
    assert calls[1][-1] == plan["components"]["tool"]["image"]
    assert calls[2][-1] == result["request"]["name"]
    after_record = json.loads((runtime.workspace / "record.json").read_bytes())
    assert after_record["pending"] is False
    for identity, item in before_record["containers"].items():
        assert after_record["containers"][identity] == item
    with pytest.raises(ComposeError, match="^resource_mismatch$"):
        await runtime.observe()


@pytest.mark.asyncio
@pytest.mark.parametrize("mutation", ["name", "project", "component", "image", "descriptor", "environment", "mount", "network", "image_inspect"])
async def test_request_cleanup_refuses_foreign_or_changed_request_before_stop(tmp_path, mutation):
    runtime, driver, reply = fixture_runtime(tmp_path)
    await runtime.prepare(b"{}")
    await runtime.start(reply["plan_sha256"])
    original_stream, original_run = driver.stream, driver.run
    cleanup = False
    async def stream(*args, **kwargs):
        nonlocal cleanup
        result = await original_stream(*args, **kwargs)
        request = next(v for v in driver.containers.values() if "-request-" in v["Name"])
        request["State"].update(Running=True, Status="running")
        if mutation == "name":
            request["Name"] += "-different"
        elif mutation == "project":
            request["Config"]["Labels"]["com.docker.compose.project"] = "foreign"
        elif mutation == "component":
            request["Config"]["Labels"]["com.docker.compose.service"] = "db"
        elif mutation == "image":
            request["Image"] = "sha256:" + "f" * 64
        elif mutation == "descriptor":
            request["ImageManifestDescriptor"] = {"digest": "sha256:" + "f" * 64,
                "mediaType": "application/vnd.oci.image.manifest.v1+json"}
        elif mutation == "environment":
            request["Config"]["Env"].append("LD_PRELOAD=/foreign")
        elif mutation == "mount":
            request["Mounts"][0]["Source"] += "-foreign"
        elif mutation == "network":
            request["NetworkSettings"]["Networks"]["fixture-network"]["NetworkID"] = "f" * 64
        cleanup = True
        return result
    async def run(argv, **kwargs):
        result = await original_run(argv, **kwargs)
        if cleanup and mutation == "image_inspect" and argv[5:7] == ["image", "inspect"]:
            value = json.loads(result.stdout)
            value[0]["Id"] = "sha256:" + "f" * 64
            return CommandResult(0, encoded(value))
        return result
    driver.stream, driver.run = stream, run
    async def source():
        yield b"request"
    async def sink(data):
        pass
    result = await runtime.run_stdio("tool", reply["plan_sha256"], stdin=source(), stdout=sink, stderr=sink)
    assert result["status"] == "unconfirmed" and result["observation"] is None
    assert result["request"]["stopped"] is False
    assert json.loads((runtime.workspace / "record.json").read_bytes())["pending"] is True
    assert not any(call[0][5:7] == ["container", "stop"] for call in driver.calls)


@pytest.mark.asyncio
@pytest.mark.parametrize("failure", ["deadline", "replacement", "still_running"])
async def test_request_stop_requires_durable_id_and_exact_stopped_readback(tmp_path, failure):
    runtime, driver, reply = fixture_runtime(tmp_path)
    await runtime.prepare(b"{}")
    await runtime.start(reply["plan_sha256"])
    original_stream, original_run = driver.stream, driver.run
    request_id = None
    stopped = False
    async def stream(*args, **kwargs):
        nonlocal request_id
        code = await original_stream(*args, **kwargs)
        request_id = next(i for i, v in driver.containers.items() if "-request-" in v["Name"])
        driver.containers[request_id]["State"].update(Running=True, Status="running")
        return code
    async def run(argv, **kwargs):
        nonlocal stopped
        if argv[5:7] == ["container", "stop"]:
            record = json.loads((runtime.workspace / "record.json").read_bytes())
            assert record["pending"] is True
            assert record["containers"][request_id]["id"] == request_id == argv[-1]
            assert kwargs["timeout"] == 1
            stopped = True
            if failure == "deadline":
                driver.calls.append((list(argv), dict(kwargs["env"])))
                raise TimeoutError("synthetic stop deadline")
        result = await original_run(argv, **kwargs)
        if stopped and argv[5:7] == ["container", "inspect"]:
            assert argv[-1] == request_id  # Never resolve the name after stopping.
            rows = json.loads(result.stdout)
            if failure == "replacement":
                rows[0]["Id"] = "f" * 64
            elif failure == "still_running":
                rows[0]["State"].update(Running=True, Status="running")
            return CommandResult(0, encoded(rows))
        return result
    driver.stream, driver.run = stream, run
    async def source():
        yield b"request"
    async def sink(data):
        pass
    result = await runtime.run_stdio("tool", reply["plan_sha256"], stdin=source(), stdout=sink, stderr=sink)
    assert result["status"] == "unconfirmed" and result["request"]["stopped"] is False
    assert result["request"]["container_id"] == request_id
    assert json.loads((runtime.workspace / "record.json").read_bytes())["pending"] is True
    assert [c[0][-1] for c in driver.calls if c[0][5:7] == ["container", "stop"]] == [request_id]


@pytest.mark.asyncio
@pytest.mark.parametrize("phase", ["capture", "settle"])
async def test_request_durable_save_failure_cannot_report_success(tmp_path, phase):
    runtime, driver, reply = fixture_runtime(tmp_path)
    await runtime.prepare(b"{}")
    await runtime.start(reply["plan_sha256"])
    original_save = runtime._save
    failed = False
    def save(record):
        nonlocal failed
        has_request = any(item["kind"] == "request" for item in record["containers"].values())
        wanted = record["pending"] if phase == "capture" else not record["pending"]
        original_save(record)
        if has_request and wanted and not failed:
            failed = True
            # Model the difficult case: replace occurred but final sync failed.
            raise OSError("synthetic directory fsync failure")
    runtime._save = save
    async def source():
        yield b"request"
    async def sink(data):
        pass
    result = await runtime.run_stdio("tool", reply["plan_sha256"], stdin=source(), stdout=sink, stderr=sink)
    assert failed and result["status"] == "unconfirmed" and result["request"]["stopped"] is False
    assert json.loads((runtime.workspace / "record.json").read_bytes())["pending"] is True
    assert not any(c[0][5:7] == ["container", "stop"] for c in driver.calls)


@pytest.mark.asyncio
async def test_request_only_graph_captures_exact_new_network(tmp_path):
    runtime, driver, reply = fixture_runtime(tmp_path)
    plan = reply["plan"]
    plan["components"] = {"tool": plan["components"]["tool"]}
    plan["components"]["tool"].update(dependencies=[], deployment_dependencies=[])
    plan.update(deployment_order=[], initialization_order=["tool"], shutdown_order=["tool"])
    document = json.loads(plan["compose_json"])
    document["services"] = {"tool": document["services"]["tool"]}
    plan["compose_json"] = encoded(document).decode()
    plan["files"]["compose.json"].update(contents=plan["compose_json"], sha256=digest(encoded(document)))
    reply["plan_sha256"] = digest(encoded(plan))
    await runtime.prepare(b"{}")
    initial = await runtime.start(reply["plan_sha256"])
    assert initial["observation"]["network"] is None
    async def source():
        yield b"request"
    async def sink(data):
        pass
    result = await runtime.run_stdio("tool", reply["plan_sha256"], stdin=source(), stdout=sink, stderr=sink)
    assert result["status"] == "observed" and result["observation"] is None
    record = json.loads((runtime.workspace / "record.json").read_bytes())
    assert record["pending"] is False
    assert record["network"] == {"id": "e" * 64, "name": "fixture-network"}
    assert (await runtime.observe())["network"] == record["network"]


@pytest.mark.asyncio
@pytest.mark.parametrize("failure", ["source", "limit"])
async def test_real_stream_input_failure_keeps_unconfirmed_result_after_request_stop(tmp_path, failure):
    import os
    runtime, driver, reply = fixture_runtime(tmp_path)
    await runtime.prepare(b"{}")
    await runtime.start(reply["plan_sha256"])
    child_ready = asyncio.Event()
    pid = None
    async def stream(argv, *, stdin, stdout, stderr, **kwargs):
        name = argv[argv.index("--name") + 1]
        driver.container("tool", name=name)
        async def child_output(chunk):
            nonlocal pid
            pid = int(chunk.strip())
            child_ready.set()
        return await ProcessRunner().stream(
            [sys.executable, "-c", "import os,time;print(os.getpid(),flush=True);time.sleep(30)"],
            cwd=tmp_path, env={"PATH": "/usr/bin:/bin"}, stdin=stdin,
            stdout=child_output, stderr=stderr, timeout=3, limit=100)
    driver.stream = stream
    async def source():
        await child_ready.wait()
        yield b"partial"
        if failure == "limit":
            yield b"x" * 101
        else:
            raise RuntimeError("synthetic source failure")
    async def sink(data):
        pass
    async with asyncio.timeout(5):
        result = await runtime.run_stdio("tool", reply["plan_sha256"], stdin=source(), stdout=sink, stderr=sink)
    assert result["status"] == "unconfirmed" and result["request"]["returncode"] is None
    assert result["request"]["stopped"] is True
    assert result["observation"] is None
    assert json.loads((runtime.workspace / "record.json").read_bytes())["pending"] is False
    with pytest.raises(ProcessLookupError):
        os.kill(pid, 0)
    assert len([c for c in driver.calls if c[0][5:7] == ["container", "stop"]]) == 1


@pytest.mark.asyncio
@pytest.mark.parametrize("status,stopped,code,accepted", [
    ("observed", True, 0, True), ("unconfirmed", True, 0, False),
    ("unconfirmed", False, 0, False), ("observed", False, 0, False),
    ("observed", True, None, False), ("observed", True, 1, False),
])
async def test_cli_success_requires_confirmed_request_cleanup(tmp_path, monkeypatch, status, stopped, code, accepted):
    import argparse
    import meerkat_toolkit.compose as module
    class Runtime:
        def __init__(self, *args):
            pass
        async def run_stdio(self, *args, **kwargs):
            return {"status": status, "observation": None,
                    "request": {"stopped": stopped, "returncode": code}}
    class Pipe:
        def __init__(self, *args):
            pass
        async def write(self, data):
            pass
        def close(self):
            pass
    async def connect(*args):
        return Pipe(), None
    monkeypatch.setattr(module, "ComposeRuntime", Runtime)
    monkeypatch.setattr(module, "_PipeSink", Pipe)
    monkeypatch.setattr(asyncio.get_running_loop(), "connect_read_pipe", connect)
    args = argparse.Namespace(workspace=tmp_path, planner="unused", planner_sha256="a" * 64,
                              action="run-stdio", component="tool", plan_sha256="b" * 64, timeout=3)
    if accepted:
        assert await module._main(args) is None
    else:
        with pytest.raises(ComposeError, match="^request_unconfirmed$"):
            await module._main(args)


def _signal_fixture_main(root, cleanup_delay):
    """Real CLI/signal boundary with inert engine facts, never a Docker proof."""
    import signal
    import meerkat_toolkit.compose as module
    root = Path(root)
    runtime, driver, reply = fixture_runtime(root)
    async def prepare():
        await runtime.prepare(b"{}")
        await runtime.start(reply["plan_sha256"])
    asyncio.run(prepare())
    closing = False
    original_run = driver.run
    async def stream(argv, *, stdin, stdout, **kwargs):
        nonlocal closing
        name = argv[argv.index("--name") + 1]
        identity = driver.container("tool", name=name)
        await stdout(b'{"jsonrpc":"2.0","method":"notifications/initialized"}\n')
        try:
            async for _ in stdin:
                pass
        finally:
            driver.containers[identity]["State"].update(Running=False, Status="exited")
            closing = True
        return 0
    async def run(argv, **kwargs):
        nonlocal closing
        if closing and argv[5:7] == ["info", "--format"]:
            closing = False
            (root / "cleanup-entered").write_text("entered")
            await asyncio.sleep(cleanup_delay)
        return await original_run(argv, **kwargs)
    driver.stream, driver.run = stream, run
    module.ComposeRuntime = lambda *args, **kwargs: runtime
    sys.argv = ["compose", "--workspace", str(runtime.workspace), "--planner", runtime.planner,
                "--planner-sha256", runtime.planner_sha256, "run-stdio",
                "--component", "tool", "--plan-sha256", reply["plan_sha256"]]
    previous = signal.getsignal(signal.SIGTERM)
    code = module.main()
    (root / "completed.json").write_text(json.dumps({"code": code,
        "handler_restored": signal.getsignal(signal.SIGTERM) is previous,
        "module": module.__file__, "module_sha256": digest(Path(module.__file__).read_bytes())}))
    raise SystemExit(code)


def _signal_fixture_command(root, delay):
    return [sys.executable, "-B", "-c",
            "import runpy,sys;runpy.run_path(sys.argv[1])['_signal_fixture_main'](sys.argv[2],float(sys.argv[3]))",
            str(Path(__file__).resolve()), str(root), str(delay)]


@pytest.mark.asyncio
@pytest.mark.parametrize("repeated", [False, True])
async def test_cli_term_after_two_second_eof_grace_joins_cleanup(tmp_path, repeated):
    import os
    import signal
    child = await asyncio.create_subprocess_exec(*_signal_fixture_command(tmp_path, 2.7),
        stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE, start_new_session=True)
    try:
        assert b"notifications/initialized" in await asyncio.wait_for(child.stdout.readline(), 5)
        begin = asyncio.get_running_loop().time()
        child.stdin.close()
        await asyncio.sleep(2)
        assert (tmp_path / "cleanup-entered").exists()
        assert child.returncode is None
        os.killpg(child.pid, signal.SIGTERM)
        if repeated:
            for _ in range(3):
                await asyncio.sleep(.05)
                assert child.returncode is None
                os.killpg(child.pid, signal.SIGTERM)
        stdout, stderr = await asyncio.wait_for(child.communicate(), 3)
        assert child.returncode == 1, (child.returncode, stderr)
        assert 2.5 <= asyncio.get_running_loop().time() - begin < 4
        assert stdout == b"" and stderr == b"compose_interrupted\n"
        completed = json.loads((tmp_path / "completed.json").read_bytes())
        assert completed["handler_restored"] is True
        record = json.loads((tmp_path / "workspace/record.json").read_bytes())
        assert record["pending"] is False
        assert all(not c["running"] for c in record["containers"].values() if c["kind"] == "request")
        assert all(c["running"] for c in record["containers"].values() if c["kind"] == "deployment")
    finally:
        if child.returncode is None:
            os.killpg(child.pid, signal.SIGKILL)
        await child.wait()


@pytest.mark.asyncio
async def test_cli_hard_kill_after_term_keeps_durable_pending(tmp_path):
    import fcntl
    import os
    import signal
    child = await asyncio.create_subprocess_exec(*_signal_fixture_command(tmp_path, 8),
        stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE, start_new_session=True)
    try:
        await asyncio.wait_for(child.stdout.readline(), 5)
        child.stdin.close()
        await asyncio.sleep(2)
        assert (tmp_path / "cleanup-entered").exists()
        os.killpg(child.pid, signal.SIGTERM)
        await asyncio.sleep(2)
        assert child.returncode is None
        os.killpg(child.pid, signal.SIGKILL)
        await asyncio.wait_for(child.communicate(), 2)
        assert child.returncode == -signal.SIGKILL
        assert not (tmp_path / "completed.json").exists()
        assert json.loads((tmp_path / "workspace/record.json").read_bytes())["pending"] is True
        with (tmp_path / "workspace/.lock").open("rb") as lock:
            fcntl.flock(lock.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
    finally:
        if child.returncode is None:
            os.killpg(child.pid, signal.SIGKILL)
        await child.wait()


@pytest.mark.asyncio
@pytest.mark.parametrize("terminate", [False, True])
async def test_cli_normal_eof_or_active_stream_term_restores_handler(tmp_path, terminate):
    import os
    import signal
    child = await asyncio.create_subprocess_exec(*_signal_fixture_command(tmp_path, .05),
        stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE, start_new_session=True)
    try:
        await asyncio.wait_for(child.stdout.readline(), 5)
        if terminate:
            os.killpg(child.pid, signal.SIGTERM)
        else:
            child.stdin.close()
        stdout, stderr = await asyncio.wait_for(child.communicate(), 3)
        assert child.returncode == (1 if terminate else 0), stderr
        completed = json.loads((tmp_path / "completed.json").read_bytes())
        assert completed["handler_restored"] is True
        assert json.loads((tmp_path / "workspace/record.json").read_bytes())["pending"] is False
        if terminate:
            assert stderr == b"compose_interrupted\n"
    finally:
        if child.returncode is None:
            os.killpg(child.pid, signal.SIGKILL)
        await child.wait()


@pytest.mark.asyncio
@pytest.mark.parametrize("outcome", ["success", "error", "cancelled", "other_action"])
async def test_cli_restores_existing_signal_handler_on_every_exit(monkeypatch, outcome):
    import argparse
    import signal
    import meerkat_toolkit.compose as module
    previous = signal.getsignal(signal.SIGTERM)
    def host_handler(signum, frame):
        pass
    async def operation(args):
        if outcome == "other_action":
            assert signal.getsignal(signal.SIGTERM) is host_handler
        else:
            assert signal.getsignal(signal.SIGTERM) is not host_handler
        if outcome == "error":
            raise ComposeError("fixture_error")
        if outcome == "cancelled":
            raise asyncio.CancelledError
        return {"status": "fixture"}
    monkeypatch.setattr(module, "_main", operation)
    signal.signal(signal.SIGTERM, host_handler)
    try:
        args = argparse.Namespace(action="observe" if outcome == "other_action" else "run-stdio")
        if outcome in ("error", "cancelled"):
            with pytest.raises(ComposeError if outcome == "error" else asyncio.CancelledError):
                await module._cli(args)
        else:
            assert await module._cli(args) == {"status": "fixture"}
        assert signal.getsignal(signal.SIGTERM) is host_handler
    finally:
        signal.signal(signal.SIGTERM, previous)
