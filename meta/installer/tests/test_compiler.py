from test_meta_guide_runtime import selected
import asyncio
import ast
import copy
from contextlib import contextmanager
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import shlex
import subprocess
import sys
import tomllib
from types import SimpleNamespace
import zipfile

import pytest

from launchkit.blueprint import Blueprint, starter_blueprint
from launchkit.compiler import application_profiles, build_archive, generate_files, generate_package, mob_toml, package_preview


def load_template(name):
    path = Path(__file__).parents[1] / "launchkit/templates_v2" / f"{name}.py"
    loader = importlib.util.spec_from_file_location(f"generated_{name}", path)
    module = importlib.util.module_from_spec(loader)
    loader.loader.exec_module(module)
    return module


def test_compiler_uses_exact_native_instance_roles_and_flow_dependencies():
    spec = starter_blueprint()
    native = tomllib.loads(mob_toml(spec))
    assert set(native["profiles"]) == {"lead", "research__1", "research__2", "review", *application_profiles(spec).values()}
    assert native["flows"]["briefing"]["steps"]["investigate"]["role"] == "research__1"
    assert native["flows"]["briefing"]["steps"]["challenge"]["role"] == "research__2"
    assert native["flows"]["briefing"]["steps"]["review"]["depends_on"] == ["investigate", "challenge"]
    assert "{{ params }}" in native["flows"]["briefing"]["steps"]["investigate"]["message"]
    assert "{{ steps.investigate }}" in native["flows"]["briefing"]["steps"]["review"]["message"]
    assert native["profiles"]["lead"]["tools"]["workgraph"] is True
    assert native["profiles"]["research__1"]["tools"]["builtins"] is False


def test_runtime_roles_preserve_durable_contract_without_setup_journal():
    data = starter_blueprint().model_dump(mode="json")
    history = "Build status not-built; no acceptance evidence; only a pre-install preview exists."
    data["project"]["description"] = history
    instructions = ("Help the team retain its reading decisions and history. Consult the librarian.\n"
                    "Keep preview fixtures isolated. Ordinary installed changes require explicit "
                    "confirmation and durable receipts.")
    data["agent_classes"][0]["instructions"] = instructions
    files = generate_files(data)
    native = tomllib.loads(files["config/mob.toml"])
    role = native["skills"]["lead_role"]["content"]
    assert instructions in role
    assert f"Team: {data['project']['name']}" in role
    assert all(history not in skill["content"] for skill in native["skills"].values())
    assert json.loads(files["blueprint.json"])["project"]["description"] == history
    assert json.loads(files["deployment.json"])["description"] == history

    # Advancing setup history must not change the runtime contract or its grants.
    data["project"]["description"] = "The application is built; installation verification is pending."
    assert tomllib.loads(mob_toml(data)) == native


def test_deployment_and_rehearsal_resolve_newer_model_from_the_same_native_registry(selected):
    from launchkit.rehearsal_worker import rehearsal_mob_toml

    data = starter_blueprint().model_dump(mode="json")
    for agent in data["agent_classes"][:2]:
        agent.update(provider="openai", model="gpt-6.1-sol")
    data["agent_classes"][2].update(provider="anthropic", model="claude-sonnet-4-6")
    spec = Blueprint.model_validate(data)
    files = generate_files(spec)
    native = tomllib.loads(files["config/mob.toml"])
    trial = tomllib.loads(rehearsal_mob_toml(spec))

    # The pinned Core owns this model. A duplicate custom declaration breaks
    # native construction even when the TOML keys themselves are unique.
    assert "models" not in native
    for profile in ("lead", "research__1", "research__2"):
        selected = native["profiles"][profile]
        assert selected["model"] == "gpt-6.1-sol"
        assert selected["provider"] == "openai"
    assert native["profiles"]["review"]["provider"] == "anthropic"
    assert native["profiles"]["review"]["model"] == "claude-sonnet-4-6"
    assert "models" not in trial
    assert trial["flows"] == native["flows"]


def test_package_zip_hashes_modes_and_preview_match():
    spec = starter_blueprint()
    legacy = generate_files(spec)
    public = {item.path: item.content for item in generate_package(spec, public_runtime=True)}
    for files in (legacy, public):
        checksums = dict(line.split("  ", 1)[::-1] for line in files["SHA256SUMS"].splitlines())
        for path, digest in checksums.items():
            assert hashlib.sha256(files[path].encode()).hexdigest() == digest
    # The download is the package Install agent team installs: the public Meta release.
    with zipfile.ZipFile(io.BytesIO(build_archive(spec))) as archive:
        for path, text in public.items():
            assert archive.read(f"{spec.project.slug}/{path}").decode() == text
        assert archive.getinfo(f"{spec.project.slug}/start.sh").external_attr >> 16 & 0o777 == 0o755
    assert package_preview(spec)["summary"]["agents"] == 4


@pytest.mark.parametrize("target", ["mac", "linux", "gcp"])
def test_exported_readme_points_to_actual_inner_script_folder_after_zip_extraction(target, tmp_path):
    data = starter_blueprint().model_dump()
    data["target"] = target
    data["project"]["slug"] = "folder-guide"
    data["gcp"]["project"] = "launch-test-123"
    extraction = tmp_path / "chosen extraction folder"
    extraction.mkdir()
    with zipfile.ZipFile(io.BytesIO(build_archive(data))) as archive:
        archive.extractall(extraction)
    # Follow the inner folder named by the actual exported README, without
    # executing any script, dependency installer, runtime or cloud command.
    readme = (extraction / "folder-guide" / "README.md").read_text()
    first_step = next(line for line in readme.splitlines() if line.startswith("1. Extract"))
    assert "project folder containing `install.sh` and `start.sh`" in first_step
    assert "if the ZIP created an inner" in first_step
    assert "Open a terminal there" in first_step
    documented_folder = first_step.split("inner `", 1)[1].split("`", 1)[0]
    project = extraction / documented_folder
    assert not (extraction / "install.sh").exists()
    for name in ("install.sh", "setup.py", "start.sh"):
        assert (project / name).is_file()


