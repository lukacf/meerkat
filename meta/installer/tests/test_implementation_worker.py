import asyncio
from copy import deepcopy
import inspect
import json
from pathlib import Path
from types import SimpleNamespace

import pytest

from launchkit.acceptance import SemanticAcceptance
from launchkit.acceptance_reviewer import AcceptanceTools
from launchkit.blueprint import starter_blueprint
from launchkit.implementation import read_receipt
from launchkit.implementation_worker import (BuildTools, NativeInteractionFailure,
                                            implementation_inspection_context, interaction_completed,
                                            native_turn, review_and_repair)
from launchkit.implementation_workspace import ImplementationWorkspace
from launchkit.templates.storage_setup import write_private
import launchkit.implementation_worker as worker
import launchkit.implementation_workspace as workspace_module
from test_meta_toolkit_runtime_selection import product as reference_product


@pytest.fixture
def team(tmp_path):
    frozen = {"blueprint": starter_blueprint().model_dump(mode="json"),
              "brief": {"requirements": [{"id": "normalize-input", "outcome": "Normalize mixed-case input and surrounding spaces", "disposition": "open"}]}}
    write_private(tmp_path / "input.json", frozen)
    write_private(tmp_path / "receipt.json", {"status": "running", "progress": [], "ready_to_package": False})
    workspace = ImplementationWorkspace(tmp_path / "workspace", frozen["blueprint"])
    workspace.seed()
    workspace.write_file("application/__init__.py", "class Integration:\n    pass\ndef build(context):\n    return Integration()\n")
    workspace.write_file("application/feature.py", "def normalize(value):\n    return value.strip()\n")
    # The builder's own tests miss the mixed-case requirement.
    workspace.write_file("tests/test_feature.py", "import unittest\nfrom application.feature import normalize\nclass Feature(unittest.TestCase):\n    def test_already_lowercase(self):\n        self.assertEqual(normalize('  hello  '), 'hello')\n")
    tools = BuildTools(tmp_path)
    acceptance = SemanticAcceptance(tmp_path / "acceptance", tools.workspace, frozen)
    tools.acceptance = acceptance
    reviewer = AcceptanceTools(tmp_path, acceptance, tools.reference)
    return tools, reviewer


def require_sandbox():
    if not workspace_module._sandbox_executable():
        pytest.skip("Native authoring checks have no unsandboxed execution fallback.")


def builder_check_fixture(tools):
    with tools.workspace._locked():
        state = tools.workspace._state()
        receipt = {"status": "pass", "tests_run": 1, "ready_to_package": True,
                   "digest": tools.workspace._digest(tools.workspace._files(state)), "checks": []}
        receipt["signature"] = tools.workspace._signed(receipt)
        state["last_check"] = receipt
        tools.workspace._save_state(state)
    tools.publication = {"summary": "Builder fixture", "unresolved": [], "checked_digest": receipt["digest"], "receipt": receipt}


def continuation_context():
    blueprint = {"classes": [{"id": "owner", "instructions": "Preserve every instruction."}],
                 "members": [{"identity": "owner:main", "instructions": "Distinct member detail."}]}
    review = {"summary": "Repair the mixed-case path.", "requirements": [{"id": "normalize-input",
              "evidence": "The preserved test covers whitespace and mixed case."}],
              "counterexamples": ["  MiXeD  "], "remaining_dependencies": ["Live credentials remain unavailable."]}
    return {"blueprint": blueprint, "independent_feedback": review,
            "brief": {"requirements": [{"id": "normalize-input", "outcome": "Normalize mixed case"}]},
            "messages": [{"role": "user", "content": "Keep multiple input channels."}],
            "implementation_requests": [{"request": "First request"}, {"request": "Keep the whole history."}],
            "request": "Repair without cancelling the other channel.",
            "previous_implementation": {"build_id": "previous-build", "guide_version": 3,
                "blueprint": deepcopy(blueprint), "independent_review": deepcopy(review),
                "request": "Distinct previous request", "status": "failed", "phase": "independent_review",
                "summary": "Distinct previous summary", "unresolved": ["Distinct previous gap"],
                "retained_acceptance_tests": [{"path": "test_normalize.py", "sha256": "a" * 64}],
                "extra_previous_fact": {"instructions": "A loose previous field also survives."}}}


def reconstruct_inspection(context):
    restored = deepcopy(context)
    references = restored["previous_implementation"].pop("identical_context_fields", {})
    for field, pointer in references.items():
        # References are absolute root JSON pointers, not ambiguous field aliases.
        assert pointer in {"/blueprint", "/independent_feedback"}
        restored["previous_implementation"][field] = deepcopy(restored[pointer[1:]])
    return restored


def test_inspection_dedup_is_lossless_nonmutating_and_preserves_all_distinct_scope():
    original = continuation_context()
    frozen = deepcopy(original)
    overview = implementation_inspection_context(original)
    assert overview["previous_implementation"]["identical_context_fields"] == {
        "blueprint": "/blueprint", "independent_review": "/independent_feedback"}
    assert "blueprint" not in overview["previous_implementation"]
    assert "independent_review" not in overview["previous_implementation"]
    assert reconstruct_inspection(overview) == original == frozen
    overview["blueprint"]["members"][0]["instructions"] = "Changed returned object"
    overview["messages"].clear()
    overview["previous_implementation"]["extra_previous_fact"]["instructions"] = "Changed returned object"
    assert original == frozen


def test_inspection_keeps_changed_previous_design_and_repair_findings():
    original = continuation_context()
    original["previous_implementation"]["blueprint"]["members"][0]["instructions"] = "Earlier distinct instruction"
    original["independent_feedback"]["counterexamples"].append("A newer repair failure")
    assert implementation_inspection_context(original) == original
    # A repeated design does not erase a different earlier assessment.
    original["previous_implementation"]["blueprint"] = deepcopy(original["blueprint"])
    overview = implementation_inspection_context(original)
    assert overview["previous_implementation"]["identical_context_fields"] == {"blueprint": "/blueprint"}
    assert overview["previous_implementation"]["independent_review"] == original["previous_implementation"]["independent_review"]
    assert reconstruct_inspection(overview) == original


@pytest.mark.parametrize("old,current", [(True, 1), (1, 1.0), ([1], [True]),
    ({"count": 1}, {"count": True}), (None, {}), ("one", "One")])
def test_inspection_only_references_exact_json_values(old, current):
    original = {"blueprint": current, "previous_implementation": {"blueprint": old}}
    assert implementation_inspection_context(original) == original


def test_inspection_dict_order_is_not_a_distinct_fact_and_existing_metadata_is_retained():
    original = {"blueprint": {"first": 1, "second": 2},
                "previous_implementation": {"blueprint": {"second": 2, "first": 1}}}
    assert reconstruct_inspection(implementation_inspection_context(original)) == original
    original["previous_implementation"]["identical_context_fields"] = {"authored": "Keep this loose prior field"}
    assert implementation_inspection_context(original) == original


@pytest.mark.asyncio
async def test_registered_inspection_full_access_preserves_persisted_input_source_and_feedback(team):
    from jsonschema import validate
    from meerkat_meta.models import SessionBuildOptions

    tools, _ = team
    extra = continuation_context()
    extra["blueprint"] = deepcopy(tools.input["blueprint"])
    extra["previous_implementation"]["blueprint"] = deepcopy(extra["blueprint"])
    write_private(tools.directory / "input.json", extra)
    tools = BuildTools(tools.directory)
    input_bytes = (tools.directory / "input.json").read_bytes()
    source_digest = tools.workspace.digest()
    input_value = deepcopy(tools.input)
    feedback_value = deepcopy(tools.independent_feedback)
    draft = SessionBuildOptions()
    tools.register(draft)
    wire = next(row for row in draft.to_dict()["tools"] if row["name"] == "inspect_implementation")
    callback = draft.tool_handlers["inspect_implementation"]
    for args in ({}, {"view": None}, {"view": "overview"}, {"view": "full"}):
        validate(args, wire["input_schema"])
    assert "root JSON pointers" in wire["description"] and "view='full'" in wire["description"]
    overview = await callback({"view": None})
    full = await callback({"view": "full"})
    assert reconstruct_inspection(overview) == full
    for key, value in input_value.items():
        assert full[key] == value
    assert full["independent_feedback"] == feedback_value
    # A later repair review is current feedback; the distinct original remains readable.
    tools.independent_feedback = {"summary": "A new independent counterexample", "counterexamples": ["NEW"]}
    later = await callback({})
    assert later["previous_implementation"]["independent_review"] == feedback_value
    assert later["independent_feedback"] == tools.independent_feedback
    assert later["previous_implementation"]["identical_context_fields"] == {"blueprint": "/blueprint"}
    full["previous_implementation"]["independent_review"]["summary"] = "Mutated returned object"
    overview["brief"]["requirements"].clear()
    assert tools.input == input_value
    assert (tools.directory / "input.json").read_bytes() == input_bytes
    assert tools.workspace.digest() == source_digest


