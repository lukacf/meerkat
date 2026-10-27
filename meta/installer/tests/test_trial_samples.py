import json
from types import SimpleNamespace
import uuid

import pytest

from launchkit.blueprint import starter_blueprint
from launchkit.generator import PackageFile
from launchkit.implementation import ImplementationService, IMPLEMENTATION_PROMPT, APPLICATION_CONTRACT
from launchkit.acceptance_reviewer import ACCEPTANCE_PROMPT
from launchkit.implementation_workspace import ImplementationWorkspace
from launchkit.projects import fingerprint
from launchkit.templates.storage_setup import write_private
from launchkit.trial_samples import trial_samples


DIGEST = "a" * 64
CASE = {"label": "Save a title twice", "request": "Add Dune to my wishlist twice and show the collection.",
        "fixtures": {"service": {"available": True}, "events": []}}


def samples(value):
    return trial_samples([PackageFile("application/trial.json", json.dumps(value))], DIGEST)


def test_authored_sample_projection_is_bounded_source_data_without_importing_code():
    files = [PackageFile("application/__init__.py", "raise AssertionError('never import authored code')\n"),
             PackageFile("application/trial.json", json.dumps({"cases": [CASE], "claims": {"accepted": True}}))]
    result = trial_samples(files, DIGEST)
    assert result["cases"] == [CASE] and result["source_digest"] == DIGEST
    assert "acceptance evidence" in result["scope"] and "claims" not in result
    assert result["detail"] is None
    assert samples({"cases": [{"label": "Try local state", "request": "Save a note"}]})["cases"][0]["fixtures"] == {}
    absent = trial_samples([], DIGEST)
    assert absent["cases"] == [] and "no authored sample choices" in absent["detail"]
    with pytest.raises(ValueError, match="exact checked source"):
        trial_samples(files, "unbound")


@pytest.mark.parametrize("value", [
    {"cases": []}, {"cases": [CASE] * 13}, {"cases": [None]},
    {"cases": [{**CASE, "label": " "}]}, {"cases": [{**CASE, "label": "x" * 161}]},
    {"cases": [{**CASE, "label": "hidden\nline"}]},
    {"cases": [{**CASE, "request": "x" * 6001}]},
    {"cases": [{**CASE, "fixtures": []}]}, {"cases": [{**CASE, "fixtures": None}]},
    {"cases": [{**CASE, "fixtures": {"value": float("inf")}}]},
    {"cases": [{**CASE, "fixtures": {"value": float("nan")}}]},
    {"cases": [CASE], "extra": "x" * 100_000},
])
def test_invalid_sample_metadata_never_creates_choices(value):
    result = samples(value)
    assert result["cases"] == [] and result["detail"]
    assert result["source_digest"] == DIGEST


@pytest.mark.parametrize("content", ["not JSON", '{"cases":[{"label":"A","request":"B","fixtures":{"x":1e999}}]}'])
def test_malformed_and_numeric_overflow_are_explicitly_unsupported(content):
    result = trial_samples([PackageFile("application/trial.json", content)], DIGEST)
    assert result["cases"] == [] and result["detail"]


def stamp_source_check(workspace):
    # Metadata projection uses an authentic host-signature fixture. Real sandbox
    # callback execution is covered by test_application_trial.py.
    with workspace._locked():
        stored = workspace._state()
        receipt = {"status": "pass", "tests_run": 1, "ready_to_package": True,
                   "digest": workspace._digest(workspace._files(stored)), "checks": []}
        receipt["signature"] = workspace._signed(receipt)
        stored["last_check"] = receipt
        workspace._save_state(stored)


@pytest.mark.asyncio
async def test_status_only_exposes_samples_for_current_authentic_checked_source(tmp_path):
    identity, build_id = str(uuid.uuid4()), str(uuid.uuid4())
    proposal = starter_blueprint().model_dump(mode="json")
    state = {"id": identity, "version": 3, "proposal": proposal, "brief": {"mission": "Keep my reading collection"}}
    service = ImplementationService(tmp_path, SimpleNamespace())
    directory = service._directory(identity, build_id)
    directory.mkdir(parents=True)
    write_private(directory.parent / "latest.json", {"build_id": build_id})
    frozen = {"guide_id": identity, "guide_version": 3, "blueprint": proposal,
              "brief": state["brief"], "acceptance_policy_version": 1}
    write_private(directory / "input.json", frozen)
    write_private(directory / "receipt.json", {"guide_id": identity, "guide_version": 3,
        "build_id": build_id, "blueprint_hash": fingerprint(proposal),
        "brief_hash": fingerprint(state["brief"]), "status": "failed"})
    workspace = ImplementationWorkspace(directory / "workspace", proposal)
    workspace.seed()
    workspace.write_file("application/__init__.py", "def build(context):\n    return object()\n")
    workspace.write_file("application/trial.json", json.dumps({"cases": [CASE]}))
    stamp_source_check(workspace)
    status = await service.status(identity, state=state)
    assert status["source_verified"] and status["trial_samples"]["cases"] == [CASE]
    assert status["trial_samples"]["source_digest"] == status["source_digest"] == workspace.digest()
    assert not status["acceptance_trusted"] and not status["ready_to_package"]
    harmless = await service.status(identity, state={**state, "version": 4,
        "messages": [{"text": "What can I try?"}]})
    assert harmless["trial_samples"] == status["trial_samples"]
    assert harmless["guide_version"] == 3 and harmless["current_guide_version"] == 4
    assert json.loads((directory / "input.json").read_text()) == frozen
    stale = await service.status(identity, state={**state, "version": 4,
        "brief": {"mission": "Keep my reading collection and accept Telegram requests"}})
    assert stale["trial_samples"] is None
    workspace.write_file("application/trial.json", json.dumps({"cases": [{**CASE, "label": "Changed source"}]}))
    changed = await service.status(identity, state=state)
    assert not changed["source_verified"] and changed["trial_samples"] is None
    assert changed["source_digest"] != status["source_digest"]
    stamp_source_check(workspace)
    checked = await service.status(identity, state=state)
    assert checked["trial_samples"]["cases"][0]["label"] == "Changed source"
    assert checked["trial_samples"]["source_digest"] != status["trial_samples"]["source_digest"]
    for invalid in ({key: value for key, value in frozen.items() if key != "guide_id"},
                    {key: value for key, value in frozen.items() if key != "guide_version"},
                    {**frozen, "guide_id": str(uuid.uuid4())},
                    {**frozen, "guide_version": True}):
        write_private(directory / "input.json", invalid)
        unbound = await service.status(identity, state=state)
        assert unbound["source_verified"] and not unbound["current"]
        assert unbound["trial_samples"] is None
    write_private(directory / "input.json", frozen)
    with workspace._locked():
        stored = workspace._state()
        stored["last_check"]["signature"] = "forged"
        workspace._save_state(stored)
    forged = await service.status(identity, state=state)
    assert forged["current"] and not forged["source_verified"]
    assert forged["trial_samples"] is None


