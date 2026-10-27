import copy

import pytest
from pydantic import ValidationError

from launchkit.blueprint import Blueprint, Connection, from_v1, starter_blueprint
from launchkit.planning_limits import MAX_IDENTITIES, MAX_INSTANCES_PER_CLASS, MAX_PEER_EDGES


def test_starter_resolves_exact_instances_and_edges():
    value = starter_blueprint()
    assert value.identities() == ["lead:main", "research:1", "research:2", "review:main"]
    assert len(value.resolved_edges()) == 3
    data = value.model_dump()
    data["topology"]["mode"] = "mesh"
    assert len(Blueprint.model_validate(data).resolved_edges()) == 6
    data["topology"]["mode"] = "pipeline"
    assert [(e.a, e.b) for e in Blueprint.model_validate(data).resolved_edges()] == [("lead:main", "research:1"), ("research:1", "research:2"), ("research:2", "review:main")]


@pytest.mark.parametrize("mutate", [
    lambda d: d["agent_classes"][1].update(id="lead"),
    lambda d: d["topology"].update(entrypoint="unknown:main"),
    lambda d: d["topology"].update(edges=[{"a": "lead:main", "b": "lead:main"}]),
    lambda d: d["topology"].update(edges=[{"a": "lead:main", "b": "research:1"}, {"a": "research:1", "b": "lead:main"}]),
    lambda d: d["agent_classes"][0]["tools"].update(mcp=[{"connection_id": "missing"}]),
    lambda d: d["agent_classes"][0]["tools"].update(knowledge=True),
    lambda d: d["agent_classes"][0].update(provider="anthropic"),
    lambda d: d["flows"][0]["steps"][0].update(depends_on=["synthesize"]),
    lambda d: d["flows"][0]["steps"][0].update(depends_on=["missing"]),
    lambda d: d["flows"][0]["steps"][0].update(agent="missing:main"),
    lambda d: d["flows"][0]["steps"][0].update(timeout_ms=True),
    lambda d: d["agent_classes"][0].update(instances=True),
])
def test_invalid_blueprints_reject_at_boundary(mutate):
    data = starter_blueprint().model_dump()
    mutate(data)
    with pytest.raises(ValidationError):
        Blueprint.model_validate(data)


