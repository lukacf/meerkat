"""Inert preparation of the tools selected by a local-knowledge cloud package."""
from copy import deepcopy
import ast
import base64
import hashlib
import json
import os
from pathlib import Path
import subprocess
import zlib

import pytest

from launchkit.blueprint import Blueprint, starter_blueprint
from launchkit import gcp_bootstrap as bootstrap
from launchkit.compiler import _runtime


def selected_definition(*commands, mode="local"):
    value = starter_blueprint().model_dump(mode="json")
    value["target"] = "gcp"
    value["gcp"].update(project="forge-knowledge-123", machine_type="e2-standard-8", disk_gb=100)
    value["knowledge"]["mode"] = mode
    checked = _runtime(Blueprint.model_validate(value))
    checked["mcpConnections"] = [{"id": "connector_" + str(index), "transport": "stdio", "command": command}
        for index, command in enumerate(commands)]
    checked["classes"][0]["tools"]["mcp"] = [{"connection_id": item["id"]}
        for item in checked["mcpConnections"]]
    return checked


def test_valid_local_knowledge_plan_selects_its_required_guest_tools():
    selected = selected_definition()
    original = deepcopy(selected)
    script = bootstrap.bootstrap_script(selected)
    assert "apt-get install -y git curl" in script
    assert "docker-compose-plugin" in script
    assert "docker-buildx-plugin" in script
    assert "usermod -aG docker mobkit" in script
    assert "runuser -u mobkit -- docker info" in script
    assert "--wait-timeout" in script
    assert selected == original


RUNTIME_SHA256 = "c1f880599035d1c82b29703f113a1998b40f249a9026ac7b687c7c427aa6bc15"
OLD_SCRIPT_HASHES = {
    (False, False): "2de304f852d356aa13874db5cf4c52c3c62f007783fe029a6923aa771030c7c8",
    (True, False): "251dcb081bac2a1a650710fdfba04fc4c0a2c216103182a1a2cb9375e71e83ee",
    (False, True): "6db638e185403895e93b504f34abaa4f46221e49fa5e1ebe3677b151e73fa839",
    (True, True): "dd183687ecc151e528bf19d2b44f62c5cfead6f55752f5ae7a5cd3768f2e1ed0",
}
TOOL_SELECTIONS = [((), (False, False)), (("npx",), (True, False)),
    (("uvx",), (False, True)), (("npm", "uv"), (True, True))]


@pytest.mark.parametrize(("commands", "expected"), TOOL_SELECTIONS)
def test_local_knowledge_payload_preserves_exact_runtime_selection_and_size(commands, expected):
    script = bootstrap.bootstrap_script(selected_definition(*commands))
    assert len(script.encode()) <= 8192
    assert subprocess.run(["bash", "-n"], input=script, text=True, capture_output=True).returncode == 0
    assert hashlib.sha256(bootstrap.RUNTIME_SETUP.encode()).hexdigest() == RUNTIME_SHA256
    if not any(expected):
        assert "MOBKIT_PREREQUISITES" not in script
        return
    code = script.split("<<'MOBKIT_PREREQUISITES'\n", 1)[1].split("\nMOBKIT_PREREQUISITES", 1)[0]
    setup, call = code.rsplit("\ninstall(", 1)
    payload = ast.parse(setup).body[1].value.args[0].args[0].args[0].value
    assert zlib.decompress(base64.b64decode(payload)) == bootstrap.RUNTIME_SETUP.encode()
    observed = []
    namespace = {"__name__": "inert_guest"}
    exec(compile(setup, "source-owned decoded bootstrap", "exec"), namespace)
    namespace["install"] = lambda node, uv: observed.append((node, uv))
    exec("install(" + call, namespace)
    assert observed == [expected]


@pytest.mark.parametrize("mode", ["none", "remote", "omitted"])
@pytest.mark.parametrize(("commands", "expected"), TOOL_SELECTIONS)
def test_nonlocal_selection_keeps_exact_historical_script(mode, commands, expected):
    selected = selected_definition(*commands, mode="none")
    if mode == "omitted":
        selected.pop("knowledge")
    else:
        selected["knowledge"]["mode"] = mode
    before = deepcopy(selected)
    script = bootstrap.bootstrap_script(selected)
    assert hashlib.sha256(script.encode()).hexdigest() == OLD_SCRIPT_HASHES[expected]
    assert "docker" not in script and "git curl" not in script
    assert before == selected


