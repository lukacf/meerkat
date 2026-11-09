"""Original helperless callback joins, without native construction or admission."""
from copy import deepcopy

import pytest

from launchkit.connect_installer_caller import CustodyError
from test_selected_member_classes import ClassFixture, startup_fixture


class HelperlessFixture(ClassFixture):
    def __init__(self, root, *, empty=False):
        super().__init__(root)
        application = self.value["application"]
        del application["helper"]
        del application["helper_profile"]
        self.roster = application["roster"] = [] if empty else self.roster[1:]
        self.members = application["members"] = [] if empty else self.members[1:]
        del self.definition["profiles"]["helper-source"]
        application["definition"]["definition"] = deepcopy(self.definition)
        self.by_profile = {name: row for name, row in self.by_profile.items()
            if any(item["identity"] == row["identity"] for item in self.roster)}
        self.deployment["agents"] = [row for row in self.deployment["agents"]
            if any(item["identity"] == row["identity"] for item in self.roster)]
        self.observed = deepcopy(self.definition)
        self.observed["runtime"] = {"native_default": "not reproduced by host"}
        self.select()


@pytest.mark.parametrize("empty", [False, True])
def test_helperless_native_observation_keeps_real_seeds_and_zero_instance_class(tmp_path, empty):
    f = HelperlessFixture(tmp_path, empty=empty)
    assert f.callbacks.helper_identity is f.callbacks.helper_profile is None
    assert set(f.callbacks.dynamic_catalog(f.deployment)) == {"reader"}
    assert len(f.callbacks._fixed) == (0 if empty else 1)
    draft = f.draft("opaque-new-member", "reusable-reader")
    with pytest.raises(CustodyError):
        f.callbacks.helper_agent(draft, f.by_profile)
    f.observe()
    assert f.callbacks.native_definition == f.observed
    assert f.callbacks.helper_agent(draft, f.by_profile) is None
    for row in f.roster:
        assert f.callbacks.project_identity_spec(row) == row
        assert f.callbacks.helper_agent(f.draft(row["identity"], row["profile"]), f.by_profile) is None
    proposed = {"identity": "reader:new", "profile": "reusable-reader"}
    assert f.callbacks.project_identity_spec(proposed) == proposed
    assert proposed == {"identity": "reader:new", "profile": "reusable-reader"}


def test_helperless_fixed_only_selection_needs_no_class_or_helper(tmp_path):
    f = HelperlessFixture(tmp_path)
    del f.value["application"]["member_classes"]
    f.select()
    f.observe()
    assert f.callbacks.member_classes == {}
    assert f.callbacks.helper_agent(f.draft("reader:main", "reader-source"), f.by_profile) is None


@pytest.mark.parametrize("case", ["empty-without-classes", "null-helper", "null-strategy",
    "orphan-tools", "orphan-strategy", "orphan-helper"])
def test_helperless_callback_selection_refuses_missing_or_partial_source(tmp_path, case):
    f = HelperlessFixture(tmp_path, empty=True)
    application = f.value["application"]
    if case == "empty-without-classes":
        application.pop("member_classes")
    elif case == "null-helper":
        application["helper"] = None
    elif case == "null-strategy":
        application["helper_profile"] = None
    elif case == "orphan-tools":
        application["helper_tools"] = "preparation"
    elif case == "orphan-strategy":
        application["helper_profile"] = "dedicated"
    else:
        application["helper"] = {"member": {"mob_id": "selected-app", "role": "reader-source",
            "member": "invented-helper"}}
    with pytest.raises(CustodyError):
        f.select()


