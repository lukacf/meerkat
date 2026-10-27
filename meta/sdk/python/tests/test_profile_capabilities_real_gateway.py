"""Real gateway profile capability tests.

These tests cover the shipping Python SDK -> rpc_gateway -> Meta/Meerkat
factory path. They intentionally assert the per-identity resolved tool surface,
not the global meta/tools/catalog.

Run:
    scripts/repo-cargo build -p meerkat-meta --bin rpc_gateway
    PYTHONPATH=sdk/python python3 -m pytest sdk/python/tests/test_profile_capabilities_real_gateway.py -q
"""
from __future__ import annotations

import os
from pathlib import Path
import subprocess

import pytest

from meerkat_meta.builder import Meta
from meerkat_meta.errors import RpcError
from meerkat_meta.identity_first_models import DurableAgentSpec


def _resolve_gateway_bin() -> str:
    override = os.environ.get("META_GATEWAY_BIN", "").strip()
    if override:
        return override
    repo_root = Path(__file__).resolve().parents[3]
    try:
        result = subprocess.run(
            [str(repo_root / "scripts" / "repo-cargo"), "--print-env", "CARGO_TARGET_DIR"],
            cwd=repo_root,
            check=True,
            capture_output=True,
            text=True,
        )
    except (OSError, subprocess.CalledProcessError):
        return ""
    for line in result.stdout.splitlines():
        if line.startswith("CARGO_TARGET_DIR="):
            target_dir = line.split("=", 1)[1].strip()
            if target_dir:
                return str(Path(target_dir) / "debug" / "rpc_gateway")
    return ""


_GATEWAY_BIN = _resolve_gateway_bin()
_BROAD_BUILTIN_TOOLS = {
    "apply_patch",
    "browse_skills",
    "task_create",
    "task_list",
}

_skip_no_binary = pytest.mark.skipif(
    not _GATEWAY_BIN or not os.path.isfile(_GATEWAY_BIN),
    reason=(
        f"Gateway binary not found at {_GATEWAY_BIN} - run: "
        "./scripts/repo-cargo build -p meerkat-meta --bin rpc_gateway"
    ),
)


_MOB_TOML = """\
[mob]
id = "profile-capabilities-python-smoke"

[profiles.domain]
model = "gpt-5.5"
external_addressable = true

[profiles.domain.tools]
comms = true
shell = false

[profiles.security]
model = "gpt-5.5"
external_addressable = true

[profiles.security.tools]
comms = true
shell = true
"""


class MutableRoster:
    def __init__(self, security_profile: str = "domain") -> None:
        self.security_profile = security_profile

    async def roster(self, context: dict) -> list[DurableAgentSpec]:
        return [
            DurableAgentSpec(
                identity="domain:plain",
                profile="domain",
                addressability="addressable",
            ),
            DurableAgentSpec(
                identity="domain:security",
                profile=self.security_profile,
                addressability="addressable",
            ),
        ]


async def _build_runtime(mob_toml: Path, state_dir: Path, roster: MutableRoster):
    return await (
        Meta.builder()
        .gateway(_GATEWAY_BIN)
        .mob(str(mob_toml))
        .persistent_state(str(state_dir))
        .roster(roster)
        .build()
    )


async def _assert_security_shell_surface(handle) -> None:
    tools = set(await handle.identity_resolved_tools("domain:security"))
    assert "shell" in tools
    assert tools.isdisjoint(_BROAD_BUILTIN_TOOLS)


_ROUTING_STATUS_REASONS = {
    "runtime_unsupported",
    "no_current_session",
    "member_lookup_failed",
    "session_not_held",
    "upstream_read_failed",
    "invalid_identity",
}


async def _assert_routing_status_is_reachable_through_a_real_gateway(handle) -> None:
    """End-to-end reachability for meta/identity/routing_status.

    Deliberately NOT asserting a particular provider: this fixture does not
    control whether the member has been addressed, and routing status is
    per-session state. What it does pin is the whole chain through a real
    gateway binary - SDK method exists, method is dispatched (not -32601),
    and the answer conforms to the contract either way.

    Both branches are correct outcomes, so both are accepted; what is NOT
    accepted is an untyped failure, which is what a caller sweeping a fleet
    cannot act on.
    """
    try:
        status = await handle.identity_routing_status("domain:security")
    except RpcError as err:
        assert err.data is not None, (
            f"routing_status must fail with typed data a caller can branch on: {err}"
        )
        assert err.data.get("kind") == "routing_status_unavailable", err.data
        assert err.data.get("reason") in _ROUTING_STATUS_REASONS, err.data
        assert err.data.get("identity") == "domain:security", err.data
        return
    assert status.identity == "domain:security"
    assert status.session_id
    # An ABSENT provider must stay None rather than becoming a string; a
    # comparison against a coerced "" would pass without reading anything.
    assert status.session_provider is None or isinstance(status.session_provider, str)