def test_local_knowledge_does_not_acquire_ungranted_http_or_private_connector_commands():
    selected = selected_definition("custom-command", "${PRIVATE_COMMAND}")
    selected["mcpConnections"] += [{"id": "ungranted", "transport": "stdio", "command": "npx"},
        {"id": "http_connector", "transport": "http", "command": "uvx"}]
    selected["classes"][0]["tools"]["mcp"].append({"connection_id": "http_connector"})
    script = bootstrap.bootstrap_script(selected)
    assert "docker-compose-plugin" in script
    assert "MOBKIT_PREREQUISITES" not in script
    assert "PRIVATE_COMMAND" not in script


def inert_guest(tmp_path, *, failure="", delayed_user=False, existing_group=False,
        manager_group=False, manager_absent=False, manager_status=None, reuse=False,
        script=None, login_user="cloud_operator"):
    """Run shell control flow only; every host mutation tool is a local recorder."""
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir(exist_ok=reuse)
    shim = bin_dir / "shim"
    shim.write_text("""#!/usr/bin/env python3
import json,os,sys
from pathlib import Path
root=Path(os.environ['INERT_GUEST_ROOT']); command=Path(sys.argv[0]).name; args=sys.argv[1:]
with (root/'calls.jsonl').open('a') as output: output.write(json.dumps([command,*args])+'\\n')
label=command+(' '+args[0] if args else '')
if os.environ.get('INERT_FAILURE')==label: raise SystemExit(23)
if command=='install': Path(args[-1]).mkdir(parents=True,exist_ok=True)
elif command=='curl': Path(args[args.index('-o')+1]).write_text('inert repository key')
elif command=='dpkg': print('amd64')
elif command=='id':
 if '-un' in args: print(os.environ['INERT_LOGIN_USER'])
 elif '-u' in args: print('1234')
 elif '-nG' in args: print('mobkit docker' if (root/'docker-group').exists() else 'mobkit')
 elif os.environ.get('INERT_DELAYED_USER')=='1' and not (root/'seen-user').exists():
  (root/'seen-user').touch(); raise SystemExit(1)
elif command=='usermod': (root/'docker-group').touch()
elif command=='systemctl' and args[:3]==['enable','--now','docker']: (root/'docker-started').touch()
elif command=='systemctl' and args[:2]==['show','--property=MainPID']:
 count=int((root/'show-count').read_text())+1 if (root/'show-count').exists() else 1
 (root/'show-count').write_text(str(count))
 if os.environ.get('INERT_FAILURE')=='manager-pid': print('unknown')
 elif os.environ.get('INERT_FAILURE')=='manager-pid-changed' and count==2: print('4323')
 elif os.environ.get('INERT_FAILURE')=='manager-pid-final-changed' and count==4: print('4323')
 else: print((root/'manager-pid').read_text())
elif command=='systemctl' and args[0] in ('start','restart'):
 assert args[1]=='user@1234.service'
 (root/'manager-pid').write_text('4322')
 (root/'proc/4322').mkdir(parents=True,exist_ok=True)
 (root/'proc/4322/status').write_text('Groups:\\t1234 999\\n' if os.environ.get('INERT_FAILURE')!='manager-groups-after' else 'Groups:\\t1234\\n')
elif command=='getent':
 if os.environ.get('INERT_FAILURE')=='docker-gid': print('invalid group')
 elif os.environ.get('INERT_FAILURE')=='duplicate-docker-gid': print('docker:x:999:mobkit\\ndocker:x:999:mobkit')
 else: print('docker:x:999:mobkit')
elif command=='runuser':
 if not (root/'docker-group').exists() or not (root/'docker-started').exists(): raise SystemExit(24)
elif command=='docker':
 if os.environ.get('INERT_FAILURE')=='compose-flags': print('usage only')
 else: print('      --wait Wait for health\\n      --wait-timeout int Wait deadline')
elif command=='sleep': pass
elif command=='loginctl': pass
elif command=='sudo': os.execvp(args[0],args)
elif command not in ('apt-get','chmod','systemctl'): raise SystemExit('Unexpected mutation command')
""")
    shim.chmod(0o755)
    for command in ("apt-get", "install", "curl", "chmod", "dpkg", "systemctl", "id", "usermod",
            "runuser", "docker", "sleep", "loginctl", "getent", "sudo"):
        if not (bin_dir / command).exists():
            (bin_dir / command).symlink_to(shim)
    (tmp_path / "sources.list.d").mkdir(exist_ok=reuse)
    if existing_group:
        (tmp_path / "docker-group").touch()
    if not reuse:
        (tmp_path / "manager-pid").write_text("0" if manager_absent else "4321")
        (tmp_path / "proc/4321").mkdir(parents=True)
        (tmp_path / "proc/4321/status").write_text(manager_status if manager_status is not None
            else "Groups:\t1234" + (" 999" if manager_group else "") + "\n")
    if script is None:
        script = bootstrap.bootstrap_script(selected_definition())
    script = script.replace("/etc/apt/keyrings", str(tmp_path / "keyrings"))
    script = script.replace("/etc/apt/sources.list.d", str(tmp_path / "sources.list.d"))
    script = script.replace("/proc/", str(tmp_path / "proc") + "/")
    result = subprocess.run(["bash"], input=script, text=True, capture_output=True,
        env=dict(os.environ, PATH=str(bin_dir) + os.pathsep + os.environ["PATH"],
            INERT_GUEST_ROOT=str(tmp_path), INERT_FAILURE=failure,
            INERT_DELAYED_USER="1" if delayed_user else "0", INERT_LOGIN_USER=login_user,
            USER="incorrect-local-user"))
    return result, [json.loads(line) for line in (tmp_path / "calls.jsonl").read_text().splitlines()]