@pytest.mark.parametrize("target", ["mac", "linux"])
@pytest.mark.parametrize("private_port", [4110, 4111])
def test_exported_console_opening_uses_startup_url_with_private_port(target, private_port, tmp_path, monkeypatch, capsys):
    data = starter_blueprint().model_dump()
    data["target"] = target
    data["advanced"]["port"] = 4110
    files = generate_files(data)
    assert "MOBKIT_PORT=4110\n" in files[".env.example"]
    definition = json.loads(files["deployment.json"])
    assert definition["port"] == 4110
    private_env = tmp_path / ".env"
    private_env.write_text(f"MOBKIT_PORT={private_port}\n")
    monkeypatch.setenv("MOBKIT_PORT", "4999")
    connectors = {"__name__": "generated_connectors"}
    exec(compile(files["connectors.py"], "connectors.py", "exec"), connectors)
    connectors["load_environment"](private_env, override=True)

    # Execute only the actual generated address selection and print statements.
    # No application, native runtime, server, connector instance or provider starts.
    run = next(node for node in ast.parse(files["app.py"]).body
               if isinstance(node, ast.AsyncFunctionDef) and node.name == "run")
    port = next(node for node in run.body if isinstance(node, ast.Assign)
                and isinstance(node.targets[0], ast.Name) and node.targets[0].id == "port")
    url = next(node for node in run.body if isinstance(node, ast.Assign)
               and isinstance(node.targets[0], ast.Attribute) and node.targets[0].attr == "console_url")
    printed_url = next(node for node in ast.walk(run) if isinstance(node, ast.Expr)
                       and isinstance(node.value, ast.Call) and isinstance(node.value.func, ast.Name)
                       and node.value.func.id == "print" and node.value.args
                       and isinstance(node.value.args[0], ast.JoinedStr)
                       and any(isinstance(value, ast.FormattedValue) and isinstance(value.value, ast.Attribute)
                               and value.value.attr == "console_url" for value in node.value.args[0].values))
    context = SimpleNamespace()
    # The shipped template has no hosted Console origin unless startup configures one.
    selected = {"os": connectors["os"], "spec": definition, "application": SimpleNamespace(context=context),
                "hosted_origin": None}
    exec(compile(ast.Module(body=[port, url, printed_url], type_ignores=[]), "app.py", "exec"), selected)
    actual_url = f"http://127.0.0.1:{private_port}/console"
    assert context.console_url == actual_url
    assert capsys.readouterr().out.strip() == f"Open {actual_url}"
    opening = next(line for line in files["README.md"].splitlines()
                   if "Console URL printed in the terminal" in line)
    assert "After startup succeeds" in opening
    assert "The configured default is [http://127.0.0.1:4110/console]" in opening
    assert "private `MOBKIT_PORT` settings may change it" in opening
    if private_port != definition["port"]:
        assert actual_url not in opening


def test_exported_gcp_console_opening_keeps_browser_port_and_explicit_private_vm_tunnel():
    data = starter_blueprint().model_dump()
    data["target"] = "gcp"
    data["gcp"]["project"] = "launch-test-123"
    data["advanced"]["port"] = 4110
    files = generate_files(data)
    readme = files["README.md"]
    assert "Console URL printed in the terminal" not in readme
    assert "On the VM, run `bash deploy/install-service.sh`" in readme
    assert "run `bash deploy/tunnel-gcp.sh` and keep that tunnel open" in readme
    opening = next(line for line in readme.splitlines() if "configured default browser address" in line)
    assert "http://127.0.0.1:4110/console" in opening
    assert "generated tunnel forwards to the VM's configured default port" in opening
    assert "If you changed `MOBKIT_PORT` on the VM, stop that tunnel" in opening
    assert "replacing `VM_PORT` with the private port" in opening
    assert "browser address above stays the same" in opening
    generated = shlex.split(files["deploy/tunnel-gcp.sh"].splitlines()[-1])[1:]
    replacement = next(command for command in opening.split("`") if command.startswith("gcloud compute ssh "))
    replacement = shlex.split(replacement.replace("VM_PORT", "4111"))
    assert generated[:-1] == replacement[:-1]
    assert generated[-2:] == ["-L", "4110:127.0.0.1:4110"]
    assert replacement[-2:] == ["-L", "4110:127.0.0.1:4111"]


