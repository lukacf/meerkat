"""Forge CLI live row: the terminal journey installs an agent team on the published Meta release.

A new state folder gets a placeholder provider key for Forge's own agents and
an accepted starter agent team plan, designed offline by Forge's own guide
service with a scripted setup team (no model) and accepted through the
command line. Forge's server then runs from this checkout as start.sh runs it,
on a free loopback port, and everything after that is the command line in its
module form (python -m launchkit.cli, which ./meta-forge runs in a package):

- version sees a compatible server and doctor reports it;
- install --team without --approve stops with exit 84 and prints the summary
  and its digest; install with that digest and --reuse-forge-keys installs the
  published Meta release and starts the team, and the Console reports every
  identity ready;
- installation status and open report the running team and its Console link;
  settings show refuses while it runs (Forge reads private settings of a
  stopped installation only);
- installation stop settles the team as stopped, and settings show then
  reports the provider key as set, never its value.

The provider key is a placeholder and no agent turn is sent, so no provider
request is made. The install downloads the published Meta release, as the
agent team live row does. Every child runs with a private home folder inside
the work folder and without the runner's provider credentials.

Usage: python installer/tests/installer_live_cli_team.py <new empty work folder>
Logs are written into the work folder. Any failed check exits non-zero.
"""
import asyncio
import io
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import time
from types import SimpleNamespace
import urllib.error
import urllib.request

INSTALLER = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(INSTALLER))

PLACEHOLDER_KEY = "forge-ci-placeholder-not-a-provider-key"
HOST = "mac" if sys.platform == "darwin" else "linux"
CREDENTIAL_VARIABLES = {"COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"}


def check(condition, message):
    if not condition:
        raise SystemExit("FAILED: " + message)
    print("ok:", message, flush=True)


def free_port():
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def environment(work):
    """The children's environment: no provider credential of the runner, a private home."""
    values = {key: value for key, value in os.environ.items()
              if not key.endswith("_API_KEY") and key not in CREDENTIAL_VARIABLES}
    for name in ("MOBKIT_RPC_GATEWAY_BIN", "MOBKIT_PORT", "META_FORGE_PORT", "MOBKIT_LAUNCH_PORT",
                 "MOBKIT_LAUNCH_HOME", "PYTHONPATH"):
        values.pop(name, None)
    home = work / "home"
    values.update(HOME=str(home), XDG_CONFIG_HOME=str(home / ".config"), XDG_CACHE_HOME=str(home / ".cache"),
                  XDG_DATA_HOME=str(home / ".local/share"), META_FORGE_HOME=str(work / "state"),
                  PYTHONPATH=str(INSTALLER), NO_COLOR="1")
    return values


def save_placeholder_key(state):
    """Forge's own saved provider key, as Guide settings would save it: a placeholder."""
    path = state / "provider-settings.json"
    path.write_text(json.dumps({"version": 1, "keys": {"openai": PLACEHOLDER_KEY}}))
    path.chmod(0o600)


class ScriptedSetupTeam:
    """Forge's setup team, scripted: it proposes the starter agent team for this computer. No model."""

    async def message(self, prompt, blueprint, *, guide_context):
        source = guide_context["current_user_message"]["id"]
        summaries = {
            "outcome": "Compare two heat-pump proposals and cite their source text.",
            "audience": "One person uses the comparison.", "interfaces": "The Console.",
            "ownership": "One assistant owns the comparison and follow-up questions.",
            "inputs": "Two UTF-8 proposal files placed in the workspace.",
            "workflow": "The user launches each comparison manually.",
            "knowledge": "Identity memory and internal notes; no knowledge service.",
            "authority": "Internal notes may be written; no outside actions.",
            "destination": "This computer.",
        }
        blueprint["project"]["name"] = "Heat-pump comparison"
        blueprint["target"] = HOST
        brief = {"facts": {topic: {"summary": summary, "status": "confirmed", "source_message_ids": [source]}
                           for topic, summary in summaries.items()}, "requirements": []}
        return {"guide": {"question": None, "message": "Your comparison team is ready to review.", "brief": brief,
                          "design_changed": True, "summary": "One comparison team.", "warnings": []},
                "proposal": blueprint}


