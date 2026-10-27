"""Compile a validated deployment specification into a complete, secret-free package."""
from __future__ import annotations

from dataclasses import asdict, dataclass
import hashlib
import io
import json
from pathlib import Path
import shlex
import zipfile

from .schema import DeploymentSpec
from .native_models import model_registry_lines
from .deployment_deadlines import render_destination_shutdown_budget

MOBKIT_VERSION = "0.8.47"
TEMPLATES = Path(__file__).parent / "templates"
PROVIDERS = {
    "openai": {"id": "openai", "name": "OpenAI", "envKey": "OPENAI_API_KEY"},
    "anthropic": {"id": "anthropic", "name": "Anthropic", "envKey": "ANTHROPIC_API_KEY"},
    "gemini": {"id": "gemini", "name": "Google Gemini", "envKey": "GEMINI_API_KEY"},
}
CONNECTORS = {
    "workspace": {
        "id": "files", "name": "Workspace files", "requiredEnv": [],
        "environment": [{"key": "FILES_ROOT", "label": "Folder to connect (leave blank for ./workspace)", "secret": False}],
    },
    "github": {
        "id": "github", "name": "GitHub", "requiredEnv": ["GITHUB_TOKEN", "GITHUB_REPOSITORY"],
        "environment": [
            {"key": "GITHUB_TOKEN", "label": "GitHub read-only token", "secret": True},
            {"key": "GITHUB_REPOSITORY", "label": "GitHub repository (owner/name)", "secret": False},
        ],
    },
    "telegram": {
        "id": "telegram", "name": "Telegram", "requiredEnv": ["TELEGRAM_BOT_TOKEN", "TELEGRAM_ALLOWED_CHAT_IDS"],
        "environment": [
            {"key": "TELEGRAM_BOT_TOKEN", "label": "Telegram bot token from BotFather", "secret": True},
            {"key": "TELEGRAM_ALLOWED_CHAT_IDS", "label": "Allowed numeric Telegram chat IDs (comma separated)", "secret": False},
        ],
    },
}


@dataclass(frozen=True)
class PackageFile:
    path: str
    content: str | bytes
    mode: int = 0o644

    def as_bytes(self):
        return self.content if isinstance(self.content, bytes) else self.content.encode("utf-8")

    def to_dict(self):
        if isinstance(self.content, bytes):
            # Model/UI projections describe host-adopted assets without sending
            # their binary payload, an encoding of it, or its origin path.
            return {"path": self.path, "mode": self.mode, "binary": True,
                    "size": len(self.content), "sha256": hashlib.sha256(self.content).hexdigest()}
        return asdict(self)


def _spec(value: DeploymentSpec | dict) -> DeploymentSpec:
    return value if isinstance(value, DeploymentSpec) else DeploymentSpec.model_validate(value)


def _json(value):
    # JSON basic strings are valid TOML basic strings for our validated inputs.
    # Escape surrogate/newline/control content instead of interpolating it as code.
    return json.dumps(value, ensure_ascii=False)


def _runtime_spec(spec):
    connectors = [CONNECTORS[key] for key in spec.connections]
    connector_ids = [connector["id"] for connector in connectors]
    agents = [{
        "id": agent.id,
        "identity": "agent:" + agent.id,
        "name": agent.name,
        "role": agent.role,
        "instructions": agent.instructions,
        "connectors": connector_ids,
    } for agent in spec.agents]
    # A visible lead coordinates every specialist. Peers remain distinct identities.
    edges = [[agents[0]["identity"], agent["identity"]] for agent in agents[1:]]
    return {
        "schemaVersion": 1, "mobkitVersion": MOBKIT_VERSION,
        "name": spec.project.name, "slug": spec.project.slug,
        "description": spec.project.description,
        "provider": {**PROVIDERS[spec.provider], "model": spec.model},
        "port": spec.advanced.port, "target": spec.target,
        "agents": agents, "connections": edges, "connectors": connectors,
        "storage": {"type": "sqlite", "runtime": ".mobkit/state", "notes": "data/notes.sqlite3"},
    }


