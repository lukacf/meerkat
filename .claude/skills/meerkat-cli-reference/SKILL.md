---
name: meerkat-cli-reference
description: "Exact rkat CLI command contract for Meerkat help answers. Use this as the authority for command names, flags, aliases, and negative CLI facts."
---

# Meerkat CLI Reference

Use this skill as the exact authority for `rkat` shell commands. Command
spelling, singular/plural forms, flags, and enum values matter.

## Hard Negative Facts

- There is no `rkat sessions`, `rkat realms`, or `rkat skills`; use singular
  `rkat session`, `rkat realm`, `rkat skill`.
- There is no `rkat resume`, `rkat continue`, or `rkat c`; use
  `rkat run --resume ...`.
- There is no `rkat rpc`; use the `rkat-rpc` binary for JSON-RPC.
- An unknown first word is not rejected as a command: it becomes the prompt,
  so `rkat rpc` runs `rkat run rpc` (extra unquoted words then fail to parse).
  Only the legacy names above are refused.
- `rkat-rpc --tcp` is not a Meerkat signed peer/comms listener; use `rkat run --comms-listen-tcp ...` for remote peers and external mob members.
- There is no root `rkat live` group. Use JSON-RPC `live/*` for ordinary
  sessions; mob members (local or placed) also have the `rkat mob live`
  group, WebSocket only.
- There is no `rkat image` command.
- There is no `rkat schedule` command; manage schedules through JSON-RPC
  `schedule/*`, REST `/schedules`, the SDK schedule helpers, or the agent
  `meerkat_schedule_*` tools inside `rkat run`.
- There is no `rkat mcp reload` CLI command.
- There is no `rkat mcp logout`; sign out with RPC `auth/logout` and an `mcp`
  target (Python `auth_mcp_logout`, TypeScript `authMcpLogout`). There is no
  CLI command to set MCP service secrets (`secret_env` values); a Rust host
  uses `meerkat::set_mcp_service_secret`.
- There is no `--tools all`; valid values are `safe`, `workspace`, `full`, `none`.
- OS sandboxing is off unless the `[confinement]` config section or the
  root `--confinement <trusted-host|workspace>` flag turns it on; without them
  `rkat` runs shell commands, command hooks and stdio MCP servers as trusted
  host processes. `--tools none`, the shell's `restrict_to_project` check and
  `--isolated` are not OS sandboxes. See `references/security.md` of the
  meerkat-platform skill.
- `rkat blob get` has no `-o` or `--out`; use `--output <FILE>`.
- `rkat session show` has no `--json` flag.
- `rkat models` and `rkat capabilities` take no arguments (no `--json`/`--format`, no `rkat models catalog`) and already print pretty JSON.
- `rkat doctor` (setup checks, no flags) and `rkat storage doctor` (storage diagnosis, `--json`/`--root`) are different commands.
- `rkat storage prune` deletes only registered maintenance backup artifacts; realm deletion is `rkat realm delete` / `rkat realm prune`.
- `--isolated` and `--default-model` cannot be combined with `rkat storage`; both are usage errors (exit 2). `rkat mob host` also rejects `--isolated`.
- `rkat run` has no hook flags; per-session hook overrides (`hooks_override`)
  exist only on RPC, REST, MCP and the SDKs.
- There is no manual compaction command on any surface.
- `rkat` has no JSON-lines event output; `--output json` prints only the final
  result.
- `rkat session interrupt` cannot stop a turn running in another process.

## Root

```bash
rkat [OPTIONS] <PROMPT>      # shorthand for rkat run <PROMPT>
rkat [OPTIONS] <COMMAND>
```

Global/realm options:

```bash
-r, --realm <REALM>
--isolated
--as-agent <AGENT>              # governed realms: act as this agent (default rkat-agent)
--instance <INSTANCE>
--realm-backend <jsonl|memory|sqlite>
--state-root <PATH>
--context-root <PATH>
--user-config-root <PATH>
--default-model <MODEL>
--confinement <trusted-host|workspace>   # OS confinement of shell, hooks, MCP stdio
--confinement-network <denied|unrestricted>
--confinement-read <PATH>                # repeatable; needs --confinement workspace
--confinement-write <PATH>               # repeatable; needs --confinement workspace
```

The `--confinement` flags replace the `[confinement]` config section for every
launch kind of the invocation; `trusted-host` is the operator's opt-out. A
confined command whose processes could change its realm state or user config
document is refused: on Linux keep the state outside the workspace with
`--state-root`.

`-h` prints the basic view and `--help` every option; realm options and
advanced flags appear only under `--help`. `--as-agent` is a root option
like `--realm` and works before or after the command name
(`rkat --as-agent <AGENT> run ...` or `rkat run --as-agent <AGENT> ...`);
0.9.0 accepted it only after the command name.

`--default-model <MODEL>` persists the default agent model into the
scope-resolved config (same project/user/realm resolution as every other
command) and exits when given without a command, or applies first when
combined with one. The model is validated against the catalog and any
configured custom models:

```bash
rkat --default-model claude-fable-5
```

Top-level commands:

