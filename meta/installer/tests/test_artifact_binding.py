from copy import deepcopy
import json

import pytest

from launchkit.artifact_binding import frozen_matches, inputs, matches, package_matches, current_version, report_brief_hash
from launchkit.blueprint import starter_blueprint
from launchkit.deployment_sources import read_package_source


def scope():
    return {"id": "guide", "version": 3, "proposal": starter_blueprint().model_dump(mode="json"),
            "brief": {"facts": {"outcome": {"summary": "Keep requested books", "status": "confirmed"}},
                      "requirements": [{"id": "ledger", "outcome": "Preserve a private reading ledger"}]}}


def test_exact_scope_retains_provenance_and_rejects_unbound_legacy_input():
    state = scope()
    frozen = {"guide_id": state["id"], "guide_version": 3, "blueprint": deepcopy(state["proposal"]), "brief": deepcopy(state["brief"])}
    receipt = {"guide_id": state["id"], "guide_version": 3, **inputs(state)}
    state["version"] = 4
    state["messages"] = [{"text": "What did the checker find?"}]
    assert frozen_matches(receipt, frozen, state)
    assert receipt["guide_version"] == frozen["guide_version"] == 3
    del frozen["brief"]
    state["brief"] = {}
    receipt.update(inputs(state))
    assert not frozen_matches(receipt, frozen, state)
    state["version"] = 3
    assert frozen_matches(receipt, frozen, state)
    frozen["guide_version"] = 2
    assert not frozen_matches(receipt, frozen, state)


@pytest.mark.parametrize("path,value", [
    (("brief", "requirements", 0, "outcome"), "Also support Telegram approvals"),
    (("brief", "facts", "outcome", "summary"), "A different application"),
    (("proposal", "target"), "gcp"),
    (("proposal", "port"), 18999),
    (("proposal", "storage"), "json"),
    (("proposal", "agent_classes", 0, "model"), "gpt-6-sol"),
    (("proposal", "agent_classes", 0, "instructions"), "A different context and authority"),
    (("proposal", "agent_classes", 0, "tools", "shell"), True),
    (("proposal", "topology", "pattern"), "custom"),
    (("proposal", "gcp", "project_id"), "other-project"),
])
def test_every_canonical_compiler_and_application_input_invalidation(path, value):
    state = scope()
    receipt = {"guide_id": state["id"], "guide_version": 3, **inputs(state)}
    parent = state
    for key in path[:-1]: parent = parent[key]
    parent[path[-1]] = value
    assert not matches(receipt, state)


def test_legacy_package_requires_full_brief_evidence_to_cross_transcript_version():
    state = scope()
    report = {"guide_version": 3, "blueprint_hash": inputs(state)["blueprint_hash"]}
    assert package_matches(report, state)
    state["version"] = 4
    assert not package_matches(report, state)
    report["acceptance"] = {"brief_digest": inputs(state)["brief_hash"]}
    assert package_matches(report, state)
    state["brief"]["requirements"][0]["outcome"] = "A new feature"
    assert not package_matches(report, state)
    report["acceptance"] = "model says accepted"
    assert not package_matches(report, state)


@pytest.mark.parametrize("location", ["report", "acceptance"])
@pytest.mark.parametrize("invalid", ["", "a" * 63, "A" * 64, False, 7, [], {}])
def test_malformed_brief_binding_cannot_become_an_unbound_legacy_package(location, invalid):
    state = scope()
    digest = inputs(state)["brief_hash"]
    report = {"guide_version": state["version"], "blueprint_hash": inputs(state)["blueprint_hash"],
              "build_id": "build", "verification": {"digest": "a" * 64},
              "acceptance": {"brief_digest": digest}}
    if location == "report":
        # A malformed explicit digest cannot fall back to the valid nested one.
        report["brief_hash"] = invalid
    else:
        report["acceptance"]["brief_digest"] = invalid
    before = deepcopy((report, state))
    assert report_brief_hash(report) is None  # Tolerant readers retain their contract.
    assert not package_matches(report, state)
    files = {"implementation-report.json": (json.dumps(report).encode(), 0o644)}
    with pytest.raises(ValueError, match="^The application source report could not be verified\\.$"):
        read_package_source(files)
    assert (report, state) == before


@pytest.mark.parametrize("invalid", ["not acceptance metadata", [], False, 7])
@pytest.mark.parametrize("brief", ["missing", "null", "valid"])
def test_malformed_acceptance_container_refuses_without_erasing_evidence(invalid, brief):
    state = scope()
    report = {"guide_version": state["version"], "blueprint_hash": inputs(state)["blueprint_hash"],
              "acceptance": invalid, "build_id": "build", "verification": {"digest": "a" * 64}}
    if brief != "missing":
        report["brief_hash"] = inputs(state)["brief_hash"] if brief == "valid" else None
    before = deepcopy((report, state))
    assert report_brief_hash(report) == (inputs(state)["brief_hash"] if brief == "valid" else None)
    assert not package_matches(report, state)
    with pytest.raises(ValueError, match="^The application source report could not be verified\\.$"):
        read_package_source({"implementation-report.json": (json.dumps(report).encode(), 0o644)})
    assert (report, state) == before


