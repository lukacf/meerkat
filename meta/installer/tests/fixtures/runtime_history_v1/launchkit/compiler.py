"""Compile blueprints into inspectable, runnable native MobKit deployments."""
from __future__ import annotations

import hashlib
import io
import json
from pathlib import Path
import re
import shlex
import zipfile

from .blueprint import Blueprint, PLACEHOLDER
from .generator import MOBKIT_VERSION, PROVIDERS, PackageFile, TEMPLATES
from .design_knowledge import COMMS_PROTOCOL
from .native_models import model_registry_lines
from .deployment_deadlines import render_destination_shutdown_budget

TEMPLATES_V2 = Path(__file__).parent / "templates_v2"
MCP_VERSION = "1.30.0"


def _spec(value):
    return value if isinstance(value, Blueprint) else Blueprint.model_validate(value)


def _json(value):
    return json.dumps(value, ensure_ascii=True)


def profile_id(item, index):
    # A per-instance native profile guarantees an identity-specific flow target.
    if item.members:
        return f"{item.id}__{item.members[index].key}"
    return item.id if item.instances == 1 else f"{item.id}__{index + 1}"


def application_profiles(spec):
    """Reusable class roles, separate from fixed instance/flow targets."""
    occupied = {profile_id(item, index) for item in spec.agent_classes
                for index in range(item.instances)}
    result = {}
    for item in spec.agent_classes:
        base = f"application__{item.id}"
        name, suffix = base, 1
        while name in occupied:
            suffix += 1
            name = f"{base}__{suffix}"
        occupied.add(name)
        result[item.id] = name
    return result


def resolved_blueprint(value):
    spec = _spec(value)
    return {"identities": [{"identity": identity, "class_id": item.id,
            "profile": profile_id(item, index), "name": item.members[index].name if item.members else item.name if item.instances == 1 else f"{item.name} {index + 1}"}
            for item in spec.agent_classes for index, identity in enumerate(item.identities())],
            "edges": [edge.model_dump() for edge in spec.resolved_edges()]}


def environment_fields(spec):
    fields = {}
    def add(key, label, secret=True, required=True):
        if key not in fields:
            fields[key] = {"key": key, "label": label, "secret": secret, "required": required}
        else:
            if required:
                fields[key]["required"] = True
            if secret:
                fields[key]["secret"] = True
                fields[key].pop("default", None)
    for item in spec.agent_classes:
        provider = PROVIDERS[item.provider]
        add(provider["envKey"], f"{provider['name']} API key")
    if spec.knowledge.mode == "local":
        # The bundled pipeline uses the proposal's extraction provider, which
        # can differ from every agent class, plus OpenAI for embeddings.
        add(PROVIDERS["openai"]["envKey"], "OpenAI API key for knowledge embeddings")
        provider = PROVIDERS[spec.provider]
        add(provider["envKey"], f"{provider['name']} API key for knowledge extraction")
    used = {grant.connection_id for item in spec.agent_classes for grant in item.tools.mcp}
    for connection in spec.connections:
        definitions = {item.name: item for item in connection.setup_inputs}
        for field in connection.required_environment:
            item = definitions.get(field)
            add(field, f"{connection.name}: {item.description or field}" if item else f"{connection.name}: {field}",
                secret=item.secret if item else True, required=connection.id in used and (item.required if item else True))
        for value in [connection.command, connection.url, *connection.args, *connection.env.values(), *connection.headers.values()]:
            for key in PLACEHOLDER.findall(value):
                item = definitions.get(key)
                add(key, f"{connection.name}: {item.description or key}" if item else f"{connection.name}: {key}",
                    secret=item.secret if item else True, required=connection.id in used and (item.required if item else True))
                if item and item.default and not item.secret and not fields[key]["secret"]:
                    fields[key]["default"] = item.default
    if any(item.tools.workspace for item in spec.agent_classes):
        add("FILES_ROOT", "Connected folder (blank uses ./workspace)", secret=False, required=False)
    if spec.knowledge.mode != "none":
        add(spec.knowledge.token_env, "Elephant agent JWT", required=any(item.tools.knowledge for item in spec.agent_classes))
        add(spec.knowledge.space_env, "Elephant space ID (must match token scope)", secret=False, required=any(item.tools.knowledge for item in spec.agent_classes))
    return list(fields.values())