@pytest.mark.asyncio
async def test_registered_exact_edit_uses_read_hash_and_invalidates_only_successful_source_changes(team):
    from jsonschema import ValidationError, validate
    from meerkat_meta.models import SessionBuildOptions

    tools, _ = team
    draft = SessionBuildOptions()
    tools.register(draft)
    wire = next(row for row in draft.to_dict()["tools"] if row["name"] == "edit_project_file")
    callback = draft.tool_handlers["edit_project_file"]
    source = await draft.tool_handlers["read_project_file"]({"path": "application/feature.py"})
    builder_check_fixture(tools)
    publication = deepcopy(tools.publication)
    state = tools.workspace._state_path.read_bytes()
    receipt = (tools.directory / "receipt.json").read_bytes()
    args = {"path": source["path"], "expected_sha256": source["sha256"],
            "old_text": "value.strip()", "new_text": "value.strip().lower()"}
    validate(args, wire["input_schema"])
    for bad in ({**args, "expected_sha256": "0" * 64}, {**args, "old_text": "missing"}):
        result = await callback(bad)
        assert not result["ok"]
        assert tools.publication == publication
        assert tools.workspace._state_path.read_bytes() == state
        assert (tools.directory / "receipt.json").read_bytes() == receipt
    for key in args:
        with pytest.raises(ValidationError):
            validate({k: v for k, v in args.items() if k != key}, wire["input_schema"])
    result = await callback(args)
    assert result["ok"] and "Run checks again" in result["check_receipt"]
    assert tools.workspace.read_file(source["path"]) == source["content"].replace(args["old_text"], args["new_text"])
    assert tools.publication is None and tools.workspace.verification() is None
    assert result["file"]["digest"] == tools.workspace.digest()
    host_receipt = read_receipt(tools.directory)
    assert host_receipt["ready_to_package"] is False
    assert next(row for row in host_receipt["files"] if row["path"] == source["path"])["sha256"] == result["file"]["sha256"]
    assert draft.tool_handlers["write_project_file"] is not None


@pytest.mark.asyncio
@pytest.mark.parametrize("args", [{"view": "other"}, {"view": False}, {"view": []},
                                      {"view": "full", "unexpected": "private input"}])
async def test_inspection_rejects_unsupported_views_without_echoing_arguments(team, args):
    tools, _ = team
    before = (tools.directory / "input.json").read_bytes()
    result = await tools.context(args)
    assert result == {"ok": False, "error": "Choose the overview or full implementation context view."}
    assert (tools.directory / "input.json").read_bytes() == before


@pytest.mark.asyncio
@pytest.mark.parametrize("agent_index", (0, 1))
async def test_messaging_reference_exposes_exact_installed_admission_and_observation_contract(team, agent_index):
    from meerkat_meta.errors import RpcError
    from meerkat_meta.identity_first_models import (CompletionCursor, CompletionProgress,
                                                     DispatchInput, DispatchResult,
                                                     IdentityInspection, SendResult)
    from meerkat_meta.runtime import IdentityAgentHandle, MetaRuntime

    agent = team[agent_index]
    offset = 0
    lines = []
    while True:
        page = await agent.reference({"name": "messaging", "offset": offset, "limit": 41})
        numbered = page["content"].splitlines()
        assert len(numbered) <= 41
        for index, line in enumerate(numbered, offset + 1):
            number, source = line.split(": ", 1)
            assert int(number) == index
            lines.append(source)
        if page["next_offset"] is None:
            assert len(lines) == page["total_lines"]
            break
        offset = page["next_offset"]
    reference = "\n".join(lines)
    for contract in (DispatchInput, DispatchResult, SendResult, CompletionCursor,
                     CompletionProgress, IdentityInspection, RpcError,
                     MetaRuntime.dispatch, MetaRuntime.dispatch_text,
                     MetaRuntime.inspect_identity, IdentityAgentHandle.inspect,
                     IdentityAgentHandle.dispatch_and_wait,
                     IdentityAgentHandle.dispatch_text_and_wait,
                     IdentityAgentHandle._wait_for_admission,
                     IdentityAgentHandle.wait_for_completion):
        assert inspect.getsource(contract).rstrip() in reference


@pytest.mark.asyncio
@pytest.mark.parametrize("agent_index", (0, 1))
async def test_roster_reference_is_registered_and_pages_exact_installed_provider_contracts(team, agent_index):
    from jsonschema import validate
    from meerkat_meta.builder import MetaBuilder
    from meerkat_meta.identity_first_models import (AgentBuildContext, AgentBuildDraft,
                                                     DurableAgentSpec, ManagedPeerEdge,
                                                     RosterContext)
    from meerkat_meta.identity_first_providers import (AgentCustomizerProtocol,
                                                        RosterProviderProtocol,
                                                        TopologyProviderProtocol)
    from meerkat_meta.models import SessionBuildOptions
    from meerkat_meta.runtime import MetaRuntime

    agent = team[agent_index]
    draft = SessionBuildOptions()
    agent.register(draft)
    registered = next(tool for tool in draft.to_dict()["tools"]
                      if tool["name"] == "read_runtime_reference")
    callback = draft.tool_handlers["read_runtime_reference"]
    offset, lines = 0, []
    while True:
        args = {"name": "roster", "offset": offset, "limit": 37}
        validate(args, registered["input_schema"])
        page = await callback(args)
        assert page["name"] == "roster" and page["offset"] == offset
        numbered = page["content"].splitlines()
        assert len(numbered) <= 37
        for number, line in enumerate(numbered, offset + 1):
            actual_number, source = line.split(": ", 1)
            assert int(actual_number) == number
            lines.append(source)
        if page["next_offset"] is None:
            assert len(lines) == page["total_lines"]
            break
        assert page["next_offset"] == offset + 37
        offset = page["next_offset"]
    reference = "\n".join(lines)
    for contract in (RosterProviderProtocol, TopologyProviderProtocol,
                     AgentCustomizerProtocol, DurableAgentSpec, ManagedPeerEdge,
                     RosterContext, AgentBuildContext, AgentBuildDraft,
                     MetaBuilder.roster, MetaBuilder.topology_provider,
                     MetaBuilder.agent_customizer, MetaRuntime.reconcile):
        assert inspect.getsource(contract).rstrip() in reference


@pytest.mark.asyncio
async def test_both_native_agents_receive_selected_recurrence_guidance_and_installed_callbacks(team, monkeypatch):
    from importlib import metadata
    from meerkat_meta.builder import MetaBuilder
    from meerkat_meta.runtime import MetaRuntime
    from launchkit.native_schedule_reference import selected_recurrence_guidance

    tools, reviewer = team
    monkeypatch.setattr(metadata, "version", lambda name: "0.9.43")
    results = []
    for agent in (tools, reviewer):
        offset, lines = 0, []
        while True:
            page = await agent.reference({"name": "schedule", "offset": offset, "limit": 47})
            assert page.get("ok") is not False
            assert page["installed_sdk_version"] == metadata.version("meerkat-meta")
            assert "No bundled contract matches" in page["native_authoring_schema"]
            assert "reference_version" not in page and "version_matches" not in page
            assert {item["path"] for item in page["source_files"]} == {"meerkat_meta/runtime.py", "meerkat_meta/builder.py"}
            numbered = page["content"].splitlines()
            assert len(numbered) <= 47
            for index, line in enumerate(numbered, offset + 1):
                number, source = line.split(": ", 1)
                assert int(number) == index
                lines.append(source)
            if page["next_offset"] is None:
                assert len(lines) == page["total_lines"]
                break
            offset = page["next_offset"]
        content = "\n".join(lines)
        assert selected_recurrence_guidance() in content
        assert "Pinned native input schemas:" not in content
        for contract in (MetaRuntime.on_schedule_fire, MetaBuilder.host_runnables):
            assert inspect.getsource(contract).rstrip() in content
        assert len(content.encode()) < 64 * 1024
        results.append(content)
    assert results[0] == results[1]


@pytest.mark.asyncio
async def test_schedule_reference_uses_host_metadata_and_never_borrows_legacy_schema(team, monkeypatch):
    import builtins

    tools, reviewer = team
    original_import = builtins.__import__
    def no_legacy_import(name, *args, **kwargs):
        if name.split(".")[0] == "meerkat_mobkit":
            raise AssertionError("Current Meta references must not import the predecessor SDK.")
        return original_import(name, *args, **kwargs)
    monkeypatch.setattr(builtins, "__import__", no_legacy_import)
    def host_version(name):
        assert name == "meerkat-meta"
        return "0.9.43"
    monkeypatch.setattr(worker.metadata, "version", host_version)
    for agent in (tools, reviewer):
        result = await agent.reference({"name": "schedule", "installed_sdk_version": "0.8.42", "limit": 250})
        assert result.get("ok") is not False
        assert result["installed_sdk_version"] == "0.9.43"
        assert "MetaRuntime.on_schedule_fire" in result["content"]
        assert "Pinned native input schemas:" not in result["content"]
        assert "reference_version" not in result and "version_matches" not in result
        lookup = await agent.reference({"name": "schedule", "query": "native authoring schema"})
        assert lookup["installed_sdk_version"] == "0.9.43"
        assert lookup["unavailable_references"] == []
        assert lookup["matches"][0]["source_file"] == "launchkit/native_schedule_reference.py#selected_recurrence_guidance"
    def missing(name):
        raise worker.metadata.PackageNotFoundError(name)
    monkeypatch.setattr(worker.metadata, "version", missing)
    result = await reviewer.reference({"name": "schedule", "limit": 250})
    assert result["installed_sdk_version"] is None
    assert "MetaRuntime.on_schedule_fire" in result["content"]
    assert "Pinned native input schemas:" not in result["content"]


