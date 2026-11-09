"""Forge's launcher, local checks, versions and Forge's own setup team."""
from __future__ import annotations

import asyncio
import os
from pathlib import Path
import platform
import shutil
import socket
import stat
import sys

from .. import FORGE_ROOT, cli_version, port_number
from ..errors import CliError, step

# Provider credentials a caller's shell may hold. serve forwards none of them
# to Forge unless --inherit-provider-env is given. Meerkat reads RKAT_<NAME>
# before <NAME> for every credential variable, so each name counts with that
# prefix too.
PROVIDER_VARIABLES = {"COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"}
STARTUP_PHASES = {"storage_preparation": "preparing storage", "gateway_preparation": "preparing the gateway",
                  "native_bootstrap": "starting the agents", "identity_warmup": "warming up the agents",
                  "readiness_check": "checking readiness"}
PROVIDERS = ("openai", "anthropic", "gemini")
PROTECTED_MAC_FOLDERS = ("Desktop", "Documents", "Downloads", "Library/Mobile Documents")


def provider_variable(name):
    base = name.removeprefix("RKAT_")
    return base.endswith("_API_KEY") or base in PROVIDER_VARIABLES


def child_environment(environ, *, inherit_provider):
    """The environment a child of meta-forge receives: no provider credentials unless asked."""
    return {name: value for name, value in environ.items() if inherit_provider or not provider_variable(name)}


def state_directory(context):
    """The state directory start.sh's server would use (relative settings are relative to the Forge folder)."""
    selected = context.option("home")
    if selected:
        return Path(os.path.abspath(os.path.expanduser(selected)))
    configured = context.environ.get("META_FORGE_HOME") or context.environ.get("MOBKIT_LAUNCH_HOME")
    if configured:
        return (FORGE_ROOT / Path(configured).expanduser()).resolve()
    return FORGE_ROOT / ".studio"


def provider_needs(server_url):
    return [{"kind": "needs_secret", "handle": "forge-provider:*",
             "one_of": [f"forge-provider:{name}" for name in PROVIDERS],
             "why": "Forge's own agents (the guide and its setup team) need a model provider key.",
             "instruction": "A person enters the key in Forge's Guide settings, in a browser on this computer "
                            "(over SSH, forward the port first: ssh -L PORT:127.0.0.1:PORT HOST). "
                            "This command never takes the key.",
             "person_opens": server_url + "/"}]


def register(commands, command_parser):
    serve = command_parser(commands, "serve", "Run Forge's launcher (start.sh) in this terminal.")
    serve.add_argument("--port", type=port_number, help="port for Forge (sets META_FORGE_PORT)")
    serve.add_argument("--prepare-only", action="store_true",
                       help="prepare Forge's environment without starting it (start.sh --prepare-only)")
    serve.add_argument("--inherit-provider-env", action="store_true",
                       help="forward provider keys and tokens from this shell's environment to Forge; unsafe "
                            "where other people have accounts on this computer, since they can reach Forge's "
                            "local port")
    serve.set_defaults(func=serve_command, name="serve")
    doctor = command_parser(commands, "doctor", "Check this computer for Forge: one ok, warn or fail line each.")
    doctor.set_defaults(func=doctor_command, name="doctor")
    version = command_parser(commands, "version", "This command's Forge release, the server's, and its runtimes.")
    version.set_defaults(func=version_command, name="version")
    provider = command_parser(commands, "provider", "Model providers for Forge's own agents.")
    actions = provider.add_subparsers(dest="action", metavar="ACTION", required=True)
    status = command_parser(actions, "status", "Which providers are configured for Forge's own agents (never values).")
    status.set_defaults(func=provider_status, name="provider status")
    guide = command_parser(commands, "guide", "Forge's setup team: status, start and stop.")
    actions = guide.add_subparsers(dest="action", metavar="ACTION", required=True)
    for name, function, text in (("status", guide_status, "The setup team's status."),
                                 ("start", guide_start, "Start the setup team and wait until it is ready."),
                                 ("stop", guide_stop, "Stop the idle setup team.")):
        parser = command_parser(actions, name, text)
        parser.set_defaults(func=function, name="guide " + name)


