Covers: provider credentials and auth: env-var API keys and precedence, realm
backends, auth profiles and bindings, backend kinds, base URLs, no-redirect
policy and cloud routes (Azure OpenAI, Bedrock, Vertex, Foundry, Copilot),
credential storage and keychain refusals, every `rkat auth` subcommand, login
and loopback callback rules, `--auth-binding` per surface, auth RPC/REST/SDK
methods and errors, connector OAuth clients. Models: references/providers_and_auth_models.md.

# Provider Credentials and Auth (Meerkat 0.9)

## Environment keys

The fastest path: export a key for the provider of the model you run. Keys
are read at resolve time, and an `RKAT_` name wins over the provider-native
name. Keys never choose the provider; the model does (the no-hint default is
OpenAI `gpt-6-astra`, see references/providers_and_auth_models.md).

| Provider | Order |
|---|---|
| Anthropic | `RKAT_ANTHROPIC_API_KEY`, `ANTHROPIC_API_KEY` |
| OpenAI | `RKAT_OPENAI_API_KEY`, `OPENAI_API_KEY` |
| Azure OpenAI | `RKAT_AZURE_OPENAI_API_KEY` + `RKAT_AZURE_OPENAI_ENDPOINT`, then `AZURE_OPENAI_API_KEY` + `AZURE_OPENAI_ENDPOINT` |
| Gemini | `RKAT_GEMINI_API_KEY`, `GEMINI_API_KEY`, `RKAT_GOOGLE_API_KEY`, `GOOGLE_API_KEY` |

The Azure pair is used only when no OpenAI key is set or
`RKAT_AZURE_OPENAI_API_KEY` / `RKAT_AZURE_OPENAI_ENDPOINT` is set.
`AZURE_OPENAI_IMAGE_GENERATION_DEPLOYMENT` and
`AZURE_OPENAI_IMAGE_GENERATION_API_VERSION` (also with an `RKAT_` prefix)
configure Azure image generation. Self-hosted servers have no env key.

The env path is a synthetic, never-persisted binding
(`origin = "synthetic_env_default"`) and the last candidate: a configured
binding for the provider whose credential resolves (for example a stored
login) is used first.

## Realm auth model

Credentials resolve through realm-scoped config. A realm (`[realm.<id>]`)
holds backend profiles, auth profiles and bindings:

```toml
[realm.prod]
default_binding = "claude"          # optional realm default
# parent = "base"                   # optional parent realm; `global` is always the tail

[realm.prod.backend.anthropic_main]
provider = "anthropic"
backend_kind = "anthropic_api"

[realm.prod.auth.anthropic_key]
provider = "anthropic"
auth_method = "api_key"
source = { kind = "env", env = "PROD_ANTHROPIC_API_KEY" }

[realm.prod.binding.claude]
backend_profile = "anthropic_main"
auth_profile = "anthropic_key"
default_model = "claude-opus-5-5"   # optional
provider_default = true             # optional: default binding for its provider
```

- `source.kind`: `env` (`env`, optional `fallback` list), `managed_store`
  (token store, filled by login or `auth/profile/create`), `inline_secret`
  (`secret`; local development only), `platform_default`,
  `external_resolver` (`handle`, a host-registered resolver), `command`
  (`program` prints a token; `args`, `cwd`, `env`, `timeout_ms` default 30000,
  `refresh_interval_ms` caches it) and `file_descriptor` (host integration
  only).
- Binding keys: `backend_profile`, `auth_profile`, `credential_account`
  (several bindings share one stored credential, as the Copilot routes do),
  `default_model`, `provider_default`, `policy` (`allow_auth_override`,
  `require_metadata_account`, `require_metadata_workspace`).
- Inheritance: config and credential reads inherit down the chain (head
  realm, parents, then `global`); a resolved binding belongs to the realm that
  defines it. Credential writes over RPC, REST and host APIs are strict-owner:
  a write addressed to a child for an inherited binding is refused
  (`binding_inherited`). Put shared credentials in `global` or a parent.