```bash
rkat init
rkat run <PROMPT> [OPTIONS]
rkat help <QUESTION> [OPTIONS]
rkat session list|show|repair-wholeblob|resolve-held-tail|export-atif|delete|interrupt ...
rkat blob get <BLOB-ID> [--output <FILE>] [--json]
rkat realm current|list|show|create|govern|grant|revoke-grant|delete|prune ...
rkat mcp add|login|remove|list|get ...
rkat mob spawn-helper|fork-helper|member-status|member-history|force-cancel|respawn|wait-kickoff|grant|revoke-grant|grants|host|bind-host|revoke-host|hosts|route-installs|live|run|runs|status|logs|attach|run-flow|flow-status|pack|inspect|validate|deploy|web ...
rkat workgraph list|show|ready|snapshot|events|goal-create|goal-status|goal-confirm|goal-close|attention-list|attention-pause|attention-resume ...
rkat skill add|remove|get|list|inspect ...
rkat config get|set|patch ...
rkat capabilities
rkat models
rkat doctor
rkat storage doctor|migrate|prune ...
rkat auth realms|profiles|profile|profile-delete|bindings|test|status|login|logout|refresh|connector-client ...
```

`mcp`, `mob`, `skill`, `realm govern|grant|revoke-grant`, and `--as-agent`
exist only when their features are compiled in; the default build and the
release binaries include them.

## Run

```bash
rkat run <PROMPT>
rkat <PROMPT>
rkat run --resume[=<SESSION>] <PROMPT>
```

Common options:

```bash
--resume [<SESSION>]            # resume SESSION (default last); since 0.9.1 a lone positional after a bare --resume is the prompt for last, unless it names a session
-m, --model <MODEL>
-d, --max-duration <DURATION>
-o, --output <text|json|html>
--json                          # alias for --output json
-s, --stream
--no-stream
--no-web-search                 # disable provider-native web search for this run
--html                          # alias for --output html
--browser                       # output html and open it in the browser
--open-in-browser               # open HTML output after writing the artifact
--html-template <NAME>          # builtin: polished
--html-template-file <PATH>
--skill <PATH_OR_ID>            # repeatable
-t, --tools <safe|workspace|full|none>
-v, --verbose
--keep-alive
--stdin <auto|blob|lines|off>
```

Advanced options (`rkat run --help` only):

```bash
--system <SYSTEM_PROMPT>
-p, --provider <anthropic|openai|gemini|self-hosted>
--max-tokens <N>                # output cap per model call (not a total budget)
--max-tool-calls <N>            # tool-call budget; run completes as budget_exhausted
--yolo                          # alias for --tools full
--param <KEY=VALUE>             # repeatable
--params-json <JSON>
--schema <SCHEMA>               # inline JSON when it starts with {, else a file path
--export-atif                   # opt-in: write an ATIF trajectory after the turn (run and --resume)
--allow-tool <TOOL>             # repeatable first-turn allow overlay
--block-tool <TOOL>             # repeatable first-turn block overlay
--label <KEY=VALUE>             # repeatable
--instructions <TEXT>           # repeatable
--app-context <JSON>
--wait-for-mcp
--mcp-auth <stored|interactive> # default stored
--line-format <text|json>       # how --stdin lines input is parsed (not output)
--comms-name <NAME>                         # stable signed comms participant name
--comms-listen-tcp <HOST:PORT>              # signed agent-to-agent TCP listener
--comms-advertise-tcp <HOST:PORT>           # advertised signed comms address; DNS name, IPv4 or [IPv6], optional tcp:// (0.9.1)
--comms-binding-out <PATH>                  # write external mob/runtime binding JSON
--comms-pairing-password <PASSWORD>         # one-time signed comms enrollment password
--comms-pairing-password-env <ENV>          # read enrollment password from env
--comms-pairing-password-file <PATH>        # read enrollment password from file
--agent-description <TEXT>
--agent-label <KEY=VALUE>                   # repeatable peer metadata
--no-comms                                  # disables comms runtime/tools
```

Auth options: `--auth-binding <REALM:BINDING[:PROFILE]>` (shown by `-h` too).

Remote signed peer example:

```bash
rkat run \
  --comms-name remote-worker \
  --comms-listen-tcp 0.0.0.0:4200 \
  --comms-advertise-tcp 203.0.113.7:4200 \
  --comms-binding-out ./remote-worker.binding.json \
  --keep-alive \
  "You are a remote worker."
```

`--comms-listen-tcp`, `--comms-advertise-tcp`, `--comms-binding-out`, and
pairing password flags cannot be combined with `--no-comms`. Use only one
pairing password source; pairing passwords must be at least 32 bytes. The
generated binding has `kind: "external"`, advertised address, Ed25519 public
identity, and typed `bootstrap_token`.

Output flag rules: `--json` cannot be combined with `--output html`, nor
`--html`/`--browser` with `--output json`; `--open-in-browser` and the template
flags require HTML output; `--html-template` and `--html-template-file` are
mutually exclusive; HTML output cannot be combined with `--schema`;
`--stream` and `--no-stream` are mutually exclusive.

Resume targets: full UUID, UUID prefix/tail handle, `<realm>:<uuid>` (the
active realm), `last`, `~`, `~0`, or `~N`.

Defaults:

