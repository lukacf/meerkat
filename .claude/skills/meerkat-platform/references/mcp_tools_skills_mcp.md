Covers: MCP servers in Meerkat 0.9. Config in `mcp.toml` and realm
`[[tools.mcp_servers]]` (fields, transports, tool_names, secret_env), `rkat mcp
add|login|remove|list|get` and every flag, which hosts start configured servers,
MCP OAuth per realm (login, status, logout, re-add and release), access ceilings,
live `mcp/add|remove|reload`, server close, deferred tools (`tool_catalog_search`,
`tool_catalog_load`), MCP Apps, errors (`outcome_uncertain`), `rkat-mcp` logging.

# MCP Servers (0.9)

Names are exact: anything not listed does not exist. "(Rust host)" marks
Rust-embedder-only APIs. Built-in tools, tool policy and callback tools are in
`mcp_tools_skills_tools.md`; skills are in `mcp_tools_skills_skills.md`.

## Quick answers

- Local (stdio) server for this project:
  `rkat mcp add fs -- npx -y @modelcontextprotocol/server-filesystem .`
- Remote server: `rkat mcp add tracker https://mcp.example.com/mcp`. With
  OAuth: add an account selection (`--oauth-account-selection discover`,
  `--oauth-account <provider-subject>`, or `--oauth-account-selection
  unverified`), then `rkat mcp login tracker`.
- Configured servers start in `rkat run` / `rkat run --resume`, governed
  `rkat-rpc` sessions, and `rkat-mcp` sessions a schedule creates (the realm's
  servers). Other `rkat-rpc`, `rkat-rest` and `rkat-mcp` sessions need live
  `mcp/add`.
- Add a server to a running session: RPC `mcp/add`, REST
  `POST /sessions/{id}/mcp/add`, `rkat-mcp` tool `meerkat_mcp_add`, Python
  `mcp_add`, TypeScript `mcpAdd`. It applies at the next turn boundary.
  There is no CLI live add and no `rkat mcp reload`.
- Sign out of an MCP server: RPC `auth/logout` with an `mcp` target (Python
  `auth_mcp_logout`, TypeScript `authMcpLogout`). There is no `rkat mcp logout`.
- `access_denied` "Tool 'X' is not allowed by policy" on an MCP tool: usually a
  deferred tool called before `tool_catalog_load` (see "Startup, readiness and
  deferred tools"), or a tool hidden by a tool policy.

## MCP server configuration

| Scope | Location | `rkat mcp --scope` |
|---|---|---|
| Project | `<context-root>/.rkat/mcp.toml` | `project` (default for `add`; `local` is an alias) |
| User | `~/.rkat/mcp.toml` | `user` |
| Realm | `[[tools.mcp_servers]]` in the realm's own config, inherited by child realms | `realm` |

- Each `mcp.toml` server is a `[[servers]]` table; realm servers use the same
  keys under `[[tools.mcp_servers]]`.
- Project wins over user for the same name. A realm server and an `mcp.toml`
  server with the same name must be identical, or sessions refuse to start:
  `MCP server '<name>' is defined differently in the realm config and in the
  <scope> mcp.toml; remove or rename one of them`.
- `mcp.toml` expands `${VAR}` in `command`, `args`, `env` values, `url` and
  `headers` values from the host environment (a missing variable is an error).
  Realm servers are literal: a config write that adds `${` to a realm server is
  refused (invalid params on RPC and `rkat-mcp`, HTTP 400 on REST).
- A child realm can add or override inherited servers but cannot remove them.

Which hosts start configured servers:

- `rkat run` / `rkat run --resume`: realm, project and user servers.
  Project servers are announced on stderr (`Loading N MCP server(s) from
  project config:`).
- Governed `rkat-rpc`: the realm's configured servers are carried into each
  session; one connects only if the agent holds a current grant for it.
- `rkat-mcp` sessions a schedule creates: the realm's servers (composed along
  its parent chain; no `mcp.toml`). Ordinary `rkat-rpc`, `rkat-rest` and
  `rkat-mcp` sessions: none.
- Mob members: profile `tools.mcp_servers` (full entries) or `tools.mcp`
  (names). The CLI-hosted mobpack RPC host starts no realm servers and warns.

