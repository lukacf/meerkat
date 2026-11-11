"""The long-term agent calls against a real gateway.

``test_identity_family.py`` pins the SDK's reading of the wire on a scripted
transport. These rows run the same calls through the shipping path, Python
SDK -> rpc_gateway -> Meta, so the SDK and the host methods cannot drift
apart unnoticed: the request types refuse an unknown member, so each call
that answers here is spelled as the gateway reads it.

No row makes a model call: an agent a create commits is dormant, no row
sends a first message, and no row delivers anything.

Run:
    scripts/repo-cargo build -p meerkat-meta --bin rpc_gateway
    PYTHONPATH=sdk/python python3 -m pytest sdk/python/tests/test_identity_family_real_gateway.py -q
"""
from __future__ import annotations

import contextlib
import os
import subprocess
from pathlib import Path

import pytest

from meerkat_meta import (
    AgentDirectoryPage,
    AgentPage,
    AgentRecord,
    AgentRef,
    AgentSet,
    AgentSpec,
    IDENTITY_REFUSAL_KINDS,
    IdentityChangePlan,
    IdentityChangesHold,
    IdentityDone,
    IdentityOperationUnknown,
    IdentityRefusedError,
)
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

pytestmark = [
    pytest.mark.skipif(
        not _GATEWAY_BIN or not os.path.isfile(_GATEWAY_BIN),
        reason=(
            f"Gateway binary not found at {_GATEWAY_BIN} - run: "
            "./scripts/repo-cargo build -p meerkat-meta --bin rpc_gateway"
        ),
    ),
    pytest.mark.asyncio,
    pytest.mark.timeout(120),
]

_MOB_TOML = """\
[mob]
id = "identity-family-python"

[profiles.researcher]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.researcher.tools]
comms = true

[profiles.writer]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.writer.tools]
comms = true
"""

# The host on the stdio surface may make every change a rule may allow
# without a person (an authority change always needs one).
_HOST_MAY_CHANGE_AGENTS = {
    "enabled": True,
    "approval": {
        "rules": [
            {
                "change": [
                    "create",
                    "descriptive",
                    "capability",
                    "peers",
                    "profile",
                    "archive",
                    "remove",
                    "adopt",
                ],
                "requester": {"kind": "host", "surface": "rpc"},
                "outcome": {"kind": "allow"},
            }
        ]
    },
    # Which peers an agent may list; the default is deny.
    "peers": {"default": "allow"},
}


class Roster:
    """The agents the host's configuration declares: one, ``scout``."""

    async def roster(self, context: dict) -> list[DurableAgentSpec]:
        return [
            DurableAgentSpec(
                identity="scout",
                profile="researcher",
                addressability="addressable",
                display_name="Scout",
                additional_instructions=["Find sources."],
                runtime_mode_override="turn_driven",
            )
        ]


@contextlib.asynccontextmanager
async def gateway(tmp_path: Path, policy: dict | None = _HOST_MAY_CHANGE_AGENTS):
    mob_toml = tmp_path / "mob.toml"
    mob_toml.write_text(_MOB_TOML)
    state_dir = tmp_path / "state"
    state_dir.mkdir(exist_ok=True)
    builder = (
        Meta.builder()
        .gateway(_GATEWAY_BIN)
        .mob(str(mob_toml))
        .persistent_state(str(state_dir))
        .roster(Roster())
    )
    if policy is not None:
        builder = builder.identity_creation(policy)
    runtime = await builder.build()
    try:
        yield runtime.identities
    finally:
        await runtime.shutdown()


async def _created(identities, name: str = "editor") -> str:
    """Create ``name`` and return its spec digest."""
    outcome = await identities.create(
        [AgentSpec(name=name, profile="writer", instructions=["Edit for clarity."])],
        request_id=f"create-{name}",
    )
    assert isinstance(outcome, IdentityDone), outcome
    assert [agent.name for agent in outcome.agents] == [name]
    digest = outcome.agents[0].spec_digest
    assert digest and digest.startswith("sha256:")
    return digest