async def run_cli(argv, transport, environ):
    from launchkit.cli import run_async
    stdout, stderr = io.StringIO(), io.StringIO()
    code = await run_async(argv, stdin=io.StringIO(), stdout=stdout, stderr=stderr, environ=environ,
                           transport=transport, poll_scale=0.2)
    return code, stdout.getvalue(), stderr.getvalue()


async def accepted_plan(work):
    """Design and accept the plan offline: Forge's guide service, the scripted setup team, the command line."""
    import httpx
    from launchkit import guide as guide_module, server
    from launchkit.guide import GuideService
    from launchkit.guide_operations import GuideOperations
    from launchkit.guide_store import GuideStore
    from launchkit.implementation import NoApplicationBuild

    guide_module.planner_capability = lambda: {"available": True, "provider": "openai", "reason": "Ready"}
    guide = GuideService(GuideStore(work / "state"), ScriptedSetupTeam())

    def no_application(_state):
        raise NoApplicationBuild("No application build")

    guide.implementation_evidence_provider = no_application
    server.guide, server.guide_operations = guide, GuideOperations()
    server.architect = SimpleNamespace(guide_context=None, proposal=None, guide_result=None,
                                       message_lock=asyncio.Lock())
    server.cloud_capability = lambda: {"available": False, "project": None, "reason": "Not used offline."}
    transport = httpx.ASGITransport(app=server.app)
    environ = {"META_FORGE_PORT": "8787", "PATH": os.environ.get("PATH", "/usr/bin:/bin")}
    code, out, err = await run_cli(["setup", "new", "--json"], transport, environ)
    check(code == 0, f"setup new created a setup ({err.strip()})")
    identity = json.loads(out)["setup"]
    code, out, err = await run_cli(["setup", "send", identity, "--text", "Compare two heat-pump proposals",
                                    "--wait", "--json"], transport, environ)
    check(code == 0 and json.loads(out)["result"]["proposal"], f"the scripted guide proposed a team ({err.strip()})")
    code, out, _ = await run_cli(["plan", "accept", identity, "--json"], transport, environ)
    summary = json.loads(out)
    check(code == 84 and summary["error"]["code"] == "approval_required", "plan accept waits for an approval")
    code, out, err = await run_cli(["plan", "accept", identity, "--approve", summary["digest"], "--json"],
                                   transport, environ)
    check(code == 0 and json.loads(out)["result"]["status"] == "ready", f"plan accept accepted the plan ({err.strip()})")
    return identity


def serve(work, port):
    """Forge's server from this checkout, as start.sh's last step runs it."""
    log = (work / "forge-server.log").open("wb")
    process = subprocess.Popen([sys.executable, "-B", "-m", "uvicorn", "launchkit.server:app", "--host", "127.0.0.1",
                                "--port", str(port), "--timeout-graceful-shutdown", "20"],
                               cwd=INSTALLER, env=environment(work), stdin=subprocess.DEVNULL, stdout=log,
                               stderr=subprocess.STDOUT, start_new_session=True)
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline and process.poll() is None:
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{port}/api/capabilities", timeout=5) as response:
                if response.status == 200:
                    return process
        except (urllib.error.URLError, OSError):
            pass
        time.sleep(1)
    check(False, "Forge's server answered (see forge-server.log)")


def stop_server(process):
    if process.poll() is None:
        os.killpg(process.pid, signal.SIGINT)
        try:
            process.wait(timeout=120)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()


def meta_forge(work, server_url, name, *arguments, timeout=300):
    """One command-line run as a child process; its output is logged and never holds the placeholder key."""
    completed = subprocess.run([sys.executable, "-B", "-m", "launchkit.cli", "--server", server_url, *arguments],
                               cwd=work, env=environment(work), stdin=subprocess.DEVNULL, capture_output=True,
                               text=True, timeout=timeout)
    (work / f"cli-{name}.log").write_text(f"$ meta-forge {' '.join(arguments)}\nexit {completed.returncode}\n"
                                          f"--- stdout\n{completed.stdout}--- stderr\n{completed.stderr}")
    check(PLACEHOLDER_KEY not in completed.stdout + completed.stderr, f"{name}: no key value in the output")
    return completed