def test_package_preserves_512_named_owners_tools_and_parallel_flow_targets(tmp_path):
    """Only compile and inspect; never initialize or send to these identities."""
    data = starter_blueprint().model_dump()
    lead, owners = data["agent_classes"][:2]
    owners.update(id="owner", name="Entity owner", instances=512,
                  description="Own the evidence and history for one distinct entity.",
                  members=[{"key": f"entity{i}", "name": f"Entity {i} owner",
                            "instructions": f"Maintain the evidence for entity {i}. Return findings to the lead."}
                           for i in range(1, 513)])
    owners["tools"].update(notes="read", mcp=[{"connection_id": "source", "tool_names": ["read_entity"]}])
    data["agent_classes"] = [lead, owners]
    data["connections"] = [{"id": "source", "name": "Entity source", "transport": "http", "url": "https://example.test/mcp"}]
    worker_steps = [{"id": f"inspect{i}", "agent": f"owner:entity{i}",
                     "prompt": f"Inspect the source evidence for entity {i}.", "depends_on": []}
                    for i in range(1, 513)]
    data["flows"] = [{"id": "inspection", "name": "Entity inspection", "steps": worker_steps + [
        {"id": "synthesize", "agent": "lead:main", "prompt": "Summarize the entity findings and exceptions.",
         "depends_on": [step["id"] for step in worker_steps]}]}]
    spec = Blueprint.model_validate(data)
    files = generate_files(spec)
    native = tomllib.loads(files["config/mob.toml"])
    runtime = json.loads(files["deployment.json"])
    expected = {"lead:main", *(f"owner:entity{i}" for i in range(1, 513))}
    assert {row["identity"] for row in runtime["agents"]} == expected
    assert len(expected) == 513
    assert len(native["profiles"]) == len(expected) + len(spec.agent_classes)
    assert {row["profile"] for row in runtime["agents"]}.isdisjoint(runtime["applicationProfiles"].values())
    assert len(runtime["connections"]) == 512
    by_identity = {row["identity"]: row for row in runtime["agents"]}
    for i in range(1, 513):
        agent = by_identity[f"owner:entity{i}"]
        assert agent["name"] == f"Entity {i} owner"
        assert agent["tools"]["mcp"] == [{"connection_id": "source", "tool_names": ["read_entity"]}]
        assert native["flows"]["inspection"]["steps"][f"inspect{i}"]["role"] == agent["profile"]
        assert f"entity {i}." in native["skills"][f"{agent['profile']}_member"]["content"]
    final = native["flows"]["inspection"]["steps"]["synthesize"]
    assert len(final["depends_on"]) == 512
    assert "{{ steps.inspect512 }}" in final["message"]
    assert "not host capacity or concurrent throughput" in files["README.md"]
    for path, content in files.items():
        target = tmp_path / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content)
    result = subprocess.run([sys.executable, str(tmp_path / "app.py"), "--check"], capture_output=True, text=True)
    assert result.returncode == 0, result.stderr
    assert "513 agents" in result.stdout


def test_reusable_class_role_keeps_grants_without_seed_private_identity_or_flow_target():
    data = starter_blueprint().model_dump()
    owner = data["agent_classes"][1]
    owner["members"] = [
        {"key": "first", "name": "First private project", "instructions": "Private canary for the first project."},
        {"key": "second", "name": "Second private project", "instructions": "Private canary for the second project."},
    ]
    for flow in data["flows"]:
        for step in flow["steps"]:
            if step["agent"] == "research:1":
                step["agent"] = "research:first"
            elif step["agent"] == "research:2":
                step["agent"] = "research:second"
    spec = Blueprint.model_validate(data)
    files = generate_files(spec)
    native = tomllib.loads(files["config/mob.toml"])
    runtime = json.loads(files["deployment.json"])
    role = runtime["applicationProfiles"]["research"]
    reused = native["profiles"][role]
    seeded = native["profiles"]["research__first"]
    assert reused["skills"] == ["research_role", "launch_comms"]
    assembled = "\n".join(native["skills"][key]["content"] for key in reused["skills"])
    assert "Private canary" not in assembled
    assert "research:first" not in assembled and "First private project" not in assembled
    assert "Report evidence and uncertainty accurately." in assembled
    for key in ("model", "provider", "runtime_mode", "resume_overrides", "tools"):
        assert reused[key] == seeded[key]
    assert "Private canary for the first project." in native["skills"]["research__first_member"]["content"]
    assert native["flows"]["briefing"]["steps"]["investigate"]["role"] == "research__first"
    assert role not in {step["role"] for step in native["flows"]["briefing"]["steps"].values()}
    assert len(runtime["agents"]) == 4


def test_reusable_profile_names_do_not_collide_with_other_fixed_roles():
    data = starter_blueprint().model_dump()
    data["agent_classes"][2]["id"] = "application__lead"
    data["topology"]["edges"] = []
    data["flows"] = []
    spec = Blueprint.model_validate(data)
    native = tomllib.loads(mob_toml(spec))
    reusable = application_profiles(spec)
    assert reusable["lead"] == "application__lead__2"
    assert "application__lead" in native["profiles"]
    assert len(native["profiles"]) == 4 + len(spec.agent_classes)
    assert application_profiles(spec) == reusable


def test_arbitrary_prompt_and_arguments_are_data(tmp_path):
    data = starter_blueprint().model_dump()
    data["agent_classes"][0]["instructions"] = 'Text " quotes\n[profiles.injected]\nmodel="bad"\n$(touch /tmp/never-launch-test)'
    files = generate_files(data)
    native = tomllib.loads(files["config/mob.toml"])
    assert "injected" not in native["profiles"]
    for path, content in files.items():
        target = tmp_path / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content)
        if path.endswith(".py"):
            compile(content, path, "exec")
        elif path.endswith(".sh"):
            subprocess.run(["bash", "-n", str(target)], check=True)
    result = subprocess.run([sys.executable, str(tmp_path / "app.py"), "--check"], capture_output=True, text=True)
    assert result.returncode == 0, result.stderr
    assert "4 agents" in result.stdout


def test_environment_grants_deduplicate_and_do_not_start_unassigned_servers():
    data = starter_blueprint().model_dump()
    data["connections"] = [{"id": "db", "name": "Database", "transport": "stdio", "command": "uvx", "args": ["database-mcp"], "env": {"TOKEN": "${DB_TOKEN}"}}, {"id": "unused", "name": "Unused", "transport": "http", "url": "https://example.test", "headers": {"Authorization": "Bearer ${UNUSED_TOKEN}"}}]
    data["agent_classes"][0]["tools"]["mcp"] = [{"connection_id": "db", "tool_names": ["query"]}]
    runtime = json.loads(generate_files(data)["deployment.json"])
    fields = {f["key"]: f for f in runtime["environment"]}
    assert fields["DB_TOKEN"]["required"] is True
    assert fields["UNUSED_TOKEN"]["required"] is False
    assert len([f for f in runtime["environment"] if f["key"] == "OPENAI_API_KEY"]) == 1