def test_reject_total_population_limit():
    data = starter_blueprint().model_dump()
    base = copy.deepcopy(data["agent_classes"][0])
    data["agent_classes"] = [{**base, "id": f"class{i}", "instances": MAX_INSTANCES_PER_CLASS}
                             for i in range(MAX_IDENTITIES // MAX_INSTANCES_PER_CLASS + 1)]
    data["topology"]["entrypoint"] = "class0:1"
    data["flows"] = []
    with pytest.raises(ValidationError, match=f"{MAX_IDENTITIES} durable"):
        Blueprint.model_validate(data)


def test_large_roster_keeps_every_identity_and_bounds_mesh_before_expansion():
    data = starter_blueprint().model_dump()
    base = copy.deepcopy(data["agent_classes"][0])
    data["agent_classes"] = [{**base, "id": f"class{i}", "instances": MAX_INSTANCES_PER_CLASS}
                             for i in range(MAX_IDENTITIES // MAX_INSTANCES_PER_CLASS)]
    data["topology"]["entrypoint"] = "class0:1"
    data["flows"] = []
    spec = Blueprint.model_validate(data)
    assert len(set(spec.identities())) == MAX_IDENTITIES
    assert len(spec.resolved_edges()) == MAX_IDENTITIES - 1
    data["topology"]["mode"] = "mesh"
    with pytest.raises(ValidationError, match=f"{MAX_PEER_EDGES} peer-edge planning limit"):
        Blueprint.model_validate(data)
    data["agent_classes"] = [data["agent_classes"][0]]
    mesh = Blueprint.model_validate(data)
    assert len(mesh.resolved_edges()) == MAX_INSTANCES_PER_CLASS * (MAX_INSTANCES_PER_CLASS - 1) // 2


def test_instance_capacity_is_shared_with_published_schema():
    schema = Blueprint.model_json_schema()
    assert schema["$defs"]["AgentClass"]["properties"]["instances"]["maximum"] == MAX_INSTANCES_PER_CLASS
    assert schema["$defs"]["AgentClass"]["properties"]["members"]["maxItems"] == MAX_INSTANCES_PER_CLASS
    data = starter_blueprint().model_dump()
    data["agent_classes"][1]["instances"] = MAX_INSTANCES_PER_CLASS + 1
    with pytest.raises(ValidationError):
        Blueprint.model_validate(data)


@pytest.mark.parametrize("patch", [
    {"env": {"TOKEN": "secret-literal"}},
    {"url": "https://example.test/mcp", "transport": "http", "headers": {"Authorization": "Bearer literal"}},
    {"url": "http://example.test/mcp", "transport": "http"},
    {"url": "https://user:secret@example.test/mcp", "transport": "http"},
    {"url": "https://example.test/mcp?token=secret", "transport": "http"},
    {"id": "elephant"},
])
def test_connection_literal_secrets_and_invalid_transports_reject(patch):
    data = {"id": "github", "name": "GitHub", "transport": "stdio", "command": "npx"}
    if patch.get("transport") == "http":
        data.pop("command")
    data.update(patch)
    with pytest.raises(ValidationError):
        Connection.model_validate(data)


def test_valid_stdio_and_http_placeholders_are_preserved_as_data():
    assert Connection(id="local", name="Local", transport="stdio", command="node", args=["./server.js"], env={"TOKEN": "${MCP_TOKEN}"}).env == {"TOKEN": "${MCP_TOKEN}"}
    assert Connection(id="remote", name="Remote", transport="http", url="https://example.test/mcp", headers={"Authorization": "Bearer ${TOKEN}"}).headers["Authorization"] == "Bearer ${TOKEN}"


@pytest.mark.parametrize("patch", [
    {"args": ["--token", "literal-credential"]},
    {"args": ["--api-key=literal-credential"]},
    {"args": ["-e", "SERVICE_TOKEN=literal-credential"]},
    {"args": ["--header", "Authorization: Bearer literal-credential"]},
    {"args": ["postgres://user:literal-credential@localhost/db"]},
    {"args": ["--database-url=postgres://user:literal-credential@localhost/db"]},
    {"args": ["--url=https://example.test/mcp?api_key=literal-credential&x=${DUMMY}"]},
    {"transport": "http", "url": "https://example.test/mcp", "headers": {"Authorization": "Bearer literal-credential ${DUMMY}"}},
    {"transport": "http", "url": "https://example.test/mcp?api_key=literal-credential&x=${DUMMY}"},
    {"transport": "http", "url": "https://example.test/mcp?%74oken=literal-credential&x=${DUMMY}"},
    {"transport": "http", "url": "https://example.test/mcp?key=literal-credential"},
])
def test_direct_blueprint_rejects_credentials_in_arguments_headers_and_queries(patch):
    connection = {"id": "custom", "name": "Custom", "transport": "stdio", "command": "node"}
    if patch.get("transport") == "http":
        connection.pop("command")
    connection.update(patch)
    data = starter_blueprint().model_dump()
    data["connections"] = [connection]
    with pytest.raises(ValidationError):
        Blueprint.model_validate(data)


def test_nonsecret_arguments_and_exact_credential_references_remain_usable():
    args = ["./server.js", "--max-tokens", "1000", "--token-file", "/path with spaces/token", "--scope", "read",
            "--token", "${MCP_TOKEN}", "--api-key=${MCP_API_KEY}", "-e", "SERVICE_TOKEN=${MCP_TOKEN}",
            "--header", "Authorization: Bearer ${MCP_TOKEN}", "${DATABASE_URL}"]
    connection = Connection(id="custom", name="Custom", transport="stdio", command="/path with spaces/node", args=args)
    assert connection.args == args
    assert Connection(id="remote", name="Remote", transport="http", url="https://example.test/mcp?token=${MCP_TOKEN}&region=europe").url.endswith("region=europe")


def test_v1_import_keeps_classes_and_models():
    old = {"project": {"name": "Test", "slug": "test", "description": "A useful test deployment."}, "agents": [{"id": "lead", "name": "Lead", "role": "Coordinator", "instructions": "Answer questions accurately."}]}
    converted = from_v1(old)
    assert converted.agent_classes[0].tools.workspace
    assert converted.agent_classes[0].tools.notes == "write"
    assert converted.topology.entrypoint == "lead:main"


def test_class_names_cannot_collide_with_instance_profiles():
    data = starter_blueprint().model_dump()
    data["agent_classes"][2]["id"] = "research__1"
    data["flows"] = []
    with pytest.raises(ValidationError, match="collide"):
        Blueprint.model_validate(data)


@pytest.mark.parametrize("machine,disk,allowed", [("e2-medium", 30, False), ("e2-standard-8", 30, False), ("e2-standard-4", 100, False), ("e2-standard-8", 100, True)])
def test_local_knowledge_cloud_capacity_is_explicit(machine, disk, allowed):
    data = starter_blueprint().model_dump()
    data["target"] = "gcp"
    data["gcp"].update(project="launch-test-123", machine_type=machine, disk_gb=disk)
    data["knowledge"]["mode"] = "local"
    if allowed:
        assert Blueprint.model_validate(data).gcp.disk_gb == disk
    else:
        with pytest.raises(ValidationError, match="Building the local knowledge service"):
            Blueprint.model_validate(data)