@pytest.mark.parametrize("kind", ["missing", "null", "nested_null", "report", "acceptance", "null_with_acceptance",
                                  "acceptance_null", "acceptance_empty"])
def test_valid_and_genuinely_absent_brief_bindings_keep_legacy_version_rules(kind):
    state = scope()
    report = {"guide_version": state["version"], "blueprint_hash": inputs(state)["blueprint_hash"],
              "build_id": "build", "verification": {"digest": "a" * 64}}
    if kind in {"null", "null_with_acceptance"}:
        report["brief_hash"] = None
    if kind == "nested_null":
        report["acceptance"] = {"brief_digest": None}
    if kind in {"acceptance_null", "acceptance_empty"}:
        report["acceptance"] = None if kind == "acceptance_null" else {}
    if kind == "report":
        report["brief_hash"] = inputs(state)["brief_hash"]
    if kind in {"acceptance", "null_with_acceptance"}:
        report["acceptance"] = {"brief_digest": inputs(state)["brief_hash"]}
    bound = kind in {"report", "acceptance", "null_with_acceptance"}
    source = read_package_source({"implementation-report.json": (json.dumps(report).encode(), 0o644)})
    assert source["brief_hash"] == (inputs(state)["brief_hash"] if bound else None)
    assert package_matches(report, state) and package_matches(source, state)
    state["version"] += 1
    assert package_matches(report, state) is bound
    assert package_matches(source, state) is bound


def test_current_version_is_only_the_host_response_snapshot():
    state = scope()
    state["version"] = 4
    receipt = {"guide_id": "guide", "guide_version": 3, "current": True}
    assert not current_version(receipt, state)
    receipt["current_guide_version"] = 4
    assert current_version(receipt, state)
    for value in (None, True, "4", 3):
        receipt["current_guide_version"] = value
        assert not current_version(receipt, state)


@pytest.mark.parametrize("value", [True, "1", None])
def test_malformed_receipt_provenance_cannot_equal_frozen_version(value):
    state = scope()
    state["version"] = 1
    frozen = {"guide_id": state["id"], "guide_version": 1, "blueprint": state["proposal"], "brief": state["brief"]}
    receipt = {"guide_id": state["id"], "guide_version": value, **inputs(state)}
    assert not frozen_matches(receipt, frozen, state)


@pytest.mark.parametrize("missing", [None, "not a full brief"])
def test_null_or_non_object_legacy_brief_cannot_prove_cross_revision_scope(missing):
    state = scope()
    state.update(version=4, brief={})
    frozen = {"guide_id": "guide", "guide_version": 3, "blueprint": state["proposal"], "brief": missing}
    receipt = {"guide_id": "guide", "guide_version": 3, **inputs(state)}
    assert not frozen_matches(receipt, frozen, state)


@pytest.mark.parametrize("change", ["replace", "remove"])
def test_organization_bound_build_and_export_cannot_match_changed_context(change):
    state = scope()
    state["organization"] = {"id": "example", "version": "1", "guide_context": "Required identity contract"}
    frozen = {"guide_id": state["id"], "guide_version": 3, "blueprint": deepcopy(state["proposal"]),
              "brief": deepcopy(state["brief"]), "organization": deepcopy(state["organization"])}
    receipt = {"guide_id": state["id"], "guide_version": 3, **inputs(state)}
    assert frozen_matches(receipt, frozen, state)
    assert package_matches(receipt, state)
    if change == "replace": state["organization"]["guide_context"] = "Different identity contract"
    else: del state["organization"]
    assert not matches(receipt, state)
    assert not frozen_matches(receipt, frozen, state)
    assert not package_matches(receipt, state)
    stripped = deepcopy(frozen)
    del stripped["organization"]
    assert not frozen_matches(receipt, stripped, state)


@pytest.mark.parametrize("full_brief", [False, True])
def test_legacy_receipt_cannot_gain_unbound_organization_context(full_brief):
    state = scope()
    receipt = {"guide_id": state["id"], "guide_version": 3, **inputs(state)}
    package = dict(receipt)
    if not full_brief:
        del package["brief_hash"]
    assert matches(receipt, state)
    assert package_matches(package, state)
    state["organization"] = {"id": "example", "version": "1", "guide_context": "Required identity contract"}
    assert not matches(receipt, state)
    assert not package_matches(package, state)