- `--tools safe`
- default model is OpenAI `gpt-6-astra` unless `-m`/`-p`, `--auth-binding` (the binding's `default_model`), `[agent] model` or `[models] <provider>` selects another; an Anthropic or Gemini env key alone does not change it. Catalog support does not imply access through Azure, Copilot, or custom backends
- current default Gemini model is `gemini-4-argon` (paid Gemini API tiers only); `gemini-3.8-flash` is the recommended Flash model and the free-tier choice (`--model gemini-3.8-flash` or `[models] gemini = "gemini-3.8-flash"`), and `rkat auth login google` and Copilot Gemini route bindings start on it
- CLI realm state is project-local by default: `<project-root>/.rkat/realms/<ws-...>/`, where the project root is the nearest ancestor containing `.rkat` and falls back to the context root outside a project; that head realm's config composes over its parent chain and the HOME-rooted `global` doc (`~/.rkat/config.toml`)
- `--verbose` also prints the active realm root
- provider-native web search is on for models that support it; `--no-web-search` turns it off
- text output streams by default when stdout is a TTY; JSON/HTML output and pipes/scripts do not
- piped stdin is blob context unless `--stdin lines` (which implies `--keep-alive`)
- for a new run, `--tools` decides builtins and shell even when
  `tools.builtins_enabled` or `tools.shell_enabled` differ; `full` adds
  WorkGraph, memory and mob tools. `--resume` keeps the stored choice unless
  `--tools`/`--yolo` is given
- a shell call without `timeout_secs` gets `[shell] timeout_secs` (default 30 s;
  0 keeps 30 s) since 0.9.1; 0.9.0 always used 30 s
- short session handles are UUID tails; resume accepts full UUID, prefix/tail, `<realm>:<uuid>`, `last`, `~`, or `~N`
- `@path` in the prompt inlines that UTF-8 file, resolved from `--context-root` or the current directory; `@/...`, `@~/...`, `@./...`, and `@../...` must name an existing file, while a bare `@name` that is not a file stays plain text; at most 16 mentions, 64 KiB / 2,000 lines per file and 128 KiB / 4,000 lines in total (truncated beyond that)

## Help

```bash
rkat help <QUESTION>
rkat help <QUESTION> --prompt <PROMPT> --plan-execution
```

Options: `--prompt`, `--plan-execution`, `-m/--model`, `-p/--provider`,
`--max-tokens`, `-o/--output`, `--json`, `--html`, `--browser`,
`--open-in-browser`, `--html-template`, `--html-template-file`, `-s/--stream`,
`--no-stream`, plus global/realm options.

## Image Generation And Blob Retrieval

There is no direct image-generation CLI command. The default `rkat run`
composite treats image-generation `Inherit` as visible when the image machine,
executor, planner, and blob store are wired. `--allow-tool generate_image` can
narrow that catalog but cannot create missing dependencies. Mob profiles and
in-process runtime-backed builds can explicitly enable the category. Once a
generated result returns a blob id, fetch it with:

```bash
rkat blob get <BLOB-ID> --output fox.png
```

Do not use `rkat sessions show --json`, `rkat rpc blob/get`, or `rkat blob get
-o`.

## Session

```bash
rkat session list [--limit N] [--offset N] [--label KEY=VALUE]   # --limit defaults to 20
rkat session show <ID>
rkat session repair-wholeblob <ID> [--apply [--accept-shorter]] [--runtime-store <FILE>] [--json]
rkat session resolve-held-tail <ID> [--apply] [--runtime-store <FILE>] [--json]
rkat session export-atif <ID> [--output <FILE>]
rkat session delete <ID>
rkat session interrupt <ID>
```

`rkat session show` is human-readable and has no `--json`.

`repair-wholeblob` diagnoses (and with `--apply` re-anchors) a WholeBlob
session whose committed document is refused because its live rows no longer
preserve the audited endpoint; it keeps every message. `--accept-shorter`
allows `--apply` when the live transcript is shorter than the audited
endpoint. `--runtime-store <FILE>` opens that runtime database directly
instead of `<state-root>/<realm>/runtime.sqlite3`; the id must then be a full
session UUID.

`resolve-held-tail` reports why durable-tail recovery holds a session's tail
as ambiguous, or holds an interrupted tail beside inputs another run left
staged or queued (a shape only pre-release builds of 0.9.5, or a store fault,
leave), and what the repair keeps, answers, ends and requeues; with `--apply`
it commits a repair the runtime machine authorizes as an interrupted run
(every row kept, the open calls answered as outcome unknown, the stranded
inputs requeued). Stop every host of the session first: the repair is refused
while the persisted lifecycle row is not quiescent, and an idle live host is
not detected. A row a host left `Stopped`
when it ended before unregistering (no current run, no unregister in progress)
counts as the `Idle` row a start would normalize it to: the report says so, and
`--apply` writes it with the repair. It exits non-zero when the
repair is refused or the tail is held for another reason, and never retries.
Without `--runtime-store` it opens the realm's runtime database (a SQLite
realm's session database, a JSONL realm's `runtime.sqlite3`); with it, the id
must be a full session UUID.

## Blob

```bash
rkat blob get <BLOB-ID> [--output <FILE>] [--json]
```

`--output` writes raw bytes to a file. `--json` prints the blob payload as JSON.
`blob get` requires an existing blob id; it does not discover blob ids.

## MCP