@pytest.fixture
def selected_reference_team(reference_product, tmp_path, request):
    """Real protected product workspace and complete inert wheels, never SDK code."""
    import hashlib
    from dataclasses import replace
    from test_meta_toolkit_runtime_selection import changed_assembly, json_bytes, put
    from test_meta_toolkit_runtime_update import catalog_target

    f = reference_product
    if getattr(request, "param", None) == "sdk-0.9.4":
        from test_sdk_wheel_integrity import InertWheel
        wheels = (InertWheel("meerkat-meta", version="0.9.4"),
                  InertWheel("meerkat-toolkit", version="0.9.4", meta_version="0.9.4"))
        for new, old in zip(wheels, f.wheels):
            new.payload = dict(old.payload)
        f.wheels = wheels
    poison = "raise RuntimeError('selected SDK source must never import or execute')\n"
    meta_sources = {
        "runtime.py": "class MetaRuntime:\n    def agent(self): return 'selected-agent'\n    def dispatch(self): return 'selected-dispatch'\n    def dispatch_text(self): return 'selected-text'\n    def inspect_identity(self): return 'selected-inspection'\n    def on_schedule_fire(self): return 'selected-schedule'\n    def mob_handle(self): return 'selected-flow'\n    def reconcile(self): return 'selected-roster'\nclass IdentityAgentHandle:\n    def send_and_wait(self): return 'selected-send'\n    def inspect(self): return 'selected-handle'\nclass MobHandle:\n    def run_flow(self): return 'selected-run'\n    def flow_status(self): return 'selected-status'\n",
        "builder.py": "class MetaBuilder:\n    def host_runnables(self): return 'selected-runnables'\n    def roster(self): return 'selected-roster-provider'\n    def topology_provider(self): return 'selected-topology'\n    def agent_customizer(self): return 'selected-customizer'\n",
        "identity_first_models.py": "class DurableAgentSpec: pass\nclass ManagedPeerEdge: pass\nclass RosterContext: pass\nclass AgentBuildContext: pass\nclass AgentBuildDraft: pass\nclass DispatchInput: pass\nclass DispatchResult: pass\nclass SendResult: pass\nclass CompletionCursor: pass\nclass CompletionProgress: pass\nclass IdentityInspection: pass\n",
        "identity_first_providers.py": "class RosterProviderProtocol: pass\nclass TopologyProviderProtocol: pass\nclass AgentCustomizerProtocol: pass\n",
        "types.py": "class MemberSnapshot: pass\n",
        "errors.py": "class RpcError: pass\n",
    }
    if getattr(request, "param", None) == "empty-flows":
        meta_sources["runtime.py"] = "class MetaRuntime:\n    pass\n"
    for name, code in meta_sources.items():
        f.wheels[0].payload["meerkat_meta/" + name] = (poison + code).encode()
    f.wheels[1].payload["meerkat_toolkit/host_control.py"] = (
        poison + "class ToolkitMobKitRuntime:\n    def __init__(self): self.source = 'selected-toolkit-restore'\n").encode()
    f.wheels[1].payload["meerkat_toolkit/application_runtime.py"] = (
        poison + "class ToolkitApplicationRuntime:\n    def connect(self): return 'selected-toolkit-fresh'\n").encode()
    if getattr(request, "param", None) == "native-contracts":
        f.wheels[1].payload["meerkat_toolkit/host_control.py"] += (
            "@dataclass(frozen=True)\nclass PrivateAuthenticatedEvent:\n    source: str\n"
            "class PrivateHostClient:\n    def authenticate(self, event: PrivateAuthenticatedEvent): pass\n").encode()
        f.wheels[1].payload["meerkat_toolkit/member_classes.py"] = (
            poison + "from dataclasses import dataclass\n"
            "MEMBER_CLASS_STATE = str\n"
            "@dataclass(frozen=True)\nclass MemberClassAddRequest:\n    identity: str\n"
            "class MemberClassClient:\n    async def begin_add(self, request: MemberClassAddRequest): pass\n"
            "class MemberClassAttempt:\n    async def join_listing(self): pass\n").encode()
        f.wheels[1].payload["meerkat_toolkit/_unadvertised.py"] = (
            poison + "UNADVERTISED_TOOLKIT_SENTINEL = 'not a reference'\n").encode()
    meta_bytes, meta = f.wheels[0].selected()
    toolkit_bytes, toolkit = f.wheels[1].selected()
    f.target = replace(f.target, meta_wheel=put(f.target.meta_wheel.path, meta_bytes), meta=meta,
        toolkit_wheel=put(f.target.toolkit_wheel.path, toolkit_bytes), toolkit=toolkit)
    payload = {**f.wheels[0].payload, **f.wheels[1].payload}
    for row in f.report["sdk_sources"]:
        name = "/".join(Path(row["path"]).parts[-2:])
        row["sha256"] = hashlib.sha256(payload[name]).hexdigest()
    verification = put(f.final / "gateway-verification.json", json_bytes(f.report))
    f.assembly["qualification"]["installed_gateway_verification_sha256"] = verification.sha256
    f.target = changed_assembly(f)
    target = catalog_target(f)
    directory = tmp_path / "selected-reference-build"
    directory.mkdir(mode=0o700)
    blueprint = starter_blueprint().model_dump(mode="json")
    blueprint["target"] = "mac"
    for item in blueprint["agent_classes"]:
        item["model"] = f.catalog["models"][0]["id"]
        item["provider"] = f.catalog["models"][0]["provider"]
    workspace = ImplementationWorkspace(directory / "workspace", blueprint)
    workspace.seed(runtime_target=target)
    framework = workspace.framework_provenance()
    frozen = {"blueprint": blueprint, "framework": framework,
        "runtime_candidate": framework["runtime_candidate"]}
    write_private(directory / "input.json", frozen)
    write_private(directory / "receipt.json", {"status": "running", "progress": []})
    tools = BuildTools(directory)
    acceptance = SemanticAcceptance(directory / "acceptance", tools.workspace, frozen)
    reviewer = AcceptanceTools(directory, acceptance, tools.reference)
    return SimpleNamespace(tools=tools, reviewer=reviewer, product=f, payload=payload, frozen=frozen)


@pytest.mark.asyncio
@pytest.mark.parametrize("environment", ["legacy", "missing", "foreign"])
async def test_selected_runtime_references_use_original_wheels_not_forge_sdk(
        selected_reference_team, monkeypatch, environment):
    import builtins
    import hashlib

    f = selected_reference_team
    calls = []
    def environment_version(name):
        calls.append(name)
        if environment == "missing":
            raise worker.metadata.PackageNotFoundError(name)
        return "0.8.42" if environment == "legacy" else "9.9.9"
    monkeypatch.setattr(worker.metadata, "version", environment_version)
    original_import = builtins.__import__
    def no_sdk_import(name, *args, **kwargs):
        if name.split(".")[0] in {"meerkat_meta", "meerkat_toolkit", "meerkat_mobkit"}:
            pytest.fail("Selected reference source may never import an SDK or substitute Forge's SDK.")
        return original_import(name, *args, **kwargs)
    monkeypatch.setattr(builtins, "__import__", no_sdk_import)
    for agent in (f.tools, f.reviewer):
        for name in ("runtime", "builder", "identity", "types", "roster", "messaging", "flows", "schedule"):
            page = await agent.reference({"name": name, "limit": 250})
            assert page.get("ok") is not False
            assert page["selected_sdk_versions"] == {"meerkat-meta": "0.9.0", "meerkat-toolkit": "0.9.0"}
            assert page["installed_sdk_version"] is None and not page["observed_destination"]
            assert "0.8.42" not in page["content"] and "MobKitRuntime" not in page["content"].replace("ToolkitMobKitRuntime", "")
            for source in page["source_files"]:
                assert source["sha256"] == hashlib.sha256(f.payload[source["path"]]).hexdigest()
        schedule = await agent.reference({"name": "schedule", "limit": 250})
        assert "MetaRuntime.on_schedule_fire" in schedule["content"]
        assert "MetaBuilder.host_runnables" in schedule["content"]
        assert "ToolkitApplicationRuntime" in schedule["content"] and "ToolkitMobKitRuntime" in schedule["content"]
        assert "Retain the user's continuing job" in schedule["content"]
        assert "Native authoring schema:" in schedule["content"]
        assert "meerkat_schedule_create:" not in schedule["content"]
    assert calls == []


@pytest.mark.asyncio
@pytest.mark.parametrize("selected_reference_team", ["sdk-0.9.4"], indirect=True)
async def test_selected_094_schedule_reference_reaches_both_agents_without_host_sdk(selected_reference_team, monkeypatch):
    import builtins

    f = selected_reference_team
    def no_host_version(name):
        pytest.fail("A selected SDK must not use Forge's installed version.")
    monkeypatch.setattr(worker.metadata, "version", no_host_version)
    original_import = builtins.__import__
    def no_sdk_import(name, *args, **kwargs):
        if name.split(".")[0] in {"meerkat_meta", "meerkat_toolkit", "meerkat_mobkit"}:
            pytest.fail("Reading a selected SDK must not execute it.")
        return original_import(name, *args, **kwargs)
    monkeypatch.setattr(builtins, "__import__", no_sdk_import)
    for agent in (f.tools, f.reviewer):
        page = await agent.reference({"name": "schedule", "limit": 250,
                                      "installed_sdk_version": "0.8.42"})
        assert page.get("ok") is not False
        assert page["selected_sdk_versions"]["meerkat-meta"] == "0.9.4"
        assert page["installed_sdk_version"] is None and not page["observed_destination"]
        assert page["schedule_reference_version"] == "0.9.4"
        assert "meerkat_schedule_create:" in page["content"]
        assert '"host_runnable"' in page["content"]
        assert "selected-schedule" in page["content"]
        assert "0.8.42" not in page["content"]
        lookup = await agent.reference({"name": "schedule", "query": "meerkat_schedule_create:"})
        assert lookup["matches"] and not lookup["unavailable_references"]
        hit = lookup["matches"][0]
        assert hit["source_file"] == "launchkit/meta_schedule_reference_0_9_4.json#rendered_contract"
        line = await agent.reference({"name": "schedule", "offset": hit["offset"], "limit": 1})
        assert line["content"] == str(hit["line"]) + ": " + hit["snippet"]