async def serve_command(context):
    args = context.args
    if context.output.mode != "text":
        raise CliError("usage", "serve runs Forge's launcher in this terminal; it has no JSON output.",
                       next=[step("meta-forge serve", "Run it without --json in its own terminal.")])
    launcher = FORGE_ROOT / "start.sh"
    if not launcher.is_file():
        raise CliError("prerequisite_unavailable", f"{launcher} is missing. Unpack a complete Forge package.",
                       next=[step("meta-forge doctor", "Check this Forge folder.")])
    environment = child_environment(context.environ, inherit_provider=args.inherit_provider_env)
    if args.port is not None:
        environment["META_FORGE_PORT"] = str(args.port)
    home = context.option("home")
    if home:
        environment["META_FORGE_HOME"] = os.path.abspath(os.path.expanduser(home))
    withheld = sorted(name for name in context.environ if provider_variable(name))
    if withheld and not args.inherit_provider_env:
        context.output.note("Not forwarding " + ", ".join(withheld) + " from this shell to Forge. Forge uses the "
                            "keys saved in its Guide settings; add --inherit-provider-env to forward them.")
    bash = shutil.which("bash", path=environment.get("PATH")) or "/bin/bash"
    argv = [bash, str(launcher), *(["--prepare-only"] if args.prepare_only else [])]
    context.output.finished = True
    context.executor(bash, argv, environment)
    return 0


def isolation_check():
    if sys.platform == "darwin":
        found = shutil.which("sandbox-exec")
        return ("ok", f"macOS sandbox-exec at {found}") if found else (
            "warn", "sandbox-exec is unavailable: application checks cannot run")
    if sys.platform == "linux":
        try:
            from ..linux_sandbox import executable
            found = executable()
        except Exception:
            found = None
        if found:
            return "ok", f"bubblewrap isolation works ({found})"
        restricted = Path("/proc/sys/kernel/apparmor_restrict_unprivileged_userns")
        try:
            apparmor = restricted.read_text().strip() == "1"
        except OSError:
            apparmor = False
        if apparmor and not Path("/etc/apparmor.d/bwrap-userns-restrict").exists():
            return "warn", ("bubblewrap cannot prepare its namespaces: load the distribution's "
                            "bwrap-userns-restrict AppArmor profile; application checks need it")
        return "warn", ("bubblewrap isolation is unavailable: install bubblewrap and libseccomp2; "
                        "application checks need it, agent teams do not")
    return "fail", f"{sys.platform} is not a supported Forge platform"


def state_check(directory):
    try:
        info = directory.stat()
    except FileNotFoundError:
        return "ok", f"{directory} does not exist yet; Forge creates it at first start"
    except OSError as error:
        return "fail", f"{directory} cannot be checked ({type(error).__name__})"
    if not stat.S_ISDIR(info.st_mode):
        return "fail", f"{directory} is not a directory"
    if info.st_uid != os.getuid():
        return "fail", f"{directory} belongs to another user"
    if info.st_mode & 0o077:
        return "warn", f"{directory} is open to other users; run chmod 700 on it (Forge keeps private files there)"
    return "ok", f"{directory} is private to this user"


def port_free(port):
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        try:
            probe.bind(("127.0.0.1", port))
        except OSError:
            return False
    return True


