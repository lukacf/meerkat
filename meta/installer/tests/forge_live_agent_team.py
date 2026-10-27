"""Forge CI live row: a compiled agent team on the published Meta release.

Exports the starter agent team for the public Meta runtime and installs it with
its own install.sh, which downloads the official meta-rpc-gateway and
meta-gateway archives and checks them against the release checksums. It then
runs the check verbs, starts the team until its Console reports every identity
ready, and stops it with SIGINT, as a terminal Control-C would.

A second package adds ordinary application code (application/__init__.py: one
scoped host callback for the lead and one schedule runnable) and runs on the
same verified gateways, which install.sh reuses without a new download.

The provider key is a placeholder and no agent turn is sent, so no provider
request is made.

Usage: python installer/tests/forge_live_agent_team.py <new empty work folder>
Logs are written into the work folder. Any failed check exits non-zero.
"""
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time
import urllib.error
import urllib.request

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from launchkit import public_meta_runtime  # noqa: E402
from launchkit.blueprint import starter_blueprint  # noqa: E402
from launchkit.compiler import generate_package  # noqa: E402

VERSION = public_meta_runtime.VERSION
TARGET = "x86_64-unknown-linux-gnu"
PLACEHOLDER_KEY = "forge-ci-placeholder-not-a-provider-key"
PROVIDER_KEYS = ("OPENAI_API_KEY", "ANTHROPIC_API_KEY", "GEMINI_API_KEY")


def check(condition, message):
    if not condition:
        raise SystemExit("FAILED: " + message)
    print("ok:", message, flush=True)


def environment():
    # No provider credential from the runner reaches the team; its .env holds a placeholder.
    values = {key: value for key, value in os.environ.items() if key not in PROVIDER_KEYS}
    values.pop("MOBKIT_RPC_GATEWAY_BIN", None)
    values.pop("MOBKIT_PORT", None)
    return values


def run(team, work, name, arguments, timeout):
    log = work / f"{name}.log"
    with log.open("wb") as output:
        completed = subprocess.run(arguments, cwd=team, env=environment(), stdin=subprocess.DEVNULL,
            stdout=output, stderr=subprocess.STDOUT, timeout=timeout)
    text = log.read_text(errors="replace")
    check(completed.returncode == 0, f"{' '.join(arguments[1:]) or arguments[0]} exited 0 (see {log.name})")
    return text


def export(team):
    plan = starter_blueprint().model_dump(mode="json")
    plan["target"] = "linux"
    team.mkdir(parents=True)
    for item in generate_package(plan, public_runtime=True):
        path = team / item.path
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(item.as_bytes())
        os.chmod(path, item.mode)
    definition = json.loads((team / "deployment.json").read_text())
    check(definition["mobkitVersion"] == VERSION, f"the exported team pins Meta {VERSION}")
    return definition


def use_placeholder_key(team):
    path = team / ".env"
    lines = path.read_text().splitlines() if path.exists() else []
    lines = [line for line in lines if not line.startswith("OPENAI_API_KEY=")]
    path.write_text("\n".join([*lines, f"OPENAI_API_KEY={PLACEHOLDER_KEY}"]) + "\n")
    path.chmod(0o600)


APPLICATION = '''"""Forge CI live row: one scoped host callback and one schedule runnable."""


class Integration:
    def __init__(self, context):
        self.context = context

    def tools_for_agent(self, agent):
        if agent.get("classId") != "lead":
            return []
        return [{"name": "forge_ci_ping", "description": "Return a fixed acknowledgement for the Forge CI live row.",
                 "schema": {"type": "object", "properties": {}, "additionalProperties": False},
                 "handler": self.ping}]

    def ping(self, arguments):
        return {"ok": True}

    def schedule_runnables(self):
        return [{"name": "forge_ci_tick", "handler": self.tick}]

    def tick(self, occurrence):
        return None


def build(context):
    return Integration(context)
'''


def identities(port):
    try:
        with urllib.request.urlopen(f"http://127.0.0.1:{port}/console/identities", timeout=10) as response:
            return response.status, json.loads(response.read()).get("identities", [])
    except (urllib.error.URLError, OSError, ValueError):
        return None, []


def start_until_ready_then_interrupt(team, work, name, definition):
    port, expected = definition["port"], len(definition["agents"])
    log = work / f"{name}.log"
    with log.open("wb") as output:
        process = subprocess.Popen(["bash", "start.sh"], cwd=team, env=environment(), stdin=subprocess.DEVNULL,
            stdout=output, stderr=subprocess.STDOUT, start_new_session=True)
    try:
        deadline, ready = time.monotonic() + 300, []
        while time.monotonic() < deadline and process.poll() is None:
            code, rows = identities(port)
            ready = [row for row in rows if row.get("health") == "ready"]
            if code == 200 and len(rows) == expected and len(ready) == expected:
                break
            time.sleep(3)
        check(process.poll() is None and len(ready) == expected,
            f"{name}: the Console reports {expected} ready identities on the real gateway")
        os.killpg(process.pid, signal.SIGINT)
        process.wait(timeout=180)
    finally:
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
    # A first start can run and settle a preparatory gateway before the serving one.
    settled = [shutdown_fields(line) for line in log.read_text(errors="replace").splitlines()
        if "rpc_gateway shutdown complete; exiting" in line]
    check(settled and interrupt_stopped(settled[-1]) and all(
        fields.get("cleanup_complete") == "true" and fields.get("runtime_cleanup_complete") == "true"
        for fields in settled),
        f"{name}: SIGINT stops the gateway with complete cleanup (signal or handshake path)")


