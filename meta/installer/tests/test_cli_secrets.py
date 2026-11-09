"""meta-forge secret hygiene: no command reads, receives, forwards, stores, prints or logs a secret.

A canary value sits in Forge's provider-settings.json, in an installation's
.env (copied there by Forge itself), in a temporary credential store and in
the command's own environment, as an agent's shell would hold it. Every
command then runs in text, json and jsonl output with -v. An audit hook
records the files the command opens and the child processes it starts.
"""
import ast
import asyncio
import json
import logging
import os
from pathlib import Path
import sys
from types import SimpleNamespace
from unittest.mock import AsyncMock

import httpx
import pytest

from cli_support import Recording, Terminal, cli
from launchkit import server
from launchkit.cli import cli_version
from launchkit.deployment import materialize
from launchkit.guide import GuideService
from launchkit.guide_operations import GuideOperations
from launchkit.guide_store import GuideStore
from launchkit.implementation import NoApplicationBuild
from launchkit.provider_settings import ProviderSettings
from test_cli_journeys import free_port, review_reply
from test_deployment import Service
from test_deployment_native_team import NoImplementation
from test_guide import SetupAgent

CANARY = "canary-6f1d0c2a-secret-value-never-shown"
CLI_ROOT = Path(__file__).resolve().parents[1] / "launchkit" / "cli"
ALLOWED_ENVIRONMENT = {"META_FORGE_PORT", "MOBKIT_LAUNCH_PORT", "META_FORGE_HOME", "MOBKIT_LAUNCH_HOME",
                       "NO_COLOR", "PATH"}
WATCHED = ("open", "subprocess.Popen", "os.exec", "os.posix_spawn", "os.spawn", "os.system")
RECORDED = None


def _audit(event, arguments):
    if RECORDED is not None and event in WATCHED:
        RECORDED.append((event, arguments))


sys.addaudithook(_audit)


def mentions(value, needle=CANARY):
    if isinstance(value, bytes):
        return needle.encode() in value
    if isinstance(value, str):
        return needle in value
    if isinstance(value, dict):
        return any(mentions(key, needle) or mentions(item, needle) for key, item in value.items())
    if isinstance(value, (list, tuple)):
        return any(mentions(item, needle) for item in value)
    return False


def private_state(root):
    """Forge's state directory with a saved provider key, and a temporary credential store."""
    state = root / "state"
    state.mkdir(mode=0o700)
    settings = state / "provider-settings.json"
    settings.write_text(json.dumps({"version": 1, "keys": {"openai": CANARY}}))
    settings.chmod(0o600)
    store = root / "config" / "meerkat" / "credentials" / "global"
    store.mkdir(parents=True, mode=0o700)
    (store / "default_openai.json").write_text(json.dumps({"api_key": CANARY}))
    return state, settings, root / "config"


def cli_environment(root, state):
    """The command's own environment: an agent shell that happens to hold credentials."""
    return {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": str(root / "home"),
            "XDG_CONFIG_HOME": str(root / "config"), "META_FORGE_HOME": str(state), "META_FORGE_PORT": "8787",
            "OPENAI_API_KEY": CANARY, "RKAT_ANTHROPIC_API_KEY": CANARY, "GITHUB_TOKEN": CANARY,
            "COPILOT_GITHUB_TOKEN": CANARY}


@pytest.fixture
def forge(tmp_path, monkeypatch):
    from launchkit import guide as guide_module
    monkeypatch.setattr(guide_module, "planner_capability",
                        lambda: {"available": True, "provider": "openai", "reason": "Ready"})
    state, settings, config = private_state(tmp_path)
    agent = SetupAgent()
    agent.reply = review_reply
    guide = GuideService(GuideStore(state), agent)

    def no_application(_state):
        raise NoApplicationBuild("No application build")

    guide.implementation_evidence_provider = no_application
    operations = GuideOperations()
    installs = Service(state, guide, NoImplementation(), operations)
    installs.service_manager = SimpleNamespace(capability=AsyncMock(return_value={"available": False}))
    saved = ProviderSettings(state, environ={})
    architect = SimpleNamespace(guide_context=None, proposal=None, guide_result=None, message_lock=asyncio.Lock(),
                                refresh_status=AsyncMock(return_value={"available": True, "status": "ready",
                                                                       "provider": "openai", "model": "gpt-5.5"}))
    monkeypatch.setattr(server, "guide", guide)
    monkeypatch.setattr(server, "guide_operations", operations)
    monkeypatch.setattr(server, "deployment", installs)
    monkeypatch.setattr(server, "architect", architect)
    monkeypatch.setattr(server, "cloud_capability", lambda: {"available": False, "project": None, "reason": "test"})
    monkeypatch.setattr(server, "guide_timeline", SimpleNamespace(read=AsyncMock(return_value={
        "available": True, "frames": [], "next_cursor": None, "latest_cursor": None, "exhausted": True})))
    # The real presence projection over the real saved file.
    monkeypatch.setattr(server, "provider_public_status", lambda: {
        **saved.status(), "locked_provider": None, "provider_binding_error": None, "work_active": False,
        "runtime_status": "ready"})
    # Forge restored its saved key into its own process environment, as its lifespan does.
    monkeypatch.setenv("OPENAI_API_KEY", CANARY)
    yield SimpleNamespace(root=tmp_path, state=state, settings=settings, config=config, guide=guide,
                          installs=installs, transport=Recording(server.app))
    for task in list(installs.tasks.values()):
        task.cancel()