async def test_list_names_every_agent_the_same_way(tmp_path):
    async with gateway(tmp_path) as identities:
        await _created(identities)
        page = await identities.list()
        assert isinstance(page, AgentPage)
        by_name = {agent.name: agent for agent in page.agents}
        assert set(by_name) == {"scout", "editor"}
        for agent in by_name.values():
            assert agent.spec_digest.startswith("sha256:")
            assert agent.archived is False
            assert isinstance(agent.readiness, str)
        assert by_name["scout"].profile == "researcher"
        assert by_name["editor"].profile == "writer"
        only = await identities.list(profile="writer")
        assert [agent.name for agent in only.agents] == ["editor"]


async def test_an_agent_without_a_display_name_has_none(tmp_path):
    """The wire leaves ``display_name`` out for an agent that has none: it
    is never the agent's name in its place."""
    async with gateway(tmp_path) as identities:
        await _created(identities)
        assert (await identities.get("editor")).display_name is None
        listed = {agent.name: agent for agent in (await identities.list()).agents}
        assert listed["editor"].display_name is None
        assert listed["scout"].display_name == "Scout"
        peers = {peer.name: peer for peer in (await identities.directory()).peers}
        assert peers["editor"].display_name is None
        assert peers["scout"].display_name == "Scout"


async def test_get_reads_one_agent_in_full(tmp_path):
    async with gateway(tmp_path) as identities:
        record = await identities.get("scout")
        assert isinstance(record, AgentRecord)
        assert record.name == "scout"
        assert record.profile == "researcher"
        assert record.display_name == "Scout"
        assert record.instructions == ("Find sources.",)
        assert record.spec_digest.startswith("sha256:")
        listed = {agent.name: agent for agent in (await identities.list()).agents}
        assert listed["scout"].spec_digest == record.spec_digest

        with pytest.raises(IdentityRefusedError) as refused:
            await identities.get("nobody")
        assert refused.value.kind == "identity_unknown"
        assert refused.value.known_kind


async def test_adopt_releases_a_configured_agent_and_replays(tmp_path):
    async with gateway(tmp_path) as identities:
        record = await identities.get("scout")

        # Declared by configuration: a run-time change is refused until it
        # is adopted.
        with pytest.raises(IdentityRefusedError) as refused:
            await identities.update(
                "scout",
                expected_spec_digest=record.spec_digest,
                request_id="update-before-adopt",
                set=AgentSet(display_name="Scout II"),
            )
        assert refused.value.kind == "owner_read_only"

        adopted = await identities.adopt(
            "scout", expected_spec_digest=record.spec_digest, request_id="adopt-scout"
        )
        assert isinstance(adopted, IdentityDone), adopted
        assert [agent.name for agent in adopted.agents] == ["scout"]
        # One spec digest on every surface; an adoption does not change it.
        assert adopted.agents[0].spec_digest == record.spec_digest
        assert (await identities.get("scout")).spec_digest == record.spec_digest

        # The same id and request return the same state.
        again = await identities.adopt(
            "scout", expected_spec_digest=record.spec_digest, request_id="adopt-scout"
        )
        assert again == adopted

        # From now on it is changed at run time.
        updated = await identities.update(
            "scout",
            expected_spec_digest=record.spec_digest,
            request_id="update-after-adopt",
            set=AgentSet(display_name="Scout II"),
        )
        assert isinstance(updated, IdentityDone), updated
        assert (await identities.get("scout")).display_name == "Scout II"


async def test_operation_reads_a_change_by_either_name(tmp_path):
    async with gateway(tmp_path) as identities:
        record = await identities.get("scout")
        adopted = await identities.adopt(
            "scout", expected_spec_digest=record.spec_digest, request_id="adopt-scout"
        )
        assert await identities.operation(request_id="adopt-scout") == adopted
        assert await identities.operation(operation_id=adopted.operation_id) == adopted

        unknown = await identities.operation(request_id="never-asked")
        assert isinstance(unknown, IdentityOperationUnknown)
        assert unknown.operation_id


