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
import subprocess
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
# HOME and XDG_CONFIG_HOME only locate Meerkat's folders, which the private-file guard never reads.
ALLOWED_ENVIRONMENT = {"META_FORGE_PORT", "MOBKIT_LAUNCH_PORT", "META_FORGE_HOME", "MOBKIT_LAUNCH_HOME",
                       "NO_COLOR", "PATH", "HOME", "XDG_CONFIG_HOME"}
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
            "COPILOT_GITHUB_TOKEN": CANARY, "RKAT_GITHUB_TOKEN": CANARY}


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
    ready = {"available": True, "status": "ready", "provider": "openai", "model": "gpt-5.5"}
    architect = SimpleNamespace(guide_context=None, proposal=None, guide_result=None, message_lock=asyncio.Lock(),
                                start_lock=asyncio.Lock(), pending_interaction=None,
                                refresh_status=AsyncMock(return_value=ready), start=AsyncMock(return_value=ready),
                                stop_for_provider=AsyncMock(), status=lambda: {**ready, "status": "stopped"})
    monkeypatch.setattr(server, "guide", guide)
    monkeypatch.setattr(server, "guide_operations", operations)
    monkeypatch.setattr(server, "deployment", installs)
    monkeypatch.setattr(server, "architect", architect)
    monkeypatch.setattr(server, "provider_owned_work", lambda: False)
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
    request = (await run(forge, "installation", "stop", identity, "--no-wait", "--json")).document["request"]
    commands = [
        ["version"], ["doctor"], ["provider", "status"], ["guide", "status"], ["guide", "start"], ["guide", "stop"],
        ["setup", "list", "--all"],
        ["setup", "show", identity], ["setup", "timeline", identity], ["plan", "show", identity],
        ["plan", "export", identity], ["plan", "validate", str(blueprint)],
        ["plan", "package", identity, "--out", str(tmp_path / f"team-{mode[-1]}.zip")],
        [*install, "--dry-run"], install[:-1] + ["--folder", str(tmp_path / f"other-{mode[-1]}")],
        ["installation", "status", identity], ["installation", "log", identity],
        ["installation", "log", identity, "--follow", "--timeout", "300ms"],
        ["installation", "open", identity], ["installation", "stop", identity, "--no-wait"],
        ["installation", "request", identity, request],
        ["installation", "settings", "show", identity], ["installation", "export", identity, "--out", str(tmp_path / "x")],
        ["install", identity, "--team", "--folder", str(tmp_path / "new"), "--set", "OPENAI_API_KEY=" + "x" * 8],
        ["installation", "start", identity], ["installation", "restart", identity], ["installation", "retry", identity],
        ["installation", "cleanup", identity], ["setup", "organization", identity],
        ["setup", "revisit", identity, "purpose"], ["setup", "retry", identity],
    ]
    for argv in commands:
        result = await run(forge, "-v", *mode, *argv)
        clean(result, label=" ".join(argv))
        assert result.code in (0, 2, 81, 82, 83, 84, 85, 86, 87), (argv, result.text)
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
    assert {"OPENAI_API_KEY", "RKAT_ANTHROPIC_API_KEY", "GITHUB_TOKEN", "COPILOT_GITHUB_TOKEN",
            "RKAT_GITHUB_TOKEN"}.isdisjoint(environment)
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


def private_files(forge):
    """Forge's private files and Meerkat's stores, each holding the canary, and a link to one of them."""
    home = forge.root / "home"
    files = {
        "Forge's saved provider keys": forge.settings,
        "a file in Forge's state folder": forge.state / "notes.json",
        "an installation's .env": forge.root / "team-folder" / ".env",
        "an installation's .mobkit file": forge.root / "team-folder" / ".mobkit" / "settings.json",
        "Meerkat's credential store": forge.config / "meerkat" / "credentials" / "global" / "default_openai.json",
        "Meerkat's credential store under ~/.config":
            home / ".config" / "meerkat" / "credentials" / "global" / "default_anthropic.json",
        "Meerkat's user configuration": home / ".rkat" / "config.toml",
    }
    for path in files.values():
        path.parent.mkdir(parents=True, exist_ok=True)
        if not path.exists():
            path.write_text(json.dumps({"token": CANARY}))
    link = forge.root / "weekly-notes.json"
    link.symlink_to(forge.settings)
    files["a link to Forge's saved provider keys"] = link
    return files