async def run(forge, *argv, **options):
    global RECORDED
    RECORDED = []
    try:
        result = await cli(list(argv), transport=forge.transport,
                           environ=cli_environment(forge.root, forge.state), **options)
    finally:
        events, RECORDED = RECORDED, None
    result.events_seen = events
    return result


def clean(result, *, label):
    assert CANARY not in result.stdout and CANARY not in result.stderr, label
    for event, arguments in result.events_seen:
        if event != "open":
            assert not mentions(arguments), f"{label}: {event} carried the canary"


async def installed(forge):
    created = await run(forge, "setup", "new", "--json")
    identity = created.document["setup"]
    assert (await run(forge, "setup", "send", identity, "--text", "Compare two heat-pump proposals",
                      "--wait", "--json")).code == 0
    review = await run(forge, "plan", "accept", identity, "--json")
    assert (await run(forge, "plan", "accept", identity, "--approve", review.document["digest"], "--json")).code == 0
    folder = forge.root / "team"
    argv = ["install", identity, "--team", "--folder", str(folder), "--port", str(free_port()), "--reuse-forge-keys"]
    review = await run(forge, *argv, "--json")
    result = await run(forge, *argv, "--approve", review.document["digest"], "--no-wait", "--json")
    assert result.code == 0, result.text
    assert CANARY in (folder / ".env").read_text(), "Forge itself copies its saved key into the installation"
    materialize(forge.installs._directory(identity))
    return identity, folder, argv


@pytest.mark.parametrize("mode", [["-o", "text"], ["--json"], ["-o", "jsonl"]])
async def test_every_command_keeps_secrets_out_of_output_children_and_logs(forge, tmp_path, mode, caplog):
    caplog.set_level(logging.DEBUG)
    identity, folder, install = await installed(forge)
    blueprint = tmp_path / "plan.json"
    blueprint.write_text(json.dumps(forge.guide.store.get(identity)["blueprint"]))
    commands = [
        ["version"], ["doctor"], ["provider", "status"], ["guide", "status"], ["setup", "list", "--all"],
        ["setup", "show", identity], ["setup", "timeline", identity], ["plan", "show", identity],
        ["plan", "export", identity], ["plan", "validate", str(blueprint)],
        ["plan", "package", identity, "--out", str(tmp_path / f"team-{mode[-1]}.zip")],
        [*install, "--dry-run"], install[:-1] + ["--folder", str(tmp_path / f"other-{mode[-1]}")],
        ["installation", "status", identity], ["installation", "log", identity],
        ["installation", "open", identity], ["installation", "stop", identity, "--no-wait"],
        ["installation", "settings", "show", identity], ["installation", "export", identity, "--out", str(tmp_path / "x")],
        ["install", identity, "--team", "--folder", str(tmp_path / "new"), "--set", "OPENAI_API_KEY=" + "x" * 8],
        ["installation", "start", identity], ["setup", "organization", identity],
        ["setup", "revisit", identity, "purpose"], ["setup", "retry", identity],
    ]
    for argv in commands:
        result = await run(forge, "-v", *mode, *argv)
        clean(result, label=" ".join(argv))
        assert result.code in (0, 2, 82, 83, 84, 85, 86), (argv, result.text)
    for record in caplog.records:
        assert CANARY not in record.getMessage()
    settings = await run(forge, "installation", "settings", "show", identity, "--json")
    assert any(field["key"] == "OPENAI_API_KEY" and field["configured"] for field in settings.document["result"]["fields"])


async def test_a_needed_secret_stops_with_a_handle_and_never_a_value(forge):
    identity, _, _ = await installed(forge)
    result = await run(forge, "install", identity, "--team", "--folder", str(forge.root / "second"), "--json")
    assert (result.code, result.document["error"]["code"]) == (85, "input_required")
    (need,) = result.document["needs"]
    assert need["kind"] == "needs_secret" and need["handle"] == f"setting:{identity}:new:OPENAI_API_KEY"
    clean(result, label="install without a secret")
    refused = await run(forge, "install", identity, "--team", "--folder", str(forge.root / "third"),
                        "--set", f"OPENAI_API_KEY={CANARY}", "--json")
    assert (refused.code, refused.document["error"]["code"]) == (2, "invalid_request")
    assert CANARY not in refused.stdout + refused.stderr
    assert not (forge.root / "third").exists()


async def test_provider_status_reports_presence_only_from_the_saved_file(forge):
    result = await run(forge, "-v", "provider", "status", "--json")
    rows = {row["provider"]: row for row in result.document["result"]["providers"]}
    assert rows["openai"]["configured"] is True and rows["openai"]["source"] == "saved"
    clean(result, label="provider status")