async def doctor_command(context):
    checks = []

    def add(status, area, message):
        checks.append({"status": status, "area": area, "message": message})

    version = sys.version_info
    implementation = platform.python_implementation()
    python = f"{implementation} {version.major}.{version.minor}.{version.micro} ({sys.executable})"
    if version >= (3, 11) and implementation == "CPython":
        add("ok", "python", python)
    else:
        add("fail", "python", python + ": Forge needs CPython 3.11 or newer")
    status, message = isolation_check()
    add(status, "isolation", message)
    gcloud = shutil.which("gcloud")
    add("ok" if gcloud else "warn", "gcloud",
        f"Google Cloud CLI at {gcloud}" if gcloud else "Google Cloud CLI not found: only Google Cloud destinations need it")
    status, message = state_check(state_directory(context))
    add(status, "state", message)
    if sys.platform == "darwin":
        home = Path.home()
        if any(FORGE_ROOT == home / name or (home / name) in FORGE_ROOT.parents for name in PROTECTED_MAC_FOLDERS):
            add("warn", "folder", f"{FORGE_ROOT} is in a protected macOS folder: background installations "
                "cannot read it; move Forge elsewhere")
    elif Path("/run/systemd/system").is_dir():
        import pwd
        user = pwd.getpwuid(os.getuid()).pw_name
        if Path("/var/lib/systemd/linger", user).exists():
            add("ok", "background", "lingering is enabled: background installations keep running after logout")
        else:
            add("warn", "background", "background installations stop at logout; run loginctl enable-linger "
                "to keep them running")
    try:
        client = await context.client(compatible=False)
    except CliError as error:
        if error.code == "prerequisite_unavailable":
            add("fail", "environment", error.detail + " Prepare it with bash start.sh --prepare-only in the Forge "
                "folder.")
        else:
            add("fail", "port", error.detail)
        client = None
    capabilities = None
    if client is not None:
        port = int(client.base_url.rsplit(":", 1)[1])
        try:
            capabilities = await client.check_compatible(cli_version())
        except CliError as error:
            if error.code == "server_unavailable":
                add("warn", "server", f"no Forge server answers at {client.base_url}; start one with meta-forge serve")
                free = port_free(port)
                add("ok" if free else "warn", "port",
                    f"port {port} is free" if free else f"port {port} is used by a program that is not answering as Forge")
            else:
                add("fail", "server", error.detail)
    if capabilities is not None:
        add("ok", "server", f"Forge {capabilities.get('version')} answers at {client.base_url}")
        try:
            checker = await client.get("/api/checker-setup")
            ready = checker.get("status") == "ready"
            add("ok" if ready else "warn", "checker", f"application checker {checker.get('status')}: "
                f"{checker.get('detail')}")
        except CliError as error:
            add("warn", "checker", f"application checker status unknown: {error.detail}")
    failed = [item for item in checks if item["status"] == "fail"]
    for item in checks:
        context.output.status_line(item["status"], f"\t{item['area']}\t{item['message']}")
    result = {"checks": checks}
    if failed:
        raise CliError("prerequisite_unavailable", f"{len(failed)} local check(s) failed.", result=result,
                       next=[step("meta-forge doctor", "Run the checks again after correcting them.")])
    context.output.status_line("ok", "\tdoctor\tForge can run here")
    context.output.success("doctor", result)
    return 0


async def version_command(context):
    cli = cli_version()
    client = await context.client(compatible=False)
    server = {"url": client.base_url, "capabilities": None, "compatible": False}
    lines = [f"meta-forge {cli} (Forge folder {FORGE_ROOT})"]
    try:
        capabilities = await client.get("/api/capabilities")
        server.update(capabilities=capabilities, compatible=capabilities.get("version") == cli)
        lines.append(f"Forge server at {client.base_url}: {capabilities.get('version')} ("
                     + ("compatible" if server["compatible"] else "another Forge release: use its own meta-forge")
                     + ")")
        team = capabilities.get("agent_team_runtime") or {}
        lines.append(f"Agent team runtime: {team.get('sdk')} {team.get('version')}")
        ordinary = capabilities.get("ordinary_runtime")
        lines.append("Application runtime: " + (f"{ordinary.get('sdk')} {ordinary.get('version')} ({ordinary.get('target')})"
                                                if isinstance(ordinary, dict) else "none selected by this launcher"))
        planner = capabilities.get("planner") or {}
        lines.append("Guide model access: " + (f"available ({planner.get('provider')})" if planner.get("available")
                                               else f"unavailable: {planner.get('reason')}"))
    except CliError as error:
        server["problem"] = {"code": error.code, "detail": error.detail}
        lines.append(f"Forge server at {client.base_url}: {error.detail}")
    result = {"cli": {"version": cli, "folder": str(FORGE_ROOT), "python": platform.python_version()},
              "server": server}
    context.output.success("version", result, lines=lines)
    return 0