@pytest.mark.asyncio
async def test_ordinary_094_schedule_reference_uses_host_version_not_caller_version(team, monkeypatch):
    monkeypatch.setattr(worker.metadata, "version", lambda name: "0.9.4")
    for agent in team:
        page = await agent.reference({"name": "schedule", "limit": 250,
                                      "installed_sdk_version": "0.9.0"})
        assert page["schedule_reference_version"] == page["installed_sdk_version"] == "0.9.4"
        assert "meerkat_schedule_create:" in page["content"]
        assert not page["observed_destination"]


@pytest.mark.asyncio
async def test_selected_reference_search_and_pages_keep_exact_sdk_class_origins(selected_reference_team):
    import hashlib
    f = selected_reference_team
    result = await f.tools.reference({"query": "selected-toolkit", "limit": 40})
    assert not result["unavailable_references"] and result["matches"]
    for hit in result["matches"]:
        data = f.payload[hit["source_file"]]
        assert hit["source_sha256"] == hashlib.sha256(data).hexdigest()
        assert hit["snippet"] == data.decode().splitlines()[hit["source_line"] - 1]
        page = await f.tools.reference({"name": hit["name"], "offset": hit["offset"], "limit": 1})
        assert page["content"] == str(hit["line"]) + ": " + hit["snippet"]
    lines, offset = [], 0
    while True:
        page = await f.reviewer.reference({"name": "schedule", "offset": offset, "limit": 7})
        assert len(page["content"].splitlines()) <= 7
        lines.extend(line.split(": ", 1)[1] for line in page["content"].splitlines())
        if page["next_offset"] is None:
            assert len(lines) == page["total_lines"]
            break
        offset = page["next_offset"]
    assert "MetaRuntime.on_schedule_fire" in "\n".join(lines)


@pytest.mark.asyncio
@pytest.mark.parametrize("selected_reference_team", ["empty-flows"], indirect=True)
async def test_selected_reference_empty_meta_extract_keeps_exact_toolkit_origins(selected_reference_team):
    import hashlib
    f = selected_reference_team
    for agent in (f.tools, f.reviewer):
        page = await agent.reference({"name": "flows", "limit": 250})
        assert page["content"].splitlines()[0] == "1: # meerkat_toolkit/host_control.py"
        assert "meerkat_meta/runtime.py:" not in page["content"]
        result = await agent.reference({"name": "flows", "query": "selected-toolkit", "limit": 40})
        assert not result["unavailable_references"]
        assert len(result["matches"]) == 2
        for hit in result["matches"]:
            data = f.payload[hit["source_file"]]
            assert hit["source_sha256"] == hashlib.sha256(data).hexdigest()
            assert hit["snippet"] == data.decode().splitlines()[hit["source_line"] - 1]
            selected = await agent.reference({"name": "flows", "offset": hit["offset"], "limit": 1})
            assert selected["content"] == str(hit["line"]) + ": " + hit["snippet"]
        assert result["matches"][-1]["snippet"] == "    def connect(self): return 'selected-toolkit-fresh'"


@pytest.mark.asyncio
@pytest.mark.parametrize("selected_reference_team", ["native-contracts"], indirect=True)
@pytest.mark.parametrize("query,module", [
    ("class MemberClassAddRequest:", "member_classes.py"),
    ("async def begin_add", "member_classes.py"),
    ("async def join_listing", "member_classes.py"),
    ("MEMBER_CLASS_STATE =", "member_classes.py"),
    ("class PrivateHostClient:", "host_control.py"),
    ("class PrivateAuthenticatedEvent:", "host_control.py"),
])
async def test_selected_native_contract_search_preserves_exact_definitions_and_origins(
        selected_reference_team, query, module):
    import hashlib
    f = selected_reference_team
    path = "meerkat_toolkit/" + module
    data = f.payload[path]
    for agent in (f.tools, f.reviewer):
        result = await agent.reference({"query": query, "limit": 40})
        assert result["unavailable_references"] == []
        assert result["total_matches"] == len(result["matches"]) == 1
        hit = result["matches"][0]
        assert hit["source_file"] == path
        assert hit["source_sha256"] == hashlib.sha256(data).hexdigest()
        assert hit["snippet"] == data.decode().splitlines()[hit["source_line"] - 1]
        page = await agent.reference({"name": hit["name"], "offset": hit["offset"], "limit": 1})
        assert page["content"] == str(hit["line"]) + ": " + hit["snippet"]
        assert page["reference_kind"] == "selected_sdk_source" and not page["observed_destination"]


@pytest.mark.asyncio
@pytest.mark.parametrize("selected_reference_team", ["native-contracts"], indirect=True)
async def test_selected_native_contract_pages_keep_complete_modules_bounded_and_inert(
        selected_reference_team, monkeypatch):
    import builtins
    import hashlib
    f = selected_reference_team
    original_import = builtins.__import__
    def no_sdk_import(name, *args, **kwargs):
        if name.split(".")[0] in {"meerkat_meta", "meerkat_toolkit", "meerkat_mobkit"}:
            pytest.fail("Selected native contracts must stay inert wheel source.")
        return original_import(name, *args, **kwargs)
    monkeypatch.setattr(builtins, "__import__", no_sdk_import)
    for agent in (f.tools, f.reviewer):
        for name in ("runtime", "roster", "messaging", "flows", "schedule"):
            lines, offset = [], 0
            while True:
                page = await agent.reference({"name": name, "offset": offset, "limit": 7})
                assert len(page["content"].splitlines()) <= 7
                lines.extend(line.split(": ", 1)[1] for line in page["content"].splitlines())
                if page["next_offset"] is None:
                    assert len(lines) == page["total_lines"]
                    break
                assert page["next_offset"] > offset
                offset = page["next_offset"]
            source = "\n".join(lines)
            for module in ("host_control.py", "application_runtime.py", "member_classes.py"):
                path = "meerkat_toolkit/" + module
                assert f.payload[path].decode().rstrip("\n") in source
                assert {"path": path, "sha256": hashlib.sha256(f.payload[path]).hexdigest()} in page["source_files"]
            assert "UNADVERTISED_TOOLKIT_SENTINEL" not in source
        result = await agent.reference({"query": "UNADVERTISED_TOOLKIT_SENTINEL"})
        assert result["matches"] == [] and result["unavailable_references"] == []


@pytest.mark.asyncio
async def test_selected_older_toolkit_does_not_fabricate_member_class_contracts(selected_reference_team):
    f = selected_reference_team
    assert "meerkat_toolkit/member_classes.py" not in f.payload
    for agent in (f.tools, f.reviewer):
        page = await agent.reference({"name": "runtime", "limit": 250})
        assert page.get("ok") is not False
        assert "ToolkitMobKitRuntime" in page["content"]
        assert all(row["path"] != "meerkat_toolkit/member_classes.py" for row in page["source_files"])
        result = await agent.reference({"name": "runtime", "query": "MemberClassClient"})
        assert result["matches"] == [] and result["unavailable_references"] == []


@pytest.mark.asyncio
@pytest.mark.parametrize("selected_reference_team", ["native-contracts"], indirect=True)
@pytest.mark.parametrize("query", [None, "begin_add"])
async def test_selected_native_contract_refuses_toolkit_change_before_return(
        selected_reference_team, monkeypatch, query):
    f = selected_reference_team
    original = f.tools._recheck_reference_sources
    calls = 0
    def change_at_final_fence(snapshot):
        nonlocal calls
        calls += 1
        if calls == 2:
            f.product.target.toolkit_wheel.path.write_bytes(b"changed after selected contract rendering")
        return original(snapshot)
    monkeypatch.setattr(f.tools, "_recheck_reference_sources", change_at_final_fence)
    args = {"name": "runtime"}
    if query is not None:
        args["query"] = query
    result = await f.tools.reference(args)
    assert calls == 2 and result["ok"] is False and "changed" in result["error"]
    assert "content" not in result and "matches" not in result


