Covers: sandboxing (OS process confinement: macOS Seatbelt, Linux namespaces
and seccomp, no Windows backend; the `[confinement]` config section and
`--confinement` flags of the stock binaries, and the AgentFactory builders)
and what is not a sandbox, shell command policy, tool access policy and read-only, where
credentials and secrets live (file store, OS keychain, MCP `secret_env`), MCP
security boundaries, audit, and the security errors and refusals reference.
Governed realms, grants, review and approvals: references/security_governance.md

# Security in Meerkat 0.9

How Meerkat 0.9 limits what an agent can do and where it keeps credentials.
Governed realms and grants, operation review (R1/R2/R3), human consent and
approvals, and native authorization for Rust hosts are in
`references/security_governance.md`. Provider login (API keys, OAuth,
bindings) is covered by the auth material.

## Security model at a glance

Each layer answers a different question. None of them substitutes for another.

| Layer | Question it answers | How it is enabled |
| --- | --- | --- |
| Provider credentials (auth bindings) | Which account pays for and serves model calls? | Realm config and `rkat auth login`; grants no tool permission |
| Tool selection | Which tools exist for this session? | `rkat run --tools` (`safe`, `workspace`, `full`, `none`), profiles, tool policy |
| Shell command policy | Which command lines may the shell tool run? | `[shell] security_mode` and `security_patterns` |
| OS process confinement (preview) | Which files, network and sockets may a launched process touch? | `[confinement]` config section or `--confinement` flags; Rust `AgentFactory` builders; off by default |
| Governed realm grants (preview) | May this agent perform this exact operation? | `rkat realm govern` + `rkat realm grant` |
| Operation review (preview) | Must a model (R2) or a human (R3) review it first? | `[[governance.review.rules]]` in a governed realm |
| Approvals | Audit records of approval decisions | `approval/*` JSON-RPC on `rkat-rpc` |
| Secret storage | Where tokens and service secrets live | Credential store; `secret_env` for MCP stdio servers |

Grants, review and approvals are detailed in
`references/security_governance.md`; the other layers are covered below.

Principles that hold everywhere:

- A credential never grants permission. An OAuth token that can delete does not
  widen an agent's grant.
- Confinement is not authorization, and authorization is not confinement.
  Installing grant owners does not confine processes, and a confinement
  requirement grants nothing.
- An approval is not execution. A recorded decision executes nothing; a later
  explicit attempt rechecks permission and spends consent once.
- Refusals are local. A refused operation returns typed tool feedback to the
  model; permitted sibling calls and the rest of the run continue.
- Nothing falls back. A refused confined launch is never retried unconfined, a
  governed realm never opens as ordinary, and unavailable review never runs at R1.

Stock defaults: `rkat run` uses `--tools safe` (no shell); realms are
ungoverned until `rkat realm govern`; launches run as the trusted host
(no OS confinement) unless `[confinement]`, `--confinement` or a Rust host
installs a requirement.

Other protections in 0.8.x-0.9:

- Agent-facing mob tools (`mob_create`, `mob_profile_create`/`update`,
  `mob_spawn_member`, `delegate`) refuse host-only fields from model arguments
  (`mcp_servers`, `rust_bundles`, host skill paths, blob image references,
  video URIs) with `InvalidArguments` (0.8.51).
- On a host with a tool consequence policy registry, child mobs created by
  agents run under the host's child application tool policy or are refused
  (`child_tool_policy_required`) (0.8.51).
- Realm MCP server definitions are literal: `${VAR}` is never expanded and a
  write containing `${` is refused (`RealmMcpServerEnvReference`); use
  `mcp.toml` for env-derived values (0.8.52).
- Use `config patch` (RFC 7396, writes only the keys named) for partial edits.
  `config set` replaces the whole document, so in a child realm it stops
  inheritance: it drops restrictions a parent set (for example
  `tools.max_concurrent`) and can re-enable capabilities a parent disabled
  (`tools.schedule_enabled`, provider web search, `skills.enabled`) (0.8.52).
- `rkat-rpc` and `rkat-mcp` refuse to start on a head realm config that does
  not load, instead of running on defaults (0.8.34).

## OS process confinement (sandboxing)

Preview. Crates `meerkat-sandbox` (OS backends) and `meerkat-core`
(`meerkat_core::confinement`). Confinement mechanically limits a process that
is about to be launched: which paths it can read or write, whether it can open
IP sockets, which Unix sockets it can connect to. It does not authenticate
anyone and does not decide whether the launch is permitted.

### What is not a sandbox

Without confinement, every shell command, command hook and MCP stdio server
runs with the OS user's full file, network and process access. None of these
is OS confinement:

- `rkat run --tools none` (or `safe`): picks which tool families exist; `none`
  turns off builtin, shell, memory, WorkGraph and mob tools. Whatever does run
  is not confined.
- Realms and `--isolated`: a realm isolates state (sessions, stores, event
  logs, config, credentials) and `--isolated` starts in a new generated realm.
  Neither limits what a process can reach.
- The shell tool's `restrict_to_project` (a `ShellConfig` field, `true` by
  default, no config key): a `working_dir` that resolves outside the project
  root is refused (`WorkingDirEscape`). It checks where a command starts, not
  which paths the command then opens.
- `[shell]` security patterns, tool access policies and read-only intent:
  checks on the command text or the tool call, made before anything runs.
- Governed realm grants, review and consent: authorization, not confinement.

### Who can turn it on

The host turns it on; no session, turn, RPC method or SDK option can. The
stock `rkat`, `rkat-rpc`, `rkat-rest` and `rkat-mcp` binaries read the
`[confinement]` section of the realm's config documents and accept
`--confinement <trusted-host|workspace>`, `--confinement-network
<denied|unrestricted>` and the repeatable `--confinement-read <path>` and
`--confinement-write <path>`, which replace the section for every launch kind
of the process:

```toml
[confinement.all]
mode = "workspace"        # trusted_host (default) | workspace | custom
network = "denied"        # or "unrestricted"
write = ["/tmp/build"]    # extra writable subtrees; read = [...] for read-only

[confinement.mcp_stdio]   # a kind's own table replaces `all` in this document
mode = "custom"
working_directory = "/srv/mcp"
requirement = { baseline = "command_runtime_v1", read = { kind = "paths", paths = [{ kind = "subtree", path = "/srv/mcp" }] }, write = { kind = "paths", paths = [] }, deny_read = [], deny_write = [], network = { kind = "denied" }, unix_connect = [], require_descendant_termination = false }
```

- `workspace` grants the workspace root (the CLI's project root, a server's
  `--context-root`) and the listed subtrees, the `command_runtime_v1`
  baseline, and IP sockets only with `network = "unrestricted"`; a server
  without `--context-root` refuses to start with it.
- From the user document through the realm chain, a requirement only holds
  or is added: a less trusted document that states a required kind
  differently is refused (`ConfinementLayerConflict`).