def manual_preparation(definition):
    import shlex
    from launchkit.generator import TEMPLATES
    script = bootstrap.render_manual_prerequisites((TEMPLATES / 'deploy-gcp.sh').read_text(), definition)
    assignment = script.split('HOST_PREPARATION=', 1)[1].split('\nRULE=', 1)[0]
    return shlex.split(assignment)[0]


@pytest.mark.parametrize('login_user', ['cloud_operator', "owner with 'quote; dollar$(false)"])
def test_exported_knowledge_preparation_uses_observed_ssh_user_without_enabling_background_services(tmp_path, login_user):
    result, calls = inert_guest(tmp_path, script=manual_preparation(selected_definition()), login_user=login_user)
    assert result.returncode == 0, result.stderr
    assert ['usermod', '-aG', 'docker', '--', login_user] in calls
    assert ['runuser', '-u', login_user, '--', 'docker', 'info'] in calls
    assert ['id', '-u', '--', login_user] in calls
    assert not any(call[0] == 'loginctl' or call[:2] in (['systemctl', 'start'], ['systemctl', 'restart']) for call in calls)
    assert not any('mobkit' in call or 'incorrect-local-user' in call for call in calls)
    assert calls.index(['systemctl', 'enable', '--now', 'docker']) < calls.index(['usermod', '-aG', 'docker', '--', login_user])


def test_exported_knowledge_failed_permission_probe_retries_without_replacing_user_or_service_manager(tmp_path):
    script = manual_preparation(selected_definition())
    result, calls = inert_guest(tmp_path, script=script, failure='runuser -u')
    assert result.returncode == 23
    assert calls[-1] == ['runuser', '-u', 'cloud_operator', '--', 'docker', 'info']
    original_calls = len(calls)
    retry, all_calls = inert_guest(tmp_path, script=script, reuse=True)
    assert retry.returncode == 0, retry.stderr
    retry_calls = all_calls[original_calls:]
    assert not any(call[0] == 'usermod' for call in retry_calls)
    assert ['runuser', '-u', 'cloud_operator', '--', 'docker', 'info'] in retry_calls
    assert not any(call[0] == 'loginctl' or call[:2] in (['systemctl', 'start'], ['systemctl', 'restart']) for call in all_calls)