@pytest.mark.asyncio
@pytest.mark.parametrize("defect", ["missing-meta", "changed-meta", "changed-toolkit", "input", "protected-selection"])
async def test_selected_reference_refuses_lost_original_source_without_installed_fallback(
        selected_reference_team, monkeypatch, defect):
    f = selected_reference_team
    if defect == "missing-meta":
        f.product.target.meta_wheel.path.unlink()
    elif defect == "changed-meta":
        f.product.target.meta_wheel.path.write_bytes(b"altered selected wheel")
    elif defect == "changed-toolkit":
        f.product.target.toolkit_wheel.path.write_bytes(b"altered selected toolkit wheel")
    elif defect == "input":
        changed = deepcopy(f.frozen)
        changed.pop("runtime_candidate")
        write_private(f.tools.directory / "input.json", changed)
    else:
        (f.tools.workspace.project / "runtime-candidate.json").write_text("{}\n")
    def forbidden(*args, **kwargs):
        pytest.fail("A lost selected source cannot fall back to Forge SDK metadata.")
    monkeypatch.setattr(worker.metadata, "version", forbidden)
    for agent in (f.tools, f.reviewer):
        result = await agent.reference({"name": "schedule"})
        assert result["ok"] is False and "selected" in result["error"]
        assert "content" not in result


@pytest.mark.asyncio
async def test_selected_reference_rechecks_wheel_after_rendering(selected_reference_team, monkeypatch):
    f = selected_reference_team
    original_parse = worker.ast.parse
    changed = False
    def mutate_after_read(source, *args, **kwargs):
        nonlocal changed
        result = original_parse(source, *args, **kwargs)
        if not changed and "selected-schedule" in source:
            changed = True
            f.product.target.meta_wheel.path.write_bytes(b"changed after exact SDK read")
        return result
    monkeypatch.setattr(worker.ast, "parse", mutate_after_read)
    result = await f.tools.reference({"name": "schedule"})
    assert changed and result["ok"] is False and "changed" in result["error"]
    assert "content" not in result


@pytest.mark.asyncio
async def test_signed_candidate_reference_keeps_exact_candidate_sdk_and_common_recurrence(
        tmp_path, monkeypatch):
    import builtins
    from launchkit.runtime_selection import RuntimeCandidateTarget
    from test_runtime_candidates import Bundle, digest, wheel_bytes
    from test_reading_runtime_refresh import previous_framework

    bundle = Bundle(tmp_path / "signed-reference-artifacts")
    poison = "raise RuntimeError('signed SDK source must never import or execute')\n"
    bundle.payload.update({
        "meerkat_mobkit/runtime.py": (poison + "class MobKitRuntime:\n    def on_schedule_fire(self): return 'selected-signed-schedule'\n").encode(),
        "meerkat_mobkit/builder.py": (poison + "class MobKitBuilder:\n    def host_runnables(self): return 'selected-signed-runnables'\n").encode(),
    })
    bundle.wheel_values.update(bundle.payload)
    bundle.wheel_path.write_bytes(wheel_bytes(bundle.wheel_values))
    bundle.sdk["source_payload"] = {name: digest(data) for name, data in bundle.payload.items()}
    bundle.provenance["source_files"] = [{"path": "sdk/python/" + name,
        "sha256": digest(data), "bytes": len(data)} for name, data in bundle.payload.items()]
    bundle.seal()
    target = RuntimeCandidateTarget(bundle.tuple_path, bundle.root, bundle.binding)
    blueprint, predecessor = previous_framework(monkeypatch, "0.8.47")
    directory = tmp_path / "signed-reference-build"
    directory.mkdir(mode=0o700)
    workspace = ImplementationWorkspace(directory / "workspace", blueprint)
    workspace.seed(predecessor, runtime_refresh=True, runtime_target=target)
    framework = workspace.framework_provenance()
    write_private(directory / "input.json", {"blueprint": blueprint,
        "framework": framework, "runtime_candidate": framework["runtime_candidate"]})
    tools = BuildTools(directory)
    def no_environment(*args, **kwargs):
        pytest.fail("An exact signed candidate must not use installed SDK metadata.")
    monkeypatch.setattr(worker.metadata, "version", no_environment)
    original_import = builtins.__import__
    def no_sdk_import(name, *args, **kwargs):
        if name.split(".")[0] == "meerkat_mobkit":
            pytest.fail("Signed SDK wheel source must remain inert.")
        return original_import(name, *args, **kwargs)
    monkeypatch.setattr(builtins, "__import__", no_sdk_import)
    result = await tools.reference({"name": "schedule", "limit": 250})
    assert result.get("ok") is not False
    assert result["selected_sdk_versions"] == {"meerkat-mobkit": "0.8.48"}
    assert "selected-signed-schedule" in result["content"]
    assert "selected-signed-runnables" in result["content"]
    assert "Native authoring schema:" in result["content"]
    assert "0.8.42" not in result["content"] and not result["observed_destination"]
    runtime = await tools.reference({"name": "runtime", "limit": 250})
    assert runtime["source_files"] == [{"path": "meerkat_mobkit/runtime.py",
        "sha256": digest(bundle.payload["meerkat_mobkit/runtime.py"])}]


@pytest.mark.asyncio
@pytest.mark.parametrize("agent_index", (0, 1))
async def test_runtime_reference_query_finds_exact_public_result_type(team, agent_index):
    import hashlib
    from importlib import metadata
    from jsonschema import validate
    from meerkat_meta.models import SessionBuildOptions
    from meerkat_meta.types import MemberSnapshot

    agent = team[agent_index]
    args = {"query": "class membersnapshot:", "limit": 8}
    result = await agent.reference(args)
    hit = next(item for item in result["matches"]
               if item["source_file"] == "meerkat_meta/types.py")
    source_path = Path(inspect.getfile(MemberSnapshot))
    source = source_path.read_text()
    assert result["installed_sdk_version"] == metadata.version("meerkat-meta")
    assert hit["name"] == "types"
    assert hit["source_line"] == hit["line"] == hit["offset"] + 1
    assert source.splitlines()[hit["source_line"] - 1] == hit["snippet"]
    assert hit["source_sha256"] == hashlib.sha256(source_path.read_bytes()).hexdigest()
    page = await agent.reference({"name": hit["name"], "offset": hit["offset"] - 1,
                                  "limit": 70})
    text = "\n".join(line.split(": ", 1)[1] for line in page["content"].splitlines())
    assert inspect.getsource(MemberSnapshot).rstrip() in text
    assert page["source_files"] == [{"path": hit["source_file"], "sha256": hit["source_sha256"]}]
    draft = SessionBuildOptions()
    agent.register(draft)
    schema = next(tool["input_schema"] for tool in draft.to_dict()["tools"]
                  if tool["name"] == "read_runtime_reference")
    validate(args, schema)
    validate({"name": "types", "offset": hit["offset"], "limit": 70}, schema)


@pytest.mark.asyncio
async def test_runtime_reference_query_is_literal_bounded_and_paged(team):
    agent, _ = team
    first = await agent.reference({"name": "runtime", "query": "def ", "limit": 250})
    assert first["total_matches"] > 40
    assert len(first["matches"]) == 40 and first["next_offset"] == 40
    small = await agent.reference({"name": "runtime", "query": "def ", "limit": 3})
    next_page = await agent.reference({"name": "runtime", "query": "def ",
                                       "offset": small["next_offset"], "limit": 3})
    assert small["matches"] + next_page["matches"] == first["matches"][:6]
    assert next_page["next_offset"] == 6
    assert all(len(item["snippet"]) <= 320 for item in first["matches"])
    empty = await agent.reference({"query": "class MemberSnapshot:.*"})
    assert empty["matches"] == [] and empty["total_matches"] == 0
    assert empty["next_offset"] is None


@pytest.mark.asyncio
async def test_runtime_reference_query_preserves_focused_source_locations(team):
    from meerkat_meta.identity_first_providers import TopologyProviderProtocol

    result = await team[0].reference({"name": "roster", "query": "class TopologyProviderProtocol"})
    hit, = result["matches"]
    source = Path(inspect.getfile(TopologyProviderProtocol)).read_text().splitlines()
    assert hit["source_file"] == "meerkat_meta/identity_first_providers.py"
    assert source[hit["source_line"] - 1] == hit["snippet"]
    page = await team[0].reference({"name": "roster", "offset": hit["offset"], "limit": 1})
    assert page["content"] == f"{hit['line']}: {hit['snippet']}"


@pytest.mark.asyncio
@pytest.mark.parametrize("query", ("", "   ", None, 17, ["MemberSnapshot"], "x" * 161))
async def test_runtime_reference_rejects_invalid_query_before_sdk_reads(team, monkeypatch, query):
    def forbidden(*args, **kwargs):
        raise AssertionError("Invalid lookup must not read installed source.")
    monkeypatch.setattr(worker.inspect, "getfile", forbidden)
    result = await team[0].reference({"query": query})
    assert result == {"ok": False, "error": "Provide a nonempty literal query of at most 160 characters."}


@pytest.mark.asyncio
async def test_runtime_reference_lookup_cannot_read_private_modules_or_arbitrary_paths(team, tmp_path, monkeypatch):
    import meerkat_meta
    from jsonschema import ValidationError, validate
    from meerkat_meta.models import SessionBuildOptions

    installed = Path(inspect.getfile(meerkat_meta)).parent
    admitted = ("runtime.py", "builder.py", "identity_first_models.py", "types.py",
                "identity_first_providers.py", "errors.py")
    sdk = tmp_path / "reference-sdk"
    sdk.mkdir()
    for name in admitted:
        (sdk / name).write_bytes((installed / name).read_bytes())
    (sdk / "_client.py").write_text("PRIVATE_REFERENCE_SENTINEL = 'must not disclose'\n")
    monkeypatch.setattr(worker.inspect, "getfile", lambda module: str(sdk / "__init__.py"))
    result = await team[0].reference({"query": "PRIVATE_REFERENCE_SENTINEL"})
    assert result["matches"] == []
    assert str(sdk) not in json.dumps(result)
    for name in ("_client", "../_client.py", str(sdk / "_client.py")):
        denied = await team[0].reference({"name": name, "query": "PRIVATE_REFERENCE_SENTINEL"})
        assert denied == {"ok": False, "error": "Choose an advertised native reference name."}
    draft = SessionBuildOptions()
    team[0].register(draft)
    schema = next(tool["input_schema"] for tool in draft.to_dict()["tools"]
                  if tool["name"] == "read_runtime_reference")
    with pytest.raises(ValidationError):
        validate({"query": "MemberSnapshot", "path": str(sdk / "_client.py")}, schema)