```bash
rkat mcp add <NAME> [-t|--transport stdio|http|sse] [--scope project|user|local|realm] [--expected-generation <N>] [-H|--header KEY:VALUE...] [-e|--env KEY=VALUE...] [--oauth-account <ACCOUNT_ID> | --oauth-account-selection discover|unverified] [--allow-operation <OPERATION>...] [-u|--url <URL> | <URL> | -- <CMD...>]
rkat mcp login <NAME> [--scope project|user|local|realm] [--allow-headless]
rkat mcp remove <NAME> [--scope project|user|local|realm] [--expected-generation <N>]
rkat mcp list [--scope project|user|local|realm] [--json]
rkat mcp get <NAME> [--scope project|user|local|realm] [--json]
```

`add` writes to `--scope project` by default; `local` is an alias for `project`.
Omit `--scope` on `list`/`get` to search all scopes. Do not pass `--scope all`.
`--scope realm` edits only `[[tools.mcp_servers]]` of the realm selected by
`--realm` (inherited by its child realms); `--expected-generation` applies to
`--scope realm` only. A realm server and an `mcp.toml` server with the same name
must be defined identically, or `rkat run` fails with a conflict naming both.
Realm servers are literal: `${VAR}` expands only in `mcp.toml`, and
`--scope realm` refuses it.
Live mutation of an already-running session uses RPC/REST/MCP/SDK surfaces, not
`rkat mcp reload`.

`get` text output masks env and header values whose name contains key,
secret, token or password; `--json` output is unmasked. Neither shows
`access`, `tool_names` or `secret_env`.

`--allow-operation <OPERATION>` (repeatable) writes the server's access ceiling
(`access = { allowed_operations = [...] }`): every other raw MCP operation is
refused and not offered. Sessions started after the write enforce it.
`rkat mcp add` has no secret flag. A realm stdio server's `secret_env` names
are written in the realm config; their values are set only by a Rust host
(`meerkat::set_mcp_service_secret`). `rkat run` launches realm-defined servers
with the stored values and refuses `secret_env` on `mcp.toml` servers.

For streamable HTTP OAuth, endpoints are discovered when connecting; no auth
schema is stored in MCP config. Interactive login needs an account selection on
the server entry, written by `rkat mcp add` (HTTP servers only; the flags
conflict): `--oauth-account <ACCOUNT_ID>` (`oauth_account`, the provider
subject ID the login must prove, not an email or display name),
`--oauth-account-selection discover` (bind the account the provider verifies
at the first login) or `--oauth-account-selection unverified` (resource-bound
access with no account evidence). `rkat mcp get` shows them; `add` prints the
selection, and for a streamable HTTP server without one or a static
`Authorization` header it names these choices. `rkat mcp login` never sets a
selection: without one it refuses before any network request and prints the
`rkat mcp remove`/`rkat mcp add` commands for each choice; a `discover` or
`--oauth-account` server whose issuer has no OpenID Connect UserInfo is
refused before client registration with nothing stored, naming `unverified`
as the deliberate alternative. Use `rkat mcp login <name>` for explicit
browser login; it refuses to run without a terminal unless `--allow-headless`
is given (the authorize URL may then be printed to stderr). `rkat run`
defaults to stored MCP OAuth tokens; use `--mcp-auth interactive` to allow
first-use browser auth and reconnect in a TTY run (any account selection).
MCP OAuth credentials are per realm, so each realm needs its own login.

## Realm

```bash
rkat realm current
rkat realm list
rkat realm show <REALM_ID>
rkat realm create <REALM_ID> [--backend sqlite|jsonl|memory]
rkat realm govern <REALM_ID>
rkat realm grant <REALM_ID> [--agent <AGENT>] (--controller --route <ROUTE_KEY>... | --catalog | --mcp <SERVER>=<OPS>) [--expires-in-secs <N>]
rkat realm revoke-grant <REALM_ID> [--agent <AGENT>] (--controller | --catalog | --mcp <SERVER> | --worker)
rkat realm delete <REALM_ID> [--force]
rkat realm prune [--isolated-only] [--older-than-hours N] [--force]   # default 24 hours
```

`memory` is available only when compiled and is explicitly ephemeral; it is
not a durable alternative to SQLite or JSONL.