def _runtime(spec):
    resolved = resolved_blueprint(spec)
    classes = {item.id: item for item in spec.agent_classes}
    agents = [{**row, "classId": row["class_id"], "role": classes[row["class_id"]].description,
               "tools": classes[row["class_id"]].tools.model_dump()} for row in resolved["identities"]]
    connections = [item.model_dump() for item in spec.connections]
    if spec.knowledge.mode != "none":
        connections.append({"id": "elephant", "name": "Elephant knowledge", "transport": "http", "url": spec.knowledge.url,
                            "headers": {"Authorization": f"Bearer ${{{spec.knowledge.token_env}}}"}})
    return {"schemaVersion": 2, "mobkitVersion": MOBKIT_VERSION, "name": spec.project.name,
            "slug": spec.project.slug, "description": spec.project.description, "target": spec.target,
            "port": spec.advanced.port, "entrypoint": spec.topology.entrypoint,
            "classes": [item.model_dump() for item in spec.agent_classes], "agents": agents,
            "applicationProfiles": application_profiles(spec),
            "connections": [[edge.a, edge.b] for edge in spec.resolved_edges()],
            "mcpConnections": connections, "environment": environment_fields(spec),
            "flows": [flow.id for flow in spec.flows], "knowledge": spec.knowledge.model_dump(),
            "storage": {"runtime": ".mobkit/state", "notes": "data/notes.sqlite3"}}


def mob_toml(value):
    spec = _spec(value)
    resolved = resolved_blueprint(spec)
    profiles = {row["identity"]: row["profile"] for row in resolved["identities"]}
    reusable = application_profiles(spec)
    lines = ["[mob]", f"id = {_json(spec.project.slug)}", "",
             "[skills.launch_comms]", 'source = "inline"', f"content = {_json(COMMS_PROTOCOL)}", ""]
    lines += model_registry_lines(((item.provider, item.model) for item in spec.agent_classes), native_version=MOBKIT_VERSION)
    for item in spec.agent_classes:
        class_identities = item.identities()
        prompt = f"You are {item.name}. {item.description}\nTeam: {spec.project.name}\n\n{item.instructions}\n\nUse your connected peers when useful. Report evidence and uncertainty accurately."
        lines += [f"[skills.{item.id}_role]", 'source = "inline"', f"content = {_json(prompt)}", ""]
        # Reusable roles are not additional identities. The native roster is
        # still supplied by the host, and fixed flow roles stay instance-specific.
        roles = [(profile_id(item, index), index) for index in range(item.instances)]
        roles.append((reusable[item.id], None))
        for profile, index in roles:
            member_skill = f"{profile}_member" if item.members and index is not None else None
            if member_skill:
                member = item.members[index]
                member_prompt = f"Your durable identity is {class_identities[index]}. Your name is {member.name}.\n{member.instructions}"
                lines += [f"[skills.{member_skill}]", 'source = "inline"',
                          f"content = {_json(member_prompt)}", ""]
            skills = [item.id + "_role", "launch_comms"] + ([member_skill] if member_skill else [])
            lines += [f"[profiles.{profile}]", f"model = {_json(item.model)}", f"provider = {_json(item.provider)}",
                      "external_addressable = true", 'runtime_mode = "turn_driven"', 'resume_overrides = ["model", "provider"]',
                      f"skills = {_json(skills)}", f"peer_description = {_json(item.description)}", "",
                      f"[profiles.{profile}.tools]", "comms = true"]
            if item.tools.read_only:
                lines.append("read_only = true")
            for key in ("builtins", "shell", "memory", "workgraph", "schedule"):
                lines.append(f"{key} = {str(getattr(item.tools, key)).lower()}")
            lines += ["# MCP and local tools are attached per class by the host SessionBuilder.", ""]
    for flow in spec.flows:
        lines += [f"[flows.{flow.id}]", f"description = {_json(flow.name + ': ' + flow.description)}", ""]
        for step in flow.steps:
            message = step.prompt + "\n\nActivation parameters:\n{{ params }}"
            for dependency in step.depends_on:
                message += f"\n\nOutput from {dependency}:\n{{{{ steps.{dependency} }}}}"
            lines += [f"[flows.{flow.id}.steps.{step.id}]", f"role = {_json(profiles[step.agent])}",
                      f"message = {_json(message)}", f"depends_on = {_json(step.depends_on)}",
                      'dispatch_mode = "one_to_one"', f"timeout_ms = {step.timeout_ms}", ""]
    return "\n".join(lines)


def _console(spec):
    return "\n".join([
        f"title = {_json(spec.project.name)}", "[brand]", f"label = {_json(spec.project.name)}", "",
        "[appearance]", 'default_theme = "dark"', 'default_variant = "graphite"', "",
        "[environment]", f"label = {_json(spec.target)}", "[layout]", 'initial_preset = "two_columns"',
        f"initial_agent = {_json(spec.topology.entrypoint)}", "",
        "[agent_list]", 'group_by = ["labels.console_group"]',
        f"default_pinned_agent_ids = [{_json(spec.topology.entrypoint)}]", "",
        "[actions]", "show_reset = false", "",
    ])