| Key | Transport | Meaning |
|---|---|---|
| `name` | all | Unique server name |
| `command`, `args`, `env` | stdio | Process to spawn (`command` makes it stdio) |
| `secret_env` | stdio | Env var names whose values come from the realm credential store |
| `cwd` | stdio | Absolute working directory, used as written; missing at launch refuses it |
| `url`, `headers` | HTTP | Endpoint and static headers |
| `transport` | HTTP | `"streamable-http"` (default) or `"sse"` (legacy) |
| `oauth_account` | streamable HTTP | Expected OAuth provider subject or account id |
| `oauth_account_selection` | streamable HTTP | `"discover"` or `"unverified"`; conflicts with `oauth_account` |
| `connect_timeout_secs` | all | Connect, handshake and tool listing bound; default 10 |
| `tool_names` | all | Raw operation name to exposed tool name |
| `access` | all | Access ceiling `{ allowed_operations = [...] }` |
| `account` | all | Only `"none"`: operations act with no account (host config) |
| `connector` | stdio | Host-only connector credential binding (Rust host) |

```toml
# .rkat/mcp.toml
[[servers]]
name = "fs"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "."]
connect_timeout_secs = 20

[[servers]]
name = "tracker"
url = "https://mcp.example.com/mcp"
oauth_account = "user-1234"
access = { allowed_operations = ["search_issues", "get_issue"] }
tool_names = { search_issues = "tracker_search" }
```

Behavior:

- Tools keep the server's raw operation names unless `tool_names` renames
  them (1-64 ASCII letters, digits, `_`, `-`; `tool_catalog_search` and
  `tool_catalog_load` are reserved). An invalid map is refused before the
  server is spawned or contacted: `Invalid tool name mapping for server '<s>',
  operation '<op>': <reason>`. If two routes expose the same name, all of
  them are withheld until `tool_names` resolves the clash.
- Discovery follows every `tools/list` page, up to 100 pages and 10,000 tools;
  beyond that the server's whole list is refused.
- An MCP `isError` result reaches the model as a failed tool result with its
  content. `structuredContent` becomes one structured block.
- Closing a server (remove, reload, replace, shutdown, a failed connect):
  stdio servers run in their own process group on Unix. The close ends stdin
  and waits until the process has exited and every holder of its stdout has
  closed it: SIGTERM to the group after 5 s, SIGKILL after 3 s more, then a
  10 s backstop (Rust hosts: `McpRouter::with_stdio_close_bounds`, each grace
  at most 60 s). Since 0.9.4 a macOS close also settles as soon as the server
  has exited and its stdout is closed, as on Linux; in 0.9.0-0.9.3 every
  macOS stdio close waited out both graces and sent SIGKILL, even to a server
  that had already exited (by default about 8 s per server; a server that
  failed setup stayed pending that long, and a live add, reload or restore of
  it was refused). A remote server's close that does not finish within 18 s
  (the default stdio bound) fails `CloseTimedOut` (`MCP server '<name>' close
  did not finish within ...`). (Rust host) `McpRouter::shutdown_with_report()`
  returns an `McpRouterShutdownReport` (`servers()`, `is_settled()`);
  `shutdown()` discards it.
- HTTP: only same-origin redirects (at most 3 hops) are followed. If the
  server drops the session (HTTP 404) during a call, or a redirected call
  fails, the call is not resent: it ends as `outcome_uncertain`, meaning it may
  have taken effect. Check the remote state before retrying. After a session
  drop the connection is dead until reconnected (for example `mcp/reload`).
  A server must answer an unsupported SSE GET with 405, not 404. Frames are
  capped at 64 MiB.
- Binaries older than 0.9 cannot read a server entry that carries `access` or
  `secret_env`; upgrade every reader of a shared realm config first.

Service secrets (`secret_env = ["API_TOKEN"]`, stdio only): values live in the
credential store of the realm whose own config defines the server and go only
to that server's process at launch, never from the host environment. A server
from `mcp.toml`, a host or a live add that declares `secret_env` is refused; a
missing value refuses the launch (`MCP server '<name>' was not launched:
...`). A Rust host sets values with `meerkat::set_mcp_service_secret`
(`remove_mcp_service_secret` deletes); there is no CLI command. Names must be
env var names, must not repeat an `env` key, and cannot be
`MEERKAT_CONNECTOR_BINDING`.

## rkat mcp commands