@pytest.mark.parametrize('commands', [(), ('npx',), ('uvx',), ('npm', 'uv')])
def test_exported_granted_runtime_preparation_keeps_exact_pinned_installer(commands):
    import shlex
    from launchkit.generator import TEMPLATES
    definition = selected_definition(*commands, mode='none')
    original = (TEMPLATES / 'deploy-gcp.sh').read_text()
    rendered = bootstrap.render_manual_prerequisites(original, definition)
    assert subprocess.run(['bash', '-n'], input=rendered, text=True, capture_output=True).returncode == 0
    if not commands:
        assert rendered == original
        return
    command = manual_preparation(definition)
    setup = shlex.split(command.split(' && sudo bash -c ', 1)[1])[0]
    code = setup.split("<<'MOBKIT_PREREQUISITES'\n", 1)[1].split('\nMOBKIT_PREREQUISITES', 1)[0]
    program = ast.parse(code)
    encoded = program.body[1].value.args[0].args[0].args[0].value
    assert hashlib.sha256(zlib.decompress(base64.b64decode(encoded))).hexdigest() == RUNTIME_SHA256
    namespace = {}
    exec(compile(ast.Module(body=program.body[:2], type_ignores=[]), 'exported checked runtime', 'exec'), namespace)
    observed = []
    namespace['install'] = lambda node, uv: observed.append((node, uv))
    exec(compile(ast.Module(body=program.body[2:], type_ignores=[]), 'exported selection', 'exec'), namespace)
    assert observed == [(bool(set(commands) & {'node', 'npm', 'npx'}), bool(set(commands) & {'uv', 'uvx'}))]


def test_exported_ungranted_http_private_and_unknown_commands_leave_original_template_unchanged():
    from launchkit.generator import TEMPLATES
    definition = selected_definition('custom-command', '${PRIVATE_COMMAND}', mode='none')
    definition['mcpConnections'] += [{'id': 'unused', 'transport': 'stdio', 'command': 'npx'},
        {'id': 'remote', 'transport': 'http', 'command': 'uvx'}]
    definition['classes'][0]['tools']['mcp'].append({'connection_id': 'remote'})
    original = (TEMPLATES / 'deploy-gcp.sh').read_text()
    assert bootstrap.render_manual_prerequisites(original, definition) == original


def test_inert_guest_prepares_engine_and_user_before_lingering_without_starting_application(tmp_path):
    result, calls = inert_guest(tmp_path, delayed_user=True)
    assert result.returncode == 0, result.stderr
    positions = {name: next(index for index, call in enumerate(calls) if call[0] == name)
        for name in ("curl", "usermod", "runuser", "docker", "loginctl")}
    packages = next(index for index, call in enumerate(calls) if "docker-compose-plugin" in call)
    engine = calls.index(["systemctl", "enable", "--now", "docker"])
    user = next(index for index, call in enumerate(calls) if call == ["id", "mobkit"])
    assert positions["curl"] < packages < engine < user < positions["usermod"] < positions["runuser"]
    assert positions["runuser"] < positions["docker"] < positions["loginctl"]
    assert ["sleep", "2"] in calls
    assert ["usermod", "-aG", "docker", "mobkit"] in calls
    assert ["runuser", "-u", "mobkit", "--", "docker", "info"] in calls
    assert ["loginctl", "enable-linger", "mobkit"] in calls
    assert calls.index(["loginctl", "enable-linger", "mobkit"]) < calls.index(["systemctl", "restart", "user@1234.service"])
    repo = (tmp_path / "sources.list.d/forge-docker.list").read_text()
    assert "https://download.docker.com/linux/debian bookworm stable" in repo
    assert "signed-by=" + str(tmp_path / "keyrings/forge-docker.asc") in repo
    assert not any(call[0] == "docker" and (call[1:2] == ["run"]
        or (call[1:3] == ["compose", "up"] and "--help" not in call)) for call in calls)