Governed realms: `realm govern` durably selects governed posture for an
existing realm and provisions its grant owner with the local user as operator;
every later `rkat` process opens it governed (no flag selects it). `realm grant`
issues one grant to an agent (default `rkat-agent`) for exactly one target and
selects it: `--controller` (one or more `--route <key>`, each a
`model-route/v1/<sha256 hex>` key; no expiry), `--catalog` (read the session's
tool catalog), or `--mcp <server>=<op>[,<op>...]` / `<server>=*`. A new grant
supersedes only that target's previous one. `realm revoke-grant` revokes one
target (`--worker` is an older selection's single worker grant). A governed
prompt refused for its controller route names the exact
`rkat realm grant <realm> --agent <agent> --controller --route <key>` to run.
A prompt needs a controller grant. To learn the route key, grant a placeholder
(`--controller --route model-route/v1/` plus 64 zeros) and run a prompt; the
refusal names the real key. `realm govern` works only on Unix and only on an
existing realm whose backend is not `memory`; governance cannot be removed.
There is no CLI command that lists grants.

A realm is a config + state namespace. The reserved realm slug `global` is an
optional chain root whose config is the single HOME-rooted doc at
`~/.rkat/config.toml`. A realm config doc may declare a parent in its own
section:

```toml
[realm.team]
parent = "global"
# ... default_binding, [realm.team.backend.*], [realm.team.binding.*], etc.
```

Resolution walks the head realm to its parent chain and, when configured, the
optional `global` tail (depth-capped, fail-closed; no flat sibling scan, no literal `default`
realm). CONFIG inherits down the chain: models, mcp servers, hooks, skills,
limits, and auth bindings. STATE never inherits: sessions, SQLite/JSONL stores,
and event stores are realm-LOCAL. Children may ADD or OVERRIDE inherited
mcp entries and ADD hook entries, but cannot REMOVE either (no tombstones); a
child hook entry reusing an inherited hook id is ignored (the first, most
ancestral entry wins). `rkat config get`/`set`
operate on the RAW head realm doc (read/write split), so an inherited entry is
never flattened into a child doc (defaults can be; see Config); a host/RPC/REST
credential write to an inherited binding is rejected as strict-owner.

## Live channels

There is no root `rkat live` CLI group. Ordinary session live channels are controlled through JSON-RPC
`live/open`, `live/status`, `live/close`, `live/send_input`,
`live/commit_input`, `live/interrupt`, `live/truncate`,
`live/playback_complete`, `live/refresh`, `live/media_health`, and
`live/webrtc/answer`. Start `rkat-rpc` with `--live-ws <ADDR>` for the
WebSocket transport; WebRTC signaling needs an `rkat-rpc` built with the
`live-webrtc` feature (not in the default build) and started with
`--live-webrtc`. Mob members, local or placed, have a separate identity-addressed
surface under `rkat mob live` (WebSocket only), listed below.

## Mob

Direct `rkat mob` covers helpers, artifacts, callable run resources,
multi-host placement and grants, member history, and member live control (local or placed).
Lifecycle creation, full member CRUD, profile management, and ordinary wiring
are available through agent `mob_*` tools or RPC `mob/*`.

```bash
rkat mob spawn-helper <mob_id> <prompt> --agent-identity <id> --result-label <label> --max-text-bytes <n> [--profile <profile>] [--model <model>] [--auth-binding <REALM:BINDING[:PROFILE]>] [--json]
rkat mob fork-helper <mob_id> <source_member> <prompt> --agent-identity <id> --result-label <label> --max-text-bytes <n> [--profile <profile>] [--model <model>] [--auth-binding <REALM:BINDING[:PROFILE]>] [--fork-context full-history|last-messages] [--last-messages N] [--json]
rkat mob member-status <mob_id> <agent_identity> [--json]
rkat mob member-history <mob_id> <agent_identity> [--from-index N] [--limit N]
rkat mob force-cancel <mob_id> <agent_identity>
rkat mob respawn <mob_id> <agent_identity> [--initial-message <MSG>]
rkat mob wait-kickoff <mob_id> [--member <agent_identity>...] [--timeout-ms N] [--json]
rkat mob grant <mob_id> <principal> --scope <scope>... [--expires-at-ms N | --expires-in DURATION]
rkat mob revoke-grant <mob_id> <principal> [--scope <scope>...]
rkat mob grants <mob_id> [--json]
rkat mob host [--listen-tcp <ADDR>] [--advertise-tcp tcp://<HOST>:<PORT>] [--live-ws <ADDR> --live-ws-advertise <URL>] [--identity-dir <DIR>] [--descriptor-out <FILE>] [--allow-remote] [--pairing-password <SECRET> | --pairing-password-env <ENV> | --pairing-password-file <FILE>]
rkat mob bind-host <mob_id> --descriptor <FILE>
rkat mob revoke-host <mob_id> <host_id>
rkat mob hosts <mob_id> [--json]
rkat mob route-installs <mob_id>
rkat mob live open <mob_id> <agent_identity> [--turning-mode provider-managed|explicit-commit] [--transport websocket|webrtc]
rkat mob live status <mob_id> <agent_identity> [--channel-id <id>]
rkat mob live control <mob_id> <agent_identity> <channel_id> <commit-input|interrupt|refresh|truncate> [--item-id <id> --content-index N --audio-played-ms MS]   # the three flags are required for truncate
rkat mob live close <mob_id> <agent_identity> <channel_id>
rkat mob run <pack_or_mob_id> [--flow <flow_id>] [--param key=value ...] [--prompt <text>] [--detach] [--json] [--trust-policy permissive|strict]
rkat mob runs <mob_id> [--flow <flow_id>] [--json]
rkat mob status <mob_id> <run_id> [--json]
rkat mob logs <mob_id> [--after-cursor N] [--limit N] [--json]
rkat mob attach <mob_id> <run_id> [--json]
rkat mob run-flow <mob_id> --flow <flow_id> [--params <json>] [-s|--stream] [--no-stream]
rkat mob flow-status <mob_id> <run_id>
rkat mob pack <dir> -o <pack> [--sign <key> --signer-id <id>]
rkat mob inspect <pack>
rkat mob validate <pack> [--trust-policy permissive|strict]
rkat mob deploy <pack> <prompt> [-m|--model <model>] [--max-total-tokens N] [-d|--max-duration D] [--max-tool-calls N] [--trust-policy permissive|strict] [--surface cli|rpc]
rkat mob web build <pack> -o <dir> --wasm <PKG_DIR|name_bg.wasm> [--trust-policy permissive|strict]
```
Since 0.9.1 `rkat mob deploy
--max-total-tokens/--max-duration/--max-tool-calls` and a pack's `[budget]`
set each member's `[limits]` (`budget`, `max_duration`, `max_tool_calls`) on
the default `--surface cli` and for `rkat mob run`; a flag wins over the
realm's `[limits]`, which wins over the pack default (0.9.0 recorded them but
did not enforce them). Since 0.9.3 the `rkat mob deploy --surface rpc` host
builds every member and session from the same deploy config, so deploy flags
and pack overlays (budget, models, `max_tokens`, compaction, `--model`) apply
there too, as defaults: an explicit `session/create` or mob spawn parameter
replaces them.