```bash
rkat mcp add <NAME> [-t|--transport stdio|http|sse] [--scope project|user|local|realm] [--expected-generation <N>] [-H|--header KEY:VALUE...] [-e|--env KEY=VALUE...] [--oauth-account <ACCOUNT_ID> | --oauth-account-selection discover|unverified] [--allow-operation <OPERATION>...] [-u|--url <URL> | <URL> | -- <CMD> [ARGS...]]
rkat mcp login <NAME> [--scope project|user|local|realm] [--allow-headless]
rkat mcp remove <NAME> [--scope project|user|local|realm] [--expected-generation <N>]
rkat mcp list [--scope project|user|local|realm] [--json]
rkat mcp get <NAME> [--scope project|user|local|realm] [--json]
```

- A command after `--` makes a stdio server; a URL makes a streamable HTTP
  server unless `--transport sse`. Do not combine `--url` with a positional
  URL, or a URL with a command.
- `-H KEY:VALUE` sets HTTP headers; `-e KEY=VALUE` sets stdio environment.
- `--oauth-account` and `--oauth-account-selection` need streamable HTTP
  without a static `Authorization` header, and conflict. `add` prints the
  selection it wrote; for a streamable HTTP server without a selection or a
  static `Authorization` header it names the three choices.
- `--allow-operation` (repeatable) writes `access = { allowed_operations = [...] }`.
- `--scope realm` edits the realm chosen by `--realm`; `--expected-generation`
  is accepted only with `--scope realm`.
- `add` refuses a name that exists in that scope. `remove` without `--scope`
  removes from the one place that defines the server and asks for `--scope`
  when there are several; an inherited server must be removed in the realm
  that configures it.
- `list` and `get` without `--scope` show every scope. `get` text output
  masks env and header values whose name contains key, secret, token or
  password; `--json` (on `list` and `get`) prints stored values unmasked.
  Neither shows `access`, `tool_names`, `secret_env` or `cwd`.
- `rkat mcp` edits configuration only; sessions read it when they start or
  when a server reloads. There is no `rkat mcp reload`, `rkat mcp logout` or
  `--scope all`.

## MCP OAuth

OAuth applies to streamable HTTP servers without a static `Authorization`
header; endpoints are discovered at connect or login time. Since 0.9
credentials are per realm: each realm logs in once for itself, a credential
stored before 0.9 is left unused, and another realm's target is refused
(`MCP server '<name>' belongs to another realm than this host serves`).

Account selection on the server entry:

- `oauth_account = "<subject>"` (`--oauth-account`): the login must prove this
  subject (by default the issuer's OpenID UserInfo `sub`; issuers without
  UserInfo refuse).
- `oauth_account_selection = "discover"` (`--oauth-account-selection
  discover`): binds the account verified at the first login; later logins
  must prove the same one.
- `oauth_account_selection = "unverified"` (`--oauth-account-selection
  unverified`): resource-bound access with no account evidence. Only the
  operator chooses it; a failed verification never falls back to it.