def _readme(spec):
    rows = "\n".join(f"| {item.name.replace('|', '/')} | {item.instances} | {item.provider} / {item.model} | {item.description.replace('|', '/')} |" for item in spec.agent_classes)
    flow_cmd = f"bash start.sh --run-flow {shlex.quote(spec.flows[0].id)} --params '{{\"topic\":\"Your question\"}}'" if spec.flows else "bash start.sh --list-flows"
    install = [f"Extract into a new folder, such as `~/src/{spec.project.slug}`."]
    if spec.target == "gcp":
        install += ["On your computer, review `bash deploy/deploy-gcp.sh --dry-run`, then run `bash deploy/deploy-gcp.sh`. It asks for your project ID before provisioning and installs the package on the VM.",
                    f"Connect with `gcloud compute ssh {spec.project.slug} --project {spec.gcp.project} --zone {spec.gcp.zone} --tunnel-through-iap`. On the VM, run `cd ~/src/{spec.project.slug}` and `.venv/bin/python setup.py`. Keep the remaining setup commands in this SSH session."]
    else:
        install += [f"Run `bash install.sh` with Python 3.11+ available. This installs pinned Meerkat/MobKit {MOBKIT_VERSION} and the official MCP Python SDK {MCP_VERSION}, then verifies native gateway downloads using published SHA-256 checksums.",
                    "Enter provider keys and connector values when the local interview asks. Values stay in a mode-600 `.env` on the deployment machine. The workbench never receives them."]
    if spec.connections:
        install += ["Install the selected MCP prerequisites listed below on the deployment machine before checking connections."]
    if spec.knowledge.mode == "local":
        install += ["Install Docker with Compose and Git on the deployment machine. Start the selected knowledge service with `.venv/bin/python knowledge/manage.py up`, then run `.venv/bin/python knowledge/manage.py verify`. Review `knowledge/README.md` for source access and semantic ingestion prerequisites."]
    elif spec.knowledge.mode != "none":
        install += ["Verify the existing Elephant endpoint, token and space with `.venv/bin/python knowledge/manage.py verify`."]
    install += ["Run `bash start.sh --doctor`. For selected MCPs or Elephant, run `bash start.sh --connections-check` to perform real handshakes and validate selected tool names."]
    install += ["On the VM, run `bash deploy/install-service.sh`. Back on your computer, run `bash deploy/tunnel-gcp.sh` and keep that tunnel open." if spec.target == "gcp" else "Run `bash start.sh` and keep the terminal open. Stop with Control-C.",
                f"Open [the native MobKit console](http://127.0.0.1:{spec.advanced.port}/console) on your computer."]
    install_steps = "\n".join(f"{index}. {step}" for index, step in enumerate(install, 1))
    return f"""# {spec.project.name}

{spec.project.description}

A complete MobKit deployment compiled from `blueprint.json` by MobKit Launch.

## Install and start

{install_steps}

First startup initializes storage with MobKit's official offline migration tool. Agents wait for user input. `--smoke` makes one paid provider request. The console is loopback-only; its session, topology, flow and tool activity are real runtime projections.

## Agent classes and exact tool grants

| Class | Instances | Model | Purpose |
| --- | --- | --- | --- |
{rows}

`blueprint.json` holds reusable class intent. The compiler derives one native profile per initial identity so a workflow targeting an instance runs on exactly that instance. `deployment.json` also maps each class to an `applicationProfiles` role containing its class instructions and grants, without an initial member's identity or private instructions. These roles are not additional agents. A class with `instances: 0` compiles only its reusable application profile, with no initial durable identity or private member instructions. Keep its `members` empty. Do not invent an unassigned or template identity when the real owners are unknown. At least one actual initial identity must remain as the topology entrypoint; edges and fixed workflow targets cannot refer to a profile-only class. The native identity roster owns durable agents and the native topology provider applies the configured peer edges. Native profiles own provider, model, builtin, shell, memory, WorkGraph and scheduling capabilities.

This package describes {len(spec.identities())} durable identities. Compilation validates configuration, not host capacity or concurrent throughput. Size the deployment and validate a representative workload before activating the full population. A fixed roster does not automatically scale workers or isolate users.

Workspace tools only read visible text inside `FILES_ROOT` or `workspace/`. SQLite notes grants distinguish no access, search-only, and search/save. Each class receives only its MCP connection grants; a nonempty tool-name list is an exact allowlist. MCP transports use the official SDK, without a shell command wrapper, and all callbacks are restored on every process restart. Unassigned MCP connections are never started. Stdio subprocesses receive ordinary process paths plus explicitly mapped variables, not every provider credential from the parent environment.

MCP tool names are mapped to provider-safe aliases with a stable digest; their descriptions preserve the original server and tool name. `--connections-check` fails if a selected tool disappeared. Selecting all tools permits future server additions. External tool content is data and may be untrusted. Some connectors can modify external services; select narrow grants for classes that only need reads.

## Native workflows

```bash
bash start.sh --list-flows
{flow_cmd}
```

Stop the main app before these commands, because they acquire the same deployment lock. The native MobKit flow engine owns dependency dispatch, parallel steps, timeouts, run state and terminal outcomes. Prior step outputs and activation parameters are delivered by the native flow engine. Flows also appear in the console. Steps targeting the same identity share its durable conversation; use separate class instances for independent parallel work. Closing this CLI while a flow is active stops this process; resume normal startup and inspect native run state rather than launching a duplicate.

## Change an existing deployment

Keep `blueprint.json` as editable source and use Launch to compile changes. Review the generated file diff before replacing configuration in a stopped deployment. Back up `.mobkit/`, `data/`, `workspace/`, configuration and secure credentials first.

Native storage pins the canonical mob definition. To activate a changed `config/mob.toml`, run `bash start.sh --activate-revision N`, supplying the revision observed with `bash start.sh --definition-revision` before replacing the configuration. That command reads the native structural event ledger and performs no agent requests. If the configuration is already replaced, restore the previous config to inspect the revision first. The native compare-and-swap ceremony rejects a stale revision. Later ordinary restarts omit the flag. Do not guess or increment revisions in local state files. Tool callback grants and desired roster/topology are product configuration read from `deployment.json`; changes to durable roles can require native identity role migration. Do not reuse an existing identity for a different class.

Activating edited class instructions updates the canonical definition without rewriting an existing conversation. This host projects the canonical native profile skills onto each ordinary application-authored `send` or keyed `dispatch` as explicit native System content, preserving identity, admission keys and history. A same-key dispatch retry replays its original admission and does not adopt newly authored instructions; a new actual host turn uses the current role. A gateway that advertises host System authoring supplies that carrier. A shipping older gateway retains ordinary SDK host admission when its actual native ledger proves the unchanged initial definition at epoch 1. After a later definition revision, unsupported role authoring is rejected before the turn; role-changing updates are blocked before source replacement without both send and dispatch support. The pinned Python SDK can use the supported native request carrier without replacing SDK bytes. The currently pinned official release may lack that native capability; a verified local development runtime is separate evidence from an official installation. Native flows, schedules, internal peer turns and direct Console input are outside this host carrier. Definition activation or healthy bootstrap alone is not evidence that continuing sessions adopted the edited role. Role-changing updates retain all current private data and observe the real native definition revision before CAS activation. A previous legacy host without this carrier cannot establish continuing-role rollback after native admission; recover the current accepted application instead of deleting or rewinding history.

## Custom application code

Optional ordinary Python code in `application/__init__.py` can export `build(context)`, synchronous or asynchronous, returning an integration object. The context exposes `root` (this package's Path), `deployment` (compiled configuration), `stop_event` (an asyncio.Event requesting host shutdown), and `runtime` (None during construction, the real native MobKit runtime after bootstrap). Keep construction and imports free of external actions; open connections and start background adapters in `start()`.

The integration may implement `tools_for_agent(agent)`, `start()` and `stop()`, synchronously or asynchronously. `tools_for_agent` receives each compiled agent including its `identity`, `classId`, `profile` and configured grants. Return a list of exact callbacks with `name`, `handler`, `description` and JSON `schema`; return an empty list for other classes. Names must be provider-safe ASCII letters, digits, underscores or hyphens, beginning with a letter, up to 64 characters. The host validates definitions/collisions, registers callbacks through native SessionBuilder, and treats their outputs as untrusted evidence for memory. Application handlers can accept named JSON fields as keyword parameters or a single argument dictionary. The host checks keyword binding first, then dictionary binding before invoking once; sync and async results are supported. For a dictionary handler, choose a parameter name distinct from JSON field names so binding is unambiguous. Non-inspectable callables use the SDK one-dictionary convention. Exceptions from application code propagate without another invocation. Package code owns its explicit class scoping; the integration does not add blueprint grants or confer user authorization.

For applications that create continuing contexts through their UI, paired `desired_roster()` and `desired_edges(target_identities)` hooks supply an application-owned desired snapshot. Read `app.py` for the exact ordinary row protocol. Retain every compiled seed identity and its class; new identities select an agreed `classId`, and the host derives its reusable `applicationProfiles` role and existing grants. Persist stable identity/class mappings in the application's registry. `context.reconcile_agents()` validates one complete snapshot and asks the real native runtime to reconcile it. Desired records are not proof of native readiness; retain failed or unknown outcomes and inspect actual native identity status. Omitting an extra identity requests native retirement without deleting application records. Archiving a project can instead retain its identities and block new jobs through application policy. The host binds dynamic callbacks to the actual adopted identity/context on creation and resume. Their names and schemas must remain within the class catalog established before native initialization. A profile-only class can gain real identities later with its configured native, local and MCP capabilities. If no actual desired identity from that class exists at startup, its application callback catalog is empty: later identities cannot add custom application callback names or schemas. An actual initial desired roster can establish that class catalog before native initialization. These application-owned real identities are additional startup intent, not compiled seeds or proof of materialization. Otherwise rebuild with the required real startup contexts; do not create a placeholder merely to obtain a catalog. No lazy catalog expansion or new registration hook is provided. This prevents a new callback from bypassing the fixed tool-content trust configuration. These hooks create no accounts, access policy or authorization. The offline trial keeps its frozen native roster and `context.runtime=None`; it does not materialize new identities.

Native identity memory remains available to classes that grant it. Generated hosts disable automatic distillation and stewardship into shared memory, and omit operator scope. Normal native Recorder tools bind writes and recall to their own identity. Approved shared knowledge belongs in an explicitly authorized application library or shared store; existing or explicitly promoted Mob/Realm memory is still shared. Neither this posture nor separate identities enforce application membership, private file access or authorized result delivery. Those checks remain application responsibilities.

`start()` runs after native identity bootstrap only in normal service mode. Inspection, connection checks, definition activation and flow CLI commands do not start background adapters. `stop()` runs before native shutdown, including failed starts and diagnostics, and must tolerate an integration that never started. Keep task cancellation, connections and servers in your integration's lifecycle; use native runtime methods for agent sends and flows rather than a parallel agent engine. File layout, UI, domain adapters and their configuration remain ordinary application code, outside the blueprint schema.

An optional `rehearse(context, fixtures)` factory, synchronous or asynchronous, returns an offline integration with the same `tools_for_agent(agent)` protocol. Reuse the production callbacks and domain state logic, substituting explicitly supplied fake I/O through ordinary adapter interfaces. Keep production tool names, input schemas and per-identity scoping. The trial context's `runtime` remains `None`; only its private `root/data` and `scratch_root` are writable. Network, credentials, user files, process creation and source writes are unavailable. Fixtures are bounded ordinary JSON supplied for that trial; document the values needed and omit callbacks whose fake dependencies are missing. Trial `start()` and `stop()` own fake resources and must handle partial startup. Launch executes this code in a separate constrained process while actual native agents own the conversation and collaboration. Retained results describe the exact source's offline behavior; they are not independent acceptance, connected adapter evidence or deployment proof.

Optional `schedule_runnables()` returns a list of exact `{{name, handler}}` definitions, synchronously or asynchronously. The host supplies these names through the published SDK's `MobKitBuilderConfig.host_runnables`, constructs `MobKitRuntime`, and binds handlers through `runtime.on_schedule_fire` before `connect()` can restore due occurrences. `context.runtime` is assigned before binding. Each handler receives the native occurrence dictionary (`schedule_id`, `occurrence_id`, `due_at`, optional `payload`); returning completes the attempt and raising fails it. This hook does not create schedules or add schedule tools to any agent. Create and inspect schedules through existing granted `meerkat_schedule_*` tools; if the design has no authoring grant, that remains a setup requirement. The native schedule store owns timing, claims and occurrence outcomes. Flow CLI tool handlers and schedule handlers must lazily open needed adapters because `start()` only runs in service mode, and restored occurrences can arrive before it.

For authenticated human chat, ordinary application source may add `application/console.json` containing exactly `{{"authentication":"local-password"}}` and declare two required fields without source defaults in `application/configuration.json`: `CONSOLE_USER_ID` (nonsecret) and `CONSOLE_LOGIN_PASSWORD` (secret, at least 12 characters). Install collects these fields privately; source and model conversations must never contain their values. This option provides one local human account. It does not provide multi-user login or authenticate Telegram or Slack.

The host serves the unchanged native Console at the configured loopback URL with a password login and an HttpOnly, SameSite=Strict session cookie. Native runs on an internal ephemeral loopback port with JWT authentication. Bearer tokens stay in the host, never in browser URLs or storage. The signing key is generated in private `.mobkit/console-signing.key`; a restart clears browser sessions. Health credentials are read-only and cannot submit human messages.

An optional integration `on_console_input(event)` receives only successfully admitted authenticated Console submissions. Its frozen event has `channel="host-authenticated-console"`, the configured `principal`, submitted `identity` and `content`, and the native `interaction_id`, `session_id`, `input_frame_id`, `cursor`, optional `conversation_id` and `handling_mode`. The host validates native admission before calling this hook. Plain SDK sends, tools, timeline frames, caller-supplied principal/origin, rejected admissions and health probes cannot emit it. String and multipart Console submissions are supported; uploaded image references in `content` remain submitted references, and native owns blob conversion. Live voice interactions do not produce this chat hook. The application owns domain interpretation and must bind any approval to the exact action, human, scope, expiry and current state at the side-effect boundary. Receiving a message does not itself grant authority.

Native may start its agent turn before this hook runs. Never let an agent tool infer approval from the chat text or a reply; it must read application-owned authorization state. The host awaits hook delivery and deduplicates successful native interaction IDs for this process. A failed delivery returns an explicit error even though native may have admitted the turn. Persist application deduplication using native IDs, make the handler idempotent, and fail closed after interrupted delivery or restart. Do not add a parallel native event log. In MobKit 0.8.42, timeline frames do not retain an authenticated human principal; this is host-authenticated channel evidence, not a principal-attesting native timeline. Native gating UI can also provide explicit gate decisions through its existing supported APIs; agent tools must never expose trusted-host `gating_decide` approver authority.

Schedule handlers are bound before native startup, including storage preparation and restored due delivery. Offline contract checks verify names and callback wiring; they do not prove an installed schedule, persisted delivery or restart recovery. Verify those behaviors on the installed system before depending on scheduled application actions. Native create has no submission idempotency key: inspect exact owned schedule IDs or labels before creating, serialize initialization, and do not stack another recurrence on every startup.

Pinned MobKit 0.8.42 advertises session, identity and mob targets in its native schedule create/update tool schema; it omits the deterministic `host_runnable` target despite the callback runtime support. Do not bypass the advertised tool contract or invent a Python schedule-creation client. A recurring agent-owned workflow can use the granted native tools' `current_session` shortcut with a prompt action; MobKit persists its stable mob-member target. That agent then uses its actual scoped application tools. This performs an agent turn on each firing, rather than direct deterministic host-runnable delivery. Keep provider cost, timezone, missed-run policy, overlap policy, the exact action authorization and live integration requirements visible. Installation requires actual native create/get/occurrence receipts and a restart check; assistant prose and offline callback tests are insufficient.

Put additional pinned Python dependencies in optional `requirements.application.txt`; `install.sh` installs them into the deployment environment when present. Both that file and `application/` are included in GCP transfer, excluding private environment files, state and caches. `bash start.sh --check` syntax-checks application Python without importing it. `bash start.sh --application-check` imports/builds the integration and validates callback definitions, but does not call `start()` or any tool handler; it is a contract check, not proof of a working adapter. Launch may load application code in isolated offline checks. Live adapters run when the user starts the exported application.

## Connections and knowledge

Inspect `config/connections.json` for exact MCP commands/endpoints and source provenance. Only local `${{ENV_NAME}}` references belong in exported configuration. Elephant access requires both the expected space ID and a JWT carrying the same `space_id` claim and a future expiry. Startup checks those claims before opening any MCP connection; Elephant verifies the signature and permissions. Opaque tokens cannot provide the required scope evidence and are refused. Install the executable needed by a stdio connector (for example Node/npm for `npx`, `uv` for `uvx`, or Docker) before connecting it. The studio records source metadata; installing third-party packages executes those publishers' code.

{'Elephant setup and token-scope verification are documented in `knowledge/README.md`.' if spec.knowledge.mode != 'none' else 'No Elephant knowledge service was selected. Add one in Launch and grant knowledge access to the desired classes.'}

## Local service and Google Cloud

On Linux, `bash deploy/install-service.sh` runs one guided private setup interview, then checks the installed application and starts an owned user systemd service. It waits for actual native bootstrap and authenticated Console health. Repeat `bash deploy/install-service.sh status`, `stop`, `restart` or `remove-service` to operate that exact unit. Removal preserves source, private configuration and native state. It never overwrites an unrelated unit. The interview explicitly asks before enabling this user's services after logout; that system setting can require sudo. Without it the service runs only while that user is logged in. On macOS, keep the terminal open. Stop with Control-C.

For GCP, inspect `bash deploy/deploy-gcp.sh --dry-run`, then run it. It asks for the exact configured project ID before creating billed resources: Debian 12 VM ({spec.gcp.machine_type}), {spec.gcp.disk_gb} GB boot disk, IAP SSH rule. The console is never exposed by a public firewall rule. Enter credentials on the VM; private `.env`, data, runtime state, and workspace files are excluded from transfer. The VM has no service account. Existing project network rules still apply. Install your selected MCP prerequisites on the VM too. `bash deploy/tunnel-gcp.sh` opens local access. Delete the VM when finished to stop compute charges; the script prints cleanup commands.

After provisioning, run `bash deploy/deploy-gcp.sh --setup` from the original local package folder. This opens an interactive IAP SSH terminal for the private interview and verified service start; credentials never enter command arguments or this local folder. Later use `--status`, `--stop`, `--restart` or `--remove-service`. These commands require the matching VM ownership label and never create a replacement VM. Stopping or removing the application service does not stop VM charges. Console health does not prove custom UI, external chat, scheduled actions or other external workflows.

If upload, SSH readiness or remote installation fails, keep the original package folder and run `bash deploy/deploy-gcp.sh --resume`. The private `.mobkit/gcp-deployment.json` receipt must match the configured project, zone, VM, complete transferred source and VM ownership label. Upload verifies the accepted SHA256SUMS manifest, includes ordinary custom UI/assets paths, and excludes private credentials, data and runtime state. Resume retries upload and installation without creating another VM or replacing credentials and runtime data. It refuses unrelated VMs and changed source. Recovery and cleanup commands are printed even when installation fails.

## Files

- `blueprint.json`: source design, classes, grants, workflows, topology, provenance.
- `deployment.json`: compiled desired roster, tool grants and local environment interview.
- `config/mob.toml`: native profiles, shared role skills and native flows.
- `config/console.toml`: native console presentation only.
- `app.py`, `mcp_connections.py`, `connectors.py`: host integration and scoped tools.
- `.mobkit/state/`: native sessions, memory, events, flow run state and blobs.
- `data/notes.sqlite3`: shared application notes, restricted by class grants.

All data stays local except requests sent to the selected model providers and connected external services. Provider API usage and cloud hosting are billed by their services. Backup while stopped and preserve complete runtime state including blobs. Do not commit secrets or runtime data.
"""