def opened(result, path):
    """The files the run opened that are this path."""
    target = path.resolve()
    return [arguments[0] for event, arguments in result.events_seen
            if event == "open" and isinstance(arguments[0], (str, bytes, os.PathLike))
            and Path(os.fsdecode(arguments[0])).resolve() == target]


async def refusal_problems(forge, reader, identity, files):
    """What went wrong when each file reaches the option: not refused, opened, sent or shown."""
    problems = []
    for label, path in files.items():
        forge.transport.requests.clear()
        if reader == "chat /attach":
            result = await run(forge, "chat", identity, stdin=Terminal(f"/attach {path}\n/quit\n"), terminal=True)
            refused = result.code == 0 and "never reads it" in result.stderr
        else:
            argv = (["plan", "validate", str(path)] if reader == "plan validate"
                    else ["setup", "send", identity, reader, str(path)])
            result = await run(forge, *argv, "--json")
            refused = (result.code == 2 and result.document.get("error", {}).get("code") == "invalid_request"
                       and "never reads it" in result.document["error"]["detail"])
        sent = [route for method, route, _, _ in forge.transport.requests if method == "POST"]
        problems += [f"{label}: {problem}" for problem, found in (
            (f"not refused (exit {result.code})", not refused), ("the canary was shown", CANARY in result.text),
            ("opened", bool(opened(result, path))), (f"sent {sent}", bool(sent)),
            ("a child process carried the canary", any(mentions(arguments) for event, arguments
                                                        in result.events_seen if event != "open"))) if found]
    return problems


@pytest.mark.parametrize("reader", ["--text-file", "--attach", "plan validate", "chat /attach"])
async def test_every_file_option_refuses_private_files_without_reading_them(forge, reader):
    identity = (await run(forge, "setup", "new", "--json")).document["setup"]
    problems = await refusal_problems(forge, reader, identity, private_files(forge))
    assert not problems, f"{reader}: " + "; ".join(problems)


@pytest.mark.skipif(not Path("/proc/self/environ").is_file(), reason="this platform has no /proc")
@pytest.mark.parametrize("reader", ["--text-file", "--attach", "plan validate", "chat /attach"])
async def test_every_file_option_refuses_process_environments(forge, reader):
    """/proc shows the environment of this command (/proc/self/environ) and of every other process of the same
    user: here another process that holds the canary, also reached through a symbolic link."""
    other = subprocess.Popen(["sleep", "60"], env={"PATH": os.environ.get("PATH", "/usr/bin:/bin"),
                                                   "OPENAI_API_KEY": CANARY})
    try:
        link = forge.root / "environment.txt"
        link.symlink_to(f"/proc/{other.pid}/environ")
        files = {"this command's environment": Path("/proc/self/environ"),
                 "another process's environment": Path(f"/proc/{other.pid}/environ"),
                 "a link to another process's environment": link}
        identity = (await run(forge, "setup", "new", "--json")).document["setup"]
        problems = await refusal_problems(forge, reader, identity, files)
    finally:
        other.kill()
        other.wait()
    assert not problems, f"{reader}: " + "; ".join(problems)


@pytest.mark.parametrize("argv,option", [
    (["setup", "send", "last", "--text-file", CANARY], "--text-file"),
    (["setup", "send", "last", "--attach", CANARY + ".txt"], "--attach"),
    (["plan", "validate", CANARY], "plan validate"),
])
@pytest.mark.parametrize("mode", [[], ["--json"]])
async def test_a_file_argument_error_names_the_option_never_the_value(forge, argv, option, mode):
    await run(forge, "setup", "new", "--json")
    result = await run(forge, *mode, *argv)
    assert result.code == 2, result.text
    assert CANARY not in result.stdout + result.stderr
    assert option in result.stdout + result.stderr


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


UPPER_CANARY = "CANARY_6F1D0C2A_SECRET_VALUE"
HEX_CANARY = "6f1d0c2adeadbeef0123"