- None of these: `rkat mcp login` refuses before any network request
  (`MCP server '<name>' has no OAuth account selection...`) and prints the
  `rkat mcp remove`/`rkat mcp add` commands for each choice. Hosts get the
  auth reason `account_selection_required` ("MCP OAuth interactive login
  requires an account selection: ...").

CLI:

- `rkat mcp login <name>` opens the browser with a loopback callback and
  waits up to 300 s. It needs an interactive terminal (or `--allow-headless`,
  which may print the authorize URL to stderr), a streamable HTTP server
  without a static `Authorization` header, and one matching entry (pass
  `--scope` if the name exists in several). Ctrl-C closes the attempt.
  Success prints `Logged in MCP server '<name>' (<url>) as <account>`
  (`... (bound by discovery)` for discover), or `... without account
  verification (unverified, resource-bound access)`.
- `rkat run --mcp-auth stored` (default) uses stored tokens only; a missing
  one reports `MCP OAuth token not found for '<name>'. Run: rkat mcp login
  <name>` or that the server is awaiting human authorization.
- `rkat run --mcp-auth interactive` lets a terminal run log in on first use
  and continue, for any server with an account selection.

Hosts (they never open a browser):

- RPC `auth/login/start` with `{"mcp": {"server_name": "...", "server_url":
  "...", "oauth_account": "..."}, "redirect_uri": "http://127.0.0.1:<port>/..."}`
  (an http loopback URL you bind) returns `authorize_url`, `state` and
  `disposition` (`started`, or `joined` for an attempt already pending).
  `auth/login/complete` takes `{mcp, code, state, redirect_uri}`;
  `auth/login/cancel` takes `{mcp, state}` or `{mcp, attempt_ref}`.
- `auth/status/get {mcp}` returns `phase` (`authorized`, `reauth_required`,
  `authorization_required`), `account_verification` (`verified`,
  `unverified`, `legacy`), optional `expires_at` and `account_id`, and a
  pending `attempt {ref, phase: "pending", expires_at}`.
- `auth/logout {mcp}` returns `{mcp, cleared}`: removes this realm's stored
  credential and retires any pending login; nothing is revoked at the provider.
- REST `POST /auth/login/start|complete|cancel` take the same MCP target; REST
  has no MCP status or logout.
- Python: `auth_mcp_login_start`, `auth_mcp_login_complete`,
  `auth_mcp_login_cancel`, `auth_mcp_login_cancel_by_attempt_ref`,
  `auth_mcp_status`, `auth_mcp_logout`. TypeScript: `authLoginStart`,
  `authLoginComplete`, `authLoginCancel`, `authMcpStatus`, `authMcpLogout`.
- The target must match a configured server (name, URL and account), else
  invalid params / HTTP 400 before any network call. Branch on
  `error.data.reason`, for example `mcp_server_not_configured`,
  `mcp_server_mismatch`, `account_selection_required`, `missing_scopes`,
  `authorization_required`, `reauth_required`, `attempt_missing`, and the 0.9
  credential-access reasons `credential_access_denied`,
  `credential_interaction_required`, `credential_access_pending`,
  `credential_store_unavailable`.

Automatic re-add and release (0.9): a successful RPC or REST
`auth/login/complete` re-stages, in every live session of that runtime waiting
for exactly that server (name, URL and account), the add or reload that failed
for lack of authorization, at its next turn boundary. A successful RPC
`auth/logout` unpublishes the server from sessions connected with that
credential at their next boundary (calls are refused at once), and a later
login adds it back. A server whose call finds no usable credential (for
example after a logout in another process) is released the same way. No
`mcp/reload` is needed.

## Access ceilings, accounts and connector approvals

- `access = { allowed_operations = [...] }` (or `--allow-operation`): other
  raw operations are not published under any name; a call that still reaches
  one is refused `access_denied` (`MCP server '<s>' does not permit operation
  '<op>' under its access policy`). An empty list is refused. A ceiling only
  narrows: it does not make operations read-only. Sessions apply it when they
  connect; a live session keeps its old definition until that server reloads.
- `account = "none"` (host config) states that operations act with no account;
  it conflicts with `oauth_account`, `oauth_account_selection` and `connector`.
  Governed review above R1 needs the server to state its account
  (`oauth_account`, a connector binding, or `account = "none"`); an operation
  on a server that states nothing (env or header credentials only) is refused
  for R2/R3.
- Governed realms (preview): `rkat realm grant <realm> --mcp <server>=<op>[,<op>...]`
  (or `<server>=*`) grants an installed server. The grant records the
  installation's connector approval (`none` or the binding's `sha256:...`
  identity). A grant without it, including every grant issued before 0.9,
  admits nothing: reissue it. Loading deferred tools also needs
  `rkat realm grant <realm> --catalog`. `[[governance.review.rules]]` can raise
  an `mcp_server` or `mcp_operation` target to R2 or R3. Schedules run no
  agent work in a governed realm yet: since 0.9.3 `rkat` and `rkat-rpc` refuse
  every fire there as `schedule_mandate_unavailable` before a session is
  created, and `rkat-rest` and `rkat-mcp` refuse to open a governed realm.

## Live attach and detach

| Surface | Calls |
|---|---|
| JSON-RPC | `mcp/add {session_id, server_config, persisted?}`, `mcp/remove {session_id, server_name, persisted?}`, `mcp/reload {session_id, server_name?}` |
| REST | `POST /sessions/{id}/mcp/add`, `.../mcp/remove`, `.../mcp/reload` (same bodies; `session_id` must match the path) |
| `rkat-mcp` | `meerkat_mcp_add`, `meerkat_mcp_remove`, `meerkat_mcp_reload` |
| SDKs | Python `mcp_add(session_id, server_config, persisted=False)`, `mcp_remove`, `mcp_reload`; TypeScript `mcpAdd(params)`, `mcpRemove`, `mcpReload` |