def _mob_toml(spec):
    lines = ["[mob]", f"id = {_json(spec.project.slug)}", ""]
    lines += model_registry_lines([(spec.provider, spec.model)], native_version=MOBKIT_VERSION)
    for agent in spec.agents:
        prompt = (
            f"You are {agent.name}, the {agent.role} for {spec.project.name}.\n"
            f"The team's purpose: {spec.project.description}\n\n{agent.instructions}\n\n"
            "Use your connected peers when their specialty is needed. Respond in clear, helpful language. "
            "When external data is missing, ask the user to connect it. Never invent tool results."
        )
        lines += [
            f"[skills.{agent.id}_role]", 'source = "inline"', f"content = {_json(prompt)}", "",
            f"[profiles.{agent.id}]", f"model = {_json(spec.model)}", f"provider = {_json(spec.provider)}",
            "external_addressable = true", 'runtime_mode = "turn_driven"',
            f"skills = [{_json(agent.id + '_role')}]", f"peer_description = {_json(agent.role)}", "",
            f"[profiles.{agent.id}.tools]", "comms = true", "memory = true", "builtins = false", "",
        ]
    return "\n".join(lines)


def _console_toml(spec):
    return "\n".join([
        f"title = {_json(spec.project.name)}", "", "[brand]", f"label = {_json(spec.project.name)}", "",
        "[appearance]", 'default_theme = "dark"', 'default_variant = "graphite"', "",
        "[environment]", f"label = {_json(spec.target)}", "",
        "[layout]", 'initial_preset = "two_columns"', 'initial_control = "roster"',
        f"initial_agent = {_json('agent:' + spec.agents[0].id)}", "",
        "[agent_list]", 'group_by = ["labels.console_group"]',
        f"default_pinned_agent_ids = [{_json('agent:' + spec.agents[0].id)}]", "",
        "[actions]", "show_reset = false", "",
    ])