@pytest.mark.parametrize("extraction_provider,extraction_model,agent_provider,agent_model", [
    ("anthropic", "claude-sonnet-4-6", "openai", "gpt-6.1-sol"),
    ("gemini", "gemini-3.1-pro-preview", "openai", "gpt-6.1-sol"),
    ("openai", "gpt-6.1-sol", "anthropic", "claude-sonnet-4-6"),
])
def test_local_knowledge_private_interview_covers_actual_pipeline_and_agent_providers(
        extraction_provider, extraction_model, agent_provider, agent_model):
    from launchkit.generator import PROVIDERS

    data = starter_blueprint().model_dump()
    data.update(provider=extraction_provider, model=extraction_model)
    data["knowledge"]["mode"] = "local"
    for agent in data["agent_classes"]:
        agent.update(provider=agent_provider, model=agent_model)
    files = generate_files(data)
    runtime = json.loads(files["deployment.json"])
    knowledge = json.loads(files["knowledge/settings.json"])
    fields = runtime["environment"]
    provider_keys = {value["envKey"] for value in PROVIDERS.values()}
    selected = [row for row in fields if row["key"] in provider_keys]
    required = {PROVIDERS[agent_provider]["envKey"], PROVIDERS[knowledge["provider"]]["envKey"],
                PROVIDERS["openai"]["envKey"]}
    assert knowledge["provider"] == extraction_provider and knowledge["model"] == extraction_model
    assert {row["key"] for row in selected} == required
    assert len(selected) == len(required)
    assert all(row["secret"] is True and row["required"] is True and "default" not in row for row in selected)
    # The destination interview consumes this same environment, without needing
    # to discover another private key inside the later knowledge subprocess.
    assert all(f"{key}=\n" in files[".env.example"] for key in required)


@pytest.mark.parametrize("knowledge_mode", ["none", "remote"])
def test_unbundled_knowledge_does_not_require_proposal_extraction_provider(knowledge_mode):
    from launchkit.generator import PROVIDERS

    data = starter_blueprint().model_dump()
    data.update(provider="anthropic", model="claude-sonnet-4-6")
    data["knowledge"]["mode"] = knowledge_mode
    fields = json.loads(generate_files(data)["deployment.json"])["environment"]
    provider_keys = {value["envKey"] for value in PROVIDERS.values()}
    assert {row["key"] for row in fields if row["key"] in provider_keys} == {"OPENAI_API_KEY"}


@pytest.mark.parametrize("provider_key", ["ANTHROPIC_API_KEY", "OPENAI_API_KEY"])
def test_connector_default_cannot_become_a_private_provider_key(provider_key):
    data = starter_blueprint().model_dump()
    data.update(provider="anthropic", model="claude-sonnet-4-6")
    data["knowledge"]["mode"] = "local"
    data["connections"] = [{"id": "reference", "name": "Reference source", "transport": "stdio",
        "command": "reference-mcp", "env": {"REFERENCE_SETTING": "${" + provider_key + "}"},
        "setup_inputs": [{"name": provider_key, "secret": False, "default": "synthetic-public-default"}]}]
    data["agent_classes"][0]["tools"]["mcp"] = [{"connection_id": "reference", "tool_names": ["read"]}]
    files = generate_files(data)
    field = next(row for row in json.loads(files["deployment.json"])["environment"] if row["key"] == provider_key)
    assert field["secret"] is True and field["required"] is True and "default" not in field
    assert f"{provider_key}=\n" in files[".env.example"]
    assert "synthetic-public-default" not in files[".env.example"]


def test_shared_connector_input_keeps_strongest_privacy_and_ordinary_nonsecret_defaults():
    data = starter_blueprint().model_dump()
    data["connections"] = [{"id": "first", "name": "First source", "transport": "stdio", "command": "source-mcp",
        "env": {"TOKEN": "${SHARED_SETTING}", "REGION": "${PUBLIC_REGION}"}, "setup_inputs": [
            {"name": "SHARED_SETTING", "secret": False, "default": "synthetic-public-default"},
            {"name": "PUBLIC_REGION", "secret": False, "default": "eu"}]},
        {"id": "second", "name": "Second source", "transport": "stdio", "command": "source-mcp",
         "env": {"TOKEN": "${SHARED_SETTING}"}, "setup_inputs": [{"name": "SHARED_SETTING", "secret": True}]}]
    for connections in [data["connections"], list(reversed(data["connections"]))]:
        data["connections"] = connections
        files = generate_files(data)
        fields = {row["key"]: row for row in json.loads(files["deployment.json"])["environment"]}
        assert fields["SHARED_SETTING"]["secret"] is True and "default" not in fields["SHARED_SETTING"]
        assert fields["PUBLIC_REGION"]["secret"] is False and fields["PUBLIC_REGION"]["default"] == "eu"
        assert "synthetic-public-default" not in files[".env.example"]
        assert "SHARED_SETTING=\n" in files[".env.example"]
        assert 'PUBLIC_REGION="eu"\n' in files[".env.example"]


async def test_mcp_pool_enforces_per_class_tool_names(monkeypatch):
    module = load_template("mcp_connections")
    started = []
    class Tool:
        description = "Example"
        inputSchema = {"type": "object"}
    class FakeConnection:
        def __init__(self, config):
            self.config = config
            self.tools = {"read": Tool(), "write": Tool()}
        async def start(self):
            started.append(self.config["id"])
        async def close(self):
            pass
        async def call(self, name, arguments):
            return {"tool": name, "arguments": arguments}
    monkeypatch.setattr(module, "MCPConnection", FakeConnection)
    reader = {"name": "Reader", "tools": {"mcp": [{"connection_id": "db", "tool_names": ["read"]}], "knowledge": False}}
    no_tools = {"name": "No tools", "tools": {"mcp": [], "knowledge": False}}
    pool = module.MCPPool({"classes": [reader, no_tools], "mcpConnections": [{"id": "db", "name": "Database"}, {"id": "unused", "name": "Unused"}]})
    await pool.start()
    try:
        assert started == ["db"]
        tools = pool.for_class(reader)
        assert len(tools) == 1
        assert await tools[0]["handler"]({"x": 2}) == {"tool": "read", "arguments": {"x": 2}}
        assert pool.for_class(no_tools) == []
        reader["tools"]["mcp"][0]["tool_names"] = ["missing"]
        with pytest.raises(RuntimeError, match="absent"):
            pool.for_class(reader)
    finally:
        await pool.close()