- `server_config` is a config entry, for example `{"name": "fs", "command":
  "npx", "args": ["-y", "@modelcontextprotocol/server-filesystem", "."]}`.
- The response is `{session_id, operation, server_name, status: "staged",
  persisted}`. Changes apply at the session's next turn boundary and tools
  appear once the server connects. Removal drains in-flight calls first.
  Reload without `server_name` reloads all; an unknown name is invalid params.
- `persisted: true` also writes the change to the host's project `mcp.toml`
  (rolled back if staging fails). The `meerkat_mcp_*` tools never persist.
- `meerkat_mcp_add` refuses `oauth_account`, `oauth_account_selection` and
  `account`. REST answers 409 `Live MCP unavailable for this session` for a
  session created before the server process started.
- Live-added HTTP servers use stored OAuth credentials; without one the
  server waits for a host login, then is re-added automatically.
- Governed runtimes: stock `rkat-rest` and `rkat-mcp` refuse to open a
  governed realm (`realm '<id>' is governed and cannot be opened as an
  ordinary realm`). On a governed runtime, REST and `meerkat_mcp_add` refuse
  live adds (no authenticated attachment control). The governed `rkat-rpc`
  stdio pipe offers `mcp/add` for live attachment only (`persisted: true` is
  refused) and decides it against the agent's grants.
- RPC records each accepted add durably. After a host restart a resumed
  session re-stages a recorded server only while it is still installed in the
  realm config or `mcp.toml` (for example added with `persisted: true`), from
  that current definition. Otherwise it is refused (retired, rebound, account
  changed, or `renewal_required` when the record cannot prove its target) and
  needs a fresh add. Outcomes arrive as `tool_config_changed` events. Since
  0.9.4 their lifecycle bit is `settled` (events and notices stored by 0.9.3
  and earlier keep `persisted`; readers accept either). The request and
  response field `persisted` of `mcp/add|remove|reload` keeps its meaning:
  whether the change is also written to the host's project `mcp.toml`.