- With no explicit binding, a session tries, for the model's provider: the
  head realm, each parent child-first, `global`, then the environment
  variables. Inside a realm the choice is `default_binding` (if it serves the
  provider), else the `provider_default` binding, else the only binding for
  that provider (several unmarked ones are never picked implicitly). The first
  candidate whose credential resolves wins; an explicit binding is strict and
  never falls back.
- `rkat --realm <id> run ...` selects the session realm (the chain head);
  `--auth-binding` selects the credential identity.

## Backends, base URLs and cloud routes

Supported `backend_kind` / `auth_method` pairs:

| Provider | `backend_kind` | `auth_method` values |
|---|---|---|
| anthropic | `anthropic_api` | `api_key`, `static_bearer`, `claude_ai_oauth`, `oauth_to_api_key`, `external_authorizer` |
| anthropic | `bedrock` | `bedrock_bearer`, `bedrock_aws_sigv4`, `external_authorizer` |
| anthropic | `vertex` | `vertex_google_auth`, `external_authorizer` |
| anthropic | `foundry` | `foundry_api_key`, `foundry_azure_ad`, `external_authorizer` |
| anthropic, openai, gemini | `copilot` | `github_copilot_oauth`, `github_copilot_pat` |
| openai | `openai_api` | `api_key`, `static_bearer`, `external_authorizer` |
| openai | `chatgpt_backend` | `managed_chatgpt_oauth`, `external_chatgpt_tokens` |
| openai | `azure_openai` | `azure_api_key` |
| gemini | `google_genai` | `api_key`, `bearer_api_key`, `external_authorizer` |
| gemini | `vertex_ai` | `adc`, `api_key_express`, `external_authorizer` |
| gemini | `google_code_assist` | `google_oauth`, `compute_adc`, `external_authorizer` |
| self_hosted | `self_hosted`, `openai_compatible` | `none`, `api_key`, `static_bearer` |

Base URLs: defaults are `https://api.anthropic.com`, `https://api.openai.com`,
`https://chatgpt.com/backend-api/codex`, `https://generativelanguage.googleapis.com`
and `https://cloudcode-pa.googleapis.com`. Override with `base_url` on the
backend profile; there is no base-URL environment variable. `azure_openai`
and `bedrock` require `base_url`; Anthropic `vertex` requires it (for example
`https://<region>-aiplatform.googleapis.com`).

