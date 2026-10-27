import json

import pytest

from launchkit import connector_discovery as discovery
from launchkit.blueprint import Connection


def entry(**parts):
    return {"server": {"name": "io.github.example/files", "version": "1.2.3", "description": "A file connector", **parts}, "_meta": {"io.modelcontextprotocol.registry/official": {"status": "active"}}}


def test_npm_is_exact_and_config_inputs_stay_local():
    result = discovery.normalize_registry_entry(entry(packages=[{"registryType": "npm", "identifier": "@example/files", "version": "1.2.3", "transport": {"type": "stdio"}, "packageArguments": [{"type": "positional", "valueHint": "root", "isRequired": True}], "environmentVariables": [{"name": "SECRET_TOKEN", "isSecret": True, "value": "never-echo-this"}]}]))
    connection = result["connection"]
    assert connection["command"] == "npx"
    assert connection["args"] == ["--yes", "@example/files@1.2.3", "${ROOT}"]
    assert list(connection["env"]) == ["SECRET_TOKEN"]
    assert connection["env"]["SECRET_TOKEN"].endswith("_SECRET_TOKEN}")
    assert "never-echo-this" not in json.dumps(result)
    assert result["source_url"].endswith("io.github.example%2Ffiles/versions/1.2.3")
    Connection.model_validate(connection)


def test_remote_headers_variables_and_blueprint_compatibility():
    result = discovery.normalize_registry_entry(entry(remotes=[{"type": "streamable-http", "url": "https://example.org/{tenant}/mcp", "variables": {"tenant": {"isRequired": True}}, "headers": [{"name": "Authorization", "value": "Bearer {access_token}", "isSecret": True}, {"name": "X-Mode", "value": "readonly"}]}]))
    conn = result["connection"]
    assert conn["headers"]["Authorization"].startswith("Bearer ${")
    assert conn["headers"]["Authorization"].endswith("_ACCESS_TOKEN}")
    assert conn["url"].startswith("https://example.org/${")
    assert conn["url"].endswith("_TENANT}/mcp")
    assert any(i["name"].endswith("_TENANT") for i in result["required_environment"])
    assert any(i["name"].endswith("_ACCESS_TOKEN") for i in result["required_environment"])
    Connection.model_validate(conn)


def test_hyphenated_remote_variable_becomes_an_exact_required_input():
    result = discovery.normalize_registry_entry(entry(remotes=[{
        "type": "streamable-http", "url": "https://{baseUrl}/mcp/{server-name}/{server-name}",
        "variables": {"baseUrl": {"isRequired": True},
                      "server-name": {"isRequired": True, "description": "The MCP server name"}},
    }]))
    assert result["installable"] is True
    conn = result["connection"]
    host, server = result["required_environment"]
    assert host["name"].endswith("_BASEURL")
    assert server["name"].endswith("_SERVER_NAME")
    assert server["description"] == "The MCP server name"
    assert host["required"] is server["required"] is True
    assert conn["required_environment"] == [host["name"], server["name"]]
    assert conn["url"] == ("https://${" + host["name"] + "}/mcp/${" + server["name"]
                           + "}/${" + server["name"] + "}")
    Connection.model_validate(conn)


@pytest.mark.parametrize("path", ["{server.name}", "{server", "server}", "${lower_case}"])
def test_unresolved_remote_placeholders_are_not_installable(path):
    result = discovery.normalize_registry_entry(entry(remotes=[{
        "type": "streamable-http", "url": "https://example.org/" + path,
    }]))
    assert result["installable"] is False
    assert result["connection"] is None
    assert any("unresolved placeholder" in warning for warning in result["warnings"])


def test_different_remote_variables_cannot_share_a_normalized_input():
    result = discovery.normalize_registry_entry(entry(remotes=[{
        "type": "streamable-http", "url": "https://example.org/{server-name}/{server_name}",
        "variables": {"server-name": {"isRequired": True}, "server_name": {"isRequired": True}},
    }]))
    assert result["installable"] is False
    assert result["connection"] is None
    assert any("same environment input" in warning for warning in result["warnings"])


def test_custom_url_requires_resolved_or_explicit_environment_placeholders():
    with pytest.raises(discovery.DiscoveryError, match="unresolved placeholder"):
        discovery.normalize_custom_config({"url": "https://example.org/mcp/{server-name}"})
    result = discovery.normalize_custom_config({"url": "https://${MCP_HOST}/mcp/${MCP_SERVER_NAME}"})
    conn = result["connections"][0]
    assert conn["url"] == "https://${MCP_HOST}/mcp/${MCP_SERVER_NAME}"
    Connection.model_validate(conn)


def test_github_auth_is_from_provider_contract():
    result = discovery.normalize_registry_entry(entry(name="io.github.github/github-mcp-server", remotes=[{"type": "streamable-http", "url": "https://api.githubcopilot.com/mcp/", "headers": [{"name": "Authorization", "isSecret": True}]}]))
    assert result["auth"]["kind"] == "bearer"
    assert result["connection"]["headers"]["Authorization"] == "Bearer ${GITHUB_PERSONAL_ACCESS_TOKEN}"
    assert len(result["required_environment"]) == 1