def generate_package(value):
    spec = _spec(value)
    runtime = _runtime(spec)
    files = [PackageFile("native_definition_continuity.py",
        (Path(__file__).parent / "native_definition_continuity.py").read_text())]
    for name in ("app.py", "mcp_connections.py", "setup.py", "application_configuration.py", "console_configuration.py", "console_auth.py", "remote_operations.py"):
        text = (TEMPLATES_V2 / name).read_text()
        if name == "remote_operations.py":
            text = render_destination_shutdown_budget(text)
        files.append(PackageFile(name, text))
    for name in ("connectors.py", "storage_setup.py", "install_gateway.py", "start.sh", "install.sh"):
        text = (TEMPLATES / name).read_text()
        if name == "install.sh":
            text = text.replace("3.10", "3.11").replace("(3, 10)", "(3, 11)")
            text = text.replace(".venv/bin/python install_gateway.py",
                                "if [[ -f requirements.application.txt ]]; then\n  .venv/bin/python -m pip install --disable-pip-version-check -r requirements.application.txt\nfi\n.venv/bin/python install_gateway.py")
        files.append(PackageFile(name, text, 0o755 if name.endswith(".sh") else 0o644))
    files += [PackageFile("blueprint.json", spec.model_dump_json(indent=2) + "\n"),
              PackageFile("deployment.json", json.dumps(runtime, ensure_ascii=True, indent=2) + "\n"),
              PackageFile("requirements.txt", f"meerkat-mobkit=={MOBKIT_VERSION}\nmcp=={MCP_VERSION}\nhttpx==0.28.1\nstarlette==1.7.0\nuvicorn==0.54.0\nPyJWT==2.15.0\njsonschema==4.26.0\n"),
              PackageFile("config/mob.toml", mob_toml(spec)), PackageFile("config/console.toml", _console(spec)),
              PackageFile("config/connections.json", json.dumps(runtime["mcpConnections"], ensure_ascii=True, indent=2) + "\n"),
              PackageFile("README.md", _readme(spec)),
              PackageFile(".gitignore", ".env\n.venv/\n.bin/\n.mobkit/\ndata/\nworkspace/\nknowledge/.env\nknowledge/.source/\n__pycache__/\n*.pyc\n"),
              PackageFile("workspace/START-HERE.txt", "Place text references here. Only classes with workspace access may read them.\n")]
    env = ["# Private values are entered on this deployment computer.", f"MOBKIT_PORT={spec.advanced.port}"]
    for field in runtime["environment"]:
        env += ["# " + field["label"], field["key"] + "=" + (json.dumps(field["default"]) if field.get("default") else "")]
    files.append(PackageFile(".env.example", "\n".join(env) + "\n", 0o600))
    if spec.knowledge.mode != "none":
        from .elephant_setup import generate_elephant_files
        for path, content in generate_elephant_files(spec.knowledge.model_dump(), spec.project.slug, spec.provider, spec.model).items():
            files.append(PackageFile(path, content, 0o755 if path.endswith(".sh") else 0o644))
    deploy = (TEMPLATES / "deploy-gcp.sh").read_text().replace("30 GB disk", f"{spec.gcp.disk_gb} GB disk").replace("--boot-disk-size 30GB", f"--boot-disk-size {spec.gcp.disk_gb}GB")
    for key, value in {"__PROJECT__": shlex.quote(spec.gcp.project), "__ZONE__": shlex.quote(spec.gcp.zone), "__INSTANCE__": shlex.quote(spec.project.slug), "__MACHINE__": shlex.quote(spec.gcp.machine_type), "__PORT__": str(spec.advanced.port)}.items():
        deploy = deploy.replace(key, value)
    files += [PackageFile("deploy/deploy-gcp.sh", deploy, 0o755),
              PackageFile("deploy/install-service.sh", (TEMPLATES / "install-service.sh").read_text(), 0o755),
              PackageFile("deploy/tunnel-gcp.sh", f"#!/usr/bin/env bash\nset -euo pipefail\nexec gcloud compute ssh {shlex.quote(spec.project.slug)} --project {shlex.quote(spec.gcp.project)} --zone {shlex.quote(spec.gcp.zone)} --tunnel-through-iap -- -N -L {spec.advanced.port}:127.0.0.1:{spec.advanced.port}\n", 0o755)]
    hashes = "".join(f"{hashlib.sha256(item.content.encode()).hexdigest()}  {item.path}\n" for item in sorted(files, key=lambda item: item.path))
    files.append(PackageFile("SHA256SUMS", hashes))
    return files