Native clients follow no HTTP redirects (since 0.8.52, same-origin
included), so configure the final URL. A `3xx` from a model endpoint fails
the call terminally (`InvalidConfig`: "the configured endpoint answered HTTP
<status> redirect ... Configure the final endpoint as the base URL"). The
credential routes (token, refresh and device-code exchanges, the Claude,
ChatGPT and Code Assist OAuth runtimes, Google and Azure credential
exchanges, the Copilot token exchange, `HostAuthService`) refuse it as
`RedirectRefused { status }`; a redirect-free client that cannot be built is
`CredentialHttpClientUnavailable`. Browser authorization redirects and the
loopback callback are unaffected.

- Azure OpenAI: the session model is the Azure deployment name. `base_url`
  may end in `/openai` or `/openai/v1`. Options
  `image_generation_deployment` and `image_generation_api_version` select
  image generation.

```toml
[realm.prod.backend.azure_openai]
provider = "openai"
backend_kind = "azure_openai"
base_url = "https://my-resource.cognitiveservices.azure.com"

[realm.prod.auth.azure_key]
provider = "openai"
auth_method = "azure_api_key"
source = { kind = "env", env = "PROD_AZURE_OPENAI_API_KEY" }

[realm.prod.binding.azure]
backend_profile = "azure_openai"
auth_profile = "azure_key"
default_model = "my-deployment"     # the Azure deployment name
```

- Bedrock `bedrock_aws_sigv4` needs an explicit region:
  `options = { aws_region = "us-east-1" }` on the backend (or `region`).
- `openai_api` (API key) and `chatgpt_backend` (ChatGPT subscription) are
  separate account systems with different model sets.
- Copilot routes keep the model's provider identity. The account's model
  list is checked first: `Copilot account does not expose <endpoint> for
  <provider> model '<model>'` lists the ids the account offers. The
  integration is provisional (it uses an observed GitHub client contract).
- `bedrock_aws_sigv4`, `vertex_google_auth` and `foundry_azure_ad` are
  compiled only with the `meerkat-anthropic` features `bedrock`, `vertex` and
  `foundry`, and `adc` / `compute_adc` only with `meerkat-gemini/adc`; the
  prebuilt binaries enable none of them, so those methods fail there with a
  missing-feature error. Key, bearer and `external_authorizer` routes need no
  extra feature. None of these native cloud methods run in the browser
  runtime.

## Credential storage

- The stock `rkat`, `rkat-rpc` and `rkat-rest` share a file token store in
  `<platform config dir>/meerkat/credentials` (`~/.config/meerkat/credentials`
  on Linux, `~/Library/Application Support/meerkat/credentials` on macOS):
  `<realm>/<binding>.json`, `<realm>/<binding>@<profile>.json`, and
  `<realm>/account~<account>.json` for shared accounts; mode 0600 on Unix.
  Login writes only non-secret declarations to `~/.rkat/config.toml`;
  `--user-config-root` moves that config, not the token store.
- The OS keychain is opt-in for embedders: build with the `native-keyring`
  feature and open `TokenStoreBackend::default_keyring_auto()` (service
  `meerkat-auth`). It keeps an existing file credential per key and writes new
  keys to the keychain; it falls back to the file only when the platform
  reports no credential store. `TokenStoreBackend::default_auto()` is the
  stock file profile.
- Keychain access never blocks callers. Status never reads the keychain and
  reports `credential_observation` (`not_observed`, `owner_busy`, `pending`,
  `denied`, `interaction_required`, `unavailable`), never a false absence.
  Unattended access (session setup, requests, refresh) makes at most one
  bounded lookup per credential; only interactive operations (login, logout,
  an explicit save or clear) may raise operating-system prompts. Refusals:
  - `credential_access_denied`: the OS reported that the user denied
    access. Not retried until an interactive login, save or clear succeeds.
  - `credential_interaction_required`: the OS needs user interaction that an
    unattended or status access does not raise; run an interactive
    operation such as `rkat auth login`.
  - `credential_access_pending`: an operation for this credential is
    already in flight; no new OS call was started. Retry shortly.
  - `credential_store_unavailable` (with a detail): the store did not
    answer (lookup capacity or the credential's owner was busy, a status
    read that did not look, or an access the backend does not support).
- OAuth tokens refresh automatically, starting 60 seconds before expiry
  (lock files in `credentials/refresh-locks` serialize refresh across
  processes). A permanent failure leaves `reauth_required`: log in again.

## rkat auth commands

`rkat auth <command> [--realm <ID>]` (default `global`). For `profiles`,
`profile`, `profile-delete`, `test`, `status` and `refresh`, `--realm` picks a
realm section of the already-loaded workspace and `global` config; it does
not load that realm's own config file. It filters `bindings`. It does not
retarget `login` or `logout`. `connector-client` is different: `set` and
`remove` write the named realm's own config document
(`<state root>/<realm>/config.toml`; for `global`, `~/.rkat/config.toml`),
`rotate-secret` reads the client from that document and replaces only its
stored secret, and `list` reads that realm's effective chain.

| Command | What it does |
|---|---|
| `realms` | Lists configured realm sections (default binding, counts). |
| `profiles` | Lists backends, auth profiles and bindings of one realm. |
| `profile <PROFILE_ID>` | Prints one auth profile as JSON. |
| `profile-delete <PROFILE_ID> [-y]` | Deletes the stored credential of the binding that uses the profile; config is untouched. |
| `bindings` | Lists realm, binding, backend profile, auth profile, default model. |
| `test <BINDING_ID>` | Resolves the binding through the provider runtime; prints `state`, `provider`, `backend_profile_id`, `has_credential`. |
| `status <PROFILE_ID>` | Profile, method, source kind, binding, `state` (`valid`, `expiring`, `expired`, `reauth_required`, `refresh_failed`, `released`, `absent`, `missing_credential`), `expires_at`. |
| `login [PROVIDER] ...` | Sign in; see Login flows. |
| `logout [<REALM>:]<BINDING_ID>` | Clears the stored credential; a bare binding means `global`. |
| `refresh <PROFILE_ID>` | Forces an OAuth refresh; a no-op for key and bearer methods. |
| `connector-client set|rotate-secret|list|remove` | Connector OAuth clients (see below). |

`status`, `refresh` and `profile-delete` act on the binding that references
the profile (the realm `default_binding` if it does, else the only one).
Logout resolves the address through the loaded config: logging out
`child:<binding>` for an inherited binding clears the owner's credential,
possibly an account shared with other routes, so address the true owner.

## Login flows

Interactive login (`rkat auth login`, or `rkat auth login <provider>`) writes
non-secret backend, auth and binding sections to the `global` realm in
`~/.rkat/config.toml`, so every workspace inherits it, and stores tokens in
the token store. It sets `[realm.global] default_binding` if none is set.

| Provider argument (aliases) | Flow | Provisions |
|---|---|---|
| `anthropic` (`claude`, `claude.ai`) | Browser OAuth, PKCE S256 (Claude Pro/Max) | `global:anthropic_oauth` (`anthropic_api` + `claude_ai_oauth`) |
| `openai` (`chatgpt`) | Browser OAuth (ChatGPT Plus/Pro) | `global:openai_oauth` (`chatgpt_backend` + `managed_chatgpt_oauth`) |
| `google` (`gemini`, `code_assist`) | Browser OAuth (Gemini Code Assist) | `global:google_oauth` (`google_code_assist` + `google_oauth`) |
| `copilot` (`github_copilot`) | GitHub device code | `global:copilot_openai`, `global:copilot_anthropic`, `global:copilot_gemini`, sharing `credential_account = "github_copilot"` |

- Each binding's `default_model` is the provider's catalog default at login
  time, except that `global:google_oauth` (Gemini Code Assist) and
  `global:copilot_gemini` start on `gemini-3.8-flash` (0.9.1), because the
  catalog's Gemini default (`gemini-4-argon`) is not verified on those routes. A plain `rkat run` still uses the global default model; select the
  login with `--auth-binding` or a matching `--model`:
  `rkat run --auth-binding global:anthropic_oauth "hi"`.
- The browser step waits 2 minutes; Ctrl-C cancels and nothing is saved
  before the code exchange.
- The loopback callback must carry this login's exact `state`, and the
  first callback decides. A different `state` fails with `StateMismatch`
  ("possible CSRF or stale browser tab"); only `error=access_denied` with
  the matching `state` is `UserDenied`; any other error or a malformed
  callback fails as a callback parse error. Nothing is saved in each case.
- `--backend` and `--method` are ignored in interactive mode.
- Copilot needs the `copilot` feature (in default builds).

Scripted API-key storage (the binding must exist first):

```toml
# ~/.rkat/config.toml
[realm.global.backend.openai_api]
provider = "openai"
backend_kind = "openai_api"

[realm.global.auth.openai_apikey]
provider = "openai"
auth_method = "api_key"
source = { kind = "managed_store" }

[realm.global.binding.default_openai]
backend_profile = "openai_api"
auth_profile = "openai_apikey"
default_model = "gpt-5.5"
```

```bash
rkat auth login openai --non-interactive --secret "$OPENAI_KEY"
echo "$KEY" | rkat auth login openai --non-interactive   # or read from stdin
rkat run --auth-binding global:default_openai "hello"
```

- Provider must be `anthropic`, `openai`, `gemini` or `self_hosted` (no
  aliases). The secret is stored under `global:default_<provider>` only: no
  backend, auth or binding is written, so add a matching binding whose auth
  profile has `source = { kind = "managed_store" }`, then run with
  `--auth-binding global:default_<provider>`.
- `--method` picks a direct-secret method (default `api_key`; also
  `static_bearer`, and per provider `azure_api_key`, `foundry_api_key`,
  `bedrock_bearer`, `api_key_express`, `bearer_api_key`). OAuth methods are
  refused. `--backend` is recorded in token metadata only.
- Logins persisted by older builds under the `dev` realm are copied to
  `global` once, without overwriting, the first time credentials are read.

## Using bindings

CLI form `<realm>:<binding>[:<profile>]` (ASCII letters, digits, `-`, `_`,
`.`); the third part selects an alternate auth profile of that realm for the
binding. Wire surfaces take an object:

```json
{"realm": "global", "binding": "openai_oauth", "profile": "optional"}
```

| Surface | Field |
|---|---|
| CLI | `rkat run --auth-binding global:openai_oauth`; also `rkat mob spawn-helper` and `fork-helper` |
| RPC | `session/create` and `turn/start` `auth_binding` (with `model`, `provider`, `provider_params`) |
| REST | `POST /sessions` and `POST /sessions/{id}/messages` body `auth_binding` |
| MCP | `meerkat_run` `auth_binding` |
| Python | `create_session(prompt, model=..., provider=..., auth_binding={...})` |
| TypeScript | `createSession(prompt, { model, provider, authBinding })` |
| Web | `withAuthBinding(authBinding, config)`; `registerExternalAuthResolver` serves `external_resolver` bindings |
| Rust | `AgentBuildConfig { model, provider, auth_binding, .. }` |

The binding (never the secret) is persisted with the session and reused on
resume. On RPC `turn/start`, omit `auth_binding` to keep it (a switch to
another provider's model drops it instead, and that provider's credential
resolves through the realm chain), pass an object or
`{"action": "set", "value": {...}}` to switch, or `{"action": "clear"}` to
drop it. REST `POST /sessions/{id}/messages` takes `auth_binding` and
rebuilds the agent; the SDK turn methods take none.

A `[model_fallback]` chain target has its own optional `auth_binding` next to
`model` and `provider`; without one, the target provider's binding is
resolved through the realm chain (see Realm auth model). Fallback and model
switching: references/hooks_models_model_operations.md.

## Auth over RPC, REST and SDKs

| RPC method | REST route | Python | TypeScript |
|---|---|---|---|
| `realm/list`, `realm/get` | `GET /realms`, `GET /realms/{id}` | `list_realms`, `get_realm` | `realmList`, `realmGet` |
| `auth/profile/list` | `GET /auth/profiles?realm_id=` | `list_auth_profiles` | `authProfileList` |
| `auth/profile/get` | `GET /auth/bindings/{binding_id}?realm_id=` | `get_auth_profile` | `authProfileGet` |
| `auth/profile/create` | `POST /auth/profiles` | `create_auth_profile` | `authProfileCreate` |
| `auth/profile/delete` | `DELETE /auth/bindings/{binding_id}?realm_id=` | `delete_auth_profile` | `authProfileDelete` |
| (none) | `POST /auth/bindings/{binding_id}/test` | (none) | (none) |
| `auth/login/start`, `auth/login/complete` | `POST /auth/login/start`, `/auth/login/complete` | `auth_login_start`, `auth_login_complete` | `authLoginStart`, `authLoginComplete` |
| `auth/login/cancel` | `POST /auth/login/cancel` | `auth_mcp_login_cancel`, `auth_connector_login_cancel` | `authLoginCancel` |
| `auth/login/device_start`, `auth/login/device_complete` | `POST /auth/login/device/start`, `/auth/login/device/complete` | `auth_login_device_start`, `auth_login_device_complete` | `authLoginDeviceStart`, `authLoginDeviceComplete` |
| `auth/login/provision_api_key` | (none) | `auth_provision_api_key` | `authLoginProvisionApiKey` |
| `auth/status/get` | `GET /auth/bindings/{binding_id}/status?realm_id=` | `auth_status` | `authStatusGet` |
| `auth/logout` | `POST /auth/bindings/{binding_id}/logout?realm_id=` | `auth_logout` | `authLogout` |

- `auth/profile/create` (`realm_id`, `binding_id`, optional `profile_id`,
  `auth_method`, `secret`) stores a direct secret for a binding whose auth
  profile uses `managed_store`; `auth_method` must equal the profile's, and
  OAuth methods use the login methods instead.
- Browser login (`provider` `anthropic`, `openai` or `google`) returns
  `authorize_url` and `state`; complete with `code`, `state` and the same
  `redirect_uri`. Device login serves `google` and `copilot`: poll
  `device_complete` until `ready` (`pending`, `slow_down`, `access_denied`,
  `expired`). Browser login for `copilot` fails with `flow_unsupported`.
- `auth/login/provision_api_key` exchanges an Anthropic Console OAuth access
  token for a stored API key (`oauth_to_api_key`).
- Login, cancel and status also accept MCP server targets (`{"mcp": {...}}`)
  and connector slots (`{"connector": {...}}`); MCP targets are described in
  `references/mcp_tools_skills_mcp.md` ("MCP OAuth") and connector clients
  under "Connector OAuth clients" below. MCP and connector status are
  RPC-only.
- Errors: RPC puts the typed reason in `error.data.reason`, REST in the body
  `reason`. Branch on it (Python `auth_error_reason(error)`, TypeScript
  `authErrorReason(error)`), never on text. Binding-login reasons include
  `invalid_target`, `realm_not_found`, `binding_not_found`,
  `binding_invalid`, `binding_inherited`, `flow_unsupported`,
  `attempt_missing`, `attempt_mismatch`, `device_poll_in_progress`,
  `device_code_already_admitted`, `device_expiry_invalid`,
  `reauth_required`, `authorization_required`, `callback_unavailable`,
  `upstream_failure`, `configuration_invalid`, `infrastructure` and the four
  `credential_*` reasons; MCP and connector targets add their own (for
  example `account_selection_required`, `slot_occupied`).
- Native Rust hosts drive logins with `HostAuthService` (`login_start`,
  `login_complete`, `device_start`, `device_poll`, `status`, `logout`) and
  share one persistence with the factory:

```rust
let persistence = TokenStoreBackend::default_auto()?.open_with_refresh_authority()?;
let runtime = Arc::new(meerkat_runtime::MeerkatMachine::ephemeral());
let factory = AgentFactory::new(sessions_root)
    .with_provider_auth_persistence(persistence.clone());
let auth = HostAuthService::new(persistence, runtime.provider_auth_runtime_authority());
```

## Connector OAuth clients

Connector OAuth stores a SaaS credential (not an LLM or MCP credential) in a
realm slot. Its OAuth clients are realm config managed by:

```bash
rkat auth connector-client set drive --realm tenant-a \
  --issuer https://accounts.google.com --client <CLIENT_ID> \
  --client-secret --secret-stdin --protocol google-v1
rkat auth connector-client rotate-secret|remove <ID>
rkat auth connector-client list
```

`set` writes `[realm.<id>.connector_client.<profile>]` into that realm's own
config document (`issuer`, `client`,
`client_auth` `none` or `client_secret_post`, `authorize_params`,
`resource_indicator`, `protocol`, `registered_redirect_uris`; flags
`--authorize-param NAME=VALUE`, `--no-resource-indicator`,
`--redirect-uri`). The secret is read from stdin, never the command line, and
kept in the credential store. Built-in protocols: `slack-user-v1`,
`salesforce-v1`, `google-v1`.

## Errors and troubleshooting

| Symptom | Cause and fix |
|---|---|
| Plain `rkat run` ignores a login | The run's model belongs to another provider (the default is `gpt-6-astra`). Use `--auth-binding global:<binding>` or a model of the logged-in provider. |
| `no credential binding for self-hosted server '<id>'` | Add a self-hosted binding whose backend has `server = "<id>"`. |
| `reauth_required` | The stored refresh failed permanently; run `rkat auth login <provider>` again. |
| `binding_inherited` | An RPC/REST credential write addressed a child realm for an inherited binding; address the realm that defines it (`rkat auth bindings`). |
| `credential_access_pending` / `credential_access_denied` | Keychain-backed hosts only: the OS keychain is busy with that credential, or access was denied (not retried until an interactive login, save or clear). |

## Not available or superseded

- There is no `auth/profile/test` RPC method; use `rkat auth test` or REST
  `POST /auth/bindings/{binding_id}/test`.
- `rkat auth login gemini` is Gemini Code Assist OAuth, not a Gemini API key;
  for an API key use `GEMINI_API_KEY` or `--non-interactive`.
- MCP OAuth credentials are per realm since 0.9.0; log in once per realm with
  `rkat mcp login`.