- An ordinary config write cannot add, change or remove the section
  (`confinement_administration_required`), and, as for `[governance]`
  (`governance_administration_required`), it cannot change the realm
  sections of the realm's own document (its `[realm.<id>].parent` edge, or a
  `[realm.global]` section that puts the user document on its chain) so that
  the realm would compose a different `[confinement]` or `[governance]`. A
  write to the user document is refused too when another realm would
  compose differently: a realm whose own documents state no
  `[realm.global]` (the user document's puts `global` on its chain), or one
  without a document that takes the user document's section for it. The
  refusal names the change and the realm. Only stores that declare the
  realm chain enforce this: the stock hosts'
  stores, `FileConfigStore::with_realm_chain` and
  `SdkConfigStore::with_realm_document`; a `FileConfigStore::new`,
  `SdkConfigStore::with_path` or in-memory store keeps only the document's
  own sections. Such writes over one state root take turns: each holds an
  exclusive lock on `<state_root>/.realm-config.lock` from before its read
  to after its write (`RealmConfigSource::composition_lock_path`; a source
  that names none leaves them unserialized). `rkat realm create`, `delete`
  and `prune` do not take it (a delete racing a write that names the realm
  as a parent leaves that realm's chain unresolved, so it does not
  compose), and an NFS mount that keeps locks on each machine (`nolock`,
  `local_lock=flock`) does not carry it between machines. Each ancestor composes from
  its own document alone, so a
  chain through a missing ancestor fails (a missing parent), and `rkat realm
  delete` and `rkat realm prune` keep a realm another realm names as its
  parent.
- A host that confines every kind keeps its own inputs out of reach: a start
  in which a confined process could change the realm state, the user config
  document or the file credential store (`<config dir>/meerkat/credentials`;
  writing it, creating it, replacing a directory on the way, or through a
  symbolic link in it) is refused, naming the input and the remedy. The
  macOS workspace preset protects `<root>/.rkat`; on Linux the state must
  live outside the workspace (`--state-root`). A convention source a
  confined process can write is not trusted: an `mcp.toml` (its MCP servers
  are refused before they are expanded, authorized, connected or spawned;
  the model sees each as that server's failed setup with
  `untrusted_configuration_source`, under its name when that is a plain
  server name, else `refused-mcp-server-<n>` (an untrusted `mcp.toml` that
  does not parse, or states an invalid access policy, still fails the
  setup); an MCP auth request naming one
  is refused as `mcp_server_not_configured`), a project `.rkat/config.toml`
  (its hooks are refused at every invocation), and a skills directory (the
  project `.rkat/skills`, a filesystem repository or the Git skill cache: its
  skills are not loaded, nor is a skill linked to a path a confined process
  can write). A host that runs any kind as the trusted host claims none of
  this.
- At startup each host reports each launch kind's requested and effective
  confinement, what keeps its own inputs from its confined processes, and
  each untrusted convention source (`rkat` on stderr, the servers in their
  log). A server reads its working directory only to resolve a relative
  `--confinement-read` or `--confinement-write` path.

A Rust embedder resolves a selection with `meerkat::HostConfinement` and
installs it with `AgentFactory::with_host_confinement`, or selects each kind
on `AgentFactory`:

| Builder | Confines | Default |
| --- | --- | --- |
| `with_shell_confinement(ShellConfinement)` | shell tool: foreground calls, background jobs, monitors | `ShellConfinement::TrustedHost` |
| `with_command_hook_confinement(ExecutionConfinement, working_directory)` | command hooks (absolute `working_directory`, else `invalid_launch`) | unset (trusted host) |
| `with_mcp_stdio_launch_profile(McpStdioLaunchProfile)` | local MCP stdio servers | `McpStdioLaunchProfile::trusted_host()` |

`with_host_confinement` also confines stdio skill sources under the
`mcp_stdio` requirement (from the kind's working directory, with only their
configured environment and an absolute program) and hands the host's
`HostConfinement::untrusted_paths` to skill resolution and credential
resolution. `McpRouter::new`, `new_with_surface_handle`,
`new_with_removal_timeout`, `new_with_surface_handle_and_removal_timeout`
and `meerkat::mcp::standalone_router` are deprecated (0.9.2): they launch
stdio servers as the trusted host without naming it. Use
`McpRouter::new_with_stdio_profile`,
`new_with_surface_handle_and_stdio_profile` or
`meerkat::mcp::standalone_router_with_stdio_profile` with the host's profile
(`AgentFactory::mcp_stdio_launch_profile`) or
`McpStdioLaunchProfile::trusted_host()`.

`ShellConfinement` is `meerkat_tools::builtin::shell::ShellConfinement`
(`TrustedHost` or `Required { requirement }`); `McpStdioLaunchProfile` is in
`meerkat_mcp` (`trusted_host()` or `required(requirement, working_directory)`,
plus `capabilities()` to check backend support before admitting a deployment).
The requirement is compiled once per factory build and retained; a session
request, shell argument or recovered job cannot select a weaker one.
`with_command_hook_confinement` cannot be combined with `hook_engine_override`.
Remote (HTTP/SSE) MCP servers, builtin tools that run inside the host process
(file, patch and task tools), in-process Rust tools and provider-hosted tools
are not launched processes and are never confined; for remote MCP see the HTTP
egress profile below.

### The requirement

`ConfinementSpec` (validated into `ExecutionConfinement`; unknown fields are
refused):

| Field | Values |
| --- | --- |
| `baseline` | `command_runtime_v1`: loaders, system libraries, CPU/OS queries, `/dev/null`, `/dev/random`, `/dev/urandom`. No home, scratch, network or credentials. |
| `read`, `write` | `{"kind":"unrestricted"}` or `{"kind":"paths","paths":[...]}`; an empty list grants nothing |
| path entries | `{"kind":"literal","path":"/abs/file"}` or `{"kind":"subtree","path":"/abs/dir"}` |
| `deny_read`, `deny_write` | path entries that override every grant |
| `network` | `{"kind":"denied"}`, `{"kind":"unrestricted"}`, or `{"kind":"connect","endpoints":["127.0.0.1:443"]}`; both 0.9.0 backends refuse a nonempty `connect` list (an empty one equals `denied`) |
| `unix_connect` | exact socket paths or socket directories |
| `require_descendant_termination` | `true` refuses on backends that cannot prove descendants end |

Paths must be absolute with no `.`, `..` or NUL; connect endpoints need a
specific IP and a nonzero port. Otherwise the requirement is refused as
`invalid_requirement`.

```rust
use meerkat_core::confinement::{
    ConfinementSpec, ExecutionConfinement, FilesystemAccess, IpNetworkAccess,
    PathAccess, PlatformBaseline,
};
use meerkat_tools::builtin::shell::ShellConfinement;

let requirement: ExecutionConfinement = ConfinementSpec {
    baseline: PlatformBaseline::CommandRuntimeV1,
    read: FilesystemAccess::Paths(vec![PathAccess::Subtree(work_root.clone())]),
    write: FilesystemAccess::Paths(vec![PathAccess::Subtree(work_root.clone())]),
    deny_read: vec![],
    deny_write: vec![],
    network: IpNetworkAccess::Denied,
    unix_connect: vec![],
    require_descendant_termination: false,
}
.try_into()?;

let factory = AgentFactory::new(sessions_path)
    .project_root(&work_root)
    .shell(true)
    .with_shell_confinement(ShellConfinement::Required { requirement });
```

Under a required profile the launch gets only its explicit environment, never
the host's: the shell gets `PWD` plus its configured `env_vars`, a command hook
its `env`, an MCP stdio server its `env` plus resolved `secret_env` values.
Programs must be given by absolute path where the launch names them directly
(command-hook `command`, MCP stdio `command`). A confined MCP stdio server's
stderr is discarded. Environment names starting `LD_`, `DYLD_`, `_RLD_`,
`__XPC_`, `BASH_FUNC_`, or equal to `ENV`, `BASH_ENV`, `SHELLOPTS`, `BASHOPTS`,
`ZDOTDIR`, `GLIBC_TUNABLES`, are refused (`unsupported_requirement`).

### Launches outside confinement

Every process a stock host starts runs under its launch kind's confinement
except these, which it runs as itself (`meerkat_core::HostLaunchExemption`):

- Git skill source (`HostLaunchExemption::git`): fetches with the host's
  Git credentials and network; a repository is not loaded when a confined
  process could change the skill cache, and a skill linked out of the
  checkout is not read. A repository's HTTPS token reaches git only for the
  repository: on Unix through git's `credential-cache` client and a socket
  the host serves for each invocation (for the repository's host; never on
  a command line or in a file; a git without `credential-cache` fails with
  the remedies instead of prompting), on Windows as a header option on that
  invocation's command line (for the configured URL, redirects not
  followed, never in a file). No credential helper of the host's git
  configuration is asked for or sent it.