WIKI_QUERY_SCHEMA = {"type": "object", "additionalProperties": False, "required": ["query"],
    "properties": {"query": {"type": "string", "minLength": 1, "maxLength": 256,
                              "pattern": r"^[^\x00-\x1f\x7f|]+(?![\s\S])"}}}


@pytest.mark.parametrize("pattern", [r"a(?=b)", r"a(?!b)", r"(?<=a)b", r"(?<!a)b", r"\\(?=a)"])
def test_openai_mcp_schema_projects_each_lookaround_without_mutating_original(pattern):
    module = load_template("mcp_connections")
    schema = {"type": "string", "pattern": pattern, "minLength": 1, "maxLength": 256,
              "description": "External input", "enum": ["a", "b"]}
    before = copy.deepcopy(schema)
    assert module.model_tool_schema(schema, "openai") == {key: value for key, value in schema.items() if key != "pattern"}
    assert schema == before
    assert module.model_tool_schema(schema, "anthropic") is schema


@pytest.mark.parametrize("pattern", [
    r"^[a-z]+$", r"\(?=literal", r"\(?!literal", r"\(?<=literal", r"\(?<!literal",
    r"[(?=)]", r"[(?!)]", r"[(?<=)]", r"[(?<!)]", r"[](?=)]", r"[^(?=)]",
    r"(?#lookaround text (?=ignored)literal",
])
def test_openai_mcp_schema_preserves_patterns_without_actual_lookaround(pattern):
    module = load_template("mcp_connections")
    schema = {"type": "string", "pattern": pattern}
    assert module.model_tool_schema(schema, "openai") is schema


def test_mcp_schema_projection_visits_only_schema_children_not_annotations_or_literal_data():
    module = load_template("mcp_connections")
    restricted = {"type": "string", "pattern": "(?=a)a", "minLength": 1}
    literal = {"pattern": "(?=keep)", "properties": {"x": restricted}}
    schema = {"type": "object", "properties": {"query": restricted, "pattern": {"const": "(?=keep)"}},
        "$defs": {"value": restricted}, "definitions": {"old": restricted},
        "patternProperties": {"^x": restricted}, "dependentSchemas": {"query": {"properties": {"x": restricted}}},
        "dependencies": {"old": {"properties": {"x": restricted}}, "query": ["old"]},
        "additionalProperties": restricted, "unevaluatedProperties": restricted, "propertyNames": restricted,
        "items": {"allOf": [restricted]}, "prefixItems": [restricted], "additionalItems": restricted,
        "contains": restricted, "unevaluatedItems": restricted, "not": restricted, "if": restricted,
        "then": restricted, "else": restricted, "contentSchema": restricted,
        "anyOf": [restricted], "oneOf": [restricted], "allOf": [restricted],
        "default": literal, "const": literal, "enum": [literal], "examples": [literal], "x-annotation": literal}
    before = copy.deepcopy(schema)
    projected = module.model_tool_schema(schema, "openai")
    assert "pattern" not in projected["properties"]["query"]
    for key in ("$defs", "definitions", "patternProperties"):
        assert all("pattern" not in child for child in projected[key].values())
    for key in ("dependentSchemas", "dependencies"):
        child = projected[key]["query" if key == "dependentSchemas" else "old"]
        assert "pattern" not in child["properties"]["x"]
    for key in ("additionalProperties", "unevaluatedProperties", "propertyNames", "additionalItems", "contains",
                "unevaluatedItems", "not", "if", "then", "else", "contentSchema"):
        assert "pattern" not in projected[key]
    for key in ("prefixItems", "anyOf", "oneOf", "allOf"):
        assert "pattern" not in projected[key][0]
    assert "pattern" not in projected["items"]["allOf"][0]
    for key in ("default", "const", "enum", "examples", "x-annotation"):
        assert projected[key] == before[key]
    assert projected["properties"]["pattern"] == before["properties"]["pattern"]
    assert schema == before
    assert module.model_tool_schema({"items": [restricted]}, "openai") == {"items": [{"type": "string", "minLength": 1}]}


def projected_mcp_tool(module, schema, provider="openai"):
    calls = []
    async def call(name, arguments):
        calls.append((name, copy.deepcopy(arguments)))
        return {"title": arguments["query"], "source": "selected-owner"}
    connection = SimpleNamespace(config={"name": "Selected reference"},
        tools={"read_title": SimpleNamespace(description="Read an exact title", inputSchema=schema)}, call=call)
    pool = module.MCPPool({})
    pool.connections["reference"] = connection
    item = {"name": "Researcher", "provider": provider,
            "tools": {"knowledge": False, "mcp": [{"connection_id": "reference", "tool_names": ["read_title"]}]}}
    return pool.for_class(item)[0], calls


async def test_projected_mcp_tool_validates_original_exact_title_before_selected_owner_call():
    module = load_template("mcp_connections")
    source, before = copy.deepcopy(WIKI_QUERY_SCHEMA), copy.deepcopy(WIKI_QUERY_SCHEMA)
    tool, calls = projected_mcp_tool(module, source)
    assert tool["schema"]["properties"]["query"] == {"type": "string", "minLength": 1, "maxLength": 256}
    assert tool["schema"]["required"] == ["query"] and tool["schema"]["additionalProperties"] is False
    assert await tool["handler"]({"query": "Astrophotography"}) == {"title": "Astrophotography", "source": "selected-owner"}
    assert calls == [("read_title", {"query": "Astrophotography"})]
    for invalid in ({"query": "PRIVATE_CANARY\n"}, {"query": "Sun|Moon"}, {"query": "a\x00b"}, {"query": "a\x7fb"},
                    {"query": ""}, {"query": "a" * 257}, {"query": 1}, {}, {"query": "Sun", "extra": "PRIVATE_CANARY"}):
        with pytest.raises(ValueError) as caught:
            await tool["handler"](invalid)
        assert str(caught.value) == "MCP tool arguments do not match the advertised input schema."
    assert len(calls) == 1 and source == before