def answering_forge():
    """A Forge server's answers: GUIDE_ID asks a question beside a proposal; OTHER_ID has an accepted plan."""
    from cli_support import GUIDE_ID, OTHER_ID
    from test_cli_api import accepted_state, guide_state, options, plan
    answers = {"/api/capabilities": {"version": cli_version(), "planner": {"available": True},
                                     "agent_team_runtime": {}},
               "/api/guide/list": {"setups": [{"id": identity, "name": "x", "status": "question", "stage": "purpose",
                                               "updated_at": "2026-11-09T10:00"} for identity in (GUIDE_ID, OTHER_ID)]},
               f"/api/guide/{GUIDE_ID}": guide_state(proposal=plan()),
               f"/api/guide/{OTHER_ID}": accepted_state(id=OTHER_ID),
               f"/api/guide/{OTHER_ID}/deployment/options": options()}

    def handler(request):
        if request.url.path in answers:
            return httpx.Response(200, json=answers[request.url.path])
        return httpx.Response(404, json={"detail": "Not Found"})
    return httpx.MockTransport(handler)


def argument_rows(tmp_path):
    """Each way a value typed in the wrong place can reach an argument error, with the option it names."""
    from cli_support import GUIDE_ID, OTHER_ID
    taken = tmp_path / CANARY
    taken.write_text("{}")
    install = ["install", OTHER_ID, "--team", "--folder", str(tmp_path / "team")]
    return [
        ([CANARY], "COMMAND"), (["provider", CANARY], "ACTION"), (["setup", "revisit", GUIDE_ID, CANARY], "TOPIC"),
        (["setup", "timeline", GUIDE_ID, "--limit", CANARY], "--limit"), (["-o", CANARY, "setup", "list"], "--output"),
        ([*install, "--mode", CANARY], "--mode"), (["setup", "list", f"--no={CANARY}"], "--no"),
        ([*install, f"--reuse-forge-keys={CANARY}"], "--reuse-forge-keys"),
        (["install", OTHER_ID, f"--team={CANARY}", "--folder", str(tmp_path / "team")], "--team"),
        (["setup", "list", f"--all={CANARY}"], "--all"), (["provider", "status", f"-k{CANARY}"], "-k"),
        (["provider", "status", CANARY], "unrecognized"), (["provider", "status", f"--api-key={CANARY}"], "--api-key"),
        (["setup", "show", CANARY], "SETUP"), (["setup", "show", HEX_CANARY], "SETUP"),
        (["setup", "send", GUIDE_ID, "--answer", CANARY], "--answer"),
        (["plan", "export", GUIDE_ID, "--out", str(taken)], "--out"),
        ([*install, "--set", f"{UPPER_CANARY}="], "--set"),
        ([*install, "--set", f"{UPPER_CANARY}=a", "--set", f"{UPPER_CANARY}=b"], "--set"),
        ([*install, "--reuse-forge-keys", "--set", f"{UPPER_CANARY}=a"], "--set"),
        ([*install, "--reuse-forge-keys", "--set", "CONSOLE_USER_ID=o", "--approve", CANARY], "--approve"),
        (["--server", CANARY, "setup", "list"], "--server"), (["--timeout", CANARY, "setup", "list"], "--timeout"),
        (["setup", "new", "--request-id", CANARY], "--request-id"),
        (["installation", "request", GUIDE_ID, CANARY], "ID"),
    ]


@pytest.mark.parametrize("mode", ["text", "json"])
async def test_argument_errors_name_the_option_never_the_value(tmp_path, mode):
    problems = []
    for argv, option in argument_rows(tmp_path):
        result = await cli([*argv, *(["--json"] if mode == "json" else [])], transport=answering_forge(),
                           environ={"META_FORGE_PORT": "8787", "PATH": os.environ.get("PATH", "/usr/bin:/bin")})
        shown = result.text
        if mode == "json":
            document = json.loads(result.stdout)
            shown = json.dumps(document) + result.stderr
            if document["ok"] or option not in document["error"]["detail"]:
                problems.append(f"{argv}: the JSON error does not name {option}: {document.get('error')}")
        elif option not in result.stderr:
            problems.append(f"{argv}: the error does not name {option}")
        if result.code not in (2, 81):
            problems.append(f"{argv}: exit {result.code}")
        for value in (CANARY, UPPER_CANARY, HEX_CANARY):
            if value in shown:
                problems.append(f"{argv}: the value given is shown")
    assert not problems, "\n".join(problems)


async def test_an_unknown_chat_command_is_not_repeated():
    from cli_support import GUIDE_ID
    result = await cli(["chat", GUIDE_ID], transport=answering_forge(), stdin=Terminal(f"/{CANARY}\n/quit\n"),
                       terminal=True)
    assert result.code == 0, result.text
    assert CANARY not in result.text and "/help" in result.text


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
