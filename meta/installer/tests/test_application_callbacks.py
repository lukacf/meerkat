"""Inert fixed callback joins; no native construction or grants are simulated."""
from copy import deepcopy
import hashlib
import json
from types import SimpleNamespace

import pytest

from launchkit.connect_application_callbacks import SelectedApplicationCallbacks
from launchkit.connect_installer_caller import CustodyError, ExactFile


class Fixture:
    def __init__(self, root, *, strategy="dedicated", native=False, result_bundles=None):
        self.source_profile = {"model": "selected-model", "provider": "openai",
            "skills": ["research"], "tools": {"comms": True, "shell": False,
                "mcp_servers": [{"name": "original-service", "command": "original-command"}]}}
        if result_bundles is not None:
            self.source_profile["tools"]["rust_bundles"] = list(result_bundles)
        self.definition = {"id": "selected-app", "profiles": {
            "helper-source": deepcopy(self.source_profile),
            "reader-source": {"model": "selected-model", "skills": ["reading"],
                "tools": {"comms": True}}},
            "skills": {"research": {"source": "inline", "content": "Research carefully."},
                "reading": {"source": "inline", "content": "Read original evidence."}}}
        self.roster = [{"identity": "helper:setup", "profile": "helper-source"},
            {"identity": "reader:main", "profile": "reader-source"}]
        self.members = [{"member": {"mob_id": "selected-app", "role": row["profile"],
            "member": "native-member-" + str(index)}} for index, row in enumerate(self.roster)]
        self.by_profile = {row["profile"]: {**row, "classId": "class-" + str(index),
            "tools": {"workspace": index == 1}} for index, row in enumerate(self.roster)}
        source = {"format": "native", "definition": deepcopy(self.definition)} if native else {
            "format": "toml", "text": '''[mob]
id = "selected-app"
[profiles.helper-source]
model = "selected-model"
provider = "openai"
skills = ["research"]
[profiles.helper-source.tools]
comms = true
shell = false
RESULT_BUNDLES
[[profiles.helper-source.tools.mcp_servers]]
name = "original-service"
command = "original-command"
[profiles.reader-source]
model = "selected-model"
skills = ["reading"]
[profiles.reader-source.tools]
comms = true
[skills.research]
source = "inline"
content = "Research carefully."
[skills.reading]
source = "inline"
content = "Read original evidence."
'''.replace("RESULT_BUNDLES", "" if result_bundles is None else
            "rust_bundles = " + json.dumps(result_bundles))}
        self.value = {"schema": 1, "application": {"schema": 4, "definition": source,
            "roster": self.roster, "members": self.members,
            "helper": {"member": deepcopy(self.members[0]["member"])},
            "helper_profile": strategy}}
        self.path = root / "original-plan.json"
        self.path.write_bytes(json.dumps(self.value).encode())
        self.path.chmod(0o600)
        self.pin = ExactFile(self.path, hashlib.sha256(self.path.read_bytes()).hexdigest())
        self.callbacks = SelectedApplicationCallbacks(self.pin)
        self.profile = "native-observed-profile-without-derived-spelling" if strategy == "dedicated" else "helper-source"
        self.observed = deepcopy(self.definition)
        projected = deepcopy(self.source_profile)
        projected["tools"]["mcp_servers"].append({"name": "toolkit-setup", "command": "actual-command",
            "args": ["actual-resource-directory"], "access": {"allowed": ["native-selected-operation"]}})
        bundles = projected["tools"].setdefault("rust_bundles", [])
        if "meerkat_toolkit.setup_results" not in bundles:
            bundles.append("meerkat_toolkit.setup_results")
        self.observed["profiles"][self.profile] = projected
        self.observed["runtime"] = {"native_default": "not reproduced by host"}

    def observe(self, value=None, *, typed=True):
        definition = self.observed if value is None else value
        context = SimpleNamespace(mob_definition=definition) if typed else {"mob_definition": definition}
        self.callbacks.observe_roster_context(context)

    def draft(self, identity="helper:setup", profile=None):
        profile = self.profile if profile is None else profile
        binding = next((member["member"] for row, member in zip(self.roster, self.members)
                        if row["identity"] == identity), None)
        member = binding["member"] if binding is not None else identity
        return SimpleNamespace(profile_name=profile, labels={"mob_id": "selected-app",
            "role": profile, "profile_name": profile, "agent_identity": member,
            "meerkat_id": member})


@pytest.mark.parametrize("native", [False, True])
@pytest.mark.parametrize("typed", [False, True])
def test_observed_helper_uses_original_exact_fixed_callback_row(tmp_path, native, typed):
    f = Fixture(tmp_path, native=native)
    before = deepcopy(f.by_profile)
    f.observe(typed=typed)
    assert f.callbacks.helper_agent(f.draft(), f.by_profile) is f.by_profile["helper-source"]
    assert f.callbacks.helper_profile == f.profile
    assert f.callbacks.native_definition == f.observed
    observed_copy = f.callbacks.native_definition
    observed_copy["profiles"].clear()
    assert f.callbacks.native_definition == f.observed
    assert f.by_profile == before
    f.observe()


def test_native_exclusive_role_keeps_original_callback_mapping(tmp_path):
    f = Fixture(tmp_path, strategy="exclusive_application_role")
    f.observe()
    assert f.callbacks.helper_agent(f.draft(), f.by_profile) is f.by_profile["helper-source"]


@pytest.mark.parametrize("strategy", ["dedicated", "exclusive_application_role"])
@pytest.mark.parametrize("native", [False, True])
@pytest.mark.parametrize("result_bundles", [[], ["original.results"],
    ["original.results", "meerkat_toolkit.setup_results"]])