def _readme(spec, runtime):
    provider = runtime["provider"]
    agent_rows = "\n".join(f"| {agent.name.replace('|', '/')} | {agent.role.replace('|', '/')} |" for agent in spec.agents)
    connector_notes = []
    if "workspace" in spec.connections:
        connector_notes.append("- **Workspace files:** place text files in `workspace/` or set `FILES_ROOT` locally. Access is read-only, restricted to that folder, and hidden files are excluded.")
    if "github" in spec.connections:
        connector_notes.append("- **GitHub:** create a [fine-grained token](https://docs.github.com/en/rest/issues/issues#list-repository-issues) scoped to the selected repository with read-only Issues permission. Set `GITHUB_TOKEN` and `GITHUB_REPOSITORY=owner/name`. This starter reads issues; it does not push code or post comments.")
    if "telegram" in spec.connections:
        connector_notes.append("- **Telegram:** create a dedicated bot with BotFather, set `TELEGRAM_BOT_TOKEN`, and set `TELEGRAM_ALLOWED_CHAT_IDS` to the numeric IDs of chats you trust (comma-separated, no wildcard). During local setup, send the bot a message and accept the optional chat-ID lookup; choose only IDs you recognize. The app long-polls new text messages, forwards admitted user text to your coordinator, and sends the reply only to that same admitted chat. Bot messages, other update types, unlisted chats, and messages older than the first bridge startup are ignored. Existing webhooks or a competing bot poller cause a clear startup error; they are never deleted automatically. See the official [`getUpdates` contract](https://core.telegram.org/bots/api#getupdates). The cursor is persisted before delivery, so a crash can lose a reply but cannot redeliver work. All allowed chats share this coordinator's conversation; only allow chats that may share context. Run a single copy of the bot bridge.")
    return f"""# {spec.project.name}

{spec.project.description}

A complete, editable Meerkat/MobKit application built for you by Meta Forge.

## Start on your computer

1. Extract this package into a new folder, such as `~/src/{spec.project.slug}`.
2. Open Terminal in that folder and run `bash install.sh`.
3. The local setup asks for your {provider['name']} API key and selected connections.
4. Run `bash start.sh`, then open [your console](http://127.0.0.1:{spec.advanced.port}/console).

Python 3.10 or newer is required. On Debian/Ubuntu, install `python3-venv` if virtual environment creation is unavailable. The installer downloads MobKit {MOBKIT_VERSION} for your platform and verifies the archive against its published SHA-256 checksum. Downloads and package installation need internet access. AI requests are billed by your provider.

`bash start.sh --doctor` checks installed dependencies and credential presence. `bash start.sh --smoke` starts the real runtime, sends one short paid request, prints the reply, and shuts down. A doctor check alone does not verify provider authentication or connector permissions.

The first start initializes durable storage through MobKit's own offline migration tool before opening the console. This is automatic for a new project and does not make AI requests. Existing imported state is only inspected; if it needs migration, the app provides the explicit backup and migration instructions. Keep the complete `.mobkit/` directory when moving a project.

Your agents wait for your messages. They do not begin tasks automatically at startup.

## Your team

| Agent | Role |
| --- | --- |
{agent_rows}

The first agent coordinates the specialists. Every agent has a durable identity and appears in the console. Peer connections are implemented through MobKit's identity-first topology provider.

## Your connections

{chr(10).join(connector_notes) or 'No external connections selected. Add text context directly in your console.'}

Every agent also has `notes_save` and `notes_search` backed by `data/notes.sqlite3`, plus native identity-scoped memory. Connection credentials are entered after download, stored in a mode-600 `.env`, and never sent to the deployment studio. Workspace and GitHub tools are shared with every agent. Telegram messages enter through the coordinator.

## Keep it running

On macOS, keep the terminal running. Press Control-C to stop cleanly. No background service is installed automatically.

This legacy schemaVersion 1 package lacks current native startup reporting. Verified Linux background service setup requires rebuilding in the current Forge wizard. This package can still run in a terminal with `bash start.sh`; stop with Control-C. It never claims verified background readiness.

## Google Cloud

The selected target is `{spec.target}`. {'Your Google Cloud scripts are configured below.' if spec.target == 'gcp' else 'Cloud deployment scripts are included for reference; choose Google Cloud in the studio and rebuild to supply a project ID.'}

1. Install the Google Cloud CLI, sign in with `gcloud auth login`, and choose a billing-enabled project with the Compute Engine API available. Your account needs VM/network creation permission, SSH access, and [IAP tunnel access](https://docs.cloud.google.com/iap/docs/using-tcp-forwarding).
2. Review `bash deploy/deploy-gcp.sh --dry-run`.
3. Run `bash deploy/deploy-gcp.sh`. It asks for the exact project ID before creating billable resources.
4. For a current schemaVersion 2 package, run `bash deploy/deploy-gcp.sh --setup` to enter credentials directly in the VM terminal and verify service startup. Rebuild this legacy package before using verified background operations.
5. Run `bash deploy/tunnel-gcp.sh` locally and open the same console URL above.

The script creates a Debian 12 VM, a 30 GB persistent boot disk, and an IAP-only SSH firewall rule targeted to this VM. A public IP is used for outbound package/provider access. Existing project firewall rules, including a default SSH rule, still apply; this script does not alter them. The console binds only to loopback and no console firewall rule is added. The VM has no attached service account. The default VPC must exist and organization policies must permit this setup. The VM keeps incurring charges until deleted.

If provisioning stops after VM creation, inspect that VM in your Cloud console and resume setup through IAP SSH; the script will refuse to overwrite an existing VM. Cleanup commands are printed after provisioning. Review any pre-existing network firewall policy in your project.

## Your files and data

- `app.py`: identity roster, topology and runtime lifecycle.
- `config/mob.toml`: models, role prompts, communication and tool defaults.
- `config/console.toml`: title, theme and initial console view.
- `connectors.py`: bounded file reads, selected connector tools and SQLite notes.
- `telegram_bridge.py`: the optional allowlisted Telegram chat bridge.
- `deployment.json`: generated machine-readable configuration.
- `.env`: private credentials, excluded from archives created by the GCP script.
- `.mobkit/state/`: runtime sessions, native memory, event history and blobs.
- `data/`: application notes database.
- `workspace/`: your local reference documents.

Stop the application before backing up. Back up the whole `.mobkit/state/` directory including `blobs/`, together with `data/`, `workspace/`, configuration, and private credentials stored securely. Restore into a stopped deployment. Do not commit `.env`, data, or runtime state.

The persistent MobKit store owns the canonical mob definition. Changing models or prompts after the first boot requires an explicit revision-pinned definition update; simply editing `config/mob.toml` is not an activation mechanism. See [MobKit documentation](https://docs.rkat.ai/mobkit/introduction) when extending an existing team.

The console is private to this computer. To expose it publicly, add a separately configured authenticated proxy and review the [deployment guide](https://docs.rkat.ai/mobkit/guides/deployment).
"""