The parser accepts `--transport websocket|webrtc` on `rkat mob live open`, but
the current member-live family rejects WebRTC for both local and placed
members; use `websocket`. Generic JSON-RPC `live/*` can negotiate WebRTC only
for a controller-local session. It cannot proxy a placed member's remote
session through the controller.

`grant`/`revoke-grant` scopes: `list`, `read_history`, `subscribe_events`,
`send_command`, `cancel`, `retire`, `wire_topology`, `live`, `admin_host`,
`admin_grants`; `revoke-grant` without `--scope` revokes the whole grant.
`rkat mob host` uses the workspace-derived (or `--realm`) realm, rejects
`--isolated`, and accepts pairing only on a loopback listener; pairing secrets
must be at least 32 bytes. Typed mob errors exit with their own codes (scope
denied 45, host unavailable 46, stale cursor 47, stale fence 48, stale delivery
scope 56; since 0.9.1 also member reload required 49, supervisor protocol
upgrade required 44, pending or timed-out lifecycle work 11, capability or
delivery scope unavailable 40, runtime unavailable 52); an error with a typed
console detail prints one `detail: {...}` JSON line on stderr; other failures
exit 1.

Packing with `--sign` embeds the signer identity and public key but does not
install that signer in a trust store. For a locally signed pack whose signer is
not installed, use the explicit permissive posture (signature verification
still runs and reports an unknown-signer warning):

```bash
rkat mob validate ./dist/release-triage.mobpack --trust-policy permissive
rkat mob run ./dist/release-triage.mobpack --flow main --trust-policy permissive
rkat mob web build ./dist/release-triage.mobpack -o ./dist/release-triage-web --wasm <PKG_DIR|name_bg.wasm> --trust-policy permissive
```

For `rkat run` prompts that need `mob_*` tools, use `--tools full` or config
that enables mob tools.

## Skill

```bash
rkat skill add <PATH> [--name <NAME>]
rkat skill remove <NAME_OR_SOURCE_UUID_OR_PATH>
rkat skill get <NAME_OR_SOURCE_UUID_OR_PATH> [--json]
rkat skill list [--json]
rkat skill inspect <SKILL_NAME> --source-uuid <SOURCE_UUID> [--json]
```

`skill add` takes a filesystem path, not an arbitrary skill id.
Despite the run help label `PATH_OR_ID`, `rkat run --skill` currently accepts
only a bare embedded-builtin skill slug. It does not preload a filesystem path
or a source registered by `skill add`; use typed RPC/SDK preload with the
source UUID for those skills.

## Config

```bash
rkat config get [--format toml|json] [--with-generation]
rkat config set [FILE] [--json <JSON> | --toml <TOML>] [--expected-generation <N>]
rkat config patch [FILE | --json <JSON>] [--expected-generation <N>]
```

`set <FILE>` imports a file into the active realm config. Raw JSON merge patch
uses `rkat config patch --json '{...}'`. `config get`/`set`/`patch` use the RAW
head realm doc (read/write split): they show and write only the head realm's own
fields, never the chain-composed effective config — so a read-modify-write cannot
flatten inherited mcp/hook/skill/binding entries into the head doc. Inherited
config is composed only on the agent-build path.

Defaults are another matter: `config get` renders the head doc as a full typed
config with every default filled in, and `config set` is a full replace (a
`.json` FILE is read as JSON, any other FILE as TOML), so a `get` then `set`
round trip writes those defaults as the head realm's own overrides and stops
inheriting them, including capabilities a parent disabled. Use `config patch`
(RFC 7396: only the named keys change; `null` removes a key so it inherits
again) for partial edits. `config set` refuses a wrapped `{config, generation}`
envelope such as `config get --format json --with-generation` output; pass the
inner config and use `--expected-generation`. `set` and `patch` print
`generation=<N>`.

## Models And Capabilities

```bash
rkat models
rkat capabilities
```

Both commands print pretty JSON by default. Neither accepts `--json` or
`--format`.

`rkat models` prints the effective runtime model registry for the active realm:
built-in catalog entries plus configured custom `[models.<id>]` models and
`self_hosted` aliases.

## Doctor

```bash
rkat doctor
```

No doctor-specific flags. Output is tab-delimited `ok|warn <area> <message>`.
A missing realm config, an MCP config load error, or a self-hosted server that
fails credential resolution or its `/models` probe exits 1; missing provider
env vars, unlisted self-hosted aliases, and missing `wasm-pack` are warnings.
Storage diagnosis is the separate `rkat storage doctor`.

## Storage

Offline storage administration. All three verbs dispatch BEFORE runtime-scope
resolution: they take no realm leases and create no realm — so they run
against split-brain realms the normal resolver refuses. Doctor, prune, and
migrate's default dry run open no persistence bundle; `migrate --apply` does
open each swept realm's bundle (under the exclusive maintenance fence) to
run structural store migrations and stamp ledgers.
`--isolated` and `--default-model` are usage errors with `rkat storage`; scope
the sweep with `--realm`/`--root` instead.