def test_inert_guest_replay_does_not_restart_current_user_service_manager(tmp_path):
    result, calls = inert_guest(tmp_path, existing_group=True, manager_group=True)
    assert result.returncode == 0, result.stderr
    assert ["runuser", "-u", "mobkit", "--", "docker", "info"] in calls
    assert ["loginctl", "enable-linger", "mobkit"] in calls
    assert not any(call[0] == "usermod" or call[:2] == ["systemctl", "restart"] for call in calls)
    assert ["systemctl", "show", "--property=MainPID", "--value", "user@1234.service"] in calls


def test_inert_guest_failed_new_group_restart_retries_observed_stale_manager(tmp_path):
    result, calls = inert_guest(tmp_path, failure="systemctl restart")
    assert result.returncode == 23
    assert ["usermod", "-aG", "docker", "mobkit"] in calls
    assert ["loginctl", "enable-linger", "mobkit"] in calls
    assert calls[-1] == ["systemctl", "restart", "user@1234.service"]
    original_calls = len(calls)
    retry, all_calls = inert_guest(tmp_path, reuse=True)
    retry_calls = all_calls[original_calls:]
    assert retry.returncode == 0, retry.stderr
    assert not any(call[0] == "usermod" for call in retry_calls)
    assert ["systemctl", "restart", "user@1234.service"] in retry_calls
    assert (tmp_path / "proc/4322/status").read_text() == "Groups:\t1234 999\n"


def test_inert_guest_starts_absent_manager_and_verifies_groups(tmp_path):
    result, calls = inert_guest(tmp_path, manager_absent=True)
    assert result.returncode == 0, result.stderr
    assert ["systemctl", "start", "user@1234.service"] in calls
    assert ["systemctl", "restart", "user@1234.service"] not in calls


@pytest.mark.parametrize("failure,manager_status", [
    ("docker-gid", None), ("duplicate-docker-gid", None), ("manager-pid", None),
    ("manager-pid-changed", None), ("", "Groups:\tunknown\n"),
    ("", "Name:\tmissing-groups\n"), ("", "Groups:\t1234\nGroups:\t999\n"),
    ("manager-groups-after", None), ("manager-pid-final-changed", None)])
def test_inert_guest_unknown_or_unapplied_manager_groups_refuse_completion(tmp_path, failure, manager_status):
    result, calls = inert_guest(tmp_path, failure=failure, manager_status=manager_status)
    assert result.returncode != 0
    if failure not in ("manager-groups-after", "manager-pid-final-changed"):
        assert not any(call[:2] in (["systemctl", "start"], ["systemctl", "restart"]) for call in calls)
    else:
        assert ["systemctl", "restart", "user@1234.service"] in calls


@pytest.mark.parametrize("failure", ["apt-get install", "curl --fail", "usermod -aG",
    "runuser -u", "compose-flags"])
def test_inert_guest_dependency_or_permission_failure_stops_before_lingering(tmp_path, failure):
    result, calls = inert_guest(tmp_path, failure=failure)
    assert result.returncode != 0
    assert not any(call[0] == "loginctl" for call in calls)
    assert not any(call[0] == "docker" and call[1:2] == ["run"] for call in calls)


@pytest.mark.asyncio
async def test_new_local_knowledge_script_is_bound_to_original_package_and_cloud_receipt(tmp_path):
    from test_gcp_provisioning import setup
    service, preflight, cloud, identity, create = setup(tmp_path)
    preflight.definition = selected_definition("npx", "uvx")
    preflight.destination.update(machine_type="e2-standard-8", disk_gb=100)
    preflight.refresh_package()
    class SelectedDiskCloud(type(cloud)):
        async def command(self, args):
            result = await super().command(args)
            if args[:3] == ["compute", "instances", "create"]:
                selected = [item.removeprefix("--boot-disk-size=") for item in args
                    if item.startswith("--boot-disk-size=")]
                assert selected == [str(self.preflight.destination["disk_gb"]) + "GB"]
                assert self.disk["sizeGb"] == "30"  # The shared historical fake's default.
                self.disk["sizeGb"] = selected[0].removesuffix("GB")
            return result
    cloud = SelectedDiskCloud(preflight)
    service.command = cloud
    result = await create()
    assert result["status"] == "vm-ready"
    raw = json.loads((service._directory(preflight.guide) / (identity + ".json")).read_text())
    script = raw["bootstrap_script"]
    assert script == bootstrap.bootstrap_script(preflight.definition)
    assert raw["bootstrap_sha256"] == hashlib.sha256(script.encode()).hexdigest()
    assert {item["key"]: item["value"] for item in cloud.vm["metadata"]["items"]}["startup-script"] == script
    assert raw["binding"]["package_sha256"] == preflight.binding["package_sha256"]
    assert "docker-compose-plugin" in script and len(script.encode()) <= 8192