async def test_create_commits_a_dormant_agent(tmp_path):
    async with gateway(tmp_path) as identities:
        outcome = await identities.create(
            [
                AgentSpec(
                    name="editor",
                    profile="writer",
                    display_name="Editor",
                    instructions=["Edit for clarity."],
                    context={"desk": "features"},
                    labels={"area": "features"},
                    peers=["scout"],
                ),
                AgentSpec(name="proofreader", profile="writer", peers=["editor"]),
            ],
            request_id="create-desk",
        )
        assert isinstance(outcome, IdentityDone), outcome
        assert [agent.name for agent in outcome.agents] == ["editor", "proofreader"]
        for agent in outcome.agents:
            assert agent.seat == "seated"
            assert agent.spec_digest.startswith("sha256:")

        record = await identities.get("editor")
        assert record.display_name == "Editor"
        assert record.instructions == ("Edit for clarity.",)
        assert record.context == {"desk": "features"}
        assert record.labels.get("area") == "features"
        assert record.peers == ("scout",)
        assert record.spec_digest == outcome.agents[0].spec_digest

        # A name is taken once.
        with pytest.raises(IdentityRefusedError) as refused:
            await identities.create(
                [AgentSpec(name="editor", profile="writer")], request_id="create-again"
            )
        assert refused.value.kind == "name_taken"
        assert refused.value.field == "agents[0].name"
        assert refused.value.details.get("by") == "identity"


async def test_update_changes_an_agent_and_says_when_instructions_apply(tmp_path):
    async with gateway(tmp_path) as identities:
        digest = await _created(identities)
        updated = await identities.update(
            "editor",
            expected_spec_digest=digest,
            request_id="update-editor",
            set=AgentSet(
                display_name="Chief editor",
                instructions=["Edit for clarity.", "Keep the author's voice."],
                labels={"area": "features"},
            ),
        )
        assert isinstance(updated, IdentityDone), updated
        agent = updated.agents[0]
        assert agent.spec_digest != digest
        assert agent.instructions is not None
        record = await identities.get("editor")
        assert record.display_name == "Chief editor"
        assert record.instructions == ("Edit for clarity.", "Keep the author's voice.")
        assert record.labels.get("area") == "features"
        assert record.spec_digest == agent.spec_digest

        # The spec the caller read is no longer the current one.
        with pytest.raises(IdentityRefusedError) as refused:
            await identities.update(
                "editor",
                expected_spec_digest=digest,
                request_id="update-stale",
                set=AgentSet(display_name="Editor"),
            )
        assert refused.value.kind == "stale_spec"
        assert refused.value.details.get("current_digest") == agent.spec_digest


async def test_archive_and_unarchive_keep_the_agent(tmp_path):
    async with gateway(tmp_path) as identities:
        digest = await _created(identities)
        with pytest.raises(IdentityRefusedError) as refused:
            await identities.unarchive(
                "editor", expected_spec_digest=digest, request_id="unarchive-early"
            )
        assert refused.value.kind == "not_archived"

        archived = await identities.archive(
            "editor", expected_spec_digest=digest, request_id="archive-editor"
        )
        assert isinstance(archived, IdentityDone), archived
        assert "editor" not in {agent.name for agent in (await identities.list()).agents}
        listed = {
            agent.name: agent
            for agent in (await identities.list(include_archived=True)).agents
        }
        assert listed["editor"].archived is True

        restored = await identities.unarchive(
            "editor", expected_spec_digest=digest, request_id="unarchive-editor"
        )
        assert isinstance(restored, IdentityDone), restored
        assert (await identities.get("editor")).archived is False