@pytest.mark.parametrize("provider,schema", [("anthropic", WIKI_QUERY_SCHEMA), ("gemini", WIKI_QUERY_SCHEMA),
    (None, WIKI_QUERY_SCHEMA), ("openai", {"type": "object"}),
    ("openai", {"type": "object", "properties": {"query": {"type": "string", "pattern": "^[a-z]+$"}}})])
async def test_unchanged_mcp_tools_keep_existing_schema_and_handler_path(monkeypatch, provider, schema):
    module = load_template("mcp_connections")
    def unexpected_validator(_schema):
        raise AssertionError("Unchanged tools must not get a new validation path.")
    monkeypatch.setattr(module, "_original_schema_validator", unexpected_validator)
    source = copy.deepcopy(schema)
    tool, calls = projected_mcp_tool(module, source, provider)
    assert tool["schema"] is source
    await tool["handler"]({"query": "owner validates this"})
    assert len(calls) == 1


async def test_projected_mcp_local_ref_retains_original_validation_and_model_copy_is_not_authority():
    module = load_template("mcp_connections")
    source = {"type": "object", "required": ["query"], "additionalProperties": False,
        "$defs": {"title": copy.deepcopy(WIKI_QUERY_SCHEMA["properties"]["query"])},
        "properties": {"query": {"$ref": "#/$defs/title"}}}
    before = copy.deepcopy(source)
    tool, calls = projected_mcp_tool(module, source)
    assert "pattern" not in tool["schema"]["$defs"]["title"]
    assert await tool["handler"]({"query": "Moon"}) == {"title": "Moon", "source": "selected-owner"}
    tool["schema"]["$defs"]["title"]["minLength"] = 0
    for query in ("", "Moon|Sun"):
        with pytest.raises(ValueError, match="advertised input schema"):
            await tool["handler"]({"query": query})
    assert calls == [("read_title", {"query": "Moon"})] and source == before


async def test_projected_mcp_remote_schema_reference_refuses_without_owner_call():
    module = load_template("mcp_connections")
    source = {**copy.deepcopy(WIKI_QUERY_SCHEMA), "allOf": [{"$ref": "https://fixture.invalid/private-schema"}]}
    before = copy.deepcopy(source)
    tool, calls = projected_mcp_tool(module, source)
    with pytest.raises(ValueError, match=r"^MCP tool arguments do not match the advertised input schema\.$"):
        await tool["handler"]({"query": "Sun"})
    assert calls == [] and source == before


def test_malformed_original_projected_schema_refuses_registration_without_source_disclosure():
    module = load_template("mcp_connections")
    source = copy.deepcopy(WIKI_QUERY_SCHEMA)
    source["properties"]["query"]["type"] = "PRIVATE_CANARY"
    before = copy.deepcopy(source)
    with pytest.raises(ValueError) as caught:
        projected_mcp_tool(module, source)
    assert str(caught.value) == "The MCP tool input schema cannot be validated."
    assert source == before


def test_mcp_aliases_avoid_collisions_and_expand_only_placeholders(monkeypatch):
    module = load_template("mcp_connections")
    assert module.tool_alias("x", "a.b") != module.tool_alias("x", "a_b")
    assert len(module.tool_alias("x" * 100, "b" * 100)) <= 64
    monkeypatch.setenv("TOKEN", "secret-value")
    assert module.expand("Bearer ${TOKEN}") == "Bearer secret-value"
    assert module.expand("$(uname)") == "$(uname)"
    with pytest.raises(RuntimeError, match="Configure MISSING_TOKEN"):
        module.expand("${MISSING_TOKEN}")


async def test_real_stdio_mcp_handshake_and_grants(tmp_path):
    pytest.importorskip("mcp")
    module = load_template("mcp_connections")
    server = tmp_path / "server.py"
    server.write_text('''from mcp.server.fastmcp import FastMCP
import os
m = FastMCP("Acceptance")
@m.tool()
def lookup(key: str) -> dict:
    return {"key":key,"value":"fixture-value","provider_secret_present": "OPENAI_API_KEY" in os.environ}
@m.tool()
def mutate(value: str) -> str:
    return value
m.run(transport="stdio")
''')
    item = {"name": "Reader", "tools": {"mcp": [{"connection_id": "fixture", "tool_names": ["lookup"]}], "knowledge": False}}
    pool = module.MCPPool({"classes": [item], "mcpConnections": [{"id": "fixture", "name": "Fixture", "transport": "stdio", "command": sys.executable, "args": [str(server)], "env": {}}]})
    await pool.start()
    try:
        tools = pool.for_class(item)
        assert len(tools) == 1
        assert "lookup" in tools[0]["name"]
        result = await tools[0]["handler"]({"key": "example"})
        assert json.loads(result["content"][0]["text"]) == {"key": "example", "value": "fixture-value", "provider_secret_present": False}
        assert len(pool.connections["fixture"].tools) == 2
    finally:
        await pool.close()


def test_cloud_local_knowledge_exports_capacity_and_excludes_private_build_data(tmp_path):
    data = starter_blueprint().model_dump()
    data["target"] = "gcp"
    data["gcp"].update(project="launch-test-123", machine_type="e2-standard-8", disk_gb=100)
    data["knowledge"]["mode"] = "local"
    files = generate_files(data)
    script = files["deploy/deploy-gcp.sh"]
    assert "MACHINE=e2-standard-8" in script
    assert "--boot-disk-size 100GB" in script
    assert 'remote_operations.py archive "$PACKAGE_ARCHIVE"' in script
    assert "'.env'" in files['remote_operations.py'] and "'.source'" in files['remote_operations.py']
    path = tmp_path / "deploy.sh"
    path.write_text(script)
    result = subprocess.run(["bash", str(path), "--dry-run"], capture_output=True, text=True)
    assert result.returncode == 0
    assert "100 GB disk" in result.stdout
    assert "root-equivalent Docker group access" in result.stdout
    assert "official Debian repository" in result.stdout
    assert script.index('root-equivalent Docker group access') < script.index('Type the project ID to deploy:')
    assert "knowledge/compose.json" in files