async def test_the_command_never_opens_forge_secret_stores(forge, tmp_path):
    """Against canned answers only, so every recorded open is the command's own."""
    envfile = tmp_path / "installed" / ".env"
    envfile.parent.mkdir()
    envfile.write_text(f"OPENAI_API_KEY={CANARY}\n")
    answers = {"/api/capabilities": {"version": cli_version(), "planner": {"available": True},
                                     "agent_team_runtime": {}},
               "/api/provider-settings": {"providers": [{"provider": "openai", "configured": True, "source": "saved"}]},
               "/api/guide/list": {"setups": []}}

    def handler(request):
        return httpx.Response(200, json=answers.get(request.url.path, {}))

    protected = [forge.settings, envfile, forge.config]
    for argv in (["version"], ["doctor"], ["provider", "status"], ["setup", "list"], ["serve", "--prepare-only"]):
        global RECORDED
        RECORDED = []
        try:
            result = await cli(argv, transport=httpx.MockTransport(handler),
                               environ=cli_environment(forge.root, forge.state), executor=lambda *a: None)
        finally:
            events, RECORDED = RECORDED, None
        assert CANARY not in result.stdout + result.stderr, argv
        opened = [Path(os.fsdecode(arguments[0])) for event, arguments in events
                  if event == "open" and isinstance(arguments[0], (str, bytes, os.PathLike))]
        for path in opened:
            resolved = path.resolve() if path.is_absolute() else path
            assert not any(resolved == item or item in resolved.parents for item in protected), (argv, path)


async def test_serve_withholds_provider_variables_unless_asked(forge):
    calls = []

    def executor(path, argv, environment):
        calls.append((path, argv, environment))

    result = await run(forge, "serve", "--port", "8899", executor=executor)
    assert result.code == 0
    (path, argv, environment) = calls[-1]
    assert argv[1].endswith("start.sh") and environment["META_FORGE_PORT"] == "8899"
    assert not mentions(environment) and not mentions(argv)
    assert {"OPENAI_API_KEY", "RKAT_ANTHROPIC_API_KEY", "GITHUB_TOKEN", "COPILOT_GITHUB_TOKEN"}.isdisjoint(environment)
    assert "Not forwarding" in result.stderr and CANARY not in result.stderr
    assert environment["META_FORGE_HOME"] == str(forge.state) and environment["XDG_CONFIG_HOME"] == str(forge.config)
    result = await run(forge, "serve", "--inherit-provider-env", "--home", str(forge.root / "other"), executor=executor)
    (_, _, environment) = calls[-1]
    assert environment["OPENAI_API_KEY"] == CANARY and environment["META_FORGE_HOME"] == str(forge.root / "other")
    assert CANARY not in result.stdout + result.stderr
    refused = await run(forge, "serve", "--json", executor=executor)
    assert refused.code == 2 and len(calls) == 2


async def test_chat_keeps_secrets_out_of_the_terminal(forge):
    script = Terminal("Compare two heat-pump proposals\n/status\n/plan\n/open\n/quit\n")
    result = await run(forge, "chat", "--new", "-v", stdin=script, terminal=True)
    assert result.code == 0, result.text
    clean(result, label="chat")


@pytest.mark.parametrize("stray", [
    ["--password", CANARY], [f"--api-key={CANARY}"], [CANARY], ["--set", CANARY], ["--set", CANARY + "="],
    ["--set", f"{CANARY}=x"],
])
@pytest.mark.parametrize("mode", [[], ["--json"]])
async def test_a_secret_typed_in_the_wrong_place_is_never_repeated(forge, stray, mode):
    result = await run(forge, "-v", *mode, "install", "last", "--team", "--folder", str(forge.root / "x"), *stray)
    assert result.code == 2, result.text
    assert CANARY not in result.stdout + result.stderr
    if stray[0].startswith("--") and stray[0] != "--set":
        assert stray[0].split("=")[0] in result.stdout + result.stderr
    assert not forge.transport.requests, "a usage error contacts no server"


def environment_reads(tree):
    """String keys the source reads from an environment mapping or os.getenv."""
    names = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute):
            owner = ast.unparse(node.func.value)
            if (node.func.attr in ("get", "pop", "getenv") and ("environ" in owner or owner == "os")
                    and node.args and isinstance(node.args[0], ast.Constant)):
                names.add(node.args[0].value)
        elif isinstance(node, ast.Subscript) and "environ" in ast.unparse(node.value) and isinstance(node.slice, ast.Constant):
            names.add(node.slice.value)
        elif isinstance(node, ast.Compare) and any(isinstance(op, (ast.In, ast.NotIn)) for op in node.ops):
            if isinstance(node.left, ast.Constant) and any("environ" in ast.unparse(item) for item in node.comparators):
                names.add(node.left.value)
    return names


def test_the_command_source_reads_no_credential_from_the_environment():
    read = set()
    for path in sorted(CLI_ROOT.rglob("*.py")):
        source = path.read_text()
        assert "getpass" not in source and "secret_value" not in source, path
        read |= environment_reads(ast.parse(source))
    assert read <= ALLOWED_ENVIRONMENT, sorted(read - ALLOWED_ENVIRONMENT)
    assert {"META_FORGE_PORT", "META_FORGE_HOME"} <= read