def guided_knowledge_harness(mode="local"):
    from launchkit.artifact_binding import proposal_hash
    from launchkit.compiler import generate_package
    from launchkit.gcp_preflight import GcpPreflightService, RestResult
    from launchkit.projects import fingerprint
    from test_gcp_preflight import Local, Cli, ZONE
    from test_gcp_guided_preflight import GuidedCloud
    local, cli, cloud = Local(), Cli(), GuidedCloud()
    proposal = local.state["proposal"]
    proposal["knowledge"]["mode"] = mode
    if mode == "local":
        proposal["gcp"].update(machine_type="e2-standard-8", disk_gb=100)
        cloud.overrides["/compute/v1/projects/" + proposal["gcp"]["project"] + "/zones/" + ZONE + "/machineTypes/e2-standard-8"] = RestResult(200,
            {"name": "e2-standard-8", "zone": ZONE, "guestCpus": 8, "memoryMb": 32768})
    local.files = {item.path: item.content.encode() for item in generate_package(proposal)}
    local.files["implementation-report.json"] = json.dumps({"build_id": local.build_id,
        "guide_version": local.state["version"], "blueprint_hash": proposal_hash(proposal),
        "brief_hash": fingerprint(local.state["brief"]), "verification": {"digest": local.digest}}).encode()
    local.package_bytes = local.archive()
    return GcpPreflightService(local, command=cli, rest=cloud), local, cli, cloud


@pytest.mark.asyncio
@pytest.mark.parametrize("mode", ["local", "none"])
async def test_guided_plan_discloses_selected_knowledge_privileges_without_claiming_readiness(mode):
    from test_gcp_preflight import ACCOUNT, PROJECT
    service, local, _, _ = guided_knowledge_harness(mode)
    result = await service.check_guided(local.state["id"], local.state["version"], local.build_id, ACCOUNT, PROJECT)
    assert result["guided_supported"] is True and result["ready_for_plan"] is True
    assert result["provisioning_verified"] is False
    assert result["binding"]["package_sha256"] == hashlib.sha256(local.package_bytes).hexdigest()
    plan = result["plan"]
    mutations = json.dumps(plan["mutations"])
    if mode == "local":
        assert "root-equivalent" in mutations and "service manager" in mutations
        assert "observed groups lack Docker access" in mutations
        assert "official Debian repository" in mutations
        assert "no knowledge service is started" in json.dumps(plan["limitations"])
        assert result["destination"]["machine_type"] == "e2-standard-8"
        assert result["destination"]["disk_gb"] == 100
    else:
        assert "root-equivalent" not in mutations and "service manager" not in mutations
        assert "A local knowledge service and additional connector executables" in json.dumps(plan["limitations"])
    assert "local_knowledge" not in result
    assert "transient-private-token" not in json.dumps(result)


@pytest.mark.asyncio
async def test_guided_knowledge_wording_cannot_use_bytes_changed_after_original_snapshot():
    from launchkit.projects import DraftConflict
    from test_gcp_preflight import ACCOUNT, PROJECT
    service, local, cli, cloud = guided_knowledge_harness()
    original = local.package
    calls = 0
    async def changing_package(identity, build_id):
        nonlocal calls
        calls += 1
        payload, slug = await original(identity, build_id)
        return (payload + b"inert changed package" if calls == 2 else payload), slug
    local.package = changing_package
    with pytest.raises(DraftConflict, match="package changed"):
        await service.check_guided(local.state["id"], local.state["version"], local.build_id, ACCOUNT, PROJECT)
    assert calls == 2
    assert cli.calls == [] and cloud.calls == []