@pytest.mark.asyncio
async def test_independent_real_checks_drive_bounded_builder_repair(team):
    require_sandbox()
    tools, reviewer = team
    built = await tools.check({})
    assert built["status"] == "pass", built
    assert (await tools.publish({"summary": "Builder says input normalization works", "unresolved": []}))["ok"]
    identities = []
    async def invoke(identity, content, key, admitted):
        identities.append(identity)
        if admitted:
            admitted("native-" + key)
        if identity == "acceptance:main":
            assert tools.publication
            context = await reviewer.context({})
            assert "Builder says" not in json.dumps(context)
            if not context["acceptance_tests"]:
                assert (await reviewer.write_test({"path": "test_normalization.py", "content": '''import unittest
from application.feature import normalize
class RequestedOutcome(unittest.TestCase):
    def test_mixed_case_spaces(self):
        self.assertEqual(normalize("  HeLLo  "), "hello")
'''}))["ok"]
            execution = await reviewer.check({})
            passed = execution["status"] == "pass"
            result = await reviewer.publish({"summary": "Requested mixed-case normalization tested independently.",
                "requirements": [{"id": "normalize-input", "status": "tested" if passed else "source",
                    "source_paths": ["application/feature.py"],
                    "test_ids": [row["id"] for row in execution["test_results"] if row["status"] == "pass"],
                    "evidence": "Mixed-case input must normalize to lowercase with spaces removed.", "remaining_dependencies": []}],
                "counterexamples": [] if passed else ["normalize('  HeLLo  ') returns 'HeLLo'; expected 'hello'."],
                "remaining_dependencies": []})
            assert result["ok"], result
        else:
            assert "counterexamples" in content
            feedback = await tools.feedback({})
            assert "HeLLo" in feedback["assessment"]["counterexamples"][0]
            test_source = await tools.feedback({"path": "test_normalization.py"})
            assert '"hello"' in test_source["content"]
            assert (await tools.write({"path": "application/feature.py", "content": "def normalize(value):\n    return value.strip().lower()\n"}))["ok"]
            checked = await tools.check({})
            assert checked["status"] == "pass", checked
            assert (await tools.publish({"summary": "Repaired actual mixed-case behavior", "unresolved": []}))["ok"]
        return {"interaction_id": "native-" + key, "native_completion": {"kind": "interaction_complete", "interaction_id": "native-" + key, "cursor": "42"}}
    result = await review_and_repair(tools.directory, tools, reviewer, invoke)
    assert result["status"] == "pass" and result["review_round"] == 2
    assert identities == ["acceptance:main", "implementation:main", "acceptance:main"]
    history = reviewer.acceptance.history()
    assert history[0]["publication"]["status"] == "needs_repair"
    assert history[0]["source_digest"] != history[1]["source_digest"]
    assert history[0]["reviewer_interaction_id"] == "native-acceptance-1"
    assert history[1]["reviewer_interaction_id"] == "native-acceptance-2"
    assert result == reviewer.acceptance.verification()


@pytest.mark.asyncio
async def test_repair_budget_preserves_failures_without_false_completion(team):
    tools, reviewer = team
    builder_check_fixture(tools)
    calls = []
    async def invoke(identity, content, key, admitted):
        calls.append(identity)
        if admitted:
            admitted("native-" + key)
        if identity == "acceptance:main":
            # Empty protected tests are a real host-issued failure, even when
            # the builder fixture has current self-check evidence.
            execution = await reviewer.check({})
            assert execution["tests_run"] == 0
            result = await reviewer.publish({"summary": "Requested outcome remains absent.",
                "requirements": [{"id": "normalize-input", "status": "unresolved", "source_paths": [],
                    "test_ids": [], "evidence": "No executable outcome has been demonstrated.",
                    "remaining_dependencies": ["Implement the requested input behavior."]}],
                "counterexamples": ["Mixed-case normalization is still missing."], "remaining_dependencies": []})
            assert result["ok"]
        else:
            tools.workspace.write_file(f"application/repair_{len(calls)}.py", "CHANGED = True\n")
            builder_check_fixture(tools)
        return {"interaction_id": "native-" + key, "native_completion": {"kind": "interaction_complete", "interaction_id": "native-" + key, "cursor": "42"}}
    with pytest.raises(RuntimeError, match="automatic repair budget"):
        await review_and_repair(tools.directory, tools, reviewer, invoke)
    assert calls == ["acceptance:main", "implementation:main", "acceptance:main", "implementation:main", "acceptance:main"]
    assert len(reviewer.acceptance.history()) == 3
    receipt = read_receipt(tools.directory)
    assert not receipt["ready_to_package"]
    assert receipt["semantic_acceptance"]["status"] == "needs_repair"


@pytest.mark.asyncio
async def test_reviewer_completion_without_publication_or_builder_without_current_checks_is_rejected(team):
    tools, reviewer = team
    builder_check_fixture(tools)
    async def invoke(identity, content, key, admitted):
        admitted("native-review-1")
    with pytest.raises(RuntimeError, match="without a current protected"):
        await review_and_repair(tools.directory, tools, reviewer, invoke)
    tools.workspace.write_file("application/changed.py", "CHANGED = True\n")
    with pytest.raises(RuntimeError, match="current checked"):
        await review_and_repair(tools.directory, tools, reviewer, invoke)


def completed_review(interaction):
    return {"interaction_id": interaction, "native_completion": {
        "kind": "interaction_complete", "interaction_id": interaction, "cursor": "console:99"}}


async def publish_checked_normalization(reviewer, execution):
    result = await reviewer.publish({"summary": "Protected input behavior was executed on this exact source.",
        "requirements": [{"id": "normalize-input", "status": "tested",
            "source_paths": ["application/feature.py"],
            "test_ids": [row["id"] for row in execution["test_results"] if row["status"] == "pass"],
            "evidence": "The actual mixed-case and whitespace result matches the requested behavior.",
            "remaining_dependencies": []}], "counterexamples": [], "remaining_dependencies": []})
    assert result["ok"], result


@pytest.mark.asyncio
async def test_normally_completed_missing_publication_gets_one_fresh_checked_native_review(team):
    require_sandbox()
    tools, reviewer = team
    tools.workspace.write_file("application/feature.py", "def normalize(value):\n    return value.strip().lower()\n")
    builder_check_fixture(tools)
    digest = tools.workspace.digest()
    calls, executions = [], []
    protected = '''import unittest
from application.feature import normalize
class RequestedOutcome(unittest.TestCase):
    def test_mixed_case_spaces(self):
        self.assertEqual(normalize("  HeLLo  "), "hello")
'''
    async def invoke(identity, content, key, admitted):
        assert identity == "acceptance:main"
        calls.append(key)
        interaction = "native-" + key
        admitted(interaction)
        history = reviewer.acceptance.history()
        assert history[-1]["execution"] is None and history[-1]["publication"] is None
        if len(calls) == 1:
            assert (await reviewer.write_test({"path": "test_normalization.py", "content": protected}))["ok"]
        else:
            assert "completed normally without calling publish_acceptance" in content
            assert "run check_acceptance again" in content
            assert reviewer.acceptance.read_test("test_normalization.py") == protected
            assert history[0]["execution"]["id"] == executions[0]["id"]
            assert history[0]["publication"] is None and history[0]["native_completion"] is None
            assert reviewer.acceptance.verification() is None
        execution = await reviewer.check({})
        assert execution["status"] == "pass", execution
        executions.append(execution)
        if len(calls) == 2:
            await publish_checked_normalization(reviewer, execution)
            assert reviewer.acceptance.verification() is None
        return completed_review(interaction)
    accepted = await review_and_repair(tools.directory, tools, reviewer, invoke)
    assert calls == ["acceptance-1", "acceptance-1-publication-retry-1"]
    assert len({row["id"] for row in executions}) == 2
    assert tools.workspace.digest() == digest
    assert accepted == reviewer.acceptance.verification()
    assert accepted["reviewer_interaction_id"] == "native-acceptance-1-publication-retry-1"
    assert accepted["execution"]["id"] == executions[1]["id"]
    history = reviewer.acceptance.history()
    assert history[0]["source_digest"] == history[1]["source_digest"] == digest
    assert history[1]["native_completion"]["interaction_id"] == accepted["reviewer_interaction_id"]
    receipt = read_receipt(tools.directory)
    assert receipt["review_attempt_reason"] == "publication_retry"
    assert receipt["review_publication_retries"] == 1
    assert "review_transport_retries" not in receipt
    assert receipt["review_publication_failures"] == [{
        "at": receipt["review_publication_failures"][0]["at"],
        "reason": "completed_without_publication", "review_round": 1, "attempt": 1,
        "protected_round": 1, "interaction_id": "native-acceptance-1",
        "native_completion": completed_review("native-acceptance-1")["native_completion"],
        "source_digest": digest, "retry_requested": True}]