```bash
rkat storage doctor [--json] [--root <PATH>]...
rkat storage migrate [--apply] [--bridge-pre-0-8-10] [--json] [--root <PATH>]... [--adopt-root <PATH>] [--fence-wait-secs <SECS>]
rkat storage prune [--apply] [--older-than-days <DAYS>] [--json] [--root <PATH>]...
```

Sweep roots (all three verbs): by default the invocation directory's
context-local candidate (`<context-root>/.rkat/realms`) and the user-global
data root. Note the runtime resolver additionally walks UP from the
invocation context to the project root before forming its local candidate —
run storage verbs from the project root (or pass `--root`) to sweep the
same location the runtime resolves. When any explicit root is
given (the global `--state-root` or one or more repeatable `--root`), ONLY
those roots are read. The global `-r/--realm` restricts the sweep to one realm
id (split-brain twins for it are still detected across all swept roots).

`storage doctor` is read-only and safe against live realms: takes no leases,
never changes logical database content, and never creates a database or
directory. SQLite may create or update the `-wal`/`-shm` sidecars required for
a coherent WAL read. Reports per-root realm inventory,
manifest state, schema-ledger versions per database, dual-root split-brain
twins, persisted-session structural decode failures, transcript-history
footprint, dangling session→blob references, and orphaned
lock/lease/backup artifacts.
Exit 0 = no error-severity findings; exit 1 = errors found.

`storage migrate` is the offline migration framework, dry-run by default;
`--apply` runs the fenced migration. Per realm: (1) ledger baseline — under
the realm's exclusive maintenance fence (`--fence-wait-secs`, default 10,
bounds the in-flight-operation drain wait), supported realm stores open through
their normal constructors so guarded schema-ledger migrations converge supported
schemas; (2) state-root adoption — report-only, realms are used where they
lie; (3) split-brain reconciliation — a realm id under 2+ swept roots is a
fail-closed refusal unless `--apply --adopt-root <PATH>` names the swept root
to keep, in which case every other copy is archived read-only under the
registered `*.pre-<version>-<timestamp>` backup naming (no merging, no
synthesis); (4) deprecated leftovers — report-only. Per-mob databases under
`mobs/` are report-only in this command; their owning store converges supported
schemas on its next open.

The ordinary-open compatibility floor introduced in 0.8.11 remains Meerkat
0.8.10. For exact supported pre-floor SQLite schemas, the explicit offline
recovery route is `rkat storage migrate --apply --bridge-pre-0-8-10`. It runs
before normal migration under the same exclusive maintenance fence and
authenticates each domain against its owning migration manifest. It is not an
arbitrary-vintage importer: unknown, ambiguous, malformed, future, or unsupported
state is refused, not stamped into legitimacy. Read the per-domain report:
legacy records can be refused, and a later domain failure can leave earlier
domains migrated. Ordinary opens never invoke this bridge; other old state
requires a compatible older-binary migration before repinning. There is no
separate `rkat session migrate` compatibility command.
`--bridge-pre-0-8-10` requires `--apply` and a realm pinned to `sqlite`.
`--adopt-root` without `--apply` is refused (exit 1). Credential stores are never
read, moved, or reported. Exit 1 = errors or fail-closed refusals (including
split-brain without `--adopt-root` and an unacquirable maintenance fence).

`storage prune` manages registered maintenance artifacts ONLY:
`*.pre-<version>-<timestamp>` migration backups (files or archived realm
directories) and `*.corrupt-<timestamp>` index quarantines. Dry-run by default
(lists artifacts with sizes and ages); `--apply` deletes those at least
`--older-than-days` old (default 30; 0 = all). Age comes from the timestamp
embedded in the registered artifact name, not filesystem mtime. Nothing
outside the registered naming patterns is ever touched. Unrelated to
`rkat realm prune`, which deletes live realms.

## Auth

```bash
rkat auth realms
rkat auth profiles
rkat auth profile <PROFILE_ID>
rkat auth profile-delete <PROFILE_ID> [-y|--yes]
rkat auth bindings
rkat auth test <BINDING_ID>
rkat auth status <PROFILE_ID>
rkat auth login [PROVIDER] [--backend <BACKEND>] [--method <METHOD>]
rkat auth login <PROVIDER> --non-interactive [--secret <SECRET>] [--backend <BACKEND>] [--method <METHOD>]
rkat auth logout [<REALM>:]<BINDING_ID>
rkat auth refresh <PROFILE_ID>
rkat auth connector-client set <ID> --issuer <ISSUER> --client <CLIENT_ID> [--client-secret [--secret-stdin]] [--authorize-param NAME=VALUE...] [--no-resource-indicator] [--protocol <PROTOCOL>] [--redirect-uri <HTTPS_URI>...]
rkat auth connector-client rotate-secret <ID>
rkat auth connector-client list
rkat auth connector-client remove <ID>
```