def generate_files(value):
    return {item.path: item.content for item in generate_package(value)}


def continuation_contract(files):
    """Configuration and dependency bytes that cannot cross a source-only repair.

    Host implementation templates may be retained from an accepted package,
    but a new native definition, setup contract or pinned dependency requires
    the freshly compiled framework and a separately verified migration.
    """
    return {item.path: {"sha256": hashlib.sha256(item.content.encode()).hexdigest(), "mode": item.mode}
            for item in files if item.path in {"blueprint.json", "deployment.json", ".env.example"}
            or item.path.startswith("config/")
            or Path(item.path).name.startswith("requirements")}


def package_preview(value):
    spec = _spec(value)
    files = generate_package(spec)
    checks = [{"label": "Native runtime", "status": "configured", "detail": f"{len(spec.identities())} durable identities across {len(spec.agent_classes)} classes"},
              {"label": "Topology", "status": "configured", "detail": f"{len(spec.resolved_edges())} peer edges"},
              {"label": "Workflows", "status": "configured", "detail": f"{len(spec.flows)} native DAG workflows"},
              {"label": "Credentials", "status": "local-setup", "detail": "Collected on the deployment computer"}]
    return {"valid": True, "issues": [], "resolved": resolved_blueprint(spec), "files": [item.to_dict() for item in files],
            "checks": checks, "summary": {"classes": len(spec.agent_classes), "agents": len(spec.identities()), "connections": len(spec.connections), "flows": len(spec.flows), "files": len(files)},
            "file_count": len(files), "name": spec.project.slug + ".zip", "mobkit_version": MOBKIT_VERSION}


def build_archive(value):
    spec = _spec(value)
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w", zipfile.ZIP_DEFLATED) as archive:
        for item in generate_package(spec):
            info = zipfile.ZipInfo(f"{spec.project.slug}/{item.path}")
            info.create_system = 3
            info.external_attr = (0o100000 | item.mode) << 16
            info.compress_type = zipfile.ZIP_DEFLATED
            archive.writestr(info, item.content.encode())
    return output.getvalue()