async def provider_status(context):
    client = await context.client()
    value = await client.get("/api/provider-settings")
    rows = value.get("providers") or []
    lines = []
    for row in rows:
        state = "configured" if row.get("configured") else "not configured"
        source = f" ({row.get('source')})" if row.get("source") else ""
        lines.append(f"{row.get('provider'):<10} {state}{source}")
    lines.append(f"guide provider: {value.get('effective_provider') or 'none'}"
                 + (f" (saved setup team uses {value['locked_provider']})" if value.get("locked_provider") else ""))
    if value.get("native_provider_setup"):
        lines.append("This Forge selects its model in its own sign-in chooser in the browser.")
    if value.get("load_error"):
        lines.append(f"Saved settings: {value['load_error']}")
    needs = [] if any(row.get("configured") for row in rows) else provider_needs(client.base_url)
    context.output.success("provider status", value, lines=lines, needs=needs,
                           next=[step("meta-forge guide start", "Start Forge's setup team with this provider.")]
                           if not needs else [])
    return 0


def guide_lines(status, base_url):
    lines = [f"Setup team: {status.get('status')}"
             + (f" ({STARTUP_PHASES.get(status['startup_phase'], status['startup_phase'])})"
                if status.get("startup_phase") else "")]
    if status.get("provider"):
        lines.append(f"  Model: {status.get('provider')}/{status.get('model')}")
    if status.get("busy"):
        lines.append("  Busy: answering a guide turn")
    if status.get("startup_error_kind"):
        lines.append(f"  Startup problem: {status['startup_error_kind']}")
    if status.get("detail") and status.get("status") in ("failed", "stopped"):
        lines.append(f"  Detail: {status['detail']}")
    if status.get("browser_console_url"):
        lines.append(f"  Guide console: {base_url}{status['browser_console_url']}")
    return lines


async def guide_status(context):
    client = await context.client()
    status = await client.get("/api/architect/status")
    context.output.success("guide status", status, lines=guide_lines(status, client.base_url))
    return 0


async def guide_start(context):
    client = await context.client()
    before = await client.get("/api/architect/status")
    if before.get("available") is False:
        raise CliError("prerequisite_unavailable", before.get("detail") or "No model provider is configured for Forge.",
                       needs=provider_needs(client.base_url),
                       next=[step("meta-forge provider status", "Check which providers are configured.")])
    context.resume = [step("meta-forge guide status", "Check whether the setup team finished starting.")]
    context.output.note("Starting Forge's setup team...")
    start = asyncio.ensure_future(client.post("/api/architect/start"))
    seen = before.get("startup_phase")
    try:
        while not start.done():
            left = context.remaining()
            if left is not None and left <= 0:
                raise context.timed_out("the setup team started")
            interval = 4 * context.poll_scale
            await asyncio.wait({start}, timeout=interval if left is None else max(min(interval, left), 0))
            if start.done():
                break
            try:
                current = await client.get("/api/architect/status")
            except CliError:
                continue
            phase = current.get("startup_phase")
            if phase and phase != seen:
                seen = phase
                context.output.event("guide start", "phase", phase=phase,
                                     message="setup team: " + STARTUP_PHASES.get(phase, phase))
        _, status = start.result()
    finally:
        if not start.done():
            start.cancel()
            await asyncio.gather(start, return_exceptions=True)
    if status.get("status") != "ready":
        raise CliError("operation_failed", status.get("detail") or "The setup team did not report ready.",
                       result=status, next=[step("meta-forge guide status", "Read the setup team's status."),
                                            step("meta-forge guide start", "Retry the start.")])
    context.output.success("guide start", status, lines=guide_lines(status, client.base_url))
    return 0


async def guide_stop(context):
    client = await context.client()
    _, status = await client.post("/api/architect/stop")
    context.output.success("guide stop", status, lines=guide_lines(status, client.base_url))
    return 0
