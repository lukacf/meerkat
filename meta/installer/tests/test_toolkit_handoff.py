"""Pure Toolkit handoff tests; no files, processes, accounts or native runtime."""
import copy
import json

import pytest

from launchkit.blueprint import Connection
from launchkit.compiler import environment_fields
from launchkit.blueprint import starter_blueprint
from launchkit.toolkit_handoff import ToolkitHandoffError, normalize_toolkit_handoff


def job(refs=None):
    # Shipped configured-install shape: backend.py returns this scope and MCP
    # object, alongside the validator's explicit credential reference report.
    return {"id": "4bc3e90f049e46faf650f6e963825999", "kind": "install", "status": "succeeded",
        "family": "services", "result": {"scope": "observed_software_installation_only",
            "mcp_path": "/fictional/installed/ready/mcp.json",
            "receipt_path": "/fictional/installed/ready/installed.json",
            "mcp_configuration": {"mcpServers": {"toolkit-services": {
                "command": "/fictional/installed/runtime/bin/python",
                "args": ["-I", "-B", "-m", "connector", "--config", "/fictional/installed/configuration/selected.json"]}}},
            "credential_environment_references": [] if refs is None else refs}}


def handoff(value):
    return normalize_toolkit_handoff(value, connection_id="calendar", name="Selected calendar")


def row(name="CALENDAR_TOKEN", path=None):
    return {"field_path": ["config", "token_env"] if path is None else path, "environment_variable": name}


def test_configured_job_retains_launch_and_only_explicit_private_slots():
    source = job([row(), row("ACCOUNT_KEY", ["api_key_env"])])
    before = copy.deepcopy(source)
    result = handoff(source)
    connection = result["connection"]
    original = source["result"]["mcp_configuration"]["mcpServers"]["toolkit-services"]
    assert connection["command"] == original["command"] and connection["args"] == original["args"]
    assert connection["env"] == {"CALENDAR_TOKEN": "${CALENDAR_TOKEN}", "ACCOUNT_KEY": "${ACCOUNT_KEY}"}
    assert connection["required_environment"] == ["CALENDAR_TOKEN", "ACCOUNT_KEY"]
    assert all(item["secret"] and item["required"] and item["default"] == "" for item in connection["setup_inputs"])
    assert Connection.model_validate(connection).id == "calendar"
    assert result["provenance"] == {"job_id": source["id"], "scope": "software_only",
        "portability": "current_host_only", "credential_environment_references": before["result"]["credential_environment_references"]}
    assert source == before
    source["result"]["credential_environment_references"][0]["environment_variable"] = "CHANGED"
    original["args"].append("changed")
    assert result["provenance"]["credential_environment_references"][0] == row()
    assert "changed" not in connection["args"]
    assert not {"grants", "authenticated", "connected", "ready"}.intersection(result)


def test_explicit_empty_projection_is_known_without_guessing_env_suffixes():
    source = job()
    source["result"]["mcp_configuration"]["mcpServers"]["toolkit-services"]["args"].extend(["--config", "/fictional/token_env.json"])
    result = handoff(source)
    assert result["connection"]["env"] == {}
    assert result["connection"]["required_environment"] == result["connection"]["setup_inputs"] == []


def test_distinct_fields_share_one_exact_credential_slot():
    result = handoff(job([row(), row(path=["optional_env"])]))
    assert result["connection"]["required_environment"] == ["CALENDAR_TOKEN"]
    assert len(result["connection"]["setup_inputs"]) == 1
    assert len(result["provenance"]["credential_environment_references"]) == 2


@pytest.mark.parametrize("name", [
    "PATH", "HOME", "USER", "LOGNAME", "SHELL", "TMPDIR", "TEMP", "TMP",
    "FILES_ROOT", "SSL_CERT_FILE", "SSL_CERT_DIR", "NODE_OPTIONS", "BASH_ENV", "ENV",
    "OPENAI_API_KEY", "ANTHROPIC_API_KEY", "GEMINI_API_KEY", "GOOGLE_API_KEY",
    "SYSTEMROOT", "VIRTUAL_ENV", "MOBKIT_RPC_GATEWAY_BIN", "PYTHONPATH",
    "PYTHONSTARTUP", "DYLD_INSERT_LIBRARIES", "LD_PRELOAD", "CONSOLE_LOGIN_PASSWORD",
])
def test_host_owned_reference_refuses_entire_handoff_without_mutating_source(name):
    source = job([row(), row(name, ["second_credential_env"])])
    source["result"]["private"] = "PRIVATE_CANARY"
    before = copy.deepcopy(source)
    with pytest.raises(ToolkitHandoffError) as caught:
        handoff(source)
    assert str(caught.value) == (
        "Configure dedicated connector credential names instead of protected host or provider names."
    )
    assert name not in str(caught.value)
    assert "PRIVATE_CANARY" not in str(caught.value)
    assert source == before