@pytest.mark.asyncio
async def test_repeated_normally_completed_missing_publication_is_bounded_and_unaccepted(team):
    tools, reviewer = team
    builder_check_fixture(tools)
    calls = []
    async def invoke(identity, content, key, admitted):
        calls.append(key)
        admitted("native-" + key)
        # A real failed/no-tests execution is retained, never auto-published.
        assert (await reviewer.check({}))["tests_run"] == 0
        return completed_review("native-" + key)
    with pytest.raises(RuntimeError, match="without a current protected"):
        await review_and_repair(tools.directory, tools, reviewer, invoke)
    assert calls == ["acceptance-1", "acceptance-1-publication-retry-1"]
    assert len(reviewer.acceptance.history()) == 2
    assert reviewer.acceptance.verification() is None
    receipt = read_receipt(tools.directory)
    assert receipt["review_publication_retries"] == 1
    assert [row["retry_requested"] for row in receipt["review_publication_failures"]] == [True, False]
    assert all(row["reason"] == "completed_without_publication" for row in receipt["review_publication_failures"])
    assert not receipt["ready_to_package"]


@pytest.mark.asyncio
@pytest.mark.parametrize("invalid", ["no_completion", "unbound", "wrong_interaction", "missing_cursor", "failed"])
async def test_missing_publication_retry_requires_matching_normally_completed_admission(team, invalid):
    tools, reviewer = team
    builder_check_fixture(tools)
    calls = []
    async def invoke(identity, content, key, admitted):
        calls.append(key)
        if invalid != "unbound":
            admitted("native-" + key)
        completed = completed_review("native-" + key)
        if invalid == "no_completion": return None
        if invalid == "wrong_interaction": completed["native_completion"]["interaction_id"] = "another-turn"
        if invalid == "missing_cursor": completed["native_completion"].pop("cursor")
        if invalid == "failed": completed["native_completion"]["is_error"] = True
        return completed
    with pytest.raises(RuntimeError, match="without a current protected"):
        await review_and_repair(tools.directory, tools, reviewer, invoke)
    assert calls == ["acceptance-1"]
    assert reviewer.acceptance.verification() is None
    assert "review_publication_retries" not in read_receipt(tools.directory)


@pytest.mark.asyncio
@pytest.mark.parametrize("changed", ["workspace", "frozen_source", "between_rounds"])
async def test_missing_publication_recovery_refuses_changed_reviewed_source(team, changed, monkeypatch):
    tools, reviewer = team
    builder_check_fixture(tools)
    calls = []
    begin = reviewer.acceptance.begin_round
    def begin_with_changed_source():
        if reviewer.acceptance.history():
            tools.workspace.write_file("application/changed.py", "CHANGED = True\n")
        return begin()
    if changed == "between_rounds":
        monkeypatch.setattr(reviewer.acceptance, "begin_round", begin_with_changed_source)
    async def invoke(identity, content, key, admitted):
        calls.append(key)
        admitted("native-" + key)
        if changed == "workspace":
            tools.workspace.write_file("application/changed.py", "CHANGED = True\n")
        elif changed == "frozen_source":
            path = reviewer.acceptance.root / "round-1" / "source" / "application" / "feature.py"
            path.write_text("def normalize(value):\n    return 'changed'\n")
        return completed_review("native-" + key)
    with pytest.raises(RuntimeError, match="current checked|reviewed source changed"):
        await review_and_repair(tools.directory, tools, reviewer, invoke)
    assert calls == ["acceptance-1"]
    assert reviewer.acceptance.verification() is None


@pytest.mark.asyncio
@pytest.mark.parametrize("stale", ["publication", "execution", "mutated_tests"])
async def test_publication_recovery_cannot_reuse_old_or_invalidated_protected_evidence(team, stale):
    require_sandbox()
    tools, reviewer = team
    tools.workspace.write_file("application/feature.py", "def normalize(value):\n    return value.strip().lower()\n")
    builder_check_fixture(tools)
    calls, old = [], {}
    protected = "import unittest\nfrom application.feature import normalize\nclass Outcome(unittest.TestCase):\n    def test_case(self):\n        self.assertEqual(normalize('  HeLLo  '), 'hello')\n"
    async def invoke(identity, content, key, admitted):
        calls.append(key)
        admitted("native-" + key)
        if len(calls) == 1:
            assert (await reviewer.write_test({"path": "test_normalization.py", "content": protected}))["ok"]
            execution = await reviewer.check({})
            assert execution["status"] == "pass", execution
            await publish_checked_normalization(reviewer, execution)
            old["publication"] = deepcopy(reviewer.publication)
            # Simulate missing in-memory acknowledgement, never seal the old assessment.
            reviewer.publication = None
        elif stale == "publication":
            reviewer.publication = old["publication"]
        elif stale == "execution":
            assert reviewer.acceptance.history()[-1]["execution"] is None
            attempted = await reviewer.publish({
                "summary": old["publication"]["summary"],
                "requirements": old["publication"]["requirements"],
                "counterexamples": [], "remaining_dependencies": []})
            assert not attempted["ok"] and "Run host acceptance tests" in attempted["error"]
        else:
            execution = await reviewer.check({})
            assert execution["status"] == "pass", execution
            await publish_checked_normalization(reviewer, execution)
            refused = await reviewer.write_test({"path": "test_normalization.py", "content": protected + "# Changed after publication.\n"})
            assert not refused["ok"] and "sealed" in refused["error"]
            # Model tools cannot mutate a published round. A changed file must
            # also fail the host's final seal, rather than reuse its old check.
            row = reviewer.acceptance.history()[-1]
            path = reviewer.acceptance.root / f"round-{row['number']}" / "tests" / "test_normalization.py"
            path.write_text(protected + "# Changed after publication.\n")
        return completed_review("native-" + key)
    with pytest.raises((RuntimeError, workspace_module.WorkspaceError), match="published reviewer assessment|without a current protected|Source or tests changed"):
        await review_and_repair(tools.directory, tools, reviewer, invoke)
    assert calls == ["acceptance-1", "acceptance-1-publication-retry-1"]
    assert reviewer.acceptance.verification() is None
    assert not read_receipt(tools.directory)["ready_to_package"]


@pytest.mark.asyncio
async def test_missing_publication_budget_is_per_build_not_per_builder_repair(team):
    tools, reviewer = team
    builder_check_fixture(tools)
    calls = []
    async def invoke(identity, content, key, admitted):
        calls.append(key)
        if admitted: admitted("native-" + key)
        if identity == "implementation:main":
            # This fixture exercises a fresh review after a changed repair;
            # an unchanged repair now retains the current failed assessment.
            tools.workspace.write_file("application/changed_repair.py", "CHANGED = True\n")
            builder_check_fixture(tools)
        elif key == "acceptance-1-publication-retry-1":
            assert (await reviewer.check({}))["tests_run"] == 0
            result = await reviewer.publish({"summary": "Behavior remains missing.",
                "requirements": [{"id": "normalize-input", "status": "unresolved", "source_paths": [],
                    "test_ids": [], "evidence": "No executable outcome has been demonstrated.",
                    "remaining_dependencies": ["Implement the requested behavior."]}],
                "counterexamples": ["Requested behavior remains absent."], "remaining_dependencies": []})
            assert result["ok"], result
        return completed_review("native-" + key)
    with pytest.raises(RuntimeError, match="without a current protected"):
        await review_and_repair(tools.directory, tools, reviewer, invoke)
    assert calls == ["acceptance-1", "acceptance-1-publication-retry-1", "repair-1", "acceptance-2"]
    assert [row["retry_requested"] for row in read_receipt(tools.directory)["review_publication_failures"]] == [True, False]
    assert reviewer.acceptance.verification() is None


@pytest.mark.asyncio
async def test_missing_publication_and_transport_retries_have_separate_bounded_keys(team):
    tools, reviewer = team
    builder_check_fixture(tools)
    calls = []
    async def invoke(identity, content, key, admitted):
        calls.append(key)
        admitted("native-" + key)
        if len(calls) == 2:
            raise incomplete_stream_failure("native-" + key)
        return completed_review("native-" + key)
    with pytest.raises(RuntimeError, match="without a current protected"):
        await review_and_repair(tools.directory, tools, reviewer, invoke)
    assert calls == ["acceptance-1", "acceptance-1-publication-retry-1", "acceptance-1-transport-retry-1"]
    receipt = read_receipt(tools.directory)
    assert receipt["review_publication_retries"] == receipt["review_transport_retries"] == 1
    assert receipt["review_attempt_reason"] == "transport_retry"
    assert reviewer.acceptance.verification() is None