def identities(port):
    try:
        with urllib.request.urlopen(f"http://127.0.0.1:{port}/console/identities", timeout=10) as response:
            return response.status, json.loads(response.read()).get("identities", [])
    except (urllib.error.URLError, OSError, ValueError):
        return None, []


def main():
    work = Path(sys.argv[1]).resolve()
    work.mkdir(parents=True, exist_ok=False)
    os.umask(0o022)
    (work / "state").mkdir(mode=0o700)
    (work / "home").mkdir(mode=0o700)
    save_placeholder_key(work / "state")
    # The offline design step imports Forge's server module, which reads its state folder and
    # probes the host on import: this process takes the children's private environment first.
    private = environment(work)
    for name in [name for name in os.environ if name not in private]:
        del os.environ[name]
    os.environ.update(private)
    identity = asyncio.run(accepted_plan(work))

    port = free_port()
    server_url = f"http://127.0.0.1:{port}"
    process = serve(work, port)
    try:
        version = meta_forge(work, server_url, "version", "version", "--json")
        check(version.returncode == 0 and json.loads(version.stdout)["result"]["server"]["compatible"],
              "version: the running Forge is this release")
        doctor = meta_forge(work, server_url, "doctor", "doctor")
        check("ok\tserver" in doctor.stdout, "doctor: the server answers")

        folder, console = work / "team", free_port()
        install = ["install", identity, "--team", "--folder", str(folder), "--port", str(console),
                   "--reuse-forge-keys", "--json"]
        summary = meta_forge(work, server_url, "install-summary", *install)
        document = json.loads(summary.stdout)
        check(summary.returncode == 84 and document["error"]["code"] == "approval_required"
              and document["digest"].startswith("sha256:"), "install: exit 84 with the summary and its digest")
        installed = meta_forge(work, server_url, "install", *install, "--approve", document["digest"], "--wait",
                               "--timeout", "25m", timeout=1800)
        check(installed.returncode == 0, "install: installed and started (see cli-install.log)")
        receipt = json.loads(installed.stdout)["result"]
        check(receipt["status"] == "running" and receipt["destination"] == str(folder),
              "install: the installation runs from the chosen folder")
        check(PLACEHOLDER_KEY in (folder / ".env").read_text(),
              "install: Forge itself copied its saved key into the installation's private settings")

        expected = len(json.loads((folder / "deployment.json").read_text())["agents"])
        deadline, ready = time.monotonic() + 300, []
        while time.monotonic() < deadline:
            code, rows = identities(console)
            ready = [row for row in rows if row.get("health") == "ready"]
            if code == 200 and len(rows) == expected and len(ready) == expected:
                break
            time.sleep(3)
        check(len(ready) == expected, f"the Console reports {expected} ready identities on the real gateway")

        status = meta_forge(work, server_url, "status", "installation", "status", identity, "--json")
        check(status.returncode == 0 and json.loads(status.stdout)["result"]["status"] == "running",
              "installation status: running")
        opened = meta_forge(work, server_url, "open", "installation", "open", identity)
        check(opened.returncode == 0 and f"127.0.0.1:{console}" in opened.stdout, "installation open: the Console link")
        running = meta_forge(work, server_url, "settings-running", "installation", "settings", "show", identity,
                             "--json")
        check(running.returncode == 82 and "stopped installation only" in json.loads(running.stdout)["error"]["detail"],
              "installation settings show: Forge shows a running installation's settings only once it is stopped")

        stopped = meta_forge(work, server_url, "stop", "installation", "stop", identity, "--wait", "--timeout", "5m",
                             "--json", timeout=600)
        check(stopped.returncode == 0 and json.loads(stopped.stdout)["result"]["status"] == "stopped",
              "installation stop: stopped")
        code, _ = identities(console)
        check(code is None, "the Console no longer answers")
        settings = meta_forge(work, server_url, "settings", "installation", "settings", "show", identity, "--json")
        check(settings.returncode == 0, "installation settings show: read (see cli-settings.log)")
        fields = {field["key"]: field for field in json.loads(settings.stdout)["result"]["fields"]}
        check(fields["OPENAI_API_KEY"]["configured"] is True, "installation settings show: the provider key is set")
    finally:
        stop_server(process)
    print(f"PASS: meta-forge installed, ran and stopped an agent team on the published Meta release ({work}).",
          flush=True)


if __name__ == "__main__":
    main()