- Credential command (auth profile `source = { kind = "command" }`,
  `HostLaunchExemption::credential_command`): produces the host's provider
  credentials; without `cwd` it runs from the user's home directory; it is
  refused before it runs (`ProviderAuthError::HostLaunchRefused`) when a
  confined process could change its program, the interpreter its `#!` line
  names by path, or its working directory, and the refusal ends the
  binding's resolution (no other credential stands in for it). A config
  write cannot add one, change what it runs, or change where its token goes
  (the profile's provider or auth method, a binding of the section that
  uses it, that binding's backend or default model, or the
  `[self_hosted.servers]` entry the backend names)
  (`confinement_administration_required`, naming the auth profile); the
  host edits the document. A command binding on a self-hosted backend must
  declare the server (`server = "<id>"`), so a request's model never
  decides where its token goes: a realm with one that declares none does
  not compose (`ConfigError::CredentialCommandServerUndeclared`), and such
  a binding is never chosen for, or explicitly sent to, a server it does
  not declare (`SelfHostedConnectionError::CredentialCommandServerUndeclared`).
  A request cannot override a binding's auth profile with another binding's
  command profile (`ProviderBindingError::CredentialCommandProfileOverride`),
  and a realm whose command binding takes its server entry from another
  document does not compose (`ConfigError::CredentialCommandServer`).
  Credentials that do not come from a command are outside these rules.
- Browser opener (`HostLaunchExemption::browser_opener`: `/usr/bin/open`,
  `xdg-open`, the system `rundll32.exe`; OAuth logins and the CLI's HTML
  artifact): must reach the user's desktop session; the URL is one argument.
  Embedders open with `meerkat::open_system_browser_guarded(url,
  host_confinement.launch_guard().as_deref())`; the unguarded
  `open_system_browser` and `McpOAuthPendingLogin::launch_system_browser`
  are deprecated (0.9.3).
- `stty` (`HostLaunchExemption::terminal_control`: `/bin/stty`, else
  `/usr/bin/stty`, else the `stty` on the trusted `PATH`, as on NixOS): host
  terminal control (hiding a client secret typed at `rkat`).
- `wasm-pack` (`HostLaunchExemption::wasm_pack`): developer build tooling
  (`rkat doctor`).

Under a host that confines any launch kind, each runs, with every program
it starts by name (an interpreter named through `env`, `ssh` or `git-lfs`
under Git, the programs a browser opener tries), from a `PATH` without
relative directories or the directories a confined process could change
(`meerkat_core::confinement::trusted_search_path`; a host that confines only
some kinds reports no writer for an unconfined kind, so the launch still
gets the deny list and the relative-`PATH` drop), and inherits no variable
that makes a program load code or startup options from elsewhere (`LD_*`,
`DYLD_*`, `PYTHON*`, `NODE_OPTIONS`, `NODE_PATH`, `PERL5LIB`, `PERL5OPT`,
`RUBYLIB`, `RUBYOPT`, `BASH_ENV`, `ENV`, `GIT_DIR`, `GIT_WORK_TREE`,
`GIT_EXEC_PATH`, ...), and every other variable loses the paths a confined
process could change (`trusted_environment`); a credential command's own
`env` loses them too (`apply_configured_environment`). Without
confinement they run with the host's environment as it is
(`HostConfinement::launch_guard` is `None`).