@pytest.mark.asyncio
async def test_native_identity_admission_and_timeline_are_retained(team, monkeypatch):
    tools, _ = team
    requested = []
    async def request(client, method, url, **kwargs):
        requested.append((method, kwargs))
        if method == "post":
            return SimpleNamespace(json=lambda: {"interaction_id": "actual-native-admission", "cursor": "40"})
        return SimpleNamespace(json=lambda: {"frames": [
            {"cursor": "41", "kind": "tool_call", "interaction_id": "actual-native-admission", "payload": {"name": "inspect_acceptance"}},
            {"cursor": "42", "kind": "interaction_complete", "interaction_id": "actual-native-admission", "payload": {}}]})
    monkeypatch.setattr(worker, "_native_request", request)
    admitted = []
    result = await native_turn(None, "http://127.0.0.1:9999", tools.directory, asyncio.Event(),
                               "acceptance:main", "review", "native-key", admitted.append)
    assert admitted == ["actual-native-admission"]
    assert requested[0][1]["json"]["identity"] == "acceptance:main"
    assert requested[1][1]["params"]["identity"] == "acceptance:main"
    frames = [json.loads(row) for row in (tools.directory / "timeline.jsonl").read_text().splitlines()]
    assert frames[0]["payload"] == {"name": "inspect_acceptance"}
    assert {frame["worker_native_identity"] for frame in frames} == {"acceptance:main"}
    receipt = read_receipt(tools.directory)
    assert receipt["acceptance_admission"] == {key: value for key, value in result.items() if key != "native_completion"}
    assert receipt["native_turns"][0]["identity"] == "acceptance:main"


def test_reviewer_tool_registry_cannot_author_builder_source_or_receipts(team):
    tools, reviewer = team
    class Draft:
        def __init__(self): self.names = []
        def register_tool(self, name, *args, **kwargs): self.names.append(name)
    builder = Draft(); independent = Draft()
    tools.register(builder); reviewer.register(independent)
    assert "write_project_file" in builder.names and "write_project_file" not in independent.names
    assert "write_acceptance_test" in independent.names and "write_acceptance_test" not in builder.names
    assert "publish_acceptance" in independent.names and "publish_acceptance" not in builder.names
    assert "publish_implementation" not in independent.names


def incomplete_stream_failure(interaction="native-review-failure"):
    return NativeInteractionFailure({"kind": "interaction_failed", "interaction_id": interaction,
        "cursor": "console:81", "payload": {
            "error": "Terminal failure: Failed (LlmFailure): LLM error (openai): Incomplete response: Stream ended without Done event",
            "terminal_cause_kind": "llm_failure", "error_report": {"class": "llm",
                "reason": {"cause_kind": "llm_failure", "outcome": "failed"}}}})


def test_retry_classification_requires_actual_typed_incomplete_stream_terminal():
    error = incomplete_stream_failure()
    with pytest.raises(NativeInteractionFailure) as raised:
        interaction_completed([error.frame], "native-review-failure")
    assert raised.value.retryable_review_transport


@pytest.mark.parametrize("mutation", [
    lambda f: f.update(kind="interaction_canceled"),
    lambda f: f.update(kind="message_delivery_failed"),
    lambda f: f["payload"].pop("error_report"),
    lambda f: f["payload"]["error_report"].update(**{"class": "tool"}),
    lambda f: f["payload"]["error_report"]["reason"].update(cause_kind="validation_failure"),
    lambda f: f["payload"].update(error="Requested approval behavior is missing."),
    lambda f: f["payload"].update(error="LLM token limit reached."),
])
def test_other_native_failures_are_not_transient_review_retries(mutation):
    frame = incomplete_stream_failure().frame
    mutation(frame)
    assert not NativeInteractionFailure(frame).retryable_review_transport


@pytest.mark.asyncio
async def test_native_stream_retry_after_repair_preserves_tests_and_requires_fresh_acceptance(team):
    require_sandbox()
    tools, reviewer = team
    assert (await tools.check({}))["status"] == "pass"
    assert (await tools.publish({"summary": "Original builder fixture", "unresolved": []}))["ok"]
    calls = []
    protected = '''import unittest
from application.feature import normalize
class RequestedOutcome(unittest.TestCase):
    def test_mixed_case_spaces(self):
        self.assertEqual(normalize("  HeLLo  "), "hello")
'''
    async def invoke(identity, content, key, admitted):
        calls.append((identity, key))
        interaction = "native-" + key
        if admitted:
            admitted(interaction)
        if identity == "implementation:main":
            assert "counterexamples" in content
            await tools.write({"path": "application/feature.py",
                               "content": "def normalize(value):\n    return value.strip().lower()\n"})
            assert (await tools.check({}))["status"] == "pass"
            assert (await tools.publish({"summary": "Repaired actual mixed-case behavior", "unresolved": []}))["ok"]
        else:
            context = await reviewer.context({})
            if not context["acceptance_tests"]:
                assert (await reviewer.write_test({"path": "test_normalization.py", "content": protected}))["ok"]
            else:
                assert reviewer.acceptance.read_test("test_normalization.py") == protected
            execution = await reviewer.check({})
            passed = execution["status"] == "pass"
            result = await reviewer.publish({"summary": "Independent mixed-case behavior check.",
                "requirements": [{"id": "normalize-input", "status": "tested" if passed else "source",
                    "source_paths": ["application/feature.py"],
                    "test_ids": [r["id"] for r in execution["test_results"] if r["status"] == "pass"],
                    "evidence": "Mixed-case input is normalized by the actual application.",
                    "remaining_dependencies": []}],
                "counterexamples": [] if passed else ["Mixed-case input is not normalized."],
                "remaining_dependencies": []})
            assert result["ok"], result
            if key == "acceptance-2":
                # Publishing a passing assessment before provider failure must
                # never count as accepted source or native completion.
                assert reviewer.acceptance.verification() is None
                raise incomplete_stream_failure(interaction)
            if "transport-retry" in key:
                assert "unfinished assessment is not accepted" in content
        return {"interaction_id": interaction, "native_completion": {
            "kind": "interaction_complete", "interaction_id": interaction, "cursor": "console:99"}}
    accepted = await review_and_repair(tools.directory, tools, reviewer, invoke)
    assert calls == [("acceptance:main", "acceptance-1"), ("implementation:main", "repair-1"),
                     ("acceptance:main", "acceptance-2"),
                     ("acceptance:main", "acceptance-2-transport-retry-1")]
    history = reviewer.acceptance.history()
    assert len(history) == 3
    assert history[0]["publication"]["status"] == "needs_repair"
    assert history[1]["publication"]["status"] == "pass"
    assert history[1]["native_completion"] is None
    assert not history[1]["publication"]["ready_to_package"]
    assert history[1]["source_digest"] == history[2]["source_digest"]
    assert accepted == reviewer.acceptance.verification()
    assert accepted["reviewer_interaction_id"] == "native-acceptance-2-transport-retry-1"
    receipt = read_receipt(tools.directory)
    assert receipt["review_transport_retries"] == 1
    assert receipt["review_transport_failures"][0]["interaction_id"] == "native-acceptance-2"
    assert receipt["review_transport_failures"][0]["retry_admitted"] is True


@pytest.mark.asyncio
async def test_transient_review_retry_is_bounded_and_retains_partial_tests(team):
    tools, reviewer = team
    builder_check_fixture(tools)
    calls = []
    test = "import unittest\nclass Partial(unittest.TestCase):\n    def test_case(self):\n        self.assertEqual(1, 1)\n"
    async def invoke(identity, content, key, admitted):
        calls.append(key)
        admitted("native-" + key)
        if len(calls) == 1:
            assert (await reviewer.write_test({"path": "test_partial.py", "content": test}))["ok"]
        else:
            assert reviewer.acceptance.read_test("test_partial.py") == test
        raise incomplete_stream_failure("native-" + key)
    with pytest.raises(NativeInteractionFailure, match="Stream ended"):
        await review_and_repair(tools.directory, tools, reviewer, invoke)
    assert calls == ["acceptance-1", "acceptance-1-transport-retry-1"]
    assert len(reviewer.acceptance.history()) == 2
    assert reviewer.acceptance.verification() is None
    receipt = read_receipt(tools.directory)
    assert not receipt["ready_to_package"]
    assert [row["retry_admitted"] for row in receipt["review_transport_failures"]] == [True, False]


@pytest.mark.asyncio
@pytest.mark.parametrize("failure", [RuntimeError("Reviewer did not publish"), asyncio.CancelledError(),
                                    NativeInteractionFailure({"kind": "interaction_failed",
                                        "payload": {"error": "Required behavior is missing."}})])
async def test_domain_failures_and_cancellation_do_not_admit_review_retry(team, failure):
    tools, reviewer = team
    builder_check_fixture(tools)
    calls = []
    async def invoke(identity, content, key, admitted):
        calls.append(key)
        admitted("native-" + key)
        raise failure
    with pytest.raises(type(failure)):
        await review_and_repair(tools.directory, tools, reviewer, invoke)
    assert calls == ["acceptance-1"]
    assert reviewer.acceptance.verification() is None
    assert not read_receipt(tools.directory)["ready_to_package"]


@pytest.mark.parametrize("agent_index", (0, 1))
async def test_builder_and_reviewer_can_read_helperless_factory_contract_without_sdk_lookup(team, agent_index, monkeypatch):
    from launchkit.application_inventory_contract import HELPERLESS_APPLICATION_CONTRACT
    def unavailable():
        raise AssertionError("The application declaration contract must not require factory discovery.")
    monkeypatch.setattr(team[0], "_selected_reference_sources", unavailable)
    page = await team[agent_index].reference({"name": "application", "limit": 180})
    content = "\n".join(line.split(": ", 1)[1] for line in page["content"].splitlines())
    assert HELPERLESS_APPLICATION_CONTRACT.strip() in content
    assert '"factory":"application:project"' in content
    assert "seed policy can coexist with a dynamic roster" in content