def generate_package(value: DeploymentSpec | dict) -> list[PackageFile]:
    spec = _spec(value)
    runtime = _runtime_spec(spec)
    files = []
    for name in ("app.py", "connectors.py", "setup.py", "install_gateway.py", "telegram_bridge.py", "storage_setup.py", "install.sh", "start.sh"):
        files.append(PackageFile(name, (TEMPLATES / name).read_text(), 0o755 if name.endswith(".sh") else 0o644))
    files.append(PackageFile("remote_operations.py", render_destination_shutdown_budget(
        (TEMPLATES.parent / "templates_v2/remote_operations.py").read_text())))
    files += [
        PackageFile("deployment.json", json.dumps(runtime, ensure_ascii=True, indent=2) + "\n"),
        PackageFile("launch-spec.json", spec.model_dump_json(indent=2) + "\n"),
        PackageFile("requirements.txt", f"meerkat-mobkit=={MOBKIT_VERSION}\n"),
        PackageFile("config/mob.toml", _mob_toml(spec)),
        PackageFile("config/console.toml", _console_toml(spec)),
        PackageFile("README.md", _readme(spec, runtime)),
        PackageFile(".gitignore", ".env\n.venv/\n.bin/\n.mobkit/\ndata/\nworkspace/\n__pycache__/\n*.pyc\n"),
        PackageFile("workspace/START-HERE.txt", "Add UTF-8 text documents here for your agents to read. Only this connected folder is accessible to the file tools.\n"),
    ]
    env_lines = ["# Private values are entered locally. Do not commit .env.", runtime["provider"]["envKey"] + "=", f"MOBKIT_PORT={spec.advanced.port}"]
    for connector in runtime["connectors"]:
        for field in connector["environment"]:
            env_lines += ["# " + field["label"], field["key"] + "="]
    files.append(PackageFile(".env.example", "\n".join(env_lines) + "\n", 0o600))
    replacements = {
        "__PROJECT__": shlex.quote(spec.gcp.project), "__ZONE__": shlex.quote(spec.gcp.zone),
        "__INSTANCE__": shlex.quote(spec.project.slug), "__MACHINE__": shlex.quote(spec.gcp.machine_type),
        "__PORT__": str(spec.advanced.port),
    }
    deploy = (TEMPLATES / "deploy-gcp.sh").read_text()
    for key, replacement in replacements.items():
        deploy = deploy.replace(key, replacement)
    files += [
        PackageFile("deploy/deploy-gcp.sh", deploy, 0o755),
        PackageFile("deploy/install-service.sh", (TEMPLATES / "install-service.sh").read_text(), 0o755),
        PackageFile("deploy/tunnel-gcp.sh", "\n".join([
            "#!/usr/bin/env bash", "set -euo pipefail",
            f"exec gcloud compute ssh {shlex.quote(spec.project.slug)} --project {shlex.quote(spec.gcp.project)} --zone {shlex.quote(spec.gcp.zone)} --tunnel-through-iap -- -N -L {spec.advanced.port}:127.0.0.1:{spec.advanced.port}", "",
        ]), 0o755),
    ]
    checksums = "".join(f"{hashlib.sha256(file.content.encode()).hexdigest()}  {file.path}\n" for file in sorted(files, key=lambda file: file.path))
    files.append(PackageFile("SHA256SUMS", checksums))
    return files


def package_preview(value: DeploymentSpec | dict) -> dict:
    spec = _spec(value)
    files = generate_package(spec)
    return {"name": spec.project.slug + ".zip", "files": [file.to_dict() for file in files], "file_count": len(files), "mobkit_version": MOBKIT_VERSION}


def build_archive(value: DeploymentSpec | dict) -> bytes:
    spec = _spec(value)
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w", zipfile.ZIP_DEFLATED) as archive:
        for file in generate_package(spec):
            info = zipfile.ZipInfo(f"{spec.project.slug}/{file.path}")
            info.create_system = 3
            info.external_attr = (0o100000 | file.mode) << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(info, file.content.encode())
    return buffer.getvalue()


def generate_files(value: DeploymentSpec | dict) -> dict[str, str]:
    """Compiler API consumed by the studio preview and archive routes."""
    return {file.path: file.content for file in generate_package(value)}
