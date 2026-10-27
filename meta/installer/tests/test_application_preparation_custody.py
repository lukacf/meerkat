"""Inert driver-plan byte custody controls, never native qualification.

The trusted live-exchange manifest precondition is supplied by these specimens.
Opaque parents are not issued or decoded here. Native projection, candidate
validation, same-owner adoption, Ready and retained continuity remain unproved.
"""
import copy
import hashlib
import json

import pytest

from launchkit.connect_installer_caller import CustodyError
from launchkit.connect_application_policy import completed_application_policy_inputs
from test_connect_completed_application_setup import BundleFixture, encoded, write


def route(name, binding, *, controller=None):
    return {"id": name, "origin": {"kind": "local_console", "member": copy.deepcopy(binding),
        "requester": "opaque-operator", "ingress_actor": "opaque-console"},
        "application_parent": [{"opaque_native_parent": "retained-application"}],
        "controller_parent": controller, "logical_executor": "opaque-executor",
        "represented_subject": None, "source_authority": "opaque-source",
        "authentication_domain": {"opaque_native_domain": "authentication"},
        "event_domain": {"opaque_native_domain": "console"}}


class PreparationFixture(BundleFixture):
    def __init__(self, path):
        super().__init__(path)
        # Use already selected native spellings, not a Python UUID normalizer.
        for selected, completed in zip(self.input_value["members"], self.setup_value["native_members"]):
            selected["logical_owner"] = completed["logical_owner"]
            selected["audience"] = copy.deepcopy(completed["audience"])
        selected_members = self.input_value["members"]
        helper_binding = self.input_value["helper"]["member"]
        owner = {"deployment": "retained-deployment", "components": ["retained-console"],
            "channels": [{"opaque_source_configuration": [False, 4], "member": copy.deepcopy(helper_binding)}],
            "native_host": {"realm": "fixture", "grant_administrator": "opaque-application",
                "remote_operator": "opaque-operator", "ingress_domain": {"opaque_native_domain": "ingress"},
                "members": copy.deepcopy(selected_members),
                "routes": [route("selected-route", selected_members[0]["member"]),
                    route("untouched-route", selected_members[1]["member"], controller=[{"opaque_native_parent": "retained-controller"}]),
                    route("helper-route", helper_binding, controller=[{"opaque_native_parent": "retained-helper-route-controller"}])]}}
        self.plan_value = {"schema": 1, "deployment_root": str(self.root), "owner": owner,
            "startup": {"kind": "restore"},
            "authorities": {"controllers": [
                {"target": {"kind": "original_route", "route": "selected-route"},
                    "issuance_id": "retained-route-controller-id", "controller_route": "opaque-controller-key"},
                {"target": {"kind": "helper"}, "issuance_id": "retained-helper-controller-id",
                    "controller_route": "opaque-helper-controller-key"}],
                "agent_setup": [{"member": copy.deepcopy(selected_members[0]["member"]),
                    "route": "selected-route", "issuance_id": "retained-agent-setup-id"}],
                "helper_report": {"member": copy.deepcopy(helper_binding), "issuance_id": "retained-helper-report-id"}},
            "application": copy.deepcopy(self.input_value),
            "acquisition": {"opaque_selected_bounds": ["find_and_install", "delegate", False, 2]},
            "output": str(self.output), "hold_timeout_ms": 30000}
        self.setup_value["native_members"][0]["setup_capability"] = {
            "route": "selected-route", "parent": {"opaque_native_parent": "actual-agent-setup"}}
        helper = self.setup_value["helper"]
        helper["controller_parent"] = [{"opaque_native_parent": "actual-helper-controller"}]
        helper["report_capability"] = {"parent": [{"opaque_native_parent": "actual-helper-report"}]}
        resolved = copy.deepcopy(owner)
        resolved["native_host"]["members"] = copy.deepcopy([
            {key: value for key, value in member.items() if key != "initial_surface"}
            for member in self.setup_value["native_members"]])
        resolved["channels"][0]["member"] = copy.deepcopy(helper["member"])
        resolved["native_host"]["routes"][0]["controller_parent"] = [{"opaque_native_parent": "actual-original-controller"}]
        resolved["native_host"]["routes"][-1]["origin"]["member"] = copy.deepcopy(helper["member"])
        self.setup_value["native_owner"] = resolved
        self.plan = write(self.root / "original-plan.json", encoded(self.plan_value))
        self.publish()

    def repin_plan(self):
        self.plan = write(self.plan.path, encoded(self.plan_value))

    def retain(self):
        return self.caller.retain_completed_application_preparation(self.manifest,
            preparation_plan=self.plan, source_files=(self.source,))

    def join_members(self):
        self.setup_value["native_owner"]["native_host"]["members"] = copy.deepcopy([
            {key: value for key, value in member.items() if key != "initial_surface"}
            for member in self.setup_value["native_members"]])


