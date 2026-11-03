"""Bootstrap source joins only; no installation, native process or credentials."""
from copy import deepcopy
import hashlib
import json

import pytest

from launchkit.connect_installer_caller import CustodyError
from launchkit.deployment_application_bootstrap import _plan_source


def inputs(root, *, empty):
    definition = b'''[mob]
id = "selected-app"
[profiles.reader]
model = "selected-model"
[profiles.reusable-reader]
model = "selected-model"
'''
    roster = [] if empty else [{"identity": "reader:main", "profile": "reader"}]
    members = [] if empty else [{"member": {
        "mob_id": "selected-app", "role": "reader", "member": "opaque-native-member"}}]
    policy = {"provider_id": "original-provider", "policy_id": "original-policy", "revision": 1,
        "members": [] if empty else [{"member_identity": "reader:main", "grants": []}]}
    raw = json.dumps(policy).encode()
    native_policy = deepcopy(policy)
    if not empty:
        native_policy["members"][0]["member_identity"] = "opaque-native-member"
    native_policy["source"] = {"source_id": "original-policy-source",
        "source_digest": "sha256:" + hashlib.sha256(raw).hexdigest()}
    plan = {"deployment_root": str(root), "application": {
        "realm": {"context_root": str(root), "state_root": str(root / ".mobkit/state")},
        "definition": {"format": "toml", "text": definition.decode()},
        "roster": roster, "members": members, "tool_policy": native_policy,
        "member_classes": [{"id": "reader", "profile": "reusable-reader",
            "factory": "application:reader", "callbacks": [], "grants": []}]}}
    package = {"config/mob.toml": (definition, None),
        "deployment.json": (json.dumps({"agents": roster}).encode(), None),
        "application/tool-policy.json": (raw, None)}
    return plan, package


@pytest.mark.parametrize("empty", [False, True])
def test_helperless_bootstrap_keeps_exact_original_initial_roster_and_policy(tmp_path, empty):
    plan, package = inputs(tmp_path, empty=empty)
    before = deepcopy(plan)
    _plan_source(plan, package, tmp_path)
    assert plan == before
    assert len(plan["application"]["members"]) == (0 if empty else 1)


@pytest.mark.parametrize("case", ["no-declarations", "null-helper", "invented-member",
    "changed-definition", "changed-policy", "changed-provenance", "changed-root"])
def test_empty_initial_bootstrap_refuses_missing_or_changed_source(tmp_path, case):
    plan, package = inputs(tmp_path, empty=True)
    application = plan["application"]
    if case == "no-declarations":
        del application["member_classes"]
    elif case == "null-helper":
        application["helper"] = None
    elif case == "invented-member":
        application["members"].append({"member": {"mob_id": "selected-app", "role": "reader", "member": "fake"}})
    elif case == "changed-definition":
        application["definition"]["text"] += '\n[profiles.invented-helper]\nmodel = "selected-model"\n'
    elif case == "changed-policy":
        application["tool_policy"]["members"].append({"member_identity": "fake", "grants": []})
    elif case == "changed-provenance":
        application["tool_policy"]["source"]["source_digest"] = "sha256:" + "0" * 64
    else:
        application["realm"]["state_root"] = str(tmp_path / "different-state")
    with pytest.raises(CustodyError):
        _plan_source(plan, package, tmp_path)


def test_helperless_bootstrap_cannot_replace_a_real_admitted_seed(tmp_path):
    plan, package = inputs(tmp_path, empty=False)
    plan["application"]["roster"] = []
    plan["application"]["members"] = []
    with pytest.raises(CustodyError):
        _plan_source(plan, package, tmp_path)