async def _assert_stale_runtime_id_rejected(
    handle,
    stale_runtime_id: str,
    stale_label: tuple[str, str],
) -> None:
    member_ids = {member.agent_identity for member in await handle.list_members()}
    assert stale_runtime_id not in member_ids

    found = await handle.find_members(stale_label[0], stale_label[1])
    assert stale_runtime_id not in {member.agent_identity for member in found}

    async def assert_stale(call):
        with pytest.raises(RpcError) as exc_info:
            await call()
        assert exc_info.value.data["kind"] == "stale_identity_runtime_binding"
        assert exc_info.value.data["live_runtime_member_id"] == stale_runtime_id

    await assert_stale(lambda: handle.get_member(stale_runtime_id))
    await assert_stale(lambda: handle.member_status(stale_runtime_id))
    await assert_stale(lambda: handle.identity_resolved_tools(stale_runtime_id))
    await assert_stale(lambda: handle.identity_routing_status(stale_runtime_id))
    await assert_stale(lambda: handle.send(stale_runtime_id, "should not deliver"))
    await assert_stale(lambda: handle.retire_member(stale_runtime_id))
    await assert_stale(lambda: handle.respawn_member(stale_runtime_id))
    await assert_stale(lambda: handle.force_cancel_member(stale_runtime_id))


@_skip_no_binary
@pytest.mark.asyncio
@pytest.mark.timeout(90)
async def test_real_gateway_reset_reprofile_materializes_shell_tools(tmp_path):
    mob_toml = tmp_path / "mob.toml"
    mob_toml.write_text(_MOB_TOML)
    state_dir = tmp_path / "state"
    state_dir.mkdir()
    roster = MutableRoster(security_profile="domain")

    runtime = await _build_runtime(mob_toml, state_dir, roster)
    try:
        handle = runtime.mob_handle()

        await runtime.reset("domain:security")
        domain_status = await runtime.status("domain:security")
        assert domain_status.profile == "domain"
        old_runtime_id = domain_status.agent_runtime_id
        assert old_runtime_id
        old_member = await handle.get_member(old_runtime_id)
        stale_label = next(iter(old_member.labels.items()), None)
        assert stale_label is not None
        assert "shell" not in await handle.identity_resolved_tools("domain:security")

        roster.security_profile = "security"
        await runtime.reset("domain:security")

        security_status = await runtime.status("domain:security")
        assert security_status.profile == "security"
        assert security_status.agent_runtime_id != old_runtime_id
        await _assert_stale_runtime_id_rejected(handle, old_runtime_id, stale_label)
        await _assert_security_shell_surface(handle)
        await _assert_routing_status_is_reachable_through_a_real_gateway(handle)

        await runtime.shutdown()
        runtime = await _build_runtime(mob_toml, state_dir, roster)
        handle = runtime.mob_handle()
        resumed_status = await runtime.status("domain:security")
        assert resumed_status.profile == "security"
        await _assert_security_shell_surface(handle)
        # Read it again AFTER a full gateway restart: this is the shape a fleet
        # sweep actually runs, and a method that only works pre-restart would
        # pass the call above and still be useless post-resume.
        await _assert_routing_status_is_reachable_through_a_real_gateway(handle)

        await runtime.reset("domain:plain")
        assert "shell" not in await handle.identity_resolved_tools("domain:plain")
    finally:
        await runtime.shutdown()


_SCHEDULE_MOB_TOML = """\
[mob]
id = "schedule-tools-python-smoke"

[profiles.scheduler]
model = "gpt-5.5"
external_addressable = true

[profiles.scheduler.tools]
comms = true
schedule = true

[profiles.plain]
model = "gpt-5.5"
external_addressable = true

[profiles.plain.tools]
comms = true
"""


class _ScheduleRoster:
    async def roster(self, context: dict) -> list[DurableAgentSpec]:
        return [
            DurableAgentSpec(
                identity="ops:scheduler",
                profile="scheduler",
                addressability="addressable",
            ),
            DurableAgentSpec(
                identity="ops:plain",
                profile="plain",
                addressability="addressable",
            ),
        ]


def _schedule_tool_names(tools) -> set[str]:
    return {name for name in tools if name.startswith("meerkat_schedule_")}


@_skip_no_binary
@pytest.mark.asyncio
@pytest.mark.timeout(90)
async def test_real_gateway_schedule_tools_name_a_schedule_granted_member_surface(tmp_path):
    """The listing names exactly the schedule tools a granted member resolves."""
    mob_toml = tmp_path / "mob.toml"
    mob_toml.write_text(_SCHEDULE_MOB_TOML)
    state_dir = tmp_path / "state"
    state_dir.mkdir()

    runtime = await _build_runtime(mob_toml, state_dir, _ScheduleRoster())
    try:
        handle = runtime.mob_handle()
        listed = {tool.name for tool in (await handle.schedule_tools()).tools}
        assert listed
        assert _schedule_tool_names(listed) == listed

        await runtime.reset("ops:scheduler")
        await runtime.reset("ops:plain")
        granted = set(await handle.identity_resolved_tools("ops:scheduler"))
        plain = set(await handle.identity_resolved_tools("ops:plain"))
        assert _schedule_tool_names(granted) == listed
        assert _schedule_tool_names(plain) == set()
    finally:
        await runtime.shutdown()