def test_dedicated_connector_names_retain_exact_refs_without_provider_collision():
    refs = [row(), row("OPENAI_CONNECTOR_KEY", ["api_key_env"])]
    source = job(refs)
    before = copy.deepcopy(source)
    result = handoff(source)
    connection = result["connection"]
    assert connection["env"] == {
        "CALENDAR_TOKEN": "${CALENDAR_TOKEN}",
        "OPENAI_CONNECTOR_KEY": "${OPENAI_CONNECTOR_KEY}",
    }
    assert connection["required_environment"] == ["CALENDAR_TOKEN", "OPENAI_CONNECTOR_KEY"]
    assert [item["name"] for item in connection["setup_inputs"]] == connection["required_environment"]
    assert all(item["secret"] and item["required"] for item in connection["setup_inputs"])
    assert result["provenance"]["credential_environment_references"] == refs
    spec = starter_blueprint()
    spec.connections.append(Connection.model_validate(connection))
    fields = {field["key"]: field for field in environment_fields(spec)}
    assert fields["OPENAI_CONNECTOR_KEY"]["label"].startswith("Selected calendar:")
    assert fields["OPENAI_API_KEY"]["label"] == "OpenAI API key"
    assert source == before


def test_private_slots_reach_existing_compiler_without_grants():
    spec = starter_blueprint()
    spec.connections.append(Connection.model_validate(handoff(job([row()]))["connection"]))
    field = next(field for field in environment_fields(spec) if field["key"] == "CALENDAR_TOKEN")
    assert field["secret"] and not field["required"]  # Unassigned connection stays optional.
    assert all(not item.tools.mcp for item in spec.agent_classes)


@pytest.mark.parametrize("change", [
    lambda value: value.update(status="running"),
    lambda value: value.update(status="unconfirmed"),
    lambda value: value.update(kind="acquire"),
    lambda value: value.update(id="not-an-owned-job"),
    lambda value: value.update(recovered=True),
    lambda value: value.update(result=None),
    lambda value: value["result"].pop("scope"),
    lambda value: value["result"].update(scope="configuration_only"),
    lambda value: value["result"].pop("credential_environment_references"),
    lambda value: value["result"].update(credential_environment_references=None),
])
def test_nonpositive_or_unknown_handoff_refused(change):
    source = job()
    change(source)
    with pytest.raises(ToolkitHandoffError):
        handoff(source)


@pytest.mark.parametrize("refs", [
    [row("lowercase")], [row("BAD-NAME")], [row("TOKEN VALUE")], [row("_TOKEN")],
    [row(path=[])], [row(path=["config.token_env"])], [row(path=["config", 1])],
    [row(path=["other", "token_env"])], [row(path=["config", "nested", "token_env"])],
    [dict(row(), secret="PRIVATE_CANARY")], [row(), row()],
    [row(f"TOKEN_{index}", [f"field_{index}"]) for index in range(51)],
    [row()] * 257,
])
def test_malformed_or_oversized_credential_projection_refused(refs):
    with pytest.raises(ToolkitHandoffError) as caught:
        handoff(job(refs))
    assert "PRIVATE_CANARY" not in str(caught.value)


@pytest.mark.parametrize("patch", [
    {"env": {"CALENDAR_TOKEN": "PRIVATE_CANARY"}},
    {"env": {"BASE_URL": "https://example.test"}},
    {"env": {"CALENDAR_TOKEN": "${CALENDAR_TOKEN}"}},
    {"url": "https://example.test/mcp"}, {"disabled": True}, {"cwd": "/fictional"},
    {"args": "--config something"}, {"args": [True]},
    {"args": ["--token", "PRIVATE_CANARY"]}, {"command": "python --unsafe"},
])
def test_unsupported_or_secret_bearing_launch_refused_without_disclosure(patch):
    source = job([row()])
    source["result"]["mcp_configuration"]["mcpServers"]["toolkit-services"].update(patch)
    with pytest.raises(ToolkitHandoffError) as caught:
        handoff(source)
    assert "PRIVATE_CANARY" not in str(caught.value)


def test_multiple_servers_and_invalid_desired_identity_refused():
    source = job()
    servers = source["result"]["mcp_configuration"]["mcpServers"]
    servers["another"] = copy.deepcopy(servers["toolkit-services"])
    with pytest.raises(ToolkitHandoffError):
        handoff(source)
    with pytest.raises(ToolkitHandoffError):
        normalize_toolkit_handoff(job(), connection_id="INVALID", name="Selected calendar")


@pytest.mark.parametrize("name", ["", "   ", "Changed\nPRIVATE_CANARY", "x" * 101, None])
def test_invalid_requested_name_refused_without_disclosure(name):
    with pytest.raises(ToolkitHandoffError) as caught:
        normalize_toolkit_handoff(job(), connection_id="calendar", name=name)
    assert "PRIVATE_CANARY" not in str(caught.value)


def test_unprojected_job_metadata_never_returns_to_the_guide():
    source = job([row()])
    source["result"].update(private="PRIVATE_CANARY", account={"token": "PRIVATE_CANARY"})
    source["error"] = "PRIVATE_CANARY"
    assert "PRIVATE_CANARY" not in json.dumps(handoff(source))
