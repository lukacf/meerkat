import base64
from contextlib import asynccontextmanager
import hashlib
import hmac
import importlib.util
import json
import os
from pathlib import Path
import re
import sys
import time
from types import ModuleType, SimpleNamespace

import pytest

from launchkit.elephant_setup import ELEPHANT_REPOSITORY, ELEPHANT_REVISION, generate_elephant_files

# The default pin that earlier versions recorded; no repository serves it now.
RETIRED_DEFAULT_REVISION = "98307999503609abde1b06861154c1fef5b113e5"


@pytest.fixture
def manager(tmp_path, monkeypatch):
    from launchkit import elephant_setup
    spec = importlib.util.spec_from_file_location("launch_elephant_manager_test", elephant_setup.TEMPLATES / "manage.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    folder = tmp_path / "knowledge"
    folder.mkdir()
    module.ROOT = folder
    monkeypatch.setattr(module, "environment", lambda: {"ELEPHANT_TOKEN": "opaque-read-token", "ELEPHANT_OPERATOR_TOKEN": "opaque-operator-token", "ELEPHANT_SPACE_ID": "test-project"})
    return module


def config(**overrides):
    return {"mode": "local", "url": "http://127.0.0.1:8091/mcp/agent", "token_env": "ELEPHANT_TOKEN", "space_env": "ELEPHANT_SPACE_ID", **overrides}


def test_local_compose_is_one_embedded_service_with_a_private_volume():
    files = generate_elephant_files(config(), "test-project")
    compose = json.loads(files["knowledge/compose.json"])
    assert list(compose["services"]) == ["elephant"]
    service = compose["services"]["elephant"]
    environment = service["environment"]
    assert service["ports"] == ["127.0.0.1:8091:3000"]
    assert service["command"] == ["sh", "-c", "elephant migrate && exec elephant serve"]
    assert "--server-only" not in " ".join(service["command"])
    assert environment["ELEPHANT_DB"] == "surreal-db"
    assert environment["SURREALDB_URL"] == "rocksdb:///data/store"
    assert environment["ELEPHANT_DURABILITY_PATH"].startswith("/data/")
    assert service["volumes"][0] == "knowledge-data:/data" and set(compose["volumes"]) == {"knowledge-data", "knowledge-assets"}
    assert environment["SKIP_AUTH"] == "false"
    # The extractor reaches this space through the service's own listener.
    assert environment["PIPELINE_AGENTIC_MCP_URL"] == "http://127.0.0.1:3000/mcp/agent"
    assert environment["PIPELINE_AGENTIC_MCP_SPACE_ID"] == environment["MCP_SPACE_ID"] == "test-project"
    assert environment["MCP_BEARER_TOKEN"] == "${MCP_BEARER_TOKEN:?Run knowledge/manage.py setup}"
    assert not any("SURREALDB_PASS" in value or "SURREALDB_USER" in key for key, value in environment.items())
    settings = json.loads(files["knowledge/settings.json"])
    assert settings["source_revision"] == ELEPHANT_REVISION
    assert settings["source_repository"] == "https://github.com/lukacf/meerkat-recall"
    dockerfile = files["knowledge/Dockerfile"]
    assert "rust:1.94-bookworm" in dockerfile
    assert "--features meerkat-extraction,rocksdb" in dockerfile
    assert "clang libclang-dev cmake" in dockerfile
    assert "Dockerfile.dockerignore" in " ".join(files)


def test_local_image_provides_every_command_the_stack_runs():
    files = generate_elephant_files(config(), "test-project")
    dockerfile = files["knowledge/Dockerfile"]
    assert "cargo build --locked --release -p meerkat-recall-bin " in dockerfile
    provided = set(re.findall(r"/usr/local/bin/([\w.-]+)$", dockerfile, re.M))
    image_command = json.loads(re.search(r"^CMD (.+)$", dockerfile, re.M).group(1))
    services = json.loads(files["knowledge/compose.json"])["services"]
    commands = [service.get("command", image_command) for service in services.values() if "build" in service]
    assert commands and all(command == image_command for command in commands)
    programs = set()
    for command in commands:
        assert command[:2] == ["sh", "-c"]
        programs |= {step.split()[1] if step.split()[0] == "exec" else step.split()[0] for step in command[2].split("&&")}
    assert programs == {"elephant"} and programs <= provided


def test_local_image_lets_its_service_user_read_the_copied_source_files():
    dockerfile = generate_elephant_files(config(), "test-project")["knowledge/Dockerfile"]
    runtime = dockerfile.rsplit("\nFROM ", 1)[1].splitlines()
    user = next(index for index, line in enumerate(runtime) if line.startswith("USER "))
    # Copied files keep the checkout's modes, so a private umask would hide them from the service user.
    copies = {index: line.split()[-1].rstrip("/") for index, line in enumerate(runtime[:user])
              if line.startswith("COPY --from=build ") and line.endswith("/")}
    assert sorted(copies.values()) == ["/app/migrations", "/app/skills"]
    for index, target in copies.items():
        assert any(line.startswith("RUN chmod -R a+rX ") and target in line.split()[4:] for line in runtime[index + 1:user])


@pytest.mark.parametrize("repository", ["https://github.com/lukacf/elephant", "https://github.com/lukacf/elephant.git", ELEPHANT_REPOSITORY])
def test_recompiled_retired_default_pin_builds_the_current_default(repository):
    files = generate_elephant_files(config(source_repository=repository, source_revision=RETIRED_DEFAULT_REVISION), "test-project")
    settings = json.loads(files["knowledge/settings.json"])
    assert (settings["source_repository"], settings["source_revision"]) == (ELEPHANT_REPOSITORY, ELEPHANT_REVISION)
    assert json.loads(files["knowledge/compose.json"])["services"]["elephant"]["image"] == f"mobkit-launch-elephant:{ELEPHANT_REVISION[:12]}"


def test_explicit_source_pin_is_kept():
    pin = {"source_repository": "https://github.com/example/knowledge-fork", "source_revision": "a" * 40}
    settings = json.loads(generate_elephant_files(config(**pin), "test-project")["knowledge/settings.json"])
    assert (settings["source_repository"], settings["source_revision"]) == (pin["source_repository"], pin["source_revision"])
    other = json.loads(generate_elephant_files(config(source_revision=RETIRED_DEFAULT_REVISION,
        source_repository="https://github.com/example/knowledge-fork"), "test-project")["knowledge/settings.json"])
    assert other["source_revision"] == RETIRED_DEFAULT_REVISION


def test_remote_mode_exports_verification_without_local_service():
    files = generate_elephant_files(config(mode="remote", url="https://knowledge.example.org/mcp/agent"), "test-project")
    assert "knowledge/manage.py" in files
    assert "knowledge/compose.json" not in files
    assert generate_elephant_files({"mode": "none"}, "test-project") == {}


@pytest.mark.parametrize("overrides", [
    {"url": "http://127.0.0.1:8091/mcp/full"},
    {"url": "http://0.0.0.0:8091/mcp/agent"},
    {"url": "https://remote.example.org/mcp/agent"},
    {"source_revision": "main"},
    {"source_repository": "file:///private/repo"},
    {"token_env": "same", "space_env": "same"},
    {"token_env": "ELEPHANT_TOKEN", "space_env": "ELEPHANT_TOKEN"},
])
def test_invalid_local_setup_does_not_emit_scripts(overrides):
    with pytest.raises(ValueError):
        generate_elephant_files(config(**overrides), "test-project")


def test_token_is_signed_scoped_and_read_only(manager):
    token = manager.jwt_token("secret", "test-project", now=1000)
    header, body, signature = token.split(".")
    decode = lambda value: base64.urlsafe_b64decode(value + "=" * (-len(value) % 4))
    claims = json.loads(decode(body))
    assert claims["space_id"] == "test-project"
    assert claims["aud"] == ["test-project"]
    assert claims["exp"] == 1000 + 90 * 86400
    assert "*" not in claims["scopes"]
    assert all(scope.startswith("read:") for scope in claims["scopes"])
    assert hmac.compare_digest(decode(signature), hmac.new(b"secret", f"{header}.{body}".encode(), hashlib.sha256).digest())


def claims_of(token):
    body = token.split(".")[1]
    return json.loads(base64.urlsafe_b64decode(body + "=" * (-len(body) % 4)))


def test_extractor_token_is_read_only_and_names_one_granted_space(manager):
    token = manager.worker_token("secret", "test-project", now=1000)
    claims = claims_of(token)
    # The five scopes the readiness check requires, plus the documented read:assertion.
    assert sorted(claims["scopes"]) == ["read:assertion", "read:doc:metadata", "read:doc:raw", "read:entity", "read:predicate", "read:source"]
    assert claims["sub"] == manager.WORKER_SUBJECT == "recall-extractor"
    assert claims["space_id"] == "test-project" and claims["spaces"] == ["test-project"]
    assert "bootstrap_space" not in claims
    assert claims["clearances"] == [{"policy_id": "default", "max_level": 0, "labels": [], "handling_allow": []}]
    assert (claims["iss"], claims["aud"]) == ("mobkit-launch", ["test-project"])
    header, body, signature = token.split(".")
    decode = lambda value: base64.urlsafe_b64decode(value + "=" * (-len(value) % 4))
    assert hmac.compare_digest(decode(signature), hmac.new(b"secret", f"{header}.{body}".encode(), hashlib.sha256).digest())


def test_bootstrap_token_only_administers_its_own_space_briefly(manager):
    claims = claims_of(manager.bootstrap_token("secret", "test-project", now=1000))
    assert claims["bootstrap_space"] == claims["space_id"] == "test-project" and claims["spaces"] == ["test-project"]
    assert 1 <= claims["exp"] - claims["iat"] <= 3600
    assert set(claims["scopes"]) == {"read:space_config", "write:space_config", "manage:grant", "manage:provider"}
    assert claims["clearances"] and not {"security_labels", "subject_allowlist", "purpose"} & set(claims)
    assert claims.get("security_level", 0) == 0


def test_setup_keeps_operator_credentials_out_of_project_env(manager, monkeypatch):
    monkeypatch.setenv("OPENAI_API_KEY", "test-openai-value")
    settings = {**config(), "provider": "openai", "project": "test-project"}
    manager.setup(settings)
    local = manager.read_env(manager.ROOT / ".env")
    project = manager.read_env(manager.ROOT.parent / ".env")
    assert set(project) == {"ELEPHANT_TOKEN", "ELEPHANT_SPACE_ID"}
    assert "ELEPHANT_OPERATOR_TOKEN" in local
    assert local["ELEPHANT_OPERATOR_TOKEN"] != project["ELEPHANT_TOKEN"]
    assert (manager.ROOT / ".env").stat().st_mode & 0o777 == 0o600
    assert (manager.ROOT.parent / ".env").stat().st_mode & 0o777 == 0o600
    assert claims_of(local["MCP_BEARER_TOKEN"])["sub"] == "recall-extractor"
    old_secret = local["JWT_SECRET"]
    manager.setup(settings, renew=True)
    assert manager.read_env(manager.ROOT / ".env")["JWT_SECRET"] == old_secret


def test_up_gives_an_older_folder_only_its_missing_extractor_token(manager, monkeypatch):
    monkeypatch.setenv("OPENAI_API_KEY", "test-openai-value")
    settings = {**config(), "provider": "openai", "project": "test-project"}
    manager.setup(settings)
    path = manager.ROOT / ".env"
    older = {key: value for key, value in manager.read_env(path).items() if key != "MCP_BEARER_TOKEN"}
    manager.write_env(path, older)
    project = (manager.ROOT.parent / ".env").read_bytes()
    monkeypatch.setattr(manager, "load_settings", lambda: settings)
    monkeypatch.setattr(manager, "local_preflight", lambda: None)
    monkeypatch.setattr(manager, "setup", lambda *_, **__: pytest.fail("up re-ran setup for a folder that keeps its secrets"))
    monkeypatch.setattr(manager.getpass, "getpass", lambda _: pytest.fail("up asked for a provider key"))
    expected = claims_of(manager.worker_token(older["JWT_SECRET"], "test-project"))
    minted = []
    for name in ("jwt_token", "worker_token"):
        monkeypatch.setattr(manager, name, lambda *args, _name=name, _mint=getattr(manager, name), **kwargs:
                            minted.append(_name) or _mint(*args, **kwargs))
    calls = []
    monkeypatch.setattr(manager, "source_checkout", lambda _: calls.append("source"))
    # Compose interpolates the token, so it must exist when the stack starts.
    monkeypatch.setattr(manager, "compose", lambda *args: calls.append((args[0], "MCP_BEARER_TOKEN" in manager.read_env(path))))
    monkeypatch.setattr(manager, "provision", lambda _, values: calls.append(("provision", "MCP_BEARER_TOKEN" in values)) or {})
    monkeypatch.setattr(manager, "space_status", lambda *_, **__: {})
    async def verify(*_args, **_kwargs):
        return {"connected": True}
    monkeypatch.setattr(manager, "mcp_operation", verify)
    monkeypatch.setattr(sys, "argv", ["manage.py", "up"])
    manager.main()
    current = manager.read_env(path)
    token = current.pop("MCP_BEARER_TOKEN")
    # Signed with the retained secret, for this project's space, with the extractor's claims.
    header, body, signature = token.split(".")
    decode = lambda value: base64.urlsafe_b64decode(value + "=" * (-len(value) % 4))
    assert hmac.compare_digest(decode(signature), hmac.new(older["JWT_SECRET"].encode(), f"{header}.{body}".encode(), hashlib.sha256).digest())
    unstamped = lambda claims: {key: value for key, value in claims.items() if key not in {"iat", "exp"}}
    assert unstamped(claims_of(token)) == unstamped(expected)
    assert current == older
    assert (manager.ROOT.parent / ".env").read_bytes() == project
    assert calls == ["source", ("up", True), ("provision", True)]
    assert minted == ["worker_token"]
    # A folder that has the token keeps it.
    before = path.read_bytes()
    manager.main()
    assert path.read_bytes() == before and minted == ["worker_token"]


def test_bad_env_value_does_not_truncate_existing_configuration(manager):
    path = manager.ROOT / ".env"
    path.write_text("PRESERVE='yes'\n")
    with pytest.raises(ValueError):
        manager.write_env(path, {"VALUE": "broken\nvalue"})
    assert path.read_text() == "PRESERVE='yes'\n"


@pytest.mark.parametrize("configured", [False, True])
def test_up_checks_local_prerequisites_before_credentials_and_source(manager, monkeypatch, configured):
    settings = {**config(), "provider": "openai", "project": "test-project"}
    monkeypatch.setattr(manager, "load_settings", lambda: settings)
    monkeypatch.setattr(manager.shutil, "which", lambda _: None)
    calls = []
    monkeypatch.setattr(manager.getpass, "getpass", lambda _: calls.append("credential-input"))
    monkeypatch.setattr(manager, "source_checkout", lambda _: calls.append("source-checkout"))
    monkeypatch.setattr(manager, "compose", lambda *_: calls.append("compose"))
    if configured:
        (manager.ROOT / ".env").write_text("PRIVATE='preserve-knowledge-value'\n")
        (manager.ROOT.parent / ".env").write_text("PRIVATE='preserve-project-value'\n")
    before = {path: path.read_bytes() for path in manager.ROOT.parent.rglob("*") if path.is_file()}
    monkeypatch.setattr(sys, "argv", ["manage.py", "up"])
    with pytest.raises(ValueError, match="Install Docker with Compose"):
        manager.main()
    assert calls == []
    assert {path: path.read_bytes() for path in manager.ROOT.parent.rglob("*") if path.is_file()} == before
    assert not (manager.ROOT / ".source").exists()


def test_unreachable_engine_stops_up_without_disclosing_or_replacing_private_settings(manager, monkeypatch):
    monkeypatch.setattr(manager, "load_settings", lambda: {**config(), "provider": "openai", "project": "test-project"})
    monkeypatch.setattr(manager.shutil, "which", lambda _: "/trusted/docker")
    paths = [manager.ROOT / ".env", manager.ROOT.parent / ".env"]
    for path in paths:
        path.write_text("PRIVATE='https://user:secret@private.invalid/token'\n")
    before = [path.read_bytes() for path in paths]
    calls = []
    def probe(args, failure):
        calls.append(args)
        if args[1] == "compose":
            return "  --wait Wait for running services\n  --wait-timeout int Maximum wait\n"
        raise ValueError(failure)
    monkeypatch.setattr(manager, "_docker_probe", probe)
    monkeypatch.setattr(manager, "source_checkout", lambda _: pytest.fail("source was accessed before engine readiness"))
    monkeypatch.setattr(manager, "setup", lambda *_: pytest.fail("credentials were accessed before engine readiness"))
    monkeypatch.setattr(sys, "argv", ["manage.py", "up"])
    with pytest.raises(ValueError, match="Docker engine is unreachable") as error:
        manager.main()
    assert "secret" not in str(error.value)
    assert "private.invalid" not in str(error.value)
    assert calls[-1] == ["docker", "info", "--format", "{{.OSType}}"]
    assert [path.read_bytes() for path in paths] == before


@pytest.mark.parametrize("help_text", ["  --wait-timeout int Wait duration\n", "  --wait Wait\n", "unavailable --wait and --wait-timeout"])
def test_compose_requires_actual_wait_options_before_engine_access(manager, monkeypatch, help_text):
    monkeypatch.setattr(manager.shutil, "which", lambda _: "/trusted/docker")
    calls = []
    monkeypatch.setattr(manager, "_docker_probe", lambda args, _: calls.append(args) or help_text)
    with pytest.raises(ValueError, match="needs --wait and --wait-timeout support"):
        manager.local_preflight()
    assert calls == [["docker", "compose", "up", "--help"]]


@pytest.mark.parametrize("engine", ["windows\n", "\n", "linux\nprivate-detail\n"])
def test_local_preflight_requires_linux_without_echoing_engine_output(manager, monkeypatch, engine):
    monkeypatch.setattr(manager.shutil, "which", lambda _: "/trusted/docker")
    monkeypatch.setattr(manager, "_docker_probe", lambda args, _: "  --wait Wait\n  --wait-timeout int Timeout\n" if args[1] == "compose" else engine)
    with pytest.raises(ValueError, match="Linux Docker engine") as error:
        manager.local_preflight()
    assert "private-detail" not in str(error.value)


def test_local_preflight_does_not_claim_stack_or_mcp_readiness(manager, monkeypatch):
    monkeypatch.setattr(manager.shutil, "which", lambda _: "/trusted/docker")
    calls = []
    def probe(args, _):
        calls.append(args)
        return "  --wait Wait\n  --wait-timeout int Timeout\n" if args[1] == "compose" else "linux\n"
    monkeypatch.setattr(manager, "_docker_probe", probe)
    assert manager.local_preflight() is None
    assert calls == [["docker", "compose", "up", "--help"], ["docker", "info", "--format", "{{.OSType}}"]]
    assert list(manager.ROOT.iterdir()) == []


def test_offline_setup_remains_usable_without_docker(manager, monkeypatch):
    monkeypatch.setenv("OPENAI_API_KEY", "test-offline-provider-key")
    monkeypatch.setattr(manager, "load_settings", lambda: {**config(), "provider": "openai", "project": "test-project"})
    monkeypatch.setattr(manager.shutil, "which", lambda _: pytest.fail("offline setup must not inspect Docker"))
    monkeypatch.setattr(sys, "argv", ["manage.py", "setup"])
    manager.main()
    assert set(manager.read_env(manager.ROOT.parent / ".env")) == {"ELEPHANT_TOKEN", "ELEPHANT_SPACE_ID"}
    assert not (manager.ROOT / ".source").exists()


@pytest.mark.parametrize("program", [
    "import sys; print('https://user:secret@private.invalid/token'); sys.exit(1)",
    "import sys; sys.stdout.buffer.write(b'private-secret' * 10000)",
    "import sys; sys.stdout.buffer.write(b'\\xff')",
])
def test_docker_probe_has_bounded_safe_failure_without_raw_output(manager, program):
    message = "Docker prerequisite unavailable."
    with pytest.raises(ValueError) as error:
        manager._docker_probe([sys.executable, "-c", program], message)
    assert str(error.value) == message


def test_docker_probe_timeout_stops_and_reaps_owned_process(manager, monkeypatch):
    original = manager.subprocess.Popen
    children = []
    def start(*args, **kwargs):
        process = original(*args, **kwargs)
        children.append(process)
        return process
    monkeypatch.setattr(manager.subprocess, "Popen", start)
    monkeypatch.setattr(manager, "PREREQUISITE_TIMEOUT", 0.05)
    started = time.monotonic()
    with pytest.raises(ValueError, match="Docker prerequisite unavailable"):
        manager._docker_probe([sys.executable, "-c", "import time; time.sleep(30)"], "Docker prerequisite unavailable.")
    assert time.monotonic() - started < 3
    assert len(children) == 1
    assert children[0].poll() is not None
    assert children[0].stdout.closed


def test_wrong_jwt_space_is_rejected_before_network(manager):
    token = manager.jwt_token("secret", "other-project")
    with pytest.raises(ValueError, match="space"):
        manager.check_space(token, "test-project")


def mock_mcp(monkeypatch, calls, *, ready=True):
    @asynccontextmanager
    async def transport(url, headers):
        calls.append(("connect", url, headers))
        yield None, None, None
    class Session:
        def __init__(self, *_):
            pass
        async def __aenter__(self):
            return self
        async def __aexit__(self, *_):
            pass
        async def initialize(self):
            calls.append(("initialize",))
        async def list_tools(self):
            names = ["get_runtime_status", "search_all", "ingest_doc_simple"]
            if calls[0][1].endswith("/agent"):
                names.append("get_capabilities")
            return SimpleNamespace(tools=[SimpleNamespace(name=name, inputSchema={"properties": {"title": {}, "text": {}, "metadata": {}, "query": {}}}) for name in names])
        async def call_tool(self, name, args):
            calls.append((name, args))
            data = {"semantic_ingest_ready": ready} if name == "get_runtime_status" else {"accepted": True}
            return SimpleNamespace(isError=False, structuredContent={"data": data})
    module = ModuleType("mcp")
    module.ClientSession = Session
    transport_module = ModuleType("mcp.client.streamable_http")
    transport_module.streamablehttp_client = transport
    monkeypatch.setitem(sys.modules, "mcp", module)
    monkeypatch.setitem(sys.modules, "mcp.client", ModuleType("mcp.client"))
    monkeypatch.setitem(sys.modules, "mcp.client.streamable_http", transport_module)


@pytest.mark.asyncio
async def test_ingestion_waits_for_readiness_and_uses_operator_token(manager, monkeypatch):
    calls = []
    mock_mcp(monkeypatch, calls, ready=False)
    settings = {**config(), "project": "test-project"}
    with pytest.raises(ValueError, match="not ready"):
        await manager.mcp_operation(settings, "ingest", path="irrelevant.txt")
    assert calls[0] == ("connect", "http://127.0.0.1:8091/mcp", {"Authorization": "Bearer opaque-operator-token"})
    assert not any(call[0] == "ingest_doc_simple" for call in calls)


@pytest.mark.asyncio
async def test_ingestion_records_source_hash_only_after_schema_and_health(manager, monkeypatch):
    calls = []
    mock_mcp(monkeypatch, calls)
    source = manager.ROOT / "brief.txt"
    source.write_text("Project evidence")
    settings = {**config(), "project": "test-project"}
    result = await manager.mcp_operation(settings, "ingest", path=source)
    name, arguments = calls[-1]
    assert name == "ingest_doc_simple"
    assert arguments["metadata"]["source_sha256"] == hashlib.sha256(source.read_bytes()).hexdigest()
    assert arguments["metadata"]["original_filename"] == "brief.txt"
    assert result["receipt"] == {"data": {"accepted": True}}
    assert [call[0] for call in calls][2:3] == ["get_runtime_status"]


@pytest.mark.asyncio
async def test_search_uses_read_only_profile_and_credential(manager, monkeypatch):
    calls = []
    mock_mcp(monkeypatch, calls)
    await manager.mcp_operation({**config(), "project": "test-project"}, "search", query="evidence")
    assert calls[0] == ("connect", "http://127.0.0.1:8091/mcp/agent", {"Authorization": "Bearer opaque-read-token"})
    assert calls[-1] == ("search_all", {"query": "evidence"})


@pytest.mark.parametrize("structured", [True, False])
def test_native_tool_error_cannot_be_reported_as_success(manager, structured):
    error = {"ok": False, "error": "Access denied"}
    result = SimpleNamespace(isError=False, structuredContent=error if structured else None, content=[SimpleNamespace(type="text", text=json.dumps(error))])
    with pytest.raises(ValueError, match="denied"):
        manager.result_data(result)


@pytest.mark.parametrize("field,name", [("token_env", "OPENAI_API_KEY"), ("space_env", "JWT_SECRET"), ("token_env", "ELEPHANT_OPERATOR_TOKEN"), ("space_env", "PATH")])
def test_knowledge_cannot_overwrite_provider_or_runtime_environment(field, name):
    from launchkit.blueprint import Knowledge
    with pytest.raises(ValueError, match="dedicated knowledge-service"):
        Knowledge(mode="local", **{field: name})
    with pytest.raises(ValueError, match="dedicated knowledge-service"):
        generate_elephant_files({"mode": "local", field: name}, "research-lab")


class FakeKnowledgeService:
    """The space and grant administration routes, as the knowledge service answers them."""

    def __init__(self, *, space=None, grant=None):
        self.space, self.grant, self.calls = space, grant, []

    def __call__(self, settings, method, path, token, *, body=None):
        self.calls.append((method, path, token, body))
        if path == f"/v1/spaces/{settings['project']}":
            return (200, self.space) if self.space else (404, {"error_code": "unknown_space"})
        if (method, path) == ("POST", "/v1/spaces"):
            self.space = {**body, "id": "space-config-1"}
            return 201, self.space
        if path == f"/v1/spaces/{settings['project']}/grants/recall-extractor":
            if method == "GET":
                return (200, self.grant) if self.grant else (404, {"error_code": "not_found"})
            assert body["expected_generation"] is None and self.grant is None
            self.grant = {"subject": "recall-extractor", "active": body["active"], "scopes": sorted(body["scopes"]), "generation": "grant-1"}
            return 200, self.grant
        raise AssertionError(f"unexpected {method} {path}")


def provisioned_project(manager, provider="openai"):
    files = generate_elephant_files(config(), "test-project", provider=provider)
    (manager.ROOT / "compose.json").write_text(files["knowledge/compose.json"])
    settings = json.loads(files["knowledge/settings.json"])
    return settings, {"JWT_SECRET": "s" * 48}


def test_up_creates_the_space_and_grant_once_in_the_documented_order(manager, monkeypatch):
    settings, values = provisioned_project(manager)
    service = FakeKnowledgeService()
    monkeypatch.setattr(manager, "api_request", service)
    result = manager.provision(settings, values)
    assert [(method, path) for method, path, *_ in service.calls] == [
        ("GET", "/v1/spaces/test-project"), ("POST", "/v1/spaces"), ("GET", "/v1/spaces/test-project"),
        ("GET", "/v1/spaces/test-project/grants/recall-extractor"),
        ("PUT", "/v1/spaces/test-project/grants/recall-extractor"),
        ("GET", "/v1/spaces/test-project/grants/recall-extractor")]
    assert service.calls[4][3] == {"active": True, "scopes": manager.WORKER_SCOPES, "expected_generation": None}
    assert all(claims_of(token)["bootstrap_space"] == "test-project" for _, _, token, _ in service.calls)
    assert result["extractor_grant"] == {"subject": "recall-extractor", "generation": "grant-1", "scopes": sorted(manager.WORKER_SCOPES)}
    # A later up only reads both back.
    service.calls.clear()
    manager.provision(settings, values)
    assert [method for method, *_ in service.calls] == ["GET", "GET"]
    # The bootstrap credential is never saved.
    used = {token for _, _, token, _ in service.calls}
    assert not any(token in path.read_text(errors="replace") for path in manager.ROOT.parent.rglob("*") if path.is_file() for token in used)


@pytest.mark.parametrize("provider", ["openai", "anthropic"])
def test_space_request_approves_each_semantic_operation_with_named_credentials(manager, provider):
    settings, _ = provisioned_project(manager, provider)
    request = manager.space_request(settings)
    processing = request["processing"]
    assert request["name"] == "test-project" and request["security"] == {"policy_id": "default", "level": 0, "labels": [], "handling": []}
    assert set(processing["bindings"]) == {"query_embedding", "document_embedding", "extraction", "support_verification"}
    profiles = processing["profiles"]
    for operation, binding in processing["bindings"].items():
        profile = profiles[binding["profile_id"]]
        assert binding["revoked"] is False and (profile["dimensions"] is not None) == operation.endswith("_embedding")
    active = profiles[processing["active_embedding_profile_id"]]
    connection = processing["connections"][active["connection_id"]]
    assert (request["embedding_provider"], request["embedding_model"], request["embedding_dims"]) == (
        connection["provider"], active["model"], active["dimensions"]) == ("openai", "text-embedding-3-large", 3072)
    generation = profiles[processing["bindings"]["extraction"]["profile_id"]]
    assert generation["model"] == settings["model"]
    assert processing["connections"][generation["connection_id"]]["provider"] == provider
    for connection in processing["connections"].values():
        assert re.fullmatch(r"[A-Z_]+_API_KEY", connection["credential_ref"])
        assert connection["endpoint"].startswith("https://") and connection["revoked"] is False


def test_an_inactive_extractor_grant_is_reported_not_rewritten(manager, monkeypatch):
    settings, values = provisioned_project(manager)
    service = FakeKnowledgeService()
    monkeypatch.setattr(manager, "api_request", service)
    manager.provision(settings, values)
    service.grant = {**service.grant, "active": False}
    service.calls.clear()
    with pytest.raises(ValueError, match="recall-extractor grant"):
        manager.provision(settings, values)
    assert [method for method, *_ in service.calls] == ["GET", "GET"]


def test_space_status_waits_for_semantic_readiness(manager, monkeypatch):
    settings, _ = provisioned_project(manager)
    monkeypatch.setattr(manager, "environment", lambda: {"ELEPHANT_OPERATOR_TOKEN": "opaque-operator-token"})
    answers = iter([(503, {"semantic_ingest_ready": False}), (200, {"semantic_ingest_ready": True, "providers_ready": True})])
    calls = []
    monkeypatch.setattr(manager, "api_request", lambda settings, method, path, token, **_: calls.append((method, path, token)) or next(answers))
    monkeypatch.setattr(manager.time, "sleep", lambda _: None)
    assert manager.space_status(settings, wait=60)["semantic_ingest_ready"] is True
    assert calls == [("GET", "/v1/spaces/test-project/status", "opaque-operator-token")] * 2
