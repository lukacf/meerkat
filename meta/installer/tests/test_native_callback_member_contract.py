"""Selected native member labels through the genuine Meta callback dispatcher.

Core stamps the exact selected member component into agent_identity. Logical
roster identities stay application-owned and are not a native wire encoding.
No native process, provider, credential or deployment runs in these controls.
"""
import hashlib
import json

import pytest

from meerkat_meta.agent_builder import CallbackDispatcher
from launchkit.connect_application_callbacks import SelectedApplicationCallbacks
from launchkit.connect_installer_caller import CustodyError, ExactFile
from test_application_callbacks import Fixture


@pytest.mark.parametrize("strategy", ["dedicated", "exclusive_application_role"])
@pytest.mark.parametrize("selected_helper", [False, True])
@pytest.mark.asyncio
async def test_selected_native_member_callback_resolves_original_row(
        tmp_path, strategy, selected_helper):
    f = Fixture(tmp_path, strategy=strategy)
    f.observe()
    index = 0 if selected_helper else 1
    logical = f.roster[index]
    native = f.members[index]["member"]
    profile = f.profile if selected_helper else logical["profile"]

    class Builder:
        async def build_agent(self, draft):
            row = f.callbacks.helper_agent(draft, f.by_profile)
            row = row if row is not None else f.by_profile[draft.profile_name]
            draft.register_tool("selected_identity", lambda arguments: row["identity"])

    dispatcher = CallbackDispatcher()
    dispatcher.register_builder(Builder())
    built = await dispatcher.handle_callback("callback/build_agent", {"options": {
        "scope_id": "original-native-scope", "profile_name": profile,
        "labels": {"mob_id": native["mob_id"], "role": profile, "profile_name": profile,
            "agent_identity": native["member"], "meerkat_id": native["member"]}}})
    assert "selected_identity" in built["tools"]
    assert await dispatcher.handle_callback("callback/call_tool", {
        "scope_id": "original-native-scope", "tool": "selected_identity",
        "arguments": {}}) == {"content": logical["identity"]}


@pytest.mark.parametrize("selected_helper", [False, True])
@pytest.mark.parametrize("changed", ["member", "mob", "role", "profile_name", "meerkat_id", "missing"])
@pytest.mark.asyncio
async def test_foreign_native_labels_never_register_callbacks(tmp_path, selected_helper, changed):
    f = Fixture(tmp_path)
    f.observe()
    index = 0 if selected_helper else 1
    row, native = f.roster[index], f.members[index]["member"]
    profile = f.profile if selected_helper else row["profile"]
    labels = {"mob_id": native["mob_id"], "role": profile, "profile_name": profile,
        "agent_identity": native["member"], "meerkat_id": native["member"]}
    if changed == "member":
        labels["agent_identity"] = labels["meerkat_id"] = f.members[1-index]["member"]["member"]
    elif changed == "missing":
        labels.pop("agent_identity")
    else:
        labels[{"mob": "mob_id", "role": "role", "profile_name": "profile_name",
            "meerkat_id": "meerkat_id"}[changed]] = "foreign-selected-value"

    class Builder:
        async def build_agent(self, draft):
            f.callbacks.helper_agent(draft, f.by_profile)
            draft.register_tool("must_not_exist", lambda arguments: "unselected")

    dispatcher = CallbackDispatcher()
    dispatcher.register_builder(Builder())
    with pytest.raises(CustodyError):
        await dispatcher.handle_callback("callback/build_agent", {"options": {
            "scope_id": "refused-native-scope", "profile_name": profile, "labels": labels}})
    with pytest.raises(ValueError, match="no handler registered"):
        await dispatcher.handle_callback("callback/call_tool", {
            "scope_id": "refused-native-scope", "tool": "must_not_exist", "arguments": {}})


@pytest.mark.asyncio
async def test_native_scopes_and_reobserved_helper_keep_their_original_rows(tmp_path):
    f = Fixture(tmp_path)
    f.observe()

    class Builder:
        async def build_agent(self, draft):
            row = f.callbacks.helper_agent(draft, f.by_profile)
            row = row if row is not None else f.by_profile[draft.profile_name]
            draft.register_tool("selected_identity", lambda arguments: row["identity"])

    dispatcher = CallbackDispatcher()
    dispatcher.register_builder(Builder())
    for index, row in enumerate(f.roster):
        native = f.members[index]["member"]
        profile = f.profile if index == 0 else row["profile"]
        await dispatcher.handle_callback("callback/build_agent", {"options": {
            "scope_id": "native-scope-"+str(index), "profile_name": profile,
            "labels": {"mob_id": native["mob_id"], "role": profile,
                "agent_identity": native["member"], "meerkat_id": native["member"]}}})
    f.observe()
    for index, row in enumerate(f.roster):
        assert await dispatcher.handle_callback("callback/call_tool", {
            "scope_id": "native-scope-"+str(index), "tool": "selected_identity",
            "arguments": {}}) == {"content": row["identity"]}


def test_duplicate_selected_native_members_are_ambiguous_before_callbacks(tmp_path):
    f = Fixture(tmp_path)
    f.value["application"]["members"][1]["member"]["member"] = f.members[0]["member"]["member"]
    data = json.dumps(f.value).encode()
    f.path.write_bytes(data)
    with pytest.raises(CustodyError, match="original native profile bindings"):
        SelectedApplicationCallbacks(ExactFile(f.path, hashlib.sha256(data).hexdigest()))