def test_native_and_local_notes_grants_are_independent(tmp_path, monkeypatch):
    root = Path(__file__).parents[1] / "launchkit"
    with monkeypatch.context() as patch:
        patch.syspath_prepend(str(root))
        patch.syspath_prepend(str(root / "templates"))
        patch.syspath_prepend(str(root / "templates_v2"))
        module = load_template("app")
    tools = module.ConnectorTools(tmp_path, {"connectors": [{"id": "files"}]})
    grant = {"tools": {"workspace": False, "notes": "none"}}
    assert module.local_tools(tools, grant) == []
    grant["tools"]["notes"] = "read"
    assert [tool["name"] for tool in module.local_tools(tools, grant)] == ["notes_search"]
    grant["tools"].update(notes="write", workspace=True)
    assert {tool["name"] for tool in module.local_tools(tools, grant)} == {"files_list", "files_read", "notes_save", "notes_search"}


@pytest.mark.parametrize("with_application", [False, True])
def test_v2_gcp_executes_selected_resources_and_archives_only_public_config(tmp_path, with_application):
    # Reuse the established no-network gcloud harness, but compile v2 files.
    from test_gcp import CloudHarness, PROJECT
    import tarfile
    cloud = CloudHarness(tmp_path)
    data = starter_blueprint().model_dump()
    data["target"] = "gcp"
    data["gcp"].update(project=PROJECT, machine_type="e2-standard-8", disk_gb=150)
    data["knowledge"]["mode"] = "local"
    for name, content in generate_files(data).items():
        destination = cloud.package / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(content)
    application = cloud.package / "application"
    if with_application:
        application.mkdir()
        (application / "__init__.py").write_text("def build(context):\n    return object()\n")
        (application / "assets").mkdir()
        (application / "assets/index.html").write_text("<p>Custom interface</p>")
        (cloud.package / "requirements.application.txt").write_text("example-package==1.0\n")
        # These files represent accepted implementation output, so the host
        # publishes their exact hashes before the source-bound cloud handoff.
        with (cloud.package / 'SHA256SUMS').open('a') as manifest:
            for name in ('application/__init__.py', 'application/assets/index.html', 'requirements.application.txt'):
                manifest.write(hashlib.sha256((cloud.package / name).read_bytes()).hexdigest() + '  ' + name + '\n')
    private = ["knowledge/.env", "knowledge/.source/.git/config", "knowledge/.source/private.txt", "knowledge/test.receipt.json", "knowledge/data/notes", ".env", ".mobkit/runtime.sqlite"]
    if with_application:
        private += ["application/.env", "application/data/secret", "application/__pycache__/private.pyc"]
    for name in private:
        path = cloud.package / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("private-v2-canary-never-upload")
    dry_run = cloud.run(args=("--dry-run",))
    assert dry_run.returncode == 0 and "root-equivalent Docker group access" in dry_run.stdout
    assert cloud.calls == [] and not (cloud.root / "uploaded.tar.gz").exists()
    cloud.configure(install_fails=True)
    failed = cloud.run()
    assert failed.returncode != 0 and "--resume" in failed.stderr
    assert (cloud.package / ".mobkit/gcp-deployment.json").is_file()
    for name in private:
        assert (cloud.package / name).read_text() == "private-v2-canary-never-upload"
    cloud.configure()
    result = cloud.run(args=("--resume",))
    assert result.returncode == 0, result.stderr
    create = next(call for call in cloud.calls if call[:3] == ["compute", "instances", "create"])
    assert len([call for call in cloud.calls if call[:3] == ["compute", "instances", "create"]]) == 1
    assert create[create.index("--machine-type") + 1] == "e2-standard-8"
    assert create[create.index("--boot-disk-size") + 1] == "150GB"
    assert "--no-service-account" in create and "--no-scopes" in create
    install = [call for call in cloud.calls if call[:2] == ['compute', 'ssh']][-1]
    command = install[install.index('--command') + 1]
    assert 'docker-compose-plugin' in command and '$(id -un)' in command
    with tarfile.open(cloud.root / "uploaded.tar.gz") as archive:
        names = archive.getnames()
        assert {"blueprint.json", "mcp_connections.py", "knowledge/compose.json", "knowledge/manage.py"} <= set(names)
        if with_application:
            assert {"application/__init__.py", "application/assets/index.html", "requirements.application.txt"} <= set(names)
        else:
            assert "requirements.application.txt" not in names and "application" not in names
        assert not any(".source" in Path(name).parts or Path(name).name.endswith(".receipt.json") for name in names)
        for item in archive.getmembers():
            if item.isfile():
                assert b"private-v2-canary-never-upload" not in archive.extractfile(item).read()
    if with_application:
        (application / "__init__.py").write_text("def build(context):\n    raise RuntimeError('changed code')\n")
        resumed = cloud.run(args=("--resume",))
        assert resumed.returncode != 0 and "source changed" in resumed.stderr


def scoped_test_token(space, expires=None):
    import base64
    import time
    encoded = base64.urlsafe_b64encode(json.dumps({"space_id": space, "exp": time.time() + 3600 if expires is None else expires}).encode()).rstrip(b"=").decode()
    return "test." + encoded + ".signature"