def test_notion_oauth_is_not_claimed_ready():
    result = discovery.normalize_registry_entry(entry(name="com.notion/mcp", remotes=[{"type": "streamable-http", "url": "https://mcp.notion.com/mcp"}]))
    assert result["installable"] is False
    assert result["connection"] is None
    assert "OAuth" in result["warnings"][0]


def test_oci_environment_inherited_and_image_pinned():
    result = discovery.normalize_registry_entry(entry(packages=[{"registryType": "oci", "identifier": "ghcr.io/example/files:1.2.3", "transport": {"type": "stdio"}, "environmentVariables": [{"name": "TOKEN", "isSecret": True}]}]))
    assert result["connection"]["args"] == ["run", "--rm", "-i", "-e", "TOKEN", "ghcr.io/example/files:1.2.3"]
    Connection.model_validate(result["connection"])


@pytest.mark.parametrize("package", [
    {"registryType": "npm", "identifier": "files", "version": "latest", "transport": {"type": "stdio"}},
    {"registryType": "npm", "identifier": "https://untrusted/install.js", "version": "1.0", "transport": {"type": "stdio"}},
    {"registryType": "oci", "identifier": "ghcr.io/example/files:latest", "transport": {"type": "stdio"}},
    {"registryType": "oci", "identifier": "bad image", "version": "1.0", "transport": {"type": "stdio"}},
    {"registryType": "mcpb", "identifier": "https://example.org/package.mcpb", "transport": {"type": "stdio"}},
])
def test_unsupported_packages_are_visible_but_not_installable(package):
    result = discovery.normalize_registry_entry(entry(packages=[package]))
    assert result["connection"] is None
    assert result["warnings"]


def test_inactive_record_cannot_generate_connection():
    record = entry(remotes=[{"type": "streamable-http", "url": "https://example.org/mcp"}])
    record["_meta"]["io.modelcontextprotocol.registry/official"]["status"] = "deleted"
    assert not discovery.normalize_registry_entry(record)["installable"]


def test_import_strips_environment_and_header_credentials():
    result = discovery.normalize_custom_config({"mcpServers": {
        "Files": {"command": "npx", "args": ["-y", "@example/files@1.2.3"], "env": {"TOKEN": "secret-token-value", "ROOT": "/private/user/path"}},
        "Hosted": {"url": "https://example.org/mcp", "headers": {"Authorization": "Bearer secret-header-value", "X-Mode": "read"}},
    }})
    output = json.dumps(result)
    assert all(value not in output for value in ["secret-token-value", "secret-header-value", "/private/user/path"])
    for conn in result["connections"]:
        Connection.model_validate(conn)
    assert result["warnings"]


@pytest.mark.parametrize("config", [
    {"url": "https://user:password@example.org/mcp"},
    {"url": "http://remote.example.org/mcp"},
    {"url": "https://example.org/mcp?token=secret"},
    {"command": "npx", "args": ["--token=secret"]},
    {"command": "npx", "args": ["postgresql://user:secret@database.example.org/db"]},
    {"command": "npx", "args": ["--token", "secret"]},
    {"command": "npx", "args": ["--header", "Authorization: Bearer secret"]},
    {"command": "npx -y"},
    {"url": "https://example.org/mcp", "headers": {"bad\rname": "secret"}},
])
def test_invalid_or_secret_inline_config_rejected(config):
    with pytest.raises(discovery.DiscoveryError):
        discovery.normalize_custom_config(config)


def test_custom_existing_references_are_preserved():
    result = discovery.normalize_custom_config({"servers": {"github": {"url": "https://example.org/mcp", "headers": {"Authorization": "Bearer ${MY_TOKEN}"}}}})
    assert result["connections"][0]["headers"]["Authorization"] == "Bearer ${MY_TOKEN}"
    assert result["required_environment"][0]["name"] == "MY_TOKEN"


@pytest.mark.asyncio
async def test_search_is_live_latest_and_prefers_verified_publisher(monkeypatch):
    paths = []
    official = entry(name="io.github.github/github-mcp-server", remotes=[{"type": "streamable-http", "url": "https://api.githubcopilot.com/mcp/"}])
    def fetch(path):
        paths.append(path)
        return official if "/versions/latest" in path else {"servers": [entry(), official], "metadata": {"nextCursor": "more"}}
    monkeypatch.setattr(discovery, "_fetch_json", fetch)
    result = await discovery.search_registry("github")
    assert result["results"][0]["registry_id"] == official["server"]["name"]
    assert len(result["results"]) == 2
    assert any("version=latest" in path for path in paths)
    assert result["warnings"]


@pytest.mark.asyncio
async def test_registry_outage_does_not_return_fake_catalog(monkeypatch):
    def fail(path):
        raise discovery.DiscoveryError("offline")
    monkeypatch.setattr(discovery, "_fetch_json", fail)
    with pytest.raises(discovery.DiscoveryError, match="offline"):
        await discovery.search_registry("database")


def test_two_connectors_do_not_share_generic_token_inputs():
    remote = {"type": "streamable-http", "url": "https://example.org/mcp", "headers": [{"name": "Authorization", "value": "Bearer {token}", "isSecret": True}]}
    first = discovery.normalize_registry_entry(entry(name="io.github.one/service", remotes=[remote]))
    second = discovery.normalize_registry_entry(entry(name="io.github.two/service", remotes=[remote]))
    assert first["connection"]["headers"] != second["connection"]["headers"]