def test_native_result_bundle_addition_keeps_exact_original_selection(
        tmp_path, strategy, native, result_bundles):
    f = Fixture(tmp_path, strategy=strategy, native=native, result_bundles=result_bundles)
    f.observe()
    assert f.callbacks.native_definition == f.observed
    assert f.callbacks.helper_agent(f.draft(), f.by_profile) is f.by_profile["helper-source"]


def test_native_defaults_in_original_nested_rows_do_not_recreate_native_semantics(tmp_path):
    f = Fixture(tmp_path)
    f.observed["profiles"]["helper-source"]["tools"]["mcp_servers"][0]["native_default"] = None
    f.observed["profiles"][f.profile]["tools"]["mcp_servers"][0]["native_default"] = None
    f.observe()
    assert f.callbacks.helper_agent(f.draft(), f.by_profile) is f.by_profile["helper-source"]


def test_ordinary_fixed_profile_matching_is_preserved_without_helper_observation(tmp_path):
    f = Fixture(tmp_path)
    assert f.callbacks.helper_agent(f.draft("reader:main", "reader-source"), f.by_profile) is None
    with pytest.raises(CustodyError, match="native member labels"):
        f.callbacks.helper_agent(SimpleNamespace(profile_name="reader-source", labels={}), f.by_profile)
    with pytest.raises(CustodyError, match="selected helper"):
        f.callbacks.helper_agent(f.draft(), f.by_profile)


@pytest.mark.parametrize("identity,profile", [
    ("reader:main", None), ("foreign:identity", None),
    ("helper:setup", "helper-source"), ("helper:setup", "reader-source"),
    ("reader:main", "unknown-profile"), ("foreign:identity", "reader-source")])
def test_foreign_identity_or_profile_never_gets_helper_or_fixed_callbacks(tmp_path, identity, profile):
    f = Fixture(tmp_path)
    f.observe()
    with pytest.raises(CustodyError):
        f.callbacks.helper_agent(f.draft(identity, profile), f.by_profile)


@pytest.mark.parametrize("changed", ["mob", "profile", "tools", "skill", "extra-profile",
    "ambiguous", "missing-server", "foreign-result-bundle", "missing-result-bundle"])
def test_changed_or_ambiguous_native_definition_is_refused(tmp_path, changed):
    f = Fixture(tmp_path)
    value = deepcopy(f.observed)
    if changed == "mob":
        value["id"] = "other-app"
    elif changed == "profile":
        value["profiles"]["reader-source"]["model"] = "changed-model"
    elif changed == "tools":
        value["profiles"][f.profile]["tools"]["shell"] = True
    elif changed == "skill":
        value["skills"]["research"]["content"] = "Changed instructions."
    elif changed == "extra-profile":
        value["profiles"]["unrelated"] = {"tools": {}}
    elif changed == "ambiguous":
        value["profiles"]["second-candidate"] = deepcopy(value["profiles"][f.profile])
    elif changed == "missing-server":
        value["profiles"][f.profile]["tools"]["mcp_servers"][-1]["name"] = "foreign-server"
    elif changed == "foreign-result-bundle":
        value["profiles"][f.profile]["tools"]["rust_bundles"].append("foreign.results")
    else:
        value["profiles"][f.profile]["tools"].pop("rust_bundles")
    with pytest.raises(CustodyError):
        f.observe(value)


@pytest.mark.parametrize("changed_field", ["runtime", "mob", "non-json"])
def test_conflicting_later_native_definition_invalidates_original_mapping(tmp_path, changed_field):
    f = Fixture(tmp_path)
    f.observe()
    changed = deepcopy(f.observed)
    if changed_field == "runtime":
        changed["runtime"]["native_default"] = "changed"
    elif changed_field == "mob":
        changed["id"] = "foreign-app"
    else:
        changed["runtime"]["native_default"] = float("nan")
    with pytest.raises(CustodyError):
        f.observe(changed)
    with pytest.raises(CustodyError, match="no longer current"):
        f.callbacks.helper_agent(f.draft(), f.by_profile)
    with pytest.raises(CustodyError, match="no longer current"):
        f.observe()


def test_original_plan_and_helper_callback_identity_must_stay_current(tmp_path):
    f = Fixture(tmp_path)
    f.observe()
    f.by_profile["helper-source"]["identity"] = "foreign:helper"
    with pytest.raises(CustodyError, match="helper callback row changed"):
        f.callbacks.helper_agent(f.draft(), f.by_profile)
    f.path.write_bytes(f.path.read_bytes() + b"\n")
    with pytest.raises(CustodyError):
        f.observe()


def test_no_context_or_definition_cannot_establish_projection(tmp_path):
    f = Fixture(tmp_path)
    with pytest.raises(CustodyError, match="selected mob definition"):
        f.callbacks.observe_roster_context(SimpleNamespace(mob_definition=None))
    with pytest.raises(CustodyError, match="not supplied"):
        _ = f.callbacks.native_definition


def test_repeated_fixed_profiles_are_not_accepted_as_a_finite_callback_map(tmp_path):
    f = Fixture(tmp_path)
    f.value["application"]["roster"][1]["profile"] = "helper-source"
    f.value["application"]["members"][1]["member"]["role"] = "helper-source"
    f.path.write_bytes(json.dumps(f.value).encode())
    pin = ExactFile(f.path, hashlib.sha256(f.path.read_bytes()).hexdigest())
    with pytest.raises(CustodyError, match="original native profile bindings"):
        SelectedApplicationCallbacks(pin)