async def test_remove_retires_the_name_for_good(tmp_path):
    async with gateway(tmp_path) as identities:
        digest = await _created(identities)
        removed = await identities.remove(
            "editor", expected_spec_digest=digest, request_id="remove-editor"
        )
        assert isinstance(removed, IdentityDone), removed
        assert "editor" not in {
            agent.name for agent in (await identities.list(include_archived=True)).agents
        }
        with pytest.raises(IdentityRefusedError) as refused:
            await identities.create(
                [AgentSpec(name="editor", profile="writer")], request_id="create-reuse"
            )
        assert refused.value.kind == "name_taken"
        assert refused.value.details.get("by") == "removed"


async def test_changes_hold_refuses_every_change_until_released(tmp_path):
    async with gateway(tmp_path) as identities:
        assert await identities.changes_hold(True, reason="deploy") == IdentityChangesHold(
            held=True
        )
        with pytest.raises(IdentityRefusedError) as refused:
            await identities.create(
                [AgentSpec(name="editor", profile="writer")], request_id="create-held"
            )
        assert refused.value.kind == "changes_held"
        assert refused.value.retryable is True
        assert await identities.changes_hold(False) == IdentityChangesHold(held=False)
        # The same request, unchanged, commits once the hold is released.
        outcome = await identities.create(
            [AgentSpec(name="editor", profile="writer")], request_id="create-held"
        )
        assert isinstance(outcome, IdentityDone), outcome


async def test_withdraw_refuses_an_operation_no_decision_is_held_for(tmp_path):
    async with gateway(tmp_path) as identities:
        with pytest.raises(IdentityRefusedError) as refused:
            await identities.withdraw(request_id="never-asked")
        assert refused.value.kind == "decision_not_found"


async def test_directory_lists_what_a_change_may_name(tmp_path):
    async with gateway(tmp_path) as identities:
        directory = await identities.directory()
        assert isinstance(directory, AgentDirectoryPage)
        assert {entry.profile for entry in directory.profiles} == {"researcher", "writer"}
        assert all(entry.verdict == "allowed" for entry in directory.profiles)
        assert "scout" in {peer.name for peer in directory.peers}
        narrowed = await identities.directory(query="writ")
        assert {entry.profile for entry in narrowed.profiles} == {"writer"}


async def test_plan_answers_the_outcome_and_writes_nothing(tmp_path):
    async with gateway(tmp_path) as identities:
        record = await identities.get("scout")
        plan = await identities.plan(
            "adopt",
            AgentRef(
                name="scout",
                expected_spec_digest=record.spec_digest,
                request_id="plan-adopt",
            ),
        )
        assert isinstance(plan, IdentityChangePlan)
        assert plan.outcome == "allow"
        assert plan.change == "adopt"
        assert plan.request_digest.startswith("sha256:")
        assert plan.subject_digest.startswith("sha256:")
        assert "scout" in plan.rendering
        # Nothing was written: the operation is unknown, and the agent is
        # still declared by configuration.
        assert isinstance(
            await identities.operation(request_id="plan-adopt"), IdentityOperationUnknown
        )
        with pytest.raises(IdentityRefusedError) as refused:
            await identities.update(
                "scout",
                expected_spec_digest=record.spec_digest,
                request_id="update-after-plan",
                set=AgentSet(display_name="Scout II"),
            )
        assert refused.value.kind == "owner_read_only"


async def test_a_policy_the_gateway_cannot_read_refuses_the_launch(tmp_path):
    """The SDK sends the policy unchanged: the gateway owns its shape, and a
    launch with a policy it cannot read fails at init, naming where."""
    for policy, where in (
        # A rule names the changes it governs.
        (
            {
                "enabled": True,
                "approval": {
                    "rules": [
                        {
                            "requester": {"kind": "host", "surface": "rpc"},
                            "outcome": {"kind": "allow"},
                        }
                    ]
                },
            },
            "identity_creation.approval.rules[0].change",
        ),
        # An authority change always needs a person's decision.
        (
            {
                "enabled": True,
                "approval": {
                    "rules": [{"change": ["authority"], "outcome": {"kind": "allow"}}]
                },
            },
            "identity_creation.approval.rules[0]",
        ),
        # A member the policy does not have.
        ({"enabled": True, "custody": "store"}, "identity_creation"),
    ):
        with pytest.raises(Exception) as raised:
            async with gateway(tmp_path, policy=policy):
                pass
        failure = getattr(raised.value, "startup_error", raised.value)
        assert isinstance(failure, RpcError), repr(failure)
        assert where in str(failure), (policy, str(failure))