def test_same_attempt_plan_and_actual_issued_output_are_retained_without_resolved_input_or_execution(tmp_path):
    f = PreparationFixture(tmp_path)
    before = {path: path.read_bytes() for path in f.root.rglob("*") if path.is_file()}
    bundle = f.retain()
    setup, policy = bundle.authenticate()
    assert setup == encoded(f.setup_value) and policy == f.policy
    assert bundle.preparation_plan == f.plan and bundle.setup_input is None
    assert bundle.pins()["preparation_plan"] == f.plan.pin()
    assert "setup_input" not in bundle.pins()
    assert json.loads(setup)["input_sha256"] != f.plan.sha256
    assert len(bundle.member_surfaces) == len(f.plan_value["application"]["members"])
    assert bundle.source_files == (f.source,)
    assert before == {path: path.read_bytes() for path in f.root.rglob("*") if path.is_file()}
    assert not f.calls and not f.caller.provision_attempt_path.exists()


@pytest.mark.parametrize("identifiers,parent_fields", [
    ({}, ()),
    ({"account_issuance_id": None, "public_issuance_id": None}, ()),
    ({"account_issuance_id": "retained-account-id"}, ("account_parent",)),
    ({"public_issuance_id": "retained-public-id"}, ("public_parent",)),
    ({"account_issuance_id": "retained-account-id", "public_issuance_id": "retained-public-id"},
     ("account_parent", "public_parent")),
], ids=["omitted", "null", "account", "public", "both"])
def test_optional_helper_parents_survive_completed_preparation_and_policy_staging_unchanged(
        tmp_path, identifiers, parent_fields):
    f = PreparationFixture(tmp_path)
    # These distinct opaque references are completed-output specimens, not grants.
    selected_parents = {
        "account_parent": [{"opaque_account_lineage": [True, None, {"native": "account"}]}],
        "public_parent": [{"opaque_public_lineage": [False, 7, {"native": "public"}]}],
    }
    f.plan_value["authorities"]["helper_report"].update(identifiers)
    f.setup_value["helper"]["report_capability"].update({
        field: copy.deepcopy(selected_parents[field]) for field in parent_fields})
    expected_request = copy.deepcopy(f.plan_value["authorities"]["helper_report"])
    expected_capability = copy.deepcopy(f.setup_value["helper"]["report_capability"])
    f.plan_value["acquisition"] = {
        "backend_generation": "retained-workgraph-generation",
        "proposal_ceiling": {"actions": [], "services": [], "frontends": [],
            "output_audiences": ["retained-audience"]},
        "output_audience": ["retained-audience"],
        "limits": {"setup_timeout_ms": 30000, "max_workers": 4,
            "max_effect_attempts": 2, "admission_timeout_ms": 10000}}
    f.repin_plan()
    f.publish()
    source_policy = write(f.root / "policy.json", b'{"opaque_source_policy":"retained"}\n')
    before = {path: path.read_bytes() for path in f.root.rglob("*") if path.is_file()}
    bundle = f.caller.retain_completed_application_preparation(f.manifest,
        preparation_plan=f.plan, source_files=(f.source, source_policy))
    setup_bytes, _ = bundle.authenticate()
    assert setup_bytes == encoded(f.setup_value)
    assert json.loads(setup_bytes)["helper"]["report_capability"] == expected_capability
    assert json.loads(bundle.preparation_plan.read())["authorities"]["helper_report"] == expected_request

    staged = completed_application_policy_inputs(bundle, source_policy)
    staged_setup = next(pin for pin in staged["source_artifacts"] if pin == bundle.setup)
    assert staged_setup.read() == setup_bytes
    assert json.loads(staged_setup.read())["helper"]["report_capability"] == expected_capability
    acquisition_bytes = staged["acquisition"].as_bytes()
    acquisition = json.loads(acquisition_bytes)
    assert acquisition == {"schema": 1, **f.plan_value["acquisition"],
        "prepared_application_setup": {"name": staged_setup.path.name, "sha256": staged_setup.sha256}}
    assert json.loads(staged["local_policy_input"])["native_host"]["setup"] == {
        "name": staged["acquisition"].path, "sha256": hashlib.sha256(acquisition_bytes).hexdigest()}
    assert before == {path: path.read_bytes() for path in f.root.rglob("*") if path.is_file()}
    assert not f.calls and not f.caller.provision_attempt_path.exists()