def shutdown_fields(line):
    """The key=value fields of one gateway shutdown line."""
    return dict(re.findall(r"(\w+)=(\S+)", line))


def interrupt_stopped(fields):
    """SIGINT reaches the start's whole process group, and the launcher's SDK
    shutdown handshake can reach the gateway before the signal does, so either
    is the interrupt's stop. Any other reason is not."""
    if fields.get("reason") == "signal":
        return fields.get("signal") == "SIGINT"
    return fields.get("reason") == "sdk_shutdown_handshake" and "signal" not in fields


def application_leg(work, team):
    """Ordinary application code on the same verified gateways, with no new download."""
    application_team = work / "application-team"
    definition = export(application_team)
    (application_team / "application").mkdir(exist_ok=True)
    (application_team / "application/__init__.py").write_text(APPLICATION)
    binaries = application_team / ".bin"
    binaries.mkdir(mode=0o700)
    for name in ("rpc_gateway", "meta_gateway"):
        for suffix, mode in (("", 0o700), (".receipt.json", 0o600)):
            target = binaries / (name + suffix)
            target.write_bytes((team / ".bin" / (name + suffix)).read_bytes())
            target.chmod(mode)
    application_team.chmod(0o755)
    installed = run(application_team, work, "application-install", ["bash", "install.sh", "--non-interactive"], 1200)
    check("Downloading Meta" not in installed and all(
        f"Reusing verified official Meta {VERSION} .bin/{name}" in installed for name in ("rpc_gateway", "meta_gateway")),
        "the application package reuses the verified gateways without a download")
    use_placeholder_key(application_team)
    contract = run(application_team, work, "application-contract",
        ["bash", "start.sh", "--application-check"], 300)
    check("1 scoped callbacks, 1 schedule runnables" in contract,
        "--application-check loads 1 scoped callback and 1 schedule runnable")
    start_until_ready_then_interrupt(application_team, work, "application-start", definition)


def main():
    work = Path(sys.argv[1]).resolve()
    work.mkdir(parents=True, exist_ok=False)
    os.umask(0o022)
    team = work / "team"
    definition = export(team)
    team.chmod(0o755)

    installed = run(team, work, "install", ["bash", "install.sh", "--non-interactive"], 1200)
    check(f"Successfully installed meerkat-meta-{VERSION}" in installed, f"install.sh installed meerkat-meta {VERSION}")
    for archive in ("meta-rpc-gateway", "meta-gateway"):
        check(f"Verified SHA-256 for {archive}-{VERSION}-{TARGET}.tar.gz." in installed,
            f"install.sh verified {archive} against the release checksums")
    probe = subprocess.run([str(team / ".venv/bin/python"), "-c",
        "import importlib.metadata as m, importlib.util as u; print(m.version('meerkat-meta'), u.find_spec('meerkat_mobkit') is None)"],
        capture_output=True, text=True, timeout=60)
    check(probe.stdout.split() == [VERSION, "True"], "the package environment has meerkat-meta and no MobKit")
    for name in ("rpc_gateway", "meta_gateway"):
        receipt = json.loads((team / ".bin" / f"{name}.receipt.json").read_text())
        check(receipt["version"] == VERSION and receipt["release_url"] == public_meta_runtime.RELEASE_URL,
            f".bin/{name} carries the official release receipt")

    use_placeholder_key(team)
    doctor = run(team, work, "doctor", ["bash", "start.sh", "--doctor"], 300)
    status = [line for line in doctor.splitlines() if line.startswith("[")]
    check(status and all(line.startswith("[ready] ") for line in status), "--doctor reports every check ready")
    run(team, work, "application-check", ["bash", "start.sh", "--application-check"], 300)
    run(team, work, "connections-check", ["bash", "start.sh", "--connections-check"], 300)
    flows = run(team, work, "list-flows", ["bash", "start.sh", "--list-flows"], 600)
    check('"briefing"' in flows, "--list-flows lists the native flow on the real gateway")
    revision = run(team, work, "definition-revision", ["bash", "start.sh", "--definition-revision"], 600)
    check('"definition_revision": 1' in revision, "--definition-revision reads revision 1 from the native ledger")

    start_until_ready_then_interrupt(team, work, "start", definition)
    application_leg(work, team)
    print(f"PASS: the agent team and its application package ran on the published Meta {VERSION} release.",
        flush=True)


if __name__ == "__main__":
    main()