@pytest.mark.parametrize("case", ["missing-profile", "extra-profile", "model", "skill", "server", "result-bundle"])
def test_helperless_native_observation_cannot_project_or_replace_original_profiles(tmp_path, case):
    f = HelperlessFixture(tmp_path)
    source_tools = f.value["application"]["definition"]["definition"]["profiles"]["reader-source"]["tools"]
    source_tools.update(mcp_servers=[], rust_bundles=[])
    f.observed["profiles"]["reader-source"]["tools"].update(mcp_servers=[], rust_bundles=[])
    f.select()
    changed = deepcopy(f.observed)
    if case == "missing-profile":
        del changed["profiles"]["reusable-reader"]
    elif case == "extra-profile":
        changed["profiles"]["invented-helper"] = deepcopy(changed["profiles"]["reader-source"])
    elif case == "model":
        changed["profiles"]["reader-source"]["model"] = "other-model"
    elif case == "skill":
        changed["skills"]["reading"]["content"] = "Different instructions."
    elif case == "server":
        changed["profiles"]["reader-source"]["tools"]["mcp_servers"].append({"name": "toolkit-setup"})
    else:
        changed["profiles"]["reader-source"]["tools"]["rust_bundles"].append("meerkat_toolkit.setup_results")
    with pytest.raises(CustodyError):
        f.observe(changed)


@pytest.mark.parametrize("case", ["foreign-seed", "seed-as-reusable", "wrong-mob", "wrong-role", "wrong-alias"])
def test_helperless_callbacks_keep_exact_native_identity_labels(tmp_path, case):
    f = HelperlessFixture(tmp_path)
    f.observe()
    draft = f.draft("reader:main", "reader-source")
    if case == "foreign-seed":
        draft.labels["agent_identity"] = draft.labels["meerkat_id"] = "foreign-member"
    elif case == "seed-as-reusable":
        draft.profile_name = draft.labels["profile_name"] = draft.labels["role"] = "reusable-reader"
    elif case == "wrong-mob":
        draft.labels["mob_id"] = "other-app"
    elif case == "wrong-role":
        draft.labels["role"] = "other-role"
    else:
        draft.labels["meerkat_id"] = "other-member"
    with pytest.raises(CustodyError):
        f.callbacks.helper_agent(draft, f.by_profile)


@pytest.mark.parametrize("case", ["definition", "plan"])
def test_helperless_callback_observation_remains_current(tmp_path, case):
    f = HelperlessFixture(tmp_path, empty=True)
    f.observe()
    if case == "definition":
        changed = deepcopy(f.observed)
        changed["runtime"]["native_default"] = "changed"
        with pytest.raises(CustodyError):
            f.observe(changed)
    else:
        f.path.write_bytes(f.path.read_bytes() + b" ")
    with pytest.raises(CustodyError):
        f.callbacks.dynamic_catalog(f.deployment)


def test_helperless_actual_class_callbacks_must_equal_selected_declarations(tmp_path):
    f = HelperlessFixture(tmp_path, empty=True)
    declaration = f.declaration["callbacks"][0]
    actual = {"name": declaration["name"], "description": declaration["description"],
        "schema": declaration["input_schema"], "handler": lambda value: value}
    agent = {"identity": "reader:new", "classId": "reader", "profile": "reusable-reader"}
    f.callbacks.require_member_callbacks(agent, [actual])
    for changed in [[], [{**actual, "name": "foreign"}], [{**actual, "schema": {}}]]:
        with pytest.raises(CustodyError):
            f.callbacks.require_member_callbacks(agent, changed)


@pytest.mark.parametrize("mode", ["fresh", "restored"])
@pytest.mark.asyncio
async def test_helperless_startup_keeps_original_dynamic_application_owners(tmp_path, monkeypatch, mode):
    f = startup_fixture(tmp_path, monkeypatch, mode, helperless=True)
    await f.owner.prepare(f.app)
    assert f.owner.callbacks.helper_identity is None
    config = f.owner.builder_config(roster_provider=f.app, topology_provider=f.app,
        agent_customizer=f.app, persistent_state=str(f.root / ".mobkit/state"))
    runtime = await f.owner.connect(config, f.app)
    assert runtime is f.owner.runtime is f.app.context.runtime
    assert config.roster_provider is config.topology_provider is config.agent_customizer is f.app
    assert f.app._application_callbacks is f.owner.callbacks
    await f.owner.shutdown(runtime)