- (Rust host) `PersistentSessionService::attach_live_mcp_server` attaches to a
  governed member at its turn boundary. `try_attach_live_mcp_server` never
  waits: a held boundary (including a call from inside the member's own turn)
  returns `LiveMcpAttachError::Busy` and attaches nothing; `Busy` grants
  nothing and is not an instruction to retry. A member without a live
  attachment surface returns `Unsupported`.
- (Rust host) Since 0.9.4 a `ConfiguredMcpAttachmentProvider` and
  `NativeWorkAuthorizationHost::configured_mcp_attachment_for_agent` receive
  the exact `RuntimeSessionRegistrationWitness` instead of a `SessionId`, and
  decide for that one registration: a later registration of the same session
  is a different one. Read the id with `registration.session_id()`; a closure
  provider is `Fn(&RuntimeSessionRegistrationWitness, SessionAgentBinding) ->
  Option<..>`.

## Startup, readiness and deferred tools

- Servers connect in the background; tools appear as each finishes. Meanwhile
  the model sees `Servers connecting: <names>. Tools will appear when ready.`
- The CLI waits before the first turn with `--wait-for-mcp`, and also
  automatically when a server may need OAuth. The wait is the largest
  `connect_timeout_secs` plus 5 s (plus 300 s with `--mcp-auth interactive`);
  servers still pending then abort the run (`MCP servers still pending after
  ...`). A server that fails during the wait prints `Warning: MCP server
  '<name>' failed: ...` and the run continues without it.
- (Rust host) `AgentBuildConfig.wait_for_mcp`,
  `McpRouterAdapter::wait_until_ready(timeout)`.

Deferred tools: MCP tools and registered callback tools are deferred-eligible.
A session switches to the deferred catalog while a server is still connecting,
or when there are 2 or more eligible tools, or when their names, descriptions
and schemas total 160 characters or more. In practice a server with two or
more tools is deferred. The model then gets two control tools instead of the
deferred schemas:

- `tool_catalog_search {query?, limit?}` (limit 1-50, default 10). Results
  carry `name`, `description`, `currently_callable` and `visibility_status`
  (`loaded`, `deferred`, `blocked_by_filter`, `temporarily_unavailable`;
  `pending_source` while a source is connecting).
- `tool_catalog_load {names: [...]}`. Accepted names become callable at the
  next boundary. Rejections: `unknown_key`, `not_deferred_eligible`,
  `not_filterable`, `temporarily_unavailable`.

A deferred tool called before it is loaded fails `access_denied` ("Tool
'<name>' is not allowed by policy"). Both control tools are read-only.

## MCP Apps

Opt-in for Rust UI hosts: `AgentFactory::mcp_apps(true)` (off by default; stock
binaries do not enable it). Connections then advertise
`io.modelcontextprotocol/ui` and keep each call's definition, arguments, full
result and UI resource (`ui://`, `text/html;profile=mcp-app`) for the host's
renderer. `ToolAudience` (`_meta.ui.visibility`) separates model and app
visibility; app-only tools are hidden from the model and the deferred catalog.
App actions go through `ToolApplicationControlRequest` on `SessionService` and
normal tool policy: visibility is not authorization.

## MCP errors and refusals

| Message or code | Meaning |
|---|---|
| `access_denied` "Tool 'X' is not allowed by policy" | Deferred and not loaded, hidden by policy, or refused by read-only |
| `access_denied` "... does not permit operation ..." | Outside the server's access ceiling |
| `tool_not_found` | Name not published (ceiling, name clash, server gone) |
| `outcome_uncertain` "Outcome of tool 'X' is uncertain: ..." | Sent, then the session expired or a redirect failed: it may have run. Not resent; check before retrying |
| `tool_unavailable` | Host egress profile refused, or route temporarily unavailable |
| `execution_failed` ("Tool call failed for ...", "Server 'X' is not accepting new calls ...") | Server or transport failure |
| `confinement_refused` | Host launch confinement refused the process |
| "MCP server 'X' is awaiting human authorization" | No usable OAuth credential: log in |
| "... already has a credential this login cannot replace; disconnect it first" | Slot holds another account, issuer or client, or an unverified credential: RPC `auth/logout` with the MCP target (SDK `auth_mcp_logout` / `authMcpLogout`) first; `rkat` has no disconnect command |
| "MCP OAuth account selection requires its OAuth resolver and streamable HTTP without static Authorization" | `oauth_account` or `oauth_account_selection` on stdio, SSE or with an `Authorization` header |
| "MCP server 'X' has no OAuth account selection, so `rkat mcp login` cannot bind its credential to an account" | Add it again with `--oauth-account-selection discover`, `--oauth-account <SUBJECT_ID>` or `--oauth-account-selection unverified` |
| "MCP server 'X' gave no account evidence this host can verify" | No usable OpenID Connect UserInfo, or the issuer was unreachable; nothing stored. Retry first; re-add with `--oauth-account-selection unverified` only if the issuer publishes no UserInfo and resource-bound access is acceptable |
| "The account signed in for MCP server 'X' is not the subject its --oauth-account names" | Another account signed in; nothing stored |
| "MCP server 'X' has an invalid access policy: ..." | Empty or malformed `access` |
| "MCP attachment refused: ..." | Governed attachment decision refused |
| "Invalid tool name mapping for server 'X', operation 'Y': ..." | Bad `tool_names` entry; nothing was started |
| `CloseTimedOut` "MCP server 'X' close did not finish within ..." | A remote close exceeded its bound |

## Troubleshooting

- Server tools missing: check the host starts configured servers (see the
  list above), `rkat mcp list`, a name clash, an access ceiling, or the
  deferred catalog (`tool_catalog_search`).
- OAuth server never connects in `rkat run`: run `rkat mcp login <name>` in the
  same realm, then retry; credentials do not carry over between realms.
- Config changed but a running session did not notice: `mcp/reload`, or start
  a new session; `rkat mcp` never touches running sessions.
- `rkat doctor` exits 1 on an MCP config load error.
- `rkat-mcp` logs only to stderr (stdout carries the MCP protocol), at `info`
  by default; set `RUST_LOG` (for example `RUST_LOG=debug`) to change it. An
  unparsable value is named on stderr and the default applies.
  `meerkat_run` and `meerkat_resume` take `verbose: true` to log the run's
  agent events to that stderr log; the tool result is unchanged.