Interactive `rkat auth login <provider>` provisions the reserved `global` realm in the
HOME-rooted doc (`~/.rkat/config.toml`), so one sign-in is inherited by every
workspace realm via the chain tail (cross-workspace). Interactive provider OAuth writes
`global:anthropic_oauth`, `global:openai_oauth`, or `global:google_oauth`.
With the default-enabled `copilot` feature, `rkat auth login copilot` runs
GitHub device OAuth (`github_copilot_oauth`) and provisions
`global:copilot_openai`, `global:copilot_anthropic`, and
`global:copilot_gemini`. These routes share the `github_copilot` credential
account and use provider-specific Copilot backends; Copilot is not a fourth
LLM `--provider` value. Reduced builds without `copilot` cannot use this login.
Non-interactive api-key login (`--non-interactive` requires the provider and
reads the secret from stdin when `--secret` is omitted) only stores the token
under `global:default_<provider>`; it writes no backend/auth/binding config, so
using it needs a matching `managed_store` binding. Interactive login ignores
`--backend`/`--method`. Legacy logins
persisted under `dev`; on the run path those credentials and the `[realm.global]`
binding section migrate to `global` once (idempotent, no-clobber), so an existing
sign-in keeps working without re-login. There is no `dev` default anymore.

`rkat auth realms` lists every configured realm section. `status` and
`refresh` take a profile under the selected realm, while `test` takes a binding
under that realm (AuthMachine leases remain binding-scoped). Logout resolves the
positional `[<realm>:]<binding>` through the loaded config; a bare binding means
`global`, and `--realm` does not retarget it. Logging out `child:<binding>` for
an inherited binding clears the owning realm's stored credential (possibly an
account shared by other routes), so address the true owner. Credential READS inherit down the chain (a child realm resolves
a binding defined in a parent or in `global`); a resolved binding's reported
owning realm is the realm that DEFINES it, not the requesting realm. Host and
RPC/REST credential WRITES are strict-owner and reject child-addressed
inherited writes. Config `get`/`set`/`patch` instead operate on the selected raw head
doc and never flatten inherited entries into it automatically.

`connector-client` manages connector OAuth client profiles in the own config
of the `--realm` realm (default `global`); `list` shows the realm's effective
clients. A client secret is read from stdin (piped, or typed without echo),
never from the command line, and is kept in the credential store, never in the
config. `--client-secret` sends it as `client_secret_post`; without
`--secret-stdin` the stored secret is kept and must already exist.

Runtime env fallback order:

- Anthropic: `RKAT_ANTHROPIC_API_KEY`, `ANTHROPIC_API_KEY`
- OpenAI: `RKAT_OPENAI_API_KEY`, `OPENAI_API_KEY`
- Gemini: `RKAT_GEMINI_API_KEY`, `GEMINI_API_KEY`, `RKAT_GOOGLE_API_KEY`, `GOOGLE_API_KEY`
- Azure OpenAI: `RKAT_AZURE_OPENAI_API_KEY` + `RKAT_AZURE_OPENAI_ENDPOINT`, then `AZURE_OPENAI_API_KEY` + `AZURE_OPENAI_ENDPOINT`; the plain `AZURE_OPENAI_*` pair applies only when no OpenAI key is set, while an `RKAT_AZURE_OPENAI_*` value overrides an OpenAI key

## WorkGraph

`rkat workgraph` is operator lookup plus narrow goal/attention control for the
active realm. Agents mutate ordinary WorkGraph items through `workgraph_*` tools.

```bash
rkat workgraph list [--namespace <NS>] [--all-namespaces] [--status <STATUS>] [--label <LABEL>] [--include-terminal] [--limit <N>] [--json]
rkat workgraph show <ID> [--namespace <NS>] [--json]
rkat workgraph ready [--namespace <NS>] [--label <LABEL>] [--limit <N>] [--json]
rkat workgraph snapshot [--namespace <NS>] [--all-namespaces] [--status <STATUS>] [--label <LABEL>] [--include-terminal] [--limit <N>] [--json]
rkat workgraph events [--namespace <NS>] [--all-namespaces] [--after-seq <N>] [--limit <N>] [--json]
rkat workgraph goal-create <SESSION_ID> <TITLE> [--namespace <NS>] [--description <TEXT>] [--mode pursue|coordinate|review|falsify|judge|observe] [--completion-policy self-attest|host-confirmed] [--json]
rkat workgraph goal-status <BINDING_ID> [--namespace <NS>] [--json]
rkat workgraph goal-confirm <BINDING_ID> --expected-revision <N> --kind <KIND> --id <ID> [--namespace <NS>] [--label <TEXT>] [--summary <TEXT>] [--json]
rkat workgraph goal-close <BINDING_ID> --expected-revision <N> [--namespace <NS>] [--status completed|cancelled|failed] [--json]
rkat workgraph attention-list [--namespace <NS>] [--status active|paused|stopped|superseded] [--json]
rkat workgraph attention-pause <BINDING_ID> --expected-revision <N> [--namespace <NS>] [--json]
rkat workgraph attention-resume <BINDING_ID> --expected-revision <N> [--namespace <NS>] [--json]
```

`--status` on `list`/`snapshot` is repeatable: `open`, `in-progress`,
`blocked`, `completed`, `cancelled`, `failed`. `--label` is repeatable.

The flags above are parsed, but the current CLI binds WorkGraph to the active
realm's immutable `default` namespace grant. Non-default `--namespace` requests
and `--all-namespaces` scans are rejected; flags do not grant cross-namespace
authority. Broader access requires separate host capability composition.