A source test fails on any process launch in shipped code that is neither
confined nor in this list, and Clippy (`disallowed_methods` in the workspace
`clippy.toml`) refuses every process-launch API outside the sites that test
lists.

### Platforms

- macOS: Seatbelt (`MacOsSeatbeltV1`), through `/usr/bin/sandbox-exec`, whose
  file identity is pinned at compile time and rechecked at every launch.
  Default-deny profile. Supports every dimension except exact `connect`
  endpoints and `require_descendant_termination`. Path components that are
  symlinks are refused, except the system aliases `/tmp`, `/var`, `/etc`.
  XPC service lookup is denied.
- Linux (x86_64 and aarch64): `LinuxNamespaceSeccompV1`, usable from 0.9.0
  (0.8.52 refused every launch) and the less qualified backend. Each launch
  gets new user, mount and IPC namespaces, a private mount view containing only
  the granted paths plus `/bin`, `/sbin`, `/usr/bin`, `/usr/sbin`, `/lib`,
  `/lib64`, `/usr/lib`, `/usr/lib64`, `/etc/ld.so.cache` and the three baseline
  devices (no `/proc`, `/sys`, home or scratch), no new privileges, dropped
  capabilities and a seccomp filter (no mount, namespace, ptrace, bpf, kernel
  keyring or signal-sending calls; no Unix-domain sockets other than
  socketpairs; IP sockets only when `network` is `unrestricted`, over the
  host's own network). It supports a narrower set: `read` and `write` must be
  path lists; each writable entry must be a `subtree` also listed in `read`;
  grants must already exist, be canonical (no symlinks), and not overlap each
  other or the baseline directories; `/`, `/proc`, `/sys`, `/dev` cannot be
  granted; `deny_read`, `deny_write`, `unix_connect`, `connect` endpoints and
  `require_descendant_termination` are refused as `unsupported_requirement`.
  The launched program and working directory must be visible in the view (a
  grant or a baseline directory). Compilation runs a probe; a kernel that refuses
  unprivileged user namespaces gives `backend_unavailable`. On Ubuntu 24.04
  AppArmor restricts them by default, so the host administrator must allow
  them for confinement to work there.
- Windows and every other platform: no backend. Any required profile is
  refused (`unsupported_requirement`); trusted-host execution is unchanged.
- WASM/browser: no processes and no confinement.

### Refusals

`ConfinementRefusal` values (wire names): `invalid_requirement`,
`invalid_launch`, `unsupported_requirement`, `backend_unavailable`,
`preparation_failed`, `untrusted_configuration_source` (0.9.1: the launch is
defined by a source a confined process can write). A refusal happens before
any target code runs and is never retried unconfined.

- Shell: tool error `{"error":"confinement_refused","message":"...","data":{"refusal":"backend_unavailable"}}`
  for foreground, background and monitor launches. A command that starts and
  then hits a denied write simply fails with its own exit status and stderr.
- Command hooks: event `hook_launch_refused` (with `hook_id`, `point`,
  `reason`); a refused pre-tool hook skips only its tool, with
  `confinement_refused` feedback. A refused mandatory model-request Guardrail
  withholds only that model operation.
- MCP stdio: the add or reload fails; the `tool_config_changed` event's
  external tool delta carries `confinement_refusal`, and the model receives a
  notice that the server could not be started.
- Rust error types: `ToolError::ConfinementRefused` (tool code
  `confinement_refused`; the shell builds it from
  `BuiltinToolError::ConfinementRefused { refusal }`) and
  `McpError::Confinement(ConfinementRefusal)` for a refused MCP stdio launch.

### Tool process custody

Separate from confinement: on Linux and macOS, an `AgentFactory` with a realm
`runtime_root` records shell processes under
`<runtime_root>/tool_process_custody/<session_id>/`, and the next agent build
settles processes an earlier host left behind, failing closed
(`CompositeDispatcherError::ProcessCustody`) if one cannot be proven stopped.

## Shell tool command policy

- `rkat run` exposes no shell by default (`--tools safe`). `--tools workspace`
  adds the shell; `--tools full` (or `--yolo`) adds shell, memory, WorkGraph and
  mob tools.
- `[shell]` keys that matter here: `program` (default `nu`), `security_mode`,
  `security_patterns` and `max_output_chars` (default 40000).

```toml
[shell]
security_mode = "allow_list"        # "unrestricted" (default), "allow_list", "deny_list"
security_patterns = ["git *", "cargo test*", "ls*"]
```

Patterns are globs (where `*` also matches `/`) applied to the parsed,
re-quoted command line. `allow_list` refuses every command that matches no
pattern (an empty list refuses all); `deny_list` refuses matching commands. The
legacy key `allowlist` sets the patterns and, without `security_mode`, selects
`allow_list`. These are text filters on the command line, not isolation: they
do not limit what an allowed program does, and neither does the shell's
`restrict_to_project` working-directory check. For an OS boundary use
confinement.

## Tool access policy and read-only

A tool access policy is a gate in the tool dispatcher, checked on each call.
It is not OS confinement.

| `tool_access_policy` (wire) | Effect |
| --- | --- |
| `{"type":"inherit"}` | a spawned child gets its parent's effective policy; a launch with no parent is unrestricted |
| `{"type":"allow_list","value":["task_list","view_image"]}` | only the named tools |
| `{"type":"deny_list","value":["shell","apply_patch"]}` | every tool except the named ones |
| `{"type":"read_only"}` | only tools whose dispatcher declares `ToolMutationClass::ReadOnly` |

Set it on JSON-RPC `mob/spawn`, `session/fork_at` or `session/fork_replace`
(`tool_access_policy`), on a Rust spawn spec, `HelperOptions` or
`AgentBuildConfig`, or declare it in a mob profile:

```toml
[profiles.reviewer.tools]
comms = true
read_only = true
```

- A profile's `read_only` and `deny = [...]` are a floor that a spawn's policy
  cannot widen, recomputed from the current profile on every build, a resume
  included.
- Read-only intent is name-independent and fails closed: MCP tools, `shell`
  and host tools that declare no mutation class are refused, and tools that
  appear later cannot widen it. A host with its own dispatcher must override
  `AgentToolDispatcher::tool_mutation_class` (the default, `Unknown`, is
  refused). Binaries older than 0.8.23 cannot decode a session launched
  read-only.
- Visibility (since 0.8.52): tools that a deny list, the complement of an
  allow list or a profile `deny` rules out are hidden from the model.
  Conditional gates keep their tools visible and refuse at call time:
  read-only intent (`access_denied`), application tool (consequence) policy
  (`policy_denied`, `policy_indeterminate`) and dispatch admission.
- Provider-native tools such as native web search bypass the dispatcher, so
  the factory turns them off itself (`ProviderNativeToolPolicy::DisableAll`)
  whenever the access policy restricts, an application tool policy governs or
  a dispatch admission is set; you do not disable them yourself. Since 0.8.52
  such a session with an explicit web-search enable gets the ordinary
  `web_search` tool, which the gate checks.
- The spawn and fork wire value (`mob/spawn`, `session/fork_at`,
  `session/fork_replace`) has no `constraints` variant.

## Secrets and credential storage

### Where credentials live

`rkat`, `rkat-rpc` and `rkat-rest` keep credentials in a file store at
`<config dir>/meerkat/credentials/` (Linux `~/.config/meerkat/credentials`,
macOS `~/Library/Application Support/meerkat/credentials`, Windows
`%APPDATA%\meerkat\credentials`): `<realm>/<binding>.json`,
`<realm>/<binding>@<profile>.json`, `<realm>/account~<id>.json`, written
atomically with mode 0600 on Unix, with refresh locks in `refresh-locks/`. The
store holds provider tokens, per-realm MCP OAuth credentials, connector
credentials and connector client secrets, MCP stdio service secrets and each
realm's MCP attachment identity key. None of these are written to config files.

### OS keychain (keyring)

The default store is file-backed on purpose: macOS keychain reads can prompt
during ordinary CLI runs. Rust hosts can opt into the OS keychain with
`TokenStoreBackend::Keyring { service_name }` or `TokenStoreBackend::Auto`
(`TokenStoreBackend::default_keyring_auto()`: service `meerkat-auth`, file
fallback root as above); the facade feature is `native-keyring`. Keychain
accounts are `<realm>:<binding>[:<profile>]`. Auto keeps one authoritative
backend per key and falls back to the file store only when the keychain is
positively absent; a denial or locked keychain is reported, never treated as a
missing credential.

0.9 keychain behavior: keychain calls run off async workers (at most 2 at once
per process, one per credential); status reads never touch the keychain;
unattended work answers a typed pending instead of waiting. Auth errors carry
`error.data.reason` (RPC) or `reason` (REST): `credential_access_denied`,
`credential_interaction_required`, `credential_access_pending`,
`credential_store_unavailable`. SDK helpers: Python `auth_error_reason`,
TypeScript `authErrorReason`.

### MCP stdio service secrets (secret_env)

A stdio MCP server defined in a realm's own `config.toml` can declare which
environment variables are secrets. Only the names are configuration:

```toml
[[tools.mcp_servers]]
name = "search"
command = "/usr/local/bin/search-mcp"
env = { REGION = "eu" }
secret_env = ["SEARCH_API_KEY"]
```

- Names are 1-128 ASCII letters, digits or underscores, not starting with a
  digit, distinct ignoring case, not also in `env`, and not
  `MEERKAT_CONNECTOR_BINDING`.
- Values live in the defining realm's credential store, bound to the server's
  exact definition (name, command, args, literal env, secret names, connector
  approval). Changing the definition orphans the value until it is set again.
- Values are never read from the host environment. At launch the values go to
  that one process only (under required confinement, as explicit
  environment). A missing value, owner or current definition refuses the launch.
- Setting values is Rust-only in 0.9.0: `meerkat::set_mcp_service_secret` and
  `remove_mcp_service_secret` (and `_entered` variants with an administrative
  entry check); no `rkat`, RPC, REST or SDK method sets them.
- `rkat run` launches realm-defined servers with their stored secrets. Servers
  from `mcp.toml`, host-supplied configs or live `mcp/add` that declare
  `secret_env` are refused (`NoOwner` or `NoCustody`).

### Other credential protections

- MCP OAuth credentials are per realm (0.9): log in once per realm with
  `rkat mcp login <name>`; pre-0.9 credentials are left unused.
- Token, refresh and device-code exchanges follow no redirects;
  credential-bearing MCP HTTP and HTTP skill requests follow only same-origin
  redirects.
- Self-hosted model credentials resolve per server, never by provider class.
- `Debug` output and error text redact tokens, env values, header values, URL
  userinfo and query, and token-endpoint bodies (details and limits:
  `references/security_governance.md`).

## MCP security boundaries

- Access ceiling: `access = { allowed_operations = ["read_file"] }` on a server
  (realm config or `mcp.toml`), written by `rkat mcp add <name> --allow-operation <OP>`
  (repeatable). Other raw operations are not published (calling one is
  `tool_not_found`) and the connection refuses them (`access_denied`). An empty
  ceiling is refused. It narrows access only and does not mark operations
  read-only. Sessions started after the write enforce it.
- Account selection (`oauth_account`, `oauth_account_selection`, `account`) and
  connector bindings are host configuration only. The agent-facing
  `meerkat_mcp_add` tool refuses them. On a governed runtime, live adds that
  carry no authenticated attachment control (REST live add, `meerkat_mcp_add`)
  are refused.
- Connector-bound stdio servers start with `MEERKAT_CONNECTOR_BINDING` and get
  a per-call credential in protected `_meta` (`meerkat/connector_credential`).
- HTTP egress profile (Rust, preview):
  `AgentFactory::with_mcp_http_egress_profile(HttpEgressProfile::required(policy))`
  with `HttpEgressPolicy::new().allow_public(origin)` or
  `.allow_local(origin, addresses)`. Every request, OAuth discovery and refresh
  included, must target an allowed origin; DNS answers outside the allowed
  addresses are refused; redirects and ambient proxies are not used. Refusals
  are `McpError::EgressRefused`/`EgressUnavailable`; a refused call is unsent.
  `HttpEgressProfile::trusted_host()` is the default.
- Checked MCP routes (Rust): `McpRouterAdapter::checked_route` exposes exact
  operations of a server under a separate grant (`GrantTarget::CheckedMcpRoute`).
- Governed service calls (Rust): a host runnable's connector-bound MCP calls
  go through `MeerkatMachine::prepare_service_call` under a service original
  the host enrolled. The operator's grant for exactly that MCP server, the
  connector approval it records and the operation owner decide each call,
  Domain-scoped, runless and audited. The occurrence identities are audit
  data, not authority; an ungoverned runtime refuses.

## Audit and security events

- Governed work records protected audit observations (prepared and refused
  decisions, outcomes, withheld mandatory-hook operations) in the work's native
  input row in the runtime store. There is no separate audit log and no CLI or
  RPC method to read it. A failed required observation is
  `operation_observation_unavailable`, never shown as a denial; an optional
  event exporter is not the authoritative record. A missing record after a crash
  does not prove no effect happened.
- Approval records (`approvals.json`) are the audit trail of approval and
  consent decisions, including approver, assurance and consumption.
- Refusal reads (0.9.4): a `Refused` audit record carries an
  `AuditRefusalRead` saying where the refusal was decided. Its `basis` is
  `owner_answer` (the owners answered at one coherent publication, a prepared
  decision's own validity window included), `publication_changed` (the
  publication changed under the attempt and the change completed) or
  `change_in_progress`; it also carries the observation time, the sequence the
  attempt read, the live sequence when the publication changed, and the
  publication instance. A `Prepared` record's policy read names its
  publication instance too, since sequences of different instances do not
  compare. Rows carry both at stored input state version 7; records written
  earlier read with neither.
- Events: `hook_launch_refused`, `hook_denied`, `operation_observation_failed`,
  `tool_config_changed` (with `confinement_refusal`), plus typed tool results.
  Each bounded wait for an owner change in progress is a `debug` event of
  target `meerkat::authz::owner_change_wait` (0.9.4; see
  `references/security_governance.md`).

## Errors and refusals reference

| Code | Where | Meaning |
| --- | --- | --- |
| `confinement_refused` | tool result | launch refused by confinement; `data.refusal` |
| `operation_refused` | tool result; REST 403 `OPERATION_REFUSED` | denied by governed authorization; tool result `data.kind` (`reprepare_required`, `denied`, `malformed_facts`) and `data.tool_invoked` (`false`, or `"unknown"` when no claim is made); neither is a permission, and a new call gets fresh admission |
| `operation_authorization_unavailable` | tool result; REST 503 | authorization owner could not decide |
| `operation_observation_unavailable` | tool result; REST 500 | required audit could not be recorded |
| `review_unsatisfied` / `REVIEW_UNSATISFIED` | tool result; REST 403 | review denied, escalated, stale, consent required, used |
| `review_unavailable` / `REVIEW_UNAVAILABLE` | tool result; REST 503 | review could not complete |
| `hook_denied`, `hook_launch_refused` | tool result and events | hook decision or hook launch refusal |
| `access_denied` | tool result | tool policy or MCP access ceiling |
| `policy_denied`, `policy_indeterminate` | tool result | host application tool policy |
| `outcome_uncertain` | tool result | sent, effect unknown; never replayed |
| `INPUT_REFUSED` -32030 / `INPUT_NOT_READY` -32031 | JSON-RPC | native input refused / not ready (HTTP 403/503; `rkat` exits 50/51 since 0.9.1, 1 before) |
| `APPROVAL_*` -32033..-32035 | JSON-RPC | approval store unavailable, conflict, commit unknown |
| `RESULT_DISCLOSURE_REFUSED` | REST 403; JSON-RPC -32025 `SCOPE_DENIED` with `data.reason` | result of a recovered governed input, until its scoped destination is bound again |
| `governance_administration_required` | config write `data.type` | governance section is host-administered |

Governed denials, review feedback kinds and the approval code table are
explained in `references/security_governance.md`.

## Removed or superseded

- `clear_tokens_and_publish_lifecycle_released` was removed in 0.8.51; use the
  `_coordinated` variants.
- Pre-0.9 label-only controller grants and MCP grants without a connector
  approval admit nothing; reissue them with `rkat realm grant` (see
  `references/security_governance.md`).

## Common questions

- How does sandboxing work in Meerkat? Only OS process confinement is a
  sandbox, and only the host turns it on, per launch kind: the stock binaries
  with the `[confinement]` config section or the `--confinement` flags, and a
  Rust host with
  `with_shell_confinement(ShellConfinement::Required { requirement })`,
  `with_command_hook_confinement(requirement, working_directory)` and
  `with_mcp_stdio_launch_profile(McpStdioLaunchProfile::required(..))`, or
  all of them with `with_host_confinement`. Five host launches stay outside
  it by design (Git skill sources, credential commands, the browser opener,
  `stty`, `wasm-pack`; see Launches outside confinement). macOS
  uses Seatbelt; Linux (usable from 0.9.0) gives each launch new user, mount
  and IPC namespaces plus a seccomp filter; Windows has no backend and refuses.
  The default is the trusted host (no confinement). `--tools none`, realms,
  `--isolated`, the shell's `restrict_to_project`, `[shell]` patterns and
  read-only policy are not sandboxes.
- Can the model still see a tool its policy denies? Not if a deny list, the
  complement of an allow list or a profile `deny` rules it out (hidden since
  0.8.52). Read-only intent, application tool policy and dispatch admission
  keep tools visible and refuse the call.
- Must I disable provider-native web search for a read-only agent? No. The
  factory disables provider-native tools under any restricting access policy,
  application tool policy or dispatch admission; an explicit web-search enable
  then gets the ordinary `web_search` tool.
- How do I restrict what an agent may do with MCP tools? Ceiling the server
  with `rkat mcp add --allow-operation`, or govern the realm and grant only the
  operations it may call (`references/security_governance.md`).
- How do I require approval before an action? Govern the realm, grant the
  operation, add an `r3` rule (`device_trusted` for the stock pipe), and
  decide over `rkat-rpc` with `approval/consent_decide`; see
  `references/security_governance.md`.
- Where are my tokens? `<config dir>/meerkat/credentials/`, not in config files.