@pytest.mark.parametrize("token,space,error", [
    (scoped_test_token("other-project"), "selected-project", "does not match"),
    ("opaque-token", "selected-project", "Opaque tokens"),
    (scoped_test_token("selected-project"), "", "Configure"),
    (scoped_test_token("selected-project", 1), "selected-project", "expired"),
    ("a.not-base64!.c", "selected-project", "could not be decoded"),
])
async def test_elephant_scope_refusal_precedes_all_mcp_connections(monkeypatch, token, space, error):
    module = load_template("mcp_connections")
    monkeypatch.setenv("ELEPHANT_TOKEN", token)
    monkeypatch.setenv("ELEPHANT_SPACE_ID", space)
    touched = []
    class ForbiddenConnection:
        def __init__(self, config):
            touched.append(config)
            raise AssertionError("No MCP may open before scope validation")
    monkeypatch.setattr(module, "MCPConnection", ForbiddenConnection)
    spec = {"classes": [{"tools": {"knowledge": True, "mcp": []}}], "knowledge": {"token_env": "ELEPHANT_TOKEN", "space_env": "ELEPHANT_SPACE_ID"}, "mcpConnections": [{"id": "elephant", "name": "Knowledge service"}]}
    pool = module.MCPPool(spec)
    with pytest.raises(RuntimeError, match=error):
        await pool.start()
    assert touched == []


def test_elephant_scope_matches_local_intent_and_requires_expected_space(monkeypatch):
    module = load_template("mcp_connections")
    data = starter_blueprint().model_dump()
    data["knowledge"]["mode"] = "remote"
    data["agent_classes"][0]["tools"]["knowledge"] = True
    runtime = json.loads(generate_files(data)["deployment.json"])
    fields = {field["key"]: field for field in runtime["environment"]}
    assert fields["ELEPHANT_SPACE_ID"]["required"] is True
    monkeypatch.setenv("ELEPHANT_TOKEN", scoped_test_token("selected-project"))
    monkeypatch.setenv("ELEPHANT_SPACE_ID", "selected-project")
    module.validate_knowledge_scope(runtime)
    runtime["classes"][0]["tools"]["knowledge"] = False
    monkeypatch.delenv("ELEPHANT_TOKEN")
    monkeypatch.delenv("ELEPHANT_SPACE_ID")
    module.validate_knowledge_scope(runtime)


def test_private_candidate_readme_requires_saved_update_and_recovery():
    from launchkit.compiler import _readme

    readme = _readme(starter_blueprint(), native_version="0.8.48", candidate=True)
    opening = readme.split("## Install and start\n\n", 1)[1].split("\n## Use and return", 1)[0]
    assert "It cannot create a fresh installation" in opening
    assert "Keep the saved\nproject folder, private settings and agent history" in opening
    assert "Do not extract this ZIP into\na new folder or run `bash install.sh` to install it separately" in opening
    assert "host-accepted\nexternal artifact bundle" in opening
    assert "Native tools retain signed-candidate provenance" in opening
    assert "exact source-built SDK is unsigned and unpublished" in opening
    assert "Keep that bundle available\non this host" in opening
    assert "**Stage accepted update**" in opening
    assert "compatibility; this does not apply the update" in opening
    assert "wait for confirmed cleanup" in opening
    assert "**Apply and verify update**" in opening
    assert "If **Recover this application** appears" in opening
    assert "**Retry current application** or **Recover previous application** only when offered" in opening
    assert "Wait for checked recovery before ordinary Start; keep the same project folder" in opening
    assert "**Start installed application** when offered if the application remains stopped" in opening
    assert "current interface links" in opening
    assert "Extract the ZIP into a new empty folder you own" not in opening
    assert "Run `bash install.sh` with Python 3.11+ available" not in opening
    assert "Run `bash start.sh` and keep the terminal open" not in opening


def test_ordinary_readme_preserves_exported_terminal_setup():
    from launchkit.compiler import _readme

    readme = _readme(starter_blueprint(), native_version="0.8.48")
    opening = readme.split("## Install and start\n\n", 1)[1].split("\n## Use and return", 1)[0]
    expected = """If Forge already installed this application, return to that saved installation
and use its **Start installed application** and **Stop application** controls
(**Start installed agent team** and **Stop agent team** for a native team).
Open the current interface links shown there; an exported default port
or an older bookmark may differ from the installed address. Do not start a second
terminal process over the same running installation. The terminal steps below are
for installing and operating the exported package yourself.

1. Extract the ZIP into a new empty folder you own. Open the project folder containing `install.sh` and `start.sh`; if the ZIP created an inner `atlas-operations` folder, use that folder. Open a terminal there for the commands below. The project folder must not allow group or other write access. If the installer reports that permission problem, run `chmod go-w .` here, then retry the same install command. This repair changes only this folder's permissions; keep `.venv`, `.env`, `workspace`, `data` and `.mobkit` in place.
2. Run `bash install.sh` with Python 3.11+ available. This installs pinned Meerkat/MobKit 0.8.48 and the official MCP Python SDK 1.30.0, then verifies native gateway downloads using published SHA-256 checksums. If dependencies fail to install, correct the reported cause and use the printed retry command in the same project folder; keep `.venv`, `.env`, `workspace`, `data` and `.mobkit`, and continue after installation succeeds.
3. Enter provider keys and connector values when the local interview asks. Values stay in a mode-600 `.env` on the deployment machine. The workbench never receives them.
4. Run `bash start.sh --doctor`. For selected MCPs or the knowledge service, run `bash start.sh --connections-check` to perform real handshakes and validate selected tool names.
5. Run `bash start.sh` and keep the terminal open. Stop with Control-C.
6. After startup succeeds, open the Console URL printed in the terminal. The configured default is [http://127.0.0.1:4110/console](http://127.0.0.1:4110/console); private `MOBKIT_PORT` settings may change it.

First startup initializes storage with MobKit's official offline migration tool. Agents normally wait for input, but saved schedules and application adapters may resume work on startup. Review their configured behavior before restarting. `--smoke` makes one paid provider request. The console is loopback-only; its session, topology, flow and tool activity are real runtime projections.
"""
    assert opening == expected
    assert "**Stage accepted update**" not in opening
    assert "It cannot create a fresh installation" not in opening