def test_requested_trial_contract_requires_shared_behavior_and_independent_source_review():
    assert "For offline-implementable requested behavior, implement rehearse" in IMPLEMENTATION_PROMPT
    assert "Make empty\nfixtures useful when no external I/O is needed" in IMPLEMENTATION_PROMPT
    assert "application/trial.json" in APPLICATION_CONTRACT
    assert "Missing\nsupport is a concrete source gap" in ACCEPTANCE_PROMPT
    assert "parallel demonstration that bypasses production constraints" in ACCEPTANCE_PROMPT
    assert "do not require an application to fabricate unavailable live I/O" in ACCEPTANCE_PROMPT



def test_builder_prompt_distinguishes_helperless_dynamic_rosters_from_legacy_fixed_rosters():
    """Builder guidance states native dynamic admission and the legacy fixed-roster limit."""
    text = " ".join(IMPLEMENTATION_PROMPT.split())
    legacy = "Legacy startup without native class composition still requires a fixed roster."
    dynamic = ("This helperless Connect route supports a dynamic roster through native governed Add "
               "and its existing SDK customizer.")
    assert legacy in text
    assert dynamic in text
    assert ("declare reusable classes in application/native-inventory.json and fixed-seed grants "
            "in application/native-tool-policy.json.") in text
    assert text.index("desired_roster/desired_edges hooks") < text.index(dynamic) < text.index(legacy)

def test_correction_contract_records_an_outcome_without_a_feasible_choice():
    assert "Where findings recommend among options, they must be able to record" in IMPLEMENTATION_PROMPT
    assert "that no option is feasible or that the decision is\nunderdetermined" in IMPLEMENTATION_PROMPT
    assert "must supersede the earlier recommendation\nwithout inventing a replacement choice" in IMPLEMENTATION_PROMPT
    assert ("Where findings recommend among options,\nexercise a correction that leaves no feasible option, "
            "or an underdetermined decision") in ACCEPTANCE_PROMPT
    assert "check for an authoritative saved outcome and corrected readback after restart" in ACCEPTANCE_PROMPT


def test_natural_language_guidance_preserves_agent_judgment_and_typed_edits():
    import ast
    from pathlib import Path
    from launchkit import implementation_worker

    assert "Use the responsible native agents, their context and tools to interpret free-form" in IMPLEMENTATION_PROMPT
    assert "explicit typed edits; application code validates authority, identifiers, revisions" in IMPLEMENTATION_PROMPT
    assert "matching review examples cannot replace agent judgment over natural language" in IMPLEMENTATION_PROMPT
    assert "fixtures may supply labelled agent outputs to test the real typed edit boundary" in IMPLEMENTATION_PROMPT
    assert "phrase matching, keyword classifiers or regex negation windows substituting for" in ACCEPTANCE_PROMPT
    assert "Do not require a deterministic free-text\nemulator" in ACCEPTANCE_PROMPT
    assert "unproven until an actual agent handles varied wording and context successfully" in ACCEPTANCE_PROMPT
    source = ast.parse(Path(implementation_worker.__file__).read_text())
    repair = next(node.args[1].left.value for node in ast.walk(source)
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Name) and node.func.id == "invoke"
        and len(node.args) > 1 and isinstance(node.args[1], ast.BinOp)
        and isinstance(node.args[1].left, ast.Constant)
        and isinstance(node.args[1].left.value, str)
        and node.args[1].left.value.startswith("The independent reviewer found"))
    assert "Treat counterexamples as evidence of a general defect, not phrases to recognize" in repair
    assert "native agent judgment over free text and repair its context/tools and typed edit path" in repair
    assert "do not substitute keyword or regex classification to satisfy a review fixture" in repair