async def _reads_answer_and_every_change_is_denied(identities) -> None:
    """The family's reads answer; a change, and the plan of one, is refused
    ``change_denied`` and writes nothing."""
    record = await identities.get("scout")
    assert record.spec_digest.startswith("sha256:")
    assert "scout" in {agent.name for agent in (await identities.list()).agents}
    assert isinstance(await identities.directory(), AgentDirectoryPage)
    request = AgentRef(
        name="scout", expected_spec_digest=record.spec_digest, request_id="adopt-scout"
    )
    for refused_call in (
        lambda: identities.adopt(
            "scout", expected_spec_digest=record.spec_digest, request_id="adopt-scout"
        ),
        lambda: identities.plan("adopt", request),
        lambda: identities.create(
            [AgentSpec(name="editor", profile="writer")], request_id="create-editor"
        ),
    ):
        with pytest.raises(IdentityRefusedError) as refused:
            await refused_call()
        assert refused.value.kind == "change_denied"
        assert refused.value.kind in IDENTITY_REFUSAL_KINDS
        assert refused.value.retryable is False
    assert isinstance(
        await identities.operation(request_id="adopt-scout"), IdentityOperationUnknown
    )
    assert {agent.name for agent in (await identities.list()).agents} == {"scout"}


async def test_a_launch_without_the_policy_refuses_every_change(tmp_path):
    """Never method not found: on a launch that gives no policy the reads
    answer and every change is the family's refusal."""
    async with gateway(tmp_path, policy=None) as identities:
        await _reads_answer_and_every_change_is_denied(identities)


async def test_a_disabled_policy_refuses_every_change(tmp_path):
    async with gateway(tmp_path, policy={"enabled": False}) as identities:
        await _reads_answer_and_every_change_is_denied(identities)


async def test_a_policy_without_a_matching_rule_denies_the_change(tmp_path):
    """An enabled policy governs the changes its rules name; a change no
    rule matches gets the default, deny."""
    adopt_only = {
        "enabled": True,
        "approval": {
            "rules": [
                {
                    "change": ["adopt"],
                    "requester": {"kind": "host", "surface": "rpc"},
                    "outcome": {"kind": "allow"},
                }
            ]
        },
    }
    async with gateway(tmp_path, policy=adopt_only) as identities:
        record = await identities.get("scout")
        with pytest.raises(IdentityRefusedError) as refused:
            await identities.create(
                [AgentSpec(name="editor", profile="writer")], request_id="create-editor"
            )
        assert refused.value.kind == "change_denied"
        assert refused.value.details.get("change") == "create"
        adopted = await identities.adopt(
            "scout", expected_spec_digest=record.spec_digest, request_id="adopt-scout"
        )
        assert isinstance(adopted, IdentityDone), adopted


async def test_params_the_gateway_cannot_read_are_the_familys_refusal(tmp_path):
    async with gateway(tmp_path) as identities:
        runtime = identities._runtime
        digest = "sha256:" + "0" * 64
        for params, problem, field in (
            # A change names its operation.
            ({"name": "scout", "expected_spec_digest": digest}, "empty", "request_id"),
            # A member is missing.
            ({"request_id": "r", "name": "scout"}, "malformed", None),
            # A member the request does not have.
            (
                {
                    "request_id": "r",
                    "name": "scout",
                    "expected_spec_digest": digest,
                    "custody": "store",
                },
                "malformed",
                None,
            ),
        ):
            with pytest.raises(IdentityRefusedError) as refused:
                await runtime._rpc("meta/identity/adopt", params)
            assert refused.value.kind == "invalid_request", params
            assert refused.value.details.get("problem") == problem, params
            assert refused.value.field == field, params