@pytest.mark.parametrize("change", ["missing_owner", "owner_setup_join", "mutual_destination_change",
    "removed_original", "original_source_change", "unselected_parent_change", "channel_source_change",
    "helper_choice_change", "unrequested_controller", "replaced_configured_parent", "setup_route_change", "unrequested_setup",
    "unrequested_report", "changed_helper_strategy", "changed_raw_definition"])
def test_driver_configuration_counterexamples_refuse_even_when_output_manifest_is_repinned(tmp_path, change):
    f = PreparationFixture(tmp_path)
    native = f.setup_value["native_owner"]["native_host"]
    if change == "missing_owner":
        del f.setup_value["native_owner"]
    elif change == "owner_setup_join":
        native["members"][0]["setup_capability"]["parent"] = {"opaque_native_parent": "other"}
    elif change == "mutual_destination_change":
        f.setup_value["native_members"][0]["audience"] = {"opaque_native_audience": "other"}
        f.join_members()
    elif change == "removed_original":
        native["routes"].pop(1)
    elif change == "original_source_change":
        native["routes"][1]["event_domain"] = {"opaque_native_domain": "other"}
    elif change == "unselected_parent_change":
        native["routes"][1]["application_parent"] = [{"opaque_native_parent": "other"}]
    elif change == "channel_source_change":
        f.setup_value["native_owner"]["channels"][0]["opaque_source_configuration"] = [True, 4]
    elif change == "helper_choice_change":
        f.setup_value["helper"]["operation_basis"] = {"opaque_native_selection": "other"}
    elif change == "unrequested_controller":
        f.plan_value["authorities"]["controllers"] = []
        f.repin_plan()
    elif change == "replaced_configured_parent":
        f.plan_value["owner"]["native_host"]["routes"][0]["controller_parent"] = [{"opaque_native_parent": "already-selected"}]
        f.repin_plan()
    elif change == "setup_route_change":
        f.setup_value["native_members"][0]["setup_capability"]["route"] = "untouched-route"
        f.join_members()
    elif change == "unrequested_setup":
        f.setup_value["native_members"][1]["setup_capability"] = copy.deepcopy(f.setup_value["native_members"][0]["setup_capability"])
        f.join_members()
    elif change == "unrequested_report":
        f.plan_value["authorities"]["helper_report"] = None
        f.repin_plan()
    elif change == "changed_helper_strategy":
        f.setup_value["helper_tools"] = "inert_acquisition"
    else:
        f.setup_value["definition_source"]["text"] = "id = 'fixture'\n"
    f.publish()
    with pytest.raises(CustodyError):
        f.retain()
    assert not f.calls and not f.caller.provision_attempt_path.exists()


@pytest.mark.parametrize("change", ["plan", "source", "handoff", "surface", "currentness", "plan_parent_privacy"])
def test_driver_plan_reuse_reauthenticates_original_custody_and_owner(tmp_path, change):
    f = PreparationFixture(tmp_path)
    bundle = f.retain()
    if change == "currentness":
        f.current = False
    elif change == "plan_parent_privacy":
        f.root.chmod(0o755)
    else:
        path = {"plan": f.plan.path, "source": f.source.path, "handoff": f.handoff.path,
            "surface": bundle.member_surfaces[0][0].path}[change]
        write(path, path.read_bytes() + b" ")
    with pytest.raises(CustodyError):
        bundle.authenticate()
    assert not f.calls


def test_plan_output_locator_cannot_be_substituted_by_a_repinned_manifest(tmp_path):
    f = PreparationFixture(tmp_path)
    f.plan_value["output"] = str(f.root / "different-output")
    f.repin_plan()
    with pytest.raises(CustodyError, match="output path"):
        f.retain()
    assert not f.calls


def test_configured_parents_remain_exact_without_any_new_authority_request(tmp_path):
    f = PreparationFixture(tmp_path)
    selected = f.plan_value["owner"]["native_host"]
    selected["members"][0]["setup_capability"] = copy.deepcopy(f.setup_value["native_members"][0]["setup_capability"])
    selected["routes"][0]["controller_parent"] = copy.deepcopy(f.setup_value["native_owner"]["native_host"]["routes"][0]["controller_parent"])
    f.plan_value["application"]["members"] = copy.deepcopy(selected["members"])
    for field in ("controller_parent", "report_capability"):
        f.plan_value["application"]["helper"][field] = copy.deepcopy(f.setup_value["helper"][field])
    f.plan_value["authorities"] = {"controllers": [], "agent_setup": [], "helper_report": None}
    f.repin_plan()
    bundle = f.retain()
    assert bundle.preparation_plan == f.plan
    assert json.loads(bundle.authenticate()[0])["native_owner"] == f.setup_value["native_owner"]
    assert not f.calls


def test_driver_and_distinct_schema4_inputs_have_separate_entries(tmp_path):
    f = PreparationFixture(tmp_path)
    with pytest.raises(CustodyError, match="distinct schema4"):
        f.caller.retain_completed_application_setup(f.manifest, setup_input=f.plan, source_files=(f.source,))
    with pytest.raises(CustodyError, match="output path"):
        f.caller.retain_completed_application_preparation(f.manifest, preparation_plan=f.input, source_files=(f.source,))
    assert not f.calls


def test_legacy_distinct_schema4_retention_still_omits_driver_requirements(tmp_path):
    f = BundleFixture(tmp_path)
    bundle = f.retain()
    assert bundle.setup_input == f.input and bundle.preparation_plan is None
    assert "native_owner" not in f.setup_value
    assert "preparation_plan" not in bundle.pins()
    assert not f.calls


def test_plan_limit_is_enforced_before_completed_custody(tmp_path):
    f = PreparationFixture(tmp_path)
    f.plan = write(f.plan.path, b" " * (256 * 1024 + 1))
    with pytest.raises(CustodyError, match="artifact limit"):
        f.retain()
    assert not f.calls


@pytest.mark.parametrize("change", ["plan_bytes", "plan_parent_privacy"])
def test_original_plan_custody_cannot_change_after_initial_observation(tmp_path, change):
    f = PreparationFixture(tmp_path)
    def change_after_reads():
        if f.checks == 3:
            if change == "plan_bytes":
                write(f.plan.path, f.plan.path.read_bytes() + b" ")
            else:
                f.plan.path.parent.chmod(0o755)
    f.on_check = change_after_reads
    with pytest.raises(CustodyError, match="selected bytes|parent directory"):
        f.retain()
    assert f.checks == 3 and not f.calls
    assert not f.caller.provision_attempt_path.exists()


def test_preparation_reuse_retains_completed_directory_identity_even_with_identical_bytes(tmp_path):
    f = PreparationFixture(tmp_path)
    bundle = f.retain()
    retained = f.root / "retained-completed"
    f.output.rename(retained)
    f.output.mkdir(mode=0o700)
    for path in retained.iterdir():
        write(f.output / path.name, path.read_bytes())
    with pytest.raises(CustodyError, match="directories changed"):
        bundle.authenticate()
    assert not f.calls and not f.caller.provision_attempt_path.exists()


def test_preparation_final_surface_observation_cannot_outlive_the_existing_owner(tmp_path, monkeypatch):
    import launchkit.connect_installer_caller as caller_module

    f = PreparationFixture(tmp_path)
    original_read = caller_module._completed_surface
    reads = 0
    def invalidate_after_last_read(pin):
        nonlocal reads
        content = original_read(pin)
        reads += 1
        if reads == 4 * len(f.setup_value["native_members"]):
            f.current = False
        return content
    monkeypatch.setattr(caller_module, "_completed_surface", invalidate_after_last_read)
    with pytest.raises(CustodyError, match="existing selected source or stopped-owner lease changed"):
        f.retain()
    assert reads == 4 * len(f.setup_value["native_members"])
    assert not f.current and not f.calls and not f.caller.provision_attempt_path.exists()


def test_non_helper_provider_route_and_channel_source_survive_helper_projection_exactly(tmp_path):
    f = PreparationFixture(tmp_path)
    original = f.plan_value["owner"]
    origin = {"kind": "provider_event", "channel_id": "selected-channel",
        "sender_id": "selected-sender", "target_identity": "original-provider-target"}
    original["native_host"]["routes"][1]["origin"] = copy.deepcopy(origin)
    original["channels"].append({"component": "retained-console", "channel_id": origin["channel_id"],
        "sender_id": origin["sender_id"], "target_identity": origin["target_identity"],
        "member": copy.deepcopy(original["native_host"]["members"][1]["member"]),
        "requester": "opaque-provider-user", "ingress_actor": "opaque-provider-source",
        "service_subject": "retained-subject", "provider_account": "retained-account",
        "realm": "fixture", "required_surfaces": ["retained-surface"]})
    resolved = f.setup_value["native_owner"]
    resolved["native_host"]["routes"][1]["origin"] = copy.deepcopy(origin)
    resolved["channels"].append(copy.deepcopy(original["channels"][-1]))
    f.repin_plan()
    f.publish()
    setup = json.loads(f.retain().authenticate()[0])
    assert setup["native_owner"]["native_host"]["routes"][1] == original["native_host"]["routes"][1]
    assert setup["native_owner"]["channels"][-1] == original["channels"][-1]
    assert setup["native_owner"]["channels"][0]["member"] == setup["helper"]["member"]
    assert not f.calls and not f.caller.provision_attempt_path.exists()
