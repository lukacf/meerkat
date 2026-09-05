# Changelog

All notable changes to this project will be documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/).

**Versioning policy (pre-1.0):** meerkat 0.x PATCH releases may contain
breaking public-API changes - this project ships deliberate clean breaks
instead of compatibility shims. Downstreams must EXACT-PIN the meerkat crate
family with an exact `=X.Y.Z` Cargo requirement and bump deliberately. Every
release that breaks public API declares it under a `### Breaking` heading
naming the changed signatures.

**What the `semver-breaks` gate actually enforces**, so this file does not
claim more than it measures: it runs cargo-semver-checks over the publishable
workspace against the published baselines, and it fails the release unless
(1) every crate the release publishes was reached by the run, (2) every break
the tool reports is NAMED in the pending section's `### Breaking` body at the
granularity of the individual finding, and (3) that pending section is stamped
`## [VERSION] - DATE` against the version being released. "Named" means the
symbols of the finding appear in the `### Breaking` body: a type gaining a
field and the same type losing a derive are two findings, and naming one does
not declare the other.

Behaviour-only breaks - a public signature that keeps its shape and changes
what it does - are invisible to cargo-semver-checks and therefore invisible to
the gate. They are declared by hand and tagged inline, and nothing enforces
them.

## [Unreleased]

### Breaking

- **`meerkat_llm_core::LlmError` gained the `QuotaExhausted { message }`
  variant** (`LlmError::QuotaExhausted`, an `enum_variant_added` finding). The
  enum is not `#[non_exhaustive]`, so exact-pinned Rust consumers that match
  `LlmError` exhaustively must add an arm; every in-workspace match and
  MobKit's `classify_llm_error` matches already carry a wildcard.
  `LlmError::from_http_status` returns the new variant for a 429, 400 or 402
  body whose provider code, documented message prefix, or Google
  `QuotaFailure` detail names exhausted quota (see Fixed).

### Billing-affecting default change

- **Anthropic prompt caching is automatic by default again on the Anthropic
  API, Vertex, and Foundry.** 0.8.22 (`3c7c8c1cb`) replaced the per-backend
  default with a blanket `disabled` and inverted the test that pinned it,
  while `docs/rust/advanced.mdx` kept promising `cache_control: automatic`; an
  operator reading the docs believed caching was on and paid the full input
  price on every turn. The provider runtime now derives the default from
  backend capability: `automatic` wherever the backend supports Anthropic's
  request-wide breakpoint, `disabled` on Amazon Bedrock and the GitHub Copilot
  backend (both reject an explicit `automatic` locally; manual `system_prefix`
  stays available), and `AnthropicClientBuilder` starts from `automatic` with
  automatic support assumed; every configured backend, the plain API-key path
  included, is built through the same two capability helpers, so the runtime
  has one authority for this default. A direct `AnthropicClientBuilder` user
  who declares `automatic_cache_control_supported(false)` for a custom backend
  without also setting a non-automatic `default_cache_control` now fails the
  first request locally with `invalid_request` naming the fix (the old
  `disabled` default made that declaration inert). A profile that pins only
  `cache_ttl` now rides the automatic default instead of failing its first
  call. Behaviour change
  for exact-pinned hosts upgrading from any release since 0.8.22: Anthropic
  sessions that never set `cache_control` start paying five-minute cache
  writes (1.25x the input rate) and reading cache hits (0.1x) again, visible
  through `Usage.cache_creation_tokens` / `cache_read_tokens`. Opt out per
  profile or request with the nested form; a flat `cache_control` key fails
  the whole definition parse:

  ```toml
  provider_params = { provider_tag = { provider = "anthropic", cache_control = "disabled" } }
  ```
### Added

- **A "Building And Deploying" guide (`docs/guides/deploying.mdx`) now states
  what a from-source build costs and what the prebuilt binaries guarantee.**
  Nothing public said that `cargo install rkat` compiles at Cargo's default
  `opt-level = 3` with no memory pins (the workspace ships no
  `[profile.release]`, and a workspace profile would not reach `cargo install`
  anyway), that the release lane's own low-memory levers (`CARGO_BUILD_JOBS`,
  per-package `opt-level` pins) exist, or that Linux release binaries are built
  inside `buildpack-deps:bullseye` behind a glibc 2.31 portability gate. The
  guide records measured RAM, time, and disk for a cold release build of
  `rkat`, the consumer-side `CARGO_PROFILE_RELEASE_*` and `.cargo/config.toml`
  overrides that `cargo install` does honor, a two-stage Dockerfile that pins
  one Debian release in both stages so the glibc the binary links against is
  the glibc it runs on, and the release workflow's binary provenance and
  portability guarantees.

### Fixed

- **Member retirement no longer fails when a runtime teardown outlives the
  2 s caller grace** (#1104). The retirement archive path disposed a terminal
  runtime registration through the grace-bounded unregister API, so a
  coordinator-owned teardown that was still completing under load surfaced as
  `UnregisterInProgress` and `retire_member` returned a hard
  `SharedRetirementFailure` although the saga finished moments later. The
  disposal now awaits the exact registration's teardown to terminal completion
  through the new
  `MeerkatMachine::unregister_terminal_session_registration_until_terminal_if_current`
  (same admission rules, no outer bound; dropping the caller never cancels the
  saga). The grace-bounded API is unchanged for existing callers.
- **`MeerkatMachine::stop_runtime_executor_until_terminal_if_current`** is the
  exact-witness variant of `stop_runtime_executor` for callers that must act
  on the STOPPED state (#1104): it awaits the owned stop cleanup coordinator
  to terminal completion instead of returning `RuntimeStopInProgress` at the
  2 s caller grace, and a stale registration witness is an idempotent
  `Ok(false)` that never reaches a same-`SessionId` replacement.
  `stop_runtime_executor` keeps its grace for existing callers.
- **Session history and transcript-revision reads no longer fail when a
  head-canonical writer commits twice while a reader is looking** (#1104).
  `PersistentSessionService` retried a `TranscriptRevisionConflict` on an
  observation load exactly once; a second commit between the reads surfaced
  the typed conflict to `read_history` callers, which under a saturated
  machine happened to a poller reading a session mid-resume. Observation
  loads now re-read up to 8 attempts (counted, not timed), each under the
  runtime turn finalization guard, and surface the conflict unchanged only
  once the budget is spent. A conflict here was never a torn snapshot, only
  writer progress between two reads.
- **`JsonlStore` writers no longer time out after 5 s of polling for the
  per-session write lock under contention** (#1104). The lock was acquired by
  polling `try_lock_exclusive` every 10 ms with a 5 s deadline; a polled wait
  has no fairness, so with several writers each holding the lock for a full
  rewrite plus `sync_all`, one waiter could starve past the deadline on a
  saturated machine and fail its save with an internal error. The lock is now
  taken with a blocking, kernel-queued `lock_exclusive` on a blocking thread,
  behind a 60 s liveness bound that only a live holder that never releases
  can hit (a crashed holder's lock is released by the kernel).
- **An exhausted provider account fails in one round trip instead of after
  the rate-limit retry window.** `LlmError::from_http_status` mapped every
  429 to the retryable `RateLimited` class, so a key whose account had no
  quota left (OpenAI `429` with `error.code = "insufficient_quota"`) was
  retried three times behind the 30-second rate-limit floor and surfaced only
  after roughly 90 seconds of `retrying` events, as `llm_rate_limited`. The
  provider error mapping now reads the body: OpenAI `insufficient_quota`,
  `credit_balance_exhausted`, the organization/project spend-limit and
  usage-limit codes and the legacy `billing_hard_limit_reached`; Anthropic's
  monthly spend cap (`rate_limit_error` with `details.error_code =
  "enforced_spend_limit_reached"`), its self-set spend limit `400` and HTTP
  `402 billing_error`; Gemini `quota_exceeded` and a `RESOURCE_EXHAUSTED`
  `QuotaFailure` on a daily window or a zero entitlement all map to the new
  non-retryable `LlmError::QuotaExhausted`. The OpenAI Responses stream
  error, realtime text adapter and live adapter paths and the Anthropic
  stream error path classify the same codes. Ordinary per-minute rate limits
  are unchanged and keep their `Retry-After` hint. On the wire the failure is
  `llm_provider_error` with `non_retryable` retryability and
  `details.class = "quota_exhausted"` as the machine-readable discriminator
  (`details.message` carries the provider body verbatim). Its
  `provider_error_kind` is still `invalid_request`: the schema-emitted
  provider error kind vocabulary is unchanged in this release, so for this
  class the kind is a known misnomer until a dedicated `quota_exhausted` kind
  lands with a schema regeneration (tracked follow-up). Consumers branch on
  `details.class`, not on the kind and not on the message text. Behaviour
  change for exact-pinned hosts: a quota-exhausted turn now emits `run_failed`
  within one round trip with no `retrying` events, and its reason is
  `llm_provider_error` rather than `llm_rate_limited`. The new `LlmError`
  variant is declared under Breaking.
- **Gemini and OpenAI-compatible requests now accept every built-in tool
  schema.** Gemini's `FunctionDeclaration.parameters` is an OpenAPI-subset
  `Schema` proto that rejects any undeclared field with `400 INVALID_ARGUMENT`,
  and the client lowered tool schemas with a denylist that had been patched
  keyword by keyword since 0.4.5 and still let `not`, object-variant `oneOf`
  and `allOf` residue (`allOf: [{}]` after conditional stripping) through. A
  Gemini session carrying `workgraph_claim` (root-level `not`, since 0.8.22),
  `workgraph_attention_reassign` or `workgraph_policy_escalate` (object
  `oneOf`), the default-on schedule tools (object `oneOf` plus
  `allOf`/`if`/`then`), or the `council` agent tool (schemars `oneOf`, since
  0.8.31, so every mob-enabled Gemini member) failed on its first LLM call with
  a non-retryable `invalid_request`. The Gemini lowering now keeps the existing
  `$ref` inlining and const/type-array/nullable passes, rewrites a surviving
  `oneOf` as `anyOf`, folds `allOf` members into their parent (members emptied
  by conditional stripping are dropped), and then reduces every node to a
  positive allowlist of the Schema proto fields; `format` values pass through
  untouched. JSON Schema booleans, which the proto cannot parse at all, are
  lowered too: `true` (the shape schemars emits for `serde_json::Value` fields
  such as the `council` merge arguments and `mob_wire` peers) becomes the
  empty Schema, and `false` is expressed by dropping the property, union
  branch or item shape it guarded. A tuple-form `items` list (or
  `prefixItems`) collapses into one union item schema because `Schema.items`
  is a single message, and `enum` members are stringified because
  `Schema.enum` is `repeated string` (a `null` member becomes `nullable`). On
  the OpenAI side every emission site (Responses tools, Chat
  Completions tools behind the `openai_compatible` transport, the realtime
  text adapter and live tools) now runs one shared normalizer
  (`meerkat_openai::normalize_openai_tool_parameters_schema`) that inlines
  local `$ref`s, drops a root-level `not` and a root-level `enum`, folds
  root-level `oneOf`/`anyOf`/`allOf` object members into
  `properties`/`required` (literal discriminators from several variants merge
  into one `enum`), and declares an untyped root `object`; nothing below the
  root is rewritten because OpenAI accepts nested combinators. A tool whose
  root declares a non-object `type` (a bare-enum root, which no provider's
  function-parameter validator accepts) is refused with a typed
  `invalid_request` naming the tool instead of being sent.
  `workgraph_claim` no longer expresses the `lease_seconds`/`lease_expires_at`
  exclusivity as a schema-level `not`: both property descriptions state it and
  the claim machine remains the only enforcement point. Behaviour change for
  exact-pinned hosts: Gemini, OpenAI Responses (api.openai.com hosts
  included) and OpenAI-compatible Chat Completions models receive a lowered
  or normalized form of the schema they were sent before; on the OpenAI paths
  local `$ref` inlining duplicates a definition referenced more than once, so
  schemars-typed tools such as `council` and `delegate` cost more tool-schema
  tokens per request; and Gemini members that failed on their first turn
  start working. `GeminiClient::build_request_body`,
  `OpenAiClient::build_request_body` and
  `OpenAiCompatibleClient::build_chat_completions_body` are now public but
  `#[doc(hidden)]` so the integration matrix can drive the real builders.
- **A profile-carried Anthropic `cache_ttl` survives a per-turn tag merge.**
  `ProviderTag::merge_missing_from` filled every Anthropic knob except
  `cache_ttl`, so a turn or draft tag layered over a profile that pinned
  `cache_ttl = "1h"` silently fell back to the five-minute lifetime while
  keeping the profile's `cache_control`. The fill is now complete. Behaviour
  change for exact-pinned hosts that merge draft tags over profile tags (the
  MobKit bridge does): the profile TTL now reaches the request.
- **Realm `[agent] provider_params` is refused instead of silently ignored.**
  `.rkat/config.toml` accepted an `[agent] provider_params` table (parsed
  fail-closed) that no build path read and that `Config::merge` did not carry,
  so an operator placing a fleet-wide cache policy there saw a clean boot and
  no effect. `Config::merge_toml_str`, `FileConfigStore::get`, and
  `Config::validate` now return a typed `ConfigError::Validation` naming the
  per-profile / per-request `provider_params` carrier and its `provider_tag`
  nesting. Behaviour change for exact-pinned hosts whose realm config already
  carries an inert `[agent] provider_params` table, whether in the head
  `.rkat/config.toml`, a parent realm document, or the user-global
  `~/.rkat/config.toml` tail: that config now fails to load on every
  runtime-backed surface (CLI, rkat-rest, rkat-rpc, rkat-mcp; the last two
  through the next entry) until the table is moved onto a profile or removed.
- **rkat-rpc and rkat-mcp fail startup on a head realm config that does not
  load.** Both binaries read the head realm document with
  `unwrap_or_else(|_| Config::default())`, and rkat-mcp additionally turned a
  `Config::validate` failure into a warn log plus `Config::default()`, so a
  head `.rkat/config.toml` that failed to read, parse, or validate was
  replaced wholesale by defaults (default model, limits, tool toggles,
  `[mob_host]`, auth bindings all dropped) and the process served on a
  configuration the operator never wrote: silently on rkat-rpc, behind a warn
  line on rkat-mcp, while rkat-rest and the CLI refused. The `[agent]
  provider_params` refusal above would have vanished the same way on those
  two surfaces. Both now propagate the typed `ConfigError` and exit before
  serving, matching rkat-rest; the head-document read on rkat-rpc and the
  store open, head read, parent-chain compose, and effective `validate` on
  rkat-mcp all fail closed. The compose step on rkat-mcp previously fell back
  to the head document without inheritance behind a warn line when a parent
  realm document or the user-global `~/.rkat/config.toml` tail failed to
  load, dropping the `global`-owned credential binding, model defaults, and
  every inherited field, after which each `create_session` re-composed the
  same chain and failed with the same error: a live server that could create
  no session. It now propagates the compose error as rkat-rpc and rkat-rest
  do. Behaviour change for exact-pinned hosts whose head, parent, or global
  config is malformed or carries `[agent] provider_params`: rkat-rpc and
  rkat-mcp now refuse to start and print the error instead of booting on
  defaults or on the head document alone. rkat-rest's
  startup-log re-read of the head document carried the same
  `unwrap_or_else(|_| Config::default())` behind its already fail-closed
  bootstrap read; it now propagates the `ConfigError` too, so the binary has
  one read path with one failure mode (no observable change: the bootstrap
  read had already refused).
- **`MobDefinition::from_toml` refuses a profile-level `system_prompt` and
  returns `MobError`.** `Profile` cannot carry `deny_unknown_fields` (it sits
  under the untagged `ProfileBinding`, where a rejected key surfaces as an
  opaque "did not match any variant" error, and it is the persisted profile
  shape), so a `system_prompt = "..."` line under `[profiles.<name>]` was
  dropped silently and the member ran on the default prompt; MobKit's own
  fixtures carried the inert key. `from_toml` now compares each inline profile
  table against the new `Profile::FIELD_NAMES` (kept honest by a serde-derived
  drift test) and refuses `system_prompt`, `prompt`, and `instructions`, the
  closed `UnsupportedProfileKey` list, with `MobError::UnsupportedProfileKey`,
  whose message names the profile, the key, and where the concept lives: a
  profile has no system prompt, the member prompt is `profile.skills` resolved
  against an inline or path `[skills.<id>]` table, and identity-first hosts
  may set `DurableAgentSpec.additional_instructions` or a customizer's
  `draft.system_prompt`. A realm-reference table (`realm_profile = "..."`) is
  refused for the same keys, because the untagged binding otherwise absorbs
  them silently. The check runs on the TOML path only (`from_toml`,
  `parse_toml`); a `MobDefinition` deserialized from JSON is not inspected.
  Behaviour change for exact-pinned hosts: a definition that booted with the
  inert key now fails to load until the key moves into a `[skills.<id>]`
  table. Signature changes named for the gate:
  `MobDefinition::from_toml` returns `Result<MobDefinition, MobError>` instead
  of `Result<MobDefinition, toml::de::Error>`; `MobError` gained the variants
  `DefinitionParse(toml::de::Error)` (with `From<toml::de::Error>`) and
  `UnsupportedProfileKey { profile, key }`; `DiagnosticCode` gained
  `UnknownProfileKey`. New public items: `Profile::FIELD_NAMES`,
  `ToolConfig::FIELD_NAMES`, `ProfileBinding::REALM_REF_FIELD_NAMES`,
  `meerkat_mob::UnsupportedProfileKey`, `MobDefinition::parse_toml`,
  `ParsedMobDefinition`, and `UnknownProfileKeys`.

### Changed

- **Mob-enabled sessions no longer render mob tool descriptions into the
  system prompt.** The facade appended every tool's description under
  `# Available Tools` even though each provider already receives the same text
  through `ToolDef.description`, so a `tools.mob` member paid the mob family's
  ~16.6 KB of descriptions twice on every request: the 19-tool agent-facing
  mob surface (~15.8 KB) and the 12-tool operator family that meerkat-mob
  composes into the member's external tools (~0.75 KB). The prompt inventory
  now skips every tool whose provenance is `ToolSourceKind::Mob`, whichever
  dispatcher mounts it, the convention exact deferred-catalog dispatchers
  already follow; the tool definitions the model receives are unchanged and
  every non-mob tool family still renders as before. Behaviour change for
  exact-pinned hosts: the system prompt of every mob-enabled session shrinks
  by roughly 16.6 KB, which moves the prompt-cache prefix once after upgrade.
- **The preloaded `workgraph-workflow` skill now states the rules members
  kept getting wrong.** Every mob member with `tools.workgraph` receives this
  skill in its prompt, but the text never said which way a `parent` edge
  points or when to add one, that `workgraph_close` records `completed` when
  `status` is omitted, that `workgraph_list` and `workgraph_snapshot` hide
  terminal items unless `include_terminal` is true, or that a `labels` filter
  requires every listed label; it also said a `blocks` edge is satisfied by any
  terminally resolved blocker when only `completed` satisfies it. The skill now
  spells out the child (`from_id`) to parent (`to_id`) direction and the
  parent join policies, that `status` must be passed explicitly with `failed`
  for a refuted hypothesis and `cancelled` for dropped work, the
  `include_terminal` and match-all `labels` filter semantics, and the
  `completed`-only blocker rule. Tool descriptions and schemas are unchanged.
  Behaviour change for exact-pinned hosts: the preloaded skill text grows by
  about 1.5 KB, which moves the prompt-cache prefix of WorkGraph-capable
  members once after upgrade.
- **The `tools.comms=false` wiring error now carries its remedy.**
  `build_agent_config` rejects a profile with comms disabled because the
  member's identity, roster entry, wiring, and supervisor bridge are keyed on
  its comms name, but `ToolConfig.comms` defaults to `false`, so a profile
  that merely omitted the key was rejected with a message that named no fix.
  The message keeps its `profile '<name>' has tools.comms=false; mob meerkats
  require comms=true` prefix and adds that the default is false so omitting
  the key counts as false, that the fix is `comms = true` under
  `[profiles.<name>.tools]`, and that a member which must not message peers
  keeps comms on and uses `read_only = true` or a per-spawn
  `tool_access_policy` deny list. The default itself is unchanged. The
  `Profile::peer_description` and `ToolConfig::comms` rustdoc now state that
  the description is peer-facing discovery metadata (the `peers` tool and
  `mob.peer_added`), not the member's system prompt, and that comms must be
  true for every profile that spawns a member.
- **Unknown `[profiles.<name>]` keys now warn instead of vanishing.** Every
  key an inline profile table declares that `Profile` does not define (a
  host-private key such as HomeCore's `role_summary`, or a typo) is reported
  once per key as an `unknown_profile_key` warning diagnostic in the
  `validate_definition` shape, and `MobDefinition::from_toml` logs one
  `tracing::warn!` per affected profile naming the profile and its ignored
  keys; parsing continues and the keys are ignored exactly as before. The new
  `MobDefinition::parse_toml` returns the typed definition together with the
  ignored keys and their diagnostics for hosts that surface diagnostics
  structurally rather than through logs. The `tools` sub-table is compared
  against the new `ToolConfig::FIELD_NAMES` the same way, so a typo such as
  `comm = true` is reported as `profiles.<name>.tools.comm` instead of
  silently leaving comms off, and a realm-reference table warns on every key
  other than `realm_profile` (`ProfileBinding::REALM_REF_FIELD_NAMES`). The
  diagnostic is produced by `parse_toml` only: no Meerkat surface emits
  `unknown_profile_key` today, and `validate_definition` cannot, because the
  typed definition it receives no longer holds the dropped keys. A host that
  surfaces validate diagnostics structurally calls `parse_toml` and merges its
  diagnostics with `validate_definition`'s; `from_toml` callers get the log
  line.
- **docs.rs builds of `meerkat`, `meerkat-runtime`, `meerkat-session`,
  `meerkat-mob`, and `meerkat-live` no longer abort in their build scripts.**
  Each script derives its `meerkat-core` bridge symbol suffix by locating the
  core checkout next to its own manifest or through the target tree's dep-info
  files, and exited the build when neither scan found one. docs.rs unpacks the
  crate under test into an isolated workdir with no sibling crates and, since
  its 2026-08 nightly, runs build scripts under a `build/<pkg>/<hash>/out`
  layout that the dep-info scan never matched, so every `meerkat` release since
  0.8.18 (and `meerkat-session` since 0.8.30) failed to document. The scripts
  now declare `cargo:rerun-if-env-changed=DOCS_RS` and, when the lookup fails
  while `DOCS_RS` is set, emit a `cargo:warning` and continue with the fixed
  placeholder suffix `docsrs_unlinked`: rustdoc never links, so the placeholder
  only has to keep the documentation build alive. Every other build still fails
  closed exactly as before, and `meerkat-core` publishes nothing new. The
  alternative of passing the suffix through Cargo `links` metadata
  (`DEP_MEERKAT_CORE_*`) was rejected: the security canary
  `authority_build_scripts_do_not_leak_factory_seal_metadata` forbids it
  because any direct dependent can read that metadata and generate matching
  validator/finalizer symbols. Canary tests compile each build script
  standalone and drive it under a docs.rs-shaped layout: the fallback applies
  only with `DOCS_RS` set and no checkout visible, a visible checkout still wins
  and warns about nothing, the unset case still fails closed, and the suffix
  every dependent derives is byte-identical to the one `meerkat-core` exports.
- **The MobKit docs mirror on docs.rkat.ai can no longer go stale silently.**
  Every `publish-mobkit-docs.yml` run that reached the publication step pushed
  the release's snapshot branch and then died at `gh pr create`, because the
  repository forbids Actions from opening pull requests (two 2026-08-29 runs
  failed earlier, at `make docs-check`); each run went red with nothing that
  named the branch or what to do next, so docs.rkat.ai served MobKit 0.8.22
  from 2026-08-24 until a hand-made pull request published 0.8.30 on
  2026-09-03. The pull-request step now prefers a dedicated
  `MOBKIT_DOCS_PR_TOKEN` secret over `github.token` when the secret exists
  (used for `gh pr create` only, so a token scoped to Pull requests: read and
  write suffices; auto-merge, which needs contents: write, runs under the
  workflow's own token), and a failure after the branch is pushed writes the branch and the
  exact recovery commands to the job summary and opens or updates one tracking
  issue with a stable title (`scripts/report-mobkit-docs-publication-failure.py`)
  instead of failing quietly. Every mirrored page now opens with a stamp naming
  the documented MobKit version and release ref (`scripts/sync-mobkit-docs.py`),
  so a reader can see that a page describes an older release than the one they
  run; `docs/mobkit` was re-synced from the clean `v0.8.30` tag and differs only
  by that stamp. A nightly `mobkit-docs-lag` job
  (`scripts/check-mobkit-docs-lag.py`) compares `docs/mobkit/_source.json` with
  the published, non-draft, non-prerelease MobKit releases and fails when more
  than one release was published after the mirrored one. Separately,
  `scripts/validate-mintlify-docs.py` stopped rejecting the one anchor form
  Mintlify resolves for a slash-containing heading (`#capabilities%2Fget`): it
  lower-cased the whole link anchor, hex digits of the escape included, while
  its own heading slugs keep them upper-case. Negative tests now pin that a
  link to a missing page and a link to a missing heading anchor both fail
  `make docs-check`.
- **`rkat-mcp` now installs a tracing subscriber, so its warnings and errors
  reach stderr instead of vanishing.** The MCP server binary depended on
  `tracing` but never installed a subscriber, so every `tracing::warn!` and
  `tracing::error!` raised in the process was dropped: an invalid realm config
  fell back to defaults with no output, a panicked event task or a terminated
  schedule-host supervisor left no trace, and the documented `verbose`
  parameter of `meerkat_run`/`meerkat_resume` (server-side event logging at
  `info`) never produced a line. The binary now mirrors `rkat-rpc`: an
  `EnvFilter` with an `info` default, `RUST_LOG` overriding it (an unparsable
  value is named on stderr before the default applies), and a formatting layer
  that writes to stderr only, because stdout is the MCP JSON channel. Hosts
  that launch `rkat-mcp` observe new stderr output; stdout framing is
  unchanged.
- `make release-doctor` no longer fails on a healthy main after the release
  workflow is reworded. The "exact-tree pre-tag semver evidence" and "30
  minute tag-to-public SLO" checks now evaluate `release.yml` job and step
  conditions under concrete events (tag push, package recovery, explicit
  historical evidence) through `scripts/check_release_workflow_contract.py`
  instead of grepping literal lines, so folding an `if:` across lines or
  passing `--slo-seconds` as a `${{ }}` expression passes while gating the
  evidence step off tags, rerunning `make semver-breaks` on a tag, or relaxing
  the 1800 second SLO still fails and names the defect.
  `scripts/test-release-doctor-workflow-contract.sh` (pre-push, CI ratchets,
  and `make release-doctor`) proves both halves on fixtures derived from the
  committed workflow, so the doctor and the workflow cannot drift silently.

## [0.8.33] - 2026-09-04

### Added

- Added `gemini-3.8-flash` as the recommended and default Gemini text model,
  cataloged `gemini-3.7-flash` as supported-only, and added recommended
  Anthropic model `claude-fable-5-1`. The previous Gemini 3.5 Flash row remains
  supported without retaining featured/default status.

### Fixed

- Stale live-session discard on `turn/start`, input ingress, and external
  event injection now captures the exact current runtime registration witness
  and awaits its owned unregister teardown to terminal completion instead of
  the ordinary 2-second caller grace. Slow teardown (for example under remote
  execution latency) no longer surfaces `UnregisterInProgress` to the caller
  that is about to rematerialize the session; a same-SessionId replacement
  registration is never joined, and an absent registration is already clean.

## [0.8.32] - 2026-09-02

### Breaking

- **Generated machine vocabularies gained exhaustive variants and shifted
  ordinal/discriminant order.** Exact-pinned consumers must update exhaustive
  matches for `Input`, `InputKind`, `Effect`, `EffectKind`, `TransitionId`,
  `MeerkatMachineInput`, `MeerkatMachineInputVariant`, `MobMachineInput`,
  `MobMachineInputVariant`, `MobMachineEffect`, and
  `MobMachineEffectVariant`. Pre-existing variant ordering changed across
  `InputKind::*`, `EffectKind::*`, `TransitionId::*`,
  `MeerkatMachineInput::*`, `MeerkatMachineInputVariant::*`,
  `MobMachineInput::*`, `MobMachineInputVariant::*`, `MobMachineEffect::*`,
  and `MobMachineEffectVariant::*`. New variants are
  `AdvanceDefinitionEpoch`, `DefinitionEpochAdvanced`,
  `ImportCommittedModelRoutingHandoff`, `ClaimModelRoutingHandoff`,
  `RealizeModelRoutingHandoff`, `DenyModelRoutingHandoff`,
  `ArchiveUnresolvedModelRoutingHandoff`,
  `ImportCommittedModelRoutingHandoffFirstIdle`,
  `ImportCommittedModelRoutingHandoffFirstAttached`,
  `ImportCommittedModelRoutingHandoffFirstRunning`,
  `ImportCommittedModelRoutingHandoffFirstRetired`,
  `ImportCommittedModelRoutingHandoffAlreadyExactIdle`,
  `ImportCommittedModelRoutingHandoffAlreadyExactAttached`,
  `ImportCommittedModelRoutingHandoffAlreadyExactRunning`,
  `ImportCommittedModelRoutingHandoffAlreadyExactRetired`,
  `ClaimModelRoutingHandoffImportedIdle`,
  `ClaimModelRoutingHandoffImportedAttached`,
  `ClaimModelRoutingHandoffImportedRunning`,
  `ClaimModelRoutingHandoffAlreadyClaimedIdle`,
  `ClaimModelRoutingHandoffAlreadyClaimedAttached`,
  `ClaimModelRoutingHandoffAlreadyClaimedRunning`,
  `ClaimModelRoutingHandoffAlreadyRealizedIdle`,
  `ClaimModelRoutingHandoffAlreadyRealizedAttached`,
  `ClaimModelRoutingHandoffAlreadyRealizedRunning`,
  `RealizeModelRoutingHandoffClaimedIdle`,
  `RealizeModelRoutingHandoffClaimedAttached`,
  `RealizeModelRoutingHandoffClaimedRunning`,
  `RealizeModelRoutingHandoffAlreadyRealizedIdle`,
  `RealizeModelRoutingHandoffAlreadyRealizedAttached`,
  `RealizeModelRoutingHandoffAlreadyRealizedRunning`,
  `DenyModelRoutingHandoffPendingIdle`,
  `DenyModelRoutingHandoffPendingAttached`,
  `DenyModelRoutingHandoffPendingRunning`,
  `DenyModelRoutingHandoffAlreadyDeniedIdle`,
  `DenyModelRoutingHandoffAlreadyDeniedAttached`,
  `DenyModelRoutingHandoffAlreadyDeniedRunning`,
  `ArchiveUnresolvedModelRoutingHandoffPendingRetired`,
  `ArchiveUnresolvedModelRoutingHandoffAlreadyArchivedRetired`, and
  `AdvanceDefinitionEpochRunning`.
- **Generated state records gained required fields.** Struct literals for
  `State`, `MeerkatMachineState`, `MobMachineState`, and
  `SessionTurnAdmissionMachineState` must provide the new
  `model_routing_handoff`, `definition_epoch`, and `teardown_authorized`
  fields as applicable.
- **The runtime LLM reconfiguration trait gained required methods.**
  Implementers of `SessionRuntimeLlmReconfigureService` must add
  `live_model_routing_control_history` and
  `commit_model_routing_control_record_durable_first`.
- **A runtime commit-result compatibility method is deprecated.**
  `PreparedRuntimeSessionCommitResult::downstream_projection_required` remains
  callable in 0.8.32 but downstreams should stop using it.
- **Mob definition authority enums gained exhaustive variants.** Consumers
  matching `MobError`, `MobEventKind`, or `MobStoreError` must handle
  `MobDefinitionAuthorityChanged`, `MobDefinitionUpdated`,
  `DefinitionEpochAuthorityRequired`, `MobDefinitionAlreadyCreated`,
  `DefinitionEpochEventHeadConflict`, `MobDefinitionProjectionMismatch`, and
  `DefinitionEpochPersistenceUnavailable`.

### Added

- Added a canonical typed transcript-replay projection across Anthropic,
  Gemini, OpenAI Responses, OpenAI-compatible Responses, and compatible Chat
  dispatch. Replay now preserves provider metadata, tool-result collapse,
  nested notice images, constructor compatibility, and profile-aware image
  capability instead of relying on provider-specific transcript rewriting.
- Added durable Mob definition epochs. `MobCreated` establishes epoch 1 and
  `MobDefinitionUpdated` is the sole successor, committed through
  `MobStorage::update_definition` under generated `MobMachine` CAS authority.
  Sealed definition snapshots and `MobBuilder::for_resume_verified` bind resume
  to the inspected definition, epoch, and event cursor. SQLite commits authority
  and projection atomically, including recovery of the exact legacy
  one-epoch-ahead projection residue; mismatched and split/custom stores remain
  fail-closed with typed errors.
- Added `brain_swap`, a model-authored permanent model switch that takes effect
  before the next input is admitted. The current run remains on its original
  model; committed requests survive restart and resolve through the existing
  model-routing, auth-binding, and account-affinity authority. The tool is a
  `builtins`-category `Mutating` tool and is registered by default only when the
  runtime can prove more than one distinct model is available. Consequently,
  builtins-enabled unrestricted sessions receive it automatically; read-only
  and exact allow/deny policies still gate invocation. In MobKit, an application
  policy constrains it only for members explicitly bound to that policy provider.

### Fixed

- Isolated concurrent Rust release-package verification into per-crate Cargo
  target and toolchain-wrapper lanes, preventing `Text file busy` executable
  races while preserving parallel failure reporting.

## [0.8.31] - 2026-08-28

### Breaking

- **Rust hook contracts gained committed-observation variants and payloads.**
  Exact-pinned downstreams must update exhaustive matches over `HookPoint` and
  struct literals for `HookInvocation`; `SessionRuntimeBindings::__from_runtime_authority`
  also gains the runtime-owned `PostCommitHookDispatcher` argument.
- **Rust auth/provider construction surfaces gained account-aware fields and
  variants.** Exact-pinned downstreams must update exhaustive matches and
  struct literals for `AuthCredentialIdentity`, `CredentialAccountId`,
  `CredentialAccountRef`, `TokenKey`, `ProviderAuthPersistence`,
  `HttpAuthorizationContent`, `HttpAuthorizationReceipt`, `LlmRequest::has_images`,
  `HttpAuthorizer::prepare_request`, `HttpAuthorizer::append_content_headers`,
  `LlmError::AuthorizationRouteChanged`, `LlmProviderErrorKind::AuthorizationRouteChanged`,
  `AgentLlmClient::prepare_request_attempt`, `AgentLlmClient::request_attempt_authority`,
  `AgentLlmRequestAttempt`,
  `RequestAttemptAuthority`, `LlmClient::project_replay_request`,
  `LlmClient::prepared_request_pressure`, `LlmClient::prepared_cache_breakpoints`,
  `LlmClient::stream_prepared`, `LlmReplayProjection`, `PreparedLlmRequest`,
  `LlmRequestRouteWitness`,
  `HttpAuthorizer::authorize_with_receipt`,
  `HttpAuthorizer::observe_response_with_receipt`, `ProviderBinding`, `ProviderBindingConfig`,
  `ResolvedConnection`, `SessionLlmRequestPolicy`, `OAuthProviderIdentity`,
  `PersistedAuthMode`, `LoginStartParams`, `LoginCompleteParams`,
  `DeviceStartParams`, `DeviceCompleteParams`, `WireLoginStart`,
  `WireLoginReady`, `WireDeviceStart`, `WireDeviceCompleteResult`, and
  `OAuthProviderDeclaration::client_secret`, `CredentialAccountBindingCandidates`, and
  generated Web SDK `WireOAuthProvider`; provider-runtime embedders may use
  `ProviderRuntimeCatalog::validate_binding_with_credential_identity`, and
  native hosts gain `HostAuthDeviceStart` / `HostAuthDevicePoll`.
- **Additional Rust provider, bridge, hook, and forked-participant contracts
  changed.** Exact-pinned downstreams must update exhaustive matches, struct
  literals, derives, and helper calls for:
  - `HostAuthError::{BrowserFlowUnsupported, DeviceFlowUnsupported,
    InvalidExpiry}`;
  - `AnthropicProviderRuntime::{UnwindSafe, RefUnwindSafe}`,
    `GoogleProviderRuntime::{UnwindSafe, RefUnwindSafe}`, and
    `OpenAiProviderRuntime::{UnwindSafe, RefUnwindSafe}`;
  - `OAuthTargetValidationError::{BackendProviderMismatch,
    BackendKindMismatch, ProviderMismatch, AuthMethodMismatch, SourceMismatch,
    SharedIssuerRequiresAccountTarget, BindingInvalid}`;
  - `BridgeMaterializePayload::forked_participant_attachment`,
    `BridgeHostBindPayload::delegated_bootstrap_proof`,
    `WireProviderBinding::credential_account`, and
    `BridgeCapabilities::forked_participants`;
  - `BridgeRejectionCause::{InvalidSupervisorSpec, InvalidPeerSpec,
    AddressMismatch, Unsupported, Internal, BindAdmissionOutcomeUnknown,
    StaleFence, StaleCursor, OversizedEvent, HistoryRowTooLarge, Unavailable,
    RuntimeRetirementInProgress, ScopeDenied, SpecDigestMismatch,
    MaterializeBuildRejected, ModelUnresolvable, AuthBindingUnresolvable,
    McpCommandMissing, RealmBackendUnavailable, EnvKeyMissing,
    HostEngineVersionChanged, ModelNotRealtime, LiveAdapterUnavailable,
    LiveTransportUnavailable, LiveChannelAlreadyBound, LiveChannelNotFound,
    LiveTransportUnsupported, ResumeSessionNotFound, CapabilityMissing,
    LaunchModeUnsupported, LaunchModePlacementMismatch,
    SessionOwnershipConflict}`;
  - `ResolvedConnectionTarget::credential_identity`,
    `ProviderBinding::credential_account`,
    `SessionLlmRequestPolicy::credential_identity`,
    `ProviderBindingConfig::credential_account`, and
    `LeaseKey::{identity, realm, binding, profile}`;
  - `LlmProviderErrorKind::{RequestTooLarge, ContentFiltered, ServerError,
    ServerOverloaded, ConnectionReset, Unknown, StreamParseError,
    IncompleteResponse}`;
  - `PersistedAuthMode::{Adc, ComputeAdc, Bedrock, Vertex, Foundry, McpOauth,
    ExternalTokens, ExternalAuthorizer, Command, GithubCopilotOauth}`;
  - `OpenAiBackendKind::Copilot`, `GoogleBackendKind::Copilot`,
    `AnthropicBackendKind::Copilot`,
    `OpenAiAuthMethod::GitHubCopilotOauth`,
    `GoogleAuthMethod::GitHubCopilotOauth`,
    `AnthropicAuthMethod::GitHubCopilotOauth`, and
    `OAuthProviderIdentity::GitHubCopilot`;
  - `HookPoint::{RuntimeInputAccepted, RuntimeInputRejected,
    RuntimeInputDeduplicated, PeerIngressCommitted, PeerEgressCommitted,
    InteractionCompleted}`;
  - `ProviderBindingError::{CredentialAccountRequiresPersistedAuth,
    CredentialAccountContractMismatch,
    CredentialAccountProfileOverrideMismatch}` and
    `ConnectionTargetError::AmbiguousCredentialAccountBindings`;
  - `GOOGLE_CLIENT_SECRET`, `ResolvedConnection::credential_identity`, and
    `LlmError::{ContentFiltered, ContextLengthExceeded, ModelNotFound,
    InvalidApiKey, Unknown, StreamParseError, IncompleteResponse}`;
  - `DurableMarkerProtocol::{credential_kind_field, account_field}`;
  - `MobHostActorConfig::forked_participant_sweep_interval`,
    `MobHostBindingRecord::forked_participant_obligations`,
    `MaterializedMemberRow::forked_participant_attachment`,
    `HostMemberSubstrate::{forked_participant_realm,
    forked_participant_store, forked_participant_source_runtime}`, and
    `HostBindRequest::{delegated_bootstrap_proof, delegated_supervisor}`;
  - `SqliteRealmProfileStore::Clone`;
  - `MobError::{ForkedParticipantSourceIneligible, ForkedParticipantRefused,
    ForkedParticipantResumeRequiresAttachment,
    ForkedParticipantRemoteLeaseUnsupported,
    ForkedParticipantOwnerHostUnavailable,
    ForkedParticipantAttachedSpawnSpecRejected,
    ForkedParticipantAttachmentCustodyUnrecorded,
    ForkedParticipantAttachmentReleaseUnproven}`; and
  - `record_materialized_member`.

### Added

- Added the first-class `council` agent mob tool for bounded cross-context
  deliberation using scoped, expiring forked-participant capabilities,
  source-owned execution context, automatic non-secret remote-host binding,
  explicit result merging, and cleanup/recovery through generated lifecycle
  authority.
- Added typed, observe-only post-commit hooks for runtime input acceptance,
  rejection, and deduplication; peer ingress and egress; and enriched
  interaction completion. Event streams remain the ordered/replay-aware
  transport, while session-owned hook tasks are reaped and aborted on teardown.
- Added native GitHub Copilot device authentication and account-scoped CAPI
  routing for OpenAI Responses/Chat Completions, Anthropic Messages, and
  Gemini Chat Completions, without a Copilot SDK or CLI dependency.

### Fixed

- Made schedule-host shutdown cancellation-safe: cancelled or dropped shutdown
  futures stop in-flight work without later delivery, while awaited shutdown
  retains the durable executor-lease release guarantee.
- Ordered output-only identity convergence projections by storage authority,
  mob, and canonical agent identity so delayed writes cannot regress diagnostics
  or treat presentation timestamps as lifecycle authority.
- Hardened release publication with exact-tree semver evidence, complete binary
  asset/provenance checks, concurrent pre-push isolation, and a guarded
  tag-to-registry critical path.

## [0.8.30] - 2026-08-26

### Breaking

- **Rust public construction, trait, enum, and generated-machine surfaces changed.**
  Downstream exact pins must update constructors and exhaustive matches. The
  following exhaustive symbol ledger names every structural finding reported
  by `cargo-semver-checks` against 0.8.29:
  - `meerkat`: `AdmissionRejectedRevokedChannel`, `AgentBuildConfig`,
               `live_session_has_instruction_activations`, `LiveOpenError`,
               `LiveTruncateCursor`, `output_id`, `PendingPromotionCleanup`,
               `RealtimeSessionOpenProjection`, `RealtimeSessionOpenProjectionError`,
               `reported_playback_prefix`, `SessionMismatch`,
               `SessionRuntimeLlmReconfigureService`, `tool_dispatch_admission`.
  - `meerkat-contracts`: `AssistantTranscriptTruncated`, `CatalogModelEntry`,
                         `execution_identity`, `instruction_activation`, `interaction_id`,
                         `LiveOpenParams`, `LiveTruncateParams`,
                         `mid_conversation_system_messages`, `output_id`, `release_stage`,
                         `reported_playback_prefix`,
                         `supports_mid_conversation_system_messages`, `System`,
                         `WireLiveAdapterObservation`, `WireModelProfile`,
                         `WireResolvedModelCapabilities`, `WireSessionMessage`.
  - `meerkat-core`: `AdmitLiveAssistantPlaybackTarget`, `AdmitLiveInteractionTranscript`,
                    `Application`, `ApplyPendingToolResults`, `ArchiveSessionDocument`,
                    `AssistantPlaybackTargetAdmitted`, `AssistantPlaybackTargetResolved`,
                    `AssistantPlaybackTerminalObserved`, `AssistantTranscriptFinal`,
                    `AssistantTranscriptTruncated`, `AssistantTurnCompleted`,
                    `AssistantTurnInterrupted`, `AuthBinding`,
                    `AuthorizeSessionBuildStatePersist`, `AuthorizeSessionMetadataPersist`,
                    `AuthorizeSessionResumeOverrides`, `CatalogEntry`, `ClassifyDurableTail`,
                    `ClassifyLiveContextCommittedRow`, `ClassifyLiveSessionAuthority`, `Close`,
                    `CommandRejected`, `CompleteLiveInteractionTranscript`,
                    `DurableTailClassified`, `Error`, `ExternalWriteFenceBackoff`,
                    `ExternalWriteFenceConflict`, `GrantAction`, `GrantScope`,
                    `instruction_activation`, `interaction_id`, `LiveAdapterCommand`,
                    `LiveAdapterObservation`, `LiveAssistantPlaybackFinalObserved`,
                    `LiveAssistantPlaybackFinalRecovered`,
                    `LiveAssistantPlaybackTargetAdmitted`,
                    `LiveAssistantPlaybackTargetRecovered`,
                    `LiveAssistantPlaybackTerminalObserved`,
                    `LiveAssistantPlaybackTerminalRecovered`,
                    `LiveAssistantPlaybackTerminalResolved`,
                    `LiveContextCommittedRowClassified`, `LiveFinalUserTranscriptReconciled`,
                    `LiveInteractionTranscriptAdmitted`, `LiveInteractionTranscriptCompleted`,
                    `LiveProvisionalUserTranscriptStaged`, `LiveSessionAuthorityClassified`,
                    `ManageRuntime`, `ModelCapabilities`, `ModelProfile`, `ModelRegistryEntry`,
                    `ObserveLiveAssistantPlaybackFinal`, `ObserveLiveAssistantPlaybackTerminal`,
                    `PendingContinuationPublicTerminalResolved`, `PendingContinuationResolved`,
                    `RealtimeTranscript`, `RealtimeTranscriptEvent`,
                    `ReconcileLiveFinalUserTranscript`, `RecoverLiveAssistantPlaybackFinal`,
                    `RecoverLiveAssistantPlaybackTarget`,
                    `RecoverLiveAssistantPlaybackTerminal`, `RecoverSessionFromStore`,
                    `RecoverSessionLifecycleTerminal`, `release_stage`,
                    `reported_playback_prefix`, `ResolveLiveAssistantPlaybackOnChannelClose`,
                    `ResolvePendingContinuation`, `ResolveRuntimeCheckpointProjection`,
                    `ResolveSessionDocumentLifecycleMerge`, `RestoreSessionBuildState`,
                    `ReviveArchivedSessionDocument`, `RuntimeCheckpointProjectionResolved`,
                    `SessionArchiveResolved`, `SessionBuildOptions`,
                    `SessionBuildStatePersistAuthorized`, `SessionBuildStateRestoreAuthorized`,
                    `SessionDocumentEffect`, `SessionDocumentInput`,
                    `SessionDocumentLifecycleMergeResolved`, `SessionError`,
                    `SessionLifecycleTerminalRecovered`, `SessionMetadataPersistAuthorized`,
                    `SessionResumeOverridesAuthorized`, `SessionResumeOverridesRejected`,
                    `SessionRevivalResolved`, `SessionStoreRecoverySourceResolved`,
                    `SessionToolResultsApplied`, `StageLiveProvisionalUserTranscript`,
                    `StatusChanged`, `SubmitToolError`, `SubmitToolResult`,
                    `supports_mid_conversation_system_messages`, `SystemMessage`,
                    `SystemPromptUpdateError`, `TargetHasInstructionActivation`,
                    `tool_dispatch_admission`, `ToolCallRequested`, `TranscriptEdit`,
                    `TranscriptRewriteCommitted`, `TruncateAssistantOutput`, `TurnCompleted`,
                    `TurnInterrupted`, `UseAuthBinding`, `UserContentCommitted`.
  - `meerkat-live`: `admit_assistant_playback_target`, `Clone`, `complete_assistant_playback`,
                    `CompleteAssistantPlayback`, `fail_assistant_output_publication`,
                    `LiveChannelId`, `LiveCommandAcceptanceKind`, `LiveProjectionSink`,
                    `LiveWebrtcAnswerAccepted`, `pending_bound_ready`, `RefUnwindSafe`, `Sync`,
                    `UnwindSafe`.
  - `meerkat-llm-core`: `AssistantTranscriptTruncated`, `ExperimentalModelRequiresLiveChannel`,
                        `FactoryError`, `interaction_id`, `RealtimeSessionEvent`.
  - `meerkat-mob`: `LifecycleOperationProgressStalled`, `MobError`, `MobSessionService`,
                   `enqueue_committed_parent_session_boundary_after_runtime_turn`.

    This ledger entry was appended after `v0.8.30` was tagged. The immutable
    tag's changelog therefore omits it; the GitHub Release body links to this
    corrected declaration. Published package source remains the original,
    qualified tag.

    `MobSessionService::enqueue_committed_parent_session_boundary_after_runtime_turn`
    is now required; its `Ok(0)`/`Unsupported` default is removed. A wrapper
    around a persistent inner service must forward to that inner service, or
    answer for itself while guarded by `supports_persistent_sessions()` - never
    return a bare `Ok(0)`. Cargo reports the method through both public trait
    paths, but they are one required implementation change.

    `MobError::LifecycleOperationProgressStalled { intent, member_id, stage }`
    is a new variant. Exhaustive matches on `MobError` must add an arm. It
    replaces fixed explicit-resume expiry with a typed stall naming the member
    and lifecycle stage that stopped making progress.
  - `meerkat-machine-kernels`: `AbandonLiveInteraction`, `AbandonLiveInteractionAttached`,
                               `AbandonLiveInteractionIdle`,
                               `AbandonLiveInteractionPreservingEarlierDelegationAttached`,
                               `AbandonLiveInteractionPreservingEarlierDelegationIdle`,
                               `AbandonLiveInteractionPreservingEarlierDelegationRetired`,
                               `AbandonLiveInteractionPreservingEarlierDelegationRunning`,
                               `AbandonLiveInteractionPreservingEarlierDelegationStopped`,
                               `AbandonLiveInteractionRetired`, `AbandonLiveInteractionRunning`,
                               `AbandonLiveInteractionStopped`,
                               `AbandonLiveInteractionWithDelegationCancellationAttached`,
                               `AbandonLiveInteractionWithDelegationCancellationIdle`,
                               `AbandonLiveInteractionWithDelegationCancellationRetired`,
                               `AbandonLiveInteractionWithDelegationCancellationRunning`,
                               `AbandonLiveInteractionWithDelegationCancellationStopped`,
                               `AbandonLiveOpenAdmission`, `AbandonLiveOpenAdmissionAttached`,
                               `AbandonLiveOpenAdmissionIdle`,
                               `AbandonLiveOpenAdmissionRetired`,
                               `AbandonLiveOpenAdmissionRunning`,
                               `AbandonLiveOpenAdmissionStopped`, `AddDirectPeerEndpoint`,
                               `AddDirectPeerEndpointAttached`, `AddDirectPeerEndpointIdle`,
                               `AddDirectPeerEndpointRunning`,
                               `AdmitLiveAssistantPlaybackTarget`, `AdmitLiveBridgeOperation`,
                               `AdmitLiveBridgeOperationExactReplayAttached`,
                               `AdmitLiveBridgeOperationExactReplayIdle`,
                               `AdmitLiveBridgeOperationExactReplayRunning`,
                               `AdmitLiveBridgeOperationFreshAttached`,
                               `AdmitLiveBridgeOperationFreshIdle`,
                               `AdmitLiveBridgeOperationFreshRunning`,
                               `AdmitLiveBridgeOperationProtocolDriftAttached`,
                               `AdmitLiveBridgeOperationProtocolDriftIdle`,
                               `AdmitLiveBridgeOperationProtocolDriftRunning`,
                               `AdmitLiveDelegation`, `AdmitLiveDelegationAttached`,
                               `AdmitLiveDelegationIdle`, `AdmitLiveDelegationRunning`,
                               `AdmitLiveInteraction`, `AdmitLiveInteractionAttached`,
                               `AdmitLiveInteractionDelegation`,
                               `AdmitLiveInteractionDelegationAttached`,
                               `AdmitLiveInteractionDelegationIdle`,
                               `AdmitLiveInteractionDelegationRunning`,
                               `AdmitLiveInteractionIdle`, `AdmitLiveInteractionRunning`,
                               `AdmitLiveInteractionTranscript`,
                               `AdvanceLiveContextCanonicalCoverage`,
                               `AdvanceLiveContextCanonicalCoverageAttached`,
                               `AdvanceLiveContextCanonicalCoverageIdle`,
                               `AdvanceLiveContextCanonicalCoverageRunning`,
                               `AdvanceSessionContext`, `AdvanceSessionContextAttached`,
                               `AdvanceSessionContextIdle`, `AdvanceSessionContextRetired`,
                               `AdvanceSessionContextRunning`, `AdvanceSessionContextStopped`,
                               `ApplyMobPeerOverlay`, `ApplyMobPeerOverlayAttached`,
                               `ApplyMobPeerOverlayIdle`, `ApplyMobPeerOverlayRunning`,
                               `ApplyPendingToolResults`, `ArchiveSessionDocument`,
                               `ArchiveSessionDocumentActive`,
                               `ArchiveSessionDocumentAlreadyArchived`,
                               `ArchiveSessionDocumentCompleteRetire`, `AttachMobIngress`,
                               `AttachMobIngressAttached`, `AttachMobIngressIdle`,
                               `AttachMobIngressRetired`, `AttachMobIngressRunning`,
                               `AttachMobIngressStopped`,
                               `AttachMobIngressTerminalSupervisorCleanup`,
                               `AttachSessionIngress`, `AttachSessionIngressAttached`,
                               `AttachSessionIngressIdle`, `AttachSessionIngressRetired`,
                               `AttachSessionIngressRunning`, `AttachSessionIngressStopped`,
                               `AttachSessionIngressTerminalSupervisorCleanup`,
                               `AuthorizeInteractionTerminalOutboxAdoption`,
                               `AuthorizeLiveActiveChannelControl`,
                               `AuthorizeLiveActiveChannelControlAttached`,
                               `AuthorizeLiveActiveChannelControlIdle`,
                               `AuthorizeLiveActiveChannelControlRunning`,
                               `AuthorizeLiveBridgeEffect`, `AuthorizeLiveBridgeEffectAttached`,
                               `AuthorizeLiveBridgeEffectIdle`,
                               `AuthorizeLiveBridgeEffectRunning`,
                               `AuthorizeLiveBridgeExecutionStart`,
                               `AuthorizeLiveBridgeExecutionStartAttached`,
                               `AuthorizeLiveBridgeExecutionStartIdle`,
                               `AuthorizeLiveBridgeExecutionStartRunning`,
                               `AuthorizeLiveBridgeSubmission`,
                               `AuthorizeLiveBridgeSubmissionAttached`,
                               `AuthorizeLiveBridgeSubmissionIdle`,
                               `AuthorizeLiveBridgeSubmissionRunning`,
                               `AuthorizeLiveConsequentialEffect`,
                               `AuthorizeLiveConsequentialEffectAttached`,
                               `AuthorizeLiveConsequentialEffectIdle`,
                               `AuthorizeLiveConsequentialEffectRunning`,
                               `AuthorizeLiveContextAppend`,
                               `AuthorizeLiveContextAppendAttached`,
                               `AuthorizeLiveContextAppendIdle`,
                               `AuthorizeLiveContextAppendRunning`,
                               `AuthorizeLiveDelegationResultDelivery`,
                               `AuthorizeLiveDelegationResultDeliveryAttached`,
                               `AuthorizeLiveDelegationResultDeliveryIdle`,
                               `AuthorizeLiveDelegationResultDeliveryRunning`,
                               `AuthorizeLiveDelegationResultRelease`,
                               `AuthorizeLiveDelegationResultReleaseAttached`,
                               `AuthorizeLiveDelegationResultReleaseIdle`,
                               `AuthorizeLiveDelegationResultReleaseRunning`,
                               `AuthorizeLiveDelegationTranscriptTerminalCancellation`,
                               `AuthorizeLiveDelegationTranscriptTerminalCancellationAttached`,
                               `AuthorizeLiveDelegationTranscriptTerminalCancellationIdle`,
                               `AuthorizeLiveDelegationTranscriptTerminalCancellationRunning`,
                               `AuthorizeLiveDelegationWorkerRetirement`,
                               `AuthorizeLiveDelegationWorkerRetirementAttached`,
                               `AuthorizeLiveDelegationWorkerRetirementIdle`,
                               `AuthorizeLiveDelegationWorkerRetirementRetired`,
                               `AuthorizeLiveDelegationWorkerRetirementRunning`,
                               `AuthorizeLiveDelegationWorkerRetirementStopped`,
                               `AuthorizeLiveDelegationWorkerStart`,
                               `AuthorizeLiveDelegationWorkerStartAttached`,
                               `AuthorizeLiveDelegationWorkerStartIdle`,
                               `AuthorizeLiveDelegationWorkerStartRunning`,
                               `AuthorizeSessionBuildStatePersist`,
                               `AuthorizeSessionMetadataPersist`,
                               `AuthorizeSessionResumeOverrides`,
                               `AuthorizeSessionResumeOverridesAcceptRecomputeProvider`,
                               `AuthorizeSessionResumeOverridesAcceptRecomputeProviderWithSelfHostedOverride`,
                               `AuthorizeSessionResumeOverridesAcceptRetainStored`,
                               `AuthorizeSessionResumeOverridesAcceptRetainStoredWithSelfHostedOverride`,
                               `AuthorizeSessionResumeOverridesAcceptUseOverride`,
                               `AuthorizeSessionResumeOverridesAcceptUseOverrideWithSelfHostedOverride`,
                               `AuthorizeSessionResumeOverridesRejectBuildOnlyAfterFirstTurn`,
                               `AuthorizeSessionResumeOverridesRejectProviderRequiresModel`,
                               `AuthorizeSupervisor`, `AuthorizeSupervisorAttached`,
                               `AuthorizeSupervisorIdle`, `AuthorizeSupervisorMobPeerOverlay`,
                               `AuthorizeSupervisorMobPeerOverlayAttached`,
                               `AuthorizeSupervisorMobPeerOverlayIdle`,
                               `AuthorizeSupervisorMobPeerOverlayRunning`,
                               `AuthorizeSupervisorRetired`, `AuthorizeSupervisorRunning`,
                               `AuthorizeSupervisorStopped`, `BindLiveContextRecoveryChannel`,
                               `BindLiveContextRecoveryChannelAttached`,
                               `BindLiveContextRecoveryChannelIdle`,
                               `BindLiveContextRecoveryChannelRunning`,
                               `BindLiveDelegationResultRecoveryChannel`,
                               `BindLiveDelegationResultRecoveryChannelAttached`,
                               `BindLiveDelegationResultRecoveryChannelIdle`,
                               `BindLiveDelegationResultRecoveryChannelRunning`,
                               `BindLiveExecutionChannel`, `BindLiveExecutionChannelAttached`,
                               `BindLiveExecutionChannelIdle`,
                               `BindLiveExecutionChannelRunning`, `BindSupervisor`,
                               `BindSupervisorAttached`, `BindSupervisorIdle`,
                               `BindSupervisorRetired`, `BindSupervisorRunning`,
                               `BindSupervisorStopped`, `CancelLiveBridgeOperation`,
                               `CancelLiveBridgeOperationAttached`,
                               `CancelLiveBridgeOperationIdle`,
                               `CancelLiveBridgeOperationRunning`, `CancelWaitAllAttached`,
                               `CancelWaitAllIdle`, `CancelWaitAllRetired`,
                               `CancelWaitAllRunning`, `CancelWaitAllStopped`,
                               `ClaimLiveBridgeSubmissionAttempt`,
                               `ClaimLiveBridgeSubmissionAttemptAttached`,
                               `ClaimLiveBridgeSubmissionAttemptIdle`,
                               `ClaimLiveBridgeSubmissionAttemptRunning`,
                               `ClassifyAssistantOutputEmptyTerminalAttached`,
                               `ClassifyAssistantOutputEmptyTerminalIdle`,
                               `ClassifyAssistantOutputEmptyTerminalRunning`,
                               `ClassifyAssistantOutputProceedAttached`,
                               `ClassifyAssistantOutputProceedIdle`,
                               `ClassifyAssistantOutputProceedRunning`,
                               `ClassifyCallTimeoutRetryableAttached`,
                               `ClassifyCallTimeoutRetryableIdle`,
                               `ClassifyCallTimeoutRetryableRunning`,
                               `ClassifyCallTimeoutTerminalAttached`,
                               `ClassifyCallTimeoutTerminalIdle`,
                               `ClassifyCallTimeoutTerminalRunning`, `ClassifyDurableTail`,
                               `ClassifyDurableTailAmbiguous`, `ClassifyDurableTailCompleted`,
                               `ClassifyDurableTailRepairable`,
                               `ClassifyLiveContextCommittedRow`,
                               `ClassifyLiveSessionAuthority`,
                               `ClassifyLiveSessionAuthorityDurableArchived`,
                               `ClassifyLiveSessionAuthorityDurableRevision`,
                               `ClassifyLiveSessionAuthorityDurableUncommitted`,
                               `ClassifyLiveSessionAuthorityLive`,
                               `ClassifyLlmFailureRecoveryExhaustedAttached`,
                               `ClassifyLlmFailureRecoveryExhaustedIdle`,
                               `ClassifyLlmFailureRecoveryExhaustedRunning`,
                               `ClassifyLlmFailureRecoveryFatalAttached`,
                               `ClassifyLlmFailureRecoveryFatalIdle`,
                               `ClassifyLlmFailureRecoveryFatalRunning`,
                               `ClassifyLlmFailureRecoveryRecoverAttached`,
                               `ClassifyLlmFailureRecoveryRecoverIdle`,
                               `ClassifyLlmFailureRecoveryRecoverRunning`,
                               `ClassifyRecoveredTerminalCompletionBatch`,
                               `ClassifyRecoveredTerminalCompletionBatchBlockedAttached`,
                               `ClassifyRecoveredTerminalCompletionBatchBlockedIdle`,
                               `ClassifyRecoveredTerminalCompletionBatchBlockedInitializing`,
                               `ClassifyRecoveredTerminalCompletionBatchBlockedRetired`,
                               `ClassifyRecoveredTerminalCompletionBatchBlockedRunning`,
                               `ClassifyRecoveredTerminalCompletionBatchBlockedStopped`,
                               `ClassifyRecoveredTerminalCompletionBatchDiscardUnrecoverableAttached`,
                               `ClassifyRecoveredTerminalCompletionBatchDiscardUnrecoverableIdle`,
                               `ClassifyRecoveredTerminalCompletionBatchDiscardUnrecoverableInitializing`,
                               `ClassifyRecoveredTerminalCompletionBatchDiscardUnrecoverableRetired`,
                               `ClassifyRecoveredTerminalCompletionBatchDiscardUnrecoverableRunning`,
                               `ClassifyRecoveredTerminalCompletionBatchDiscardUnrecoverableStopped`,
                               `ClassifyRecoveredTerminalCompletionBatchRecoverAttached`,
                               `ClassifyRecoveredTerminalCompletionBatchRecoverIdle`,
                               `ClassifyRecoveredTerminalCompletionBatchRecoverInitializing`,
                               `ClassifyRecoveredTerminalCompletionBatchRecoverRetired`,
                               `ClassifyRecoveredTerminalCompletionBatchRecoverRunning`,
                               `ClassifyRecoveredTerminalCompletionBatchRecoverStopped`,
                               `ClassifyTurnTerminalCauseClassBudgetExhaustedIdle`,
                               `ClassifyTurnTerminalCauseClassMissingIdle`,
                               `ClassifyTurnTerminalCauseClassOtherFailureIdle`,
                               `ClassifyTurnTerminalCauseClassRetryExhaustedIdle`,
                               `ClassifyTurnTerminalCauseClassStructuredOutputValidationFailedIdle`,
                               `ClassifyTurnTerminalCauseClassTimeBudgetExceededIdle`,
                               `ClassifyTurnTerminalCauseClassUnknownIdle`,
                               `ClassifyTurnTerminalityNonTerminalAttached`,
                               `ClassifyTurnTerminalityNonTerminalIdle`,
                               `ClassifyTurnTerminalityNonTerminalRunning`,
                               `ClassifyTurnTerminalityTerminalAttached`,
                               `ClassifyTurnTerminalityTerminalIdle`,
                               `ClassifyTurnTerminalityTerminalRunning`, `ClearLocalEndpoint`,
                               `ClearLocalEndpointAttached`, `ClearLocalEndpointIdle`,
                               `ClearLocalEndpointRunning`, `ClearTurnToolOverlay`,
                               `ClearTurnToolOverlayAttached`, `ClearTurnToolOverlayIdle`,
                               `ClearTurnToolOverlayRetired`, `ClearTurnToolOverlayRunning`,
                               `ClearTurnToolOverlayStopped`, `CloseSurfaceConnection`,
                               `CommitDeferredNames`, `CommitDeferredNamesAttached`,
                               `CommitDeferredNamesIdle`, `CommitDeferredNamesRetired`,
                               `CommitDeferredNamesRunning`, `CommitDeferredNamesStopped`,
                               `CommitVisibilityFilter`, `CommitVisibilityFilterAttached`,
                               `CommitVisibilityFilterIdle`, `CommitVisibilityFilterRetired`,
                               `CommitVisibilityFilterRunning`, `CommitVisibilityFilterStopped`,
                               `CommsTrustReconcileRequested`, `CompleteAssistantPlayback`,
                               `CompleteLiveInteraction`, `CompleteLiveInteractionAttached`,
                               `CompleteLiveInteractionIdle`, `CompleteLiveInteractionRunning`,
                               `CompleteLiveInteractionTranscript`,
                               `ConfirmLiveBridgeFinalInput`,
                               `ConfirmLiveBridgeFinalInputAttached`,
                               `ConfirmLiveBridgeFinalInputIdle`,
                               `ConfirmLiveBridgeFinalInputRunning`,
                               `ConsumeLiveActiveChannelControl`,
                               `ConsumeLiveActiveChannelControlAttached`,
                               `ConsumeLiveActiveChannelControlIdle`,
                               `ConsumeLiveActiveChannelControlRunning`,
                               `ConsumeLiveBridgeEffectAuthority`,
                               `ConsumeLiveBridgeEffectAuthorityAttached`,
                               `ConsumeLiveBridgeEffectAuthorityIdle`,
                               `ConsumeLiveBridgeEffectAuthorityRunning`,
                               `DeclareRecoveredTerminalCompletionUnrecoverable`,
                               `DeclareRecoveredTerminalCompletionUnrecoverableAttached`,
                               `DeclareRecoveredTerminalCompletionUnrecoverableIdle`,
                               `DeclareRecoveredTerminalCompletionUnrecoverableInitializing`,
                               `DeclareRecoveredTerminalCompletionUnrecoverableRetired`,
                               `DeclareRecoveredTerminalCompletionUnrecoverableRunning`,
                               `DeclareRecoveredTerminalCompletionUnrecoverableStopped`,
                               `DetachIngress`, `DetachIngressAttached`, `DetachIngressIdle`,
                               `DetachIngressRetired`, `DetachIngressRunning`,
                               `DetachIngressStopped`, `DetachIngressTerminalSupervisorCleanup`,
                               `DrainQueuedRunRetiredRetainingUnsettledCompletion`,
                               `DurableTailClassified`, `Effect`, `EffectKind`,
                               `EmitExternalToolDelta`, `EnqueueLiveContextRow`,
                               `EnqueueLiveContextRowAttached`, `EnqueueLiveContextRowIdle`,
                               `EnqueueLiveContextRowRunning`,
                               `ExperimentalLiveExecutionStaged`,
                               `FenceRestoredLiveBridgeOperationForRestart`,
                               `FenceRestoredLiveBridgeOperationForRestartExactReplayAttached`,
                               `FenceRestoredLiveBridgeOperationForRestartExactReplayIdle`,
                               `FenceRestoredLiveBridgeOperationForRestartExactReplayRetired`,
                               `FenceRestoredLiveBridgeOperationForRestartExactReplayRunning`,
                               `FenceRestoredLiveBridgeOperationForRestartExactReplayStopped`,
                               `FenceRestoredLiveBridgeOperationForRestartFreshAttached`,
                               `FenceRestoredLiveBridgeOperationForRestartFreshIdle`,
                               `FenceRestoredLiveBridgeOperationForRestartFreshRetired`,
                               `FenceRestoredLiveBridgeOperationForRestartFreshRunning`,
                               `FenceRestoredLiveBridgeOperationForRestartFreshStopped`,
                               `InboundPeerInteractionStateChanged`, `Input`, `InputKind`,
                               `InteractionStreamAbandoned`,
                               `InteractionStreamAbandonedAfterUnregister`,
                               `InteractionStreamAbandonedAttached`,
                               `InteractionStreamAbandonedIdle`,
                               `InteractionStreamAbandonedRetired`,
                               `InteractionStreamAbandonedRunning`,
                               `InteractionStreamAbandonedStopped`, `InteractionStreamAttached`,
                               `InteractionStreamAttachedAfterUnregister`,
                               `InteractionStreamAttachedAttached`,
                               `InteractionStreamAttachedIdle`,
                               `InteractionStreamAttachedRetired`,
                               `InteractionStreamAttachedRunning`,
                               `InteractionStreamAttachedStopped`, `InteractionStreamCleanup`,
                               `InteractionStreamClosedEarly`,
                               `InteractionStreamClosedEarlyAfterUnregister`,
                               `InteractionStreamClosedEarlyAttached`,
                               `InteractionStreamClosedEarlyIdle`,
                               `InteractionStreamClosedEarlyRetired`,
                               `InteractionStreamClosedEarlyRunning`,
                               `InteractionStreamClosedEarlyStopped`,
                               `InteractionStreamCompleted`,
                               `InteractionStreamCompletedAfterUnregister`,
                               `InteractionStreamCompletedAttached`,
                               `InteractionStreamCompletedIdle`,
                               `InteractionStreamCompletedRetired`,
                               `InteractionStreamCompletedRunning`,
                               `InteractionStreamCompletedStopped`, `InteractionStreamExpired`,
                               `InteractionStreamExpiredAfterUnregister`,
                               `InteractionStreamExpiredAttached`,
                               `InteractionStreamExpiredIdle`,
                               `InteractionStreamExpiredRetired`,
                               `InteractionStreamExpiredRunning`,
                               `InteractionStreamExpiredStopped`, `InteractionStreamReserved`,
                               `InteractionStreamReservedAfterUnregister`,
                               `InteractionStreamReservedAttached`,
                               `InteractionStreamReservedIdle`,
                               `InteractionStreamReservedRetired`,
                               `InteractionStreamReservedRunning`,
                               `InteractionStreamReservedStopped`,
                               `InteractionStreamStateChanged`,
                               `InteractionTerminalOutboxAdoptionAuthorized`,
                               `live_abandoned_interactions`,
                               `live_activation_receipt_by_channel`,
                               `live_active_control_operation_by_authority`,
                               `live_active_interaction_by_channel`,
                               `live_assistant_interaction_by_turn`,
                               `live_assistant_turn_channel_by_ref`,
                               `live_awaiting_assistant_interaction_by_channel`,
                               `live_bridge_agent_identity_by_operation`,
                               `live_bridge_cancellation_reason_by_operation`,
                               `live_bridge_channel_by_operation`,
                               `live_bridge_consumed_effect_authorities`,
                               `live_bridge_context_revision_by_operation`,
                               `live_bridge_effect_kind_by_authority`,
                               `live_bridge_effect_operation_by_authority`,
                               `live_bridge_effect_outcome_by_authority`,
                               `live_bridge_execution_result_digest_by_operation`,
                               `live_bridge_execution_started_operations`,
                               `live_bridge_execution_terminal_by_operation`,
                               `live_bridge_in_flight_effect_authorities`,
                               `live_bridge_interaction_by_operation`,
                               `live_bridge_model_computation_authorized_operations`,
                               `live_bridge_operation_by_channel`,
                               `live_bridge_outcome_receipt_operations`,
                               `live_bridge_outcome_receipt_required_operations`,
                               `live_bridge_phase_by_operation`,
                               `live_bridge_provider_call_by_operation`,
                               `live_bridge_provider_delegation_by_operation`,
                               `live_bridge_provider_turn_by_operation`,
                               `live_bridge_read_snapshot_authorized_operations`,
                               `live_bridge_request_digest_by_operation`,
                               `live_bridge_submission_digest_by_operation`,
                               `live_bridge_submission_output_kind_by_operation`,
                               `live_bridge_submission_state_by_operation`,
                               `live_client_context_capable_channels`,
                               `live_consequential_effect_operation_by_authority`,
                               `live_consumed_active_control_authorities`,
                               `live_context_ambiguous_no_retry`,
                               `live_context_cursor_by_channel`,
                               `live_context_delivered_append_ids`,
                               `live_context_pending_append_by_channel`,
                               `live_context_pending_channel_by_append`,
                               `live_context_pending_next_cursor_by_append`,
                               `live_context_pending_previous_cursor_by_append`,
                               `live_context_queued_append_by_cursor`,
                               `live_context_queued_commit_token_by_append`,
                               `live_context_queued_cursor_by_append`,
                               `live_context_queued_digest_by_append`,
                               `live_context_queued_disposition_by_append`,
                               `live_context_queued_session_by_append`,
                               `live_context_recovery_append_by_channel`,
                               `live_context_recovery_fence_by_channel`,
                               `live_context_recovery_generation_by_channel`,
                               `live_context_recovery_identity_by_channel`,
                               `live_context_recovery_replacement_by_channel`,
                               `live_context_recovery_runtime_id_by_channel`,
                               `live_context_recovery_seed_cursor_by_channel`,
                               `live_context_recovery_session_by_channel`,
                               `live_context_recovery_source_by_replacement`,
                               `live_delegation_cancellation_reason_by_operation`,
                               `live_delegation_interaction_by_channel`,
                               `live_delegation_interaction_by_operation`,
                               `live_delegation_late_terminal_operations`,
                               `live_delegation_operation_by_channel`,
                               `live_delegation_provider_turn_by_channel`,
                               `live_delegation_provider_turn_by_operation`,
                               `live_delegation_reconciliation_by_operation`,
                               `live_delegation_result_eligible_operations`,
                               `live_delegation_worker_identity_by_operation`,
                               `live_delegation_worker_phase_by_operation`,
                               `live_delegation_worker_terminal_by_operation`,
                               `live_execution_fence_by_channel`,
                               `live_execution_generation_by_channel`,
                               `live_execution_mode_by_channel`,
                               `live_execution_phase_by_channel`,
                               `live_execution_profile_by_channel`,
                               `live_execution_runtime_id_by_channel`,
                               `live_experimental_execution_channels`,
                               `live_experimental_pending_receipt_by_channel`,
                               `live_experimental_staged_fence_by_channel`,
                               `live_experimental_staged_generation_by_channel`,
                               `live_experimental_staged_runtime_by_channel`,
                               `live_experimental_staged_seed_cursor_by_channel`,
                               `live_function_bridge_capable_channels`,
                               `live_interaction_channel_by_id`,
                               `live_playback_owner_by_channel`,
                               `live_playback_readiness_by_channel`,
                               `live_provider_interaction_by_turn`,
                               `live_provider_turn_by_channel`,
                               `live_provider_turn_channel_by_ref`,
                               `live_result_delivery_channel_by_operation`,
                               `live_result_delivery_digest_by_operation`,
                               `live_result_delivery_observation_by_operation`,
                               `live_result_delivery_operation_by_channel`,
                               `live_result_recovery_digest_by_channel`,
                               `live_result_recovery_fence_by_channel`,
                               `live_result_recovery_generation_by_channel`,
                               `live_result_recovery_identity_by_channel`,
                               `live_result_recovery_operation_by_channel`,
                               `live_result_recovery_replacement_by_channel`,
                               `live_result_recovery_runtime_id_by_channel`,
                               `live_result_recovery_seed_cursor_by_channel`,
                               `live_result_recovery_session_by_channel`,
                               `live_result_recovery_source_by_replacement`,
                               `live_result_release_disposition_by_operation`,
                               `live_result_released_operations`,
                               `live_result_speech_suppressed_operations`,
                               `live_revoked_execution_channels`,
                               `LiveActiveChannelControlAuthorityIssued`,
                               `LiveActiveChannelControlDispatchAuthorized`,
                               `LiveAssistantPlaybackFinalObserved`,
                               `LiveAssistantPlaybackFinalRecovered`,
                               `LiveAssistantPlaybackTargetAdmitted`,
                               `LiveAssistantPlaybackTargetRecovered`,
                               `LiveAssistantPlaybackTerminalObserved`,
                               `LiveAssistantPlaybackTerminalRecovered`,
                               `LiveAssistantPlaybackTerminalResolved`,
                               `LiveAssistantTurnStarted`, `LiveBridgeEffectAuthorityIssued`,
                               `LiveBridgeEffectDispatchAuthorized`,
                               `LiveBridgeEffectOutcomeRecorded`,
                               `LiveBridgeExecutionStartAuthorized`,
                               `LiveBridgeExecutionTerminalRecorded`,
                               `LiveBridgeFinalInputAuthorized`, `LiveBridgeOperationAdmitted`,
                               `LiveBridgeOperationCancellationAuthorized`,
                               `LiveBridgeOperationReplayObserved`,
                               `LiveBridgeOperationRetirementResolved`,
                               `LiveBridgeOutcomeReceiptRecorded`,
                               `LiveBridgeProtocolDriftCloseAuthorized`,
                               `LiveBridgeSubmissionAttemptClaimed`,
                               `LiveBridgeSubmissionAuthorized`,
                               `LiveBridgeSubmissionLocalWriteRecorded`,
                               `LiveBridgeSubmissionRecoveredAmbiguous`,
                               `LiveBridgeSubmissionResolved`, `LiveChannelCloseCustodyRevoked`,
                               `LiveChannelStatusResolved`, `LiveCommandPublicKind`,
                               `LiveConsequentialEffectAuthorized`,
                               `LiveContextAmbiguityRecoveryAuthorized`,
                               `LiveContextAppendAuthorized`, `LiveContextAppendResolved`,
                               `LiveContextCanonicalCoverageAdvanced`,
                               `LiveContextCommittedRowClassified`,
                               `LiveContextRecoveryChannelBound`, `LiveContextRowQueued`,
                               `LiveDelegationAdmitted`, `LiveDelegationCancellationAuthorized`,
                               `LiveDelegationCancellationResolved`,
                               `LiveDelegationResultAmbiguityRecoveryAuthorized`,
                               `LiveDelegationResultDeliveryAuthorized`,
                               `LiveDelegationResultDeliveryResolved`,
                               `LiveDelegationResultRecoveryChannelBound`,
                               `LiveDelegationResultReleaseAuthorized`,
                               `LiveDelegationTranscriptReconciled`,
                               `LiveDelegationWorkerRestartReconciled`,
                               `LiveDelegationWorkerRetirementAuthorized`,
                               `LiveDelegationWorkerRetirementResolved`,
                               `LiveDelegationWorkerStartAuthorized`,
                               `LiveDelegationWorkerStartResolved`,
                               `LiveDelegationWorkerTerminalRecorded`,
                               `LiveExecutionChannelBound`,
                               `LiveExecutionModeAdmissionResolved`,
                               `LiveFinalUserTranscriptReconciled`, `LiveInteractionAbandoned`,
                               `LiveInteractionAdmitted`, `LiveInteractionCompleted`,
                               `LiveInteractionDelegationAdmitted`,
                               `LiveInteractionSupersededWithoutCancellation`,
                               `LiveInteractionTranscriptAdmitted`,
                               `LiveInteractionTranscriptCompleted`,
                               `LiveOpenAdmissionAbandoned`, `LiveOpenAdmissionRejection`,
                               `LiveOpenAdmissionResolved`, `LivePlaybackOwnerReady`,
                               `LivePlaybackOwnerRevoked`, `LiveProviderTurnFinished`,
                               `LiveProviderTurnStarted`, `LiveProvisionalUserTranscriptStaged`,
                               `LiveSessionAuthorityClassified`,
                               `LiveWebrtcAnswerAcceptedAndExecutionBound`,
                               `LiveWebsocketTokenAdmissionResolved`,
                               `LiveWebsocketTokenIssued`, `LocalEndpointChanged`,
                               `McpServerConnected`, `McpServerConnectedAfterUnregister`,
                               `McpServerConnectedAttached`, `McpServerConnectedIdle`,
                               `McpServerConnectedRetired`, `McpServerConnectedRunning`,
                               `McpServerConnectedStopped`, `McpServerConnectPending`,
                               `McpServerConnectPendingAfterUnregister`,
                               `McpServerConnectPendingAttached`, `McpServerConnectPendingIdle`,
                               `McpServerConnectPendingRetired`,
                               `McpServerConnectPendingRunning`,
                               `McpServerConnectPendingStopped`, `McpServerDisconnected`,
                               `McpServerDisconnectedAfterUnregister`,
                               `McpServerDisconnectedAttached`, `McpServerDisconnectedIdle`,
                               `McpServerDisconnectedRetired`, `McpServerDisconnectedRunning`,
                               `McpServerDisconnectedStopped`, `McpServerFailed`,
                               `McpServerFailedAfterUnregister`, `McpServerFailedAttached`,
                               `McpServerFailedIdle`, `McpServerFailedRetired`,
                               `McpServerFailedRunning`, `McpServerFailedStopped`,
                               `McpServerReload`, `McpServerReloadAfterUnregister`,
                               `McpServerReloadAttached`, `McpServerReloadIdle`,
                               `McpServerReloadRequested`, `McpServerReloadRetired`,
                               `McpServerReloadRunning`, `McpServerReloadStopped`,
                               `McpServerStateChanged`, `MobEventStreamCloseResolved`,
                               `MobEventStreamOpenResolved`, `MobEventStreamTerminalResolved`,
                               `NotifyDrainExitedAfterUnregister`,
                               `ObserveLiveAssistantPlaybackFinal`,
                               `ObserveLiveAssistantPlaybackFinalJoinsComplete`,
                               `ObserveLiveAssistantPlaybackFinalJoinsPrefix`,
                               `ObserveLiveAssistantPlaybackFinalPendingTerminal`,
                               `ObserveLiveAssistantPlaybackTerminal`,
                               `ObserveLiveAssistantPlaybackTerminalJoinsComplete`,
                               `ObserveLiveAssistantPlaybackTerminalJoinsPrefix`,
                               `ObserveLiveAssistantPlaybackTerminalPendingFinal`,
                               `ObserveLiveAssistantPlaybackUnmeasured`,
                               `ObserveLiveAssistantTurnStarted`,
                               `ObserveLiveAssistantTurnStartedAttached`,
                               `ObserveLiveAssistantTurnStartedIdle`,
                               `ObserveLiveAssistantTurnStartedRunning`,
                               `ObserveLiveProviderTurnStarted`,
                               `ObserveLiveProviderTurnStartedAttached`,
                               `ObserveLiveProviderTurnStartedIdle`,
                               `ObserveLiveProviderTurnStartedRunning`,
                               `ObserveSupervisorRotation`,
                               `ObserveSupervisorRotationCompletedAttached`,
                               `ObserveSupervisorRotationCompletedDestroyed`,
                               `ObserveSupervisorRotationCompletedIdle`,
                               `ObserveSupervisorRotationCompletedRetired`,
                               `ObserveSupervisorRotationCompletedRunning`,
                               `ObserveSupervisorRotationCompletedStopped`,
                               `ObserveSupervisorRotationNextPublishPendingAttached`,
                               `ObserveSupervisorRotationNextPublishPendingDestroyed`,
                               `ObserveSupervisorRotationNextPublishPendingIdle`,
                               `ObserveSupervisorRotationNextPublishPendingRetired`,
                               `ObserveSupervisorRotationNextPublishPendingRunning`,
                               `ObserveSupervisorRotationNextPublishPendingStopped`,
                               `ObserveSupervisorRotationNotFoundAttached`,
                               `ObserveSupervisorRotationNotFoundDestroyed`,
                               `ObserveSupervisorRotationNotFoundIdle`,
                               `ObserveSupervisorRotationNotFoundRetired`,
                               `ObserveSupervisorRotationNotFoundRunning`,
                               `ObserveSupervisorRotationNotFoundStopped`,
                               `ObserveSupervisorRotationPreviousRevokePendingAttached`,
                               `ObserveSupervisorRotationPreviousRevokePendingDestroyed`,
                               `ObserveSupervisorRotationPreviousRevokePendingIdle`,
                               `ObserveSupervisorRotationPreviousRevokePendingRetired`,
                               `ObserveSupervisorRotationPreviousRevokePendingRunning`,
                               `ObserveSupervisorRotationPreviousRevokePendingStopped`,
                               `ObserveSupervisorRotationRejectedAttached`,
                               `ObserveSupervisorRotationRejectedDestroyed`,
                               `ObserveSupervisorRotationRejectedIdle`,
                               `ObserveSupervisorRotationRejectedRetired`,
                               `ObserveSupervisorRotationRejectedRunning`,
                               `ObserveSupervisorRotationRejectedStopped`,
                               `PeerIngressClassified`, `PeerIngressDequeueResolved`,
                               `PeerIngressReceiveResolved`, `PeerInteractionCleanup`,
                               `PeerInteractionStateChanged`, `PeerProjectionChanged`,
                               `PeerRequestReceived`, `PeerRequestReceivedAfterUnregister`,
                               `PeerRequestReceivedAttached`, `PeerRequestReceivedDestroyed`,
                               `PeerRequestReceivedIdle`, `PeerRequestReceivedRetired`,
                               `PeerRequestReceivedRunning`, `PeerRequestReceivedStopped`,
                               `PeerRequestSendFailed`, `PeerRequestSendFailedAfterUnregister`,
                               `PeerRequestSendFailedAttached`, `PeerRequestSendFailedIdle`,
                               `PeerRequestSendFailedRetired`, `PeerRequestSendFailedRunning`,
                               `PeerRequestSendFailedStopped`, `PeerRequestSent`,
                               `PeerRequestSentAfterUnregister`, `PeerRequestSentAttached`,
                               `PeerRequestSentIdle`, `PeerRequestSentRetired`,
                               `PeerRequestSentRunning`, `PeerRequestSentStopped`,
                               `PeerRequestTimedOut`, `PeerRequestTimedOutAfterUnregister`,
                               `PeerRequestTimedOutAttached`, `PeerRequestTimedOutIdle`,
                               `PeerRequestTimedOutRetired`, `PeerRequestTimedOutRunning`,
                               `PeerRequestTimedOutStopped`, `PeerResponseProgressArrived`,
                               `PeerResponseProgressArrivedAfterUnregister`,
                               `PeerResponseProgressArrivedAttached`,
                               `PeerResponseProgressArrivedIdle`,
                               `PeerResponseProgressArrivedRetired`,
                               `PeerResponseProgressArrivedRunning`,
                               `PeerResponseProgressArrivedStopped`, `PeerResponseRejected`,
                               `PeerResponseRejectedAfterUnregister`,
                               `PeerResponseRejectedAttached`, `PeerResponseRejectedIdle`,
                               `PeerResponseRejectedRetired`, `PeerResponseRejectedRunning`,
                               `PeerResponseRejectedStopped`, `PeerResponseReplied`,
                               `PeerResponseRepliedAfterUnregister`,
                               `PeerResponseRepliedAttached`, `PeerResponseRepliedDestroyed`,
                               `PeerResponseRepliedIdle`, `PeerResponseRepliedRetired`,
                               `PeerResponseRepliedRunning`, `PeerResponseRepliedStopped`,
                               `PeerResponseReplyClassified`, `PeerResponseTerminalArrived`,
                               `PeerResponseTerminalArrivedAfterUnregister`,
                               `PeerResponseTerminalArrivedCompletedAttached`,
                               `PeerResponseTerminalArrivedCompletedIdle`,
                               `PeerResponseTerminalArrivedCompletedRetired`,
                               `PeerResponseTerminalArrivedCompletedRunning`,
                               `PeerResponseTerminalArrivedCompletedStopped`,
                               `PeerResponseTerminalArrivedFailedAttached`,
                               `PeerResponseTerminalArrivedFailedIdle`,
                               `PeerResponseTerminalArrivedFailedRetired`,
                               `PeerResponseTerminalArrivedFailedRunning`,
                               `PeerResponseTerminalArrivedFailedStopped`,
                               `PendingContinuationPublicTerminalResolved`,
                               `PendingContinuationResolved`,
                               `PrepareAttachedRetainingUnsettledCompletion`,
                               `PrepareIdleRetainingUnsettledCompletion`,
                               `PublishLocalEndpoint`, `PublishLocalEndpointAttached`,
                               `PublishLocalEndpointIdle`, `PublishLocalEndpointRunning`,
                               `PublishLocalEndpointTerminalSupervisorCleanupDestroyed`,
                               `PublishLocalEndpointTerminalSupervisorCleanupRetired`,
                               `PublishSupervisorTrustEdge`, `RealtimeTranscriptAppended`,
                               `ReconcileLiveDelegationTranscript`,
                               `ReconcileLiveDelegationTranscriptConfirmedAttached`,
                               `ReconcileLiveDelegationTranscriptConfirmedIdle`,
                               `ReconcileLiveDelegationTranscriptConfirmedRunning`,
                               `ReconcileLiveDelegationTranscriptMaterialConflictAttached`,
                               `ReconcileLiveDelegationTranscriptMaterialConflictIdle`,
                               `ReconcileLiveDelegationTranscriptMaterialConflictRunning`,
                               `ReconcileLiveDelegationTranscriptMissingAttached`,
                               `ReconcileLiveDelegationTranscriptMissingIdle`,
                               `ReconcileLiveDelegationTranscriptMissingRunning`,
                               `ReconcileLiveFinalUserTranscript`,
                               `ReconcileRevokedLiveBridgeExecutionTerminal`,
                               `ReconcileRevokedLiveBridgeExecutionTerminalExactReplayAttached`,
                               `ReconcileRevokedLiveBridgeExecutionTerminalExactReplayIdle`,
                               `ReconcileRevokedLiveBridgeExecutionTerminalExactReplayRetired`,
                               `ReconcileRevokedLiveBridgeExecutionTerminalExactReplayRunning`,
                               `ReconcileRevokedLiveBridgeExecutionTerminalExactReplayStopped`,
                               `ReconcileRevokedLiveBridgeExecutionTerminalFreshAttached`,
                               `ReconcileRevokedLiveBridgeExecutionTerminalFreshIdle`,
                               `ReconcileRevokedLiveBridgeExecutionTerminalFreshRetired`,
                               `ReconcileRevokedLiveBridgeExecutionTerminalFreshRunning`,
                               `ReconcileRevokedLiveBridgeExecutionTerminalFreshStopped`,
                               `ReconcileRevokedLiveDelegationWorkerAfterRestart`,
                               `ReconcileRevokedLiveDelegationWorkerAfterRestartExactReplayAttached`,
                               `ReconcileRevokedLiveDelegationWorkerAfterRestartExactReplayIdle`,
                               `ReconcileRevokedLiveDelegationWorkerAfterRestartExactReplayRetired`,
                               `ReconcileRevokedLiveDelegationWorkerAfterRestartExactReplayRunning`,
                               `ReconcileRevokedLiveDelegationWorkerAfterRestartExactReplayStopped`,
                               `ReconcileRevokedLiveDelegationWorkerAfterRestartFreshAttached`,
                               `ReconcileRevokedLiveDelegationWorkerAfterRestartFreshIdle`,
                               `ReconcileRevokedLiveDelegationWorkerAfterRestartFreshRetired`,
                               `ReconcileRevokedLiveDelegationWorkerAfterRestartFreshRunning`,
                               `ReconcileRevokedLiveDelegationWorkerAfterRestartFreshStopped`,
                               `ReconcileRevokedLiveDelegationWorkerAfterRestartTerminalCustodyAttached`,
                               `ReconcileRevokedLiveDelegationWorkerAfterRestartTerminalCustodyIdle`,
                               `ReconcileRevokedLiveDelegationWorkerAfterRestartTerminalCustodyRetired`,
                               `ReconcileRevokedLiveDelegationWorkerAfterRestartTerminalCustodyRunning`,
                               `ReconcileRevokedLiveDelegationWorkerAfterRestartTerminalCustodyStopped`,
                               `RecordLiveBridgeEffectOutcome`,
                               `RecordLiveBridgeEffectOutcomeExactReplayAttached`,
                               `RecordLiveBridgeEffectOutcomeExactReplayIdle`,
                               `RecordLiveBridgeEffectOutcomeExactReplayRetired`,
                               `RecordLiveBridgeEffectOutcomeExactReplayRunning`,
                               `RecordLiveBridgeEffectOutcomeExactReplayStopped`,
                               `RecordLiveBridgeEffectOutcomeFreshAttached`,
                               `RecordLiveBridgeEffectOutcomeFreshIdle`,
                               `RecordLiveBridgeEffectOutcomeFreshRetired`,
                               `RecordLiveBridgeEffectOutcomeFreshRunning`,
                               `RecordLiveBridgeEffectOutcomeFreshStopped`,
                               `RecordLiveBridgeExecutionTerminal`,
                               `RecordLiveBridgeExecutionTerminalExactReplayAttached`,
                               `RecordLiveBridgeExecutionTerminalExactReplayIdle`,
                               `RecordLiveBridgeExecutionTerminalExactReplayRunning`,
                               `RecordLiveBridgeExecutionTerminalFreshAttached`,
                               `RecordLiveBridgeExecutionTerminalFreshIdle`,
                               `RecordLiveBridgeExecutionTerminalFreshRunning`,
                               `RecordLiveBridgeOutcomeReceipt`,
                               `RecordLiveBridgeOutcomeReceiptExactReplayAttached`,
                               `RecordLiveBridgeOutcomeReceiptExactReplayIdle`,
                               `RecordLiveBridgeOutcomeReceiptExactReplayRetired`,
                               `RecordLiveBridgeOutcomeReceiptExactReplayRunning`,
                               `RecordLiveBridgeOutcomeReceiptExactReplayStopped`,
                               `RecordLiveBridgeOutcomeReceiptFreshAttached`,
                               `RecordLiveBridgeOutcomeReceiptFreshIdle`,
                               `RecordLiveBridgeOutcomeReceiptFreshRetired`,
                               `RecordLiveBridgeOutcomeReceiptFreshRunning`,
                               `RecordLiveBridgeOutcomeReceiptFreshStopped`,
                               `RecordLiveBridgeSubmissionLocalWrite`,
                               `RecordLiveBridgeSubmissionLocalWriteAttached`,
                               `RecordLiveBridgeSubmissionLocalWriteIdle`,
                               `RecordLiveBridgeSubmissionLocalWriteRunning`,
                               `RecordLiveChannelRequestRejected`,
                               `RecordLiveChannelRequestRejectedAttached`,
                               `RecordLiveChannelRequestRejectedIdle`,
                               `RecordLiveChannelRequestRejectedRetired`,
                               `RecordLiveChannelRequestRejectedRunning`,
                               `RecordLiveChannelRequestRejectedStopped`,
                               `RecordLiveChannelStatus`, `RecordLiveChannelStatusAttached`,
                               `RecordLiveChannelStatusIdle`, `RecordLiveChannelStatusRetired`,
                               `RecordLiveChannelStatusRunning`,
                               `RecordLiveChannelStatusStopped`, `RecordLiveCloseClosed`,
                               `RecordLiveCloseClosedAttached`, `RecordLiveCloseClosedIdle`,
                               `RecordLiveCloseClosedRetired`, `RecordLiveCloseClosedRunning`,
                               `RecordLiveCloseClosedStopped`, `RecordLiveCommandAccepted`,
                               `RecordLiveCommandAcceptedAttached`,
                               `RecordLiveCommandAcceptedIdle`,
                               `RecordLiveCommandAcceptedRetired`,
                               `RecordLiveCommandAcceptedRunning`,
                               `RecordLiveCommandAcceptedStopped`, `RecordLiveCommandRejected`,
                               `RecordLiveCommandRejectedAttached`,
                               `RecordLiveCommandRejectedIdle`,
                               `RecordLiveCommandRejectedRetired`,
                               `RecordLiveCommandRejectedRunning`,
                               `RecordLiveCommandRejectedStopped`,
                               `RecordLiveDelegationWorkerTerminal`,
                               `RecordLiveDelegationWorkerTerminalAttached`,
                               `RecordLiveDelegationWorkerTerminalIdle`,
                               `RecordLiveDelegationWorkerTerminalRetired`,
                               `RecordLiveDelegationWorkerTerminalRunning`,
                               `RecordLiveDelegationWorkerTerminalStopped`,
                               `RecordLiveRefreshQueued`, `RecordLiveRefreshQueuedAttached`,
                               `RecordLiveRefreshQueuedIdle`, `RecordLiveRefreshQueuedRetired`,
                               `RecordLiveRefreshQueuedRunning`,
                               `RecordLiveRefreshQueuedStopped`,
                               `RecordLiveWebrtcAnswerAccepted`,
                               `RecordLiveWebrtcAnswerAcceptedAndBindExecution`,
                               `RecordLiveWebrtcAnswerAcceptedAndBindExecutionAttached`,
                               `RecordLiveWebrtcAnswerAcceptedAndBindExecutionIdle`,
                               `RecordLiveWebrtcAnswerAcceptedAndBindExecutionRunning`,
                               `RecordLiveWebrtcAnswerAcceptedAttached`,
                               `RecordLiveWebrtcAnswerAcceptedIdle`,
                               `RecordLiveWebrtcAnswerAcceptedRetired`,
                               `RecordLiveWebrtcAnswerAcceptedRunning`,
                               `RecordLiveWebrtcAnswerAcceptedStopped`,
                               `RecordLiveWebrtcTokenIssued`,
                               `RecordLiveWebrtcTokenIssuedAttached`,
                               `RecordLiveWebrtcTokenIssuedIdle`,
                               `RecordLiveWebrtcTokenIssuedRetired`,
                               `RecordLiveWebrtcTokenIssuedRunning`,
                               `RecordLiveWebrtcTokenIssuedStopped`,
                               `RecordLiveWebsocketTokenIssued`,
                               `RecordLiveWebsocketTokenIssuedAttached`,
                               `RecordLiveWebsocketTokenIssuedIdle`,
                               `RecordLiveWebsocketTokenIssuedRetired`,
                               `RecordLiveWebsocketTokenIssuedRunning`,
                               `RecordLiveWebsocketTokenIssuedStopped`,
                               `RecordMobEventStreamOpened`,
                               `RecordMobEventStreamOpenedAttached`,
                               `RecordMobEventStreamOpenedIdle`,
                               `RecordMobEventStreamOpenedRetired`,
                               `RecordMobEventStreamOpenedRunning`,
                               `RecordMobEventStreamOpenedStopped`,
                               `RecordMobEventStreamTerminated`,
                               `RecordMobEventStreamTerminatedAttached`,
                               `RecordMobEventStreamTerminatedIdle`,
                               `RecordMobEventStreamTerminatedRetired`,
                               `RecordMobEventStreamTerminatedRunning`,
                               `RecordMobEventStreamTerminatedStopped`,
                               `RecordSessionEventStreamOpened`,
                               `RecordSessionEventStreamOpenedAttached`,
                               `RecordSessionEventStreamOpenedIdle`,
                               `RecordSessionEventStreamOpenedRetired`,
                               `RecordSessionEventStreamOpenedRunning`,
                               `RecordSessionEventStreamOpenedStopped`,
                               `RecordSessionEventStreamTerminated`,
                               `RecordSessionEventStreamTerminatedAttached`,
                               `RecordSessionEventStreamTerminatedIdle`,
                               `RecordSessionEventStreamTerminatedRetired`,
                               `RecordSessionEventStreamTerminatedRunning`,
                               `RecordSessionEventStreamTerminatedStopped`,
                               `RecoveredTerminalCompletionBatchClassified`,
                               `RecoveredTerminalCompletionDeclaredUnrecoverable`,
                               `RecoverLiveAssistantPlaybackFinal`,
                               `RecoverLiveAssistantPlaybackTarget`,
                               `RecoverLiveAssistantPlaybackTerminal`,
                               `RecoverLiveBridgeSubmission`,
                               `RecoverLiveBridgeSubmissionAttached`,
                               `RecoverLiveBridgeSubmissionExactReplayAttached`,
                               `RecoverLiveBridgeSubmissionExactReplayIdle`,
                               `RecoverLiveBridgeSubmissionExactReplayRetired`,
                               `RecoverLiveBridgeSubmissionExactReplayRunning`,
                               `RecoverLiveBridgeSubmissionExactReplayStopped`,
                               `RecoverLiveBridgeSubmissionIdle`,
                               `RecoverLiveBridgeSubmissionRetired`,
                               `RecoverLiveBridgeSubmissionRunning`,
                               `RecoverLiveBridgeSubmissionStopped`, `RecoverSessionFromStore`,
                               `RecoverSessionFromStoreAuthorized`,
                               `RecoverSessionFromStoreUnrecoverable`,
                               `RecoverSessionLifecycleTerminal`,
                               `RefreshSupervisorBindingRoute`,
                               `RefreshSupervisorBindingRouteAttached`,
                               `RefreshSupervisorBindingRouteDestroyed`,
                               `RefreshSupervisorBindingRouteIdle`,
                               `RefreshSupervisorBindingRouteRetired`,
                               `RefreshSupervisorBindingRouteRunning`,
                               `RefreshSupervisorBindingRouteStopped`,
                               `RefreshVisibleSurfaceSet`, `RegisterLivePlaybackOwner`,
                               `RegisterLivePlaybackOwnerAttached`,
                               `RegisterLivePlaybackOwnerIdle`,
                               `RegisterLivePlaybackOwnerRunning`,
                               `RegisterSessionRefusedUnregisterDrainingAttached`,
                               `RegisterSessionRefusedUnregisterDrainingIdle`,
                               `RegisterSessionRefusedUnregisterDrainingRetired`,
                               `RegisterSessionRefusedUnregisterDrainingRunning`,
                               `RegisterSessionRefusedUnregisterDrainingStopped`,
                               `RejectSurfaceCall`, `RemoveDirectPeerEndpoint`,
                               `RemoveDirectPeerEndpointAttached`,
                               `RemoveDirectPeerEndpointIdle`,
                               `RemoveDirectPeerEndpointRunning`,
                               `RepairAddDirectPeerEndpointAttached`,
                               `RepairAddDirectPeerEndpointIdle`,
                               `RepairAddDirectPeerEndpointRunning`,
                               `RepairMobPeerOverlayAttached`, `RepairMobPeerOverlayIdle`,
                               `RepairMobPeerOverlayRunning`,
                               `RepairRemoveDirectPeerEndpointAttached`,
                               `RepairRemoveDirectPeerEndpointIdle`,
                               `RepairRemoveDirectPeerEndpointRunning`,
                               `RepairSupervisorMobPeerOverlayAttached`,
                               `RepairSupervisorMobPeerOverlayIdle`,
                               `RepairSupervisorMobPeerOverlayRunning`,
                               `ReplaceDeferredToolAuthorityCatalog`,
                               `ReplaceDeferredToolAuthorityCatalogAttached`,
                               `ReplaceDeferredToolAuthorityCatalogIdle`,
                               `ReplaceDeferredToolAuthorityCatalogRetired`,
                               `ReplaceDeferredToolAuthorityCatalogRunning`,
                               `ReplaceDeferredToolAuthorityCatalogStopped`,
                               `ReplaceFilterToolAuthorityCatalog`,
                               `ReplaceFilterToolAuthorityCatalogAttached`,
                               `ReplaceFilterToolAuthorityCatalogIdle`,
                               `ReplaceFilterToolAuthorityCatalogRetired`,
                               `ReplaceFilterToolAuthorityCatalogRunning`,
                               `ReplaceFilterToolAuthorityCatalogStopped`,
                               `ReplaceVisibilityState`, `ReplaceVisibilityStateAttached`,
                               `ReplaceVisibilityStateIdle`, `ReplaceVisibilityStateRetired`,
                               `ReplaceVisibilityStateRunning`, `ReplaceVisibilityStateStopped`,
                               `RequestCommsDrainExitForUnregister`,
                               `RequestCompletionWaiterResolutionForUnregister`,
                               `RequestDeferredTools`, `RequestRuntimeLoopStopForUnregister`,
                               `RequestSupervisorTrustPublish`,
                               `RequestSupervisorTrustPublishAttached`,
                               `RequestSupervisorTrustPublishDestroyed`,
                               `RequestSupervisorTrustPublishIdle`,
                               `RequestSupervisorTrustPublishRetired`,
                               `RequestSupervisorTrustPublishRunning`,
                               `RequestSupervisorTrustPublishStopped`, `RequestWaitAllAttached`,
                               `RequestWaitAllIdle`, `RequestWaitAllRetired`,
                               `RequestWaitAllRunning`, `RequestWaitAllStopped`,
                               `ResolveLiveAssistantPlaybackOnChannelClose`,
                               `ResolveLiveBridgeSubmission`,
                               `ResolveLiveBridgeSubmissionAttached`,
                               `ResolveLiveBridgeSubmissionIdle`,
                               `ResolveLiveBridgeSubmissionRunning`, `ResolveLiveContextAppend`,
                               `ResolveLiveContextAppendAmbiguousAttached`,
                               `ResolveLiveContextAppendAmbiguousIdle`,
                               `ResolveLiveContextAppendAmbiguousRunning`,
                               `ResolveLiveContextAppendDeliveredAttached`,
                               `ResolveLiveContextAppendDeliveredIdle`,
                               `ResolveLiveContextAppendDeliveredRunning`,
                               `ResolveLiveContextAppendRejectedAttached`,
                               `ResolveLiveContextAppendRejectedIdle`,
                               `ResolveLiveContextAppendRejectedRunning`,
                               `ResolveLiveDelegationCancellation`,
                               `ResolveLiveDelegationCancellationAttached`,
                               `ResolveLiveDelegationCancellationIdle`,
                               `ResolveLiveDelegationCancellationRetired`,
                               `ResolveLiveDelegationCancellationRunning`,
                               `ResolveLiveDelegationCancellationStopped`,
                               `ResolveLiveDelegationResultDelivery`,
                               `ResolveLiveDelegationResultDeliveryAmbiguousAttached`,
                               `ResolveLiveDelegationResultDeliveryAmbiguousIdle`,
                               `ResolveLiveDelegationResultDeliveryAmbiguousRunning`,
                               `ResolveLiveDelegationResultDeliveryAttached`,
                               `ResolveLiveDelegationResultDeliveryIdle`,
                               `ResolveLiveDelegationResultDeliveryRunning`,
                               `ResolveLiveDelegationWorkerRetirement`,
                               `ResolveLiveDelegationWorkerRetirementAttached`,
                               `ResolveLiveDelegationWorkerRetirementIdle`,
                               `ResolveLiveDelegationWorkerRetirementRetired`,
                               `ResolveLiveDelegationWorkerRetirementRunning`,
                               `ResolveLiveDelegationWorkerRetirementStopped`,
                               `ResolveLiveDelegationWorkerStart`,
                               `ResolveLiveDelegationWorkerStartAttached`,
                               `ResolveLiveDelegationWorkerStartIdle`,
                               `ResolveLiveDelegationWorkerStartRunning`,
                               `ResolveLiveExecutionModeAdmission`,
                               `ResolveLiveExecutionModeAdmissionAttached`,
                               `ResolveLiveExecutionModeAdmissionIdle`,
                               `ResolveLiveExecutionModeAdmissionRunning`,
                               `ResolveLiveOpenAdmissionDrainingAttached`,
                               `ResolveLiveOpenAdmissionDrainingIdle`,
                               `ResolveLiveOpenAdmissionDrainingRunning`,
                               `ResolveLiveOpenAdmissionRetiredRetired`,
                               `ResolveLiveOpenAdmissionRevokedChannelIdAttached`,
                               `ResolveLiveOpenAdmissionRevokedChannelIdIdle`,
                               `ResolveLiveOpenAdmissionRevokedChannelIdRunning`,
                               `ResolveLiveOpenAdmissionStopDeferredAttached`,
                               `ResolveLiveOpenAdmissionStopDeferredRunning`,
                               `ResolveLiveOpenAdmissionStoppedStopped`,
                               `ResolveLiveWebrtcAnswerAdmission`,
                               `ResolveLiveWebrtcAnswerAdmissionAcceptedAttached`,
                               `ResolveLiveWebrtcAnswerAdmissionAcceptedIdle`,
                               `ResolveLiveWebrtcAnswerAdmissionAcceptedRetired`,
                               `ResolveLiveWebrtcAnswerAdmissionAcceptedRunning`,
                               `ResolveLiveWebrtcAnswerAdmissionAcceptedStopped`,
                               `ResolveLiveWebrtcAnswerAdmissionChannelNotBoundAttached`,
                               `ResolveLiveWebrtcAnswerAdmissionChannelNotBoundIdle`,
                               `ResolveLiveWebrtcAnswerAdmissionChannelNotBoundRetired`,
                               `ResolveLiveWebrtcAnswerAdmissionChannelNotBoundRunning`,
                               `ResolveLiveWebrtcAnswerAdmissionChannelNotBoundStopped`,
                               `ResolveLiveWebrtcAnswerAdmissionTokenAlreadyConsumedAttached`,
                               `ResolveLiveWebrtcAnswerAdmissionTokenAlreadyConsumedIdle`,
                               `ResolveLiveWebrtcAnswerAdmissionTokenAlreadyConsumedRetired`,
                               `ResolveLiveWebrtcAnswerAdmissionTokenAlreadyConsumedRunning`,
                               `ResolveLiveWebrtcAnswerAdmissionTokenAlreadyConsumedStopped`,
                               `ResolveLiveWebrtcAnswerAdmissionTokenChannelMismatchAttached`,
                               `ResolveLiveWebrtcAnswerAdmissionTokenChannelMismatchIdle`,
                               `ResolveLiveWebrtcAnswerAdmissionTokenChannelMismatchRetired`,
                               `ResolveLiveWebrtcAnswerAdmissionTokenChannelMismatchRunning`,
                               `ResolveLiveWebrtcAnswerAdmissionTokenChannelMismatchStopped`,
                               `ResolveLiveWebrtcAnswerAdmissionTokenExpiredAttached`,
                               `ResolveLiveWebrtcAnswerAdmissionTokenExpiredIdle`,
                               `ResolveLiveWebrtcAnswerAdmissionTokenExpiredRetired`,
                               `ResolveLiveWebrtcAnswerAdmissionTokenExpiredRunning`,
                               `ResolveLiveWebrtcAnswerAdmissionTokenExpiredStopped`,
                               `ResolveLiveWebrtcAnswerAdmissionTokenNotFoundAttached`,
                               `ResolveLiveWebrtcAnswerAdmissionTokenNotFoundIdle`,
                               `ResolveLiveWebrtcAnswerAdmissionTokenNotFoundRetired`,
                               `ResolveLiveWebrtcAnswerAdmissionTokenNotFoundRunning`,
                               `ResolveLiveWebrtcAnswerAdmissionTokenNotFoundStopped`,
                               `ResolveLiveWebsocketTokenAdmission`,
                               `ResolveLiveWebsocketTokenAdmissionAcceptedAttached`,
                               `ResolveLiveWebsocketTokenAdmissionAcceptedIdle`,
                               `ResolveLiveWebsocketTokenAdmissionAcceptedRetired`,
                               `ResolveLiveWebsocketTokenAdmissionAcceptedRunning`,
                               `ResolveLiveWebsocketTokenAdmissionAcceptedStopped`,
                               `ResolveLiveWebsocketTokenAdmissionChannelNotBoundAttached`,
                               `ResolveLiveWebsocketTokenAdmissionChannelNotBoundIdle`,
                               `ResolveLiveWebsocketTokenAdmissionChannelNotBoundRetired`,
                               `ResolveLiveWebsocketTokenAdmissionChannelNotBoundRunning`,
                               `ResolveLiveWebsocketTokenAdmissionChannelNotBoundStopped`,
                               `ResolveLiveWebsocketTokenAdmissionTokenAlreadyConsumedAttached`,
                               `ResolveLiveWebsocketTokenAdmissionTokenAlreadyConsumedIdle`,
                               `ResolveLiveWebsocketTokenAdmissionTokenAlreadyConsumedRetired`,
                               `ResolveLiveWebsocketTokenAdmissionTokenAlreadyConsumedRunning`,
                               `ResolveLiveWebsocketTokenAdmissionTokenAlreadyConsumedStopped`,
                               `ResolveLiveWebsocketTokenAdmissionTokenChannelMismatchAttached`,
                               `ResolveLiveWebsocketTokenAdmissionTokenChannelMismatchIdle`,
                               `ResolveLiveWebsocketTokenAdmissionTokenChannelMismatchRetired`,
                               `ResolveLiveWebsocketTokenAdmissionTokenChannelMismatchRunning`,
                               `ResolveLiveWebsocketTokenAdmissionTokenChannelMismatchStopped`,
                               `ResolveLiveWebsocketTokenAdmissionTokenExpiredAttached`,
                               `ResolveLiveWebsocketTokenAdmissionTokenExpiredIdle`,
                               `ResolveLiveWebsocketTokenAdmissionTokenExpiredRetired`,
                               `ResolveLiveWebsocketTokenAdmissionTokenExpiredRunning`,
                               `ResolveLiveWebsocketTokenAdmissionTokenExpiredStopped`,
                               `ResolveLiveWebsocketTokenAdmissionTokenNotFoundAttached`,
                               `ResolveLiveWebsocketTokenAdmissionTokenNotFoundIdle`,
                               `ResolveLiveWebsocketTokenAdmissionTokenNotFoundRetired`,
                               `ResolveLiveWebsocketTokenAdmissionTokenNotFoundRunning`,
                               `ResolveLiveWebsocketTokenAdmissionTokenNotFoundStopped`,
                               `ResolveMobEventStreamClose`,
                               `ResolveMobEventStreamCloseActiveAttached`,
                               `ResolveMobEventStreamCloseActiveIdle`,
                               `ResolveMobEventStreamCloseActiveRetired`,
                               `ResolveMobEventStreamCloseActiveRunning`,
                               `ResolveMobEventStreamCloseActiveStopped`,
                               `ResolveMobEventStreamCloseAlreadyClosedAttached`,
                               `ResolveMobEventStreamCloseAlreadyClosedIdle`,
                               `ResolveMobEventStreamCloseAlreadyClosedRetired`,
                               `ResolveMobEventStreamCloseAlreadyClosedRunning`,
                               `ResolveMobEventStreamCloseAlreadyClosedStopped`,
                               `ResolvePendingContinuation`,
                               `ResolvePendingContinuationWithBoundary`,
                               `ResolvePendingContinuationWithoutBoundary`,
                               `ResolveRuntimeCheckpointProjection`,
                               `ResolveRuntimeCheckpointProjectionActive`,
                               `ResolveRuntimeCheckpointProjectionArchived`,
                               `ResolveSessionDocumentLifecycleMerge`,
                               `ResolveSessionDocumentLifecycleMergeArchivedAbsorbing`,
                               `ResolveSessionDocumentLifecycleMergeAuthority`,
                               `ResolveSessionEventStreamClose`,
                               `ResolveSessionEventStreamCloseActiveAttached`,
                               `ResolveSessionEventStreamCloseActiveIdle`,
                               `ResolveSessionEventStreamCloseActiveRetired`,
                               `ResolveSessionEventStreamCloseActiveRunning`,
                               `ResolveSessionEventStreamCloseActiveStopped`,
                               `ResolveSessionEventStreamCloseAlreadyClosedAttached`,
                               `ResolveSessionEventStreamCloseAlreadyClosedIdle`,
                               `ResolveSessionEventStreamCloseAlreadyClosedRetired`,
                               `ResolveSessionEventStreamCloseAlreadyClosedRunning`,
                               `ResolveSessionEventStreamCloseAlreadyClosedStopped`,
                               `ResolveSupervisorAuthorizeAdmission`,
                               `ResolveSupervisorAuthorizeAdmissionIdempotentAckAttached`,
                               `ResolveSupervisorAuthorizeAdmissionIdempotentAckDestroyed`,
                               `ResolveSupervisorAuthorizeAdmissionIdempotentAckIdle`,
                               `ResolveSupervisorAuthorizeAdmissionIdempotentAckRetired`,
                               `ResolveSupervisorAuthorizeAdmissionIdempotentAckRunning`,
                               `ResolveSupervisorAuthorizeAdmissionIdempotentAckStopped`,
                               `ResolveSupervisorAuthorizeAdmissionIdentityChangeRejectedAttached`,
                               `ResolveSupervisorAuthorizeAdmissionIdentityChangeRejectedIdle`,
                               `ResolveSupervisorAuthorizeAdmissionIdentityChangeRejectedRunning`,
                               `ResolveSupervisorAuthorizeAdmissionNotBoundAttached`,
                               `ResolveSupervisorAuthorizeAdmissionNotBoundDestroyed`,
                               `ResolveSupervisorAuthorizeAdmissionNotBoundIdle`,
                               `ResolveSupervisorAuthorizeAdmissionNotBoundRetired`,
                               `ResolveSupervisorAuthorizeAdmissionNotBoundRunning`,
                               `ResolveSupervisorAuthorizeAdmissionNotBoundStopped`,
                               `ResolveSupervisorAuthorizeAdmissionProceedAttached`,
                               `ResolveSupervisorAuthorizeAdmissionProceedIdle`,
                               `ResolveSupervisorAuthorizeAdmissionProceedRunning`,
                               `ResolveSupervisorAuthorizeAdmissionRotationNotAllowedDestroyed`,
                               `ResolveSupervisorAuthorizeAdmissionRotationNotAllowedRetired`,
                               `ResolveSupervisorAuthorizeAdmissionRotationNotAllowedStopped`,
                               `ResolveSupervisorAuthorizeAdmissionSenderMismatchAttached`,
                               `ResolveSupervisorAuthorizeAdmissionSenderMismatchDestroyed`,
                               `ResolveSupervisorAuthorizeAdmissionSenderMismatchIdle`,
                               `ResolveSupervisorAuthorizeAdmissionSenderMismatchRetired`,
                               `ResolveSupervisorAuthorizeAdmissionSenderMismatchRunning`,
                               `ResolveSupervisorAuthorizeAdmissionSenderMismatchStopped`,
                               `ResolveSupervisorAuthorizeAdmissionStaleSupervisorAttached`,
                               `ResolveSupervisorAuthorizeAdmissionStaleSupervisorDestroyed`,
                               `ResolveSupervisorAuthorizeAdmissionStaleSupervisorIdle`,
                               `ResolveSupervisorAuthorizeAdmissionStaleSupervisorRetired`,
                               `ResolveSupervisorAuthorizeAdmissionStaleSupervisorRunning`,
                               `ResolveSupervisorAuthorizeAdmissionStaleSupervisorStopped`,
                               `ResolveSupervisorBindAdmission`,
                               `ResolveSupervisorBindAdmissionAlreadyBoundAttached`,
                               `ResolveSupervisorBindAdmissionAlreadyBoundIdle`,
                               `ResolveSupervisorBindAdmissionAlreadyBoundRunning`,
                               `ResolveSupervisorBindAdmissionBootstrapAttached`,
                               `ResolveSupervisorBindAdmissionBootstrapIdle`,
                               `ResolveSupervisorBindAdmissionBootstrapRunning`,
                               `ResolveSupervisorBindAdmissionIdempotentAckAttached`,
                               `ResolveSupervisorBindAdmissionIdempotentAckIdle`,
                               `ResolveSupervisorBindAdmissionIdempotentAckRunning`,
                               `ResolveSupervisorBindAdmissionRevocationPendingAttached`,
                               `ResolveSupervisorBindAdmissionRevocationPendingIdle`,
                               `ResolveSupervisorBindAdmissionRevocationPendingRunning`,
                               `ResolveSupervisorBindAdmissionSenderMismatchAttached`,
                               `ResolveSupervisorBindAdmissionSenderMismatchIdle`,
                               `ResolveSupervisorBindAdmissionSenderMismatchRunning`,
                               `ResolveSupervisorBindMaterialAdmission`,
                               `ResolveSupervisorBindMaterialAdmissionAcceptAttached`,
                               `ResolveSupervisorBindMaterialAdmissionAcceptIdle`,
                               `ResolveSupervisorBindMaterialAdmissionAcceptRunning`,
                               `ResolveSupervisorBindMaterialAdmissionAddressMismatchAttached`,
                               `ResolveSupervisorBindMaterialAdmissionAddressMismatchIdle`,
                               `ResolveSupervisorBindMaterialAdmissionAddressMismatchRunning`,
                               `ResolveSupervisorBindMaterialAdmissionInvalidBootstrapTokenAttached`,
                               `ResolveSupervisorBindMaterialAdmissionInvalidBootstrapTokenIdle`,
                               `ResolveSupervisorBindMaterialAdmissionInvalidBootstrapTokenRunning`,
                               `ResolveSupervisorBindMaterialAdmissionInvalidPeerSpecAttached`,
                               `ResolveSupervisorBindMaterialAdmissionInvalidPeerSpecIdle`,
                               `ResolveSupervisorBindMaterialAdmissionInvalidPeerSpecRunning`,
                               `ResolveSupervisorBindMaterialAdmissionSenderMismatchAttached`,
                               `ResolveSupervisorBindMaterialAdmissionSenderMismatchIdle`,
                               `ResolveSupervisorBindMaterialAdmissionSenderMismatchRunning`,
                               `ResolveSupervisorBridgeCommandAdmission`,
                               `ResolveSupervisorBridgeCommandAdmissionAcceptedAttached`,
                               `ResolveSupervisorBridgeCommandAdmissionAcceptedIdle`,
                               `ResolveSupervisorBridgeCommandAdmissionAcceptedRunning`,
                               `ResolveSupervisorBridgeCommandAdmissionNotBoundAttached`,
                               `ResolveSupervisorBridgeCommandAdmissionNotBoundIdle`,
                               `ResolveSupervisorBridgeCommandAdmissionNotBoundRunning`,
                               `ResolveSupervisorBridgeCommandAdmissionSenderMismatchAttached`,
                               `ResolveSupervisorBridgeCommandAdmissionSenderMismatchIdle`,
                               `ResolveSupervisorBridgeCommandAdmissionSenderMismatchRunning`,
                               `ResolveSupervisorBridgeCommandAdmissionStaleSupervisorAttached`,
                               `ResolveSupervisorBridgeCommandAdmissionStaleSupervisorIdle`,
                               `ResolveSupervisorBridgeCommandAdmissionStaleSupervisorRunning`,
                               `ResolveSupervisorCleanupCommandAdmission`,
                               `ResolveSupervisorCleanupCommandAdmissionAcceptedAttached`,
                               `ResolveSupervisorCleanupCommandAdmissionAcceptedDestroyed`,
                               `ResolveSupervisorCleanupCommandAdmissionAcceptedIdle`,
                               `ResolveSupervisorCleanupCommandAdmissionAcceptedPendingRevokeAttached`,
                               `ResolveSupervisorCleanupCommandAdmissionAcceptedPendingRevokeDestroyed`,
                               `ResolveSupervisorCleanupCommandAdmissionAcceptedPendingRevokeIdle`,
                               `ResolveSupervisorCleanupCommandAdmissionAcceptedPendingRevokeRetired`,
                               `ResolveSupervisorCleanupCommandAdmissionAcceptedPendingRevokeRunning`,
                               `ResolveSupervisorCleanupCommandAdmissionAcceptedPendingRevokeStopped`,
                               `ResolveSupervisorCleanupCommandAdmissionAcceptedRetired`,
                               `ResolveSupervisorCleanupCommandAdmissionAcceptedRunning`,
                               `ResolveSupervisorCleanupCommandAdmissionAcceptedStopped`,
                               `ResolveSupervisorCleanupCommandAdmissionCommandNotAllowed`,
                               `ResolveSupervisorCleanupCommandAdmissionNotBoundAttached`,
                               `ResolveSupervisorCleanupCommandAdmissionNotBoundDestroyed`,
                               `ResolveSupervisorCleanupCommandAdmissionNotBoundIdle`,
                               `ResolveSupervisorCleanupCommandAdmissionNotBoundRetired`,
                               `ResolveSupervisorCleanupCommandAdmissionNotBoundRunning`,
                               `ResolveSupervisorCleanupCommandAdmissionNotBoundStopped`,
                               `ResolveSupervisorCleanupCommandAdmissionPendingRevokeSenderMismatchAttached`,
                               `ResolveSupervisorCleanupCommandAdmissionPendingRevokeSenderMismatchDestroyed`,
                               `ResolveSupervisorCleanupCommandAdmissionPendingRevokeSenderMismatchIdle`,
                               `ResolveSupervisorCleanupCommandAdmissionPendingRevokeSenderMismatchRetired`,
                               `ResolveSupervisorCleanupCommandAdmissionPendingRevokeSenderMismatchRunning`,
                               `ResolveSupervisorCleanupCommandAdmissionPendingRevokeSenderMismatchStopped`,
                               `ResolveSupervisorCleanupCommandAdmissionPendingRevokeStaleSupervisorAttached`,
                               `ResolveSupervisorCleanupCommandAdmissionPendingRevokeStaleSupervisorDestroyed`,
                               `ResolveSupervisorCleanupCommandAdmissionPendingRevokeStaleSupervisorIdle`,
                               `ResolveSupervisorCleanupCommandAdmissionPendingRevokeStaleSupervisorRetired`,
                               `ResolveSupervisorCleanupCommandAdmissionPendingRevokeStaleSupervisorRunning`,
                               `ResolveSupervisorCleanupCommandAdmissionPendingRevokeStaleSupervisorStopped`,
                               `ResolveSupervisorCleanupCommandAdmissionSenderMismatchAttached`,
                               `ResolveSupervisorCleanupCommandAdmissionSenderMismatchDestroyed`,
                               `ResolveSupervisorCleanupCommandAdmissionSenderMismatchIdle`,
                               `ResolveSupervisorCleanupCommandAdmissionSenderMismatchRetired`,
                               `ResolveSupervisorCleanupCommandAdmissionSenderMismatchRunning`,
                               `ResolveSupervisorCleanupCommandAdmissionSenderMismatchStopped`,
                               `ResolveSupervisorCleanupCommandAdmissionStaleSupervisorAttached`,
                               `ResolveSupervisorCleanupCommandAdmissionStaleSupervisorDestroyed`,
                               `ResolveSupervisorCleanupCommandAdmissionStaleSupervisorIdle`,
                               `ResolveSupervisorCleanupCommandAdmissionStaleSupervisorRetired`,
                               `ResolveSupervisorCleanupCommandAdmissionStaleSupervisorRunning`,
                               `ResolveSupervisorCleanupCommandAdmissionStaleSupervisorStopped`,
                               `ResolveTranscriptEditAdmission`,
                               `ResolveTranscriptEditAdmissionAttachedAdmissible`,
                               `ResolveTranscriptEditAdmissionAttachedBusy`,
                               `ResolveTranscriptEditAdmissionIdleAdmissible`,
                               `ResolveTranscriptEditAdmissionIdleBusy`,
                               `ResolveTranscriptEditAdmissionRunningAdmissible`,
                               `ResolveTranscriptEditAdmissionRunningBusy`,
                               `ResolveTurnSurfaceResultBudgetExhaustedFailureIdle`,
                               `ResolveTurnSurfaceResultBudgetExhaustedSuccessIdle`,
                               `ResolveTurnSurfaceResultCancelledCancelledIdle`,
                               `ResolveTurnSurfaceResultCancelledFailureIdle`,
                               `ResolveTurnSurfaceResultCompletedFailureIdle`,
                               `ResolveTurnSurfaceResultCompletedSuccessIdle`,
                               `ResolveTurnSurfaceResultFailedHardFailureIdle`,
                               `ResolveTurnSurfaceResultNoneMissingTerminalIdle`,
                               `ResolveTurnSurfaceResultStructuredOutputValidationFailedHardFailureIdle`,
                               `ResolveTurnSurfaceResultTimeBudgetExceededHardFailureIdle`,
                               `ResolveWaitAllAdmissionAcceptedAttached`,
                               `ResolveWaitAllAdmissionAcceptedIdle`,
                               `ResolveWaitAllAdmissionAcceptedRetired`,
                               `ResolveWaitAllAdmissionAcceptedRunning`,
                               `ResolveWaitAllAdmissionAcceptedStopped`,
                               `ResolveWaitAllAdmissionActiveRejectedAttached`,
                               `ResolveWaitAllAdmissionActiveRejectedIdle`,
                               `ResolveWaitAllAdmissionActiveRejectedRetired`,
                               `ResolveWaitAllAdmissionActiveRejectedRunning`,
                               `ResolveWaitAllAdmissionActiveRejectedStopped`,
                               `ResolveWaitAllAdmissionDuplicateRejectedAttached`,
                               `ResolveWaitAllAdmissionDuplicateRejectedIdle`,
                               `ResolveWaitAllAdmissionDuplicateRejectedRetired`,
                               `ResolveWaitAllAdmissionDuplicateRejectedRunning`,
                               `ResolveWaitAllAdmissionDuplicateRejectedStopped`,
                               `ResolveWaitAllAdmissionNotFoundRejectedAttached`,
                               `ResolveWaitAllAdmissionNotFoundRejectedIdle`,
                               `ResolveWaitAllAdmissionNotFoundRejectedRetired`,
                               `ResolveWaitAllAdmissionNotFoundRejectedRunning`,
                               `ResolveWaitAllAdmissionNotFoundRejectedStopped`,
                               `RestoreSessionBuildState`, `ResumeSupervisorRotation`,
                               `ResumeSupervisorRotationCompletedAttached`,
                               `ResumeSupervisorRotationCompletedDestroyed`,
                               `ResumeSupervisorRotationCompletedIdle`,
                               `ResumeSupervisorRotationCompletedRetired`,
                               `ResumeSupervisorRotationCompletedRunning`,
                               `ResumeSupervisorRotationCompletedStopped`,
                               `ResumeSupervisorRotationNextPublishAttached`,
                               `ResumeSupervisorRotationNextPublishDestroyed`,
                               `ResumeSupervisorRotationNextPublishIdle`,
                               `ResumeSupervisorRotationNextPublishRetired`,
                               `ResumeSupervisorRotationNextPublishRunning`,
                               `ResumeSupervisorRotationNextPublishStopped`,
                               `ResumeSupervisorRotationPreviousRevokeAttached`,
                               `ResumeSupervisorRotationPreviousRevokeDestroyed`,
                               `ResumeSupervisorRotationPreviousRevokeIdle`,
                               `ResumeSupervisorRotationPreviousRevokeRetired`,
                               `ResumeSupervisorRotationPreviousRevokeRunning`,
                               `ResumeSupervisorRotationPreviousRevokeStopped`,
                               `RetireSettledLiveBridgeOperation`,
                               `RetireSettledLiveBridgeOperationAlreadyAbsentAttached`,
                               `RetireSettledLiveBridgeOperationAlreadyAbsentIdle`,
                               `RetireSettledLiveBridgeOperationAlreadyAbsentRetired`,
                               `RetireSettledLiveBridgeOperationAlreadyAbsentRunning`,
                               `RetireSettledLiveBridgeOperationAlreadyAbsentStopped`,
                               `RetireSettledLiveBridgeOperationAttached`,
                               `RetireSettledLiveBridgeOperationIdle`,
                               `RetireSettledLiveBridgeOperationRetired`,
                               `RetireSettledLiveBridgeOperationRunning`,
                               `RetireSettledLiveBridgeOperationStopped`,
                               `RetryPendingSupervisorRevokeAttached`,
                               `RetryPendingSupervisorRevokeDestroyed`,
                               `RetryPendingSupervisorRevokeIdle`,
                               `RetryPendingSupervisorRevokeRetired`,
                               `RetryPendingSupervisorRevokeRunning`,
                               `RetryPendingSupervisorRevokeStopped`,
                               `ReviveArchivedSessionDocument`, `RevokedChannelId`,
                               `RevokeLiveChannelCloseCustody`,
                               `RevokeLiveChannelCloseCustodyAttached`,
                               `RevokeLiveChannelCloseCustodyClosedReplayAttached`,
                               `RevokeLiveChannelCloseCustodyClosedReplayIdle`,
                               `RevokeLiveChannelCloseCustodyClosedReplayRetired`,
                               `RevokeLiveChannelCloseCustodyClosedReplayRunning`,
                               `RevokeLiveChannelCloseCustodyClosedReplayStopped`,
                               `RevokeLiveChannelCloseCustodyIdle`,
                               `RevokeLiveChannelCloseCustodyRunning`,
                               `RevokeLivePlaybackOwner`, `RevokeLivePlaybackOwnerAttached`,
                               `RevokeLivePlaybackOwnerIdle`, `RevokeLivePlaybackOwnerRunning`,
                               `RevokeSupervisor`, `RevokeSupervisorAttached`,
                               `RevokeSupervisorDestroyed`, `RevokeSupervisorIdle`,
                               `RevokeSupervisorRetired`, `RevokeSupervisorRunning`,
                               `RevokeSupervisorStopped`, `RevokeSupervisorTrustEdge`,
                               `RuntimeCheckpointProjectionResolved`, `SatisfyWaitAllAttached`,
                               `SatisfyWaitAllIdle`, `SatisfyWaitAllRetired`,
                               `SatisfyWaitAllRunning`, `SatisfyWaitAllStopped`,
                               `ScheduleSurfaceCompletion`,
                               `session_live_assistant_final_chars`,
                               `session_live_assistant_final_digest`,
                               `session_live_assistant_playback_content_index`,
                               `session_live_assistant_playback_item_id`,
                               `session_live_assistant_playback_response_id`,
                               `session_live_assistant_terminal_observation`,
                               `session_live_assistant_terminal_prefix_chars`,
                               `session_live_assistant_terminal_prefix_digest`,
                               `session_live_channel_id`, `session_live_interaction_id`,
                               `session_live_provisional_transcript_present`,
                               `session_live_transcript_reconciliation`,
                               `SessionArchiveResolved`, `SessionBuildStatePersistAuthorized`,
                               `SessionBuildStateRestoreAuthorized`, `SessionContextAdvanced`,
                               `SessionDocumentLifecycleMergeResolved`,
                               `SessionEventStreamCloseResolved`,
                               `SessionEventStreamOpenResolved`,
                               `SessionEventStreamTerminalResolved`,
                               `SessionLifecycleTerminalRecovered`,
                               `SessionLlmReconfigurePlanResolved`,
                               `SessionMetadataPersistAuthorized`,
                               `SessionResumeOverridesAuthorized`,
                               `SessionResumeOverridesRejected`, `SessionRevivalResolved`,
                               `SessionStoreRecoverySourceResolved`,
                               `SessionToolResultsApplied`, `SetTurnToolOverlay`,
                               `SetTurnToolOverlayAttached`, `SetTurnToolOverlayIdle`,
                               `SetTurnToolOverlayRetired`, `SetTurnToolOverlayRunning`,
                               `SetTurnToolOverlayStopped`, `SpawnDrain`, `SpawnDrainAttached`,
                               `SpawnDrainIdle`, `SpawnDrainRetired`, `SpawnDrainRunning`,
                               `SpawnDrainStopped`, `SpawnDrainTask`,
                               `SpawnTerminalSupervisorCleanupDrain`, `StageDeferredNames`,
                               `StageDeferredNamesAttached`, `StageDeferredNamesIdle`,
                               `StageDeferredNamesRetired`, `StageDeferredNamesRunning`,
                               `StageDeferredNamesStopped`, `StageExperimentalLiveExecution`,
                               `StageExperimentalLiveExecutionAttached`,
                               `StageExperimentalLiveExecutionIdle`,
                               `StageExperimentalLiveExecutionRunning`,
                               `StageLiveProvisionalUserTranscript`, `StageVisibilityFilter`,
                               `StageVisibilityFilterAttached`, `StageVisibilityFilterIdle`,
                               `StageVisibilityFilterRetired`, `StageVisibilityFilterRunning`,
                               `StageVisibilityFilterStopped`, `State`, `StopDrain`,
                               `StopDrainAttached`, `StopDrainIdle`, `StopDrainRetired`,
                               `StopDrainRunning`, `StopDrainStopped`,
                               `StopTerminalSupervisorCleanupDrain`, `SubmitSupervisorRotation`,
                               `SubmitSupervisorRotationAdoptCurrentAttached`,
                               `SubmitSupervisorRotationAdoptCurrentIdle`,
                               `SubmitSupervisorRotationAdoptCurrentRunning`,
                               `SubmitSupervisorRotationConflictAttached`,
                               `SubmitSupervisorRotationConflictDestroyed`,
                               `SubmitSupervisorRotationConflictIdle`,
                               `SubmitSupervisorRotationConflictRetired`,
                               `SubmitSupervisorRotationConflictRunning`,
                               `SubmitSupervisorRotationConflictStopped`,
                               `SubmitSupervisorRotationExistingCompletedAttached`,
                               `SubmitSupervisorRotationExistingCompletedDestroyed`,
                               `SubmitSupervisorRotationExistingCompletedIdle`,
                               `SubmitSupervisorRotationExistingCompletedRetired`,
                               `SubmitSupervisorRotationExistingCompletedRunning`,
                               `SubmitSupervisorRotationExistingCompletedStopped`,
                               `SubmitSupervisorRotationExistingPendingAttached`,
                               `SubmitSupervisorRotationExistingPendingDestroyed`,
                               `SubmitSupervisorRotationExistingPendingIdle`,
                               `SubmitSupervisorRotationExistingPendingRetired`,
                               `SubmitSupervisorRotationExistingPendingRunning`,
                               `SubmitSupervisorRotationExistingPendingStopped`,
                               `SubmitSupervisorRotationExistingRejectedAttached`,
                               `SubmitSupervisorRotationExistingRejectedDestroyed`,
                               `SubmitSupervisorRotationExistingRejectedIdle`,
                               `SubmitSupervisorRotationExistingRejectedRetired`,
                               `SubmitSupervisorRotationExistingRejectedRunning`,
                               `SubmitSupervisorRotationExistingRejectedStopped`,
                               `SubmitSupervisorRotationNewAttached`,
                               `SubmitSupervisorRotationNewIdle`,
                               `SubmitSupervisorRotationNewRunning`,
                               `SubmitSupervisorRotationPersistPreflightRejectedAttached`,
                               `SubmitSupervisorRotationPersistPreflightRejectedIdle`,
                               `SubmitSupervisorRotationPersistPreflightRejectedRunning`,
                               `SubmitSupervisorRotationPersistRejectedAttached`,
                               `SubmitSupervisorRotationPersistRejectedIdle`,
                               `SubmitSupervisorRotationPersistRejectedRunning`,
                               `SubmitSupervisorRotationUnavailableAttached`,
                               `SubmitSupervisorRotationUnavailableDestroyed`,
                               `SubmitSupervisorRotationUnavailableIdle`,
                               `SubmitSupervisorRotationUnavailableRetired`,
                               `SubmitSupervisorRotationUnavailableRunning`,
                               `SubmitSupervisorRotationUnavailableStopped`,
                               `SupersedeCompletedLiveInteractionDelegationWithCancellationAttached`,
                               `SupersedeCompletedLiveInteractionDelegationWithCancellationIdle`,
                               `SupersedeCompletedLiveInteractionDelegationWithCancellationRunning`,
                               `SupersedeLiveInteraction`,
                               `SupersedeLiveInteractionWithDelegationCancellationAttached`,
                               `SupersedeLiveInteractionWithDelegationCancellationIdle`,
                               `SupersedeLiveInteractionWithDelegationCancellationRunning`,
                               `SupersedeLiveInteractionWithoutDelegationCancellationAttached`,
                               `SupersedeLiveInteractionWithoutDelegationCancellationIdle`,
                               `SupersedeLiveInteractionWithoutDelegationCancellationRetired`,
                               `SupersedeLiveInteractionWithoutDelegationCancellationRunning`,
                               `SupersedeLiveInteractionWithoutDelegationCancellationStopped`,
                               `SupervisorAuthorizeAdmissionResolved`,
                               `SupervisorBindAdmissionResolved`,
                               `SupervisorBindMaterialAdmissionResolved`,
                               `SupervisorBridgeCommandAdmissionResolved`,
                               `SupervisorRotationNextPublished`,
                               `SupervisorRotationNextPublishedAttached`,
                               `SupervisorRotationNextPublishedDestroyed`,
                               `SupervisorRotationNextPublishedIdle`,
                               `SupervisorRotationNextPublishedRetired`,
                               `SupervisorRotationNextPublishedRunning`,
                               `SupervisorRotationNextPublishedStopped`,
                               `SupervisorRotationObservationResolved`,
                               `SupervisorRotationPreviousRevoked`,
                               `SupervisorRotationPreviousRevokedAttached`,
                               `SupervisorRotationPreviousRevokedDestroyed`,
                               `SupervisorRotationPreviousRevokedIdle`,
                               `SupervisorRotationPreviousRevokedRetired`,
                               `SupervisorRotationPreviousRevokedRunning`,
                               `SupervisorRotationPreviousRevokedStopped`,
                               `SupervisorRotationSubmissionResolved`,
                               `SupervisorTrustEdgePublished`,
                               `SupervisorTrustEdgePublishedAttached`,
                               `SupervisorTrustEdgePublishedDestroyed`,
                               `SupervisorTrustEdgePublishedIdle`,
                               `SupervisorTrustEdgePublishedRetired`,
                               `SupervisorTrustEdgePublishedRunning`,
                               `SupervisorTrustEdgePublishedStopped`,
                               `SupervisorTrustEdgePublishFailed`,
                               `SupervisorTrustEdgePublishFailedAttached`,
                               `SupervisorTrustEdgePublishFailedDestroyed`,
                               `SupervisorTrustEdgePublishFailedIdle`,
                               `SupervisorTrustEdgePublishFailedRetired`,
                               `SupervisorTrustEdgePublishFailedRunning`,
                               `SupervisorTrustEdgePublishFailedStopped`,
                               `SupervisorTrustEdgeRevoked`,
                               `SupervisorTrustEdgeRevokedAttached`,
                               `SupervisorTrustEdgeRevokedDestroyed`,
                               `SupervisorTrustEdgeRevokedIdle`,
                               `SupervisorTrustEdgeRevokedRetired`,
                               `SupervisorTrustEdgeRevokedRunning`,
                               `SupervisorTrustEdgeRevokedStopped`,
                               `SupervisorTrustEdgeRevokeFailed`,
                               `SupervisorTrustEdgeRevokeFailedAttached`,
                               `SupervisorTrustEdgeRevokeFailedDestroyed`,
                               `SupervisorTrustEdgeRevokeFailedIdle`,
                               `SupervisorTrustEdgeRevokeFailedRetired`,
                               `SupervisorTrustEdgeRevokeFailedRunning`,
                               `SupervisorTrustEdgeRevokeFailedStopped`, `SurfaceApplyBoundary`,
                               `SurfaceCallFinished`, `SurfaceCallStarted`,
                               `SurfaceFinalizeRemovalClean`, `SurfaceFinalizeRemovalForced`,
                               `SurfaceMarkPendingFailed`, `SurfaceMarkPendingSucceeded`,
                               `SurfaceRegister`, `SurfaceSetRemovalTimeout`, `SurfaceShutdown`,
                               `SurfaceSnapshotAligned`, `SurfaceStageAdd`,
                               `SurfaceStageReload`, `SurfaceStageRemove`, `TranscriptEdit`,
                               `TranscriptEditAdmissionResolved`, `TranscriptEditFork`,
                               `TranscriptEditRewrite`, `TranscriptRewriteCommitted`,
                               `TransitionId`.
  - `meerkat-machine-schema`: `AbandonLiveInteraction`, `AbandonLiveOpenAdmission`,
                              `AddDirectPeerEndpoint`, `AdmitLiveAssistantPlaybackTarget`,
                              `AdmitLiveBridgeOperation`, `AdmitLiveDelegation`,
                              `AdmitLiveInteraction`, `AdmitLiveInteractionDelegation`,
                              `AdmitLiveInteractionTranscript`,
                              `AdvanceLiveContextCanonicalCoverage`, `AdvanceSessionContext`,
                              `ApplyMobPeerOverlay`, `ApplyPendingToolResults`,
                              `ArchiveSessionDocument`, `AttachMobIngress`,
                              `AttachSessionIngress`,
                              `AuthorizeInteractionTerminalOutboxAdoption`,
                              `AuthorizeLiveActiveChannelControl`, `AuthorizeLiveBridgeEffect`,
                              `AuthorizeLiveBridgeExecutionStart`,
                              `AuthorizeLiveBridgeSubmission`,
                              `AuthorizeLiveConsequentialEffect`, `AuthorizeLiveContextAppend`,
                              `AuthorizeLiveDelegationResultDelivery`,
                              `AuthorizeLiveDelegationResultRelease`,
                              `AuthorizeLiveDelegationTranscriptTerminalCancellation`,
                              `AuthorizeLiveDelegationWorkerRetirement`,
                              `AuthorizeLiveDelegationWorkerStart`,
                              `AuthorizeSessionBuildStatePersist`,
                              `AuthorizeSessionMetadataPersist`,
                              `AuthorizeSessionResumeOverrides`, `AuthorizeSupervisor`,
                              `AuthorizeSupervisorMobPeerOverlay`,
                              `BindLiveContextRecoveryChannel`,
                              `BindLiveDelegationResultRecoveryChannel`,
                              `BindLiveExecutionChannel`, `BindSupervisor`,
                              `CancelLiveBridgeOperation`, `ClaimLiveBridgeSubmissionAttempt`,
                              `ClassifyDurableTail`, `ClassifyLiveContextCommittedRow`,
                              `ClassifyLiveSessionAuthority`,
                              `ClassifyRecoveredTerminalCompletionBatch`, `ClearLocalEndpoint`,
                              `ClearTurnToolOverlay`, `CloseSurfaceConnection`,
                              `CommitDeferredNames`, `CommitVisibilityFilter`,
                              `CommsTrustReconcileRequested`, `CompleteAssistantPlayback`,
                              `CompleteLiveInteraction`, `CompleteLiveInteractionTranscript`,
                              `ConfirmLiveBridgeFinalInput`, `ConsumeLiveActiveChannelControl`,
                              `ConsumeLiveBridgeEffectAuthority`,
                              `DeclareRecoveredTerminalCompletionUnrecoverable`,
                              `DetachIngress`, `DurableTailClassified`, `EmitExternalToolDelta`,
                              `EnqueueLiveContextRow`, `ExperimentalLiveExecutionStaged`,
                              `FenceRestoredLiveBridgeOperationForRestart`,
                              `InboundPeerInteractionStateChanged`,
                              `InteractionStreamAbandoned`, `InteractionStreamAttached`,
                              `InteractionStreamCleanup`, `InteractionStreamClosedEarly`,
                              `InteractionStreamCompleted`, `InteractionStreamExpired`,
                              `InteractionStreamReserved`, `InteractionStreamStateChanged`,
                              `InteractionTerminalOutboxAdoptionAuthorized`,
                              `live_abandoned_interactions`,
                              `live_activation_receipt_by_channel`,
                              `live_active_control_operation_by_authority`,
                              `live_active_interaction_by_channel`,
                              `live_assistant_interaction_by_turn`,
                              `live_assistant_turn_channel_by_ref`,
                              `live_awaiting_assistant_interaction_by_channel`,
                              `live_bridge_agent_identity_by_operation`,
                              `live_bridge_cancellation_reason_by_operation`,
                              `live_bridge_channel_by_operation`,
                              `live_bridge_consumed_effect_authorities`,
                              `live_bridge_context_revision_by_operation`,
                              `live_bridge_effect_kind_by_authority`,
                              `live_bridge_effect_operation_by_authority`,
                              `live_bridge_effect_outcome_by_authority`,
                              `live_bridge_execution_result_digest_by_operation`,
                              `live_bridge_execution_started_operations`,
                              `live_bridge_execution_terminal_by_operation`,
                              `live_bridge_in_flight_effect_authorities`,
                              `live_bridge_interaction_by_operation`,
                              `live_bridge_model_computation_authorized_operations`,
                              `live_bridge_operation_by_channel`,
                              `live_bridge_outcome_receipt_operations`,
                              `live_bridge_outcome_receipt_required_operations`,
                              `live_bridge_phase_by_operation`,
                              `live_bridge_provider_call_by_operation`,
                              `live_bridge_provider_delegation_by_operation`,
                              `live_bridge_provider_turn_by_operation`,
                              `live_bridge_read_snapshot_authorized_operations`,
                              `live_bridge_request_digest_by_operation`,
                              `live_bridge_submission_digest_by_operation`,
                              `live_bridge_submission_output_kind_by_operation`,
                              `live_bridge_submission_state_by_operation`,
                              `live_client_context_capable_channels`,
                              `live_consequential_effect_operation_by_authority`,
                              `live_consumed_active_control_authorities`,
                              `live_context_ambiguous_no_retry`,
                              `live_context_cursor_by_channel`,
                              `live_context_delivered_append_ids`,
                              `live_context_pending_append_by_channel`,
                              `live_context_pending_channel_by_append`,
                              `live_context_pending_next_cursor_by_append`,
                              `live_context_pending_previous_cursor_by_append`,
                              `live_context_queued_append_by_cursor`,
                              `live_context_queued_commit_token_by_append`,
                              `live_context_queued_cursor_by_append`,
                              `live_context_queued_digest_by_append`,
                              `live_context_queued_disposition_by_append`,
                              `live_context_queued_session_by_append`,
                              `live_context_recovery_append_by_channel`,
                              `live_context_recovery_fence_by_channel`,
                              `live_context_recovery_generation_by_channel`,
                              `live_context_recovery_identity_by_channel`,
                              `live_context_recovery_replacement_by_channel`,
                              `live_context_recovery_runtime_id_by_channel`,
                              `live_context_recovery_seed_cursor_by_channel`,
                              `live_context_recovery_session_by_channel`,
                              `live_context_recovery_source_by_replacement`,
                              `live_delegation_cancellation_reason_by_operation`,
                              `live_delegation_interaction_by_channel`,
                              `live_delegation_interaction_by_operation`,
                              `live_delegation_late_terminal_operations`,
                              `live_delegation_operation_by_channel`,
                              `live_delegation_provider_turn_by_channel`,
                              `live_delegation_provider_turn_by_operation`,
                              `live_delegation_reconciliation_by_operation`,
                              `live_delegation_result_eligible_operations`,
                              `live_delegation_worker_identity_by_operation`,
                              `live_delegation_worker_phase_by_operation`,
                              `live_delegation_worker_terminal_by_operation`,
                              `live_execution_fence_by_channel`,
                              `live_execution_generation_by_channel`,
                              `live_execution_mode_by_channel`,
                              `live_execution_phase_by_channel`,
                              `live_execution_profile_by_channel`,
                              `live_execution_runtime_id_by_channel`,
                              `live_experimental_execution_channels`,
                              `live_experimental_pending_receipt_by_channel`,
                              `live_experimental_staged_fence_by_channel`,
                              `live_experimental_staged_generation_by_channel`,
                              `live_experimental_staged_runtime_by_channel`,
                              `live_experimental_staged_seed_cursor_by_channel`,
                              `live_function_bridge_capable_channels`,
                              `live_interaction_channel_by_id`,
                              `live_playback_owner_by_channel`,
                              `live_playback_readiness_by_channel`,
                              `live_provider_interaction_by_turn`,
                              `live_provider_turn_by_channel`,
                              `live_provider_turn_channel_by_ref`,
                              `live_result_delivery_channel_by_operation`,
                              `live_result_delivery_digest_by_operation`,
                              `live_result_delivery_observation_by_operation`,
                              `live_result_delivery_operation_by_channel`,
                              `live_result_recovery_digest_by_channel`,
                              `live_result_recovery_fence_by_channel`,
                              `live_result_recovery_generation_by_channel`,
                              `live_result_recovery_identity_by_channel`,
                              `live_result_recovery_operation_by_channel`,
                              `live_result_recovery_replacement_by_channel`,
                              `live_result_recovery_runtime_id_by_channel`,
                              `live_result_recovery_seed_cursor_by_channel`,
                              `live_result_recovery_session_by_channel`,
                              `live_result_recovery_source_by_replacement`,
                              `live_result_release_disposition_by_operation`,
                              `live_result_released_operations`,
                              `live_result_speech_suppressed_operations`,
                              `live_revoked_execution_channels`,
                              `LiveActiveChannelControlAuthorityIssued`,
                              `LiveActiveChannelControlDispatchAuthorized`,
                              `LiveAssistantPlaybackFinalObserved`,
                              `LiveAssistantPlaybackFinalRecovered`,
                              `LiveAssistantPlaybackTargetAdmitted`,
                              `LiveAssistantPlaybackTargetRecovered`,
                              `LiveAssistantPlaybackTerminalObserved`,
                              `LiveAssistantPlaybackTerminalRecovered`,
                              `LiveAssistantPlaybackTerminalResolved`,
                              `LiveAssistantTurnStarted`, `LiveBridgeEffectAuthorityIssued`,
                              `LiveBridgeEffectDispatchAuthorized`,
                              `LiveBridgeEffectOutcomeRecorded`,
                              `LiveBridgeExecutionStartAuthorized`,
                              `LiveBridgeExecutionTerminalRecorded`,
                              `LiveBridgeFinalInputAuthorized`, `LiveBridgeOperationAdmitted`,
                              `LiveBridgeOperationCancellationAuthorized`,
                              `LiveBridgeOperationReplayObserved`,
                              `LiveBridgeOperationRetirementResolved`,
                              `LiveBridgeOutcomeReceiptRecorded`,
                              `LiveBridgeProtocolDriftCloseAuthorized`,
                              `LiveBridgeSubmissionAttemptClaimed`,
                              `LiveBridgeSubmissionAuthorized`,
                              `LiveBridgeSubmissionLocalWriteRecorded`,
                              `LiveBridgeSubmissionRecoveredAmbiguous`,
                              `LiveBridgeSubmissionResolved`, `LiveChannelCloseCustodyRevoked`,
                              `LiveChannelStatusResolved`, `LiveCommandPublicKind`,
                              `LiveConsequentialEffectAuthorized`,
                              `LiveContextAmbiguityRecoveryAuthorized`,
                              `LiveContextAppendAuthorized`, `LiveContextAppendResolved`,
                              `LiveContextCanonicalCoverageAdvanced`,
                              `LiveContextCommittedRowClassified`,
                              `LiveContextRecoveryChannelBound`, `LiveContextRowQueued`,
                              `LiveDelegationAdmitted`, `LiveDelegationCancellationAuthorized`,
                              `LiveDelegationCancellationResolved`,
                              `LiveDelegationResultAmbiguityRecoveryAuthorized`,
                              `LiveDelegationResultDeliveryAuthorized`,
                              `LiveDelegationResultDeliveryResolved`,
                              `LiveDelegationResultRecoveryChannelBound`,
                              `LiveDelegationResultReleaseAuthorized`,
                              `LiveDelegationTranscriptReconciled`,
                              `LiveDelegationWorkerRestartReconciled`,
                              `LiveDelegationWorkerRetirementAuthorized`,
                              `LiveDelegationWorkerRetirementResolved`,
                              `LiveDelegationWorkerStartAuthorized`,
                              `LiveDelegationWorkerStartResolved`,
                              `LiveDelegationWorkerTerminalRecorded`,
                              `LiveExecutionChannelBound`, `LiveExecutionModeAdmissionResolved`,
                              `LiveFinalUserTranscriptReconciled`, `LiveInteractionAbandoned`,
                              `LiveInteractionAdmitted`, `LiveInteractionCompleted`,
                              `LiveInteractionDelegationAdmitted`,
                              `LiveInteractionSupersededWithoutCancellation`,
                              `LiveInteractionTranscriptAdmitted`,
                              `LiveInteractionTranscriptCompleted`,
                              `LiveOpenAdmissionAbandoned`, `LiveOpenAdmissionRejection`,
                              `LiveOpenAdmissionResolved`, `LivePlaybackOwnerReady`,
                              `LivePlaybackOwnerRevoked`, `LiveProviderTurnFinished`,
                              `LiveProviderTurnStarted`, `LiveProvisionalUserTranscriptStaged`,
                              `LiveSessionAuthorityClassified`,
                              `LiveWebrtcAnswerAcceptedAndExecutionBound`,
                              `LiveWebsocketTokenAdmissionResolved`, `LiveWebsocketTokenIssued`,
                              `LocalEndpointChanged`, `McpServerConnected`,
                              `McpServerConnectPending`, `McpServerDisconnected`,
                              `McpServerFailed`, `McpServerReload`, `McpServerReloadRequested`,
                              `McpServerStateChanged`, `MeerkatMachineEffect`,
                              `MeerkatMachineEffectVariant`, `MeerkatMachineInput`,
                              `MeerkatMachineInputVariant`, `MeerkatMachineState`,
                              `MobEventStreamCloseResolved`, `MobEventStreamOpenResolved`,
                              `MobEventStreamTerminalResolved`,
                              `ObserveLiveAssistantPlaybackFinal`,
                              `ObserveLiveAssistantPlaybackTerminal`,
                              `ObserveLiveAssistantTurnStarted`,
                              `ObserveLiveProviderTurnStarted`, `ObserveSupervisorRotation`,
                              `PeerIngressClassified`, `PeerIngressDequeueResolved`,
                              `PeerIngressReceiveResolved`, `PeerInteractionCleanup`,
                              `PeerInteractionStateChanged`, `PeerProjectionChanged`,
                              `PeerRequestReceived`, `PeerRequestSendFailed`, `PeerRequestSent`,
                              `PeerRequestTimedOut`, `PeerResponseProgressArrived`,
                              `PeerResponseRejected`, `PeerResponseReplied`,
                              `PeerResponseReplyClassified`, `PeerResponseTerminalArrived`,
                              `PendingContinuationPublicTerminalResolved`,
                              `PendingContinuationResolved`, `PublishLocalEndpoint`,
                              `PublishSupervisorTrustEdge`, `RealtimeTranscriptAppended`,
                              `ReconcileLiveDelegationTranscript`,
                              `ReconcileLiveFinalUserTranscript`,
                              `ReconcileRevokedLiveBridgeExecutionTerminal`,
                              `ReconcileRevokedLiveDelegationWorkerAfterRestart`,
                              `RecordLiveBridgeEffectOutcome`,
                              `RecordLiveBridgeExecutionTerminal`,
                              `RecordLiveBridgeOutcomeReceipt`,
                              `RecordLiveBridgeSubmissionLocalWrite`,
                              `RecordLiveChannelRequestRejected`, `RecordLiveChannelStatus`,
                              `RecordLiveCloseClosed`, `RecordLiveCommandAccepted`,
                              `RecordLiveCommandRejected`, `RecordLiveDelegationWorkerTerminal`,
                              `RecordLiveRefreshQueued`, `RecordLiveWebrtcAnswerAccepted`,
                              `RecordLiveWebrtcAnswerAcceptedAndBindExecution`,
                              `RecordLiveWebrtcTokenIssued`, `RecordLiveWebsocketTokenIssued`,
                              `RecordMobEventStreamOpened`, `RecordMobEventStreamTerminated`,
                              `RecordSessionEventStreamOpened`,
                              `RecordSessionEventStreamTerminated`,
                              `RecoveredTerminalCompletionBatchClassified`,
                              `RecoveredTerminalCompletionDeclaredUnrecoverable`,
                              `RecoverLiveAssistantPlaybackFinal`,
                              `RecoverLiveAssistantPlaybackTarget`,
                              `RecoverLiveAssistantPlaybackTerminal`,
                              `RecoverLiveBridgeSubmission`, `RecoverSessionFromStore`,
                              `RecoverSessionLifecycleTerminal`,
                              `RefreshSupervisorBindingRoute`, `RefreshVisibleSurfaceSet`,
                              `RegisterLivePlaybackOwner`, `RejectSurfaceCall`,
                              `RemoveDirectPeerEndpoint`, `ReplaceDeferredToolAuthorityCatalog`,
                              `ReplaceFilterToolAuthorityCatalog`, `ReplaceVisibilityState`,
                              `RequestCommsDrainExitForUnregister`,
                              `RequestCompletionWaiterResolutionForUnregister`,
                              `RequestDeferredTools`, `RequestRuntimeLoopStopForUnregister`,
                              `RequestSupervisorTrustPublish`,
                              `ResolveLiveAssistantPlaybackOnChannelClose`,
                              `ResolveLiveBridgeSubmission`, `ResolveLiveContextAppend`,
                              `ResolveLiveDelegationCancellation`,
                              `ResolveLiveDelegationResultDelivery`,
                              `ResolveLiveDelegationWorkerRetirement`,
                              `ResolveLiveDelegationWorkerStart`,
                              `ResolveLiveExecutionModeAdmission`,
                              `ResolveLiveWebrtcAnswerAdmission`,
                              `ResolveLiveWebsocketTokenAdmission`,
                              `ResolveMobEventStreamClose`, `ResolvePendingContinuation`,
                              `ResolveRuntimeCheckpointProjection`,
                              `ResolveSessionDocumentLifecycleMerge`,
                              `ResolveSessionEventStreamClose`,
                              `ResolveSupervisorAuthorizeAdmission`,
                              `ResolveSupervisorBindAdmission`,
                              `ResolveSupervisorBindMaterialAdmission`,
                              `ResolveSupervisorBridgeCommandAdmission`,
                              `ResolveSupervisorCleanupCommandAdmission`,
                              `ResolveTranscriptEditAdmission`, `RestoreSessionBuildState`,
                              `ResumeSupervisorRotation`, `RetireSettledLiveBridgeOperation`,
                              `ReviveArchivedSessionDocument`, `RevokedChannelId`,
                              `RevokeLiveChannelCloseCustody`, `RevokeLivePlaybackOwner`,
                              `RevokeSupervisor`, `RevokeSupervisorTrustEdge`,
                              `RuntimeCheckpointProjectionResolved`,
                              `ScheduleSurfaceCompletion`, `session_live_assistant_final_chars`,
                              `session_live_assistant_final_digest`,
                              `session_live_assistant_playback_content_index`,
                              `session_live_assistant_playback_item_id`,
                              `session_live_assistant_playback_response_id`,
                              `session_live_assistant_terminal_observation`,
                              `session_live_assistant_terminal_prefix_chars`,
                              `session_live_assistant_terminal_prefix_digest`,
                              `session_live_channel_id`, `session_live_interaction_id`,
                              `session_live_provisional_transcript_present`,
                              `session_live_transcript_reconciliation`,
                              `SessionArchiveResolved`, `SessionBuildStatePersistAuthorized`,
                              `SessionBuildStateRestoreAuthorized`, `SessionContextAdvanced`,
                              `SessionDocumentEffect`, `SessionDocumentEffectVariant`,
                              `SessionDocumentInput`, `SessionDocumentInputVariant`,
                              `SessionDocumentLifecycleMergeResolved`,
                              `SessionDocumentMachineState`, `SessionEventStreamCloseResolved`,
                              `SessionEventStreamOpenResolved`,
                              `SessionEventStreamTerminalResolved`,
                              `SessionLifecycleTerminalRecovered`,
                              `SessionLlmReconfigurePlanResolved`,
                              `SessionMetadataPersistAuthorized`,
                              `SessionResumeOverridesAuthorized`,
                              `SessionResumeOverridesRejected`, `SessionRevivalResolved`,
                              `SessionStoreRecoverySourceResolved`, `SessionToolResultsApplied`,
                              `SetTurnToolOverlay`, `SpawnDrain`, `SpawnDrainTask`,
                              `StageDeferredNames`, `StageExperimentalLiveExecution`,
                              `StageLiveProvisionalUserTranscript`, `StageVisibilityFilter`,
                              `StopDrain`, `SubmitSupervisorRotation`,
                              `SupersedeLiveInteraction`,
                              `SupervisorAuthorizeAdmissionResolved`,
                              `SupervisorBindAdmissionResolved`,
                              `SupervisorBindMaterialAdmissionResolved`,
                              `SupervisorBridgeCommandAdmissionResolved`,
                              `SupervisorRotationNextPublished`,
                              `SupervisorRotationObservationResolved`,
                              `SupervisorRotationPreviousRevoked`,
                              `SupervisorRotationSubmissionResolved`,
                              `SupervisorTrustEdgePublished`,
                              `SupervisorTrustEdgePublishFailed`, `SupervisorTrustEdgeRevoked`,
                              `SupervisorTrustEdgeRevokeFailed`, `SurfaceApplyBoundary`,
                              `SurfaceCallFinished`, `SurfaceCallStarted`,
                              `SurfaceFinalizeRemovalClean`, `SurfaceFinalizeRemovalForced`,
                              `SurfaceMarkPendingFailed`, `SurfaceMarkPendingSucceeded`,
                              `SurfaceRegister`, `SurfaceSetRemovalTimeout`, `SurfaceShutdown`,
                              `SurfaceSnapshotAligned`, `SurfaceStageAdd`, `SurfaceStageReload`,
                              `SurfaceStageRemove`, `TranscriptEdit`,
                              `TranscriptEditAdmissionResolved`, `TranscriptRewriteCommitted`.
  - `meerkat-rpc`: `Caches`, `experimental_live_open_authority`,
                   `experimental_live_playback_custodies`, `Library`, `LiveOpenHandlerContext`,
                   `SessionRuntime`, `Users`.
  - `meerkat-runtime`: `AbandonLiveInteraction`, `AbandonLiveOpenAdmission`,
                       `AddDirectPeerEndpoint`, `AdmitLiveBridgeOperation`,
                       `AdmitLiveDelegation`, `AdmitLiveInteraction`,
                       `AdmitLiveInteractionDelegation`, `AdvanceLiveContextCanonicalCoverage`,
                       `AdvanceSessionContext`, `ApplyMobPeerOverlay`, `AttachMobIngress`,
                       `AttachSessionIngress`, `AuthorizeInteractionTerminalOutboxAdoption`,
                       `AuthorizeLiveActiveChannelControl`, `AuthorizeLiveBridgeEffect`,
                       `AuthorizeLiveBridgeExecutionStart`, `AuthorizeLiveBridgeSubmission`,
                       `AuthorizeLiveConsequentialEffect`, `AuthorizeLiveContextAppend`,
                       `AuthorizeLiveDelegationResultDelivery`,
                       `AuthorizeLiveDelegationResultRelease`,
                       `AuthorizeLiveDelegationTranscriptTerminalCancellation`,
                       `AuthorizeLiveDelegationWorkerRetirement`,
                       `AuthorizeLiveDelegationWorkerStart`, `AuthorizeSupervisor`,
                       `AuthorizeSupervisorMobPeerOverlay`, `BindLiveContextRecoveryChannel`,
                       `BindLiveDelegationResultRecoveryChannel`, `BindLiveExecutionChannel`,
                       `BindSupervisor`, `CancelLiveBridgeOperation`,
                       `ClaimLiveBridgeSubmissionAttempt`,
                       `ClassifyRecoveredTerminalCompletionBatch`, `ClearLocalEndpoint`,
                       `ClearTurnToolOverlay`, `CloseSurfaceConnection`, `CommitDeferredNames`,
                       `CommitVisibilityFilter`, `CommsTrustReconcileRequested`,
                       `CompleteAssistantPlayback`, `CompleteLiveInteraction`,
                       `ConfirmLiveBridgeFinalInput`, `ConsumeLiveActiveChannelControl`,
                       `ConsumeLiveBridgeEffectAuthority`,
                       `DeclareRecoveredTerminalCompletionUnrecoverable`, `DetachIngress`,
                       `EmitExternalToolDelta`, `EnqueueLiveContextRow`,
                       `ExperimentalLiveExecutionStaged`,
                       `FenceRestoredLiveBridgeOperationForRestart`,
                       `InboundPeerInteractionStateChanged`, `InteractionStreamAbandoned`,
                       `InteractionStreamAttached`, `InteractionStreamCleanup`,
                       `InteractionStreamClosedEarly`, `InteractionStreamCompleted`,
                       `InteractionStreamExpired`, `InteractionStreamReserved`,
                       `InteractionStreamStateChanged`,
                       `InteractionTerminalOutboxAdoptionAuthorized`,
                       `live_abandoned_interactions`, `live_activation_receipt_by_channel`,
                       `live_active_control_operation_by_authority`,
                       `live_active_interaction_by_channel`,
                       `live_assistant_interaction_by_turn`,
                       `live_assistant_turn_channel_by_ref`,
                       `live_awaiting_assistant_interaction_by_channel`,
                       `live_bridge_agent_identity_by_operation`,
                       `live_bridge_cancellation_reason_by_operation`,
                       `live_bridge_channel_by_operation`,
                       `live_bridge_consumed_effect_authorities`,
                       `live_bridge_context_revision_by_operation`,
                       `live_bridge_effect_kind_by_authority`,
                       `live_bridge_effect_operation_by_authority`,
                       `live_bridge_effect_outcome_by_authority`,
                       `live_bridge_execution_result_digest_by_operation`,
                       `live_bridge_execution_started_operations`,
                       `live_bridge_execution_terminal_by_operation`,
                       `live_bridge_in_flight_effect_authorities`,
                       `live_bridge_interaction_by_operation`,
                       `live_bridge_model_computation_authorized_operations`,
                       `live_bridge_operation_by_channel`,
                       `live_bridge_outcome_receipt_operations`,
                       `live_bridge_outcome_receipt_required_operations`,
                       `live_bridge_phase_by_operation`,
                       `live_bridge_provider_call_by_operation`,
                       `live_bridge_provider_delegation_by_operation`,
                       `live_bridge_provider_turn_by_operation`,
                       `live_bridge_read_snapshot_authorized_operations`,
                       `live_bridge_request_digest_by_operation`,
                       `live_bridge_submission_digest_by_operation`,
                       `live_bridge_submission_output_kind_by_operation`,
                       `live_bridge_submission_state_by_operation`,
                       `live_client_context_capable_channels`,
                       `live_consequential_effect_operation_by_authority`,
                       `live_consumed_active_control_authorities`,
                       `live_context_ambiguous_no_retry`, `live_context_cursor_by_channel`,
                       `live_context_delivered_append_ids`,
                       `live_context_pending_append_by_channel`,
                       `live_context_pending_channel_by_append`,
                       `live_context_pending_next_cursor_by_append`,
                       `live_context_pending_previous_cursor_by_append`,
                       `live_context_queued_append_by_cursor`,
                       `live_context_queued_commit_token_by_append`,
                       `live_context_queued_cursor_by_append`,
                       `live_context_queued_digest_by_append`,
                       `live_context_queued_disposition_by_append`,
                       `live_context_queued_session_by_append`,
                       `live_context_recovery_append_by_channel`,
                       `live_context_recovery_fence_by_channel`,
                       `live_context_recovery_generation_by_channel`,
                       `live_context_recovery_identity_by_channel`,
                       `live_context_recovery_replacement_by_channel`,
                       `live_context_recovery_runtime_id_by_channel`,
                       `live_context_recovery_seed_cursor_by_channel`,
                       `live_context_recovery_session_by_channel`,
                       `live_context_recovery_source_by_replacement`,
                       `live_delegation_cancellation_reason_by_operation`,
                       `live_delegation_interaction_by_channel`,
                       `live_delegation_interaction_by_operation`,
                       `live_delegation_late_terminal_operations`,
                       `live_delegation_operation_by_channel`,
                       `live_delegation_provider_turn_by_channel`,
                       `live_delegation_provider_turn_by_operation`,
                       `live_delegation_reconciliation_by_operation`,
                       `live_delegation_result_eligible_operations`,
                       `live_delegation_worker_identity_by_operation`,
                       `live_delegation_worker_phase_by_operation`,
                       `live_delegation_worker_terminal_by_operation`,
                       `live_execution_fence_by_channel`,
                       `live_execution_generation_by_channel`, `live_execution_mode_by_channel`,
                       `live_execution_phase_by_channel`, `live_execution_profile_by_channel`,
                       `live_execution_runtime_id_by_channel`,
                       `live_experimental_execution_channels`,
                       `live_experimental_pending_receipt_by_channel`,
                       `live_experimental_staged_fence_by_channel`,
                       `live_experimental_staged_generation_by_channel`,
                       `live_experimental_staged_runtime_by_channel`,
                       `live_experimental_staged_seed_cursor_by_channel`,
                       `live_function_bridge_capable_channels`,
                       `live_interaction_channel_by_id`, `live_playback_owner_by_channel`,
                       `live_playback_readiness_by_channel`,
                       `live_provider_interaction_by_turn`, `live_provider_turn_by_channel`,
                       `live_provider_turn_channel_by_ref`,
                       `live_result_delivery_channel_by_operation`,
                       `live_result_delivery_digest_by_operation`,
                       `live_result_delivery_observation_by_operation`,
                       `live_result_delivery_operation_by_channel`,
                       `live_result_recovery_digest_by_channel`,
                       `live_result_recovery_fence_by_channel`,
                       `live_result_recovery_generation_by_channel`,
                       `live_result_recovery_identity_by_channel`,
                       `live_result_recovery_operation_by_channel`,
                       `live_result_recovery_replacement_by_channel`,
                       `live_result_recovery_runtime_id_by_channel`,
                       `live_result_recovery_seed_cursor_by_channel`,
                       `live_result_recovery_session_by_channel`,
                       `live_result_recovery_source_by_replacement`,
                       `live_result_release_disposition_by_operation`,
                       `live_result_released_operations`,
                       `live_result_speech_suppressed_operations`,
                       `live_revoked_execution_channels`,
                       `LiveActiveChannelControlAuthorityIssued`,
                       `LiveActiveChannelControlDispatchAuthorized`, `LiveAssistantTurnStarted`,
                       `LiveBridgeEffectAuthorityIssued`, `LiveBridgeEffectDispatchAuthorized`,
                       `LiveBridgeEffectOutcomeRecorded`, `LiveBridgeExecutionStartAuthorized`,
                       `LiveBridgeExecutionTerminalRecorded`, `LiveBridgeFinalInputAuthorized`,
                       `LiveBridgeOperationAdmitted`,
                       `LiveBridgeOperationCancellationAuthorized`,
                       `LiveBridgeOperationReplayObserved`,
                       `LiveBridgeOperationRetirementResolved`,
                       `LiveBridgeOutcomeReceiptRecorded`,
                       `LiveBridgeProtocolDriftCloseAuthorized`,
                       `LiveBridgeSubmissionAttemptClaimed`, `LiveBridgeSubmissionAuthorized`,
                       `LiveBridgeSubmissionLocalWriteRecorded`,
                       `LiveBridgeSubmissionRecoveredAmbiguous`, `LiveBridgeSubmissionResolved`,
                       `LiveChannelCloseCustodyRevoked`, `LiveChannelStatusResolved`,
                       `LiveCommandPublicKind`, `LiveConsequentialEffectAuthorized`,
                       `LiveContextAmbiguityRecoveryAuthorized`, `LiveContextAppendAuthorized`,
                       `LiveContextAppendResolved`, `LiveContextCanonicalCoverageAdvanced`,
                       `LiveContextRecoveryChannelBound`, `LiveContextRowQueued`,
                       `LiveDelegationAdmitted`, `LiveDelegationCancellationAuthorized`,
                       `LiveDelegationCancellationResolved`,
                       `LiveDelegationResultAmbiguityRecoveryAuthorized`,
                       `LiveDelegationResultDeliveryAuthorized`,
                       `LiveDelegationResultDeliveryResolved`,
                       `LiveDelegationResultRecoveryChannelBound`,
                       `LiveDelegationResultReleaseAuthorized`,
                       `LiveDelegationTranscriptReconciled`,
                       `LiveDelegationWorkerRestartReconciled`,
                       `LiveDelegationWorkerRetirementAuthorized`,
                       `LiveDelegationWorkerRetirementResolved`,
                       `LiveDelegationWorkerStartAuthorized`,
                       `LiveDelegationWorkerStartResolved`,
                       `LiveDelegationWorkerTerminalRecorded`, `LiveExecutionChannelBound`,
                       `LiveExecutionModeAdmissionResolved`, `LiveInteractionAbandoned`,
                       `LiveInteractionAdmitted`, `LiveInteractionCompleted`,
                       `LiveInteractionDelegationAdmitted`,
                       `LiveInteractionSupersededWithoutCancellation`,
                       `LiveOpenAdmissionAbandoned`, `LiveOpenAdmissionRejection`,
                       `LiveOpenAdmissionResolved`, `LivePlaybackOwnerReady`,
                       `LivePlaybackOwnerRevoked`, `LiveProviderTurnFinished`,
                       `LiveProviderTurnStarted`, `LiveWebrtcAnswerAcceptedAndExecutionBound`,
                       `LiveWebrtcAnswerAdmissionAuthority`,
                       `LiveWebsocketTokenAdmissionResolved`, `LiveWebsocketTokenIssued`,
                       `LocalEndpointChanged`, `McpServerConnected`, `McpServerConnectPending`,
                       `McpServerDisconnected`, `McpServerFailed`, `McpServerReload`,
                       `McpServerReloadRequested`, `McpServerStateChanged`,
                       `MeerkatMachineEffect`, `MeerkatMachineEffectVariant`,
                       `MeerkatMachineInput`, `MeerkatMachineInputVariant`,
                       `MeerkatMachineState`, `MobEventStreamCloseResolved`,
                       `MobEventStreamOpenResolved`, `MobEventStreamTerminalResolved`,
                       `ObserveLiveAssistantTurnStarted`, `ObserveLiveProviderTurnStarted`,
                       `ObserveSupervisorRotation`, `PeerIngressClassified`,
                       `PeerIngressDequeueResolved`, `PeerIngressReceiveResolved`,
                       `PeerInteractionCleanup`, `PeerInteractionStateChanged`,
                       `PeerProjectionChanged`, `PeerRequestReceived`, `PeerRequestSendFailed`,
                       `PeerRequestSent`, `PeerRequestTimedOut`, `PeerResponseProgressArrived`,
                       `PeerResponseRejected`, `PeerResponseReplied`,
                       `PeerResponseReplyClassified`, `PeerResponseTerminalArrived`,
                       `PublishLocalEndpoint`, `PublishSupervisorTrustEdge`,
                       `RealtimeTranscriptAppended`, `ReconcileLiveDelegationTranscript`,
                       `ReconcileRevokedLiveBridgeExecutionTerminal`,
                       `ReconcileRevokedLiveDelegationWorkerAfterRestart`,
                       `RecordLiveBridgeEffectOutcome`, `RecordLiveBridgeExecutionTerminal`,
                       `RecordLiveBridgeOutcomeReceipt`, `RecordLiveBridgeSubmissionLocalWrite`,
                       `RecordLiveChannelRequestRejected`, `RecordLiveChannelStatus`,
                       `RecordLiveCloseClosed`, `RecordLiveCommandAccepted`,
                       `RecordLiveCommandRejected`, `RecordLiveDelegationWorkerTerminal`,
                       `RecordLiveRefreshQueued`, `RecordLiveWebrtcAnswerAccepted`,
                       `RecordLiveWebrtcAnswerAcceptedAndBindExecution`,
                       `RecordLiveWebrtcTokenIssued`, `RecordLiveWebsocketTokenIssued`,
                       `RecordMobEventStreamOpened`, `RecordMobEventStreamTerminated`,
                       `RecordSessionEventStreamOpened`, `RecordSessionEventStreamTerminated`,
                       `RecoveredTerminalCompletionBatchClassified`,
                       `RecoveredTerminalCompletionDeclaredUnrecoverable`,
                       `RecoverLiveBridgeSubmission`, `RefreshSupervisorBindingRoute`,
                       `RefreshVisibleSurfaceSet`, `RegisterLivePlaybackOwner`,
                       `RejectSurfaceCall`, `RemoveDirectPeerEndpoint`,
                       `ReplaceDeferredToolAuthorityCatalog`,
                       `ReplaceFilterToolAuthorityCatalog`, `ReplaceVisibilityState`,
                       `RequestCommsDrainExitForUnregister`,
                       `RequestCompletionWaiterResolutionForUnregister`, `RequestDeferredTools`,
                       `RequestRuntimeLoopStopForUnregister`, `RequestSupervisorTrustPublish`,
                       `ResolveLiveBridgeSubmission`, `ResolveLiveContextAppend`,
                       `ResolveLiveDelegationCancellation`,
                       `ResolveLiveDelegationResultDelivery`,
                       `ResolveLiveDelegationWorkerRetirement`,
                       `ResolveLiveDelegationWorkerStart`, `ResolveLiveExecutionModeAdmission`,
                       `ResolveLiveWebrtcAnswerAdmission`, `ResolveLiveWebsocketTokenAdmission`,
                       `ResolveMobEventStreamClose`, `ResolveSessionEventStreamClose`,
                       `ResolveSupervisorAuthorizeAdmission`, `ResolveSupervisorBindAdmission`,
                       `ResolveSupervisorBindMaterialAdmission`,
                       `ResolveSupervisorBridgeCommandAdmission`,
                       `ResolveSupervisorCleanupCommandAdmission`,
                       `ResolveTranscriptEditAdmission`, `ResumeSupervisorRotation`,
                       `RetireSettledLiveBridgeOperation`, `RevokedChannelId`,
                       `RevokeLiveChannelCloseCustody`, `RevokeLivePlaybackOwner`,
                       `RevokeSupervisor`, `RevokeSupervisorTrustEdge`,
                       `ScheduleSurfaceCompletion`, `SessionContextAdvanced`,
                       `SessionEventStreamCloseResolved`, `SessionEventStreamOpenResolved`,
                       `SessionEventStreamTerminalResolved`, `SessionLlmCapabilitySurface`,
                       `SessionLlmReconfigureHost`, `SessionLlmReconfigurePlanResolved`,
                       `SetTurnToolOverlay`, `SpawnDrain`, `SpawnDrainTask`,
                       `StageDeferredNames`, `StageExperimentalLiveExecution`,
                       `StageVisibilityFilter`, `StopDrain`, `SubmitSupervisorRotation`,
                       `SupersedeLiveInteraction`, `SupervisorAuthorizeAdmissionResolved`,
                       `SupervisorBindAdmissionResolved`,
                       `SupervisorBindMaterialAdmissionResolved`,
                       `SupervisorBridgeCommandAdmissionResolved`,
                       `SupervisorRotationNextPublished`,
                       `SupervisorRotationObservationResolved`,
                       `SupervisorRotationPreviousRevoked`,
                       `SupervisorRotationSubmissionResolved`, `SupervisorTrustEdgePublished`,
                       `SupervisorTrustEdgePublishFailed`, `SupervisorTrustEdgeRevoked`,
                       `SupervisorTrustEdgeRevokeFailed`,
                       `supports_mid_conversation_system_messages`, `SurfaceApplyBoundary`,
                       `SurfaceCallFinished`, `SurfaceCallStarted`,
                       `SurfaceFinalizeRemovalClean`, `SurfaceFinalizeRemovalForced`,
                       `SurfaceMarkPendingFailed`, `SurfaceMarkPendingSucceeded`,
                       `SurfaceRegister`, `SurfaceSetRemovalTimeout`, `SurfaceShutdown`,
                       `SurfaceSnapshotAligned`, `SurfaceStageAdd`, `SurfaceStageReload`,
                       `SurfaceStageRemove`, `TranscriptEditAdmissionResolved`,
                       `transport_seal`.

- **Behaviour-only live authority break (not measurable by `cargo-semver-checks`).**
  Experimental GPT Live open requests accept only `version` and trusted
  `profile_id`; caller-selected provider, model, server, and auth-binding
  authority is removed. The voice model receives only the fixed ClientContext
  delegation path and no ordinary, MCP, consumer, or direct-effect tools.

- **Exact changed callable and field signatures:**
  `RealtimeSessionOpenProjection.owner_session_id`,
  `PendingPromotionCleanup.replenish_staged_capacity_admission`,
  `PendingPromotionCleanup.recover_materialized_staged_capacity_admission`,
  `LiveProjectionSink.truncate_assistant_transcript`, `handle_live_close`,
  `SessionRuntime.truncate_live_output`,
  `SessionLlmReconfigureHost.apply_live_session_llm_identity`,
  `SessionLlmCapabilitySurface.image_generation`,
  `SessionLlmCapabilitySurface.realtime`, and
  `SessionLlmCapabilitySurface.call_timeout_secs` changed construction or
  parameter shape as reported by `cargo-semver-checks`.

- **Behaviour-only activation break (not measurable by `cargo-semver-checks`).**
  System prompt replacement now refuses a target with effective durable
  instruction activations. RuntimeStore decorators must implement the external
  write-fence contract or activation fails closed as unsupported.

### Added

- Experimental GPT Live channels now ship a qualified ClientContext delegation
  path. The realtime model receives no provider, MCP, consumer, or direct-effect
  tools; it can request work only through the fixed client-context bridge, which
  delegates to an ordinary durable Meerkat executor and projects the bounded
  result back into the live conversation.
- Durable instruction activation adds revisioned instruction identities,
  digests, activation expectations and receipts, store-fenced commit authority,
  session-runtime activation and read APIs, and matching JSON-RPC, Python, and
  TypeScript surfaces. Unsupported model/runtime combinations fail closed
  before activation authority is published.
- The ordinary Mob MCP tool surface adds `fork_off`, a separate durable
  fork-and-run capability with bounded result certification. It is not exposed
  to the realtime voice model and does not share the voice channel's authority.

### Changed

- Experimental GPT Live open requests now carry only a trusted `profile_id`.
  The host owns the fixed OpenAI `gpt-live-1-codex` execution identity and its
  same-realm OAuth binding. The unqualified Responses/FunctionBridge path stays
  unadvertised and fail-closed.
- GPT Live delegation now retains exact work and result identities across
  restart, fences superseded worker results before speech authorization, and
  admits result projection only after the canonical final user transcript turn.

## [0.8.29] - 2026-08-25

### Added

- `MobStorage::created_definition` reads the latest durable `MobCreated`
  definition without building or actuating a mob, allowing hosts to compare an
  edited composition against the event-log authority before resume.

### Fixed

- Mob resume now recognizes a structurally valid legacy runtime alias at any
  generation when its embedded durable identity matches the requested member.
  Exact mob and role checks remain required, and malformed generations or
  different identities continue to fail closed.

## [0.8.28] - 2026-08-24

### Added

- `MobHandle::respawn_with_successor_spec` atomically replaces a member from a
  fully lowered `SpawnMemberSpec` while Meerkat retains authority over
  predecessor retirement, successor generation and fence minting, fresh
  session creation, and topology restoration. `MemberRespawnReceipt::runtime_id`
  exposes the exact committed successor runtime identity to bridge surfaces.

### Fixed

- `require_existing` adoption now preserves the complete ordered durable
  system-prompt transcript as the sole prompt authority. Explicit lossy prompt
  restatements are rejected instead of replacing an existing multi-row
  transcript with one message.
- Successor respawn now treats an omitted runtime binding as preserve-current,
  while continuing to reject an explicitly different binding. This allows
  MobKit to apply a new roster profile atomically without changing placement.
- HTML output artifacts are flushed before their path is published or opened,
  preventing an immediate reader from observing an empty or partial file.

## [0.8.27] - 2026-08-24

### Fixed

- Resuming a MobKit session now recognizes the exact comms-safe stable member
  identity as the successor of that same identity's legacy generation-zero
  runtime binding. This preserves the session across the transition to stable
  roster identities while continuing to reject wrong identities, generations,
  mobs, roles, and malformed encodings.

## [0.8.26] - 2026-08-22

### Breaking

- **The first-class member tool-policy contract adds fields to existing public
  Rust construction surfaces.** Direct struct literals must now provide the
  new policy fields on `meerkat_contracts::PortableSpawnOverlay`,
  `meerkat_core::AgentConfig`, `meerkat_core::SessionBuildOptions`,
  `meerkat_core::ResumeOverrideMask`, `meerkat_core::SessionLlmRequestPolicy`,
  `meerkat_core::SessionTooling`, `meerkat_mob::DesiredMemberOverlay`,
  `meerkat_mob::IdentityIntentRecord`, `meerkat_mob::SpawnMemberSpec`,
  `meerkat_mob::DecompiledMemberBuild`, and `meerkat::AgentBuildConfig`.
  Callers should prefer the provided defaults, builders, and wire constructors,
  which preserve the prior unmanaged and inherited behavior.
- **The exact added tool-policy fields are public API breaks.** They are
  `AgentBuildConfig.application_tool_policy`,
  `AgentBuildConfig.tool_consequence_policy_registry`,
  `PortableSpawnOverlay.tool_category_overrides`,
  `PortableSpawnOverlay.application_tool_policy`,
  `SessionBuildOptions.application_tool_policy`,
  `SessionBuildOptions.tool_consequence_policy_registry`,
  `SessionTooling.application_tool_policy`,
  `SessionLlmRequestPolicy.provider_native_tools`,
  `ResumeOverrideMask.application_tool_policy`,
  `AgentConfig.provider_native_tools`,
  `HostMemberSubstrate.tool_consequence_policy_registry`,
  `DesiredMemberOverlay.tool_category_overrides`,
  `DesiredMemberOverlay.application_tool_policy`,
  `DecompiledMemberBuild.web_search_override`, and
  `DecompiledMemberBuild.application_tool_policy`.
- **Tool-policy enum evolution changes exhaustive matches and implicit
  discriminants.** New variants are `WireResolvedToolAccessPolicy::Constraints`,
  `ToolExecutionPolicyError::EmptyConstraints`,
  `ToolAccessPolicy::Constraints`, `AgentErrorClass::PolicyIndeterminate`,
  `ToolDispatchTerminalErrorKind::PolicyDenied`,
  `ToolDispatchTerminalErrorKind::PolicyIndeterminate`,
  `ToolError::PolicyDenied`, and `ToolError::PolicyIndeterminate`. Inserting
  the new variants shifts the implicit discriminants of
  `AgentErrorClass::Mcp`, `AgentErrorClass::SessionNotFound`,
  `AgentErrorClass::Budget`, `AgentErrorClass::MaxTokens`,
  `AgentErrorClass::ContentFiltered`, `AgentErrorClass::MaxTurns`,
  `AgentErrorClass::Cancelled`, `AgentErrorClass::InvalidState`,
  `AgentErrorClass::OperationNotFound`, `AgentErrorClass::DepthLimit`,
  `AgentErrorClass::ConcurrencyLimit`, `AgentErrorClass::Config`,
  `AgentErrorClass::Internal`, `AgentErrorClass::Build`,
  `AgentErrorClass::Auth`, `AgentErrorClass::CallbackPending`,
  `AgentErrorClass::Skill`, `AgentErrorClass::StructuredOutput`,
  `AgentErrorClass::InvalidOutputSchema`, `AgentErrorClass::Hook`,
  `AgentErrorClass::Terminal`, `AgentErrorClass::NoPendingBoundary`,
  `ToolDispatchTerminalErrorKind::Other`, and
  `ToolDispatchTerminalErrorKind::CallbackPending`.
- **`ExecutionPolicyGatedDispatcher` no longer implements the auto traits
  `UnwindSafe` and `RefUnwindSafe`.** Code requiring either bound must wrap or
  otherwise isolate the dispatcher explicitly.
- **Identity convergence gains an explicit drain-and-replacement protocol.**
  Added public fields are `ClassifyIdentityReconciliation.replacement`, the
  `replacement` field of `MobMachineInput::ClassifyIdentityReconciliation`,
  `IdentityReconcileFacts.replacement`,
  `IdentityIntentRecord.convergence_directive`,
  `IdentityConvergenceStatus.active_intent_revision`, and the `wiring_custody`
  field of `IdentityIntent::Present`. New exhaustive-match cases are
  `IdentityReconcileDecision::CloseMemberAdmission`,
  `IdentityReconcileDecision::AwaitMemberDrain`,
  `IdentityReconcileDecision::DrainBlocked`,
  `IdentityReconcileDecision::CancelActiveMember`,
  `MobError::IdentityConvergenceAdmissionClosed`,
  `IdentityConvergenceCondition::DrainBlocked`, and
  `MobStoreError::IdentityAdoptionUnavailable`.
- **The new identity decisions shift the implicit discriminants of every
  later `IdentityReconcileDecision` variant.** The affected variants are
  `IdentityReconcileDecision::SealRetirementProven`,
  `IdentityReconcileDecision::SealSessionCreationConsumed`,
  `IdentityReconcileDecision::EnsureSessionAuthority`,
  `IdentityReconcileDecision::EnsureRuntimeRegistration`,
  `IdentityReconcileDecision::AwaitExternalBindingCeremony`,
  `IdentityReconcileDecision::EnsureExternalBindingReceipt`,
  `IdentityReconcileDecision::EnsureExternalBinding`,
  `IdentityReconcileDecision::EnsureMemberMaterialization`,
  `IdentityReconcileDecision::EnsureInitialDeliveryReceipt`,
  `IdentityReconcileDecision::EnsureInitialDelivery`,
  `IdentityReconcileDecision::AwaitInitialDelivery`,
  `IdentityReconcileDecision::ReconcileWiring`,
  `IdentityReconcileDecision::RetireMemberMaterialization`,
  `IdentityReconcileDecision::RetireRuntimeRegistration`,
  `IdentityReconcileDecision::ReleaseSessionAuthority`,
  `IdentityReconcileDecision::Converged`,
  `IdentityReconcileDecision::Tombstoned`, and
  `IdentityReconcileDecision::Quarantined`.
- **`MobIdentityStore` implementors must add
  `apply_member_tool_declaration` and
  `resolve_identity_convergence_block`.** Both trait methods are required at
  the canonical `meerkat_mob::MobIdentityStore` and
  `meerkat_mob::store::MobIdentityStore` public paths.

### Added

- Mobs now support a first-class, durable, revisioned per-member tool policy.
  Declarations use compare-and-swap revision authority, converge across resume
  and rematerialization, and compose allow and deny constraints conjunctively
  with call-level, provider-native, and application consequence policy. The
  sealed result is enforced at the outer dispatcher, while application
  evaluators remain narrow-only, bounded, and fail-closed. RPC, REST, MCP,
  Python, TypeScript, Web, persistence, schemas, and machine authority carry
  the same contract.
- A canonical `CompiledApplicationToolPolicy` schema and strict canonical-JSON
  parser are published for application policy providers. Unknown fields,
  absent fail-closed defaults, non-canonical encodings, and digest mismatches
  are rejected.

### Changed

- Public documentation and companion agent skills now describe the current
  checkpoint-free session authority, durable jobs, scheduling and WorkGraph,
  approvals, event projection, live transport, multi-host mobs, realms, auth,
  providers, SDKs, APIs, and CLI behavior. New guides cover durable jobs and
  storage operations.
- CI now gives ordinary unit tests a bounded named timeout, reports slow
  integration tests without killing them, validates load-bearing contracts on
  docs-only changes, and documents the observed recovery rules for cancelled
  or superseded check suites.
- The `WorkGraphStore::update_item_and_attention_cas` contract now states that
  it is one atomic store primitive. Implementations using per-key locks must
  acquire the complete item and attention key set in deterministic order and
  must not call public methods that reacquire those locks.

### Corrected

- Runtime unregister can no longer admit queued executor work after the
  machine has committed `Draining` and observed no current run. Queue dequeue
  now requires an Active registration under the same mutation gate, while an
  already-admitted run retains Active-or-Draining authority to commit and
  terminalize. Refused queued work is resolved by canonical stop rather than
  being dropped. The Mob test service also preserves exact interrupts delivered
  after run admission but before its cooperative interrupt subscription.
- Resuming a durable mob member now reasserts the current mob's `comms_name`,
  peer metadata, standard identity labels, and current conflicting labels on
  every resume, not only after a role migration. Durable-only adopter labels
  remain preserved, and admission still fails closed on mob, member, or role
  mismatch.

## [0.8.25] - 2026-08-20

### Breaking

- **`meerkat_mob::MemberLaunchMode::Resume` gains
  `resume_from_role: Option<ProfileName>`**, and the trusted remote-host
  **`meerkat_contracts::MaterializeLaunchMode::Resume` gains
  `resume_from_role: Option<String>`**. Direct struct-variant constructors must
  provide the new field. Omitted JSON remains backward compatible and means a
  strict same-role resume. `meerkat_mob::MobError` gains
  `MemberRoleMigrationRequired` and `MemberRoleMigrationRejected`; exhaustive
  matches must add both arms.
- **`meerkat_rpc::handlers::event::ExternalEventParams` is removed.** The
  `session/external_event` handler now consumes the catalog-generated
  `meerkat_contracts::SessionExternalEventParams`; downstream imports of the
  handler-local struct must move to that canonical contract type.

### Changed

- Python and TypeScript SDK transports now bind every public JSON-RPC request
  and result boundary to generated contract types. The former grandfathered
  hand-written wrapper baseline and expiry waiver are gone; signature parity
  now rejects an ad hoc wrapper shape for both new and historical methods.

### Corrected

- Python and TypeScript RPC clients now bind all 164 catalog methods to
  generated request and result contracts instead of maintaining a parallel
  hand-written wire surface. Python auth login-start requests now include the
  required realm and binding selectors, and the generated
  `session/external_event` request contract includes its required `session_id`.
  Python codegen also preserves outer object properties around root `oneOf`
  variants, while TypeScript content normalization leaves opaque bridge JSON
  unchanged.
- Durable mob members can now perform an explicitly declared, one-shot role
  migration on an exact Resume request. The declaration must name the one
  durable predecessor role; mob id and member identity never migrate, wrong or
  absent declarations fail closed, and an exact live session is refused. A
  successful cold build preserves the session and transcript while restamping
  the current comms name, typed member binding, role/profile labels, callback
  context, and explicitly configured tooling. The declaration is available on
  trusted host `SpawnMemberSpec` and remote-host materialization paths, not on
  agent-callable spawn wire commands or standing profiles. The restamp is
  forward-only against a shared durable store: rollback requires another
  declared forward migration (or separately versioned durable state), not an
  old binary silently reclaiming the predecessor role.
- Persistent session event logs no longer consume the bounded, best-effort UI
  broadcast stream. A long or bursty turn could previously outrun that ring
  while the durable projector appended events, permanently halting the session
  with `SessionEventProjectionLagged`. The singular durable projector now uses
  its own dedicated queue, while UI subscribers retain typed
  `StreamTruncated(StreamLagged { dropped })` markers when they fall behind. A
  rate-limited warning reports
  projector queue depth and high-water once the backlog reaches 1,024 events,
  and shared event envelopes avoid duplicating large payloads between the
  durable queue and ordinary broadcast stream.
- Provider failures whose class is genuinely unknown now enter the existing
  machine-authorized, bounded retry path instead of terminalizing the agent on
  the first attempt. The retry budget and exponential backoff are unchanged;
  explicit terminal classes such as invalid request, authentication failure,
  missing model, content filtering, context overflow, and oversized requests
  remain non-retryable. OpenAI Responses streaming now also recognizes the
  observed `server_is_overloaded` code as the typed retryable overload class.
  With the default policy, a persistently unknown failure can now add three
  backoff waits totaling about 3.5 seconds plus up to four provider round trips
  before terminalizing. These attempts consume the aggregate turn budget, so
  operators should check turn deadlines, stall alerts, and console read
  timeouts against their provider latency. As an intentional REST wire-contract
  consequence, such exhausted unknown failures now report error kind
  `retry_exhausted` rather than `llm_failure`; clients branching on that code
  must handle the new value.
- CLI run and resume shutdown now await the exact epoch-fenced runtime
  registration's terminal unregister before shutting down the session service.
  The wait has an explicit timeout and returns a cleanup error instead of
  acknowledging teardown while the runtime still owns the session. Mob
  crash-stop handling likewise closes the command receiver before its
  acknowledgement so no later command can be admitted behind the stop.

## [0.8.24] - 2026-08-18

### Breaking

- **`meerkat_core::BackendProfile` gains a `server: Option<String>` field** and
  **`meerkat_core::BackendProfileConfig` gains a `server: Option<String>`
  field**. Both are the canonical owner of "which `[self_hosted.servers.<id>]`
  endpoint this backend authenticates". Struct-literal constructions of either
  type must add `server: None`; `..Default::default()` and TOML ingestion are
  unaffected (the field is `#[serde(default)]` and omitted when `None`, so no
  wire break, no schema change, no SDK regeneration).
- **`meerkat_core::ProviderBindingError` gains two variants**,
  `ServerRequiresSelfHostedProvider { backend, provider, server }` and
  `EmptySelfHostedServerId { backend }`: declaring `server` on a non-self-hosted
  backend, or declaring it empty, now fails ingestion closed. Exhaustive matches
  on this enum must add both arms.
- **`meerkat_llm_core::FactoryError` gains a `SelfHostedBinding` variant**
  carrying the new `meerkat_core::SelfHostedConnectionError`. Exhaustive matches
  on `FactoryError` must add the arm.
- **`AgentEvent` gains two variants**, `TurnUsageAccountingUnmeasured` and
  `TurnUsageAccountingIdentityDisputed`. The enum is `#[non_exhaustive]`, so a
  match with a wildcard arm keeps compiling; both discriminators are added to
  `meerkat_contracts::KNOWN_AGENT_EVENT_TYPES` and to every generated SDK
  inventory, without which a version-matched Python/TypeScript client would
  reject them as `UNKNOWN_EVENT_TYPE`.

- **`AgentEvent::TurnCompleted.usage` is now `Option<TurnUsage>`** (was
  `TurnUsage`), and `meerkat_core::agent`'s internal
  `validate_provider_turn_usage_identity` is replaced by
  `classify_provider_turn_usage_identity` returning a `TurnUsageIdentityVerdict`
  instead of a `Result`. `agent::compact::CompactionOutcome` gains
  `summary_usage_identity_dispute`. On the wire the measured case is unchanged:
  `usage` is skipped when absent, so every existing `turn_completed` row
  serializes byte-for-byte as before. Consumers that unconditionally read
  `event.usage` must handle the absent case, and must SKIP it rather than fold
  it in as zero. The Python and TypeScript SDKs type it optional
  (`TurnCompleted.usage: Usage | None`, `TurnCompletedEvent.usage?: Usage`).

- **`meerkat::surface::RUNTIME_HEALTH_DIMENSIONS` is now `[&str; 5]`** (was
  `[&str; 4]`). The declared runtime-health coverage gains `session_run_start`;
  downstream code binding the constant by its exact array type must add the new
  dimension. The `checks` map itself is an open string map on the wire, so the
  new key is additive: no wire break, no schema change, no SDK regeneration.
- **`meerkat_contracts::JobHealthSummary` loses `delivery_backlog: u64`** and
  gains `pending_outbox_jobs: u64`, `runtime_inbox_backlog: u64` and
  `coverage: JobHealthCoverage`. The single `delivery_backlog` number conflated
  two different queues - jobs owing an outbox notification, and deliveries a
  runtime has not drained - so one of them could be zero for the wrong reason.
  The struct is not `#[non_exhaustive]`, so struct-literal constructions and
  exhaustive field reads must be updated.
- **`meerkat_contracts::JobHealthStatus` gains an `Unreadable` variant.** The
  census can now say it did not look, which it previously could not express;
  exhaustive matches must add the arm. SDK literal unions are regenerated.

  For adopters folding this into a two-state surface: `Unreadable` is a THIRD
  state and no boolean carries it. `stale_leases == 0` means none were SEEN, not
  that none EXIST. Folding `Unreadable` into "healthy" reintroduces exactly the
  blindness this change removes - a green board over an unexamined store.
  Folding it into "degraded" pages an operator about a fleet that may be
  entirely fine. If a boolean is unavoidable, prefer NOT-healthy so the failure
  is visible rather than silent, but the intended consumption is to surface the
  third state distinctly and say WHICH term could not be measured.
- **`meerkat_contracts::JobHealthCoverage` is a new wire enum**
  (`complete` | `truncated { scanned, limit }`), so a saturated census window is
  a typed fact rather than a silent count.
- **`meerkat_jobs::JobHealthSnapshot::is_degraded()` is REMOVED**, replaced by
  `reading() -> JobHealthReading`. The removal is deliberate rather than a
  deprecation: `is_degraded()` collapsed "nothing is wrong" and "I could not
  look" into one boolean, and deleting it made the compiler find every consumer.
  `delivery_backlog` on the same type is likewise renamed to
  `pending_outbox_jobs`, and `coverage` is added.
- **`meerkat_jobs::DetachedJobStore` gains two REQUIRED methods**,
  `count_pending_outbox_jobs(realm_id)` and
  `list_census_candidates(realm_id, limit)`. They are deliberately not
  defaulted: a default falling back to a capped scan would silently reintroduce
  exactly the age-blindness this change exists to remove. Out-of-tree
  implementors must implement both.
- **`meerkat_runtime::RuntimeStore` gains `list_runtime_delivery_authorities()`**,
  which IS defaulted (returning `Unsupported`, which the census maps to
  `Unreadable`), so existing implementors do not break.
- **`meerkat_comms::RegistrationOutcome::ReboundOwnName` is removed.** The enum
  is not `#[non_exhaustive]`, so any `match` naming that variant, and any
  exhaustive `match` that relied on it existing, stops compiling. There are now
  three outcomes: `Registered`, `ReplacedPubkey { evicted_name }`,
  `Rejected { reason }`.
- **BEHAVIOUR-ONLY, NO TOOL WILL CATCH THIS: there is now ONE claim rule for a
  comms participant name - publish only while the name is UNBOUND.**
  `InprocRegistry::register_with_meta_in_namespace` (and everything reaching it:
  `InprocRegistry::register`, `CommsRuntime::inproc_only*`,
  `PreparedCommsRuntime::publish`, and therefore `MobSupervisorBridge::new`)
  used to fork on key identity: a FOREIGN key was refused, while the SAME key
  rebound the live route onto its newer inbox generation and reported
  `ReboundOwnName`. That same-key arm is gone. A live route under the claimed
  name is now refused whichever key holds it, with the same
  `RegistrationRejection::NameOccupied { holder_pubkey }` - `holder_pubkey` may
  now be the claimant's own key, and it is evidence of who holds the name, not a
  statement that the claimant is a different peer.

  What flips from silent success to typed failure: **building a second runtime
  for one participant name while the predecessor is still published.** In mob
  terms, constructing a second `MobSupervisorBridge`/mob runtime for one mob id
  under the same persisted supervisor authority while the predecessor actor is
  alive. This was never safe: nothing above comms excluded two live hosts of one
  mob, so in-proc route occupancy was in practice the only guard, and the
  same-key case is exactly where the displaced route is most likely still live.

  Succession is admitted on EVIDENCE, in one of two forms, both of which already
  existed:
  - the incumbent declares a generation-exact release -
    `CommsRuntime::retire_inproc_route`, reached by `MobHandle::shutdown()` (and
    by the test-support `crash_stop_preserving_durable_work_for_test`) via
    `MobSupervisorBridge::shutdown` - after which the name is unbound and the
    successor publishes ordinarily, including under the SAME authority key;
  - or the successor hands in the exact predecessor generation through
    `PreparedCommsRuntime::publish_replacing` /
    `InprocRegistry::replace_sender_in_namespace`. Supervisor rotation already
    took this path, so live rotation is unaffected.

  Not changed: `ReplacedPubkey` (one key renaming itself onto a FREE name) is
  still admitted - the claimed name is unbound, which is the one rule. Session
  identities are unaffected in ordering terms: `SessionClaimHandle::try_acquire`
  still fails closed with `SessionIdentityInUse` before registration is reached.

  The refusal message changed on both layers and is now remedy-first: it says
  the incumbent has not released the route and must be retired first, names
  `MobHandle::shutdown`, states that holding the same authority key is not a
  claim on the route, and marks the public key as evidence rather than a
  key/trust failure. Code matching the old prose must match the typed variant
  (`MobError::ParticipantNameOccupied`, or
  `CommsRuntimeError::InprocRegistrationRejected`).
- **`meerkat_core::BudgetLimits` gains the field `max_turn_duration:
  Option<Duration>`.** Struct-literal constructors of `BudgetLimits` must add
  it (`max_turn_duration: None` preserves today's behaviour exactly); builder
  and `..Default::default()` users are unaffected. The field is
  `#[serde(default, skip_serializing_if = "Option::is_none")]`, so persisted
  and wire payloads written by older versions still deserialize, and `None` -
  the default - keeps every existing deployment on exactly the behaviour it
  has today. The skip is load-bearing: a spec carrying no turn ceiling keeps
  its historical canonical bytes, so the frozen portable-spec digest pin and
  every `spec_digest` already recorded in host stores still match, exactly as
  `PortableToolConfig.read_only` did in 0.8.23. No default value is being
  turned on in this release: see the proposal under Added.
- **`meerkat_mcp_server::BudgetLimitsInput` gains
  `max_turn_duration_secs: Option<u64>`** (`#[serde(default)]`, additive on the
  MCP tool input schema; struct-literal constructors must add the field).
- **`meerkat_core::LimitsConfig` gains
  `max_turn_duration: Option<Duration>`.** This is the configuration twin of
  `BudgetLimits::max_turn_duration`; struct-literal constructors must add the
  field (`None` preserves the previous unbounded behavior).
- **The generated Meerkat machine API gains explicit terminal-completion
  recovery and unregister-drain facts.** Struct and struct-variant constructors
  and destructuring patterns must add the fields (or use `..` where legal), and
  exhaustive enum matches must add the new variants. The additions are:
  - `SessionRegistrationRejectReasonKind::UnregisterTeardownInProgress`;
  - `SessionRegistrationRejected` gains
    `unregister_runtime_loop_drain_pending`,
    `unregister_comms_drain_exit_pending`, and
    `unregister_completion_waiter_drain_pending`;
  - `State` and `MeerkatMachineState` gain
    `runtime_completion_result_resolved`;
  - `MeerkatMachineEffect::SessionRegistrationRejected` gains
    `unregister_runtime_loop_drain_pending`,
    `unregister_comms_drain_exit_pending`, and
    `unregister_completion_waiter_drain_pending`;
  - `Effect`, `EffectKind`, `MeerkatMachineEffect`, and
    `MeerkatMachineEffectVariant` gain
    `RecoveredTerminalCompletionBatchClassified` and
    `RecoveredTerminalCompletionDeclaredUnrecoverable`;
  - `Input`, `InputKind`, `MeerkatMachineInput`, and
    `MeerkatMachineInputVariant` gain
    `ClassifyRecoveredTerminalCompletionBatch` and
    `DeclareRecoveredTerminalCompletionUnrecoverable`.
- **The generated Meerkat machine `TransitionId` enum gains 32 variants.**
  Exhaustive matches must add the new arms. The exact additions are
  `RegisterSessionRefusedUnregisterDrainingIdle`,
  `RegisterSessionRefusedUnregisterDrainingAttached`,
  `RegisterSessionRefusedUnregisterDrainingRunning`,
  `RegisterSessionRefusedUnregisterDrainingRetired`,
  `RegisterSessionRefusedUnregisterDrainingStopped`,
  `ClassifyRecoveredTerminalCompletionBatchRecoverInitializing`,
  `ClassifyRecoveredTerminalCompletionBatchRecoverIdle`,
  `ClassifyRecoveredTerminalCompletionBatchRecoverAttached`,
  `ClassifyRecoveredTerminalCompletionBatchRecoverRunning`,
  `ClassifyRecoveredTerminalCompletionBatchRecoverRetired`,
  `ClassifyRecoveredTerminalCompletionBatchRecoverStopped`,
  `ClassifyRecoveredTerminalCompletionBatchDiscardUnrecoverableInitializing`,
  `ClassifyRecoveredTerminalCompletionBatchDiscardUnrecoverableIdle`,
  `ClassifyRecoveredTerminalCompletionBatchDiscardUnrecoverableAttached`,
  `ClassifyRecoveredTerminalCompletionBatchDiscardUnrecoverableRunning`,
  `ClassifyRecoveredTerminalCompletionBatchDiscardUnrecoverableRetired`,
  `ClassifyRecoveredTerminalCompletionBatchDiscardUnrecoverableStopped`,
  `ClassifyRecoveredTerminalCompletionBatchBlockedInitializing`,
  `ClassifyRecoveredTerminalCompletionBatchBlockedIdle`,
  `ClassifyRecoveredTerminalCompletionBatchBlockedAttached`,
  `ClassifyRecoveredTerminalCompletionBatchBlockedRunning`,
  `ClassifyRecoveredTerminalCompletionBatchBlockedRetired`,
  `ClassifyRecoveredTerminalCompletionBatchBlockedStopped`,
  `DeclareRecoveredTerminalCompletionUnrecoverableInitializing`,
  `DeclareRecoveredTerminalCompletionUnrecoverableIdle`,
  `DeclareRecoveredTerminalCompletionUnrecoverableAttached`,
  `DeclareRecoveredTerminalCompletionUnrecoverableRunning`,
  `DeclareRecoveredTerminalCompletionUnrecoverableRetired`,
  `DeclareRecoveredTerminalCompletionUnrecoverableStopped`,
  `PrepareIdleRetainingUnsettledCompletion`,
  `PrepareAttachedRetainingUnsettledCompletion`, and
  `DrainQueuedRunRetiredRetainingUnsettledCompletion`.
- **`meerkat_mob::runtime::host_actor::MobHostActorError` and
  `meerkat_mob::runtime::host_materialize::MaterializeServeError` each gain a
  `ParticipantNameOccupied` variant.** Exhaustive matches must handle the typed
  refusal. This is the host-side surface of the one-live-participant rule
  described above; the same additions are also discussed under Corrected.

### Added

- **A turn's aggregate wall-clock has an owner: `limits.max_turn_duration`.**
  Every segment of a turn was already bounded - the per-call LLM timeout, the
  300s stream-inactivity watchdog, the 600s per-tool-call timeout - and their
  SUM was bounded by nothing. Five slow-but-legal tool calls is fifty minutes
  of entirely legal non-advance, and no owner ever asked whether the turn was
  ALLOWED to take that long. `BudgetLimits::max_turn_duration` is that owner.
  Its epoch is re-armed at every run entry by `Budget::begin_turn`, which is
  the fact the pre-existing `max_duration` cannot express: `Budget::new` runs
  once when a session's agent is built, so `max_duration` measures the AGENT'S
  LIFETIME, including the idle wall-clock between turns, and would terminalize
  a turn that had done nothing. Exhaustion is deliberately a TERMINAL and not
  a report: a turn past its deadline has been invalidated - we can no longer
  say when or whether it will produce output - so unlike an accounting or
  observability fault it fails closed. It travels the EXISTING terminal path
  (`BudgetDimension::Time` -> `TurnExecutionInput::BudgetLimitExceeded` ->
  `TurnTerminalOutcome::TimeBudgetExceeded`); there is no second terminal for
  "ran out of time". The generated authority already encoded that judgement
  and the new horizon inherits it unchanged: in
  `generated::terminal_surface_mapping`, an exhausted token/tool-call budget
  classifies as `Success` (an orderly stop that still answers the caller)
  while an exhausted TIME budget classifies as `HardFailure`, surfacing as
  `AgentError::TerminalFailure { outcome: TimeBudgetExceeded, cause_kind:
  TimeBudgetExceeded, .. }`. Enforcement is at SEGMENT BOUNDARIES: an expired
  horizon never pre-empts a tool call that is already executing, so the loop cannot
  tear a tool down mid-write. The honest ceiling is therefore
  `max_turn_duration + the longest segment already in flight` - for a tool
  batch that is the largest single per-call tool timeout, since every call in
  a batch starts its clock together and its timeout wraps its concurrency
  wait, so a batch cannot sum; for an LLM call it is zero, because each call
  is wrapped with the horizon's remaining time. One known gap: a barrier-ops
  wait (`ops_lifecycle.wait_all`) is not interrupted by the horizon and stays
  unbounded - a separate seam, not closed here. Retries cannot double-count:
  the horizon is one monotonic clock from run entry. Configure with
  `[limits] max_turn_duration = "30m"`, `BudgetLimits::with_max_turn_duration`,
  the `budget_limits` field on REST/RPC/portable-spec requests, or
  `max_turn_duration_secs` on the MCP surface. `0` is rejected at config
  validation rather than producing a fleet of instantly-dead turns.
- **PROPOSED, NOT SHIPPED - a non-`None` default for
  `limits.max_turn_duration`.** The default in this release is `None`:
  unbounded turns, byte-identical behaviour to 0.8.23. That means this release
  does not by itself fix a mute member; it gives the bound an owner, a way to
  declare it, and one terminal when it blows. Turning a ceiling on by default
  is a behaviour break for every fleet at once and a wrong number kills
  legitimate long turns, so the value goes to the fleets before it ships. The
  proposal on the table is **30 minutes**, which is roughly three of today's
  600s tool calls back to back; a fleet that legitimately runs longer turns
  declares its own. If accepted, that default lands under `### Breaking` with
  the terminal it introduces named explicitly.
- **`runtime/health` measures `session_liveness` on both surfaces** - the
  dimension 0.8.23 shipped honestly unmeasured, closed by the incident that
  proved it out: a household member wrote no transcript row for five days,
  resumed ACTIVE on every boot, and read 17/17 green on every board, because
  every existing probe measured registration state while the wedge lived in
  lane truth. The new probe reports `degraded` while any registered session is
  PARKED ON QUEUED WORK, on either of two axes, both requiring the executor
  registration `Active` and no run in flight. The AGED axis: an input in the
  machine's queued phase whose `updated_at` - the instant it entered its
  current state - is older than the notice bound (120s, deliberately the same
  constant as `session_run_start` so "overdue" means one thing across the
  pipeline). The STAGE-CHURN axis: an input the machine has staged and rolled
  back at least twice (`input_attempt_counts`, the DSL's own count) that is
  queued again, with a `created_at` floor of the same bound. The second axis
  exists because the first is structurally defeated by exactly the state it
  most needs to see: every Staged -> Queued rollback re-stamps `updated_at`,
  so an input flapping through stage -> fail -> rollback faster than the
  bound reads as forever-fresh on the age axis - and the flapping member is
  the more alarming one. Queued work behind a live turn is a backlog, not a
  wedge, and is never counted; a session whose registration cannot stage
  anything is not accused of failing to. The verdict is recomputed per scrape
  from existing owners only - machine lane truth (`input_phases`,
  `input_attempt_counts`) and the ledger's per-input clocks; no new state
  exists anywhere for this probe, so there is nothing to go stale. Both reads
  are non-blocking tries taken strictly in sequence, never nested; a miss on
  either publishes `unreadable:session_liveness`, never a rung. With this,
  every declared dimension is measured on the RPC surface (REST still
  publishes `unmeasured:jobs`), and the remaining coverage boundary is stated
  in the handler doc rather than left to be discovered: a turn that BEGINS and
  then produces nothing (mid-turn progress) is a distinct dimension with no
  probe and no declared name yet.
- **A completed turn is no longer killed because token accounting was absent.**
  A provider stream that ends without ever sending a usage event used to fail
  the turn - after the caller had already streamed and read the answer, and
  before the assistant message was committed, so the transcript lost a turn the
  user had seen. The absent number is an accounting fact, and a fault may only
  terminalize what it actually invalidates: the turn now completes, the
  assistant message commits, and the absence is published as a typed marker.
  `turn_completed` carries no `usage`, and a companion
  `turn_usage_accounting_unmeasured` event names the provider and model that
  went unaccounted under the operator marker `unmeasured:turn_usage_accounting`
  (the vocabulary `runtime/health` already publishes). That marker claims only
  the absence and that no axis moved for it; whether the turn completed stays
  owned by `turn_completed`, because the marker is published at the model
  boundary and a turn can still fail after it on unrelated grounds. The token
  axis does not move: nothing is charged to the budget, nothing is added to
  session usage, and `last_input_tokens` keeps the value the last measured turn
  left. Nothing is substituted for the missing measurement - not raw
  `input_tokens` (a different denominator on cache-heavy sessions) and not
  `TurnUsage::host_declared` (which would mint provider attribution for
  counters no provider issued). Budget enforcement on measured turns is
  unchanged.

- **A turn-usage accounting identity mismatch is now disputed, not fatal.** The
  counters in that case exist and are internally consistent - the
  presented-token convention travels with the number - so only attribution is
  in question. The number is recorded and the token axis advances as usual,
  while a `turn_usage_accounting_identity_disputed` event publishes BOTH the
  active and the reported provider/model under the marker
  `disputed:turn_usage_accounting_identity`. The reported identity is never
  rewritten to the active one: an agreement nobody observed is worse than a
  stated disagreement. The same classification now routes the compaction
  summary call's accounting identity instead of failing the compaction.

- **`runtime/health` measures `session_run_start` on both surfaces** (JSON-RPC
  `runtime/health` and REST `GET /runtime/health`): `degraded` while any
  registered session holds a staged run that is overdue to begin executing -
  staged more than the watchdog's notice bound (120s) ago while machine
  authority still shows the run current with its primitive un-applied and its
  turn start signalled. The verdict is recomputed from machine truth on every
  scrape via the same classification the staged-run watchdog logs, so the wire
  claim and that log line cannot disagree; there is no latched flag anywhere,
  which is what makes a stale window degrade to clear instead of into a false
  or missing alarm. A window whose run is no longer current is positively
  resolved and never counted - including the appends-empty and retired-drain
  classes that never signal a turn start, whose stale windows must not stand a
  healthy idle session amber. A past-bound window whose run IS still current
  but cannot be interpreted (unbound runtime, unsignalled turn start), or
  whose authority cannot be read without blocking, publishes
  `unreadable:session_run_start` rather than a rung: an absence of observation
  may not publish as health, and the holder of an unreadable authority is the
  prime suspect for the wedge itself, so "could not look" must reach the
  operator instead of rolling up as `ok`. (The watchdog's own refusal to
  ESCALATE on unobservable classes is deliberately not copied here - dropping
  an apply future on an unproven condition is forbidden, but health has no
  such constraint and the opposite duty.) The observation is
  mechanical and read-only by declared contract: the moment anything other
  than the health census branches on it, it becomes a semantic fact needing a
  machine owner, and a source-grep test
  (`run_start_window_stays_out_of_machine_authority`) enforces that boundary.

### Fixed

- **An OpenAI-compatible provider that reports usage in a separate SSE event
  no longer fails every turn** (`meerkat-openai`). The chat-completions
  adapter requests `stream_options.include_usage: true`, then emitted the
  terminal `LlmEvent::Done` the moment it saw `finish_reason`. Because
  `streaming::ensure_terminal_done` stops consuming at `Done`, a usage-only
  event arriving afterwards was never read, and the turn then failed in
  `Agent::commit_calling_llm_response` with "provider turn usage is missing
  normalized accounting evidence" - AFTER the model's answer had already been
  streamed to the user, and again on every retry. vLLM emits exactly this
  order (metadata, deltas, `finish_reason`, usage-only event with empty
  `choices`, `[DONE]`), so whole self-hosted deployments were unusable.

  The adapter now latches the derived `StopReason` instead of spending it,
  keeps consuming, emits `UsageUpdate` where it lands, and emits the single
  terminal `Done` at `[DONE]`, at end of stream, or when the post-finish
  trailer window expires - whichever comes first. Two facts became typed to
  make that expressible: `parse_chat_completions_line` returns
  `ChatCompletionsLine::{Chunk, Done, Ignored}` instead of an `Option` that
  collapsed "the turn is over" and "a line this adapter does not interpret"
  into one `None`; and the stop reason is a `latched_stop` spent exactly once.
  `ChatCompletionsChunk.choices` gains `#[serde(default)]` so a usage-only
  chunk deserializes.

  **Latching the stop reason extended the read past `finish_reason`, so three
  further shapes had to stop being able to destroy a delivered turn.** Every
  byte in that new window is load-bearing, and none of it invalidates an answer
  the caller has already read:
  - **A bounded trailer window.** `DEFAULT_POST_FINISH_TRAILER_WINDOW` is 30s,
    overridable per client with
    `OpenAiCompatibleClient::with_post_finish_trailer_window`. It is ONE budget
    measured from the latch instant and re-armed by nothing - notably not by
    SSE keepalive comments, which carry no progress toward a trailer. Expiry is
    END OF STREAM, never a turn failure. Without it, a server that holds the
    connection open after finishing hung the turn indefinitely: there is no
    HTTP client timeout, and keepalive comments re-arm the agent loop's stall
    watchdog. 30s is deliberately an order of magnitude under the 300s
    `stream_inactivity_timeout` so the two cannot race - equal windows would
    have made the outcome a coin flip between this clean end-of-stream and the
    loop's RETRYABLE `StreamStalled`.
  - **A transport fault after the latch is end of stream, not failure.** A
    truncated body once the answer is complete previously surfaced as
    `ConnectionReset`, which is classified retryable - so the answer streamed,
    the turn failed, and the retry answered again. That is the shape of the
    defect above, on a different trigger.
  - **A provider error envelope is typed.** `choices` gaining
    `#[serde(default)]` made `{"object":"error",...}` and `{"error":{...}}`
    decode as valid EMPTY chunks and fall into the ignore path, so a server
    dying mid-stream presented as a SUCCESSFUL turn carrying truncated text.
    `ChatCompletionsLine::ServerError` is now checked before chunk decoding.
    Before the latch it fails the turn carrying the provider's own message
    (the answer really is truncated); after the latch it does not (the answer
    is complete and the engine is merely tearing down).

  If usage never arrives at all, that stays an accounting fact owned
  downstream - nothing here mints or substitutes accounting.

- **Reasoning and content carried in the SAME delta are no longer emitted in
  reverse order** (`meerkat-openai`). A server emits both fields in one chunk
  on the reasoning-to-content transition; the reasoning in such a chunk was
  produced BEFORE the content beside it, and 0.8.23 emitted the content first.
  Field-reported as output that reads as corruption, and nothing downstream can
  repair it: by the time the events leave the adapter, the interleaving IS the
  stream. `ReasoningDelta` now precedes `TextDelta` within a chunk.

  Why the existing reasoning test could not catch it, which is the part worth
  keeping: every chunk in that fixture carries reasoning OR content, never
  both, so no ordering question ever arose. The usage defect above has the same
  shape - every fixture co-located usage with `finish_reason`. A corpus
  assembled from one provider's chunking encodes that chunking as if it were
  the protocol. The new test records the ORDER the two channels arrive in
  rather than their contents, and is mutation-proven: restoring the 0.8.23
  order turns it red with the intended diagnostic.


- **A self-hosted model no longer resolves its credential by provider CLASS, so
  one server's secret can no longer be sent to another server's endpoint.**
  `self_hosted` classifies a private vLLM box and a hosted OpenAI-compatible
  gateway identically, and credential selection keyed on that class alone: from
  a workspace whose realm default binding was a different self-hosted server,
  `rkat -m <model-on-another-server>` picked the workspace default, sent its key
  to the model's endpoint, and surfaced the far end's `Unauthorized` - which
  explains nothing about the fact that WE chose the wrong secret. Selection is
  now constrained to the server that serves the model
  (`meerkat_core::resolve_self_hosted_binding_for_server`), regardless of which
  workspace the command runs from. A realm backend declares its endpoint with
  `server = "<server_id>"`; a binding written before that field existed is
  still identified by its `default_model`, and a lone unannotated self-hosted
  binding still serves everything (the documented one-server setup is
  unchanged). When the server cannot be identified honestly - no binding names
  it, or several unannotated bindings are reachable - the build fails closed
  with a typed error naming the server and every binding considered, instead of
  guessing. An explicit `--auth-binding realm:binding` still wins, except when
  that binding DECLARES a different server, which is a contradiction and fails
  closed (this is what protects sessions that persisted the wrong binding
  before the constraint existed).

- **An ambiguous peer delivery no longer reaches a REST caller as a failed
  send** (`meerkat-rest`, `meerkat-core`). `SendError::AmbiguousDelivery` -
  which means the envelope may already be on the receiver's queue and a later
  drain may still commit it - fell into the `send_failed` catch-all of
  `normalize_rest_comms_send_error`. "Failed" is the one word that must not
  reach this caller, because the action it invites is a retry and a retry can
  duplicate work; a model-mediated caller reading the prose will retry on that
  word alone. It now normalizes to a distinct `send_ambiguous` code carrying
  `retry_safe: false`, `required_action: "reconcile"`, and the `envelope_id`
  as the correlation evidence reconciliation needs.

  `SendError::AmbiguousDelivery`'s own documentation now states the
  precondition the recovery advice depends on: reconciliation only works if
  the dedup key is COARSER than the retry. An adopter on this exact path was
  safe because their key was `{schedule_id}:{YYYYMMDD}` - and it had been
  per-attempt timestamped first, changed only after it bit them. A per-attempt
  or timestamped key gives zero protection while looking exactly like
  idempotency.

- **`cargo test -p meerkat` reported success over zero tests** (`meerkat`). The
  facade's feature-gated tests compiled away entirely in a package-scoped lane
  because `session-store` is not a default feature there - 28 tests in
  `src/persistence.rs` (including the pre-0.8.10 bridge tests that guard a
  field-reported P0), another 28 behind `jsonl-store`, and 5 behind `comms`.
  The failure mode was silence, not a red lane. CI was never affected, because
  `cargo unit` is `--workspace` and rpc/rest/mcp-server/mob all enable
  `session-store` on the facade, so feature unification built the files on -
  which is exactly why it survived: the evidence was intact in the lane nobody
  doubted and absent from the one a developer actually runs. The facade now
  carries a path-only self dev-dependency (stripped at publish, same remedy
  `meerkat-runtime` already used) enabling the full non-live set, so coverage
  no longer depends on which sibling crates happen to share the build graph.

- **A host that goes away now releases its members' participant routes**
  (`meerkat-mob`). A member's route is held by its comms drain, which runs in
  PersistentHost mode inside a detached task holding an owned `Arc`, so dropping
  the host actor never brought the reference count to zero and the route was
  never freed. Through 0.8.23 that leak was harmless: the in-process registry
  admitted a same-key rebind over a still-published route
  (`RegistrationOutcome::ReboundOwnName`), so a successor holding the same
  durable identity simply took the name back. This release removes that arm
  deliberately - see the `### Breaking` notes - which turns the leak into a
  typed refusal: the orphaned route refuses the very host that replaces it, and
  the successor has neither a handle to it nor any action that clears it.

  Fail-closed against a LIVE incumbent is the rule working as intended.
  Fail-closed against a corpse is not, so the release now happens on the
  incumbent side, which is the rule's own first admitted evidence - the
  incumbent released the name. Per-session disposal frees the route it
  published, and host teardown frees every route the host still holds, mirroring
  the supervisor bridge's shutdown-time release that the member path never had.
  Retirement is generation-exact, so it can never remove a successor that has
  since rebound the same key.

  An operator who hits this on 0.8.23 sees "already has a live route" after a
  restart and goes looking for a live incumbent that does not exist. If you have
  read that message as a test-isolation problem, this is the other cause.

- **`@rkat/web` did not handle the two new turn-accounting events.** The Web
  SDK's compile-time exhaustiveness assertion over `AgentEvent` had no arms for
  `turn_usage_accounting_unmeasured` or
  `turn_usage_accounting_identity_disputed`, so the package failed to build
  against its own generated types. Both are now rendered with the distinction
  that matters: the unmeasured marker names the provider/model that went
  unaccounted and must be SKIPPED rather than read as zero, while the disputed
  marker shows the reported and active identities side by side, since the
  counters do exist and neither side is ever rewritten to agree.

### Known issues

- **`delegate` with inherited tooling does not work for a host that registers
  its own tools** (`meerkat-core`, `meerkat-tools`, `meerkat-mob-mcp`). NOT new
  in this release, and documented here because two adopter fleets diagnosed it
  during this train and the workaround is not discoverable from the error.

  The tool-visibility witness is derived from one field:
  `filter_witness_for_tool` builds `ToolVisibilityWitness` from
  `ToolDef.provenance`, and `has_identity_witness()` is `is_some()`. Tools that
  arrive through meerkat's own MCP client are stamped with provenance; tools
  registered through the host SDK are not, because `ToolDef::new` creates a
  `ToolDef` WITHOUT provenance. So a delegation that INHERITS parent tooling
  cannot satisfy the witness for any host-registered tool, and the refusal names
  the tool rather than the reason - a caller is told a tool is not inheritable
  and cannot discover that the real answer is "this tool never carried the field
  the check reads".

  Measured on a live fleet: of 10 real `delegate` calls, the 5 that inherited
  parent tooling (`{"mode":"inherit_parent"}`, `null`, or a `deny_overlay` over
  inherit) all failed, and the 5 that declared a fresh inline profile all
  succeeded - zero exceptions over three days. The 57 tools ever reported
  missing a witness were an exact partition: every one was host-registered,
  while `shell`, `peers`, `send_message`, `delegate`, `workgraph_create` and the
  other builtins never appeared once. A host that proxies MCP in-process, or
  simply registers its own tools - which is what the host SDK is for - therefore
  sees a 100% failure rate on the inheriting path and will read it as an
  intermittent fault.

  WORKAROUND until this is fixed: declare an explicit tool profile on the
  delegation instead of inheriting, e.g.
  `{"mode":"profile","source":{"type":"inline","tools":{...}}}`. Do not stamp
  provenance from the host: the platform contract puts that on the registry, and
  a host-side stamp would be asserting an identity the host does not own.

- **Every tool result body is carried TWICE in the event vocabulary**
  (`meerkat-core`). `AgentEvent::ToolResultReceived` and
  `AgentEvent::ToolExecutionCompleted` are both emitted for every tool call
  and differ by exactly one field, `duration_ms`; both carry the full
  `content` blocks. Any consumer that durably persists both therefore stores
  every result body twice. Measured independently on two adopter fleets with
  different storage engines: a console frame store (~348 MB against ~348 MB,
  pairwise-identical maxima, from 153 camera-tool calls) and a warehouse
  events table (89,033 rows / 3.35 GB against 88,934 rows / 1.69 GB). It is a
  PER-CALL cost, not a scale problem - one tool returning a large blob is
  enough.

  Both events are legitimate and both should exist: one is the conversation
  fact (this result entered the transcript), the other the execution fact (the
  call finished, and here is how long it took). What is wrong is that the
  execution fact's cost scales with the size of the result instead of with the
  fact itself. This release documents it on both variants rather than
  half-fixing it: `id` is present on both and is already the join key, so a
  consumer persisting both should store the body once against `id`. Note that
  capping bytes at a downstream writer does NOT fix this - it still writes the
  body twice, only smaller, and every consumer has to reimplement the cap.
  Removing `content` from the execution event is a wire break and is deferred
  to the next release rather than rushed into this one.

- **Streaming events carry no turn identity, so a durable consumer cannot
  group a turn's frames** (`meerkat-core`). `AgentEvent::TextDelta` and
  `AgentEvent::TextComplete` carry only their text. The agent emits bare
  `AgentEvent`s, and the envelope - whose `source` is the only slot that could
  carry identity - is attached downstream by a wrapper that does not know
  which turn produced the frame. Identity therefore lives in delivery context,
  which is exactly what a durable consumer does not keep.

  What this costs in the field: an adopter could not run a coverage check
  before pruning streamed deltas, because the correlation column was populated
  on 747 of 532,190 delta rows (0.14%). Of the 15 turns that were correlatable,
  2 had no terminal text frame at all - meaning for those turns the delta
  stream was the only surviving record of what the agent said. They correctly
  declined to prune. The fix is not an optional envelope field: populated from
  the paths that already carry an interaction, it would be absent on exactly
  the scheduled turns that need it, reproducing the same hole inside a wire
  contract that asserts the field exists. It needs a turn identifier minted
  unconditionally by the emitter, which is design work for the next release.

### Corrected

- **The 0.8.23 notes claimed `session_liveness` "needs a watchdog bridge that
  is 0.8.24 work". That was wrong, and this release does not clear that key.**
  `session_liveness` names the PRE-staging class - a live, open session parked
  while machine-owned lane truth still holds selectable queued work that never
  gets staged at all. The staged-run watchdog (and the new `session_run_start`
  dimension built on its classification) observes the POST-staging window,
  which opens at the durable `StageForRun` commit; it structurally cannot see
  work that never reaches staging. The two are disjoint classes.
  `unmeasured:session_liveness` therefore remains published, honestly, until a
  lane-truth probe exists. Clearing it on the strength of the staged-run
  census would have republished the one-reading-stands-for-the-whole-dimension
  defect 0.8.23 existed to remove, from inside the item that cited it.

### Changed

- The `semver-breaks` release gate now checks that reported breaks are
  declared, not merely that a `### Breaking` heading exists. It diffs the
  cargo-semver-checks findings against the section body per finding, requires
  the pending section to be stamped against the workspace version (notes left
  under `## [Unreleased]` after the version bump are now a failure), and fails
  closed when the tool run did not reach every publishable library crate. It
  also prints the three published crates cargo-semver-checks cannot look at
  (`meerkat-machine-derive`, `meerkat-machine-dsl`, `rkat`). The gate also
  runs in the release workflow now (job `release_semver_gate`), gating
  `publish_github_release`, `publish_registries`, and
  `publish_unix_release_and_homebrew`; previously it only existed in
  `make release-preflight`.

- The release now STAMPS its own notes (`scripts/stamp-changelog-release.py`,
  wired into `scripts/release-hook.sh`). The gate above requires the section a
  release is declared in to name the version being released, and nothing
  produced that - which made the requirement unsatisfiable by hand: stamping
  before the bump is rejected as "declared against a different version", and
  stamping after the tag publishes notes titled "Unreleased". The hook is the
  only point where the stamp and the version bump are the same commit, so the
  stamp is owned there. It leaves the empty `## [Unreleased]` stub the gate
  expects, is idempotent across hook re-entry, and fails closed on missing
  notes, a duplicate version, or a malformed date rather than inventing a
  heading. Covered by `make semver-breaks-selftest`, which now asserts the
  round trip - stamp, then judge the output with the gate's own checker.

- Three `meerkat-mob` error enums gain a `ParticipantNameOccupied {
  participant_name: String, holder_pubkey: meerkat_comms::PubKey }` variant.
  NONE of the three is `#[non_exhaustive]`, so any downstream `match` without a
  wildcard arm stops compiling:
  - `meerkat_mob::MobError` (also `meerkat_mob::error::MobError`)
  - `meerkat_mob::runtime::host_actor::MobHostActorError`
  - `meerkat_mob::runtime::host_materialize::MaterializeServeError`
- BEHAVIOUR-ONLY, NO TOOL WILL CATCH THIS: the three publish/construct sites
  that previously flattened a comms name-occupancy refusal into a string now
  return the typed variant instead, so the OLD MESSAGE TEXT IS GONE. Code that
  matched prose must match the variant.
  - `MobError::Internal("failed to publish prepared mob supervisor comms
    runtime: ...")` (supervisor bridge publish)
  - `MobHostActorError::Comms { detail: "failed to construct host comms runtime
    '<name>': ..." }` (host comms runtime construction)
  - `MaterializeServeError::Comms { detail: <CommsRuntimeError display> }`
    (member materialize)

  Only the name-occupancy case changes; every other comms failure still lands
  on the same string variant it did before. The new message is remedy-first
  ("retire the incumbent ... before publishing this name again") and explicitly
  disclaims the crypto reading that the 0.8.23 wording invited.
- NOT changed: `MaterializeServeError::wire_cause()` still projects the
  name-occupancy case to `BridgeRejectionCause::Internal`, exactly as the
  flattened `Comms` variant did. Remote bridge controllers see no wire change;
  the typed fact is available to in-process embedders only. Minting a dedicated
  `BridgeRejectionCause` is a `meerkat-contracts` change and was deliberately
  left out of this item.

## [0.8.23] - 2026-08-16

### Breaking

**Some entries below are BEHAVIOUR-ONLY: a public function keeps its signature
and changes what it does. `cargo-semver-checks` cannot see those, so the
`semver-breaks` gate stays green through them. They are declared here by hand
and tagged inline. Read this section rather than trusting the gate - the gate
checks that this section EXISTS, not that it covers every break.**

- The facade's and `meerkat-mob`'s `atif` re-export moves behind a new
  off-by-default `atif` feature on each crate (`meerkat/atif`,
  `meerkat-mob/atif`). Hosts that referenced `meerkat::atif` or
  `meerkat_mob::atif` must enable the feature; `rkat` and `rkat-rpc` depend on
  `meerkat-atif` directly and are unaffected.
- BEHAVIOUR-ONLY, NO TOOL WILL CATCH THIS:
  `meerkat::surface::build_runtime_host_health()` now returns
  `status: Degraded` with four `unmeasured:<dimension>` entries where it
  previously returned `status: Ok` with an empty `checks` map. The signature is
  unchanged. It is the "probed nothing" projection, and returning `Ok` from it
  meant any caller that forgot to supply a real reading shipped an invisible
  clean bill of health. An embedder calling it directly, or embedding it via
  `build_runtime_host_info()`, gets the loud default instead; supply your own
  measured value if you want a rollup that reflects a probe.
- BEHAVIOUR-ONLY, NO TOOL WILL CATCH THIS:
  `rkat storage migrate --apply --bridge-pre-0-8-10` no longer aborts the realm
  on the first domain that cannot be authenticated. It attempts every domain,
  commits the ones that succeed, and reports each refusal with a typed
  classification. A script that treated a non-zero exit as "nothing changed" is
  now wrong: read the per-domain report, which the CLI prints on failure as
  well as on success.
- `meerkat_sqlite::SchemaDomain` gains a `bridge_recoverable_versions` field.
  Any out-of-tree construction of that struct literal must supply it; an empty
  slice declares that no offline bridge recovers the domain.
- `meerkat_sqlite::MaintenanceBridgeReport` and `MaintenancePrepareReport` NO
  LONGER DERIVE `Copy`, because each gained a `refused` field listing the
  records the bridge could not carry forward. This is the least visible break
  in the release: code that relied on implicit copies (`let a = report;` and
  then using `report` again) stops compiling without ever having named `Copy`,
  and the compiler error points at the use rather than the change. Both types
  still derive `Clone`.
- `SqliteStoreError::UnledgeredDomainObjects` and
  `StoreError::UnledgeredDomainObjects` gain a `bridgeable` field carrying
  whether the pre-0.8.10 bridge can actually authenticate that domain's
  on-disk shape. Exhaustive struct-variant patterns must bind or ignore it.
- `meerkat_sqlite::SqliteStoreError` gains a `WalConversionContended` variant.
  The enum is not `#[non_exhaustive]`, so a downstream matching it exhaustively
  must add an arm. It is returned where an exhausted WAL-conversion retry
  budget previously surfaced as a bare rusqlite "database is locked".
- `meerkat_comms::RegistrationOutcome` loses the `EvictedName` and
  `ReplacedPubkeyAndEvictedName` variants and the `displaced_existing()` method,
  and gains `ReboundOwnName`. The enum is not `#[non_exhaustive]`, so both
  matches and constructions break.
- `meerkat_comms::RegistrationRejection` gains `#[non_exhaustive]` (a break for
  exhaustive matchers by itself) and a `NameOccupied` variant.
- In-process registration that previously SUCCEEDED by displacing a live
  foreign route now fails closed. An embedder that relied on displacement gets
  `CommsRuntimeError::InprocRegistrationRejected` where it used to get a working
  runtime. This is the intended fix, and it is a behaviour break.
  **IF YOUR TEST SUITE JUST WENT RED, START HERE.** The pattern that trips is
  a suite that constructs MORE THAN ONE RUNTIME IN A SINGLE PROCESS UNDER THE
  SAME PARTICIPANT NAME - typically a shared `build_test_runtime()` helper
  called once per test. Before this release the second and later registrations
  silently displaced the first and handed back a working runtime, so every test
  after the first was running against a route it had taken from its
  predecessor. That was never isolation; it was a collision that happened to be
  survivable. This release refuses it, so those tests now fail at construction.
  **The message will not lead you here on its own.** It reads
  `the participant name already has a live route under a different public key
  ed25519:...`, which names the key and sends you to key management, rotation
  or trust config - none of which is the problem. The fix is to give each
  runtime its own participant name, or to retire the previous route
  (`CommsRuntime::retire_inproc_route`, or `publish_replacing` when you hold
  the predecessor handle deliberately).
  Production code is usually unaffected: one runtime per process has no second
  registration to collide with. Reported from the field by an adopter whose
  21 failing tests were all this, and whose production path was not.
- `meerkat_core::ops::ToolAccessPolicy` gains a `ReadOnly` variant. The enum is
  not `#[non_exhaustive]`, so exhaustive matches break.
  **READ THIS BEFORE ENABLING READ-ONLY ANYWHERE: it makes rollback one-way for
  the sessions it touches.** Stated as a precondition rather than a footnote,
  because the trigger is a one-line config change that reads as trivially
  reversible and is not.
  - **Trigger.** Setting `read_only: true` on a mob profile's tool config
    (`MobToolConfigInput.read_only`, `PortableToolConfig.read_only`) resolves
    that member's policy to `ReadOnly` at build time. It is a floor, not a
    default: a read-only profile REPLACES whatever the spawn site asked for
    rather than being overridden by it, so every member built from that profile
    acquires the marker. The RPC session-create and fork paths accept it
    directly as well.
  - **Sticky.** The policy is persisted RESOLVED into
    `SessionMetadata.tooling.tool_access_policy` when the session is created.
    Turning the profile flag back off changes what NEW sessions get; it does not
    rewrite metadata already written. The property is "ever carried", not
    "currently carries".
  - **Blast radius is per session, not per fleet.** A partial rollout leaves a
    mixed estate in which some sessions are pinned forward and others are not,
    and the pinned ones are exactly those a read-only profile touched.
  - **You can ask rather than infer.** The marker is a field on the persisted
    session metadata document, so a host can query its own session store for a
    non-null `tooling.tool_access_policy` to enumerate which sessions have
    walked through the door. Do that before planning a downgrade rather than
    reconstructing it from deployment history.
    Query the FIELD, never grep for the string `ReadOnly`. Validated against a
    4,170-session production estate: the field matched zero sessions and the
    bare string matched one, which was a ticket summary an agent had quoted
    inside a transcript. Agents discuss the concept in ordinary work, so a
    substring search returns false positives on real data at exactly the moment
    someone is deciding whether a rollback is still available.
  - **Capture the baseline BEFORE upgrading.** Run that query on the old binary
    first. The field is new in this release, so a pre-upgrade count is trivially
    zero - and that is the point: it establishes that any non-null appearing
    later is attributable to a decision you made rather than inherited state.
  - Failing to decode is the INTENDED outcome. A downgrade that silently read
    the marker as `Inherit` would drop the enforcement, which is worse than
    refusing to resume.
  The wire mirrors
  `meerkat_contracts::WireToolAccessPolicy` and `WireResolvedToolAccessPolicy`
  gain the same variant.
- `meerkat_mob::MobEventKind` gains `SupervisorEscalationFailed`. The enum is not
  `#[non_exhaustive]`, so any consumer that mirrors every event kind needs the
  arm.
- `meerkat_core::CompactionCommitCoordinationError::Rejected` is renamed to
  `Refused`.
- `meerkat_core::agent::compact::build_compaction_context` takes a sixth
  parameter (the composed-request budget the escalation side now measures).
- Externally-constructible structs gain fields, so struct-literal construction
  breaks: `CompactionContext.request_context_budget`;
  `FlowStepSpec.failure_policy` and `FrameStepSpec.failure_policy`;
  `FlowFrameSeedAuthorityRecord.node_failure_policy`; the wire inputs
  `MobFlowStepInput.failure_policy`, `MobFrameStepInput.failure_policy`,
  `MobRepeatUntilInput.failure_policy`, `MobToolConfigInput.read_only` and
  `PortableToolConfig.read_only`.
- Machine-authority vocabulary changes, which are public because the generated
  kernels are published. `MeerkatMachineInput` / `Input` gain
  `ResolveUnstageableQueuedInput`; `MeerkatMachineEffect` / `Effect` gain
  `SessionRegistrationRejected`; `InputAbandonReason` gains `NeverExecuted`;
  `MeerkatMachineInput::RegisterSession` gains a `runtime_epoch_id` field and
  `MobMachineInput::CreateFrameSeed` gains `node_failure_policy`;
  `TransitionId::ResolveMemberRevivalSucceededRunning` is replaced by
  `...RunningLocal` and `...RunningPlaced`, and new `TransitionId` variants land
  for the epoch-conflict and unstageable-input transitions. Because
  `EffectKind`, `InputKind` and `TransitionId` are dense enums, adding variants
  also SHIFTS the discriminants of later ones - anything persisting a
  discriminant numerically rather than by name must re-derive it.
- `meerkat_sqlite::BridgeEligibility::Recoverable` is renamed
  `CatalogAuthenticated` and `is_recoverable()` is renamed
  `catalog_authenticates()`. The old names claimed an outcome the check never
  established: it reads catalog shape and never looks at a record.
- `meerkat_sqlite::MaintenancePrepareReport` and `MaintenanceBridgeReport` gain
  a `refused: Vec<MaintenanceRecordRefusal>` field and therefore lose `Copy`.
  Struct-literal constructions break; `MaintenancePrepareReport::rewrote(n)`
  builds the "nothing refused" form. `meerkat::PreV0810DomainBridgeReport`
  likewise gains `refused_records`.
- `meerkat_sqlite::SchemaDomain` gains a required
  `bridge_recoverable_versions: &'static [i64]` field. Every domain literal
  must add it. It declares the exact source versions the explicit offline
  bridge may infer for an unledgered file of that domain, and an empty list
  declares that no offline bridge recovers the domain at all. It exists
  because the same question was previously answered twice - once by the bridge
  and once by the message an operator reads - and the two disagreed.
- Seven more externally-constructible structs gain fields, so any out-of-tree
  struct literal must supply them. All are additive at the wire level; the
  break is to Rust construction, and none of them appear in the
  `cargo-semver-checks` summary in a form that names the owning type:
  - `meerkat_mob::profile::ToolConfig` and
    `meerkat_contracts::wire::WireMobToolConfig` gain `read_only: bool`. These
    are the two the read-only entry below did NOT name; it named
    `MobToolConfigInput` and `PortableToolConfig` only.
  - `meerkat_mob::definition::FrameStepSpec`, `RepeatUntilSpec` and
    `FlowStepSpec` gain `failure_policy: FlowNodeFailurePolicy`. Three structs,
    not one.
  - `meerkat_contracts::wire::WireMobRunResultEnvelope` gains
    `accounting: Option<WireMobRunAccounting>`.
  - `meerkat::PreV0810RealmBridgeReport` gains
    `failures: Vec<PreV0810DomainBridgeFailure>`.
- BEHAVIOUR-ONLY, NO TOOL WILL CATCH THIS, AND IT IS THE LIBRARY TWIN OF THE
  CLI CHANGE BELOW: `meerkat::bridge_pre_0_8_10_realm_storage_in` still returns
  `Ok(PreV0810RealmBridgeReport)` with an unchanged signature, but `Ok` NO
  LONGER MEANS EVERY DOMAIN BRIDGED. The call attempts every domain and reports
  per-domain refusals in the new `failures` field instead of returning `Err` on
  the first one. A library caller that treated `Ok` as success now treats a
  partial bridge as a complete one. Check `failures.is_empty()`, or the
  `all_landed()` helper, rather than matching on `Ok`.

### Known issues

- An input REFUSED STAGING until it exhausts its attempt budget is durably
  terminalized as `Abandoned { MaxAttemptsExhausted }`, and that terminal is
  never delivered anywhere in typed form. The waiter receives a mechanical
  "authority unavailable" string naming neither abandonment nor attempts, and
  the mob's directed-turn watcher correctly treats that as plumbing noise and
  records nothing. The refusal terminalizes the WHOLE batch, so innocent
  sibling inputs are abandoned alongside the culpable one.
  Three costs, none of them a hang. A directed flow step fails after its
  30-second step timeout with `FlowTurnTimedOut` instead of failing at once
  with an abandonment reason, so the failure is misattributed rather than
  stalled. The mob's tracked-turn journal keeps a Pending row that **a restart
  does NOT clear** (recovery requires a `Consumed` terminal and this row is
  `Abandoned`). And one `runtime_input_states` row per affected input retains
  its full ingress payload permanently: never archived, never retired, and
  invisible to the sweep that would otherwise find it. The storage growth is
  the only cost that compounds.
  Reachability is believed very low. Provider flakiness, tool errors and model
  failures do NOT consume the staging budget; only a staging refusal does, and
  the commit that introduced this path also removed the only refusal cause ever
  observed in the field. Every remaining refusal predicate was checked against
  the shipped tree without finding a reachable one, which is exhaustion of a
  predicate set rather than a proof. If a refusal does occur it reaches the cap
  within a single wake, since the laps carry no backoff.
  Operators can measure it directly. `"generated staging authority refused an
  accepted input batch"` is the denominator, `"queued input refused by
  generated staging authority; deferred behind the backlog"` is a survivable
  deferral, and `"queued input abandoned after generated max stage attempts of
  refused staging"` means this defect fired.
  A fix was written for this release and withdrawn twice. The diagnosis was
  right; both repairs were not. The second could convert a bounded
  misattribution into a stopped runtime loop taking out every session on that
  runtime, on a path no test exercised. A wider blast radius than the defect is
  not an improvement. The full fix also needs a migration for rows already
  committed, which are invisible to every sweep that could reach them, so it
  waits for 0.8.24 rather than shipping on a deadline.

### Fixed

- **A compaction whose projection handoff was refused could not be persisted,
  because the runtime epoch a session registered under was being overwritten at
  placement time.** This is the lane the release was opened for, and it had no
  entry here until a downstream reviewer pointed out that the headline fix was
  the only one with no narrative.
  The entry epoch is now a REGISTRATION-TIME FACT: it is installed when the
  session registers and is never written by a placement arm, so a placement can
  only assert the epoch it was registered with. The five placement guards moved
  from "absent or equal" to exact equality against the registration. That
  matters more than it sounds: under the old guard a caller passing `None`
  satisfied the check and silently wedged the session, and the seam did exactly
  that. Under exact equality the same caller gets an immediate typed rejection.
  A silent wedge became a loud refusal.
  A new machine-owned rejection effect carries that verdict to BOTH staging
  sites. Previously it was dropped by a non-exhaustive effect read at one of
  them, which is why the failure presented as a stuck session rather than an
  error: the verdict existed and had nowhere to go.
  **Wire-visible:** compaction failures of this shape now surface as
  `CompactionFailureReason::projection_handoff_refused` rather than as a
  generic failure, so a host can tell a refused handoff from an LLM or budget
  failure without parsing a message string.
- **`runtime/health` reported a healthy host on the strength of one job-backlog
  reading; it now measures three dimensions and states the one it does not.**
  The handler computed exactly one check, job delivery backlog, and reported
  top-level `status: ok` whenever it passed. A session's runtime loop, its
  durability state, and its progress were never looked at, so a wedged session
  returned `ok` and every operator alert built on the endpoint was watching a
  dimension it did not care about. `GET /runtime/health` was worse: it served a
  hardcoded `ok` with an empty `checks` map.
  Two things changed. First, the rule is enforced where the projection is
  built: a diagnostic may only assert the scope it measured, so a surface passes
  in what it measured and every declared dimension it did not measure is
  published as `unmeasured:<dimension>` at `degraded`. Second, `runtime/health`
  actually measures two more dimensions, read straight off live runtime state
  with non-blocking probes that cannot park behind the wedge they are reporting:
  - `session_durability` - `degraded` while any registered session's shared
    durability gate demands a cold reload before it may execute or mutate
    durable state. Storeless sessions carry no durability contract and are
    never counted.
  - `session_runtime_loop` - `degraded` while any registered session still
    claims a runtime loop whose task is gone or whose channels are closed. Until
    now a dead loop and an idle one were indistinguishable from outside.
  **What an operator does with this.** `status` is the worst rung over the
  MEASURED checks, so `status != "ok"` is a usable alert predicate: on a healthy
  host it is `ok`, and it moves to `degraded` when a real session goes
  reload-required or a real runtime loop dies. Unmeasured coverage is published
  in `checks` and deliberately does NOT set `status`, because a `status` that is
  permanently `degraded` on every healthy host is muted within a week and then
  the real incident arrives to a silenced alert. Read `checks` for the coverage
  claim; alert on `status`.
  **What is still not covered, stated plainly:** `session_liveness` remains
  `unmeasured:session_liveness`. Nothing here observes whether a live session is
  *progressing* - a session whose loop task is alive and whose channels are open,
  but which is parked while machine-owned lane truth still holds selectable
  queued work, moves no value this endpoint reads. That is the signature of the
  disarmed-member defect fixed elsewhere in this release, and detecting it needs
  a watchdog bridge that is 0.8.24 work. An alert on `status` will not fire on
  that class today.
  The `checks` map is an open string map on the wire, so the added keys are
  additive: no wire break, no schema change, no SDK regeneration.
  `GET /runtime/health` in `meerkat-rest` measures nothing at all and therefore
  reports `status: "degraded"` with four `unmeasured:*` entries, rather than the
  hardcoded `ok` it used to serve: a caller that measured nothing may not mint a
  clean bill of health.

- **The pre-0.8.10 bridge now opens a real 0.7.x realm.** It shipped able to
  authenticate a 0.7.x catalog and then refuse the rows inside it, so the
  remedy the product prints (`storage migrate --apply --bridge-pre-0-8-10`)
  half-bridged the realm and left it unopenable: session-store, schedule-store
  and workgraph committed, runtime-store refused. The row gate had been written
  from this repository's current source rather than from bytes any release
  wrote, and it disagreed with released bytes on nearly every constant - it
  required 16 fields where the 0.7.x writer emits 14 (two `Option::None` fields
  are skipped), demanded `RollbackStaged` where 0.7.x records
  `ResolveStagedRollback`, and demanded three attempts where a first-turn
  failure records one. It refused every realm it existed to rescue.
  The enumeration is deleted. A row is now admitted when it decodes through the
  current typed contract, binds to its own row key, agrees with the generated
  admission authority, and re-encodes without losing a field - a check derived
  from the row's own bytes rather than from a list. Fields the current types do
  not name are still refused, because serde would drop them silently.
- A record the bridge cannot carry no longer costs the operator the realm.
  Preparation refusals are per record: every other row is carried, the refused
  row's bytes are left exactly as found (never deleted, never blanked), and
  `storage migrate` names it, its reason, and the consequence. Only realm-level
  conditions - an unreadable ledger, a storage failure - still refuse a domain.
- **A successful bridge no longer deletes the operator's prompt.** On a realm
  written by published 0.7.28, the ingress payload `persisted_input.content`
  was present in `sessions.sqlite3` before the bridge and gone after it, while
  the bridge printed success and said nothing. The cause was the runtime-store
  v1 -> v2 released-row importer, which retires the ingress payload of a
  terminal input: correct for an upgrade, because the binary that wrote the row
  already retires payloads at the terminal transition, and wrong for a rescue,
  which is carrying rows across from before the durable-state floor. The
  released-row importers no longer run under the rescue at all, so the rescued
  realm keeps its payloads. Verified on the real CLI: `"hello"` reads back
  after the bridge, after `session list`, and after `session show`.
- The remedy sentence for an unledgered pre-floor domain no longer promises an
  outcome it did not check. It states that only the schema shape was verified
  and that the bridge reports any record it cannot carry.
- A record the preparation refuses is now genuinely untouched for the whole
  bridge, which is what the CLI already claimed. The released-row importers ran
  after the preparation and would re-decode a refused row, drop the field the
  current types cannot name, and rewrite it; or, for a row that did not decode
  at all, fail and refuse the entire domain after the operator had been told
  the schema was recognized and to run the bridge. Both are gone with the same
  change: under the rescue the preparation callback is the sole authority over
  row content.
- The eligibility answer an operator reads and the bridge that then runs are
  now one function. The message was derived only from the frozen predecessor
  verifiers while the bridge also accepts an exact code-derived migration
  prefix, so an ordinary open told operators a realm this binary bridges
  successfully was not recoverable - a false negative that replaced the earlier
  false promise. Both directions are pinned against realms written by published
  binaries, and every co-tenant domain of `sessions.sqlite3` is checked rather
  than one of the three.
- `SqliteStoreError::UnledgeredDomainObjects` carries the same remedy sentence
  the `meerkat-store` rendering does. After a partial bridge the raw sqlite
  Display reached operators with no next step at all.
- The per-record refusal note no longer ships two stray runs of spaces from a
  botched line continuation.
- `storage migrate --apply --bridge-pre-0-8-10` reports records left behind as
  notes and still exits 0 when every domain landed. The realm opens and the
  operator is told, per record, what stayed behind and that a session depending
  on it may not resume; a domain that actually refused is still an error.
- A failed turn no longer silently disarms a member. When a turn failed, its
  staged input was rolled back to queued and nothing re-armed the runtime loop.
  The input stayed immediately selectable and nobody took the next lap, so it
  sat until unrelated traffic happened to wake the loop, or forever. The loop
  can no longer park while machine-owned lane truth still holds selectable
  queued work, which makes returning work to a lane inseparable from coming back
  for it.
  Consequence to plan for, because it changes what a failing input costs: the
  shell was under-delivering the machine's declared `max_stage_attempts = 3`,
  granting one attempt per external wake with no wake owed, so a quiet member
  spent one attempt and went dark. A persistently failing turn now spends three
  applies BACK TO BACK WITHIN ONE WAKE and then terminalizes as
  `Abandoned { MaxAttemptsExhausted }`. That is roughly three times the model
  spend for a persistently failing input, in a burst, bought in exchange for a
  typed terminal in seconds instead of silence.
- An accepted input can no longer be starved indefinitely behind a backlog: a
  refused head is re-minted behind the queue with its attempt counted, and the
  attempt budget ends in a typed terminal rather than an unbounded retry. Two
  consequences an operator will meet: at the cap the WHOLE refused batch
  terminalizes, not only the culpable member, so innocent inputs can appear as
  `Abandoned { MaxAttemptsExhausted }`; and those terminals are observable today
  in durable input rows and through completion waiters, not on the event stream.
- A provider-authored cache breakpoint that cannot bind to the committed
  transcript head no longer kills the turn. The unbindable claim is discarded
  and reported; the turn proceeds. Previously this failed the whole turn with an
  internal error, which is how a live member could wedge on every attempt.
- Synthetic notice refresh no longer refuses beside an audited transcript
  prefix. The guard tested whether any audited row was touched by scanning the
  whole prefix; it now tests whether the lowest mutated index actually falls
  inside it.
- In-process comms registration is identity-keyed, so a second registration
  under a name another identity holds is a typed `NameOccupied` rejection
  instead of silently displacing the incumbent. A generation rebinding its own
  name is still allowed and now reports that it superseded a predecessor.
- A durable read-write store opened through the `Primary` profile, the profile
  every production constructor uses, can no longer end up non-WAL without
  failing closed. `ReadOnly` and `Maintenance` deliberately preserve the mode
  they find, including `Maintenance { write: true }`, which is the offline
  surgeon's profile and must not convert a database another step is about to
  relocate. `JournalPolicy` states that choice per profile, so a profile added
  later has to make it rather than inherit an answer from a match arm's shape.

### Changed

- `session/export_atif` now folds the durable log into the trajectory page by
  page instead of buffering every event, and its replay bound rises from
  100,000 to 500,000 events. Passing that bound is a typed `INVALID_PARAMS`
  rejection naming the bound and pointing at `events/list_since` or the
  file-writing `rkat session export-atif`, replacing the previous
  `INTERNAL_ERROR`.
- `session/export_atif` also bounds what it accumulates, not just what it reads:
  the fold measures its own retained document bytes per page and refuses with
  `BUDGET_EXHAUSTED` once they pass the outbound message limit, naming the limit
  and the bytes accumulated. Previously an oversized export built the whole
  document before outbound admission refused it; the outcome is unchanged and
  the transient cost is bounded. The event bound guards all-delta logs, the byte
  bound guards tool-heavy ones.
- `session/export_atif` stops reporting `SESSION_NOT_FOUND` for conditions that
  are not a missing session: a host without the durable event projection fails
  with `INVALID_REQUEST` (`event replay is not enabled for this runtime host`),
  and an existing session with an empty durable log exports an empty trajectory
  that names the session. A genuinely missing session still fails with
  `SESSION_NOT_FOUND`.
- Usage aggregation semantics are documented and pinned by test, and the Python
  and TypeScript SDKs now expose the per-call model and provider attribution
  that `turn_completed.usage.accounting` already carried. The scope matters for
  cost accounting: attribution covers run-closing calls, so intermediate
  tool-loop, extraction and compaction calls publish no row, and
  `run_completed.usage` stays cumulative and session-scoped and must NOT be
  summed against the per-turn rows.

### Added

- `meerkat_atif::TrajectoryBuilder`: incremental ATIF export that folds event
  pages as they are read. `trajectory_from_events` is now a thin wrapper over
  it and keeps its signature. `TrajectoryBuilder::retained_bytes` reports a
  strict lower bound on the serialized size of the document folded so far, so a
  paginating host can bound its accumulation without finishing the document.
- `meerkat_rpc::handlers::session::ATIF_EXPORT_MAX_EVENTS`: the export's event
  bound is public. The bounds-injecting entry point beside it
  (`handle_export_atif_bounded`, `AtifExportBounds`) is `#[doc(hidden)]`
  test-facing surface, not a supported way to serve `session/export_atif`;
  `handle_export_atif` is the wire path and supplies the host bounds.
- `meerkat_runtime::submit_bounded` and its outcome vocabulary
  (`BoundedSubmitOutcome`, `BoundedSubmission`, `BoundedSubmitReport`,
  `SubmitBound`, `DEFAULT_SUBMIT_BOUND`, `SubmitRefusal`, `SubmitTimeoutCause`,
  `SubmitTimeoutDisposition`, `SubmitUnknownCause`, `AdmissionDurability`,
  `AdmittedWorkState`): a submit that cannot wait forever. The caller supplies
  the bound (there is no unbounded variant), and every path lands on a typed
  outcome: durable acceptance, a typed refusal, or a timeout that states
  whether the work is durably queued or its fate is unknown. Admission collapses
  on the idempotency key at the durable admission point, so a bounded submit
  retried after a timeout does not become two deliveries on a persistent
  runtime. On a store-less runtime the collapse holds only while the admission
  is retained in live machine state, and a timeout with no durable witness does
  not promise exactly-once across a process death - which is what the `Unknown`
  disposition is for. This is a Rust library API; no RPC or REST surface serves
  it yet.
- `AgentEvent::ProviderCacheBreakpointsDiscarded`: the session event stream now
  reports discarded provider cache breakpoints, distinguishing claims persisted
  by an earlier turn (which includes fork-inherited evidence) from claims
  authored this turn. Present in the wire catalog and all three SDK event
  inventories. Consumers match on `DiscardedCacheBreakpoint`,
  `CacheBreakpointDiscardOrigin` and `CacheBreakpointDiscardReason`; a retained
  count of 0 means caching was lost outright for that turn.
- `meerkat_sqlite::JournalPolicy` and `ConnectionProfile::journal_policy()`:
  whether an open establishes WAL or preserves the mode it finds is now stated
  per profile rather than implied by a match arm.
- Read-only tool enforcement is first-class: `ToolExecutionPolicy` gates at the
  call level while preserving the tool list, so a denied call is an ordinary
  `access_denied` tool error in the transcript rather than a vanished tool. It
  fails closed on tools whose mutation class is unknown. The boundary is
  documented explicitly: a read-only launch is only truthful when the host also
  disables provider-native tool capabilities.
  The seam a host must implement: `AgentToolDispatcher::tool_mutation_class`
  defaults to `ToolMutationClass::Unknown`, and unknown is denied under
  read-only intent, so a host with its own dispatcher that does not override it
  will see EVERY one of its tools refused on a read-only launch.
- Mob run accounting on the run result, including per-member usage. A member
  whose usage could not be read contributes `usage_unavailable` naming the
  reason, and the run carries `members_usage_unavailable`, so a total is a
  documented floor rather than a number quietly short by however many members
  failed to report. Nothing reported usage here before. When the projection
  itself fails, the CLI omits the whole accounting block with a warning on
  stderr, so an absent block is also a signal.
- `meerkat_comms::CommsRuntime::retire_inproc_route` and
  `PreparedCommsRuntime::publish_replacing`, for hosts that need to hand a name
  over deliberately now that displacement is refused.

## [0.8.22] - 2026-08-09

### Breaking

- Provider usage is split into typed per-turn and cumulative carriers.
  `LlmEvent::UsageUpdate`, `AgentEvent::TurnCompleted`, realtime completion,
  compaction, budgeting, and assistant-block append paths now consume
  `TurnUsage`; `AgentEvent::RunCompleted` consumes `CumulativeUsage`;
  `Usage` gains `provider_accounting`; and `Session::record_usage` is replaced
  by `record_turn_usage` plus `record_cumulative_usage`. Custom adapters must
  emit matching provider/model accounting or the turn fails closed.
- Turn-completion wire payloads now require normalized accounting through
  `WireTurnUsage`, including provider, model, convention, aggregation
  provenance, and presented-token total.
- Durable peer delivery removes drain/pop compatibility authority.
  `CommsRuntime` consumers must use `claim_classified_inbox_interaction` and
  exact volatile handoff methods. `Inbox::new` becomes
  `Inbox::new_transport_only`, `ClassifiedInboxInteraction` is removed, and
  public sender bypasses and drain/receive authority methods are removed or
  narrowed.
- `PeerDeliveryOutcome::HandedOff` becomes `VolatileHandedOff`,
  `DurablyResolved { outcome }` is added, every peer send receipt gains a
  delivery disposition, and durable rejection or ambiguity are typed send
  errors.
- The supervisor bridge advances to protocol V5 for direct-member retirement.
  `BindMember` installs and returns a host-minted exact incarnation fence, and
  `RetireMember` must present that fence. A delayed command for an older
  generation can no longer retire a same-endpoint successor. Persisted V4
  peer-only mobs resume without mutating their authority, but direct-member
  lifecycle operations fail closed with `SupervisorProtocolUpgradeRequired`
  until the operator calls `MobHandle::rotate_supervisor`; that generated,
  durable rotation crosses to a new V5 key and epoch before exact member-fence
  adoption.
- `CoreExecutorInterruptHandle::hard_cancel_current_run(reason) -> Result<()>`
  becomes `hard_cancel_run_if_current(&RunId, reason) -> Result<bool>`, fencing
  stale cancellation from successor runs.
- System messages and generic System rewrite values gain optional
  `prompt_version`. Keyed prompt versions may be minted only by explicit
  update, and materialization or compaction suppresses superseded keyed rows.
- `SessionForkResult` gains required `cache_inheritance`. Fork points that
  split tool-use/result groups now reject instead of truncating.
- `MobSessionService::prepare_session_for_resume` is removed. Persistent
  create and materialization paths consume `SessionResumePreparationReceipt`,
  and runtime executors bind an exact `LiveSessionActorWitness` or actor slot
  instead of deriving publication authority from `SessionId`.
- `PersistentRuntimeExecutor::{new,new_with_workgraph_service}` no longer
  imply terminal-publication authority. Callers that publish terminals must
  bind a service-minted actor witness or slot. Runtime archive publication
  uses owned exact capabilities and deadline-aware convergence.
- Custom HeadCanonical backends must implement store-wide
  `IncrementalSessionStore::activate_head_canonical_store`; runtime stores must
  implement `activate_head_canonical_runtime_authority`.
- Schedule due claims require the exact `&ScheduleExecutorLease`; a host
  without that lease is standby. `ScheduleTickReport` gains
  `executor_authorized`.
- WorkGraph enablement requires `WorkGraphNamespaceGrant`. Services reject
  cross-namespace and `all_namespaces` access, and session/factory build
  configuration gains exact grant fields.
- WorkGraph public construction gains failed and cancelled child-join policy,
  claim-expiry observation, durable event facts, complete goal item inputs,
  and `WorkGraphEventKind::ReadinessObserved`. Recovery listing now requires
  an exact namespace, and unsupported atomic operations fail closed.
- WorkGraph claims accept exactly one relative or absolute lease. Relative
  leases are capped, readiness includes deterministic blockers and child-join
  policy, and expiry is released only by an observing mutation.
- Known-degraded JSONL listing changes from warning-and-serve to typed
  `SessionStoreError::ProjectionReadRefused` until canonical verification or
  rebuild succeeds.
- Provider cache behavior changes: OpenAI uses explicit stable prefix evidence
  where supported; Anthropic caching is profile opt-in,
  `AnthropicCacheControlPolicy` adds `SystemAndConversation`, and
  `AnthropicProviderTag` gains `cache_ttl`.
- Public WorkGraph bypass dispatch `handle_workgraph_tools_call` becomes
  crate-private. Hosts must compose the declared capability dispatcher.
- `PeerIngressRuntimeSnapshot::submission_queue_len` is removed. Consumers use
  `queue.queue_depth()` plus claim and handoff counters.
- `ProviderRequestPressure` gains lowered-request provenance,
  and the associated public enums gain variants that exhaustive consumers must
  handle.
- Helper execution is a full-operation exact-result redesign with no
  compatibility shim. Rust `MobHandle::{spawn_helper,fork_helper}` and
  `MobMcpState::{mob_spawn_helper,mob_fork_helper}` require `result_label` and
  `max_text_bytes` and return `BoundedHelperRunOutcome` instead of a helper
  snapshot. RPC, REST, CLI, WASM, Python, TypeScript, and Web requests require
  the same projection inputs. Their result carrier requires `output`,
  `tokens_used`, `agent_identity`, `member_ref`, `bounded_result`, `session_id`,
  `usage`, `turns`, and `tool_calls`, with optional `retirement_error`.

### Added

- Added explicit keyed system-prompt replacement for durable sessions with
  monotonic versions, optimistic checks, duplicate detection, REST
  `POST /sessions/{id}/system_prompt`, RPC `session/update_system_prompt`, and
  Python and TypeScript wrappers. Boot, resume, generic append, rewrite, and
  compaction cannot mint prompt versions.
- Added real `MobHandle::fork_member`: an idle source forks to a new durable
  session and member identity, validates blobs and complete tool groups before
  commit, and provisions through ordinary resume. Provisioning failure leaves
  the committed child recoverable.
- Added typed provider-cache inheritance to durable fork results. `Available`
  proves a byte-identical rendered prefix ending at a provider-authored
  breakpoint; every unproved case reports typed `Unavailable`.
- Added receiver-bounded helper projection with `{label,status,text}`,
  UTF-8-safe byte limits, and explicit truncated status and marker.
- Added opt-in cache authoring: OpenAI stable system/profile-prefix evidence
  and Anthropic five-minute or one-hour TTL plus system-and-conversation
  breakpoints, with no backend-global Anthropic economic default.
- Added a store-owned, fenced Schedule executor lease with acquire, renew,
  release, observation, firing-host status, and in-transaction claim
  authorization.
- Added typed WorkGraph `ItemReady`, `LeaseExpired`, and `NamespaceTerminal`
  facts committed in the same mutation that observes them, plus attention
  binding to an existing item.
- Added read-only peer ingress observability for queue depth, outstanding claim
  age, handoff and durable-admission counts, terminal outcomes, handover state,
  and delivery correlation. These projections cannot authorize mutation.
- Added ATIF trajectory export: the new `meerkat-atif` crate converts a
  session's committed event log into an ATIF-v1.7 trajectory document,
  exposed via `rkat session export-atif <session> [--output FILE]`, the
  JSON-RPC `session/export_atif` method (bounded durable-event pagination),
  and opt-in automatic trajectory persistence via `--export-atif` on
  `rkat run` (including `--resume`), which writes
  `trajectories/<session>.json` under the realm root after the turn
  terminalizes.

### Changed

- Provider adapters emit one normalized per-turn total for tokens presented to
  the model, with provider/model identity, convention, and aggregation
  provenance. Turn and cumulative usage remain distinct, and budgeting never
  interprets raw provider cache counters.
- Crash-replayable schedule dispatch, detached-job delivery, runtime terminal
  publication, and compaction projection persist typed effect intents with
  their owning state commit and realize them idempotently. Non-replayable hooks
  retain their narrower contracts.
- WorkGraph is the shared-work ledger, not a scheduler. It owns deterministic
  readiness, child-join policy, claims, namespace terminality, complete goal
  inputs, and attention binding. Schedule alone owns timers, recurrence,
  sweeps, latency bounds, and redrive; peer messages are wake acceleration,
  not durable work authority.

### Fixed

- Durable peer ingress retains the exact FIFO head until an opaque runtime
  terminal receipt commits it. Cancellation, actor replacement, restart, and
  redelivery release or deduplicate the stable envelope/claim instead of
  losing admitted input; volatile control traffic uses a separate handoff.
- Context and compaction decisions carry normalized provider evidence and
  lowered-request provenance. Forecasts remain observational, only exact
  provider evidence may refuse before dispatch, and provider context errors
  terminalize as typed `ContextExceeded`.
- Persistent Mob resume carries one owner-issued committed-boundary preparation
  receipt through one bracketed resume/materialization pipeline. The redundant
  second recovery route is removed and parkable failures remain typed.
- Externally injected HeadCanonical stores cross exactly once before
  session-service construction. The facade consumes store-issued activation
  authority and rejects missing, duplicate, or semantically drifting results.
- Known-degraded JSONL reads refuse with typed `ProjectionReadRefused` when
  canonical data could answer. A post-commit projection failure never rolls
  back a canonical save or delete.
- Exact actor-bound terminal publication now survives actor-only recovery,
  rejects stale predecessors and unrelated successors, runs custom publication
  outside machine and recovery locks, and retains durable retry authority
  across callback failure, panic, deadline, and restart.
- Runtime and Mob interruption are exact-run fenced, process-owned,
  panic-safe, deduplicated, and caller-bounded. A late custom interrupt cannot
  cancel a successor run.
- Mob retirement publishes durable `Retiring` before external cleanup, keeps
  one exact incarnation saga for retries, and returns typed retryable
  in-progress results without laundering them to invalid-parameter or generic
  transport errors.
- Concurrent detached-job delivery acknowledgements now reload and reapply
  generated-machine authority after exact CAS conflicts instead of leaking a
  stale revision from an otherwise idempotent acknowledgement.
- Named authority-erasing comms drain/receive APIs, redundant Mob resume
  preparation, and misleading peer-authority constructors are removed.

### Known limitations

- `VolatileHandedOff` proves exact FIFO handoff to a volatile consumer, not
  durable runtime admission or completed work. Consequential work still
  belongs in WorkGraph or application state.
- Exact pre-dispatch context refusal requires adapter-supplied exact token
  evidence. Bundled adapters expose lowered-body provenance and retain the
  provider's typed context rejection when exact synchronous counting is
  unavailable.
- `MobHandle::fork_member` currently resolves the child profile, provider, and
  model after durable commit, so it conservatively reports cache inheritance
  unavailable instead of installing unproved evidence.
- Helper and fork-helper operations wait for the exact admitted turn and return
  its receiver-bounded certified result. Bare `fork_member` remains
  provisioning-only; callers that want fork plus execution use the explicit
  fork-and-turn operation.
- Meerkat contains the Schedule lease contract, but removing MobKit's
  process-local firing-host gate remains paired downstream release work.
- The compatible MobKit release is not yet named. Do not retain the previous
  Meerkat 0.8.21/MobKit 0.8.15 pairing for this release.

## [0.8.21] - 2026-08-08

### Breaking

- `IncrementalSessionStore` implementations must implement
  `cross_head_canonical_authority` before HeadCanonical v1 data is used through
  the v2 write path. `RuntimeSessionAuthorityOps` implementations must provide
  one atomic `load_session_resume_observation` over session authority, catalog
  state, and the machine-lifecycle row.
- Schedule stores return trusted-backend transition commits from definition
  and occurrence mutations. Raw lifecycle-mutator destructors and
  `ScheduleStore::append_receipt` were removed; claim, renewal, mutation, and
  terminalization callers must consume the returned effects.
- The body-only Mob resume compatibility projection was removed. Resume
  consumers must retain and revalidate the owner-issued
  `SessionResumeVerdict` authority.

### Fixed

- Mob restore, revival, host materialization, and reconciliation consume one
  authority-bracketed resume verdict, preventing the body, catalog row,
  lifecycle row, or runtime generation from being reclassified independently.
- Schedule lifecycle effects remain attached to the durable store commit and
  are validated by services and drivers. Missing lease, dispatch, terminal,
  supersession, and revision effects now fail closed.
- SQLite session stores cross HeadCanonical v1 authority to v2 transactionally
  with source CAS, replay verification, rollback, and schema-v4 migration.
- Context-budget forecasts now carry provenance and derive their ceiling only
  from `ModelProfileWitness`; provider context rejection still terminalizes as
  typed `ContextExceeded`.
- A JSONL index failure after a canonical save or delete no longer turns a
  committed durable write into an apparent failure. The projection is marked
  degraded and reported by storage diagnostics.

## [0.8.20] - 2026-08-07

### Breaking

- Removed `EphemeralRuntimeDriver::queue` and
  `EphemeralRuntimeDriver::steer_queue`. Runtime lane membership and ordering
  are exposed only through the machine-owned `queue_lane` and `steer_lane`
  views.

### Fixed

- Runtime input execution no longer keeps mutable physical queue copies beside
  generated `input_lane` authority. Authorized batches validate the exact
  machine-owned prefix, hydrate payloads from the ledger, and remove work only
  through the staging transition.
- Mob-member rematerialization no longer replays profile, persisted-spawn, or
  resume-time system-prompt configuration as a new System message. Callers that
  intend ordered System input must use the typed System-context admission API.

## [0.8.18] - 2026-08-06

### Fixed

- Runtime steer admission preserves the submitted content, and Mob steer input
  arriving during member kickoff is queued behind kickoff instead of racing or
  being dropped.

## [0.8.17] - 2026-08-06

### Fixed

- Runtime comms drains admit one classified input at a time, preserving the
  durable FIFO tail when a drain task is cancelled or replaced.
- Automatic Mob-member rematerialization no longer replays persisted system
  prompt configuration as a new System message. Explicit new System input
  remains ordered through the normal admission path.
- Audited transcript hydration accepts a semantically identical live prefix
  after adapter rematerialization changes only physical row bookkeeping. The
  content-addressed append path reanchors current row authority while still
  rejecting divergent semantic prefixes.

## [0.8.16] - 2026-08-05

### Added

- Added the durable WorkGraph-to-Mob-Flow execution bridge. A
  `WorkExecutionLifecycleMachine` owns launch uncertainty, run observation,
  terminal evidence projection, retry eligibility, and closure handoff without
  taking WorkGraph item truth or Mob run truth away from their owners.
- Added MCP launch, reconciliation, uncertain-launch abandonment, and redacted
  binding-read tools for WorkGraph Flow execution. Bindings commit the exact
  Flow configuration and deterministic run identity before launch so recovery
  cannot blindly create a replacement run.

## [0.8.15] - 2026-08-03

### Added

- Mob work submission can now carry a store-owned `MobDeliveryIdentity`
  through `MobHandle::submit_work_with_mode_and_delivery_identity`. The
  runtime derives a stable work reference and uses the exact identity for
  durable turn admission in both turn-driven and autonomous-host modes, so a
  scheduler can redeliver the same occurrence after a crash without executing
  it twice.

### Changed

- `ToolNameSet` now serializes in canonical sorted order. Equivalent tool
  policies no longer produce different JSON, hashes, or cache identities due
  only to hash-set insertion order.

### Fixed

- Built-in context compaction now bounds the summarization request and the
  exact recent-turn tail it retains. A single oversized tool result can no
  longer make the compactor resend or preserve the same over-context payload;
  live token or provider-byte pressure bypasses the ordinary cadence guard,
  and typed provider-capacity failure falls back to a deterministic
  progress-making rewrite.
- Pre-0.8.10 durable SQLite realms now have one explicit current-binary bridge:
  `rkat ... storage migrate --apply --bridge-pre-0-8-10`. The frozen,
  fail-closed importer authenticates supported pre-floor schemas and rows
  before installing current authority. JSONL and memory realms are rejected
  without database mutation. Ordinary store opens remain strict.
  The maintenance transaction preserves queued and nonterminal inputs without
  scheduling or replaying them; a later session activation follows normal
  recovery.
- Fresh prompt-first runs and `rkat help` no longer fail only because their
  default workspace-derived realm cannot open. No historical session from the
  failed realm is loaded into the fresh run, and the explicit compatibility
  bridge is not invoked automatically. The run receives a newly generated
  durable SQLite realm under the same state root while workspace config, auth,
  provider, and tool policy remain in force. Explicit realms, isolated runs,
  resumes, and session commands stay fail-closed, and historical access remains
  available only through the explicit pre-0.8.10 maintenance bridge.
- Imported archived session documents with no competing runtime record are
  now revivable. When a runtime record exists, its lifecycle remains the
  authoritative classification and overrides stale document metadata.
- OpenAI responses no longer claim that web search ran merely because the
  response contains an empty or unrelated annotation list. Only a recognized,
  non-empty URL citation produces web-search provenance.
- `meerkat-mob` and the RPC, REST, and MCP server crates compile in their
  supported minimal feature configurations. Runtime-adapter and comms-only
  calls are no longer left reachable when those features are disabled.
- The schedule occurrence lifecycle schema remains frozen at wire version 9;
  durable delivery identity does not alter the persisted occurrence format.

## [0.8.14] - 2026-08-03

### Fixed

- Mob retirement now quiesces an active turn before detaching its actor, so a
  successful retirement cannot race an in-flight executor.
- A failed host-owned retirement keeps its retryable actor projection instead
  of deleting the actor while the durable lifecycle still requires the host
  to retry cleanup.

## [0.8.13] - 2026-08-03

### Fixed

- Runtime successor wakes are retained across the boundary claim that creates
  their work, while redundant wakes are suppressed and only genuinely queued
  successor work can schedule another turn.

## [0.8.12] - 2026-08-02

### Fixed

- Concurrent peer fan-in to a member that is already turning no longer loses
  accepted messages when the exact live-boundary witness becomes stale or its
  run advances during preparation. Those conditions now discard only the
  transient delivery attempt and atomically normalize the durable input to the
  ordinary queued path. Genuine boundary mechanism faults still fail closed.
- Recoverable runtime-turn failures no longer delete the live actor projection
  while leaving its durable machine state `Attached`. The error still surfaces,
  but the actor and its event stream remain available for queued work.
- Exact live-boundary steer context no longer re-proves its authority through a
  singular transcript identity and conversational User row. Heterogeneous and
  peer-only fan-in now projects the already-authorized context at the model
  request tail without mutating durable session history.
- Terminal run receipts now take the machine-authorized dense successor after
  any live-boundary checkpoints. A run that accepts transient context no longer
  reuses its last checkpoint sequence, repair-blocks on the receipt collision,
  and loses the projected actor stream.

## [0.8.11] - 2026-07-31

### Breaking

- The durable-state compatibility floor is now Meerkat 0.8.10. Current
  Session envelope v3 is domain state only; embedded checkpoint stamps,
  witness formats, verification seals, representation axes, and rebind
  operations are removed from the live persistence contract. Store-issued
  revisions and physical tokens are the only write, resume, and recovery
  authority. The exact 0.8.10 envelope is accepted by a one-time activation
  importer only with same-transaction proof of the released store schema and
  exact source row/blob identity. Old embedded stamps and witnesses are
  untrusted migration input: they are stripped rather than treated as
  self-authentication. State older than 0.8.10 must be migrated with an older
  binary before upgrading.
- `Session::set_system_prompt*`, `SystemPromptMutationKind`,
  `SessionSystemPromptSource`, and the generated system-prompt mutation
  authorization surface are removed. A system instruction is an ordinary
  ordered `Message::System`; callers that construct sessions directly append
  one with `Session::append_system_message`. The `system_prompt` turn
  convenience appends one at that admitted boundary. Materialization and
  resume inject nothing, regardless of host-config drift, and no request-time
  overlay substitutes or rewrites an earlier message.
- The unused generated embedded-checkpoint read-source/conflict surface is
  removed: `CheckpointProvenanceClass`,
  `RuntimeSnapshotReadDisposition`, `RuntimeProjectionConflictDisposition`,
  `DurableTailExecutionEvidence`, and their `SessionDocumentMachine`
  inputs/effects no longer exist. Current recovery consumes store-issued
  committed or provisional-tail authority and retains only
  `ClassifyDurableTail`.
- `PreparedRuntimeSessionCommitOutcome` adds
  `AlreadyAppliedReleasedEquivalent`. It reports the one-time case where an
  exact 0.8.10 receipt and every surviving committed effect match, but the
  released schema did not retain enough prior-CAS information to claim an
  exact request retry. The migration-bound receipt marker is consumed while
  installing the first current request witness; subsequent retries report
  `AlreadyAppliedExact`.
- `AnthropicCacheControlPolicy` and
  `WireAnthropicCacheControlPolicy` add the `Automatic` variant.
- `OpenAiProviderTag` adds the `prompt_cache_enabled` field.
- `WireProviderTag::OpenAi` adds the `prompt_cache_enabled` field.
- `SessionError` adds the `DurableTailRecoveryRefused` variant (code
  `SESSION_DURABLE_TAIL_RECOVERY_REFUSED`), `DurableResumeHold` adds
  `RecoveryRefused` (wire token `recovery_refused`), and
  `DurableTailRecoveryError` adds `InvalidEvidence`.
  `DurableTailRecoveryOutcome` adds `AlreadyAligned { recovered }`, while
  `Committed` gains the exact recovered `Session`. A machine-REFUSED
  durable-tail recovery
  (conflicting persisted runtime facts: another live runtime, or boundary
  receipts that already cover or contradict the tail) no longer surfaces as
  `DurableTailHeldForRecovery`; operators now get the refusal's own cause
  and remediation (retry after the conflicting runtime quiesces) instead of
  the hold's "await reconciliation".
- `DurableTailRecoveryRequest` and
  `authorize_and_commit_durable_tail_recovery` are removed. Call
  `meerkat_runtime::recovery::recover_durable_tail(&dyn RuntimeStore,
  &SessionId)` instead. Recovery loads and seals the exact store-owned source,
  classification, receipt facts, candidate identity, and CAS tokens
  internally; callers no longer provide recovered bytes or recovery facts.
- `meerkat_core::BlobStoreError` adds the exhaustive
  `WriteLimitExceeded { max_blob_bytes, actual_encoded_bytes }` variant —
  the typed store-side refusal of an oversized write. Downstream exhaustive
  matches must add an arm.
- `meerkat::surface::schedule_attempt_idempotency_key` is renamed to
  `schedule_delivery_idempotency_key` and the key it mints drops the
  `:attempt:{n}` segment (occurrence-level delivery identity; see the P0
  lease fix under **Fixed**). Hosts that persisted or matched on the old
  attempt-bearing key shape must treat the occurrence-level key as the
  delivery identity.
- `meerkat_schedule::OccurrenceLifecycleInput` adds the `RenewLease` variant
  and `OccurrenceLifecycleEffect` adds `LeaseRenewed`. Downstream exhaustive
  matches must add arms.
- `meerkat_schedule::ScheduleStore` adds `next_action_time_utc` and
  `renew_occurrence_lease_if_current`. External stores must provide
  store-clock next-action projection and atomically screen the exact
  `{ occurrence, attempt, claim_token, owner }` renewal witness.
- `ModelCapabilities` adds
  `supports_mid_conversation_system_messages`. External model catalogs must
  declare whether each model admits ordered System messages after the leading
  prefix; unknown and older models should set it to `false`.
- Mob work delivery adds the per-turn System carrier:
  `WorkSpec::system_prompt`, `BridgeDeliveryPayload::system_prompt`, and
  `PeerInput::system_prompts`. `MobBoundMemberRuntimeBridge::deliver_member_input`
  adds the `system_prompt: Option<String>` parameter. External struct literals
  and bridge implementations must initialize and forward these fields.
- `StickyModelFallbackCommitOperation::wait` now returns
  `Result<Option<SessionControlCommitReceipt>,
  StickyModelFallbackCommitError>` instead of
  `Result<(), StickyModelFallbackCommitError>`, exposing the exact durable
  control receipt when one was committed. Callers that need only completion
  may explicitly discard the optional receipt.
- `CoreExecutor` adds
  `acknowledge_committed_session_boundary(&CommittedSessionBoundaryAuthority)`.
  Store-backed executors must override the rejecting default and consume the
  exact typed authority after the runtime boundary commits.
  `checkpoint_committed_session_snapshot` now receives `Arc<Vec<u8>>`;
  implementations should retain or borrow the shared serialized artifact
  instead of copying the accumulated document.
- `RuntimeStore` adds the required
  `session_authority_ops(&self) -> &dyn RuntimeSessionAuthorityOps` accessor.
  This doc-hidden, implementor-only carrier owns the complete store-issued
  session-authority capability block. All 19 operations are required, so real
  backends must implement profile-specific support or an explicit typed
  refusal instead of inheriting a silent `Unsupported` default. Operational
  callers continue to use `RuntimeStore`, preserving transparent and
  fault-injection decorator overrides; decorators forward the carrier accessor
  and override only the individual `RuntimeStore` operations they intentionally
  perturb.
- `MobSessionService` adds the required
  `acknowledge_committed_runtime_session_boundary_under_turn_finalization_boundary`
  method. Implementations and wrappers must forward the exhaustive
  `CommittedSessionBoundaryAuthority` carrier. Its committed-session
  checkpoint hooks now receive `Arc<Vec<u8>>` for the same shared-artifact
  contract.
- `MobSessionService` adds the required
  `prepare_session_for_resume(&SessionId) -> Result<(), SessionError>` method.
  Persistent implementations converge any store-owned durable tail before an
  operational resume materializes the committed Session body; transparent
  wrappers must forward it. The composed `materialize_session_for_resume`
  helper performs preparation followed by the existing typed resume load,
  while observation-only callers continue using `load_session_for_resume`.
- Archived-resume authority is renamed to match its store-owned semantics:
  `PromotedArchivedResumeCommitLease` becomes
  `AuthorizedArchivedResumeCommitLease`,
  `PreparedArchivedResumeCommitLease::confirm_document_promoted` becomes
  `confirm_runtime_store_authority`, and
  `MobSessionService::promote_revivable_retired_session` becomes
  `authorize_revivable_retired_session`. Revival no longer mutates or
  persists an Archived marker in the Session document.
- `OccurrenceLifecycleInput` and `OccurrenceLifecycleEffect` add
  `DispatchAccepted`; exhaustive matches must add arms. The input carries
  `DeliveryAdmissionOutcome` plus `at_utc`.
- `PlanningTurnRequest` adds the public `planning_turn: u64` field.
- Every `AdaptiveDriverRuntime` operation after `now_ms` now receives an
  `&AdaptiveOperationDeadline`. The trait also adds the required
  `cancel_planning_turn` and `cancel_layer_flow` operations. External
  runtimes must retain exact planning/child-flow custody until terminality or
  cancellation is acknowledged.

### Billing-affecting default change

- Anthropic API, Vertex, and Foundry text requests now default to Anthropic's
  five-minute automatic prompt cache, which advances a breakpoint across the
  growing conversation. Explicit `cache_control` request policy still wins:
  `disabled` opts out and `system_prefix` preserves the narrower legacy
  behavior. Amazon Bedrock remains disabled by default because its Anthropic
  Messages backend does not support automatic caching, and explicitly
  requesting `automatic` there now fails locally. A qualifying five-minute
  cold cache write bills input tokens at 1.25x the base rate; cache hits bill
  cached input tokens at 0.1x. Automatic lookup scans backward at most 20
  cacheable blocks, so adding more than 20 blocks between requests can still
  miss. Write/read accounting remains visible through
  `Usage.cache_creation_tokens` / `cache_read_tokens`.
- Factory-built public OpenAI API GPT-5.6 sessions now send
  `prompt_cache_options: {"mode":"implicit","ttl":"30m"}` and a stable
  per-session `prompt_cache_key` derived from the durable `SessionId`. A
  caller-supplied key still wins, allowing hosts such as MobKit to use a
  longer-lived identity bucket. These defaults are re-derived below persisted
  provider parameters, so existing resumed sessions inherit them without a
  metadata rewrite. GPT-5.6 cache writes bill at 1.25x the uncached input-token
  rate; hits bill at the model's cached-input rate. `mode: "explicit"` authors
  append-monotone breakpoints at deterministic message boundaries for both
  Responses and Chat Completions. Set `prompt_cache_enabled: false` for a
  durable opt-out: Meerkat emits explicit-only mode without a breakpoint, so
  the request performs no cache reads or writes and incurs no cache-write
  charges. Enabling explicit mode on a large cold transcript may create up to
  four writes; normal growth adds one new boundary per model invocation.
  Automatic defaults are capability-gated off for the private ChatGPT backend
  and Azure OpenAI until those backend contracts explicitly admit them.
  OpenAI recommends keeping traffic near 15 requests per minute per cache key;
  higher-volume hosts should partition keys with a stable mapping. Agentic
  tool loops can issue several model requests inside one user turn, so a
  single tool-heavy turn can reach that guidance even without concurrent
  users; neither a per-session nor per-identity key removes that risk. Cache
  reads consider only the latest 50 breakpoints.
- These defaults make Meerkat's policy and routing affinity deterministic;
  they do not make a changing prefix cacheable. This patch does not prove
  every system-prompt writer stable. A timestamp, changing tool membership,
  or any other mutation before the breakpoint can still produce repeated
  billable writes with no reads.

### Fixed

- **Member-status observation cannot wedge the mob command lane.** A busy
  member's execution snapshot is bounded to 250 ms; timeout degrades the
  progress projection to typed `Unknown` instead of holding lifecycle
  commands such as objective completion behind an unbounded read.
- **P0: scheduled deliveries longer than the 60s lease were reclaimed
  mid-flight**, dispatching a duplicate turn every ~60s under
  `MisfirePolicy::CatchUpWithin` (a production fleet measured its largest
  delivery at 57s against the fixed 60s lease; a routine multi-minute session
  turn was guaranteed to trip it). Three coordinated changes:
  - **Lease renewal is machine authority.** `OccurrenceLifecycleMachine`
    gains a `RenewLease` input (guarded: only the current claim-token holder,
    only from `Dispatching`/`AwaitingCompletion`, only monotonic extensions)
    and a `LeaseRenewed` effect. The store samples its own time, screens
    occurrence/attempt/token/owner, and commits the extension within one
    lock or transaction. The completion waiter renews at ~lease/2 cadence;
    typed stale evidence stops renewal, while transient store faults retry
    with bounded backoff inside the last proven lease budget. A genuinely
    expired lease still means the deliverer is presumed dead.
  - **Delivery identity is occurrence-level.** The runtime-facing
    idempotency key drops the `:attempt:{n}` segment (now
    `schedule:{schedule}:occurrence:{occurrence}`), so a reclaim retry of a
    live or already-ran turn deduplicates at runtime admission and attaches
    to the existing input's completion instead of admitting a duplicate turn.
    Attempt counts and claim tokens remain store-side claim fencing only;
    stale-completion screening (`ClassifyStaleCompletionArrival`) is
    untouched.
  - **Durable lease state is the only reclaim authority.** The abandoned
    process-local live-waiter exemption is removed: no caller-supplied set can
    veto a machine-classified lease expiry, and separate hosts observe the
    same renewal/reclaim result from the store.
- **Durable session authority is store-owned and profile-specific.**
  WholeBlob commits are fenced by `{session_id, store_revision, blob_sha256}`;
  HeadCanonical commits are fenced by `{session_id, store_revision,
  boundary_head, committed_head_token}`, with the head binding exact row,
  rewrite, graph, component, and metadata prefixes. A deserialized Session
  can no longer authorize a write or recovery decision.
- **Intra-turn persistence promotes exact provisional receipts.** Each
  successful candidate write returns a `RunCheckpointReceipt` bound to one
  committed base and run, with a contiguous candidate sequence. The final
  boundary promotes the latest exact candidate. WholeBlob reuses the already
  encoded, hashed, and written candidate bytes; HeadCanonical promotes the
  exact applied physical revision/head token. Neither path reconstructs the
  accumulated document or reapplies the delta at final commit.
- **Session listing no longer creates a second document authority.**
  RuntimeStore maintains a small catalog projection in the same atomic
  boundary as the selected physical profile. It contains listing and
  lifecycle facts only, never a transcript, graph, component body, or
  serialized Session.
- **Opaque JSON now has one durable transcript identity.** Tool-use arguments
  and structured tool results serialize as recursively key-sorted JSON, and
  transcript digest format 3 binds that canonical value instead of producer
  spelling. Exact 0.8.10 activation accepts the released format-2 graph only
  under store-issued physical authority, validates the observable graph and
  row topology, remaps every revision and span identity together, drops only
  rewrite occurrences whose endpoints become identical after
  canonicalization, renumbers retained occurrences contiguously, and then
  runs the full current validator. Current-format reads remain strict; the
  one-time importer is the only relaxed boundary.
- **Recovered WorkGraph jobs honor durable cancellation before replay.** A
  cancelled monitor acknowledges immediately after process containment,
  without a redundant settlement-lease CAS or diagnostic drain in front of
  the terminal write. If the host dies in that window, recovery applies the
  persisted cancel request from `LossObserved` before considering replay or
  checkpoint resume, so a cancelled job cannot restart after reboot.
- **Detached shell cancellation now converges on durable job authority.**
  Ordinary and monitor-backed executions observe cancel requests written by
  another manager, contain the process before acknowledging cancellation, and
  settle from the exact store-returned terminal snapshot. Competing delivery
  acknowledgements, lease renewals, and terminal CAS writes are reloaded and
  converged with bounded retries instead of leaving a job indefinitely
  `Running` or publishing a process-local shadow terminal. A committed cancel
  also prevents generated `Complete` or `Fail` transitions from winning while
  the job is `Running` or `WaitingExternal`.
- **Retired session revival no longer rewrites a Session lifecycle marker.**
  The exact prepared runtime lease authorizes the store-owned
  `Retired -> Idle` transition while the Session body remains ordinary domain
  state. Catalog or physical boundary authority still protects a retained
  session, but a bare lifecycle registration left after body loss is treated
  as executor residue instead of resurrecting archive ownership.

### Added

- **Request-only host context for runtime turns.**
  REST, RPC, MCP, schedule, and mob runtime-backed turn carriers accept
  `transient_turn_context`, a sealed text-only value retained only with the
  pending runtime input for exact crash retry. Foreground provider requests
  project it once immediately before the exact admitted conversational user
  message across retries and tool-loop calls; it never becomes a System or
  Session message and is excluded from compaction and extraction. Deferred
  create and standalone browser/WASM live turns do not accept it.
- **Blob content addressing exported for external stores.**
  `content_blob_id` — the REQUIRED
  `sha256(canonical_media_type || 0x00 || base64_payload)` addressing every
  `BlobStore` implementation must mint byte-for-byte — is now re-exported
  from the `meerkat_core` crate root and the `meerkat` facade (alongside
  `BlobId`, `BlobRef`, `BlobPayload`, and `BlobStoreError`), with a published
  known-answer vector in its docs. First-consumer feedback: with only the
  trait re-exported, an external store re-derived the addressing by hand and
  silently diverged from core's read-back verification.
- **`BlobStore::max_blob_bytes` size contract.** New defaulted trait method
  (`None` = unbounded) advertising the store's per-write bound on the encoded
  (base64) payload. Backends with hard request/row limits can refuse an
  oversized `put_image`/`put_artifact` with the typed
  `BlobStoreError::WriteLimitExceeded` instead of an opaque backend
  `WriteFailed`; the pending-blob retry classifier treats that refusal as
  definitive (deterministic for a given payload) rather than retrying it
  forever.

### Changed

- `Message::System` is repeatable and legal anywhere in the ordered
  transcript. `StartTurnRequest.system_prompt` is accepted on every turn and
  appends exactly one System message at that turn boundary. It is not a
  create-time field, does not replace an earlier System message, and is not
  inferred from transcript text or position. `WorkSpec::system_prompt` carries
  the same semantics through turn-driven and placed mob-member delivery,
  including different System messages on successive turns. Controller-local
  autonomous inbox delivery rejects it before admission because that
  plain-event path has no authored turn boundary. Resume preserves the exact
  ordered prefix; compaction retains every System message in relative order;
  provider adapters preserve exact chronology where their wire supports it.
  A limited provider wire returns a typed projection error when it cannot
  represent the requested shape; it never makes the durable Session invalid,
  blocks System-message authorship, or rewrites existing history. Rebinding to
  an exact provider can execute that Session unchanged.
- Provider lowering never changes that durable meaning. Standard OpenAI
  Responses, OpenAI-compatible Chat Completions, and OpenAI Realtime preserve
  System interleaving. Cataloged Anthropic Fable 5, Opus 4.8, and Opus 5
  support mid-conversation System messages; Meerkat lowers a canonical
  turn-scoped `System -> User` boundary to Anthropic's legal
  `User -> System -> Assistant` placement. Other Anthropic models and Gemini
  accept only a leading System prefix; the private ChatGPT Responses backend
  accepts one leading System row. Other shapes receive a typed non-retryable
  provider projection error. Empty, whitespace-only, and duplicate entries
  are never trimmed, deduplicated, delimiter-joined, replaced, or dropped.
  Realtime's separate `runtime_system_context` carrier is retired; reconnect
  applies the same ordered replay-window policy to every role and replays
  retained System rows in place, while `SystemNotice` remains an explicit
  history event.
- Typed peer terminal-response facts now persist as deduplicated
  `SystemNotice` messages at the conversation tail without mutating any
  ordered `System` message. Active-turn delivery commits the notice to the
  canonical transcript, and exact 0.8.10 activation converts the released
  terminal projection without losing its applied/idempotency records.
- Evidence honesty across the perf/restart suites (no product behavior
  change):
  - `e2e_smoke_mob_turn_head_canonical_storage_cost_gate` now asserts the
    complete ordinary HeadCanonical cost contract: delta-bounded digest work,
    zero whole-session encodes and WholeBlob rows, store-issued HC authority,
    bounded suffix rows/bytes, and database-plus-WAL growth;
  - the cold-restart resume suites document the same-process caveat (teardown
    is graceful, not a kill, and the narrow legacy-heal probe memo survives),
    while the base contract is also proven across two OS processes: one
    writes and exits, then a fresh process reopens, reads, and continues the
    durable session with full validation;
  - the BuildBuddy e2e-smoke foundation selector now builds the previously
    omitted `smoke_mob_idle_burn`, `smoke_mob_turn_latency`, and
    `live_meerkat_regression` targets, records the foundation build's BEP
    stream, and the smoke materializer requires a successful terminal while
    binding every artifact to that invocation's canonical workspace, output
    base, and exact Bazel configuration path — a partial/failed build or a
    warm output from another invocation/configuration cannot authorize an
    artifact by target filename alone;
  - Turbo-S stabilization now applies at its real execution boundary: the
    Bazel `sh_test` cohort is forced to one active action with one Bazel retry.
    The ineffective Nextest-only heavy-runtime override is removed;
  - `meerkat-core/tests/fixtures/README.md` retires the committed 0.8.8
    vectors as release-compatibility evidence: they remain historical
    digest known-answer inputs only. The supported upgrade floor is 0.8.10,
    and a byte-manifested synthetic realm captured with the published
    `rkat` 0.8.10 artifact now proves the real 0.8.10→0.8.11 store migration
    and idempotent reopen path.

## [0.8.10] - 2026-07-28

### Breaking

- **Durable transcript-history retention is now anchor + splice deltas.** The
  durable encoding of `session_transcript_history_state_v1.revisions` changed:
  entry 0 is the chain anchor carrying full `messages`, and every later entry
  carries `rebase {base, at, removed, insert}` — a minimal splice with the
  shared prefix *and* suffix elided. Decode materializes front-to-back in one
  pass, and full-body entries are still accepted on decode. **Documents
  written by 0.8.10 cannot be read by earlier releases.** The in-memory
  `TranscriptHistoryState` / `TranscriptRevisionBody` types are unchanged, so
  no consumer code moves and no published wire schema changed
  (`TranscriptRevisionBody`'s own serialization is untouched; regenerating the
  schema artifacts produces a version-only diff).
  Motivation, measured on a production fleet: retention was `N × document`,
  because every rewrite retained the full transcript twice — 490.6 MB of
  durable documents carrying 6.55 MB of conversation (74.9×; worst identity
  118×) across 1687 retained revisions. Retention is now `one document + Σ
  edits`: ~82 MB of history metadata on the measured session becomes ~1.4 MB.
  Content addressing is unchanged — every retained body is still hashed after
  materialization, so a splice that reconstructs the wrong messages fails
  exactly the check it always did — and both transcript-history witness
  formats are byte-identical, so checkpoint digests and stamps are unaffected.
- **Session-store schema ledger v1 → v2** (`strand-supersession-links`). A
  0.8.9 binary opening a v2 file refuses it wholesale as
  `SchemaFromTheFuture`. Strand rows previously had no collection path at all,
  and a rewrite at message 0 — the per-resume prompt-refresh shape — shares no
  prefix, so each one wrote a complete new transcript of rows that nothing
  ever reclaimed. A session now keeps at most ONE materialized strand (the
  live head); superseded strands retain only their spliced span and resolve
  the rest through their successor, with supersession derived by comparing
  persisted bytes so it can never claim sharing that does not exist.
- **Public API additions that are breaking under `cargo-semver-checks`:**
  `SessionCheckpointProvenance::RecoveredLegacyBoundaryCommit` and
  `DurableTailRecoveryClass::LegacyCompletedCandidate` are new enum variants;
  `SessionDocumentInput::ResolveRuntimeSnapshotReadSource` and
  `::ClassifyDurableTail` gained a required field, and the corresponding
  generated `resolve_runtime_snapshot_read_source` / `classify_durable_tail`
  functions gained a parameter.

### Added

- **Storage doctor census (read-only).** Four findings so this class of defect
  cannot run unobserved again: `transcript-history-oversized` (warns past a
  4.0× document-to-conversation ratio — the production shape was 74.9×),
  `strand-duplication-reclaimable` (2.0×; the production shape was ~88×),
  `frozen-blob-archive-reclaimable`, and `storage-census-unmeasured` so data
  the census cannot read is reported unknown rather than healthy. A 1 MiB
  floor keeps small sessions quiet. The 490 MB above accumulated over weeks
  precisely because nothing measured the ratio.
- **Store conformance: `chained_prefix_rewrites`.** Pins reconstruction
  fidelity across rewrites that share no prefix with their parent. It
  deliberately does not encode any one storage strategy — a backend that keeps
  whole revisions materialized still passes.

### Fixed

- **Resume no longer re-reads the whole transcript-rewrite log.** Every
  authoritative load re-proved every retained rewrite record, hashing both
  full transcript bodies each record carries: measured 0.17s at 7 retained
  revisions rising to 11.9s at 79, with 1.39 GB hashed per resume and a
  log-log slope of 2.0 in revision count. Three changes remove it. The
  coverage check now reads each row with a commit-only partial decode instead
  of materializing two transcripts per record; records carry an additive
  `digest_format` marker so the legacy revision-string heal — a full-transcript
  hash per body, previously unconditional at the serde boundary — is skipped
  for records that cannot need it, mirroring the existing marker on
  `TranscriptHistoryState`; and a replay cursor stamped into the session's
  transcript graph lets a load start after the records it already folded.
  Measured after: a load reads **1 log row regardless of retained revision
  count** (1 of 3 at 2 rewrites, 1 of 9 at 8) and materializes **zero** record
  bodies.
  **Scope, because the stamp only reaches disk inside a graph write:** the flat
  cost applies to paths that write the graph — CLI resume, which mints a
  system-prompt-refresh rewrite every time — and *not* to read-only paths such
  as `rkat sessions show`, which continue to read the full log.
  Only sessions with an `EventStore` wired are affected at all, i.e. `rkat`
  CLI/RPC/REST with disk persistence; hosts that wire no event store never
  executed this path.
  Verification semantics change deliberately from verify-always to
  verify-on-use: a load proves the graph it serves (checkpoint stamp plus
  `validate_transcript_history_state`) and every record it actually folds, but
  no longer re-proves historical log bodies it will never materialize. A
  cursor that does not describe its graph, that claims a sequence past the
  log's high-water mark, or whose tail does not chain from the reconciled
  boundary is refused, logged, and degrades to a full read from sequence 1 —
  a wrong cursor costs a slow load, never a skipped record.

- **0.8.8 → 0.8.9 upgrade boundary: legacy durable-tail adoption.** A clean
  ≤0.8.8 shutdown routinely leaves a session whose durable head is one
  committed turn ahead of the runtime snapshot (`intra_turn_checkpoint`
  stamp over a `run_boundary_commit` base) with the turn's own input row
  still queued — ≤0.8.8 persisted staged run bindings only inside the
  boundary commit. 0.8.9 held such sessions forever
  (`DurableTailHeldForRecovery`). The recovery machine now has a
  machine-authorized legacy arm: a digest-proven completed tail whose head
  row carries pre-witness-v3 stamp evidence commits as a recovered boundary
  and RETAINS the unbound input row for ordinary redelivery — nothing is
  terminalized, no consumption is fabricated, no input can be dropped; the
  worst case is one duplicate redelivered turn, matching the legacy fleet's
  own restart semantics. Identity-less tails written by pre-v0.7.12
  binaries adopt through a distinct
  `SessionCheckpointProvenance::RecoveredLegacyBoundaryCommit` (note:
  sessions adopted under this provenance are unreadable by 0.8.9-or-older
  binaries — same one-way door class as the v2 recovered stamps). Modern
  (witness-v3-stamped) shapes keep the fail-closed hold byte-for-byte.
- **0.8.8 → 0.8.9 upgrade boundary: first-boundary-save refusal over
  inline-graph runtime rows.** The transcript-history erase guard derived
  its reference witness for a previous INLINE graph always in format 2, so
  a 0.8.9 slim boundary save (v3 witness carrier) could never match — even
  a same-graph round trip refused, and any post-resume save (whose graph
  legitimately evolved) wedged the session on its first post-upgrade turn.
  Affects any 0.8.8-written session with a transcript graph (compacted or
  prompt-rewritten), plain `rkat` included. The carve-out is now
  format-aware, and legitimate evolution is accepted through caller-proved,
  store-verified evidence: the evolved graph is sealed (every retained body
  digest-verified), bound to the incoming document's own witness, checked
  as a strict-prefix descendant of the previous inline graph, and the
  accepted write replaces the row with the slim representation — a one-time
  O(graph) migration per session, O(1) on every later turn. Evidence is
  threaded through every boundary-commit seam, including the queued-input
  machine boundary paths and `atomic_apply` (snapshot, receipt,
  machine-lifecycle record, and input terminalization stay one
  all-or-nothing transaction; the migration adds no crash window).

- **Recovered boundaries no longer strand proven-consumed inputs.** The
  durable-tail observation returned early on the first unbound input row,
  discarding the rows the same scan had already proved bound to the recovered
  run (durable staging bindings, or a committed boundary receipt naming
  them). Those rows stayed non-terminal, and the input lifecycle then rolled
  Staged back to Queued and re-admitted them — re-executing a turn the
  boundary had just committed, with a duplicate provider call and re-fired
  tool side effects. The scan now completes before deciding: proven-bound
  rows are terminalized, genuinely unbound rows are retained for ordinary
  redelivery. Pre-0.8.9 documents are unaffected by construction (their
  staging bindings never reached disk, so they attribute nothing).
- **The upgrade-boundary evidence source is wired on every surface.** Only
  mob hosts wired it; the facade's `PersistenceBundle` — the production
  machine for CLI, REST and RPC — constructed its `MeerkatMachine` without
  it, so machine-owned boundary commits on those surfaces fell back to the
  evidence-less default and refused the first slim save over a legacy inline
  row. Hosts whose session store is not incremental keep the previous
  fail-closed behavior.
- **A transient probe failure no longer disables the upgrade path.** The
  driver's one-shot inline-row hint cached `false` when the probe's snapshot
  load failed, permanently disabling evidence assembly for that driver's
  lifetime; the hint now stays unresolved so the next commit re-probes.

### Upgrade notes

- Identities with an in-flight input at shutdown replay it once after
  upgrade — the same behavior as a pre-upgrade restart. Expect one
  duplicate reply per such identity on upgrade day.
- **Migrating a session with a large retained transcript history is
  expensive and should be planned.** The head-canonical conversion
  re-materializes every retained revision as strand rows; a production
  session with 98 retained revisions over a 371-message transcript produced
  ~16.7k rows and took minutes. Sessions whose durable document is dominated
  by retained history should be pruned before conversion, not after.

## [0.8.9] - 2026-07-27

*(Historical cumulative detail: the release hook did not stamp
v0.7.29-v0.8.8, so this section preserves entries accumulated across
v0.7.29-v0.8.9. Release-local summaries for v0.7.29-v0.8.8 are backfilled
below; treat this section as cumulative context, not an exclusive v0.8.9
delta.)*

### Breaking

- **Checkpoint stamp schema v3 — the witness-v3 one-way door.** A session
  document that retains a transcript-history graph now mints its checkpoint
  stamp with `schema_version` 3
  (`SESSION_CHECKPOINT_STAMP_SCHEMA_VERSION_WITNESS_V3`), because its
  canonical digest folds the new FORMAT-3 transcript-history witness (a
  domain-separated digest over the head revision, the canonical commit log,
  and the sorted retained revision-identity set — O(retained revisions),
  never O(retained body bytes)). Pre-v3 binaries refuse such stamps through
  their existing typed future-schema path; upgrading is one-way PER SESSION
  on its first post-upgrade boundary save. Sessions without a transcript
  graph keep minting the lowest schema their provenance allows and stay
  readable by older binaries. v2 witness evidence (every existing durable
  row) keeps verifying under the v2 computation indefinitely — mixed
  v2/v3 stores, no migration, no flag day; slim head-canonical rows carry a
  typed `TranscriptHistoryWitness` (`witness_format`,
  `revision_digest_format`, `digest`; pre-v3 bare digest strings normalize
  to format 2 and are re-persisted bare) and can never relabel themselves
  v3. Unknown witness formats refuse typed at document ingress, before any
  normalization or healing. The graph's `digest_format` (revision-STRING
  format) is untouched at 2. `session_transcript_history_checkpoint_digest`
  keeps its signature; `session_transcript_history_witness` exposes the
  typed carrier; the accepted witness-format set lives in the generated
  `SessionPersistenceVersionAuthority`
  (`restore_transcript_history_witness_format`, accepted `[2, 3]`).

### Performance

- **Flat turn curve: turn-boundary digest work is size-independent.** An
  identical one-word turn used to hash the whole accumulated session
  document many times over (measured 435 MB hashed per turn at a ~10 MB
  fixture; 60 s at 14 MB and 180 s at 94 MB per turn on a live 0.8.6
  fleet). The four attributed O(document) drivers are removed without
  deleting a single check:
  - **History witness (was ~109 MB/turn):** format-3 witness derives from
    revision identities instead of re-absorbing every retained body's bytes
    on every derivation (see Breaking above); body-byte integrity is owned
    by seal-at-ingress, where it was already proven.
  - **Per-decode graph validation (was ~55 MB/turn):** every typed producer
    seam now admits the graph it just proved (or extended under its typed
    inductive proof) into the validated decode memo, so a warm process's
    next decode of those exact bytes substitutes the proven graph instead
    of re-hashing every retained body. First sight after a cold start still
    verifies fully; `MEERKAT_DISABLE_GRAPH_DECODE_MEMO` still restores the
    full per-decode cost end to end.
  - **Compaction-path revalidations (was ~59 MB/turn on compacting turns,
    a ~6x multiplier on every real compaction):** the compaction authority
    is minted from the session accumulator's O(delta) digest plus ONE hash
    of the rebuilt transcript, the no-op precheck answers from the
    authority's own digests, whole-span commit digests reuse the one
    required hash (partial spans keep paying O(span)), the rewrite commit
    proves its successor graph by construction (mirroring the append fast
    path; debug builds still run the full validator with digest accounting
    suppressed), and the store guards demand the sealed
    `ValidatedTranscriptHistory` instead of re-running the whole-graph
    validator once per retained commit — the adoption arm now also checks
    chain coherence, which the per-commit loop never did.
  - **Checkpoint canonical passes (was ~36 MB/turn):** the canonical
    checkpoint digest is served from a retained SHA-256 midstate over the
    document's transcript span (`created_at`/`id` sort before `messages`
    and are immutable, so the prefix is constant), spliced with the
    O(metadata) suffix — byte-identical digests, pinned against a committed
    v0.8.8 fixture, with a sampled full-path cross-check and fail-open
    fallback to the full canonical pass on any surprise.
  Flatness is asserted in-lane by the re-armed
  `e2e_smoke_mob_turn_latency_gate` (large/small hashed-bytes ratio, plus
  the small-side honesty band); whole-document boundary SERIALIZATION is
  still one pass per boundary by design (the per-fixture encode envelope
  keeps pinning the repeated-reserialize class) and is the next tracked
  axis, not part of this claim. Cold starts pay one full validation per
  session document, once.

- `meerkat_mob::MobSessionService` gains a REQUIRED `load_session_for_resume`
  returning the new `ResumeSessionLoad` enum (no default: a composition over
  the legacy optional reads can never produce `ArchivedNotRevivable`, so
  implementers must state their own truth); `MobError` gains
  `SessionUnavailableForResume` with a typed
  `SessionResumeUnavailableReason`, and `MobFailureClass` gains
  `TargetArchived`; `RuntimeProjectionRollbackDisposition` /
  `ResolveRuntimeProjectionRollback` are replaced by
  `RuntimeProjectionConflictDisposition` / `ResolveRuntimeProjectionConflict`
  (and `RebuildToAuthority` no longer exists), whose input gains
  `row_provenance`; `SessionCheckpointProvenance` gains
  `RecoveredRunBoundaryCommit` and `RecoveredInterruptedBoundary` under a new
  per-record stamp schema v2 (`SESSION_CHECKPOINT_STAMP_SCHEMA_VERSION_RECOVERED`
  — ordinary stamps still write v1, so only recovered records refuse on older
  binaries, and they refuse typed rather than as unknown-enum corruption);
  `CoreApplyOutput`'s public `session_snapshot`/`session` fields collapse
  into one private sealed `BoundSessionCommit` field (constructors
  `CoreApplyOutput::new` / `with_untyped_snapshot` replace struct literals;
  accessors `committed()` / `snapshot_bytes()` / `session()` /
  `into_parts()`), and `with_session` returns `Result` because it mints the
  sealed pair by serializing the session itself, so the certified typed
  session and the persisted bytes cannot diverge or be re-paired;
  `commit_runtime_loop_run` takes the sealed commit; `SessionError` gains
  two typed variants, `DurableTailHeldForRecovery` and
  `DurableEvidenceQuarantined` (stable codes
  `SESSION_DURABLE_TAIL_HELD_FOR_RECOVERY` /
  `SESSION_DURABLE_EVIDENCE_QUARANTINED`, typed `durable_resume_hold`
  structured payload), so exhaustive matches over `SessionError` need two
  new arms; `ResolveRuntimeSnapshotReadSource` input/effect shapes changed;
  `AuthorizeDurableTailRecovery` gains persisted-fact inputs
  (`observed_lifecycle`, `observed_current_run`, `last_committed_sequence`)
  plus the `prior_commit` / `input_evidence` evidence enums
  (`DurableRecoveryPriorCommit`, `DurableRecoveryInputEvidence`), and commit
  verdicts arrive as the `DurableTailRecoveryCommitAuthorized` effect
  carrying the machine-minted boundary sequence;
  `DurableTailRecoveryRequest`'s fields are private and its only
  constructor, `from_classification`, requires the classifier's own
  `DurableTailClassified` verdict effect; `RuntimeStore` gains
  `load_committed_boundary_receipts` and `load_input_states_with_versions`
  (both defaulted) and two fencing error variants
  (`InputRowVersionConflict` / `MachineLifecycleVersionConflict`) — stores
  applying fenced records must enforce `expected_row_digest` inside the
  writing transaction.

### Fixed

- **Head-canonical cold resume no longer serves a stale runtime snapshot in
  place of the committed durable head** (advisory
  `ADVISORY-0.8.6-head-canonical-resume.md`, form 1) and **archived sessions
  are no longer reported as missing** (form 2). Cold reads drive a typed,
  machine-owned read-source table; a durable tail whose boundary commit lost
  a shutdown race is RECOVERED through a machine-authorized pipeline
  (classification → authorization → one atomic `atomic_apply` of recovered
  snapshot + receipt + input terminalization) under new recovered-boundary
  provenances anchored to the last committed authority. The recovery rule:
  every verified durable descendant is preserved — committed as completed,
  closed as interrupted (typed recovery notice, input terminalized not
  requeued), or held intact. Nothing
  rolls back; `RebuildToAuthority` is deleted, with
  `ConvergeSupersededProjection` covering the abort-replay projection case
  its wedge invariant proves safe. Exactly-once holds across recovery: the
  commit terminalizes the consumed input so the delivery layer does not
  re-run a recovered turn.
- **Recovery hardening (external review of the recovery pipeline).** A
  dangling `tool_use` in a durable tail now HOLDS the tail for reconciliation
  instead of being auto-closed with synthetic results: the call proves
  intent, not execution, and its external side effect may already have fired.
  Recovery authorization judges the PERSISTED machine-lifecycle row and the
  durably committed receipts (typed machine inputs) instead of a freshly
  registered, vacuously quiescent authority; the machine MINTS the recovery
  boundary sequence one past the last committed receipt for the run (a
  hard-coded sequence 1 could collide with an interrupted tool loop's
  `BoundaryContinue` receipt) and records the run terminal, and the shell
  realizes it through the lifecycle-aware atomic seam fenced on the exact
  observed row version. The machine also owns the already-recovered refusal:
  a candidate the highest committed receipt already covers refuses instead
  of minting a phantom duplicate boundary (receipt-key uniqueness fences
  only same-sequence races, not a second recovery that observes the first
  one's receipt), and a prior commit the candidate neither extends nor
  equals refuses as divergent. Input terminalization now uses durable
  identity only (persisted run bindings, receipt `contributing_input_ids`)
  with fenced row-version CAS; text matching is gone, and an unbound
  non-terminal content input holds the recovery instead of guessing — both
  hold verdicts are machine-minted, the shell observes input evidence before
  the drive and never downgrades a commit authorization on its own. A held
  or quarantined session surfaces typed
  (`SESSION_DURABLE_TAIL_HELD_FOR_RECOVERY` /
  `SESSION_DURABLE_EVIDENCE_QUARANTINED` with the `durable_resume_hold`
  payload) instead of as internal-error prose. Run→input bindings are
  persisted at staging, BEFORE execution, so a crash mid-run leaves identity
  evidence to recover against. Recovery promotes the whole durable document
  (usage, timestamps, every non-transcript metadata key — including
  compaction projection intents) rather than messages alone. A committed
  strict descendant now wins over a stale live snapshot regardless of local
  actor liveness (an archived head whose runtime retirement failed was
  masked), and projection convergence requires intra-turn row provenance so
  two committed siblings can never overwrite each other. Read-triggered
  recovery takes an exclusive per-session fence, re-observes the head under
  it, and converges idempotently when a competing process wins the commit.
  The advisory's own "Am I affected?" command no longer uses
  `sqlite3 ?immutable=1`, which ignores the WAL and can report a false
  "unaffected".
- **P0: process-global verification memos no longer bless bytes nobody
  validated.** The transcript-graph decode memo stores the proven graph
  object (a hit substitutes proven content) with `digest_format` and
  per-body `created_at` pinned into its key; the global stamp-verification
  cache is replaced by a per-`Session` seal cleared by every content
  mutation, including the three that do not bump `updated_at`.
- Slim head-canonical projections whose carried transcript-history witness
  exactly matches the previous graph's derived witness no longer read as
  "would erase retained transcript history state" (both erase sites).
- Conversation digests unified on the canonical `sha256:<hex>` accumulator
  format across every producer and validator (the mob ephemeral producer and
  the machine-terminal validation still minted/expected bare hex — failing
  every completed-run commit on those paths).
- `cargo check -p meerkat-core --no-default-features` compiles again
  (ungated `JsonSchema` derive demanded a feature-gated impl; fixed with a
  schema-invariant `cfg_attr` skip).
- Debug-build stack overflow at 2 MiB worker stacks: the agent loop's
  1700-line polling arm is split per-phase (813K → 170K frame), child-agent
  construction runs on a fresh task via the new
  `meerkat_runtime::stack_relief` (never nested in a parent's poll stack),
  and the mob stack-budget gate is un-ignored at 2 MiB / opt-level 0
  (measured high-water 1.9–2.0 MiB before, ≤1.125 MiB after; budget and
  opt-level untouched).

### Changed

- The turn-latency smoke gate asserts what is actually true today: fixture
  validity, a calibrated small-side band on bytes hashed per turn
  (instrument honesty — a broken counter reads zero, an inflated baseline
  reads high), and a per-fixture boundary-serialization envelope
  (repeated-reserialize backstop, using a new
  `global_session_encode_bytes` counter, because a digest-flat turn can
  still hide an O(document) reserialize that hashes nothing). The
  large/small FLATNESS ratio is recorded as a measurement, not asserted:
  size-independent turn-boundary work has not landed, and its assertion
  lives in `mob_turn_flatness_red_by_design`, deliberately outside every
  lane until the witness-v3 migration arms it. Shipping a known-red
  assertion inside a required lane would have been a false green.
- Large-fixture turn cost 479 → 435 MB hashed per turn (sealed
  transcript-history threading; the 8 per-turn document decodes reuse the
  sealed parse; transcript-history extracted from `session.rs` into
  `session/transcript_history/`). Not flat yet — remaining drivers are fully
  attributed (history witness 109 MB/turn with a format-v3 design under
  review; restore-seam revalidation 47; compaction-path revalidations ~59;
  per-decode graph validation 55, proven not memoizable; checkpoint
  canonical passes 36). Note the large fixture runs a compaction rewrite
  every measured turn; real sessions pay that share at compaction cadence.

### Added

- **`claude-opus-5`**: Claude Opus 5 joins the curated Anthropic catalog
  (Claude 5 family sibling of Fable 5; 1M context, 128k max output,
  adaptive-only thinking, full low..max effort ladder, compaction +
  structured outputs + web search, sampling parameters removed) and
  replaces `claude-opus-4-8` as the Anthropic provider default. The
  cross-provider global default is unchanged (`gpt-5.6-sol`).

### Added (storage unification arc)

- **`meerkat-sqlite`**: new leaf crate owning the shared SQLite mechanics —
  DDL-free connection opening under named policy profiles (`Primary` /
  `ReadOnly` / `Maintenance`), the per-file schema migration ledger
  (`meerkat_schema(domain, version)` with the pinned concurrent-open
  transaction protocol and a typed, health-visible `SchemaFromTheFuture`
  refusal), the `JsonColumnBytes` codec, per-operation maintenance-fence
  guards, and storage-level error classification.
- **`meerkat-store-conformance`**: new published crate with the per-trait
  storage conformance profiles (baseline / incremental / guarded-projection),
  the capability-discovery chapter (`as_incremental` swallow made loud), the
  append-only-media chapter (emulated-CAS revision-guard semantics,
  superseded-sibling dedup ownership, checkpoint monotonicity), the
  legacy-data axis, and blob/artifact chapters — downstream backends run the
  identical suite.
- **`meerkat_core::StorageLayout`**: the single path authority resolved at
  bootstrap (invocation context, walked-up project root, the
  `user_home_root`/`user_rkat_root` split, credentials/comms-identity/cache
  slots, state root), plus realm-id-first dual-root resolution: an explicit
  `--state-root` wins; a realm existing under exactly one candidate root
  (project-local `.rkat/realms` or the user-global data dir) is used where it
  lies; both is a typed split-brain refusal pointing at doctor; the resolver
  never creates an empty twin.
- **`meerkat::storage_provider`**: the `RealmStorageProvider` seam (one
  provider supplies all durable stores for a realm; the facade composes),
  machine-readable durability classes (`Durable` / `RebuildableCache` /
  `Scratch`) with fail-closed enforcement (an undeclared non-persistent
  durable slot is a startup error, never a silent in-memory fallback), and
  realm-manifest v2 read defenses (`manifest_format` refusal for future
  formats; provider-pinned realms refuse disk opens typed).
- **`rkat storage doctor`** (read-only, live-realm-safe, `--json`): per-root
  realm inventory, schema-ledger state per database, dual-root twin
  detection, checkpoint-evidence census, dangling blob references, orphaned
  lease/lock/backup artifacts. `StorageMigrator::diagnose` is the
  shape-stable hook remote/mobkit backends implement.
- **`rkat storage migrate [--apply]`** (dry-run by default, offline,
  fail-closed) and **`rkat storage prune`**: ledger baselining, split-brain
  reconciliation (exact-dedup report + adopt-one-root/archive-other; no
  synthesis), bulk machine-owned legacy-checkpoint adoption
  (`PersistentSessionService::adopt_legacy_checkpoints`), registered
  `*.pre-<version>-<timestamp>` backup artifacts with a prune lifecycle.
- New CI gate `storage-ambient-gate`: bans ambient root resolution
  (`dirs::*`, `HOME`/`XDG` reads) in production code outside the
  bootstrap/layout modules and documented conventions.

### Changed (storage unification arc — operator-visible)

- The shared SQLite opener is DDL-free: schedule/runtime opens no longer
  plant empty session tables in co-tenant files (existing stray tables are
  tolerated by ledger baselining and doctor).
- `SQLITE_BUSY_TIMEOUT_MS` harmonizes on the one shared 60s value (was
  redefined six times at 5s/60s); the workgraph attention-column upgrade and
  the mob event-route/operator-fence upgrades became once-per-file ledger
  migrations instead of per-open probes.
- Every SQLite file gains a `meerkat_schema` ledger table and a sibling
  `<file>.mfence` fence-lock file (operators reading realm directories will
  see both; doctor inventories them).
- Server surfaces (`rkat-rpc`/`rkat-rest`/`rkat-mcp`) keep their no-flags
  behavior; with an explicit `--context-root` plus `--realm`, a realm already
  materialized project-locally now resolves to that root instead of the
  user-global default.

### Deprecated (storage unification arc)

- `StorageConfig.directory` (never consumed by any surface; warns when set).
- The ambient no-`_in` realm helpers (`realm_paths`, `start_realm_lease`,
  `inspect_realm_leases`, `ensure_realm_manifest`,
  `open_realm_session_store`) and `meerkat_skills::resolve_repositories` —
  use the explicit-root variants.

### Added (durable jobs arc)

- **`meerkat-jobs`**: new publishable crate owning realm-scoped detached job
  execution. The generated `DetachedJob` machine is the single lifecycle
  authority: attempt leases with fencing tokens, heartbeats, restart
  checkpoints, typed terminals, and machine-authorized retry (scheduled
  retry due times are honored across recovery). Ships `DetachedJobStore`
  (SQLite-backed on a new per-realm `jobs.sqlite3` under the `jobs` ledger
  domain, plus in-memory), predicate watches, and a durable notification
  outbox.
- **Runtime delivery inbox** (`meerkat-runtime`): job terminals and
  notifications reach the origin session through a generated ordered-cursor
  delivery machine — idempotent submission, sequence-ordered application,
  exactly-once acks — drained by a per-realm driver. Its tables live under
  the lazily-provisioned `runtime-delivery` ledger domain (see Fixed: only
  actually using durable delivery stamps a realm's file).
- **Durable callback protocol**: external callbacks now carry
  `tool_use_id` end to end, and a suspended assistant batch with multiple
  external calls is typed instead of impossible —
  `AgentError::CallbackBatchPending` surfaces the complete pending set
  (`PendingCallbackToolCall`) so a host supplies one exact result set; no
  callback is silently selected or dropped.
  `AgentEvent::InteractionCallbackPending` gained the full
  `pending_tool_calls` set (empty on lines written by older producers).
- **17 new JSON-RPC methods** (generated catalog + `docs/api/rpc.mdx`):
  `jobs/get`, `jobs/list`, `jobs/cancel`, `jobs/progress`, `jobs/result`,
  `jobs/artifacts`, `jobs/retry`, `jobs/health`, `jobs/subscribe`,
  `jobs/unsubscribe`, `monitors/start`, and the host-worker lease surface
  `mobkit/jobs/heartbeat`, `mobkit/jobs/progress`, `mobkit/jobs/checkpoint`,
  `mobkit/jobs/complete`, `mobkit/jobs/fail`, `mobkit/jobs/cancel_ack`.
  Wire types in `meerkat_contracts::wire::jobs` feed the regenerated
  schemas and SDK types.
- **Streaming-tool supervision** (`meerkat_core::streaming_tool`): a tool
  declared `Streaming` runs under a canonical supervisor that hands it a
  typed progress sink and cancellation token
  (`ToolStreamingDispatchContext`); validated `ToolProgressFrame`s reset an
  inactivity watchdog, and a stalled stream fails typed as
  `ToolError::InactivityTimeout` (error code `inactivity_timeout`, wire
  class `timeout`) under the execution policy's inactivity and absolute
  deadlines instead of hanging the turn. Mob member upcalls carry the new
  class across the bridge. Fast and detached dispatch contexts expose no
  streaming surface; a streaming-declared tool fails closed without one
  rather than fabricating progress authority.
- **Shell builtin rewritten onto durable jobs**: `shell(background: true)`
  runs as a realm-scoped detached job — durable progress and terminal
  delivery back into the origin session, blob-spooled output, a monitor
  protocol for live supervision (`monitors/start`), and restart recovery
  that reconciles still-running jobs instead of forgetting them.
- **Jobs × WorkGraph × Schedule composition** (`meerkat::job_composition`):
  the ownership seams stay separate — `DetachedJobMachine` owns execution,
  WorkGraph owns evidence and closure, Schedule owns occurrence delivery,
  and MeerkatMachine owns explicit per-session wait bindings — and the
  facade composes them: `ScheduledJobTemplate` /
  `ScheduledDurableJobRunnable` let schedule occurrences launch durable
  jobs as host runnables, `JobWorkGraphLink` +
  `JobTerminalEvidenceProjector` file typed job-terminal evidence onto work
  items, and `JobAwaitCoordinator` registers deterministic wait bindings
  (`OperationId::for_detached_job_wait`) that a reconstructed runtime
  re-derives after volatile operation state is discarded. A shared-realm
  e2e lane exercises the full loop.

### Changed (durable jobs arc — operator-visible)

- **The external-callback deadline quadrupled: 30s → 120s.** Every
  `tools/register` client (IDE hosts, embedding gateways) now holds a turn
  open for up to two minutes on an unresponsive callback host where it
  previously failed at thirty seconds; hosts and transports add bounded
  handoff margins of 125s/130s on top
  (`meerkat-rpc/src/callback_dispatcher.rs`). Callers that relied on the
  30s failure as a liveness probe must bring their own timeout.
- **`shell(background: true)` now requires a durable realm.** Detached
  execution is gated on a persistent job store AND a persistent blob store
  AND a realm id with a delivery projector; where those are absent —
  memory-backend realms, ephemeral services, WASM, most examples — the
  former in-process background shell is gone and the call fails closed with
  a typed tool error ("detached shell execution requires a durable realm
  job/blob runtime"). This is a deliberate capability removal from a
  default-on builtin: an in-process fallback would silently drop the
  durability the tool now advertises.
- Sqlite-backend realms gain a `jobs.sqlite3` database (with a `jobs`
  schema-ledger domain), created at realm open. Pre-0.8.8 binaries never
  open this file, so it is rollback-inert.
- **Error-taxonomy debt, recorded honestly:** the typed deferred
  tool-results ingress refusals (`DeferredToolResultsIngressError`) and the
  pending-callback-batch validation errors are currently flattened to
  `AgentError::ConfigError` strings at the session boundary
  (`classify_callback_result_ingress`, `try_stage_tool_results` call
  sites), so SDK callers see one `config_error` code and cannot distinguish
  "safe to retry" from "client bug". Typed surfacing is a follow-up.

### Fixed (durable jobs arc)

- **Three upgrade/rollback breaks closed before they could ship.** All
  three were invisible to CI because no legacy on-disk fixtures existed;
  each now has a v0.8.7-shaped regression fixture that fails on the
  pre-fix code.
  - **A 0.8.8 realm open no longer locks 0.8.7 out.** The durable-delivery
    tables were originally migration 2 of the `runtime-store` ledger
    domain, applied unconditionally at every realm open — so the first
    0.8.8 open stamped `runtime-store=2` into `sessions.sqlite3` and a
    v0.8.7 binary then refused EVERY realm open
    (`SchemaFromTheFuture`, no downgrade verb). `runtime-store` is pinned
    at version 1; the delivery tables moved to their own `runtime-delivery`
    domain, provisioned lazily by the first durable-delivery WRITE (reads
    of an unprovisioned domain report empty and stamp nothing). Older
    binaries never read foreign ledger domains, so a delivery-stamped file
    still opens on v0.8.7 for everything except the delivery feature it
    predates.
  - **Pre-0.8.8 persisted rows keep decoding.** The durable-callback
    `tool_use_id` was added as a required field inside two persisted
    contracts without a version change: stored input-state rows
    (`interaction_terminal_outbox` candidates, whose SHA-256
    `candidate_digest` must also keep verifying byte-identically) and the
    session event log (`run_failed` callback reasons under
    `EVENT_SCHEMA_VERSION` 2). Both now decode legacy shapes via
    optional/defaulted fields that re-serialize byte-identically, so
    upgraded fleets read their own history. Known residue, on record:
    unpublished mid-flight callback rows WRITTEN BY 0.8.8 do not survive a
    downgrade to v0.8.7 (settled and published data does); the follow-up
    that removes the persisted id entirely is chartered for the next
    release.
  - **One damaged input-state row no longer poisons a runtime.**
    `RuntimeStore::load_input_states` previously turned a single
    undecodable row into a whole-call `ReadFailed`, making every durable
    input of that runtime unreadable (recovery then backs off forever —
    the session is bricked). Rows now surface individually with a typed
    per-row corruption witness; recovery proceeds loudly with the
    decodable rows and leaves the damaged row on disk for forensics, and
    strict callers get the failing row's identity in the error.
- **Durable delivery livelock on lifecycle-terminal waits.** A retired or
  unregistered wait is persisted as `Terminated`, but terminal matching
  recognized only `Completed`/`Cancelled`/`Failed` — the resulting
  `Corrupt` error aborted the drain before the row was acked and the ~1s
  driver retried an unackable delivery forever: head-of-line blocking at
  three levels (session inbox, cross-session drain, per-job projection).
  Terminal acceptance is now an exhaustive match with no wildcard arm (a
  new outcome variant is a compile error, not a silent corrupt bucket);
  the drain is fail-safe by RETENTION, never discard — per-row/per-job
  outcomes let rows ahead of a poisoned one still apply and ack, other
  jobs keep projecting, and the RPC drain aggregates failures instead of
  aborting the realm; the delivery driver backs off 1s → 60s on a
  non-progressing pass and resets on progress.

### Breaking

- **Removed public helpers (storage unification arc):**
  `meerkat_core::config::find_project_root`,
  `meerkat_core::config::data_dir`, and the `meerkat_core::config::dirs`
  module (its `home_dir` HOME-env stub) were removed. Replacements:
  project-root discovery is `meerkat_core::storage_layout::find_project_root`
  (re-exported at the crate root; note the semantic change — any existing
  `.rkat` entry counts, not only a directory, matching the historically live
  `meerkat_tools::find_project_root` behavior, which now delegates to it).
  `data_dir()` has no drop-in replacement: realm state roots resolve through
  `meerkat_core::StorageLayout` / `default_state_root()`, and the user
  `~/.rkat` root is `StorageLayout::user_rkat_root()`. The `dirs::home_dir`
  stub maps onto `StorageLayout::user_home_root()` (or the
  `user_config_root` bootstrap input). `StorageConfig::default()` no longer
  pre-fills `directory` from the ambient data dir (the field is deprecated
  and ignored).
- `meerkat_core::realm_exists_under` now returns
  `Result<bool, RuntimeBootstrapError>` and probes fail closed: only
  NotFound means absent (other IO outcomes are the new typed
  `RootProbeFailed`), an identity-colliding directory (two realm ids
  sanitizing to one directory name) is the new typed
  `RealmDirectoryCollision`, and an unreadable manifest is the new typed
  `ManifestUnreadable`. `DualRootResolution` gained the required
  `candidate_roots` field, and `meerkat::PersistenceError` gained the
  `Bootstrap` and `FirstStart` variants (cross-candidate first-start
  reservation refusals from
  `meerkat_store::realm::ensure_realm_manifest_pin_with_candidates`).
- Generated `MeerkatMachine` recovery alphabets replace the eight
  phase-shaped `RecoverRuntimeAuthority*` inputs with the single total
  `ClassifyRuntimeAuthorityReconciliation` observation input. Downstream
  exhaustive matches over generated inputs, input kinds, or transition kinds
  must handle the new classifier instead of replaying a persisted process
  phase as runtime authority.
- `meerkat_core::RefreshFailureObservation::requires_reauth` was removed;
  `meerkat_auth_core::auth_oauth::OAuthRefreshPermanence` and
  `OAuthError::refresh_permanence` were removed with the competing public
  permanence verdict;
  AuthMachine now returns the typed `RefreshFailureDisposition` through
  `AuthLeaseHandle::resolve_refresh_failure_disposition`. The exhaustive
  `RefreshError` enum gained `Classified` and `ReauthRequired`, and
  `DurableTerminalCommit` gained the required `disposition` field. Refresh
  shells must submit boundary observations to AuthMachine and project its
  verdict instead of classifying permanent failures themselves.
- `meerkat_mob::AdaptiveDriverRuntime::provision_layer` now returns
  `AdaptiveLayerProvision<Self::Layer>` instead of `Result<Self::Layer,
  AdaptiveError>`, so a failed provision can retain teardown ownership of a
  partially acquired layer. The trait gained the `Capability` associated type,
  provisioning receives the capability/layer attempt, cleanup borrows an
  `AdaptiveLayerLease<Self::Layer>`, and provision results now carry that
  cancellation-safe lease. `AdaptiveKernel` gained the required
  `record_layer_interrupted` and `cancel_run` methods; its synchronous
  `initialize_run` now returns `AdaptiveRunInitialization`, whose independent
  command/reply owner publishes the accepted capability together with an armed
  run lease. Kernels must retain that ownership from initialization enqueue
  until the run reaches a machine-owned terminal state.
- Generated AuthMachine alphabets replaced the raw-observation `RefreshFailed`
  payload with a typed disposition and gained the runtime-internal
  `ResolveRefreshFailureDisposition`; generated MobMachine public alphabets
  gained `RecordLayerInterrupted` plus replay-safe adaptive cancel and cleanup
  transition variants. Downstream exhaustive matches over the generated input,
  effect, input-kind, or transition-kind enums must handle the new variants.
- `meerkat_core::CompactionResult` gained the required
  `summary: CompactionSummary` and `retained: Vec<CompactionRetained>` fields,
  and its `discarded` field is now `Vec<CompactionDiscard>` instead of
  `Vec<Message>`. Custom compactors must identify the exact rebuilt summary
  slot, use the canonical typed summary content, provide exact source/rebuilt
  offsets for retained messages, and provide canonical source offsets for
  discarded messages; retained and discarded entries must partition the full
  pre-compaction transcript.
- `meerkat_core::agent::compact::CompactionOutcome::discarded` is now
  `Vec<CompactionDiscard>` instead of `Vec<Message>`, and the exhaustive
  `CompactionError` enum gained `InvalidRebuild(String)`.
- `meerkat_core::AgentLlmFallbackSwitch` replaced the caller-owned
  `capability_base_filter`, `context_window`, and `max_output_tokens` fields
  with the required `target_profile: ModelProfileWitness`. Profile witnesses
  are minted by `ModelRegistry::profile_witness_for_provider` for one exact
  typed provider/model pair and one construction-scoped effective-registry
  authority; unresolved targets and unprovenanced or foreign-registry profiles
  fail closed. The exhaustive `AgentBuildPolicyError` enum gained
  `MissingModelRoutingHandle` and `MissingEffectiveModelRegistry`.
- `meerkat_core::AgentLlmClient::commit_model_fallback` now takes
  `(previous_identity, target_identity)` and returns `Result<(), AgentError>`
  instead of accepting one identity with a default no-op. Fallback-capable
  clients must also implement the new `active_model_fallback_identity` exact
  identity projection and, when supporting structured extraction fallback,
  `compile_model_fallback_schema` for inactive target-client compilation. The
  client-local `active_capability_base_filter` and `active_max_output_tokens`
  semantic projections were removed; ToolScope and the registry-minted active
  profile now own those facts across later turns.
- `meerkat_core::handles::ModelRoutingHandle` gained the required
  `stage_sticky_model_fallback(activation_proof, visibility_plan)` method,
  which returns a one-shot `StickyModelFallbackMachineCommit` instead of
  mutating immediately.
  `StickyModelFallbackActivationProof` has no public constructor and is minted
  only by the core agent loop after recovery and effective-registry validation;
  implementers must preview the exact generated parent and make the returned
  token reject if that parent changes before identity, registry-proven profile
  provenance, routing, and visibility are committed.
- Generated MeerkatMachine public alphabets gained the exhaustive
  `CommitStickyModelFallback` input and `CommitStickyModelFallbackRunning`
  transition. Downstream matches over generated DSL/kernel input, input-kind,
  or transition-kind enums must handle the new variants.
- Generated MobMachine public alphabets gained the exhaustive
  `ClassifyRetirePendingSpawnDisposition` input and structural retire
  disposition effects. Retire shells must consume the machine-authorized exact
  runtime, generation, and pending session instead of deriving cancellation
  from a roster projection.
- `meerkat_runtime::MeerkatMachine::unregister_session` now returns
  `Result<(), RuntimeDriverError>` instead of `()`. Required cleanup callers
  must propagate or explicitly combine machine-owned unregister failures. The
  `Ok(())` now proves removal rather than meaning another teardown may still
  be running. Concurrent stop/unregister callers join the same machine-owned
  teardown result, and a runtime loop cannot await or abort its own unregister
  coordinator task.
- `meerkat_runtime::mob_adapter::unregister_mob_member` now returns
  `Result<(), RuntimeDriverError>` instead of `()`. Mob-facing callers must
  handle incomplete machine-owned unregister cleanup explicitly.
- `meerkat_runtime::RuntimeStore` gained the required
  `commit_unregister_finalization(runtime_id, commit, input_states)` method.
  Custom runtime stores must atomically publish the machine lifecycle/input
  updates and delete the matching ops-lifecycle snapshot; a fallback composed
  from `commit_machine_lifecycle` plus `delete_ops_lifecycle` is not crash-safe
  and is deliberately unsupported. Ordinary errors guarantee no finalization
  effects. Stores that cannot classify a commit acknowledgement use the new
  non-exhaustive
  `RuntimeStoreError::UnregisterFinalizationOutcomeUnknown` variant; it maps to
  `RuntimeDriverError::UnregisterFinalizationOutcomeUnknown`, which retains the
  live retry anchor without publishing an unsafe compensating lifecycle
  rollback.
- Semantic-memory compaction gained a resultful staged-projection lifecycle.
  `MemoryStore` now exposes `compaction_projection_persistence`,
  `stage_compaction_batch`, `finalize_compaction_batch`,
  `abort_compaction_batch`, and `reconcile_compaction_stages`; unknown stores
  default to `CompactionProjectionPersistence::Unsupported` and must opt into
  either process-local immediate publication or durable staging. `CoreExecutor`
  and session-agent/service execution hooks gained compaction-outbox
  reconciliation, while `RuntimeStore` gained capability, pending-load, and
  finalized-ack methods for the atomic compaction projection outbox. Custom
  durable stores/executors must implement these seams or compaction fails
  closed while preserving transcript history.
- `TranscriptRewriteSelection` gained current-format `EditMessageRange` and
  `CompactionMessageRange` variants. New generic `MessageRange` requests are
  persisted as typed edits, while only the core compaction path can mint the
  opaque compaction semantic. Exhaustive downstream matches must handle the
  new variants; presentation code can use `TranscriptRewriteSelection::bounds`
  to retain the existing half-open-range rendering.
- Exact per-turn self-hosted routing added the required
  `self_hosted_server_id` field to public `RuntimeTurnMetadata`,
  `WireRuntimeTurnMetadata`, `SessionLlmIdentityOverride`, and
  `SessionLlmReconfigureRequest` literals. Exhaustive matches must also handle
  the new `MobMemberCapability::SessionLlmReconfigure` and
  `SessionLlmIdentityOverrideError` variants. The public
  `SessionRuntimeLlmReconfigureHost::service` field now accepts
  `Arc<dyn SessionRuntimeLlmReconfigureService>` instead of the concrete
  session service so embedded runtimes can install the same canonical
  reconfiguration boundary.
- **Callback identity is typed through the public enums (durable jobs
  arc):** `AgentError::CallbackPending` gained the required `tool_use_id`
  field, and `meerkat_runtime::completion::CompletionOutcome` and
  `meerkat_core::lifecycle::core_executor::CoreApplyTerminal` (neither is
  `#[non_exhaustive]`) gained `CallbackBatchPending` variants — external
  exhaustive matchers and field-exhaustive destructuring of the
  callback-pending shapes must be updated. The wire siblings
  (`AgentErrorReason::CallbackPending.tool_use_id`,
  `AgentEvent::InteractionCallbackPending.pending_tool_calls`) default when
  absent, so previously persisted events keep decoding; only source-level
  matching breaks.
- **`RuntimeStore::load_input_states` returns per-row outcomes (durable
  jobs arc):** the signature moved from `Vec<StoredInputState>` to
  `Vec<InputStateRow>` (`Decoded` | `Corrupt { input_id, detail }`) so one
  damaged row cannot poison the load; implementations must wrap rows, and
  callers wanting the old all-or-error semantics use the provided
  `load_input_states_strict`.
- **New variants on exhaustively-matchable enums (durable jobs arc):**
  `ToolError` gained `InactivityTimeout { name, inactivity_ms }` (error
  code `inactivity_timeout`; maps to the `timeout` wire class),
  `OperationKind` gained `DetachedJobWait` (`OperationKind::ALL` is now
  four entries), and `OperationSource` gained `DetachedJob { realm_id,
  job_id }`. None of these enums is `#[non_exhaustive]`, deliberately —
  external exhaustive matches must add the arms. (Per this project's
  versioning policy these are declared clean breaks; a compile error on
  the next variant is the intended behavior, not an oversight.)

### Added

- **`IncrementalSessionStore` range-read capability probes** (substrate for
  head-trusted O(page) history reads; the service read path is unchanged in
  this release): two additive verbs with conservative defaults.
  `load_canonical_head` returns the persisted head row only when head+rows
  are the session's canonical durable representation — `None` for absent AND
  blob-only sessions, never synthesizing from the legacy blob (default:
  `None`, so non-overriding stores and non-forwarding wrappers stay on the
  whole-load path). `load_rewrite_commits` returns the adopted rewrite
  commits, oldest first, without materializing retained revision bodies, and
  always equals `load_rewrites`' commits (default derives from
  `load_rewrites`). `SqliteSessionStore` and `MemoryStore` override both
  (head-row-only probe; commit-row-only query bounded by the adopted count);
  `JsonlStore` is deliberately untouched (whole-blob, no incremental
  capability). The conformance suite gains conditional pins: `None`-for-all
  stores remain conformant, an advertised canonical head must be the
  persisted row with page-exact strand serving, and delegating wrappers must
  forward both verbs (`capability_discovery` chapter).
- Mob identity convergence now exposes actor-serialized declaration-manifest
  apply/read APIs backed by durable direct desired material and wiring,
  bounded incarnation-fenced leases, immutable operation receipts, total
  stored-row observations, and output-only convergence status. The generated
  stateless classifier selects one resource-local obligation per fresh
  observation pass; conflicts requeue and re-observe.
- Mob members can now admit completion-bearing tracked turns through
  `MemberHandle::start_turn` and `MemberTurnOptions`. The returned
  `MemberTurnHandle` separates ingress admission, actual executor-applied LLM
  identity, and committed terminal completion; terminal events remain gated on
  the MobMachine outcome, including finalization and structured-extraction
  failures.
- Embedded runtimes can install `SessionRuntimeLlmReconfigureHostBlueprint` to
  apply provider, model, exact self-hosted server, provider-parameter, and auth
  overrides at the serialized executor boundary. Queued turns retain their own
  identity route, and unsupported member backends reject overrides before mob
  admission instead of silently using the prior model.

### Fixed

- **Idle busy-loop, part 2: the per-member idle drivers are gone.** The
  0.8.5 schema-caching fix removed the idle spin's amplifier; the drivers —
  fixed-cadence loops doing session-document- or fleet-state-scale work per
  tick — remained and burned ~0.3 core per idle durable member as I/O and
  serialization churn. Three drivers fixed:
  - **Checkpoint re-verification is memoized.** Every 25 ms identity
    reconcile pass re-verified full checkpoint-stamp digests (canonical
    JSON + sha256 over the whole session document). A process-lifetime
    bounded memo keyed on stamp digest and document shape now verifies each
    (session, revision) once; recovery/write paths still verify in full.
    `TranscriptHistoryState` gained a `digest_format` marker so decoding a
    current-format document no longer pays the legacy-heal digest probe,
    and slim head materialization is verified once per head revision. This
    also cuts boot: production-scale restores paid the same redundant
    digests on every read (~1.3 s/MB observed).
  - **Converged identities skip per-pass session loads.** The mob actor's
    identity reconcile keeps a convergence witness per member; steady-state
    passes no longer load and re-serialize the session document at all
    (full re-verify is retained on a 300 s interval and on any witness
    invalidation).
  - **Member-list projection no longer deep-clones machine state.** All
    `list_members*` projections borrow the actor-published
    `MobMachineState` watch inside sync blocks instead of cloning the full
    (restore-scale) state per call — the former 250 ms monitor cadence in
    mobkit cloned it 16×/s per handle. New opaque change seam
    `MobHandle::machine_state_changes()` (`MobMachineStateChanges`) lets
    interval observers go event-driven instead of polling projections on a
    timer, and `MobMcpState::mob_set_changes()` signals managed-mob-set
    changes without snapshot polling.
- **Upgraded fleets whose continuity rows were adopted while runtime
  snapshots stayed legacy are no longer refused on every resume.** In
  mobkit identity-first fleets, sanctioned adoption (lazy-at-restore or the
  operator migrate tool's bulk sweep) stamps the continuity session-store
  row with typed checkpoint authority while the RuntimeStore still holds
  the pre-adoption legacy session snapshot. The one-time legacy checkpoint
  migration mapped that shape to a blanket fail-closed refusal that falsely
  blamed transcript divergence and pointed at an operator tool that cannot
  heal it — bricking every such session on resume. The machine now resolves
  the shape relation-aware: when the typed authority's transcript contains
  the legacy snapshot's (identical or prefix extension), the new
  `ConvergeSnapshotOntoTypedProjection` disposition overwrites the stale
  snapshot with the typed authority bytes under the existing guarded
  commit (nothing is re-stamped); only a legacy snapshot carrying
  transcript content the typed authority lacks still refuses, with an
  error that names the actual shape and the computed relation. The
  `only_cursor_free` bulk-adoption guard also keeps this stamp-free
  convergence shape eligible instead of skipping it as cursor-ambiguous.
- **Idle busy-loop: machine schemas are constructed once per process.** The
  `machine!` macro now emits a `LazyLock`-cached `schema_static()` (with
  `schema()` cloning the cache), and the schedule/occurrence wire-header
  validation uses process-lifetime identity stamps. Previously every
  schedule-host tick (250 ms, per member) re-parsed entire machine DSLs per
  persisted row — ~0.25-0.3 core per idle durable member; an idle fleet now
  costs ~zero. On 0.8.2/0.8.3 this escalates to **availability loss** on
  restart: the spin contends with member restore, a large fleet's boot can
  exceed the client init timeout, and the host process-manager restart loop
  re-pays the full restore every iteration — the deployment never comes
  back up on default timeouts. Upgrading to this release removes the
  contention source; structural regression tests pin the caching (pointer
  identity) and the serde path (no schema construction per row) so the
  amplifier cannot silently return.
- **A stalled LLM provider stream no longer wedges the turn forever.** New
  stream-inactivity watchdog (`[retry] stream_inactivity_timeout`, **default
  ON at 300s**; `"disabled"` opts out): a provider call whose stream
  produces no events inside the window is aborted with the retryable
  `StreamStalled` failure, so one stall retries and repeated stalls exhaust
  the retry budget and fail the turn typed instead of hanging indefinitely.
  Behavior change: provider calls that previously sat silent for >5 minutes
  and eventually completed are now aborted and retried. The compaction LLM
  call is not yet watchdog-guarded (follow-up).
- **`force_cancel` is legal from `Running`.** Force-cancelling a running mob
  member now transitions through the machine's cancelling phase and
  interrupts the in-flight work instead of refusing with
  `invalid state transition: Running -> Running`; a second force-cancel
  while cancelling is an idempotent success, and force-cancelling an
  identity the roster has never seen answers `MemberNotFound`.

- **Mob-shutdown restart wedge: a cancelled checkpointer gate no longer
  silently drops the committed-boundary SessionStore projection.** Mob stop
  cancels (and retry-flaps) per-session checkpointer gates before member
  loops quiesce; boundary commits landing in that window advanced the
  RuntimeStore authority while the projection skip returned success, so two
  such commits left the SessionStore more than one revision behind — a
  divergence the resume-side authority reconciler refuses forever
  ("RuntimeStore and SessionStore checkpoint authorities conflict",
  identities permanently degraded after restart). The gate keeps its
  no-write-after-cancel contract but now fails with the retryable
  projection error, so the terminal-recovery drain re-projects the
  committed snapshot on restart and the stores reconverge. The
  authority-conflict error now names both stamps and their bases.

- Cold runtime recovery is level-triggered over every persisted lifecycle
  shape. Decodable prior-process rows normalize through target-local CAS to a
  fresh unbound Idle shell over the existing session; unsupported or unsafe
  malformed rows return typed repair refusal, transport failures back off, and
  no recovery shape can fall through `NoMatchingTransition`. Checkpoint
  content authority is now structurally independent from lease ownership:
  lineage, generation, revision, digest, and provenance establish the head,
  while fencing values seed only lease high-water. A Meerkat-to-MobKit crash
  matrix and the checksum-transferred HomeCore `parent-1` fixture prove that a
  continuity fence of 14462 with snapshot fence 11130 retains the exact
  generation-0/revision-859 session, all 371 messages, and consumed initial
  input across migration, lease takeover, runtime CAS, member registration,
  lost replies, status corruption, and cold restart.
- Runtime member-disposal retries now re-admit an exact machine-proven
  retained cleanup sidecar while cancellation-safe retirement is still
  uncertain. Stale registration or attachment witnesses continue to fail
  closed instead of blocking the valid retry path.
- CLI OAuth guidance now uses the inherited global binding, and embedded
  `rkat help` includes the exact mobpack manifest, definition, wiring, flow,
  required member-comms shapes, automatic flat-step role provisioning, and
  executable local signed-pack trust and web-runtime arguments.
- Added an explicit, dry-run-first `rkat session migrate` path for exact
  v0.6.34 completed-idle SQLite sessions. Apply mode refuses live/unknown realm
  leases, translates lifecycle and terminal input state through current machine
  authority in one transaction, and retains append-only original row bytes.
  Operators must still stop all v0.6.34 processes because one-shot legacy CLI
  runs did not consistently publish lease evidence.
- Full-tools CLI `mob_create` followed by `mob_spawn_member` now stays within
  the production 2 MiB Tokio worker-stack budget.
- Direct `rkat mob run` and `rkat mob deploy` now complete the legacy OAuth
  credential-read bootstrap before loading member runtime configuration.
- Rejected atomic runtime commits now abort every pre-commit session
  projection—not only compaction—including the live transcript/checkpointer
  and pending context events before recovery checkpoints can reload the prior
  authority. Failed compaction aborts retain the exact live retry carrier for
  teardown recovery instead of partially discarding the rejected run.
- Explicit JSON `null` payloads in typed system-notice blocks now remain
  present across deserialize/serialize round trips, preventing persisted
  transcript bytes and digests from changing when nullable comms, external,
  runtime, or unknown notice payloads are reloaded.
- Cold recovery of a Running local autonomous mob member now republishes the
  existing exact runtime/fence startup-ready transition after its comms drain
  and interaction injector are observed. Recovery does not synthesize a
  second kickoff; placed, turn-driven, and Stopped members retain their prior
  readiness semantics.
- Delivery-time Mob member revival now carries an explicit machine-authorized
  missing-live-materialization intent. Exact ownerless `Idle` or `Attached`
  records with `Queuing` or orphaned `Active` registration are re-driven
  in-process through the generated
  executor-exit observation and same-session resume intent, clearing the full
  V3 runtime-binding tuple without touching queued inputs. Executor
  publication is also cancellation-safe: the exact runtime-loop attachment,
  its initial backlog wake, and startup projection/persistence belong to the
  loop owner, preventing a false `Attached`/ready state without an executor.
- Cold local-resource registration now treats every recovered runtime binding
  as a prior-process fact, including bindings whose ops epoch is intentionally
  retained. The existing generated executor-exit/readmission ladder clears the
  dead tuple before a reset or reprofile binds its distinct runtime identity,
  preventing a guard rejection followed by unbounded teardown.
- Transcript revision history no longer retains a complete mechanical head
  snapshot after every ordinary append once any audited rewrite exists.
  Snapshots preserve genuine rewrite endpoints plus the live head, typed
  synthetic-notice maintenance stays off the audited undo path, and legacy
  parentless revision chains compact on read/save with strict digest, lineage,
  recurrence, cycle, and persisted-MCP integrity checks intact. Long-lived
  sessions therefore stop growing quadratically with turn count while real
  transcript revisions remain listable and restorable.
- Explicit mob-member resume now revives an archived session in authority
  order: promote the session document, synchronize the live semantic snapshot,
  reset the durable `Retired` runtime, then attach the executor. Ordinary
  executor registration still cannot cross the `Retired` terminal, and any
  later provisioning failure restores the exact `Archived` + `Retired` pair
  without leaving a live agent, executor, or provisioner sidecar.
- Python and TypeScript now validate canonical `comms/send` result variants and
  member-progress snapshots before returning them, rejecting missing, legacy,
  malformed, or unknown fields instead of casting them to generated types.
- Helper spawn/fork APIs now carry the canonical `model_override` through the
  Python, TypeScript, Web, and WASM lowering paths.
- Public mob retire now asks MobMachine for an incarnation-scoped pending-spawn
  disposition. Only the exact machine-authorized pending session can be
  canceled; an absent committed identity preserves a pending later incarnation.
- Batched runtime inputs now merge transcript causality field by field with a
  conflict-stable consensus. Distinct interactions can retain one shared
  objective, while a conflicting interaction, run, or objective remains
  cleared for the rest of the batch and cannot be reseeded by a later input.
- Durable compaction no longer exposes discarded-history memory before the
  authoritative transcript rewrite commits. The agent stages an invisible
  batch keyed by the exact semantic `TranscriptRewriteCommit`, RuntimeStore
  records the matching intent in the same `atomic_apply` transaction, and the
  runtime finalizes memory only from that durable outbox. Cancellation,
  process loss, cold restart, duplicate delivery, stale runtime epochs, and
  scope deletion now reconcile idempotently without compatibility SessionStore
  metadata becoming commit authority; unsupported and standalone durable
  pairings preserve the original transcript and fail closed. Finalized outbox
  identities remain tombstones, and every runtime snapshot write seam rejects
  stale metadata that attempts to replay one without re-publishing authority.
- Compaction projection authority is now derived from the typed rewrite
  semantic rather than the free-form audit reason. Generic rewrites cannot mint
  a projection by spelling `reason.kind` as `compaction`, including when their
  replacement contains a typed compaction summary. Marker-free durable records
  from earlier releases are migrated only when the retained parent/revision
  bodies prove a full-range shrinking compaction; existing projection IDs keep
  matching their exact legacy fingerprint.
- HNSW scope deletion now commits a durable per-session tombstone in the same
  transaction as visible-row and staged-projection removal. Concurrent or
  post-restart stage, finalize, reconcile, direct-index, and cached-index
  publication cannot resurrect the dropped scope; pending outbox finalization
  instead receives an idempotent zero-entry success and can clear its intent.

- Runtime executor stop now transfers required post-terminalization cleanup to
  an external teardown task and resolves a request-specific acknowledgement
  only after loop exit, cleanup, and completion authority settle. Ordinary
  stop preserves the registered `Stopped` session; explicit unregister and
  executor-required teardown continue through durable registry removal.
  Cleanup failure is returned to the exact stop caller and the exact executor
  handoff remains retryable instead of timing out on a two-second self-join or
  reporting success from the projected `Stopped` state alone.
- Sticky model fallback now prevalidates extraction/provider parameters and
  target-client schema lowering, reversibly activates the exact prebuilt client,
  and preauthorizes identity, effective-registry-scoped catalog-profile
  provenance, capability/routing truth, canonical visibility state, and
  visibility revision through generated MeerkatMachine authority. Persistent
  runtimes then supervise an exact control-only RuntimeStore CAS before
  consuming the generated commit token and publishing prepared session/request
  state. The CAS changes identity and typed visibility without committing the
  in-flight prompt, fallback notice, transcript history, or usage; a terminal
  fallback retry and cold restart therefore remain sticky to the target without
  duplicating the failed input. Lost acknowledgements are reconciled by exact
  reread, machine rejection compensates the exact CAS, and unprovable outcomes
  force executor teardown. Client-proposed capability filters, context windows,
  and output limits are absent from the public fallback proposal and are
  derived only from the captured effective registry; unresolved fallback
  targets are rejected. The generated routing commit requires an opaque,
  core-minted one-shot activation proof, while later turns read ToolScope and
  the registry-minted active profile instead of client-local semantic
  projections. The supervised result survives a dropped run future so hard
  interrupt settles publication or compensation before session discard.
  Standalone sessions now share one turn/routing/visibility authority for the
  same path. Fallback visibility also rebases the staged revision after its
  immediate active commit, preserving pending staged intent without allowing a
  later boundary to decrease the active revision.
- Archive cleanup no longer reports success after generated unregister,
  lifecycle-persistence, or snapshot-finalization failure. Failed post-commit
  durability restores a retryable machine snapshot and required live/RPC
  cleanup callers preserve both primary and cleanup errors.
- Runtime completion relays now publish an outcome only after required cleanup
  succeeds. REST, RPC, and MCP treat archive/live-state reads and generated
  cleanup resolution as fallible authority, retain pre-admission/surface state
  on failure, and reserve benign cleanup races for a proven archived session
  whose runtime registration is already absent.
- Mob runtime teardown awaits the authoritative unregister without a competing
  timeout, probes no unknown state as absence, attempts every member binding,
  and retains the actor outside `Stopped` as the retry owner until cleanup is
  complete.
- Compaction validates exact summary/retained/discarded provenance, requires a
  real non-growing reduction, prepares a transcript replacement, and rejects
  arbitrary summary insertion or no-op rewrites before committing the
  semantic-memory projection. Failed rewrite/index attempts now still persist
  their cadence guard, preventing immediate retry loops as well as shifted
  source ranges or memory rows for content that never left the authoritative
  transcript.
## [0.8.8] - 2026-07-25

### Breaking

- `AgentError::CallbackPending` gained the required `tool_use_id` field, and
  exhaustive matches over `CompletionOutcome` and `CoreApplyTerminal` must
  handle their new `CallbackBatchPending` variants.
- `RuntimeStore::load_input_states` now returns per-row `InputStateRow`
  outcomes instead of `Vec<StoredInputState>`; strict callers use
  `load_input_states_strict`.
- Exhaustive matches must handle `ToolError::InactivityTimeout`,
  `OperationKind::DetachedJobWait`, and `OperationSource::DetachedJob`.

### Added

- Added durable detached jobs, persistent job stores, ordered runtime
  delivery, callback batches, job monitoring and RPC control, and durable
  background shell execution.
- Added supervised streaming-tool execution with typed progress,
  cancellation, inactivity, and absolute deadlines.
- Composed detached jobs with WorkGraph and Schedule, and added Claude Opus 5
  to the catalog as the Anthropic default.

### Changed

- The external-callback deadline increased from 30 seconds to 120 seconds.
  `shell(background: true)` now fails closed without a durable job, blob, and
  delivery runtime instead of falling back to process-local execution.

### Fixed

- Preserved pre-0.8.8 callback and event rows across upgrade, isolated corrupt
  input-state rows, and prevented one unacknowledgeable delivery from blocking
  other jobs or sessions.

## [0.8.7] - 2026-07-24

### Fixed

- Restored WASM compilation by using Meerkat's WASM-compatible `SystemTime`
  alias in checkpoint memo keys, and corrected the legacy digest fixture so it
  actually omits the modern `digest_format` marker.

## [0.8.6] - 2026-07-24

### Breaking

- `TranscriptHistoryState` gained the required public
  `digest_format: u32` field. Stored rows default the field for compatibility,
  but downstream struct literals must initialize it.

### Changed

- Added event-driven `MobHandle::machine_state_changes` and
  `MobMcpState::mob_set_changes` seams so observers do not need to poll and
  clone full machine projections.

### Fixed

- Removed the remaining per-member idle CPU amplification by memoizing
  checkpoint verification, skipping converged identity loads, and borrowing
  member projections instead of deep-cloning them.
- Corrected the crate publication order for runtime, comms, and provider
  packages.

## [0.8.5] - 2026-07-24

### Breaking

- Exhaustive matches over `LegacyCheckpointMigrationDisposition` must handle
  the new `ConvergeSnapshotOntoTypedProjection` variant.

### Fixed

- Upgraded fleets can converge a legacy runtime snapshot onto already-adopted
  typed session-store authority when the transcript is identical or a prefix.
  Divergent content still fails closed, and cursor-free migration eligibility
  is preserved.

## [0.8.4] - 2026-07-23

### Breaking

- Removed `meerkat_core::config::find_project_root`, `data_dir`, and the
  `config::dirs` module. Path discovery now goes through `StorageLayout` and
  `storage_layout::find_project_root`.
- `realm_exists_under` now returns `Result<bool, RuntimeBootstrapError>`,
  `DualRootResolution` gained required `candidate_roots`, and
  `PersistenceError` gained `Bootstrap` and `FirstStart` variants.

### Added

- Shipped the storage-unification arc: `meerkat-sqlite`,
  `meerkat-store-conformance`, the `RealmStorageProvider` seam, manifest-v2
  durability declarations, dual-root resolution, and `rkat storage doctor`,
  `migrate`, and `prune`.

### Changed

- Machine schemas are cached per process. Provider streams now have a
  default-on 300-second inactivity watchdog, and running-member force cancel
  is legal and idempotent.

### Fixed

- A cancelled shutdown checkpointer no longer silently drops a committed
  SessionStore projection and leaves the runtime and session authorities
  permanently divergent.

## [0.8.3] - 2026-07-22

### Breaking

- Generated session-document alphabets gained
  `ResolveRuntimeCheckpointProjection` and
  `ResolveLegacyCheckpointMigration` inputs with their effects and
  transitions. Generated Mob alphabets gained the
  `ResolveAutonomousShutdownMemberAction` family. Downstream exhaustive
  matches must handle the new variants.

### Fixed

- Added machine-owned late-write gates for archived sessions and terminal
  shutdown anchors.
- Added relation-aware, machine-owned migration of pre-typed session
  checkpoints and hardened it against races and partial adoption.
- Hardened flow dispatch, remote retirement recognition, release lockfile
  provenance, and runtime reliability.

## [0.8.2] - 2026-07-21

### Breaking

- Generated `MeerkatMachine` recovery alphabets replaced the eight
  `RecoverRuntimeAuthority*` phase inputs with the total
  `ClassifyRuntimeAuthorityReconciliation` observation.
- `Session::set_runtime_checkpoint_provenance` and
  `clear_runtime_checkpoint_provenance` became deprecated resultful calls;
  `has_runtime_checkpoint_provenance` was replaced by
  `try_has_runtime_checkpoint_provenance`.

### Added

- Added actor-serialized mob identity declaration manifests with durable
  desired state, leases, receipts, row observations, and convergence status.

### Fixed

- Cold runtime recovery is level-triggered over every persisted lifecycle
  shape, with checkpoint content authority separated from lease fencing and
  cross-version fleet fixtures covering takeover and restart.

## [0.8.1] - 2026-07-18

### Added

- Added dry-run-first `rkat session migrate` support for exact v0.6.34
  completed-idle SQLite sessions, with lease checks, append-only source
  retention, and one transactional apply path.

### Fixed

- Re-admitted exact machine-proven member-disposal retries, reclaimed cold
  same-epoch runtime bindings, and joined an unregister already in progress
  during shutdown.
- Kept full-tools mob spawn within the 2 MiB worker-stack budget and completed
  OAuth bootstrap before direct `rkat mob run` and `rkat mob deploy` config
  loading.

## [0.8.0] - 2026-07-17

### Breaking

- Removed `RefreshFailureObservation::requires_reauth`,
  `OAuthRefreshPermanence`, and `OAuthError::refresh_permanence`; auth refresh
  now projects AuthMachine's typed `RefreshFailureDisposition`.
- `AdaptiveDriverRuntime::provision_layer` now returns
  `AdaptiveLayerProvision`, and `AdaptiveKernel` gained resultful interruption,
  cancellation, and lease-owning initialization contracts. Generated Auth and
  Mob alphabets gained the corresponding exhaustive variants.
- Exact per-turn routing added required `self_hosted_server_id` fields to
  runtime and wire metadata and reconfiguration requests.
  `SessionRuntimeLlmReconfigureHost::service` now accepts the service trait
  object.

### Added

- Shipped distributed multi-host mobs and `rkat mob host`, including durable
  host bindings, placement, supervisor recovery, and conservative remote bind
  policy.
- Added completion-bearing tracked member turns and exact per-turn provider,
  model, self-hosted server, provider-parameter, and auth routing.

### Fixed

- Rejected runtime commits now abort every pre-commit session projection.
  Nullable system-notice payloads remain byte-stable, and cold autonomous
  members republish their exact startup-ready transition without a second
  kickoff.

## [0.7.31] - 2026-07-13

### Changed

- Updated Bazel `rules_cc` to 0.2.22.

### Fixed

- Re-drove ownerless missing-live mob-member materialization through
  machine-authorized revival, while making executor publication and its
  initial backlog wake cancellation-safe.

## [0.7.30] - 2026-07-12

### Changed

- Release manifests are derived from uploaded assets, and Homebrew publishing
  runs before the long Windows release build.

### Fixed

- Stopped quadratic transcript revision-history growth while preserving real
  rewrite endpoints and strict lineage validation.
- Revived archived mob-member sessions before executor attachment, with exact
  rollback if later provisioning fails.

## [0.7.29] - 2026-07-12

### Breaking

- `CompactionResult` and `CompactionOutcome` now carry typed summary,
  retained, and discarded provenance; custom compactors must satisfy the new
  reconstruction invariants.
- `AgentLlmFallbackSwitch`, `AgentLlmClient`, and `ModelRoutingHandle` now use
  registry-minted profile witnesses and staged sticky activation. Generated
  MeerkatMachine and MobMachine alphabets gained the corresponding routing and
  pending-spawn-retirement variants.
- Runtime and mob unregister APIs are resultful, and custom `RuntimeStore`
  implementations must atomically implement `commit_unregister_finalization`.
- `MemoryStore` gained the staged compaction-projection lifecycle, and
  `TranscriptRewriteSelection` gained typed edit and compaction ranges.

### Fixed

- Made durable compaction projection atomic, provenance-checked, restart-safe,
  and protected by durable scope tombstones.
- Made runtime stop, unregister, archive cleanup, completion publication, and
  mob teardown authoritative and retryable.
- Hardened SDK comms and member-result validation, preserved helper
  `model_override` across SDK, Web, and WASM surfaces, flattened release asset
  manifest paths, and corrected the Python helper parity fixture.

## [0.7.28] - 2026-07-11

### Breaking

- `meerkat_contracts::LiveOpenParams` gained the optional `seed_max_chars`
  field. Rust struct literals must initialize it; omission on the wire retains
  complete-history realtime seeding.
- `RealtimeSessionOpenConfig` and `meerkat_core::LiveProjectionSnapshot`
  gained `canonical_user_image_decoded_bytes`; downstream struct literals
  must initialize the new image-budget sidecar.
- `ReasoningEffort` and `WireReasoningEffort` gained `Max`;
  `OpenAiProviderTag` and `WireProviderTag::OpenAi` gained `reasoning_mode`,
  `reasoning_context`, `text_verbosity`, and `prompt_cache_options`; and
  `ModelCapabilities` gained `openai_responses_params`.
- Generated MobMachine state and exhaustive alphabets gained
  `member_prior_peer_endpoints`,
  `AuthorizeMemberEndpointMigrationTrustCleanup`,
  `RecoverSpawnedMemberPeerEndpoint`, `RecoverMemberPeerEndpoint`, and their
  associated kind and transition variants. Downstream state literals and
  exhaustive matches must handle them.

### Added

- Added OpenAI GPT-5.6 Sol, Terra, Luna, and the official `gpt-5.6` Sol alias
  with 1.05M context, 128K output, multimodal and tool capabilities, `max`
  reasoning effort, reasoning mode/context, text verbosity, and request-wide
  prompt-cache controls.
- `live/open.seed_max_chars` can now bound provider seed history to an
  affordable complete-turn suffix while preserving enabled root context, an
  affordable compaction summary, identity/tombstone/rewrite-generation and
  canonical-image sidecars, and explicit degraded-continuity reporting.
  Omission preserves complete-history behavior and disabled roots remain
  disabled.

### Changed

- `gpt-5.6-sol` is now the OpenAI provider default and Meerkat global catalog
  default. Existing explicit GPT-5.5 pins remain honored. GPT-5.6 is a limited
  preview; deployments without access should explicitly retain GPT-5.5.

### Fixed

- Cold restart and snapshotless session-head recovery now replay each member's
  exact generation peer endpoint from private, legacy-v8-compatible metadata,
  retain prior endpoints across same-generation rotation, and preserve the
  authority needed to clean them during retirement and crash retry.
- Endpoint recovery and retirement now fail closed on same-`PeerId`
  descriptor drift, cross-member `PeerId` reuse, and external topology
  without migration provenance; trust-repair batches preflight before mutation,
  and local or peer-only retirement removes historical and current trust rows.

## [0.7.27] - 2026-07-10

### Breaking

- `meerkat_contracts::RealtimeInputKind` gained the exhaustive `Image`
  variant and `meerkat_contracts::RealtimeInputChunk` gained the exhaustive
  `ImageChunk(RealtimeImageChunk)` variant. Downstream Rust matches over
  either enum must handle image input; generated SDK unions now include the
  corresponding `"image"` and `"image_chunk"` cases.
- Live image construction now requires a caller-stable `idempotency_key` on
  `meerkat_core::LiveInputChunk::Image`,
  `meerkat_contracts::LiveInputChunkWire::Image`, and
  `meerkat_contracts::RealtimeImageChunk`. Rust struct/variant literals must
  provide it. Python signatures are now
  `live_send_input_image(channel_id, idempotency_key, mime, data_base64)` and
  `LiveChannel.send_input_image(idempotency_key, mime, data_base64)`;
  TypeScript signatures are now
  `liveSendInputImage(channelId, idempotencyKey, mime, dataBase64)` and
  `LiveChannel.sendInputImage(idempotencyKey, mime, dataBase64)`.
- `meerkat_core::RealtimeTranscriptEvent` gained the exhaustive
  `UserContentFinal` variant. Provider/session integrations that match the
  internal event stream must handle canonical non-text user content. The
  public wire transcript union deliberately omits this byte-bearing command.
- `meerkat_core::RealtimeTranscriptApplyOutcome` gained the public
  `user_content` field, so downstream struct literals must initialize it.
  `meerkat_live::LiveProjectionSink::append_realtime_transcript` now returns
  `Result<RealtimeTranscriptApplyOutcome, LiveProjectionError>` instead of
  `Result<(), LiveProjectionError>` so the host can synthesize the redacted
  receipt only after durable reducer application.
- `meerkat_core::BlobStoreError` gained the exhaustive `InvalidId`,
  `ReadLimitExceeded { blob_id, max_encoded_bytes, actual_encoded_bytes }`,
  and `Corrupt { blob_id, detail }` variants. Downstream matches must handle
  canonical-ID, bounded-read, and integrity rejection. The new
  `BlobStore::get_with_encoded_limit` method has a default implementation, so
  external stores remain source-compatible but should override it to reject
  before materializing an oversized payload.
- `RealtimeSessionFactory::attach_external_session` now takes
  `(&RealtimeExternalSessionTarget, &RealtimeSessionOpenConfig)` instead of
  separate identity and turning-mode arguments. `RealtimeSessionOpenConfig`
  struct literals must also initialize `user_content_identities`,
  `user_content_tombstones`, and `transcript_rewrite_generation`.
- `meerkat_core::LiveProjectionSnapshot` struct literals must initialize the
  new `user_content_identities`, `user_content_tombstones`, and
  `transcript_rewrite_generation` fields used by exact-retry and live rewrite
  guards.
- `WireLiveAdapterObservation::RealtimeTranscript.event` now uses the
  public-safe `WireRealtimeTranscriptEvent` Rust type instead of the internal
  `RealtimeTranscriptEvent`. Its serialized/schema shape preserves the public
  transcript variants, while internal byte-bearing user content is no longer
  representable on the public observation type.
- Generated SessionDocument public alphabets gained the realtime user-content
  identity lane: core `SessionDocumentInput` and `SessionDocumentEffect`, and
  kernel `Input`, `InputKind`, `Effect`, `EffectKind`, and `TransitionId`, all
  have new exhaustive variants. `RestoreRealtimeTranscriptState` input
  construction and `SessionDocumentMachineAuthority::restore_realtime_transcript_state`
  also require the four new user-content identity invariant booleans.
- Mob retirement now has an explicit durable start boundary. The exhaustive
  `MobEventKind` gained `MemberRetirementStarted`; generated
  `MobLifecycleJournalKind` gained the releasing, binding-preserving, and
  peer-only start variants; `RecoverRosterMemberRetirementStarted` now carries
  `generation`; and `ObserveMemberRetirementArchived` now carries `generation`
  plus `session_id`. The obsolete unjournaled `RetireMember` signal was removed;
  callers must use the canonical journaled `Retire` input.
- Generated MobMachine public alphabets gained typed runtime-consumer refusal
  closure: `ResolveRuntimeBindingRefusal`, `ResolveRuntimeIngressRefusal`, and
  `ResolveRuntimeRetireRefusal` inputs plus their corresponding classified
  effects. Downstream exhaustive matches over generated MobMachine input,
  effect, transition, or kernel kinds must handle these variants.
- `meerkat_rpc::TransportError` gained the exhaustive `FrameTooLarge`,
  `FrameAdmissionBackpressured`, `FrameProgressTimeout`, and `WriteTimeout`
  variants. JSONL framing, incremental process memory, minimum read progress,
  and outbound write lifetime are now explicitly bounded.
- `meerkat_rpc::RpcNotification::params` now uses `Box<serde_json::value::RawValue>`
  instead of `serde_json::Value`, and `RpcResponse` / `RpcNotification` now
  carry private process-admission ownership. Downstream Rust code must use
  `RpcResponse::{success,error,error_with_data}` and `RpcNotification::new`
  instead of struct literals; `RpcNotification::new` now returns
  `Result<RpcNotification, RpcOutboundAdmissionError>` so admission failure is
  explicit and cannot fabricate an unmetered fallback. Notification consumers can call
  `RpcNotification::params_value()` when an owned `Value` is required. This
  clean break prevents queued notifications and responses from retaining
  unmetered expanded JSON trees.
- The generated RPC/SDK `Schedule` response contract now matches the canonical
  flattened wire object: `ScheduleConfig` fields such as
  `planning_horizon_days`, `created_at_utc`, and `labels` live at the top
  level instead of under `config`. Public REST/RPC responses no longer expose
  private persisted `machine_state`. In Rust,
  `meerkat_contracts::ScheduleListResult.schedules` is now
  `Vec<meerkat_contracts::wire::Schedule>`,
  `ScheduleOccurrencesResult.occurrences` is now
  `Vec<meerkat_contracts::wire::Occurrence>`, and the REST
  `ScheduleListResponse` / `ScheduleOccurrencesResponse` types are aliases to
  those canonical wire results. Python and TypeScript schedule wrappers now
  reject malformed required fields, missing result arrays, and non-object
  top-level results as typed `INVALID_RESPONSE` failures instead of
  synthesizing empty/default records.

### Added

- Image input on the OpenAI Realtime live channel: `gpt-realtime-2` accepts
  still images, and the live surface now carries them end to end — a
  `LiveInputChunk::Image` (wire `kind: "image"`, base64 data) renders as a
  provider `conversation.item.create` `input_image` data URL. An image is
  staged CONTEXT for the turn that follows (server-VAD audio, explicit
  commit, or a text chunk); it never synthesizes a response by itself.
  `LiveChannelCapabilities.image_in` and the realtime capability
  projection's new `RealtimeInputKind::Image` follow the bound model's
  catalog `vision` fact, so clients feature-detect instead of
  try-and-catch; non-vision realtime bindings keep the documented scoped
  `image_input_not_implemented` rejection (channel survives). OpenAI image
  input accepts PNG and JPEG up to Meerkat's 20 MiB decoded safety
  ceiling, verifies that bytes match the declared MIME type, byte-budgets
  the adapter queue, and redacts image data from diagnostics. Image submissions
  require a non-empty session-scoped idempotency key of at most 128 bytes.
  Exact same-key/content retries do not resend provider input and reproduce the
  existing durable receipt; changed content fails closed as
  `image_input_idempotency_conflict`, and an image behind staged text/audio is
  rejected as `image_input_requires_commit`. Accepted images materialize in
  canonical history, persist through the blob store, and are hydrated for
  reconnect replay under a 40 MiB aggregate decoded-image ceiling (repeated
  blob references count per occurrence); hydration also re-verifies the
  content-addressed identity before sending bytes back to the provider.
  Clients receive a byte-free
  `user_content_committed` ordering receipt carrying the idempotency key after
  persistence. `live/send_input` success remains queue acceptance; later
  scoped failures arrive as `command_rejected`, and only the receipt is the
  durable-success barrier. New wire vocabulary:
  `RealtimeImageChunk` + `RealtimeInputChunk::ImageChunk`.
  Smoke scenario 91 now verifies the selected image in canonical history,
  reopens the channel, and identifies it without resending it.
- The non-exhaustive `meerkat_live::LiveInputChunkDecodeError` gained
  `ImageEncodedTooLarge`, `ImageDecodedTooLarge`, and `ImageMimeTooLong`
  variants so callers can distinguish bounded image-ingress failures.
- Added `meerkat_core::image_content::RealtimeUserImageHydrationError` with
  typed blob-store, cumulative-budget, invalid-base64, and content-address
  mismatch failures. Realtime reconnect rejects a blob response whose returned
  identity or hydrated content does not match the durable image reference.
- Live image overload and transport routing are explicit scoped rejections:
  `image_input_backpressured` bounds queued/pending bytes, while WebRTC data
  channels return `image_input_transport_unsupported` and direct callers to
  JSON-RPC `live/send_input` for the image control plane.
- Direct live WebSocket ingress now has a 2 MiB aggregate/per-frame ceiling
  for text and negotiated raw PCM audio. Each of the 32 advertised upgraded
  connections reserves one full raw codec message before upgrade; listener
  sockets and partial HTTP headers have separate process-wide count, size,
  and deadline bounds, and token authority must resolve within 10 seconds.
  Inline images are not a direct-WebSocket input and must use the 64
  MiB-bounded JSON-RPC `live/send_input` control plane.

### Fixed

- Direct `RealtimeSession` image submissions now acquire the same process-wide
  conservative image-memory admission used by `OpenAiLiveAdapter`. The permit
  is acquired before base64 decode/provider send and remains attached through
  provider ACK and canonical-event completion; adapter-originated submissions
  transfer their existing permit without double charging.
- `live/open` now reserves the full cold image-hydration and provider-seed
  amplification window process-wide. Concurrent tiny opens fail fast under
  admission instead of multiplying decoded images and provider projections in
  memory.
- Realtime cold-open now reconciles any durable pending image anchor before
  constructing provider seed history. A verified post-ACK/post-put anchor is
  committed and hydrated into the reopened provider session; an invalid
  anchor is cleared, and inconclusive verification fails the open closed.
  Recovery can no longer defer the old anchor until a different new image ACK
  and thereby claim canonical content that the reconstructed provider never
  saw.
- Member retirement is now crash-safe across routed-runtime refusal, archive
  failure, and final-event append failure. A durable generation-bound start
  event retains the roster and exact pending session, cold replay restores the
  Retiring authority (including stopped-era and duplicate replay), and
  `MemberRetired` publishes only after routed retirement and critical cleanup.
- Routed Mob runtime refusals now close through generated, effect-specific
  feedback instead of collapsing binding, ingress, and retirement failures into
  one restore terminal. Binding refusal degrades only the owning member,
  ingress refusal rejects only the addressed work item, and retirement refusal
  remains retryable without discarding unrelated queued effects.
- Re-archiving an Archived session after a process restart now observes durable
  runtime lifecycle residue instead of relying only on the empty in-memory
  registry. Non-terminal residue converges to Retired, while terminal Destroyed
  state is correctly treated as quiescent rather than retried through an
  impossible Retire transition.
- The embedded `mob-communication` declaration and body are now owned by
  `meerkat-comms`, alongside the capability and tools whose operating policy
  they describe. The facade's `skills` feature mechanically links that owner
  even when the `comms` tool surface is disabled, preserving durable builtin
  `SkillKey` resolution across binaries without making the facade a semantic
  skill author.
- Create-session model resolution is now one typed facade policy shared by CLI,
  RPC, REST, and MCP. Explicit model/provider intent, inherited auth-binding
  defaults, configured global/per-provider defaults, and catalog fallback use
  one precedence ladder; malformed or provider-mismatched bindings fail closed
  and retain owner-stamped realm provenance. Nonempty `agent.model` values are
  preserved verbatim, including supported `claude-opus-4-7` pins, instead of
  being reinterpreted through a frozen legacy-default list. Malformed
  server-owned registry, binding, or default configuration is reported as a
  REST configuration error and an RPC/MCP internal error; caller-owned
  model/provider mistakes and unknown named realms or bindings remain request
  or invalid-params errors.

## [0.7.26] - 2026-07-09

Meerkat 0.7.26 closes the field bugs reported against 0.7.25's first week:
the torn-shutdown save wedge on classic (non-incremental) session stores,
the cold-revival re-bind rejection that broke identity-first member
revival, the misleading "session not found" laundering in front of it, and
the whole-mob blast radius of a single member's composition-dispatch
rejection. It also documents the 0.7.25 storage-layout migration and the
durable runtime_state vocabulary, and hardens the release pipeline's
Homebrew tap update against assets-only backfills of old tags.

### Fixed

- Cold revival of a stopped session re-binds under its fresh registration
  epoch (field, HomeCore on 0.7.24/0.7.25 identity-first gateways: member
  revival failed terminally with "session not found in runtime adapter
  after registration" / "DSL rejected PrepareBindings: GuardRejected
  { phase: Attached }"). The 0.7.24 revival arcs preserved the runtime
  binding tuple — but that tuple is epoch-scoped: phase Stopped proves the
  bound epoch's executor exited, and a cold revival registers under a
  freshly minted epoch, so every `PrepareBindings` arm (all guarded
  absent-or-same) rejected the re-bind by construction. Warm revivals
  (same process, same epoch) passed, which is why the 0.7.24 class tests
  were green. The revival arcs (`RegisterSessionResumesStopped`,
  `EnsureSessionWithExecutorStopped`) now clear the dead epoch's binding
  tuple while preserving session identity and hydrated LLM/capability
  state; the next `PrepareBindings` / mob `RequestRuntimeBinding` binds the
  new epoch. Red-verified with a torn-shutdown cold-restart class test.
- `MeerkatMachine::prepare_local_session_bindings` no longer launders every
  preparation failure into "session not found in runtime adapter after
  registration": typed machine rejections (e.g. a `PrepareBindings` guard)
  now surface verbatim as `RuntimeBindingsError::PrepareFailed` — the
  misleading not-found string cost the field a debugging cycle.
- One member's typed composition-dispatch rejection no longer terminates
  the whole mob actor task ("composition dispatch failed; terminating mob
  actor task" killed every healthy sibling). Every routed effect carries
  its target bridge session, so a session-scoped dispatch failure now
  degrades that member through the machine-owned restore-failure fact
  (`RecoverMemberRestoreFailure` + restore diagnostic) and drops the
  member's queued effects, while the actor keeps serving. Failures with no
  session scope, unknown sessions, and destroy-cleanup dispatch failures
  keep the fatal/bubble-up contract (incomplete destroy stays retryable).
  Adjacent gap fixed: the per-session routed-effect discard now also covers
  `RequestRuntimeIngress`.

- The torn-shutdown save wedge now also clears on CLASSIC (non-incremental)
  session stores (field, identity-first gateways on meerkat 0.7.25: resume
  saves rejected with "message count 165 is shorter than previously
  persisted 167" retried forever). 0.7.24's write-half rollback
  (`ResolveRuntimeProjectionRollback` → `RebuildToAuthority`) was consulted
  on the incremental head-canonical save path and the audited
  authoritative-projection path, but external `SessionStore` implementations
  route plain projection saves through the storage-normalization bridge —
  which re-threw `MonotonicityViolation` against a checkpointer-stamped
  ahead row on every retry. The bridge now consults the same machine-owned
  arbitration and converges via the CAS projection write; unstamped or
  content-forked rows keep failing closed.

## [0.7.25] - 2026-07-08

Meerkat 0.7.25 is the outstanding-asks sweep: O(delta) incremental session
persistence, mob revival completeness, restart-first-class status queries,
the structural peer-reply affordance, attention-binding uniqueness with
break-glass and GC, a metadata-only session read seam — plus the field fix
for the runtime-queue defer wedge and the pre-1.0 exact-pin versioning
policy with its release gate.

### Breaking

- `WorkGraphAttentionTurnOverlayError::MultipleActiveBindings` (facade) is
  removed. Multiple active bindings resolving to one session now arbitrate
  deterministically (newest binding wins) with a loud diagnostic instead of
  hard-failing every turn; new duplicates are prevented at the store.
- `meerkat_core::event_injector::SubscribableInjector` gained a required
  method `inject_with_interaction_id(...)`. Implementors outside this repo
  must provide it (delegating to `inject` and discarding the id is NOT
  faithful — the id must reach runtime admission).
- `meerkat_mob::WorkSpec` gained the public field
  `interaction_id: Option<InteractionId>`. Struct-literal constructors must
  set it (serde `default` keeps wire compatibility).
- `SqliteSessionStore` storage is now head-canonical: session heads and
  transcript strands replace the monolithic `session_json` blob for new
  writes, with a one-time transparent migration on open. External tools
  reading `sessions.session_json` via direct SQL will not see content for
  sessions written by 0.7.25+ binaries; use the store API (or `load_meta`
  for metadata-only reads). Migration note and the supported read paths are
  documented in `docs/reference/session-contracts.mdx` ("Session storage
  layout (0.7.25+)").
- `session/input_state` / `session/input_status` for persistent sessions
  without a live runtime registration now answer from durable state
  (`Ok`/`NotFound`) instead of `NotReady { reason: Destroyed }`. Pollers
  that treated `NotReady` as "try again later" get truthful terminal
  answers after a host restart.
- `meerkat_rpc::SessionRuntime::create_session` now takes
  `self: &Arc<Self>` (callers holding a bare `&SessionRuntime` must hold an
  `Arc`, which every runtime consumer already does). The gratuitous
  `&mut self` receivers on `set_config_runtime`, `set_realm_config_source`,
  `set_default_llm_client`, `set_agent_llm_client_decorator`, and
  `set_skill_identity_roots` are relaxed to `&self` (they were interior
  mutability all along).

### Fixed

- Agent-authored schedules fire in embedded `SessionRuntime` hosts (field,
  0.7.23: `meerkat_schedule_create` returned an id and the planner minted
  occurrences, but they stayed pending 12+ hours past due). The runtime
  binds the `meerkat_schedule_*` agent tools to its own schedule store at
  construction, but only the RPC router's startup path spawned the firing
  host — an embedder constructing `SessionRuntime` directly handed agents a
  store nothing drives. The runtime now arms its firing host itself the
  moment agent work can run (session creation and executor attach; atomic
  fast path, loud warn + retry on start failure), so
  tools-bound-but-undriven is no longer a representable topology.
  REST/MCP/CLI already start their hosts eagerly and are unaffected.

- A failed input batch no longer wedges the runtime queue when the machine's
  own failure realization resolves batch members out of the queued world
  (field, 0.7.23 crew gateway: a steer-shaped input's third failed stage
  attempt was abandoned by the machine's retry policy, the loop's
  whole-batch defer sweep then hit "DSL rejected DeferInputBehindBacklog:
  GuardRejected { phase: Attached }", dropped the pending wake, and
  stranded the backlog for ~13 minutes until an unrelated external wake).
  The defer sweep is now machine-owned-total:
  `DeferInputBehindBacklogAlreadyResolved` accepts tracked members that
  provably left `Queued` (max-attempts abandonment, boundary-applied
  members) as an explicit no-op, while a tracked-but-lane-less input still
  claiming `Queued` stays a loud rejection (projection corruption). The
  runtime loop's three batch-failure arms (apply failure, primitive
  rejection, turn-state preparation failure) now share one canonical
  backlog resolution: defer the failed batch and keep draining other queued
  work in the same wake, park only when nothing else is queued, and fail
  closed through the canonical executor-stop path — not a silent
  wake-dropping return — if the defer genuinely breaks.
- Dogma-audit fixes (#853 + review hardening): the embedded
  `mob-communication` skill is registered by the facade again (with a
  content pin so an empty or drifted skill body fails tests); the
  TypeScript and Python SDK model-catalog parsers fail closed on malformed
  catalogs instead of coalescing missing fields; the ownership-ledger
  `cfg(test)` detector recurses into `any()`/`all()` attribute groups and
  `feature = "test-support"`; Bazel test-support variant generation is
  seeded for `meerkat-mcp` alongside `meerkat-runtime`.

### Added

- `IncrementalSessionStore` (upstream ask 11): O(delta) session
  persistence. `SessionStore::as_incremental` exposes the optional trait;
  the SQLite store becomes head-canonical (`SessionHead` +
  `TranscriptStrandId` vocabulary, per-strand append tables, CAS'd head
  swings), `PersistentSessionService` writes deltas when the store offers
  them, and compaction rewrites the head so shrunken context is durable —
  saving a long session no longer serializes the full transcript on every
  turn.
- Mob member revival completeness (studio M2): reviving a stopped member
  recomposes its per-spawn external tools from the durable spawn spec
  (spawn/respawn/dispose threading plus cold-restore seeding), and
  `MemberSpawnedEvent` durably carries `effective_profile_override` so
  roster replay restores the member's effective profile instead of `None`.
- Restart-first-class terminal status (studio M4): `session/input_status`
  and `session/input_state` answer from durable runtime rows without
  requiring a live registration — a fresh host process can report terminal
  outcomes for work finished before the restart, with zero wire-shape
  changes.
- Interaction ids persist onto committed transcript messages (mobkit ask 15
  addendum): `SubscribableInjector::inject_with_interaction_id` and
  `WorkSpec.interaction_id` thread host-supplied interaction identity to
  runtime admission, so transcript messages join with the caller's live
  interaction frames.
- Structural peer-reply affordance (upstream ask 26): peer-message
  deliveries mint a `PeerReplyCapability` into the turn's dispatch context
  (key `comms.peer_reply`), and the pre-addressed `reply_to_peer` comms
  tool replies to the delivering peer without the agent restating
  addressing. Capability minting is batch-level (`TurnToolOverlay::compose`
  lifted to `meerkat-core` as the one canonical compose); errors are typed
  and fail closed. No wire changes.
- Metadata-only session read seam (upstream ask 24 clause 3):
  `SessionStore::load_meta` (SQL projection over stored metadata; ambiguity
  delegates to the full canonical load) and
  `MobSessionService::load_persisted_session_metadata` — list/status paths
  read session metadata without deserializing full transcripts.
- `semver-breaks` release-preflight gate (studio M3): cargo-semver-checks
  runs against the last published crates.io baselines; detected public-API
  breaks fail the release unless declared under a `### Breaking` changelog
  heading. The exact-pin policy in this file's header is the other half of
  the contract.
- Attention-binding uniqueness is now service/store-owned (mobkit ask 25):
  at most one ACTIVE attention binding per target, enforced transactionally
  inside the same store write that mints or revives a binding (`create_goal`,
  reassignment, `resume_attention`), with a typed `Conflict` naming the
  occupant binding. Host-layer admission guards demote to defense-in-depth.
- Break-glass host-plane attention reassignment (mobkit ask 23, reframed):
  `WorkGraphService::break_glass_reassign_attention` moves any non-terminal
  binding regardless of mode-derived authority, for the one state the graph
  cannot heal agent-natively (a binding stuck on a wedged/retired agent with
  no coordinator holding authority). Mandatory principal + reason are
  recorded in the workgraph event stream and a WARN log. Host API only —
  never exposed on the agent tool surface or wire catalogs; the agent-plane
  mode restriction is untouched.
- Terminal attention-binding GC (mobkit ask 24):
  `WorkGraphService::prune_terminal_attention` deletes superseded/stopped
  binding rows (optionally bounded by `updated_before`); the event stream
  keeps the audit history.

### Changed

- SQLite attention queries push realm/namespace/status/target filters into
  SQL over new indexed `status`/`target_key` columns (mobkit ask 24) instead
  of decoding every row in the store. The columns are added and backfilled
  by an idempotent open-time migration; all readers are NULL-tolerant, so
  stores shared with older binaries stay correct.
- Multiple active attention bindings resolving to one session no longer
  hard-fail every turn (`MultipleActiveBindings` removed): the turn overlay
  arbitrates deterministically — newest binding wins — with a loud
  diagnostic, mirroring the newest-session arbitration. Reachable only via
  legacy rows, mixed-version writers, or cross-kind targets; new duplicates
  are prevented at the store.
- `make machine-verify` and the pre-push machine gate now route through the
  canonical TLC lane (`xtask/tests/machine_verify_all_tlc_test.sh`), which
  owns the documented over-budget composition skips (`meerkat_mob_seam` /
  `adaptive_mob_bundle` full ci.cfg sweeps) and the bounded adaptive witness
  proof. The previous bare `machine-verify --all` form ran the full mob-seam
  state-space sweep, which fits no local or pre-push budget; the unbounded
  sweep remains available on demand as `make machine-verify-full`.

## [0.7.24] - 2026-07-08

Meerkat 0.7.24 closes the 0.7.19–0.7.23 resume-strand class at its root:
the machine now owns revival of stopped sessions, and cold resume
reconciles a stale runtime snapshot against the durable store head. This
is the release for downstream pins chasing broken resumes
("guard rejected transition from phase Stopped"), stranded disposals, or
permanent save rejections after a torn shutdown.

### Fixed

- The Stopped phase is no longer absorbing for resume — the root cause of
  the entire 0.7.19–0.7.23 resume-strand class (field: "guard rejected
  transition from phase Stopped for input::PublishLocalEndpoint", archive
  NotFound strands, disposal escalations, wedged mob members retrying
  forever). The machine now owns revival at the canonical resume seams:
  `RegisterSession` on a same-session Stopped machine re-admits it to Idle
  preserving identity, runtime bindings, and hydrated LLM state
  (`RegisterSessionResumesStopped`); `EnsureSessionWithExecutor` on Stopped
  re-admits to Attached and grants the active executor claim; `Retire`
  admits Stopped so disposal of an executor-stopped session is a machine
  transition (the mob durable-retire helper's shell phase probes are gone).
  Both revival arms refuse while an unregister drain window is open, and a
  machine-emitted `Recover` notice keys a durable lifecycle persist so a
  revived session is never left durably `Stopped` for cross-process
  readers. The accumulated per-input Stopped-tolerance arms
  (`HydrateSessionLlmStateStopped`, `PrepareBindingsStopped`,
  `PublishCommittedVisibleSetStopped`, `SetSilentIntentsStopped`, and the
  Stopped admissions of `SetModelRoutingBaseline`/`StagePersistentFilter`/
  `RequestDeferredTools`) are deleted outright: post-revival the resume
  build never runs at Stopped, so a build input reaching a Stopped machine
  is a loud typed rejection, never a silent self-loop.
- Cold resume no longer wedges permanently when the committed runtime
  session snapshot froze as a stale strict prefix of the durable store head
  (a completed turn's boundary save landed before the snapshot recommitted
  under a torn shutdown; every subsequent save then tripped the append-only
  guard forever). The `SessionDocumentMachine` owns the read-source verdict
  over three typed observations — the head provably extends the snapshot
  (the save guard's own continuity proof), the head row's intra-turn
  checkpoint provenance stamp, and in-process liveness. A cold load of an
  unstamped continuity-proven extension serves the store head; stamped
  ahead rows (uncommitted intra-turn residue), diverged rows, and live
  sessions keep the snapshot authoritative.

### Testing

- Stopped-phase revival class battery: the machine-schema exit-lattice pin
  (every arc out of Stopped is enumerated; the revival/teardown/disposal/
  destruction set is exact), resume-intent totality and
  no-tolerance-arms-return sweeps, runtime warm class tests (the exact
  field repro: peer-comms install succeeds post-revival, rejected at
  Stopped), the revival↔stop race pin (the historical error signature now
  only means a raced stop), retire-from-Stopped, drain-window refusal in
  both entry orders, and the mob stopped-member disposal sibling of the
  ask-21d regression. Red-verified against the pre-fix machine.
- Stale-snapshot read-source pins: cold strict-prefix defers to the head
  (red-verified against the old unconditional snapshot preference),
  checkpoint-stamped and diverged heads stay snapshot-served, live sessions
  unaffected; the former fail-closed-resume expectation — which WAS the
  field wedge — now pins a successful resume.

### Added

- `rkat mob host` — the mob member-host daemon (multi-host mobs phase 2):
  host acceptor with identity demux, one-time bootstrap-token bind ceremony
  (`BindHost`/`RebindHost` served through the generated
  `MobHostBindingAuthority`), 0600 host binding descriptor, realm-durable
  `runtime_mob_host_bindings` rows with recover-on-restart, schedule host
  with typed mob-target refusal, and an optional (inert until remote live
  channels ship) live-ws listener. `--isolated` is a typed startup
  rejection; `[mob_host]` config block added with flag-over-file precedence.

### Changed

- `rkat-rpc --live-ws` now enforces the same conservative TCP bind policy
  as `--tcp` (DL7): a non-loopback live-ws bind without `--allow-remote` is
  a typed startup error. Previously such binds were silently accepted.

## [0.7.23] - 2026-07-08

Meerkat 0.7.23 completes the never-run-member disposal arc for
identity-first workers (upstream ask 21d), ships the end-to-end WorkGraph
goals and attention wiring, and folds in the fixes from the pre-release
adversarial review of that wiring.

### Added

- WorkGraph goals and attention are wired end to end (#850): goal creation
  with attention bindings, machine-backed CAS reassignment on the
  attention-scoped tool surface (with a server-injected authority witness —
  callers cannot forge projections), per-turn attention overlays injected on
  every runtime surface, escalate-only completion-policy transitions guarded
  in the machine lattice, SQLite workgraph store hardening
  (busy_timeout/FULL synchronous/IMMEDIATE transactions on all mutating
  paths), and a fail-closed public MCP split for workgraph tools.

### Fixed

- Identity-first mob workers no longer strand in archive-NotFound during
  disposal (upstream ask 21d, #849): when the archive authority reports
  NotFound for a session whose runtime is in the terminal Retired phase
  while still registered, disposal completes (durable retire + runtime
  unregister) instead of escalating. Stopped runtimes are deliberately
  excluded — they are recoverable.
- Scheduled prompt delivery no longer snapshots the WorkGraph attention
  projection at enqueue time (facade schedule host and CLI schedule host).
  A queued prompt behind a running turn that mutated its work item carried a
  projection that failed exact-currency validation at apply, deterministically
  failing the scheduled delivery. The runtime executors inject a fresh
  projection at apply time — the single canonical injection point — so the
  enqueue-time composition was removed outright.
- WorkGraph attention overlays now arbitrate newest-session-wins on the
  canonical apply-time injection path. The arbitration guard previously ran
  only on the (removed) enqueue-time composition path, so during mob member
  respawn overlap both the old and new session matched the same owner-key
  binding and both received the attention overlay. The session-listing
  arbitration is now threaded through every injection surface (CLI, REST,
  RPC, MCP, mob provisioner, runtime executors); a session not yet visible in
  the listing (mid-creation) is treated as the newest carrier of its labels.

### Testing

- Pinned the apply-time attention injection on `CliRuntimeExecutor` and the
  newest-session arbitration semantics (overlap denial + mid-creation allow).
- Pinned the non-wasm fail-closed factory contract: WorkGraph enabled without
  a supplied dispatcher refuses to build with the typed `Config` error.
- Pinned the SQLite UNIQUE-violation → typed `Conflict` mapping for duplicate
  work item and attention binding inserts.

## [0.7.22] - 2026-07-07

Meerkat 0.7.22 fixes the runtime-loop self-deadlock that was the root
producer of the never-run-member disposal failures (upstream asks 20, 21,
21b) and, on 0.7.21, wedged entire mobs (ask 21c). This is the release for
downstream pins.

### Fixed

- Runtime-loop stop paths never run executor cleanup under the session
  mutation gate (upstream ask 21c, P0). The loop task acquired the
  per-session gate for terminal/effect handling and, still holding it,
  entered stop realization — whose cleanup re-enters the machine (the mob
  executor unregisters its session), and unregister's first await is the
  same non-reentrant gate: the task parked forever, leaking a permanently
  registered session (the state asks 20/21/21b kept hitting) and, on
  0.7.21, deadlocking the whole single-task mob actor behind the gate.
  Stops are now guard-free by construction: the effect drain surfaces stop
  effects instead of applying them (callers apply after releasing their
  authority guard), every stop call site drops its live guard first, and
  `retire_runtime_control_plane` acquires the gate with a 30s bound and a
  typed error naming the deadlock class so future regressions fast-fail
  instead of wedging mobs. Defensive class tests drive a machine-re-entrant
  executor through three distinct stop entry paths under hard timeouts —
  red-verified to deadlock on the unfixed code.
- With unregister completing, never-run member disposal converges through
  the 0.7.20/0.7.21 archive arms instead of manufacturing the
  permanently-registered state.

## [0.7.21] - 2026-07-07

Meerkat 0.7.21 fixes the 0.7.19/0.7.20 external-tool poison regression
(every MCP tool call failing after the session bind) and makes archive
retries converge for the last never-run-member retiring strand. It also
supersedes 0.7.20, whose registry packages published but whose Windows
GitHub-release assets were blocked by a runner disk flake — install
0.7.21.

### Fixed

- External tool servers staged/connected before the session-authority bind
  no longer poison the tool surface (meerkat-studio P0 regression on
  0.7.19/0.7.20). 0.7.19's composite bind forwarding delivered the session
  bind to nested MCP adapters for the first time, and the adapter refused
  pre-bind surface facts with a permanently poisoned handle — every
  subsequent call failed with "surface is poisoned … refusing to replay
  handwritten snapshot". The pre-bind state is machine-validated state on
  the construction-time authority, not a handwritten snapshot: the bind
  now re-derives every fact through generated inputs on the session
  authority (the sibling pattern to the MCP lifecycle bind's
  connect-pending seed), and only a genuine machine refusal still poisons
  fail-closed.
- Archive retries converge for the partial state left by a failed runtime
  retire (upstream ask 21b): an Archived document with a still-registered
  runtime used to resolve AlreadyArchived → NotFound on every retry,
  leaving the runtime registered forever (never-run mob-plane members
  stranded in `retiring`). The SessionDocumentMachine now owns the
  convergence — the re-archive completes the retire (no document rewrite)
  and reports success; quiescent duplicates keep the public NotFound
  contract. The REST archive retry that completes retained mob cleanup now
  returns 200 and removes the retry anchor instead of 404-with-a-leaked
  runtime.

## [0.7.20] - 2026-07-07

Meerkat 0.7.20 stops the one-shot occurrence-regeneration runaway (HomeCore:
223 misfired occurrences in ~2 minutes from one past-due one-shot) and
unbricks retire/respawn for created-but-never-run mob members.

### Fixed

- A one-shot (or any trigger) whose occurrence went terminal no longer
  regenerates occurrences unboundedly (upstream ask 22, P0). Root cause: the
  machine-owned planning cursor is millisecond precision while trigger due
  times carried nanoseconds — `truncate_ms(due) < due`, so the planner
  re-yielded an already-planned due every tick once nothing pending was left
  to dedupe against. The trigger engine now yields and compares
  ms-truncated timestamps (one fact, one representation); existing runaway
  stores heal in place on upgrade. Defense in depth: the
  ScheduleLifecycleMachine now owns planning monotonicity —
  `RecordPlanningWindow` refuses a cursor that does not strictly advance,
  so any future planner or representation bug converges as a visible
  per-tick refill fault instead of unbounded generation.
- Archiving a created-but-never-run session (durable record exists, runtime
  snapshot never committed) no longer rejects as a store-only projection
  (upstream ask 21, P1). Archive is a lifecycle terminal, not a projection
  promotion: the durable record is the complete truth for a never-run
  session, so mob members that never received a prompt can be
  retired/respawned instead of stranding in `retiring`. Control MUTATIONS
  (context appends, tool staging) still reject store-only projections, and
  already-archived sessions keep the typed NotFound contract.

### Security

- `crossbeam-epoch` 0.9.18 → 0.9.20 (RUSTSEC-2026-0204).

## [0.7.19] - 2026-07-06

Meerkat 0.7.19 hardens the schedule subsystem against poisoned durable rows
(HomeCore field incident: one bad row silently starved every schedule), makes
member cancellation and retire/respawn real for embedders (meerkat-studio P0s),
and gives library embedders first-class MCP wiring and a durable
run-reconciliation query.

### Fixed

- Mid-turn member cancellation no longer crashes the host process
  (meerkat-studio M1, P0). A boundary handle that re-entered
  `MeerkatMachine::cancel_after_boundary` from inside the machine's own
  dispatch — the shape meerkat's own blocked-method error text steered
  embedders into — recursed unboundedly until the tokio worker overflowed
  its stack (SIGABRT). The machine now owns a
  `boundary_cancel_dispatch_pending` fact: the first cancel in a turn window
  dispatches, repeats converge as typed no-ops, and a new turn re-arms
  dispatching. The misleading contract text on
  `PersistentSessionService::interrupt`/`cancel_after_boundary` and the
  `MobSessionService` cancel seams now states that implementations apply
  the cancel to the live agent and must never re-enter the machine.
- Retire/respawn no longer fails deterministically for session-owned mob
  members (meerkat-studio K1, P0). Members adopted from a host-owned session
  service (e.g. embedder console sessions via `ensure_member`) have no
  record in the mob archive authority; disposal escalated that to a fatal
  "authority returned NotFound for registered runtime session" while
  leaving the runtime registered, so every retry failed identically and
  the member could be neither respawned nor removed. Disposal now reads the
  typed ownership fact from the authority itself
  (`session_known_to_archive_authority`) before archiving: host-owned
  members retire their runtime session and release the binding; the
  fail-closed split-state escalation is preserved verbatim for
  authority-owned sessions.
- One poisoned schedule row no longer starves every schedule (upstream asks
  16–19, HomeCore field incident). The sqlite claim scan and driver tick are
  now per-row tolerant: rows that fail typed recovery, due classification,
  or per-schedule horizon refill are skipped as typed, attributable faults
  (`ScheduleStoreRowFault`, `ScheduleRefillFault` on `ScheduleTickReport`)
  while healthy neighbors keep claiming; the in-memory store mirrors the
  same per-row semantics. Store write failures inside the claim transaction
  still abort wholesale — a committed misfire receipt without its
  terminalized occurrence row can never split-commit.
- Schedule host ticks are no longer silent: tick failures and degraded
  (row-fault) ticks log an ERROR incident on change, a rate-limited WARN
  heartbeat while the same condition persists, and an INFO recovery line
  with the full outage length. The tracker uses the wasm-safe clock
  (`std::time::Instant` panics on wasm32-unknown-unknown and would have
  killed the browser schedule host on its first tick).
- Legacy `Deleted` schedule tombstones that persisted a planning cursor
  (written before the Delete transitions cleared it) heal at the
  durable-format parse boundary instead of failing every strict `list()`
  wholesale; each healed drop is logged. The claim scan is bounded in SQL
  (active schedules, live-phase occurrences, due/lease-expired only), so
  multi-GB terminal history never pays per-tick deserialization and a
  poisoned terminal row cannot poison the claim path; the live-phase list
  is compile-time ratcheted against `OccurrencePhase`.
- Strict schedule listing failures now name the poisoned row
  (`schedule row '<id>' failed typed recovery`) instead of an
  unattributable serialization error.
- Resuming a durably-stopped session no longer fails on the LLM capability
  hydration phase guard ("guard rejected transition from phase Stopped") —
  the hydrate transitions now admit Stopped and Retired sessions like the
  sibling baseline/visibility inputs already did, so restart-resume repair
  converges instead of retrying the same guard forever.

### Added

- Declarative MCP for library embedders (meerkat-studio M2):
  `SessionBuildOptions::mcp_servers` / `AgentBuildConfig::mcp_servers`
  materialize MCP stdio/HTTP servers into a session-owned router bound to
  the build mode's canonical external-tool surface authority — composing
  with builtins, no test-code ephemeral-handle incantation. Mob profiles
  gain the durable `tools.mcp_servers`, so revived members recompose their
  MCP tools from the profile instead of losing the in-process spawn
  overlay. `CompositeDispatcher` now forwards
  `bind_external_tool_surface_handle`/`bind_mcp_server_lifecycle_handle` to
  its external child, making session-time late binding reach nested MCP
  adapters. `meerkat::mcp::standalone_router()` is the supported
  constructor for raw-router hosts.
- Durable run reconciliation (meerkat-studio M4): the machine-owned
  idempotency binding is queryable read-only —
  `SessionServiceRuntimeExt::input_state_by_idempotency_key` returns the
  input's stored state (terminal outcome, resolving run id, boundary
  sequence) and survives host restart via session re-registration recovery.
  Exposed over JSON-RPC as `session/input_state` (by input id or
  idempotency key) with Python (`client.input_state`) and TypeScript
  (`client.inputState`) SDK wrappers, so embedders can delete hand-rolled
  run journals.
- Versioning and compatibility policy (meerkat-studio M3): documented in
  `docs/guides/cd-and-distribution.md` — pre-1.0 patch releases may change
  public APIs, embedders pin exact versions, the only supported crate
  combination is exact version parity, breaking changes land under a
  `### Breaking` changelog heading — plus a downstream compatibility
  matrix.

### Changed

- Repeat `cancel_after_boundary` calls while a dispatch is outstanding are
  accepted no-ops (previously each call re-dispatched and could saturate
  the runtime effect channel); the next turn re-arms dispatching.

## [0.7.18] - 2026-07-06

Meerkat 0.7.18 fixes the resume regression that stranded idle mob members
after prompt-drifting host upgrades (mobkit 0.7.23 field incident: 14 of 15
identities permanently refused resume).

### Fixed

- Chained turn-less resume refreshes no longer strand sessions. A member
  whose system prompt carries drifting parts (comms rosters, host context)
  gets a `resume-system-prompt-refresh` rewrite committed on every boot;
  with no turn in between, the transcript history graph retains several
  chained refresh commits, and the rewrite-chain walk failed closed on the
  next turn's run-boundary commit ("incoming append-only save would change
  retained transcript revision graph"): it selected an OLDER refresh commit
  under the system-refresh equivalence, walked forward onto the revision
  the cursor had already reached, and aborted as a cycle — rejecting a
  valid plain-append continuation. Reproduced end-to-end against stores
  seeded by real meerkat 0.7.13/0.7.14/0.7.15 binaries. The walk now
  proves continuity in authority order per iteration: exact graph edge
  first (real rewrite commits stay on the audited persistence chain), then
  the plain append continuation (with the leading-System-refresh
  equivalence still unprovable there, so unaudited System swaps keep
  failing), and only then the fuzzy refresh-equivalence edge; both
  selection scans skip already-visited revisions. Sessions stranded by
  this bug recover on their next resume — no data was lost (the durable
  transcripts were preserved the whole time).
- Crafted-state hardening from the adversarial review of the fix: the
  run-boundary rewrite branch re-validates the incoming transcript history
  state; the non-empty-chain arm validates every retained commit's
  recorded bodies (mirroring the empty-chain arm); and the revision
  ancestry walk is bounded so cyclic revision-parent metadata fails closed
  instead of hanging the boundary commit.


## [0.7.17] - 2026-07-05

Meerkat 0.7.17 fixes the machine-catalog compile blowup at its root and
supersedes 0.7.16, whose registry packages published but whose GitHub
release assets were blocked by the Windows binary lane. (Same shape as
0.7.15 superseding 0.7.14 — install 0.7.17.)

### Fixed

- The generated machine catalog no longer blows up rustc. The DSL
  `schema()` emission produced one enormous function per machine, driving
  rustc/LLVM to tens of GB of peak memory and deeply recursive opt-3
  passes — the root cause of every Windows release-lane failure from
  v0.7.14 through v0.7.16 (LLVM out-of-memory on `meerkat-mcp`,
  `0xc0000409` stack overruns on `meerkat-runtime` and
  `meerkat-machine-schema`) and of multi-hour local release builds. The
  emission is now chunked so the catalog compiles with sane memory on
  every platform (#835).

### Changed

- Windows release lane: with the generator fixed, every opt-level pin
  that papered over the blowup is removed (`meerkat-machine-schema`'s
  release pin from #832 in #835; the lane-local `meerkat-runtime` /
  `meerkat-mcp` pins from #834 in #836) — release binaries ship full
  codegen again on every platform. The lane keeps a two-job parallelism
  cap as commit-capacity headroom on the 16GB no-overcommit runner
  (#833, #836), alongside the pagefile expansion (#831).

## [0.7.16] - 2026-07-05

Meerkat 0.7.16 closes all six rows of the 2026-07-04 dogma audit — most
visibly, JSONL realms gain durable runtime authority and OpenAI live channels
resolve credentials per-open from the session's own auth binding — and moves
Windows release binaries to GitHub-hosted runners. Every row was
adversarially re-verified against HEAD before the fix, and the combined diff
went through a second adversarial review whose confirmed findings are all
resolved in this release.

### Fixed

- OpenAI live channels authenticate with the session's credential, not a
  process-default one. `rkat-rpc` used to resolve the default OpenAI binding
  once at startup and bake that secret into every realtime socket, so a
  session with an explicit `auth_binding` had live admission scoped to one
  identity while the provider socket authenticated as another. Realtime
  credentials now resolve per-open from the session's `SessionLlmIdentity`
  (owning-realm binding provenance, live config store — never a stale startup
  clone) through a new registry-owned
  `ProviderRuntime::build_realtime_session_factory` seam that carries the
  same fail-closed backend/auth gating as the text path (ChatGPT-backend,
  Azure, authorizer-auth, and custom-base-url bindings are rejected with the
  typed errors instead of opening a mis-keyed socket; the gate matrix has one
  shared owner). The facade never extracts secrets;
  `attach_external_session` carries the session identity; startup performs a
  wiring preflight only, so explicit-binding-only configs boot.
- JSONL realms run on durable runtime authority. The jsonl persistence
  bundle used to carry no runtime store, silently degrading every
  runtime-backed surface to an in-memory control plane (fresh ops epoch per
  restart: queued inputs, run-boundary receipts, and admission state all
  evaporated). JSONL realms now mount a sqlite runtime companion
  (`runtime.sqlite3`, typed `RealmPaths::runtime_sqlite_path`) next to the
  JSONL session documents; `runtime_store` is non-optional across
  `PersistenceBundle` and `PersistentSessionService`, and every store-only
  compatibility branch is deleted. Cold-restart parity for jsonl realms
  (runtime snapshot, boundary receipts, queued-input recovery) is pinned by
  tests. A durable document carrying the Archived lifecycle terminal with no
  runtime lifecycle state (legacy store-only archives) reads as archived —
  terminal and non-resumable — instead of failing realm `list()` closed with
  an untyped error.
- Skill inventory shadowing is computed over canonical identity.
  `CompositeSkillSource` canonicalizes every descriptor through the
  `SourceIdentityRegistry` before computing active/shadowed status
  (canonical-host rule: only the source physically hosting the canonical key
  can be active; remapped-away copies list inactive naming the active host),
  so remap/merge lineage can no longer advertise duplicate active canonical
  skills in browse/introspection. Listing fails closed without a registry;
  the registry-less composite constructors are deleted; the engine never
  rewrites keys and fail-closed-resolves entries that carry no typed source
  identity, so inventory can never advertise a skill the load authority
  refuses.
- `memory_search` no longer advertises previous-session recall. The
  model-facing ToolDef, the embedded memory-retrieval skill, the crate docs,
  and the guides now state the session-scoped contract (recall from earlier
  in this session, including turns compacted away before a resume), matching
  the typed scope model and the HNSW session filter; the dead second
  guidance string (`usage_instructions`) is deleted and the contract is
  pinned by tests.
- Python and TypeScript mob member-status wrappers stopped dropping and
  fabricating wire facts: required `member_ref` is surfaced, missing
  `tokens_used`/`is_final` fail closed instead of coercing to `0`/`false`,
  `peer_connectivity` is parsed as the tagged tri-state (unknown tags and
  the legacy flat shape rejected) with non-negative-integer validation on
  counts and tokens, and present-but-malformed `kickoff` payloads fail
  closed — mirroring the Web SDK reference in both SDKs. The four
  `mob/member_status` grandfather entries are deleted from the signature
  parity baseline (cap ratcheted down and enforced).
- The WASM browser contract mirror is truthful and actually executed. The
  wasm-bindgen suite still asserted the pre-0.6.23 contract (in-band
  `status` strings, addressable archived state after `destroy_session`) and
  — deeper — lacked `run_in_browser`, so even the BuildBuddy chrome lane had
  been silently skipping it while exiting green. The suite is rewritten to
  the shipped contract (tagged `WirePromptInput`, typed `WireRunResult`
  assertions, fail-closed `invalid_session_handle` after destroy) and now
  genuinely runs in headless Chrome.

### Changed

- CI: `wasm-check` compiles and lints all wasm32 targets (`--all-targets`),
  and a new path-filtered `wasm-contract` job executes the browser contract
  suite via `wasm-pack test --headless --chrome` on every wasm-relevant
  change (unconditionally in nightly), closing the gate hole that let the
  stale mirror survive.
- Windows release binaries build on GitHub-hosted runners (the org pool has
  no self-hosted Windows RBE executors); Linux/macOS release binaries stay
  on the BuildBuddy lane. `meerkat-machine-schema` pins its release opt
  level to avoid the rustc-LLVM OOM in the Windows lane.

## [0.7.15] - 2026-07-04

Meerkat 0.7.15 fixes cold-restart transcript loss for hosts that re-send
explicit system prompts on resume (the SDK-gateway shape), with the
system-prompt reconciliation hardened by an adversarial review of its tail
verification and continuation-walker seams. It also carries the scoped
Windows release-lane ThinLTO fix. (0.7.14 published to all registries but its
GitHub release assets were blocked by offline Windows build executors; 0.7.15
supersedes it.)

### Fixed

- Cold-restart resume no longer loses the transcript when the host re-sends
  an explicit per-request system prompt (the SDK-gateway shape: member specs
  carry `SystemPromptOverride::Set` on every build). The factory build used
  to blind-replace the resumed session's leading System message with the
  re-assembled base prompt, discarding every runtime-applied system-context
  append (comms rosters, host context) and re-stamping the typed
  `mutation_kind` — so the resumed projection was no longer a continuation
  of the persisted transcript revision, the continuity preflight failed
  closed on the very first post-resume persist, the live session was
  discarded, and downstream hosts fell back to fresh empty spawns (silent
  history loss on every restart, including sessions freshly written by the
  same version). The final contract removes resume-time prompt reconciliation
  entirely: materialization restores every ordered System message
  byte-for-byte and never injects current host configuration. New System
  instructions enter only through an admitted turn boundary. The rewrite-chain
  continuation walker also no longer aborts as a
  cycle when a same-length rewrite commit sits exactly at the persisted
  head (`find_transcript_rewrite_commit_chain_extending_session` skips
  commits that cannot advance the cursor), which previously rejected the
  first post-resume turn after such a rewrite. Current regression coverage
  instead pins byte-exact ordered-System preservation across materialization,
  persistence round-trip, compaction, and cold restart.
- Historical resume-time prompt reconciliation was hardened against malformed
  context tails and untyped leading-System replacement. Meerkat 0.8.11 later
  removed that model entirely: resume is transcript-invisible and every new
  System is an ordinary admitted event.

## [0.7.14] - 2026-07-04

Meerkat 0.7.14 makes cold-restart resume survivable (content-addressed
transcript revisions + machine-owned resume projection authority) and lets
sqlite stores read legacy TEXT JSON rows carried in from external writers.

### Fixed

- Sqlite stores accept legacy TEXT JSON payload rows (upstream ask A). Meerkat
  writes JSON payload columns as BLOB, but SQLite affinity keeps whatever type
  a writer bound, so carried stores written by external hosts could hold the
  same UTF-8 JSON as TEXT — and one such row failed every
  `list()`/`get()`/claim with `Invalid column type Text`. All JSON payload
  reads in `meerkat-store` (schedule store including the claim-due JOIN,
  session store `session_json`/`metadata_json`, session index `meta_json`) now
  go through a typed dual-encoding read boundary (`Text | Blob` → UTF-8 JSON
  bytes). Writes stay canonical BLOB; rewritten rows normalize. Upgrade-carry
  tests degrade committed rows via `CAST` and assert every read path carries.
- Cold-restart resume no longer fails closed on re-stamped transcripts
  (upstream ask B, part 1). The transcript revision digest is now a CONTENT
  address: construction bookkeeping — `TranscriptMessageIdentity`
  (run/interaction ids, re-minted by every re-created runtime authority) and
  `created_at` timestamps — is erased from the digest form, so a resume that
  re-projects the same conversation digests to the same revision as the
  persisted row instead of tripping `append_only_save_guard`
  (`TranscriptContinuityViolation`) and discarding the live session. Typed
  semantic facts (`transcript_role`, `mutation_kind`, `render_metadata`,
  notice kinds/blocks) stay in the digest. Persisted rewrite graphs carrying
  pre-0.7.14 revision strings heal at the durable-format parse boundary:
  every retained revision body re-verifies under the legacy digest and
  re-derives to the content-addressed format (`TranscriptHistoryState` /
  `TranscriptRewriteRecord` deserialization); unverifiable strings are left
  for the validators to reject exactly as before. Hosts holding revision-id
  strings from a pre-0.7.14 process should re-list
  `session/transcript_revisions` after upgrading.
- Cold-restart resume converges an ahead-of-authority durable row instead of
  stranding the session (upstream ask B, part 2). Every runtime-backed turn
  has two non-atomic durable commit points — the intra-turn best-effort
  checkpointer writes the session-store row, the machine boundary commit
  writes the runtime-store snapshot — so a host kill between them (or an
  in-process lifecycle-commit failure that evicted the uncommitted live turn)
  left the row carrying turn content the machine never acknowledged. Every
  subsequent runtime-authoritative persist then rejected against the newer
  row (`MonotonicityViolation`), permanently stranding the session (mob
  members came back terminally `Broken`; only respawn recovered). The
  canonical `SessionDocumentMachine` now owns the projection-rollback
  disposition (`ResolveRuntimeProjectionRollback`): the intra-turn
  checkpointer stamps its rows with a typed provenance fact
  (`session_runtime_checkpoint_provenance_v1`, stripped by every
  boundary-following persist), and when an ahead-of-authority row both
  carries that stamp AND is a faithful continuation of the authority
  transcript — judged by the same run-boundary proof the save guard uses —
  the CAS projection write rebuilds the row onto committed truth. Rows
  without the system's own provenance stamp (out-of-band writers) and
  genuine content forks keep failing closed. The runtime authority stays
  singular — the row is never adopted as truth — and no user input is lost:
  the discarded tail's durably admitted input (durable-before-ack) is
  redelivered after restart and re-executes through a fresh
  machine-committed run. Regression suites cover mob revival after a kill
  between commit points (including input redelivery), compaction across
  restarts (flushed, shrink + post-restart compaction, and lagged durable
  row), stale out-of-band row divergence staying fail-closed, and
  bookkeeping-divergent persisted rows.

## [0.7.13] - 2026-07-03

Meerkat 0.7.13 completes the MobKit content-taint story (upstream asks 9+10)
and ships provider prompt-cache hints and Gemini video-by-URI support.

### Added

- Host-consumable content-taint surfaces (upstream ask 9, completing ask 5):
  - New `peer_content_ingested` agent event — a typed projection of committed
    inbound peer content (canonical peer identity, comms kind, request id, and
    the sender's signed `sender_taint` declaration), emitted for both queued
    and steered deliveries. Host taint trackers consume typed facts instead of
    parsing rendered peer-message text; covers peer requests and cross-process
    senders. Joins the wire event inventory across all three SDKs.
  - Per-member outbound taint declaration: the core `CommsRuntime` trait gains
    `set_outbound_content_taint` (typed `Unsupported` default), reachable per
    mob member via `MobHandle::declare_member_outbound_taint` /
    `MemberHandle::declare_outbound_taint`. The declaration installs on the
    member's own comms runtime, so every outbound content-bearing envelope
    carries it inside the signed region; external-bound members receive it
    over the supervisor bridge via the new
    `BridgeCommand::DeclareMemberOutboundTaint`. Declarations reset on
    respawn/reset (fresh-context taint semantics).
- Runtime-backed schedule hosts can register host runnables (upstream ask 10):
  `spawn_runtime_backed_schedule_host` / `_with_mobs` accept an optional
  `ScheduleRunnableHost`, so `HostRunnable` schedule targets dispatch through
  the runtime host's occurrence driver.
- Provider params now expose typed prompt-cache hints for OpenAI, Anthropic,
  and Gemini. OpenAI Responses requests still default to `store: false`, with
  explicit overrides for `store`, `prompt_cache_key`, and
  `prompt_cache_retention`; Anthropic can mark the stable system prefix for
  `cache_control`; Gemini can pass an explicit cached-content resource name.
- OpenAI Responses streaming can reuse stored response IDs as
  `previous_response_id` continuation hints when `store: true` is explicitly
  enabled, while preserving full local transcript replay as the fallback path.
- Gemini video inputs can now be passed by provider-readable URI as
  `VideoData::Uri` / `WireVideoData::Uri`. Vertex accepts `gs://` references
  directly. Gemini API clients use public or already-registered file URIs as
  `fileData`, and can register `gs://` references through the Files API before
  generation when Google bearer auth is available.

## [0.7.12] - 2026-07-02

Meerkat 0.7.12 lands the eight upstream asks from the MobKit agent-memory
initiative, plus a pre-existing schedule-store fix.

### Added

- Typed injected-context transcript class (`TranscriptUserRole::InjectedContext`):
  hosts attach ambient context as separate typed user-channel messages on every
  submit-work path (service/RPC/REST create+turn, runtime inputs, mob
  `WorkSpec`/`mob/turn_start`/`mob/submit_work`, supervisor-bridge delivery,
  Python/TS wrappers). Injected context and discarded compaction summaries are
  excluded from semantic-memory indexing; transcript rewrites may carry the
  role (compaction summaries stay runtime-mintable-only, rejected fail-closed).
- `MemoryStore` lifecycle: per-scope `drop_scope` delete/GC and paged
  `enumerate_scoped` (durable-id order, `source_range` overlap and
  `indexed_after` filters). `HnswMemoryStore` opens lazily — no more
  every-session re-embed on each agent build — with an in-place SQLite
  migration, transactional never-reused point-ID allocation, and mixed-version
  row healing.
- Host-supplied compaction curator (`CompactionCurator`): produces the summary
  instead of the LLM call (zero-LLM-cost compaction); curator failure is a
  typed `CompactionFailed` reason with no silent LLM fallback.
- `session/transcript_revisions`: transcript revision list (with head) on the
  JSON-RPC transcript family; restore now resolves the `current` selector.
- Comms content-taint channel: signed-when-present `content_taint` declaration
  on content-bearing envelopes, host-set outbound declaration with tri-state
  per-send override, receiver-side typed `sender_taint` on comms transcript
  notices. Hook payloads gain typed tool `provenance` and provider-native
  `server_tool_content` for synchronous dispatch-time classification.
- Call-level tool authorization: `SpawnMemberSpec.tool_access_policy` is now
  enforced end-to-end via a list-preserving execution gate (prompt-cache
  prefix unchanged; denials are ordinary tool errors). `Inherit` resolves to
  the parent's effective policy on persistent AND ephemeral backends, and
  agent-created schedules with helper targets inherit the creator's policy at
  creation.
- Host-runnable schedule targets (`target_kind: "host_runnable"`): library
  hosts register named runnables; occurrences flow through the normal
  occurrence lifecycle.

### Fixed

- Persisted Flow-target schedule rows were unreadable (raw `Box<RawValue>`
  params cannot deserialize through internally-tagged serde buffering); Flow
  params now use a canonicalizing carrier and old rows heal on read.
- Prior compaction summaries and injected-context messages no longer count
  toward the compactor's retained-turn budget; retained turns keep their
  preceding injected-context run.

## [0.7.11] - 2026-06-18

### Breaking

- Session and mob request types across REST, RPC, and MCP now carry
  `WireAuthBindingRef` instead of core `AuthBindingRef` values. Downstream Rust
  request literals must use the wire type.

### Fixed

- Closed auth-binding origin-provenance laundering across server surfaces.
- Reduced Windows release-build memory pressure so the `meerkat-mob` binary
  can be published without the prior rustc/LLVM out-of-memory failure.

## [0.7.10] - 2026-06-18

### Breaking

- RPC and REST mob-helper request structs now carry `WireAuthBindingRef` and
  no longer accept a client-owned `origin`. Downstream Rust literals and
  helper adapters must provide the wire shape.

### Fixed

- Closed auth-binding origin-provenance laundering in mob helpers and made
  the related surface-authority checks fail closed.

## [0.7.9] - 2026-06-18

### Breaking

- Config `max_tokens` fields changed from `u32` to `Option<u32>` with explicit
  resolution helpers. Realm configuration gained an explicit parent chain and
  global realm, and exhaustive matches over `RealmBackend` must handle
  `Memory`.
- Web builds now require the generated JavaScript glue emitted by
  `mob web build`; downstream packages cannot treat the raw WASM alone as a
  complete bundle.

### Added

- Added hierarchical realm-config inheritance with owning-realm provenance,
  explicit memory realm backends, typed provider cache hints, OpenAI stored
  response continuations, prompt `@file` expansion, and runnable web bundles.

### Fixed

- Fixed scheduled delivery into already-attached sessions, mob readiness wait
  starvation, and partial-feature races in realm and mob construction.

## [0.7.8] - 2026-06-17

### Breaking

- `PersistentRuntimeExecutor` and its construction, materialization,
  interrupt, peer-ingress, and schedule-host helpers gained a generic
  `SessionAgentBuilder` parameter. Downstream type annotations and helper
  signatures must supply or infer the builder type.

### Added

- Runtime-backed schedule hosts and executors can use custom session-agent
  builders instead of the concrete default builder.

## [0.7.7] - 2026-06-17

### Fixed

- Mob retirement now unregisters the runtime adapter before discarding the
  live session, fixing idle-member retire, respawn, and reset failures while
  preserving teardown ownership.

## [0.7.6] - 2026-06-17

### Breaking

- `VideoData` and `WireVideoData` gained the exhaustive `Uri` variant.
  Generated machine alphabets removed the dead
  `BindAdmissionRuntimeGrouping` family and added machine-owned execution
  control plans and effects; downstream exhaustive matches must be updated.

### Added

- Added Gemini provider-readable video URI input and preserved OpenAI image
  blocks returned from tool results.

### Changed

- Runtime queues and run commits now flow through sealed, machine-authorized
  execution plans instead of shell-owned dequeue and staging methods.

### Fixed

- Preserved mob member labels on session creation and closed the remaining
  execution-authority audit findings.

## [0.7.5] - 2026-06-15

### Changed

- Runtime ingress and execution control moved behind machine-owned typed run
  identities and terminality checks.

### Fixed

- Joining the comms-drain task during detach prevents revival from colliding
  with a still-active session identity.
- Closed ten fail-closed, single-owner, typed-contract, and terminality audit
  findings.

## [0.7.4] - 2026-06-15

### Breaking

- Generated MobMachine spawn alphabets replaced the single spawn transition
  family with `BeginSpawnExec`, `CommitSpawnMembership`,
  `CommitSpawnActivation`, and `AbortSpawnExec`. Downstream generated-state
  literals and exhaustive matches must handle the staged spawn phases.

### Changed

- Mob spawn now follows a machine-owned execution ladder with bounded
  finalization and explicit membership and activation commits.

### Fixed

- Closed spawn authority gaps and reduced the spawn execution stack so the
  production worker-stack budget is respected.

## [0.7.3] - 2026-06-14

Meerkat 0.7.3 is a follow-up to 0.7.2 fixing two mob-teardown regressions
(surfaced by the e2e-smoke mob lane, which is not in CI), an e2e-system lane
gap, and two confirmed dogma violations with immediate operational effect.

### Fixed

- **Bounded comms-drain teardown await** (#770) — `unregister_session`'s
  two-phase drain awaited the comms-drain task unbounded while the runtime-loop
  handle was grace-bounded. An external member (e.g. a TCP transport drain)
  whose task does not observe cooperative abort promptly could wedge teardown.
  The comms-drain await is now bounded by a grace window + abort, matching the
  loop handle. Regular CI-gated test added.
- **Idempotent `discard_live_session`** (#770) — the teardown drain quiesces
  the runtime loop, whose clean exit discards the live session; an explicit
  caller-side discard then raced it and returned `NotFound`, breaking
  same-process mob restart. `discard_live_session` is now idempotent
  (codifies the existing `Ok(()) | Err(NotFound) => Ok(())` caller idiom).
  Regular CI-gated test added.
- **`rkat auth refresh` for `external_chatgpt_tokens`** (#770) — refreshability
  was derived from a hand-maintained raw-string allowlist that omitted the
  `external_chatgpt_tokens` OAuth-login mode, so refresh wrongly reported a
  no-op ("credentials don't expire") and never exchanged the persisted token.
  It now branches on the typed auth-method → persisted-mode owner.
- **`@rkat/web` helper profile option** (#770) — `spawnHelper`/`forkHelper`
  serialized `profile_name`, which the WASM helper deserializer silently
  dropped, so the `profileName` option built helpers with the default profile.
  The web SDK now serializes the canonical `role_name`, matching the wire
  contract and the Python/TypeScript SDKs.
- **e2e-system `cli-resume` lane** — built rkat without `memory-store-session`,
  so the yolo `memory` tool degraded to Disable and the scenario's assertion
  failed; the scenario now builds with the memory feature.

### Notes

- Nine additional confirmed/partial dogma rows with no immediate operational
  effect are tracked for a follow-up.

## [0.7.2] - 2026-06-14

Meerkat 0.7.2 hardens machine authority over shell-driven inputs. A campaign to
discipline a confirmed inventory of "undisciplined shell input" sites makes the
inputs the shell feeds canonical machine flows pass through a machine-owned
lifecycle: teardown now drains each producer class under an explicit
machine-owned obligation window, and inputs that legitimately arrive after
teardown resolve as typed no-ops instead of runtime errors. "Not currently
expected" no longer means "runtime ERROR".

### Changed

- **Machine-owned teardown drain (Layer 1)** — `MeerkatMachine` session
  unregister, `AuthMachine` OAuth-flow release, and `MobMachine` kickoff
  teardown open a machine-owned draining window: a `Begin*` transition emits one
  drain-request effect per producer class, the shell quiesces those producers
  (abort + bounded await), feedback inputs discharge the obligations, and the
  final teardown transition is guarded on every obligation being closed. A
  teardown can no longer commit while one of its producer classes is still live.
- **Total post-teardown inputs (Layer 2)** — inputs that legitimately interleave
  after a teardown window (late completion acks, drained-producer callbacks,
  post-unregister lifecycle signals) resolve as machine-owned no-op transitions
  or typed-benign dispatch rather than surfacing as runtime errors.
- **Destroy/detach obligation pairing** — effect handoff protocols pair
  `EffectTeardownClass::DestroyRequest` with the `DetachBeforeDestroy`
  obligation, with seam-inventory audits enforcing drain completeness across
  compositions.

### Notes

- The token-gating pilot (Layer 3 of the campaign) is deferred to a follow-up
  release pending a deeper review of the token model.

## [0.7.1] - 2026-06-12

Meerkat 0.7.1 restores the core/provider boundary: the model catalog returns
to a real `meerkat-models` crate and `meerkat-core` is provider-free again,
guarded by a recurrence gate. It also ships adaptive flow mobpacks and a
one-liner for switching the default model.

### Added

- **`meerkat-models` is a real crate again** (#763) — all provider model data
  (catalog entries, per-provider capability tables, image-generation profiles,
  defaults) lives in `meerkat-models`, restoring the pre-B2 public API
  (`meerkat_models::{catalog, capabilities, profile}` and the historical
  function surface). Downstream consumers that imported `meerkat_models::*`
  before 0.7.0 work again unchanged.
- **Adaptive flow mobpacks** (#762) — mobpack flows adapt at runtime.
- **`rkat --default-model <model>`** (#763) — validates against the catalog
  and configured custom models, persists `agent.model` through the
  scope-resolved config (project/user/realm), and exits when given bare or
  applies before the given command. Documented in `rkat help`'s built-in
  reference, the CLI skill, and the docs site.

### Changed

- **BREAKING (Rust API): core takes the catalog as an explicit parameter**
  (#763) — `Config::validate`, `Config::model_registry`,
  `ModelRegistry::from_config*`, `MemoryConfigStore::new`, and
  `FileConfigStore::{new, global, project}` now accept a
  `meerkat_core::ModelCatalog` (pass `meerkat_models::canonical()`).
  `Provider::infer_from_model` is removed; use
  `meerkat_models::infer_provider`. meerkat-core embeds no provider data and
  must not depend on meerkat-models (enforced by a workspace gate).
- **Default model resolves through the catalog** (#763) — fresh configs no
  longer pin a model id; an empty `agent.model` resolves through the catalog's
  per-provider defaults and the global default (`gpt-5.5`) at session-creation
  time. Existing configs with a pinned model are unaffected.
- **Mob storage removal fails closed** (#763) — `mob_destroy` now surfaces
  storage-file removal failures (with post-remove verification) instead of
  reporting a clean destroy over a surviving store.
- **Dogma ledger fixes** (#764) — the SessionDocument recovery verdict owns
  runtime-projection quarantine (no shell-side fallback); WebRTC input
  handoff failures terminate with typed client error frames; Codemob built-in
  packs require machine-owned `main` flows (local comms completion removed).
- **Adaptive pack format hardened before first release** — packs with an
  `[adaptive]` section are stamped with a required `adaptive_flow` capability
  at build time, and hosts now enforce `[requires]` fail-closed (unknown or
  unsatisfied capabilities reject the pack with the supported set named).
  Hosts older than 0.7.1 ignore `[requires]` and will silently run adaptive
  packs as static packs — deploy adaptive packs to 0.7.1+ hosts only.
  `adaptive/policies.toml` is parsed and completeness-validated at pack time
  (garbage TOML or zero limits fail the build, not the run), and
  `adaptive/layer-decision.schema.json` is emitted by the builder from the
  canonical types (hand-rolled or stale schemas fail closed with a
  regenerate hint); inline layer profiles are now structurally validated.
- **BREAKING (flow semantics): step `output_format` defaults are
  schema-aware** — an omitted `output_format` resolves to `json` when the
  step declares an expected schema and `text` otherwise (previously always
  `json`, which made schema-less free-text steps fail as "malformed JSON
  output"). Definitions that wrote the field explicitly are unchanged;
  explicit `json` without a schema now earns a definition-time warning.
  Adaptive plan flows that relied on the old implicit json default must say
  `output_format = "json"` explicitly.
- **BREAKING (CLI behavior): legacy default-model healing removed** (#764) —
  a persisted `agent.model` naming an old built-in default (e.g.
  `claude-opus-4-7`) is now used as-is instead of being silently redirected
  to the current catalog default. An explicitly configured model always wins;
  use `rkat --default-model <id>` to change it. This deliberately supersedes
  the earlier by-design adjudication that kept healing (dogma resolution
  log #254): silently overriding a user's persisted choice was itself
  surface-owned magic.

## [0.7.0] - 2026-06-11

Meerkat 0.7.0 promotes the 0.7 line from generated-authority canary to the
stable release train. The stable delta focuses on dogma-driven correctness:
typed ownership at decision points, fail-closed terminality, generated-artifact
freshness gates, and surface/runtime alignment across the Rust crates, SDKs,
WASM, REST/RPC/MCP, and release tooling.

### Added

- **Contributor-visible generated-artifact gates** (#756) — REST surface
  alignment, schema freshness, SDK codegen freshness, RPC/REST wrapper parity,
  and machine drift checks now run in the default contributor CI lanes instead
  of only owner-only BuildBuddy paths.
- **Typed governance and remediation gates** (#760) — doctrine mirror checking,
  stricter RPC SDK wrapper alignment, and machine-backed evidence for previously
  overflagged dogma rows are now part of the workspace gates.
- **Provider/auth lease ownership** (#757) — MCP OAuth freshness is bound to an
  injected per-binding `AuthMachine` lease, while provider backend and auth
  defaults now flow through typed provider-matrix owners.

### Changed

- **Typed decision ownership across surfaces** (#754, #755, #757, #760) —
  string/JSON/Option/bool re-derivation was replaced with typed owners across
  LLM identity overrides, mob bindings, auth methods, model/provider defaults,
  structured-output retry policy, SDK status parsing, and image routing.
- **Generated contracts and SDK surfaces aligned** (#756) — RPC catalog types,
  REST OpenAPI paths, MCP tool rosters, docs tables, SDK wrappers, Web SDK
  runtime version checks, and workspace crate version inheritance are now bound
  to their source authorities by tests or rerun-and-diff gates.
- **Dependency floor refreshed for the 0.7 release** (#761) — updates include
  `tokio` 1.52.3, `uuid` 1.23.3, `tempfile` 3.27.0, `toml` 0.9,
  `toml_edit` 0.25, `strum` 0.28, `fs4` 0.13, and Bazel `platforms` 1.1.0,
  with the Bazel lockfile refreshed.

### Fixed

- **Terminal faults now propagate instead of laundering to success or empty
  defaults** (#758, #759) — malformed provider stream data, incomplete
  Anthropic EOF, ops-lifecycle invariant failures, poisoned completion cursors,
  config-read failures, durable projection writes, corrupt realm leases,
  pending-event lag, SDK registration errors, RPC result serialization failures,
  and session-store sidecar faults now retain typed fault information.
- **Mob/comms trust and routing correctness** (#754, #759, #760) — typed member
  bindings fixed the `mob.{id}`/`mob:{id}` persisted-session mismatch, comms
  namespace authority moved behind typed peers, supervisor trust rollback is now
  scope-correct and machine-owned, and bridge acknowledgements carry canonical
  truth.
- **Auth and surface correctness regressions** (#755, #759, #760) — direct-secret
  auth profile creation no longer rejects valid methods, scheduled sessions now
  honor configured structured-output retries, token writes acquire durable
  lifecycle markers before persisting bytes, and SDK/WASM parsing paths fail
  closed on unknown or malformed wire data.

## [0.7.0-alpha.0] - 2026-06-04

Meerkat 0.7.0-alpha.0 publishes the generated-authority canary for downstream
testing. It includes breaking wire and SDK-surface changes, so downstream
canaries should pin exact dependencies with `=0.7.0-alpha.0`.

### Changed

- **Supervisor bridge protocol bumped to V3 (breaking wire change)** — peer
  wiring commands (`WireMember`/`UnwireMember`) now carry the MobMachine peer
  overlay. The protocol negotiates cleanly across versions: V2 is still accepted
  for persisted authority records and pre-overlay peers, a V3 payload sent to a
  V2 binary is rejected with a typed `UnsupportedProtocolVersion` (not a
  deserialization error), and a V2 wiring payload received by a V3 binary is
  rejected with the same typed cause rather than silently failing. **Mixed-version
  distributed mobs cannot wire peers across the V2/V3 boundary — upgrade all
  hosts in a distributed mob together.**

### Removed

- **`reachability` / `last_unreachable_reason` removed from `PeerDirectoryEntry`
  (breaking wire change)** — peer reachability is no longer projected onto the
  wire directory entry. SDK consumers pinned to an earlier release that read
  these fields must update; the 0.x patch-compatibility predicate does not
  signal this removal.

### Deprecated / Compatibility

- **Pre-`status_info` event logs are not resumable** — session event logs written
  before the `ToolConfigChanged` event gained its structured `status_info` field
  (roughly v0.4–v0.5) that recorded only the legacy `status` string can no longer
  be replayed; resuming such a session fails fast with a clear error. Re-create
  the session. Logs written by current releases replay unchanged (regression-tested).

## [0.6.34] - 2026-06-03

Meerkat 0.6.34 adds interactive OAuth for streamable HTTP MCP servers and
promotes the Homebrew tap install path across the public docs.

### Added

- **Interactive OAuth for HTTP MCP servers** (#752) — adds `rkat mcp add` URL
  forms for Codex/Claude-compatible HTTP MCP servers, runtime-discovered OAuth
  with DCR, PKCE, refresh, stored and interactive modes, plus `rkat mcp login`.
- **Homebrew install documentation** (#753) — promotes
  `brew install lukacf/meerkat/rkat` in the README and quickstart, documents
  macOS/Linux tap support, and expands distribution docs for tap credentials and
  companion binaries.

## [0.6.33] - 2026-06-03

Meerkat 0.6.33 adds paired TCP comms for remote mob targets and hardens
supervisor bridge completion for signed remote sessions.

### Added

- **Paired TCP comms for remote mob targets** (#751) — adds `rkat run` comms
  flags for signed TCP listeners, external binding output, pairing password
  sources, and target metadata labels, plus password-proof pairing that installs
  trusted peers without sending the raw password.

### Fixed

- **Remote supervisor bridge completion** (#751) — returns bridge delivery
  completion through the comms drain and local bridge so external mob targets
  can complete supervisor requests reliably.
- **Remote mob shutdown cleanup** (#751) — stops the mob supervisor bridge on
  shutdown and normalizes idle peer `steer` admission so execution runs through
  the runtime loop as `queue`.

## [0.6.32] - 2026-06-02

Meerkat 0.6.32 fixes external supervisor bridge responses for remote mob
members after bind and wire trust changes.

### Fixed

- **External supervisor bridge response routing** (#750) — installs a scoped
  response route from the supervisor descriptor carried in bridge payloads,
  preserves existing supervisor trust after replies, routes bind-validation
  failures back to verified requesters, and keeps wire/unwire acknowledgements
  reliable across idempotent trust-projection races.

## [0.6.31] - 2026-06-02

Meerkat 0.6.31 fixes supervisor bridge response routing, compaction retry
cadence, and OpenAI streaming completion fallback behavior.

### Fixed

- **Supervisor bridge response routing** (#748) — installs an authenticated
  supervisor response route before authorized bridge replies, preferring
  private trusted-peer registration so supervisor routing does not leak through
  public peer discovery.
- **Compaction retry cadence** (#749) — persists failed compaction attempt
  boundaries and feeds them into the compaction cadence guard so sessions over
  threshold do not immediately retry the same failing compaction path.
- **OpenAI stream done fallback** (#749) — treats `data: [DONE]` as a terminal
  fallback only after streamed output, tool, or reasoning content has actually
  been observed.

## [0.6.30] - 2026-06-01

Meerkat 0.6.30 fixes production external TCP bridge replies and compacted
singleton retire/archive recovery.

### Fixed

- **External bridge reply routing** (#746) — repairs target-runtime trust
  projection before idempotent `BindMember` acknowledgements, and decodes the
  canonical `BridgeReply` envelope emitted by production `comms_drain` so
  external TCP member binds can complete the supervisor round trip.
- **Compacted singleton retire stranding** (#747) — lets runtime-backed session
  projections persist after a legitimate compaction when the durable store lags
  across the compaction boundary, avoiding `MonotonicityViolation` during
  archive.
- **Mob disposal hardening** (#747) — removes dead roster anchors even when
  `ArchiveSession` fails, so respawn/reset can recover without requiring a
  process restart.

## [0.6.29] - 2026-06-01

Meerkat 0.6.29 fixes remote external mob binding so TCP-backed members can
reply through a routable supervisor bridge.

### Fixed

- **Remote external bind routing** (#744) — gives mob supervisor bridge
  runtimes a signed TCP listener while preserving in-process descriptors for
  local members, and uses recipient-aware supervisor descriptors so TCP
  external members bind and reply through a routable supervisor address.
- **External TCP bind regression coverage** (#744) — adds coverage for a TCP
  external bind followed by a peer turn on the remote runtime.

## [0.6.28] - 2026-05-31

Meerkat 0.6.28 updates the Anthropic default catalog target to Claude Opus
4.8 and refreshes the surrounding examples, docs, and capability metadata.

### Changed

- **Anthropic Opus 4.8 default** (#743) — promotes the default Anthropic
  catalog model from `claude-opus-4-7` to `claude-opus-4-8`, retargets
  examples/tests/docs away from older Opus defaults, and keeps Opus 4.7 only
  where it remains an intentional legacy or fallback catalog entry.
- **Opus 4.8 capability metadata** (#743) — documents the current Opus 4.8 API
  behavior around adaptive thinking, `xhigh` effort, fast mode, beta/header
  handling, and unsupported non-default sampling knobs.

## [0.6.27] - 2026-05-28

Meerkat 0.6.27 hardens mob lifecycle cleanup when session-bound members fail
or pending spawns are cancelled.

### Fixed

- **Mob cleanup retry anchors** (#742) — keeps respawn and retire cleanup
  anchors retryable after archive/unregister failures, tracks pending-spawn
  cancellation cleanup as a retryable lifecycle operation, and fails lifecycle
  commands closed when archive cleanup is ambiguous instead of dropping roster
  truth early.

## [0.6.26] - 2026-05-27

Meerkat 0.6.26 fixes runtime-backed mob session continuity after transcript
rewrite and checkpoint projection failures.

### Fixed

- **Mob runtime session continuity** (#741) — preserves runtime authority
  across transcript rewrite projections, bridges storage-normalized append
  histories after media externalization, and keeps checkpoint quarantine from
  promoting unrelated store-only session projections.

## [0.6.25] - 2026-05-27

Meerkat 0.6.25 is a release packaging hotfix for Windows native assets.

### Fixed

- **Cross-platform JSONL session write locks** — replaces the Unix-only
  `nix::fcntl` JSONL session lock with a portable `fs4` file lock so hosted
  BuildBuddy Windows release builds compile the session store.

## [0.6.24] - 2026-05-27

Meerkat 0.6.24 adds same-session transcript rewrite/restore flows and
WorkGraph-backed goal attention surfaces.

### Added

- **Same-session transcript rewrites** (#739) — adds durable transcript
  rewrite revision history, restore support, canonical audit events, and
  REST/RPC/Rust/Python/TypeScript/Web SDK surfaces for rewrite, revision, and
  restore flows while preserving full assistant block traces.
- **WorkGraph goals and attention bindings** (#740) — adds high-level goal
  creation/status flows, attention-bound continuations, scoped WorkGraph tool
  authority, and REST/RPC/CLI/SDK observability for goal and attention state.

### Changed

- **WorkGraph machine authority reference** (#740) — documents WorkGraph
  attention as a canonical lifecycle machine and composition protocol, including
  scoped continuation, projection currentness, and goal completion policy
  behavior.

## [0.6.23] - 2026-05-22

Meerkat 0.6.23 is a mob runtime hotfix release for nonblocking observation,
profile-scoped auto-wiring, and active-turn steer admission.

### Fixed

- **Mob observation during active turns** (#738) — broad mob observation now
  reads actor-published snapshots instead of querying each mob actor, so
  `mob_list` and profile observation do not block behind in-flight member turns.
- **Profile-scoped auto-wire spawns** (#738) — profile-scoped legacy
  `spawn_member` calls can request `auto_wire_parent` without requiring full
  manage authority, while preserving owner/profile context for agent mob spawns.
- **Active-turn steer admission** (#738) — keeps operator and member steers
  runnable during active turns, routes staging through runtime-backed live
  boundary probes, keeps runtime steers transient, and rolls back staged live
  boundary state on commit failure.

### Changed

- **Release packaging Python selection** (#738) — release packaging checks now
  prefer Python 3.11 for more predictable local packaging validation.

## [0.6.22] - 2026-05-21

Meerkat 0.6.22 is a mob runtime hotfix release for turn-completion caller
semantics and actor-loop responsiveness.

### Fixed

- **TurnCompleted actor-loop responsiveness** (#737) — preserves
  `TurnCompleted` caller semantics while moving the runtime-completion wait out
  of the serialized mob actor command loop, allowing same-mob observation and
  mutation commands such as member listing and spawn to proceed while the
  turn-driven runtime call is still in flight.

## [0.6.21] - 2026-05-20

Meerkat 0.6.21 is a runtime hotfix release for interrupt-yielding live steer
injection during active runs.

### Fixed

- **Interrupt-yielding live steer injection** (#736) — adds a machine-owned
  live boundary path for accepted steer inputs during active runs, projecting
  them into pending system context so interrupt-yielding peer messages can be
  consumed at the next inner boundary without cancelling or replaying the
  active run.

## [0.6.20] - 2026-05-20

Meerkat 0.6.20 refreshes Gemini model guidance and fixes mob peer delivery
lifecycle behavior under backpressure.

### Changed

- **Gemini model and post-0.6.5 guidance refresh** (#734) — updates the
  featured Gemini text model to `gemini-3.5-flash` across the catalog, default
  provider model, tests, examples, public docs, and Meerkat skill guidance.

### Fixed

- **Classified inbox wake and peer delivery lifecycle** (#735) — fixes
  classified inbox capacity wake registration, wakes senders on close, and
  moves mob peer-message delivery out of the actor command loop so
  backpressured recipients do not wedge unrelated mob handle calls.

## [0.6.19] - 2026-05-19

Meerkat 0.6.19 is a runtime/session projection hotfix release for
runtime-committed session checkpointing.

### Fixed

- **Runtime-committed session projections** (#733) — checkpoints committed
  runtime snapshots back into the `SessionStore` projection after the machine
  commit succeeds, restoring per-turn projection saves for MobKit and
  UnifiedRuntime consumers without introducing a pre-commit split-brain.

## [0.6.18] - 2026-05-19

Meerkat 0.6.18 is a runtime/session reliability hotfix release for persistent
session checkpointing and lost mob session state.

### Fixed

- **Runtime-store session checkpointing** (#731) — fixes persistent session
  checkpointing through the runtime store so session state is written and
  recovered through the intended storage path.
- **Lost mob session status** (#732) — marks lost mob sessions as broken,
  preserving an explicit failed state instead of leaving unavailable sessions
  looking active or recoverable.

## [0.6.17] - 2026-05-18

Meerkat 0.6.17 improves mob spawn boundary configuration, task-workflow
guidance, and steer cancellation behavior.

### Added

- **Mob spawn boundary customization** (#728) — mob spawn flows can customize
  member spawn boundaries through the mob runtime path, with tests covering the
  builder, runtime handle, actor, and tool surfaces.
- **Mob task workflow guidance preload** (#729) — mob build profiles now preload
  task workflow guidance so spawned members receive the expected workflow context
  at construction time.

### Fixed

- **Steer admission boundary cancellation** (#730) — fixes cancellation behavior
  at the steer admission boundary and updates the machine contract/spec coverage
  around the cancellation path.

## [0.6.16] - 2026-05-18

Meerkat 0.6.16 is a hotfix release for mob peer wake behavior and
BuildBuddy-backed Windows release assets.

### Changed

- **Mob peer send and tool bridge parity** (#727) — aligns mob peer send
  behavior with the tool bridge path so peer-to-peer wake and delivery semantics
  stay consistent across mob surfaces.
- **Windows BuildBuddy release endpoint** (#726) — Windows release binary builds
  now route directly to the hosted King BuildBuddy endpoint instead of a mutable
  secret-backed endpoint, preventing stale private endpoint routing from
  breaking release asset publication.

### Fixed

- **Peer wake interrupt semantics** (#727) — fixes peer wake interruption so
  mob members are woken correctly when new peer traffic arrives.

## [0.6.15] - 2026-05-18

Meerkat 0.6.15 is a smoke-test hotfix release for mob lifecycle, image comms,
and release asset reliability.

### Changed

- **Release asset routing** (#717) — BuildBuddy release assets now cover
  validation plus Linux and macOS binaries, while the Windows release binary is
  always built on GitHub-hosted runners to avoid the known Windows remote
  execution input-tree failures.

### Fixed

- **E2E smoke mob lifecycle and image comms** (#718) — fixes the mob lifecycle
  and generated-image communication regressions found by e2e smoke testing,
  including runtime/session handling and MCP/tool-surface behavior needed for
  the smoke suite to pass again.

## [0.6.14] - 2026-05-17

Meerkat 0.6.14 adds batched local mob-member wiring, with public RPC, REST, MCP,
Python, and TypeScript SDK surfaces.

### Added

- **Batch mob topology materialization** (#715) — mobs can now wire dense local
  member graphs through `MobHandle::wire_members_batch(...)`, preserving
  MobMachine ownership of peer wiring truth while coalescing roster projection
  and event replay into a compact `MembersWiredBatch` event.
- **Batch wiring public surfaces** (#716) — `mob/wire_members_batch`,
  `POST /mob/{id}/wire-members-batch`, `meerkat_mob_wire_members_batch`,
  `MeerkatClient.mob_wire_members_batch(...)`, `Mob.wire_members_batch(...)`,
  `client.mobWireMembersBatch(...)`, and `mob.wireMembersBatch(...)` now expose
  the local-member batch wiring path across JSON-RPC, REST, MCP, Python, and
  TypeScript.

### Changed

- **Burst mob delivery backpressure** (#715) — runtime-originated in-process
  peer sends, lifecycle notifications, peer-retire fanout, mob command capacity,
  and deferred turn-event buffering now handle bursty dense mob workloads without
  converting full queues into semantic message loss.

## [0.6.13] - 2026-05-16

Meerkat 0.6.13 is a hotfix release for committed lifecycle and runtime effect
delivery under burst mob load.

### Fixed

- **Committed lifecycle backpressure** (#713) — mob lifecycle signal projection
  and committed runtime effect delivery now use backpressured sends instead of
  fail-fast bounded queue sends, preserving semantic lifecycle/effect facts
  when large mob fan-outs temporarily fill actor queues.
- **Rollback cleanup peer absence** (#713) — rollback compensation now treats
  typed `PeerNotFound` send errors as benign already-absent cleanup fallout
  while preserving other rollback failures.

## [0.6.12] - 2026-05-15

Meerkat 0.6.12 is a hotfix release for autonomous mob member capability
validation and active-turn retirement safety.

### Fixed

- **Autonomous member injector capability** (#712) — autonomous mob members
  must now expose `interaction_event_injector` before spawn/resume dispatch
  treats them as seated and active, surfacing missing injector support as typed
  `MissingMemberCapability` / `missing_member_capability` errors.
- **Mob retire active-turn teardown** (#711) — runtime retirement now waits for
  archive/unregister coordination and machine-observed quiescence before
  unregistering, avoiding races where an active turn could emit state
  transitions after the runtime was marked retired.

## [0.6.11] - 2026-05-15

Meerkat 0.6.11 is a hotfix release for turn-driven mob spawn prompt delivery.

### Fixed

- **Turn-driven spawn initial messages** (#710) — explicit initial messages for
  turn-driven mob spawns and respawns are now submitted through `SubmitWork`
  after the member is live, preserving caller intent instead of silently
  dropping the prompt during deferred session initialization.

## [0.6.10] - 2026-05-14

Meerkat 0.6.10 is a hotfix release for mob delegate authority inheritance.

### Fixed

- **Mob delegate authority inheritance** (#708) — child delegates can now
  inherit explicit profile-spawn authority within a mob without being granted
  full mob-management scope, preserving the typed authority boundary while
  allowing nested mob delegation workflows to continue.
- **Reasoning-only LLM completions** (#709) — assistant turns that only emit
  reasoning content or provider continuity metadata now satisfy core commit
  validation, and streamed reasoning deltas are finalized on successful
  completion so GPT-5.5-style silent reasoning responses are preserved.

## [0.6.9] - 2026-05-14

Meerkat 0.6.9 is a hotfix release for turn cancellation and multimodal mob
comms delivery, plus release-lane recovery improvements from the 0.6.8 rollout.

### Changed

- **Web SDK release recovery** — release workflows can now publish `@rkat/web`
  from an already-built `rkat-web-package` artifact, allowing npm publish
  retries without rebuilding the WebAssembly runtime.
- **Web SDK release build parallelism** — the Web SDK release package build no
  longer serializes Cargo work, reducing cold release-package build time on the
  GCP runner lane.

### Fixed

- **Steer-boundary cancellation** (#706) — fixed turn-state cancellation around
  steer boundaries so interrupted or redirected turns do not leave stale
  cancellation state in the runtime.
- **Multimodal comms notices** (#707) — fixed multimodal comms notice delivery
  across Anthropic, Gemini, OpenAI, and OpenAI-compatible providers so generated
  image notices and related mob comms survive provider-specific content
  projection.
- **Web SDK package/archive checks** — fixed release packaging checks that could
  fail a valid `@rkat/web` tarball under `pipefail`, and made npm publish use an
  absolute tarball path so local recovery artifacts are not interpreted as git
  package specs.

## [0.6.8] - 2026-05-13

Meerkat 0.6.8 is a hotfix release for long-context model compaction defaults
and Web SDK release publishing.

### Changed

- **Model-aware compaction defaults** (#704) — default session compaction now
  scales from the selected model catalog context window, using 80% of the
  resolved context window for known long-context models while preserving
  explicit custom thresholds and the conservative fallback for unknown models.
- **Web SDK release publishing** (#705) — release workflows now build the
  `@rkat/web` package once as an artifact and publish that tarball from the
  credentialed npm step, so Rust/Python/TypeScript publishing no longer waits on
  a cold WebAssembly rebuild.

### Fixed

- **OpenAI long-context sessions** (#704) — `gpt-5.5` and other large-context
  catalog models no longer compact prematurely at the old static 100k-token
  threshold.
- **Web SDK npm metadata** (#705) — normalized the `rkat-web-proxy` bin path so
  npm publish no longer auto-corrects the package metadata.

## [0.6.7] - 2026-05-13

Meerkat 0.6.7 is a feature-bearing release: WorkGraph,
Azure OpenAI, project-local CLI defaults, HTML artifacts, typed transcript
notices, capability companion skills, provider image/search improvements, and
several runtime/provider fixes.

### Added

- **WorkGraph subsystem** (#684, #688, #689, #694, #696, #699, #702) — added
  `meerkat-workgraph`, a realm-scoped durable commitment graph with work
  items, namespaces, labels, priorities, due/not-before/snooze gates, owners,
  claim leases, external references, evidence references, topology edges,
  revision/CAS checks, event history, ready-item derivation, snapshots, and the
  catalog-owned `WorkGraphLifecycleMachine`.
- **WorkGraph agent tools and host observability** (#684, #699, #702) — agents
  can mutate WorkGraph through `workgraph_create`, `workgraph_claim`,
  `workgraph_release`, `workgraph_update`, `workgraph_block`,
  `workgraph_close`, `workgraph_link`, and `workgraph_add_evidence`, while
  CLI/RPC/REST/SDK callers get read-only lookup through `list`, `show/get`,
  `ready`, `snapshot`, and `events` surfaces.
- **WorkGraph SDK support** (#684, #702) — Python and TypeScript clients now
  forward `enable_workgraph` / `enableWorkGraph` on session creation and expose
  typed read-only WorkGraph APIs for items, ready work, snapshots, and events.
- **Azure OpenAI backend support** (#700) — OpenAI provider bindings can now
  target Azure OpenAI through `backend_kind = "azure_openai"` and
  `auth_method = "azure_api_key"`, including Azure endpoint normalization,
  `api-key` auth, deployment-name model selection, image-generation deployment
  options, Azure image headers, and RKAT-prefixed env overrides.
- **HTML output mode** (#693) — `rkat run` now supports `--output html`,
  `--html`, `--browser`, `--open-in-browser`, `--html-template`, and
  `--html-template-file`, writing standalone HTML artifacts under the active
  realm's presentation output.
- **Project-local CLI realms** — CLI `run`, `run --resume`, and `session`
  commands now default to a workspace-derived `ws-...` realm under
  `<context-root>/.rkat/realms`, with `--context-root`, `--state-root`,
  `--realm`, and `--isolated` preserving explicit control.
- **Provider-native search/image delegation** (#682, #691) — Anthropic,
  Gemini, and OpenAI gained provider-native search fallback executors; OpenAI
  hosted image generation now supports `provider_params.reasoning_effort` and
  `provider_params.web_search` on the hosted `gpt-image-2` path.
- **Typed capability registry** — added `meerkat-capabilities` as the typed
  capability vocabulary and feature-owned registration point for sessions,
  streaming, structured output, hooks, builtins, shell, comms, memory,
  schedule, WorkGraph, session store, compaction, skills, MCP live, and live.
- **Companion workflow skills** (#694) — added/expanded embedded companion
  skills for WorkGraph, scheduling, skill discovery, and built-in utility
  workflows so tool descriptions can stay schema-oriented while operational
  guidance lives in skills.
- **Dogma and WorkGraph documentation** (#689, #694) — added public Meerkat
  dogma docs, the `meerkat-dogma-inquisition` skill, WorkGraph concept/guide/
  reference/example docs, realm docs, Azure OpenAI docs, HTML-output docs, and
  capability/skill reference updates.

### Changed

- **Default CLI model is OpenAI** — the CLI now defaults to OpenAI `gpt-5.5`,
  with provider-aware default-model resolution that follows auth binding
  provider/defaults, repairs legacy built-in defaults, and preserves explicit
  user model choices.
- **Runtime metadata is typed transcript data** (#686) — comms, external
  events, MCP changes, tool config, auth, background jobs, and runtime notices
  are persisted as typed `system_notice` blocks instead of being reclassified
  from `[SYSTEM NOTICE]` user-text prefixes. Provider-facing notice text is now
  a projection assembled for the model, not the stored operator prompt.
- **Mob task-board surface retired** — the old mob task-board model and
  `mob_tasks` profile knob were removed in favor of agent-facing mob tools and
  WorkGraph for durable cross-agent commitments.
- **BuildBuddy and release preflight hardened** — release preflight now runs
  `scripts/release-doctor`; BuildBuddy runs keep pending jobs queued; release
  backend selection, wasm-pack checks, npm/crates readiness, and Web SDK
  recovery timeouts were tightened.
- **OpenAI request behavior tightened** — image and Responses API calls now
  use `store: false` where appropriate, hosted image generation streams through
  the ChatGPT backend path, and OpenAI/Azure request construction is more
  explicit about backend-specific headers and URLs.
- **Skills identity tightened** — skill references now use structured
  `SkillKey { source_uuid, skill_name }` identity, legacy string refs are
  rejected on the wire, and large skill collections use a collection summary
  mode rather than injecting all content eagerly.

### Fixed

- **CLI session handle ambiguity** — short session handles now use the UUID
  tail, while resume accepts either prefix or tail and ambiguous matches report
  fuller realm/session refs, avoiding UUIDv7 timestamp-prefix collisions such
  as repeated `019e...` handles.
- **WorkGraph runtime gaps** (#688, #689, #702) — WorkGraph machine authority
  is durable, legacy refresh paths are hardened, invalid graph topology is
  rejected, and WorkGraph/delegate runtime wiring stays available across turns.
- **OpenAI image and stream failures** (#691, #692, #695) — OAuth-backed image
  generation, streamed image error events, HTTP failure bodies, compaction
  stream failures, `response.failed` handling, and image smoke timeout behavior
  now surface as structured provider/runtime failures instead of timeouts or
  parse surprises.
- **Anthropic/Claude OAuth edge cases** — fixed Anthropic OAuth scope refresh
  failures, Claude AI OAuth request envelopes, beta header composition, and
  rate-limit routing via the `x-app` header.
- **Deferred session and mob runtime behavior** (#697, #701, #702) — fixed
  deferred session context promotion, forwarded CLI mob runtime apply, and kept
  mob wiring responsive while turns are in flight.
- **Azure OpenAI regressions** (#700) — fixed CI regressions, env precedence,
  endpoint normalization, image-deployment edge cases, and auth-contract
  vocabulary coverage for Azure OpenAI.
- **Typed notice regressions** (#686) — fixed CLI typed notice display, clippy
  issues, typed turn-gate behavior, smoke summaries, and provider projections.
- **HTML artifact output edge cases** (#693) — fixed browser/open handling and
  artifact output behavior for HTML mode.

## [0.6.6] - 2026-05-11

Meerkat 0.6.6 is a patch release for live transport smoke coverage, session
scheduling ergonomics, auth binding fallback behavior, inherited mob tooling,
and release/documentation polish on top of the 0.6.5 live-adapter release.

### Added

- **Current-session schedule targets** (#671) — scheduler tools now accept a
  `current_session` shortcut for create/update calls and persist it as the
  concrete running `resumable_session` target during agent execution.
- **Live WebRTC smoke transport** (#676) — the JSON-RPC live path now has a
  WebRTC smoke-test surface and live controller peer ingress forwards helper
  and delegate comms into the active live adapter.
- **Docs validation gate** (#672) — public docs now include a Mintlify
  validation command that checks navigation, frontmatter, links, anchors,
  fences, orphan pages, and generated HTML before publishing.

### Changed

- **Auth binding fallback resolution** (#674) — agent construction now scans
  configured realm bindings when provider/model/auth binding are omitted, while
  preserving strict behavior for explicit provider, model, or binding requests.
- **Public docs refresh** (#672) — architecture, mobs, self-hosting, runtime,
  machine, and build documentation were rewritten around the current 0.6.5+
  surfaces and obsolete/internal public pages were removed from navigation.
- **Dependency refresh** (#663, #664, #665) — updated `apple_support`,
  `rules_cc`, and `tower-http` to keep toolchain and HTTP middleware
  dependencies current.
- **Release workflow hardening** — release CI now accepts the reusable Cargo
  gate shape, uses native Rust toolchain executables on Windows, and keeps Web
  SDK recovery builds alive while serializing the expensive WASM publish build.

### Fixed

- **Inherited mob tooling category caps** (#677) — inherited parent-visible tool
  filters no longer get silently capped by the selected profile's tool category
  booleans before the inherited allow-list is applied.
- **Live peer ingress visibility** (#676) — comms tools remain visible before
  trusted peers are wired so live sessions can use peer and message tools after
  later live wiring.
- **Auth binding model precedence** (#674) — explicit model selections now stay
  ahead of resolved binding `default_model` values after a fallback binding is
  selected.

## [0.6.5] - 2026-05-10

Meerkat 0.6.5 ships the live-adapter MVP and a SessionRuntime split that moves
runtime ownership into clearer session-runtime modules while keeping public
surfaces aligned through regenerated SDKs, schemas, and release packaging.

### Added

- **Live adapter MVP** (#659) — `meerkat-live` provides the composable
  WebSocket transport for live channels, `rkat-rpc --live-ws` exposes
  live/open token flow end to end, and the OpenAI realtime bridge now supports
  live input, output, interruption, and refresh observations through typed
  public contracts.
- **Live channel SDK helpers** (#659) — Python and TypeScript SDKs gain typed
  live/open and live-channel helpers backed by generated wire contracts,
  including named payload types for inline variants and explicit unknown
  variants for forward-compatible mirrors.
- **SessionRuntime split** (#659) — runtime admission, staged promotion,
  recovery, live orchestration, LLM reconfiguration, skill identity, and runtime
  state observers now live behind session-runtime modules instead of the older
  monolithic runtime shape.

### Changed

- **Realtime model catalog** (#659) — live realtime support is aligned with the
  current `gpt-realtime-2` catalog and model-affecting config changes propagate
  to live channels without overwriting per-session model overrides.
- **Live public boundary typing** (#659) — live adapter status, observations,
  refresh results, config rejection reasons, transcript sources, modality
  continuity, and transport results now use typed wire contracts rather than
  opaque values or string detail fields.
- **Release surface** (#659) — the new live-adapter crates are included in the
  release crate list and generated Bazel metadata, keeping packaging and
  BuildBuddy release validation in sync.

### Fixed

- **Live session lifecycle** (#659) — deferred-session promotion, duplicate
  live channel rejection, channel ownership cleanup, provider EOF propagation,
  cancel-safe receive handling, and text-only response requests are covered by
  regression tests and typed runtime observations.
- **Live WebSocket and examples** (#659) — example protocol usage, docs, and
  smoke scenarios were refreshed for the live/open flow and the deleted legacy
  realtime channel surface.
- **BuildBuddy batch diagnostics** (#660) — CI batch diagnostics now preserve
  clearer lane-level failure context for release and validation runs.

## [0.6.4] - 2026-05-08

### Fixed

- **Release package publishing** — Rust crate publishing now follows a
  dependency-ordered crate list, streams per-crate `cargo publish` logs, applies
  a bounded timeout, and skips Cargo's duplicate verifier during real uploads
  because release validation already packages and links the published surface.
- **Web SDK publishing** — npm Web SDK publish steps now publish the artifact
  already built by the workflow instead of re-running the expensive wasm build
  through `prepublishOnly`; Web SDK recovery also restores the Rust cache and a
  bounded job timeout.
- **BuildBuddy binary release metadata** — generated Bazel Rust targets now set
  `CARGO_PKG_VERSION` from the workspace package version, and release packaging
  rejects binaries that do not embed the requested release version.

## [0.6.3] - 2026-05-08

Meerkat 0.6.3 is a patch release for published Rust crate consumers. It fixes
the AgentFactory facade/core bridge symbol selection in registry layouts and
adds a packaged-crate downstream link smoke to the release gate.

### Fixed

- **Published facade linking** — `meerkat` now resolves the exact same-version
  `meerkat-core` package when computing the private AgentFactory policy bridge
  symbol, preventing downstream native link failures when older registry
  versions are cached locally.

## [0.6.2] - 2026-05-08

Meerkat 0.6.2 is a patch release for runtime authority, provider/tooling polish, blob/file tooling, OpenAI replay continuity, and the recovered BuildBuddy release path. It makes the runtime lifecycle spine machine-owned, improves model/tool visibility and multimodal handling, and simplifies published release assets to the four supported public runtime binaries.

### Added

- **Blob file tools** (#648) — agents can read, write, inspect, and route blob-backed files through the builtin utility tool surface.
- **Machine-owned runtime lifecycle spine** (#636) — runtime lifecycle authority now flows through the machine-owned spine, keeping lifecycle decisions under the generated machine contract.
- **OpenAI replay continuity** (#648) — OpenAI response replay preserves continuity across provider-native tool and content events.
- **Provider replay projection contract** (#639) — provider replay facts now have a typed projection contract for downstream replay/debug consumers.
- **Typed transcript fork/edit API** (#642) — transcript fork and edit flows are exposed through typed runtime APIs instead of ad hoc mutation paths.
- **Universal agent LLM client decorator** (#643) — agent LLM clients can be wrapped consistently across providers and runtime surfaces.
- **Resolved model capability visibility** (#645) — resolved model capabilities are surfaced explicitly so clients can inspect provider/model feature availability.
- **Canonical assistant image event** — assistant image generation now has a canonical event shape for runtime and SDK consumers.

### Changed

- **Provider web search defaults** (#646) — provider-native web search is enabled by default when the selected model supports it.
- **Release artifacts** — GitHub Releases and Homebrew now publish only `rkat`, `rkat-rpc`, `rkat-rest`, and `rkat-mcp`; all mini binaries are source-build-only custom profiles.
- **BuildBuddy release lanes** — BuildBuddy release builds now target the same four public runtime binaries as the GitHub-hosted release path.
- **OpenAI realtime dependency** — Meerkat now uses the published `oai-rt-rs` crate instead of an unpublished/local realtime dependency.
- **Release validation** — release readiness now parallelizes independent contract, Rust packaging, Python SDK, and TypeScript SDK checks inside the BuildBuddy validation lane.

### Fixed

- **BuildBuddy release backend** — full and asset-recovery release workflows can now select the BuildBuddy-backed release binary path while leaving the public GitHub-hosted path as the default.
- **BuildBuddy release diagnostics** — `buildbuddy-doctor` now enforces the no-mini public release surface and verifies the BuildBuddy release branch selection.
- **Cross-platform release assets** — BuildBuddy release packaging was repaired for Linux arm64, macOS arm64/x86_64, and Windows asset collection/output layout.
- **Private BuildBuddy endpoint handling** — enterprise BuildBuddy endpoint selection stays secret-scoped and owner-only; public contributors continue through the standard GitHub-hosted release path.
- **Rust package verification** — Rust release packaging and publish dry-runs work on Bash 3 and preserve the private `meerkat-core` bridge lookup needed by the facade crate during Cargo package verification.
- **Release recovery flows** — asset-only and Web SDK recovery lanes now rebuild from the release tag and skip unrelated registry/validation work.
- **Release recovery cleanup** — removed the obsolete one-off mini asset repair workflow so future repairs cannot accidentally republish mini binaries.
- **Mob executor and SDK realtime edge cases** — fixed a mob executor `handling_mode` leak and Python SDK realtime deserialization issue.
- **LLM error reporting** — provider/runtime LLM errors now surface with clearer structured context.

## [0.6.1] - 2026-05-06

Meerkat 0.6.1 is a focused patch release for provider-native tool visibility. It restores provider web search plumbing across request injection and response capture, and adds a typed profile visibility seam for image generation in mob/session tooling.

### Added

- **Provider-native search evidence** (#637) — server-executed search output now flows through typed `ServerToolContent` blocks across LLM events, assistant blocks, agent events, and wire projections. Anthropic, OpenAI, and Gemini search/grounding metadata are captured instead of being dropped as unknown content.
- **Image-generation profile visibility** (#638) — `tools.image_generation` now maps through `ToolCategoryOverride` into session build config and persisted session tooling, so mob profiles can explicitly expose or hide `generate_image` without conflating visibility with substrate availability.

### Fixed

- **Provider web search defaults** (#637) — provider-native web search defaults are no longer suppressed by Meerkat tool category overrides when the selected model supports web search.
- **Image-generation dispatcher rebinding** (#638) — image-generation visibility overrides are preserved when dispatcher state is rebound, keeping resumed/recovered sessions aligned with the profile contract.

## [0.6.0] - 2026-05-05

Meerkat 0.6.0 is the machine-authority release. It converges the runtime onto **five canonical machines** generated from a single DSL source of truth, lands **identity-first live voice** so realtime attachment is keyed on stable `AgentIdentity` rather than per-runtime bindings, completes the AuthMachine OAuth freshness model under scoped leases, types every major runtime contract that previously rode on strings or `serde_json::Value`, hardens fail-closed semantics across the runtime/REST/RPC/WASM surfaces, makes durable event storage sequence-authoritative, retires the last legacy compatibility paths from peer ingress and runtime visibility, and ships a realtime audio example plus a documentation refresh for every shipping surface.

This is a breaking release. Wire contracts that previously accepted free-form strings now require typed variants (`TerminalCause`, `CommsIntent`, `CommsResult`, `HookId`, `terminal_status`, `MemberSessionBinding`, `ProviderRuntimeBackend`, etc.). Several legacy session/runtime verbs and the `mob/realtime_attach` / `mob/realtime_detach` REST endpoints have been removed. SDK consumers should regenerate against `meerkat-contracts@0.6.0`.

### Added

#### Machine-authority architecture
- **Single-source machine DSL** (#259) — new `machine_dsl!` proc macro with parser and code generators emits runtime dispatch, phase projection, input/signal/effect enums, state structs, and `MachineSchema` artifacts from one DSL body. Catalog DSL (`meerkat-machine-schema/src/catalog/dsl/`) is the sole source of truth for production machine semantics.
- **Two-kernel DSL cutover** (#259) — handwritten authorities absorbed into canonical machines; production modules become bridge shells around catalog-owned DSL bodies. The old hand-written machine catalog has been deleted.
- **Five canonical machines** — `MeerkatMachine`, `MobMachine`, `ScheduleLifecycleMachine`, `OccurrenceLifecycleMachine`, and `AuthMachine` (split out of `MeerkatMachine` in 0.6) are now the only canonical machines, each with a DSL source under `meerkat-machine-schema/src/catalog/dsl/`.
- **Composition-protocol seams** — five typed composition protocols formalized at the seams: `meerkat_mob_seam`, `schedule_bundle`, `schedule_runtime_bundle`, `schedule_mob_bundle`, `auth_lease_bundle`.
- **Catalog/production parity gates** — `runtime_schema_parity` and `runtime_alphabet_parity` are CI-enforced ratchets; string whitelists for command classification are forbidden, replaced by typed alphabet manifests.
- **TLA+ generation and TLC verification** — `meerkat-machine-codegen` generates TLA+ models from the catalog DSL and runs TLC for closed-world composition verification.

#### Identity-first live voice
- **Identity-first live voice groundwork** (#250) — realtime attachment is keyed on stable `AgentIdentity` (survives respawn) and `AgentRuntimeId` / `FenceToken` for per-binding rotation safety. SDKs now resolve identity first, then open the realtime channel.
- **Capability-driven realtime transport** — `ModelCapabilities.realtime` on the session's resolved model decides attach/detach. There is no caller-initiated attach/detach RPC.
- **`realtime_attachment_status` projection** — typed projection on session and per-member surfaces (`session/realtime_attachment_status`, `mob/member_status.realtime_attachment_status`).
- **Live-topology reconfigure flow** — `reconfigure_live_topology` orchestration in `meerkat-runtime/src/meerkat_machine/llm_reconfigure.rs` covers in-place model/provider swaps without tearing down the session.
- **Python and TypeScript SDK ports** — both SDKs ported to identity-first resolve-then-open for `RealtimeChannel.mob_member`.

#### Realtime audio and protocol
- **Realtime audio Python example** (#539) — end-to-end OpenAI Realtime API example with streaming audio input/output, proper shutdown, and error handling.
- **Typed realtime protocol version** (#584) — `protocol_version` is now wire-typed; runtime owns version validation.
- **Realtime tool timeouts via runtime dispatch** (#610) — tool timeout enforcement routed through runtime dispatch rather than inline, so timeouts are observable per session.
- **Realtime transcript canonicalization** (#611) — transcript appends canonicalized at ingress to prevent duplicates and ordering skew across reconnects.
- **Machine-owned realtime bootstrap eligibility** (#587) — realtime attachment eligibility validated at the machine boundary; invalid reconnect states rejected.

#### Durable event storage
- **FileEventStore sequence authority** (#591) — file-backed event streams carry typed sequence authority; importers gate on sequence numbers to prevent gaps and out-of-order replay.

#### Typed machine contracts
- **Typed terminal cause spine** (#564) — structured `TerminalCause` enum replaces stringly-typed terminal reasons; invalid terminal transitions rejected at the DSL boundary.
- **Typed comms intent/result contract** (#572) — `CommsIntent` and `CommsResult` typed variants replace bare strings; routing authority validated at the machine level.
- **Typed background job completion status** (#579) — `background_job_completed` events now require typed `terminal_status`; the legacy `status` string is retained only as an optional display mirror.
- **Typed `HookId`** — hook event errors carry typed `HookId` instead of string names.
- **Typed provider runtime backend/auth matrix** (#571) — provider overrides typed with validated policy lookups.
- **Typed mob spawn-many member outcomes** (#586) — `SpawnMemberOutcome` variants typed for success/failed/skipped paths; spawn-many batches envelope results.
- **Typed mob lifecycle action dispatch** (#577) — mob lifecycle actions carry typed dispatch envelopes through machine transitions.
- **Typed `EventEnvelope` source identity** (#585) — source identity carried as typed context; source-string drift scanner replaces text-based fallbacks.
- **Typed peer directory wire facts** — peer directory facts (LUC-154) and peer endpoint parity scanner (#559) replaced with typed AST-based authority.
- **Typed runtime alphabet manifests** — generated named string domains constrained at codegen (LUC-290, LUC-292).

#### AuthMachine and OAuth freshness
- **AuthMachine OAuth freshness gate** (#612) — OAuth freshness enforced at turn admission; stale tokens rejected before execution.
- **AuthMachine cloud authorizer freshness** (#575) — cloud token leases tracked under the auth seam; freshness enforced via lease authority.
- **OAuth flow lifecycle under scoped auth authority** (#521) — OAuth flow transitions move under the scoped auth machine.
- **Managed OAuth freshness under lease** (#552) — managed OAuth admission recut under explicit lease semantics; stale poll persistence and admission resurrection prevented.
- **Auth status derived from typed phase** (#407) — auth status surfaces are lease-owned and projected from typed phases (LUC-58, LUC-193).
- **OAuth terminal state machine-owned** (#598) — OAuth terminal transitions (success/error/cancelled) owned by AuthMachine; no external override path.

#### Machine-owned policy and lifecycle
- **Machine-owned budget exhaustion** (#599) — budget-exceeded transitions owned by the machine; stale budget state in agents prevented.
- **Machine-owned hook failure policy** (#597) — hook denial/failure handling policy-enforced at the machine; terminalization atomic.
- **Mob admission via MobMachine guards** — mob membership admission centralized in MobMachine guards (LUC-189, LUC-200, LUC-205, LUC-214).
- **Surface request lifecycle classification** (#432) — request lifecycle (start/resume/external-event) routed through canonical lifecycle authority (LUC-190).
- **Comms ingress classification at machine** (#427) — comms ingress classification owned by the machine; stale/duplicate ingress events prevented.

#### Profile and tool scoping
- **`profile.tools.{mob,mcp}` actually-scoping** (#600) — tool scoping applies the canonical resolver and provenance filter; tools inherit from parent scope correctly.
- **Catalog-owned image generation defaults** (#583) — image generation defaults sourced from the model catalog rather than provider-specific overrides.
- **Provider-aware model capability boundary** (#562) — capability detection (vision, web search, reasoning) gated on the provider boundary.

#### Runtime composition and recovery
- **Boundary-atomic runtime terminalization** (#608) — surface request terminals routed through canonical lifecycle authority; cleanup atomic.
- **Scoped session recovery config** (#601) — session recovery configuration is realm-scoped and supports pluggable recovery strategies per session family.
- **Canonical runtime identity** (#526) — runtime identity split from session aliases; canonical alias recovery and snapshot authority preserved across restarts (LUC-209).
- **Mob-aware schedule delivery** (#446) — schedule delivery unified across mob members under a single typed dispatch path (LUC-93).

#### Web/SDK improvements
- **Generated Web auth wire contracts** (#580) — Web SDK auth contracts generated from `meerkat-contracts`; bearer-string fallback removed.
- **Profile overrides for TS/Web auth helpers** — auth helpers in TS and Web SDKs accept profile overrides (LUC-48).
- **Typed MCP add-config contract** (#596) — MCP add-config operations validated through typed contracts.
- **Configured MCP tools exposed to RPC mob members** (#566) — mob members reached over RPC see the same configured MCP toolset as direct callers.
- **RPC mob MCP transports kept alive** (#609) — mob MCP transports survive idle periods on RPC.

#### Help and discoverability
- **Dedicated Meerkat help surfaces** (#629) — first-class `rkat help` and platform help skill provide accurate, grounded answers about CLI commands, flags, and surfaces (LUC-443).
- **Embedded `rkat` CLI help skill** — CLI help skill embedded in the binary for offline use; help prompt grounding hardened against fabrication.
- **Refreshed platform help skill CLI facts** (#630) — platform help skill updated with current CLI command names, flags, aliases, and negative facts.
- **`meerkat-cli-reference` skill** — exact CLI command contract published as the authority for help answers.

#### Completion events
- **Structured output in completion events** (#627) — completion events now carry typed structured-output payloads (text + tool result blocks) for downstream consumption; replaces opaque string concatenation.

#### Robustness and validation
- **Fail-closed supervisor rotation** (#625) — mob supervisor rotation rejects partial / inconsistent rotations rather than advancing local authority on a divergent peer (LUC-438).
- **Fail-closed partial mob destroy** (#626) — partial mob destroy rejected; either the entire mob tears down cleanly or the operation errors and rolls back.
- **Quarantined hook semantic rewrites** (#624) — hooks that attempt semantic rewrites on event contents are now quarantined and rejected at the boundary.
- **Amputated skill builtin raw ingress paths** (#623) — built-in skills no longer admit raw ingress; all skill content flows through the typed skill resolver.
- **Hardened mobpack validation** (#621) — mobpack archive validation rejects malformed/inconsistent archives earlier and surfaces structured errors.
- **Release-grade auth smoke lane** (#616) — new dedicated smoke lane exercises live auth flows end-to-end before tag-cut.

#### Dogma gate and machine-schema audit
- **Dogma cleanup review gate** (#558) — mandatory review gate for dogma cleanup changes.
- **Self-validating immutable dogma gate** (#589) — dogma gate self-validates and audits its own freshness against generated artifacts.
- **AST-based machine drift detection** — peer terminal string ratchet and other text-based drift detectors replaced by AST checks.

#### CI / infrastructure
- **BuildBuddy stabilization** — workspace CI profiling, stale run cancellation, and self-hosted runner tuning for deterministic builds.
- **Per-branch stale CI cancellation** — superseded CI runs on the same branch are cancelled to reduce slot burn.
- **0.6 release gate stabilization** (#606) — release-preflight lanes stabilized for the 0.6 cut.

#### Documentation
- **README architecture diagram** — README now ships a top-level architecture diagram and surface map, placed in the architecture section.
- **README and example refresh** (#613, #614) — README rewritten for current Meerkat surfaces; examples updated and validated.
- **Architecture and API references refresh** (#622) — architecture and API reference docs updated for 0.6 surfaces, contracts, and machine boundaries.
- **Skills refresh for 0.6** (#615) — `meerkat-platform`, `meerkat-architecture`, `meerkat-wasm` skills updated for 0.6 wire contracts and surfaces.

### Changed

- `background_job_completed` events require typed `terminal_status` for completion semantics; the legacy `status` string is retained only as an optional display mirror.
- `TerminalCause`, `CommsIntent`, `CommsResult`, and `HookId` are now typed enums on the wire — code matching on bare strings will fail to deserialize.
- `MobMemberListEntry` and `SpawnResult` tightened to use typed `MemberSessionBinding` atoms.
- Provider policy overrides validated against catalog owner authority; mismatches rejected at admission.
- Session capacity is now active-work bounded; agents that previously accumulated unbounded queued turns will be admission-limited (LUC-294, LUC-298).
- OAuth freshness enforced at turn admission rather than at provider-call time; previously-stale tokens that would have been rejected mid-turn now fail before execution.
- Stale session projections from persistence rejected if they disagree with live runtime state; persistent stores with diverged state will fail recovery instead of silently masquerading (#560, #573).
- Tool-call argument projection now fail-closed (#581) — serialization failures block tool execution rather than silently degrading.
- Tool-call result content now preserves multimodal blocks through hook events and persisted history.
- Multimodal history preserved through compaction via blob-backed placeholders rather than text degradation.
- Provider identity sourced from the agent builder's durable identity, not from runtime overrides.
- WASM subscription mutations that fail to serialize now rejected at the boundary (#569) — prevents client/server subscription desync.
- REST terminal operations require runtime-stamped terminal evidence (#593).
- Inproc comms sends routed by canonical peer identity (LUC-287).
- Zero-pubkey peer trust paths rejected at admission (#545, LUC-286).
- Web mob decoders fail closed on schema mismatch (#567, LUC-339).
- `connection_ref` renamed to `auth_binding` across all wire contracts, REST/RPC payloads, and SDK surfaces (#618, LUC-404). Semantics unchanged.
- `rkat` default tracing quieted (#620) — default log output trimmed to user-relevant signal; verbose runtime tracing now opt-in.
- Default session capacity raised (hotfix 67afe9b65) — single-process default cap increased to better fit current mob workloads.

### Removed

- **Hand-written machine catalog** — handwritten machine bodies have been deleted; the catalog DSL is the sole source of truth for production machine semantics. Production modules that previously authored competing semantics are now bridge shells around catalog-owned DSL bodies.
- **`mob/realtime_attach` and `mob/realtime_detach` REST endpoints** — caller-initiated realtime attach/detach removed; transport is capability-driven via `ModelCapabilities.realtime`. Use the `realtime_attachment_status` projection instead.
- **Retired runtime/session verbs** — Python SDK and RPC docs scrubbed of dead verbs (`status`, `submit`, `retire`, `reset`, `submission`, `submissions`, `realtime_attachment_statuses`); typed `auth_binding` and typed realm context replace the legacy locator shapes.
- **WASM bearer-string auth fallback** (#516) — legacy string-based WASM auth path removed; all WASM surfaces route through the typed auth seam (`auth_binding` / `AuthProfile`).
- **`connection_ref` (renamed to `auth_binding`)** (#618) — the `connection_ref` field name is removed from all wire contracts, REST/RPC payloads, and SDKs; use `auth_binding` instead. The semantics are unchanged (LUC-404).
- **Peer ingress compatibility authority** (#568) — legacy peer ingress routes removed; the typed peer ingress machine is the sole authority.
- **Runtime visibility fallback** (#561) — fallback visibility restoration removed; visibility must be machine-owned or denied.
- **Recovered runtime force-authority fallback** (fd8ac40ec) — the force-authority fallback used during recovery has been amputated.
- **Runtime session compat nouns** (#576) — legacy compat nouns retired from the runtime session API surface (LUC-345).
- **Runtime session control compat routes** (#563) — legacy session-control compat routes retired.
- **Store-only session promotion** (#578) — automatic promotion of store-only sessions to live runtime removed; explicit recovery required (LUC-350).
- **Source-string machine drift scanner** (#592) — text-based drift scanner demoted in favor of AST-based detection.

### Fixed

- **Realtime reconnect retry truth machine-owned** — reconnect retry state fully machine-owned; stale retry attempts across runtime cycles prevented.
- **Flow projection persistence atomic** — flow projection state persistence atomic; half-written flow state prevented.
- **MCP identity for mob wiring** — mob-to-peer wiring uses typed identities instead of placeholder session keys.
- **Durable session fallback truth** (#401) — durable session recovery gates on real persistence state, not soft defaults.
- **Hook denial terminalization** — pre-tool and post-tool hook denials now properly terminate the turn; stale pending turns cleaned up.
- **Deferred tool load authority** (#542) — deferred tool admission owned by the machine; previously-skipped tools no longer silently disappear (LUC-288).
- **Effect authority audit self-test** (LUC-305) — multiline interrupt audit coverage restored; effect authority audit shrunk and self-validated.
- **Memory prompt extraction state** — memory prompt extraction state corrected (LUC-91).
- **Mob flow supervisor authority** — mob flow supervisor authority corrected (LUC-90).
- **Hook execution allocations** — hook execution path optimized to remove redundant allocations.
- **AGX Orin / aarch64 build hygiene** — feature-matrix lanes and surface modularity gates stabilized for cross-target builds.
- **OAuth browser auth release UX** (#619) — OAuth browser-flow release path now surfaces clear status to the user; release acknowledgement no longer hangs.
- **OAuth login config TOML serialization** (9e78ce719) — OAuth login config round-trips correctly through TOML; previous serialization could drop nested fields.
- **OAuth provider canaries** (52e50acf8) — provider canary jobs now exercise the full OAuth admission path; previously they short-circuited and missed regressions.
- **e2e-smoke realtime + WASM setup** (#617) — fixed setup races in the e2e smoke lane that surfaced as flaky realtime / WASM failures.
- **e2e-smoke realtime mob root cause** (dbdd3114f) — root-caused intermittent realtime mob failures in the smoke lane and stabilized the lane.
- **BuildBuddy bazel metadata refresh** (c02e6695f) — Bazel metadata refresh now correctly invalidates stale lanes after lockfile churn.
- **Decoupled structured output extraction terminalization** (#634) — structured-output extraction no longer terminalizes the turn on its own; ordering now matches the rest of the completion path.
- **Image tool visible without image auth** (#633) — image tool is now listed in the agent's tool catalog even when image-generation auth is absent; previously it disappeared silently from prompts.
- **Runtime boundary rollback and REST capacity tests** (7ed7cd712) — fixed flaky runtime boundary rollback and REST capacity tests.
- **Bazel generator awareness of CLI help skill** (fb2421e17) — Bazel BUILD-file generator now includes the embedded CLI help skill.

## [0.5.2] - 2026-04-12

### Added

#### Self-hosted model registry
- Two-tier model registry: server definitions (`[self_hosted.servers.<id>]`) and model aliases (`[self_hosted.models.<alias>]`).
- OpenAI-compatible transport with `chat_completions` and `responses` API styles — works with Ollama, vLLM, LM Studio, and any `/v1/chat/completions` endpoint.
- Self-hosted aliases merge into the runtime model catalog as first-class models. Provider inferred by exact alias match before prefix inference.
- `rkat doctor` validates server reachability, bearer token resolution, and remote model availability.
- `rkat models catalog` shows self-hosted models under the `self_hosted` provider group with backing server metadata.
- Per-model capability flags: `vision`, `supports_thinking`, `supports_reasoning`, `supports_web_search`, `context_window`, `max_output_tokens`, `call_timeout_secs`.
- Bearer token resolution via environment variable reference (`bearer_token_env`) — no literal tokens in config.
- Self-hosted models work identically across all surfaces (CLI, REST, RPC, MCP, SDKs).

#### RuntimeBinding — first step toward identity-first mobs
- New `RuntimeBinding` enum (`Session` | `External { peer_id, address }`) separates backend kind (definition/profile level) from runtime binding (spawn/provision level).
- External mob members now carry real process comms identity instead of phantom placeholder session keys.
- `SpawnMemberSpec.binding` field on all spawn surfaces; `ProvisionMemberRequest.binding` replaces bare `MobBackendKind` tag at provisioner level.
- `WireRuntimeBinding` wire type in `meerkat-contracts` for public MCP and RPC spawn inputs.
- Respawn preserves binding from old roster entry, maintaining real identity across incarnations.
- Bare `MobBackendKind::External` without `RuntimeBinding` is rejected — external members must declare their process identity at spawn time.
- Conflict handling: `backend` + `binding` on the same request is rejected if they disagree.

#### mob_wire / mob_unwire agent tools
- `mob_wire` and `mob_unwire` tools added to `AgentMobToolSurface` — agents can now create and remove comms trust relationships between mob members.
- Supports both local peers (within the same mob roster) and external peers (outside the roster with explicit identity).
- Reuses the existing `MobMcpState::mob_wire()` / `mob_unwire()` state API.

#### Hive agent (example 035)
- Full `SessionRuntime` + RPC server in the kennel binary for the hive agent.
- Hive mob created on startup with external-backend target profiles; targets spawned as members on registration.
- `PeerWire` / `PeerUnwire` kennel payloads for target-to-target comms mesh wiring.
- Kennel sends `PeerWire` at registration time — all-to-all mesh so targets can communicate directly.
- Target handles `PeerWire` by adding the other target as a trusted comms peer.
- `TargetRegistered` includes hive pubkey + comms address for bidirectional trust.
- Hive system prompt updated with `mob_wire`, `mob_unwire`, `mob_list_members` guidance.

#### Deferred tool catalog
- Adaptive deferred catalog discovery: tools from MCP servers and other async sources are discovered in the background and become available as each source completes.
- Typed schemas for catalog control tools.
- Deferred catalog composition and exactness hardened.

#### TCP RPC server
- `serve_tcp` and `serve_tcp_connection` in `meerkat-rpc` for JSON-RPC over TCP.
- `--tcp` flag on `rkat-rpc` binary for network listener mode.
- TCP e2e tests that spawn the real `rkat-rpc` binary.
- Concurrent connection handling (spawns each connection independently).

#### Default-on provider web search
- Web search enabled by default for all verified catalog models: Anthropic (`web_search_20250305`), OpenAI (`web_search`), Gemini (`google_search`).
- New `[provider_tools]` config section with per-provider `web_search`/`google_search` toggle (presence-aware TOML merge).
- `ModelProfile.supports_web_search` capability flag gates injection per model family.
- `SelfHostedModelConfig.supports_web_search` (default `false`) for self-hosted model opt-in.
- Non-persisted `AgentConfig.provider_tool_defaults` re-derived on every build (including resume) from current config + profile.
- Per-turn merge via RFC 7396 merge-patch in `state.rs`; extraction turns strip tool keys.
- Opt-out: `provider_tools.anthropic.web_search = false`, `provider_tools.openai.web_search = false`, or `provider_tools.gemini.google_search = false` in config; `rkat run --no-web-search`; or the provider-native null key in `provider_params` per request.

#### Realm-scoped mob profiles
- `RealmProfileStore` with `InMemoryRealmProfileStore` and `SqliteRealmProfileStore` for realm-local reusable profile definitions.
- Profile CRUD tools: `mob_profile_create`, `mob_profile_get`, `mob_profile_list`, `mob_profile_update`, `mob_profile_delete`, `mob_profile_list_sources`.
- Public MCP tools: `meerkat_mob_profile_create|get|list|update|delete|list_sources`.
- RPC methods: `mob/profile/create|get|list|update|delete` and `mob/profile/sources/list`.
- `SpawnTooling` enum (`InheritParent`, `Minimal`, `Profile`) for agent-driven child spawn tooling modes.
- `ProfileBinding::RealmRef` for mob definitions that reference realm-stored profiles.
- `ToolProvenance` metadata on `ToolDef` with `ToolSourceKind` propagated across builtins, shell, comms, schedule, mob, callback, and MCP tool sources.
- `ToolScope` snapshot seam for parent-selected child tooling at spawn time.
- `effective_profile_override` persisted in mob roster for lifecycle-safe respawn/restore.
- `tooling` parameter exposed in `delegate` and `mob_spawn_member` tool schemas.

#### Scheduler as first-class surface capability
- `ScheduleToolDispatcher` implements `AgentToolDispatcher`, wired natively into CLI, REST, RPC, and MCP surfaces.
- Schedule tools added to MDM target and hive configurations.

#### Test lane reorganization
- Unified e2e test lanes under cargo aliases: `e2e-fast` (deterministic), `e2e-system` (real local-resource), `e2e-live` (targeted live-provider), and `e2e-smoke` (kitchen-sink live smoke).
- Legacy aliases `int-real` (→ `e2e-system`) and `e2e` (→ `e2e-live` + `e2e-smoke`) retained for compatibility.

#### Homebrew tap and macOS codesigning
- `lukacf/homebrew-meerkat` Homebrew tap: `brew install lukacf/meerkat/rkat` installs all 4 binaries.
- Release workflow auto-updates the tap formula on tag push with correct checksums.
- macOS release binaries are now ad-hoc codesigned before packaging.

### Changed

#### Comms tool split (breaking)
- Agent-facing `send` tool split into three purpose-specific tools: `send_message`, `send_request`, and `send_response`.
- `handling_mode` (steer/queue) is now a required field on `send_message` and `send_request`.

#### Peer ingress machine-owned
- Peer ingress handling is now machine-owned via `PeerIngressMachine` with full handling-mode rollout.
- `wait` removed from peer ingress; peer reservations removed.

#### Communication-first delegate
- Delegate flow redesigned to be communication-first; peer reservations removed from the delegate path.

#### Mob lifecycle seams promoted to canonical machines
- `MobMemberBootstrapMachine`, session turn admission, and peer ingress owners promoted from ad-hoc seams to canonical machine authority.

#### Surface recipes replace RuntimeSessionHost
- `RuntimeSessionHost` extracted and then replaced with free recipe functions at proper crate boundaries: `wire_runtime_bindings`, `materialize_session`, `configure_peer_ingress`, `default_persistent_executor`.

#### AgentBuilder facade now uses AgentFactory
- Public `meerkat::AgentBuilder` now routes builds through `AgentFactory::build_agent()` and returns `Result<DynAgent, BuildAgentError>`.
- Standalone-only direct injections (`provider_tool_defaults`, `compactor`, `memory_store`, `with_turn_state_handle`) now fail loudly on the facade builder instead of being ignored; configure the factory path for facade-owned settings.

### Removed
- **Redb persistence**: All `RedbSessionIndex`, `RedbSessionStore`, and redb-backed storage removed. All persistence is now SQLite (WAL mode) via `SqliteSessionIndex`.
- Unused MCP runtime ingress helper removed.
- Peer reservations removed from comms and delegate paths.

### Fixed
- TUX scroll overflow: text no longer renders below the input box.
- TUX auto-scroll resumes when user scrolls to bottom instead of staying paused permanently.
- TUX session resume now loads and displays conversation history in the timeline.
- TUX idle CPU usage reduced from ~25% to <1% via dirty-flag rendering and coarser timers.
- Scenario 56 test passes API key to RPC/REST subprocesses and skips when unavailable.
- Unused `meerkat-schedule` dependency removed from example 035.
- Post-merge test regressions: send tool rename assertions and redb rejection paths updated.
- Mob peer auto-wiring semantics corrected.
- Mob authority via typed tool effects (no re-entrant deadlock).
- `InterruptYielding` wired to cooperative wait interrupts and emits `WakeRuntime` for queued inputs.
- Comms drain notification race fixed.
- Auto-derive `comms_name` for CLI keep-alive sessions.
- Mob member list projection made non-blocking; `list_members` stall during concurrent spawn fixed.
- Mob state restored across REST with callback peer smoke coverage.
- Terminal peer responses correctly produce single-event runtime work.
- Centralized `extract_prompt` and context-only dispatch on `RunPrimitive`.
- Typed `PostAdmissionSignal` with batch-safe `execution_kind`.
- Canonical `StartTurnDisposition` for turn admissibility.
- `ResumePending` boundary check counts staged tool results; mixed-batch assert removed.
- `execution_kind` forwarded in `MobRpcRuntimeExecutor`.
- Shared `JsonlStore` instance in example 035 target.
- Scheduler schema and CLI first-turn tools fixed.
- WASM `async_trait` on schedule dispatcher fixed.
- TUX scheduler delivery and tool guidance fixed.
- CI prereqs for wasm and shared-realm e2e lanes fixed.
- Inert session bug in MCP/mob/RPC ingress: `contains_session` replaced with `session_has_executor` to prevent turn/start hangs on sessions without a runtime loop.
- Executor notification sink staleness: sink is now read at apply time from `RwLock`, not captured at construction.
- `MethodRouter::new()` preserves existing mob state instead of overwriting per TCP connection.
- `serve_tcp` spawns connections concurrently instead of sequentially.
- `start_turn_via_runtime` no longer downgrades externally-configured comms drain.
- `trusted_peer_spec` for external members uses bridge comms key (transport) instead of `BackendPeer.peer_id` (identity) — fixes identity/transport conflation.
- External member phantom identity: `BackendPeer.peer_id` is now the real external process key, not the placeholder session's comms key.
- Self-hosted model switching: `config_runtime` set and provider inferred correctly.
- Hot-swap filter persistence and audit lane hardened.
- Projector replay writes hardened.
- Web and shared-realm test flows stabilized.

## [0.5.1] - 2026-04-06

Meerkat 0.5.1 is a feature release adding the scheduler subsystem, flow-frame loops, background job completion notifications, and the runtime epoch model — plus broad correctness fixes across mob orchestration, session recovery, and tool visibility.

### Highlights

- **Scheduler subsystem** (`meerkat-schedule`): cron and interval triggers, occurrence lifecycle, misfire/overlap/missing-target policies, schedule tools, and surface rollout across CLI, REST, RPC, and MCP.
- **Flow-frame loops**: `repeat_until` loop construct for mob flows, with frame-based execution, loop iteration authority, and durable resume/recovery.
- **Background job completion**: `CompletionFeed` delivers canonical completion entries to the agent boundary, enabling `[BG_JOB]` notices and idle wake on shell job completion.
- **Runtime epoch model**: `SessionRuntimeBindings` + `RuntimeBuildMode` eliminate the split-owner ops lifecycle bug class. All runtime-backed surfaces use `prepare_bindings()`.
- **Mob delegation tools**: Agent-facing tools for delegate, mob_create, mob_spawn_member, mob_send, and mob lifecycle management.

### Added

#### Scheduler subsystem
- New `meerkat-schedule` crate with `ScheduleLifecycleAuthority`, `OccurrenceLifecycleAuthority`, `ScheduleDriver`, `ScheduleService`, and `ScheduleStore`.
- Cron and interval trigger specs with `next_due_after()` and `occurrences_for_horizon()`.
- Misfire, overlap, and missing-target policies with configurable behavior.
- Schedule tools (`schedule_create`, `schedule_update`, `schedule_list`, `schedule_read`, `schedule_delete`, `schedule_pause`, `schedule_resume`) exposed across all surfaces.
- `ScheduleTargetDelivery` and `ScheduleTargetProbe` traits for pluggable delivery backends.
- Schedule host surface integration with `RuntimeSessionAdapter` for runtime-backed delivery.
- TLA+ formal specs for schedule and occurrence lifecycle state machines.
- Atomic planning mutations (`atomic_plan_mutation()`) on `ScheduleStore` for safe multi-step schedule changes.

#### Flow-frame loops
- `repeat_until` loop construct in mob flow specs via `FlowFrameMachine` and `LoopIterationMachine`.
- Frame-based execution model replacing flat-step dispatch for loop bodies.
- Durable loop state: iteration count, evaluation results, and resume context survive restart.
- Loop body/evaluate seam ownership via `LoopIterationAuthority`.

#### Background job completion and runtime epoch model
- `CompletionFeed` trait and `RuntimeOpsLifecycleRegistry` integration for canonical completion delivery.
- Agent boundary `[BG_JOB]` notices when background shell jobs complete.
- Idle wake fires when all background ops complete and agent is idle.
- `RuntimeEpochId`, `SessionRuntimeBindings`, `RuntimeBuildMode` types in `meerkat-core`.
- `prepare_bindings()` on `RuntimeSessionAdapter` — single canonical helper replacing hand-rolled register/extract/pass pattern.
- Factory validates `SessionOwned` bindings session_id matches build session.
- `StandaloneEphemeral` is the explicit default for test/standalone/WASM surfaces.
- `EpochCursorState` with shared atomics for persistence-ready cursor tracking.
- `PersistedOpsSnapshot` type for durable ops lifecycle recovery (bounded-loss, no invisible completions).
- `recover_or_create_ops_state()` shared recovery helper on `RuntimeSessionAdapter`.
- Persistence channel on terminal transitions (capture-and-queue pattern).
- `persist_ops_lifecycle` / `load_ops_lifecycle` on `RuntimeStore` trait with SQLite, Redb, and in-memory implementations.

#### Mob delegation and orchestration
- Agent-facing delegation tools: `delegate`, `mob_create`, `mob_spawn_member`, `mob_send`, `mob_list_members`, `mob_read_member`, `mob_finalize` via `AgentMobToolSurface`.
- `MobMcpState` and `MobMcpDispatcher` for MCP-hosted mob tool exposure.
- Built-in mob tools wired into example 035 MDM TUX target.

#### Storage and infrastructure
- `SqliteTaskStore` implementing unified storage trait contracts.
- SQLite replaces Redb for mob storage (eliminates single-writer lock contention).
- Session identity claim leak fix in mob storage migration.
- Unified `StorageTrait` contracts across task, mob, and session stores.

#### Comms and peers
- Typed `handling_mode` override on `PeerInput` for actionable peer conventions (Message, Request). ResponseProgress and ResponseTerminal are validated to reject the field at runtime admission.
- `handling_mode` field on MCP `meerkat_comms_send` tool for parity with RPC/REST.
- Shared `CommsRuntime` replaces per-surface homebrew comms wiring in example 035.

#### Examples
- Example 035: MDM TUX — ratatui device manager using P2P comms with target and TUX binaries.

#### Multimodal content
- Inline video content blocks (`ContentBlock::Video`) with `VideoData::Inline` for base64-encoded video. Gemini-only native support (`inlineData`); Anthropic and OpenAI degrade replayed video to `[video: media_type]` text placeholders.
- `duration_ms` field on video blocks for caller-provided clip duration, used in token estimation for compaction.
- `inline_video: bool` on `ModelProfile` for capability detection (Gemini=true, Anthropic/OpenAI=false).
- Video ingress validation at RPC, REST, and ephemeral session boundaries — rejects non-Gemini video with typed error.
- Video in tool results rejected at all three providers and the agent state machine.
- Compaction strips video blocks to text placeholders alongside images. Token estimation uses `max(data_size/4, duration_ms*300/1000)`.

#### Typed notices and build-seam cleanup
- `Message::SystemNotice(SystemNoticeMessage)` variant with typed `SystemNoticeKind` enum (`McpPending`, `BackgroundJob`, `ToolScope`, `ToolScopeWarning`, `Generic`). Replaces stringly-typed `Message::User` with `[SYSTEM NOTICE]` prefixes.
- Backward-compatible deserialization: old `Message::User` with `[SYSTEM NOTICE]` prefixes auto-promote to `Message::SystemNotice` on load.
- `render_metadata: Option<RenderMetadata>` on `UserMessage` for structured classification.
- `WireSessionMessage::SystemNotice` variant in wire contracts.
- All three LLM providers render `SystemNotice` as user-role text with `rendered_text()`.

#### Other
- `ToolCategoryOverride` enum (`Inherit | Enable | Disable`) for typed tool category control in `SessionTooling`.
- Typed `RejectReason` enum on `AcceptOutcome::Rejected` replacing bare `String` (NotReady, DurabilityViolation, PeerHandlingModeInvalid).
- Callback-pending completion outcome for runtime-backed surfaces.
- `codemob-mcp` session continuation, skills, and UX improvements.

### Changed
- `SessionBuildOptions.runtime_build_mode` is now a required field (default: `StandaloneEphemeral`). Replaces `ops_lifecycle_override`.
- `PreparedSurfaceSession` carries `SessionRuntimeBindings` instead of bare `ops_lifecycle`.
- All runtime-backed surfaces (CLI, RPC, REST, MCP, example 035) migrated to `prepare_bindings()` + `RuntimeBuildMode::SessionOwned(bindings)`.
- Mob provisioner pre-registers sessions via `prepare_bindings()` before `create_session()` with orphan reconciliation.
- `PersistentSessionService` uses `set_runtime_bindings_provider()` instead of `set_ops_lifecycle_provider()`.
- Prefab enum and all prefab-based mob creation deleted.
- Redundant `MobActorCoreExecutor` deleted; `ensure_autonomous_runtime_ready` slimmed.
- Mob operator tool authority boundaries tightened.
- Tool override fields on `SessionBuildOptions` and `AgentBuildConfig` migrated from `Option<bool>` to `ToolCategoryOverride`. Tool-specific bits removed from `ResumeOverrideMask`.

### Fixed
- Background shell job completions now correctly wake the agent in all runtime-backed surfaces (previously silent due to split-owner registry bug).
- Tool category suppression on session resume: new tool categories (e.g. mob tools added after session creation) now inherit correctly instead of being frozen at creation time.
- RPC stdout `WouldBlock` crash on high-throughput streaming.
- Callback tools wired into mob agents (previously missing).
- 035 target freeze: replaced homebrew comms with `CommsRuntime` for correct lifecycle management.
- Shared `CommsRuntime` no longer overrides per-session comms identity.
- Session identity claim leak in mob storage.
- Version corrected from 0.6.0 to 0.5.1.

## [0.5.0] - 2026-03-26

Meerkat 0.5 is a large architecture and surface cutover. It formalizes runtime ownership around generated authorities and runtime-backed session services, removes a wide set of legacy public-surface residue, brings persistent session and mob recovery much closer to truthful replay, and adds a realm blob store for image content.

### Highlights

- Runtime-backed semantics are now the canonical public model across CLI, REST, RPC, MCP, Rust, Python, TypeScript, and Web surfaces.
- `host_mode` has been fully replaced by `keep_alive`, with stricter validation, clearer tri-state behavior, and consistent cross-surface ownership.
- Generated machine authorities, formal schemas, and seam-audit enforcement now back the most important runtime, mob, comms, turn, and ops-lifecycle semantics.
- Durable image handling moved to the new blob-backed model, with aligned history/read semantics and better multimodal behavior across providers and comms paths.
- Persistent session and mob resume behavior is much closer to truthful replay, including stronger identity continuity, broken-member handling, and runtime-backed recovery.
- Surface cancellation, commit-boundary, and external-event behavior were tightened so successful committed work is preserved and invalid/runtime-owned requests fail more honestly.

### Upgrade notes

- Treat 0.5 as a real semantic cutover from 0.4.x, not a small additive release.
- Runtime-backed session services are now the intended integration path; direct low-level construction is an expert/internal escape hatch.
- Expect `keep_alive`, blob-backed image durability, richer structured content/history models, and stronger typed lifecycle semantics across surfaces and SDKs.
- If you are upgrading an older integration, use the 0.4x -> 0.5 migration guidance in the docs/skills rather than assuming 0.4 behavior still holds.

### Added

#### Formal runtime authorities, machine schema, and seam enforcement
- New machine-authority toolchain:
  - `meerkat-machine-schema`
  - `meerkat-machine-codegen`
  - `meerkat-machine-kernels`
- New generated authority / protocol artifacts across runtime, mob, comms, external tools, and ops lifecycles.
- New formal specs and compositions under `specs/machines` and `specs/compositions`, plus `xtask` support for machine/codegen/audit workflows.
- New architecture doctrine and audit material:
  - `docs/architecture/meerkat-runtime-dogma.md`
  - `docs/architecture/formal-seam-closure.md`
  - `docs/architecture/RMAT.md`
  - `docs/architecture/finite-ownership-ledger.md`
- New CI / pre-push enforcement around schema freshness, generated artifacts, clippy cleanliness, and seam-audit drift.

#### Runtime-backed request execution and cancellation
- Shared cancellable surface request execution helpers in `meerkat::surface`.
- Runtime-backed request lifecycle and cancellation support added across:
  - JSON-RPC
  - REST
  - MCP stdio hosting
  - `examples/034-codemob-mcp`
- JSON-RPC now supports explicit request cancellation notifications.
- MCP stdio hosting now supports long-running cancellable tool execution without serially blocking the read loop.
- Successful state-advancing operations now publish committed success correctly instead of being rewritten to cancellation by late races.

#### Realm blob storage for image content
- New `BlobId`, `BlobRef`, `BlobPayload`, and `BlobStore` contracts in core.
- New built-in blob store implementations:
  - `MemoryBlobStore`
  - `FsBlobStore`
- `PersistenceBundle` now owns a matched set of:
  - `SessionStore`
  - `RuntimeStore`
  - `BlobStore`
  - `RuntimeSessionAdapter`
- New blob fetch surfaces:
  - REST `GET /blobs/{blob_id}`
  - RPC `blob/get`
  - MCP `meerkat_blob_get`
  - CLI `rkat blob get`
  - SDK blob-get helpers

#### New and expanded contracts on public surfaces
- Public session history is now fully runtime-backed and aligned across REST/RPC/MCP/SDK surfaces.
- REST external-event ingress is now canonical at `POST /sessions/{id}/external-events`.
- JSON-RPC external-event ingress is now canonical at `session/external_event`.
- REST/RPC/MCP contracts were regenerated and expanded:
  - richer RPC and REST catalogs
  - refreshed wire types and schema artifacts
  - generated web event types from contracts
- New `ErrorCode::RequestCancelled` / request-cancelled semantics in contracts.

#### Mob runtime and orchestration improvements
- New runtime-owned Broken-member projection for partial persistent resume.
- Persistent mob resume now restores missing member sessions from durable state with:
  - same `session_id`
  - preserved transcript/history
  - preserved durable LLM identity
  - preserved native inproc comms identity / `peer_id`
- New stronger mob lifecycle/orchestrator/runtime authorities and kernels.
- New real-API mob smoke coverage, including collaborative-resume and multimodal pictionary scenarios.

### Changed

#### Runtime architecture and ownership
- Runtime, comms, mob, and surface semantics now route through explicit authorities instead of shell-side lifecycle decisions.
- Input lifecycle, runtime ingress, runtime control, comms drain lifecycle, peer comms, peer reachability, ops lifecycle, turn execution, mob lifecycle, mob orchestrator, and flow-run semantics were all formalized and tightened.
- Surface code is now more explicitly “skin/mechanics only,” with semantic truth moved into authorities, protocols, and typed control seams.

#### Session service and public surface defaults
- Runtime-backed `SessionService` embedding is now the documented and tested default across Rust docs, examples, CLI, REST, RPC, MCP, and SDK guides.
- Direct `AgentBuilder` construction is now treated as an expert/internal escape hatch rather than the primary integration path.
- Session create/continue/resume behavior is more explicitly split between:
  - live/runtime-backed mutation
  - rebuild-required paths
  - committed-create vs pre-commit failure behavior

#### `host_mode` → `keep_alive`
- `host_mode` was renamed to `keep_alive` across:
  - core/session metadata
  - RPC/REST/MCP/CLI surfaces
  - SDKs
  - docs/examples/skills
  - schema/codegen artifacts
- Keep-alive now follows stricter tri-state and validation rules:
  - explicit overrides are preserved across resumed/rebuilt sessions
  - invalid keep-alive requests are rejected before stateful execution
  - disabling keep-alive now actually stops existing drain ownership

#### Image content storage and history semantics
- Durable session/runtime state no longer treats inline base64 image bytes as canonical truth.
- Durable session history and durable runtime inputs now store blob-backed image data instead of inline-only image payloads.
- Compaction now strips session images from active history, replacing them with textual placeholders, so compacted sessions do not keep paying context cost for image-bearing turns.
- History/read surfaces now align with the new blob-backed image model instead of implying old inline-image durability.

#### Mobs and comms behavior
- `AutonomousHost` behavior was tightened so active autonomous members remain live for peer ingress instead of depending on a one-shot loop handle.
- Mob runtime now uses one canonical runtime adapter per runtime instance instead of splitting turn execution and comms ingress across different adapters.
- Persistent resume no longer silently fresh-creates missing session-backed members on persistent services.
- Broken members are now consistently excluded from wiring/selection/host-loop startup paths while remaining inspectable and repairable.

#### SDKs and generated types
- Python, TypeScript, and Web SDKs were realigned with the runtime-backed/session-first API.
- Generated SDK types and helpers were updated to reflect:
  - deferred sessions
  - richer session history contracts
  - blob-backed image content
  - regenerated event/catalog artifacts
- Python and TypeScript history parsing now preserve structured text/image content instead of flattening it back to plain strings.

### Removed

- Legacy `host_mode` terminology from public docs/examples/SDKs/contracts.
- Removed old `docs/architecture/0.5/*` planning dump in favor of the new normative architecture docs plus `.rct` material.
- Removed / scrubbed legacy delegated/helper-agent and sub-agent public-surface residue that no longer matched the settled 0.5 surface model.
- Removed a variety of dead or obsolete runtime shell helpers, stale driver entry methods, and old host-mode ownership residue that no longer matched authority-owned semantics.

### Fixed

#### Persistent resume, recovery, and identity continuity
- Fixed persistent mob resume so session-backed members preserve durable identity instead of silently coming back as fresh sessions.
- Fixed idle-live session detection during mob resume; persisted-only summaries no longer masquerade as live sessions, and live idle sessions are no longer misclassified as missing.
- Fixed session-scoped native comms identity so resumed sessions preserve `peer_id` across runtime roots.
- Fixed autonomous member runtime ownership so comms drains and runtime turns use the same canonical adapter.

#### Keep-alive / continue / resume correctness
- Fixed keep-alive ordering so validation happens before stateful execution and rejected requests are side-effect free.
- Fixed keep-alive propagation across RPC/REST/MCP/CLI resume and turn paths.
- Fixed drain survival / cleanup semantics for committed keep-alive sessions on error paths.
- Fixed late-cancel races so successful committed `turn/start` / `meerkat_resume` / similar operations are not rewritten to `REQUEST_CANCELLED`.

#### Multimodal comms and image handling
- Fixed multimodal peer ingress for autonomous mob members after kickoff completion.
- Fixed comms/runtime paths that flattened or dropped multimodal image blocks.
- Fixed multimodal body-vs-rendered-text handling so raw peer message bodies are preserved instead of replaced by lossy projections.
- Fixed provider-specific image serialization regressions (including Gemini user messages and Anthropic tool-result images).

#### Surface and contract regressions
- Fixed CLI runtime-backed teardown/output pipeline regressions.
- Fixed RPC/REST/MCP runtime-backed ingress, resume, and external-event regressions.
- Fixed REST request cancellation races and cleanup paths.
- Fixed MCP cancellability and responsiveness in `034-codemob-mcp`.
- Fixed runtime batch staging, metadata merge, UTF-8 panic, callback timeout, and retry-hint propagation regressions.

#### Compaction and budget correctness
- Fixed compaction cadence fallback across reused / legacy sessions.
- Fixed compaction token estimation so base64 image data and tiny text blocks are not miscounted.
- Fixed timeout / time-budget terminalization so structured timeout conditions retain their typed meaning.

#### Clippy, WASM, CI, and publishability
- Brought the workspace to clean `clippy -D warnings` status across a very large legacy warning backlog.
- Fixed multiple WASM compilation and gating issues across `meerkat-web-runtime`, web bindings, and example paths.
- Fixed publish / path-dependency / workspace packaging issues across crates.
- Hardened pre-push and CI gates for feature branches and generated artifact freshness.

### Breaking changes

- `host_mode` has been renamed to `keep_alive` across public surfaces and generated SDK/contracts.
- Runtime-backed session services, not direct builder execution, are now the intended public integration path.
- Durable image/history semantics changed to the new blob-backed model; old inline-image durable formats are not preserved.
- Python/TypeScript/Web SDK content and history models changed to preserve structured content instead of flattening it.
- A significant amount of stale legacy/internal surface residue was removed or renamed to match the settled 0.5 contracts.

## [0.4.13] - 2026-03-16

### Fixed

- **Mob multimodal content silently discarded** — `MobHandle::send_message` took `String`, flattening multimodal `ContentInput` (images + text) to plain text before it reached the session service. Threaded `ContentInput` end-to-end through `MobHandle`, `MobCommand`, actor dispatch, `SpawnMemberSpec`, and `to_create_session_request`. TurnDriven mode now passes `ContentInput` directly to `StartTurnRequest`; AutonomousHost mode extracts text at the `EventInjector` boundary (known limitation).
- **Wire boundaries blocked multimodal mob content** — JSON-RPC mob param structs (`MobSpawnParams`, `MobSendParams`, `MobRespawnParams`) accepted `String` instead of `ContentInput`. Updated to accept `ContentInput` directly via serde untagged deserialization (backward compatible — plain strings still work). WASM bindings now parse incoming strings as `ContentInput` JSON with text fallback. Python SDK mob methods widened to `str | list[dict]`, TypeScript/Web SDK mob methods widened to `string | ContentBlock[]`.

## [0.4.12] - 2026-03-16

### Added

#### Multimodal Content Support (Images) (#154)
- **`ContentBlock` type** — `Text` and `Image` variants in `meerkat-core`, threaded through tool results (`ToolResult.content: Vec<ContentBlock>`), user messages (`UserMessage.content: Vec<ContentBlock>`), and all provider adapters. Backwards-compatible serde: plain strings deserialize to `[Text]`, text-only content serializes as string.
- **`ContentInput` type** — untagged `Text(String) | Blocks(Vec<ContentBlock>)` accepted by `CreateSessionRequest.prompt` and `StartTurnRequest.prompt` across all surfaces (REST, RPC, CLI, MCP Server).
- **`ToolOutput` enum** — `Json(Value) | Blocks(Vec<ContentBlock>)` replaces `Value` return on `BuiltinTool::call()`, enabling tools to return multimodal content.
- **`view_image` builtin tool** — reads images from disk with path sandboxing (symlink-safe via `canonicalize`), 5MB size limit, extension validation (PNG/JPEG/GIF/WebP/SVG). Returns `ToolOutput::Blocks` with base64-encoded image data. Guarded with `#[cfg(not(target_arch = "wasm32"))]`.
- **Provider capability gating** — `ModelProfile` gains `vision` and `image_tool_results` fields. `view_image` hidden via `ToolScope` external filter for models that can't process image tool results (OpenAI). Dynamic refresh on model hot-swap: filter composes with existing restrictions instead of clobbering.
- **Provider image serialization** — Anthropic: native `image.source.base64` format in user messages and tool results. OpenAI: `image_url` data URIs in user messages, text degradation for tool results. Gemini: `inlineData` parts in user messages and alongside `functionResponse` for tool results.
- **MCP image passthrough** — `McpConnection::call_tool()` returns `Vec<ContentBlock>`, capturing `image` content from MCP servers as `ContentBlock::Image`.
- **Comms multimodal plumbing** — `blocks: Option<Vec<ContentBlock>>` added alongside `body: String` at every comms layer (`MessageKind`, `CommsContent`, `InteractionContent`, `CommsCommand`, `PlainMessage`, `InboxItem`, `SendInput`). CBOR backwards compat verified. Turn-boundary drain and host-mode batching paths preserve blocks.
- **Runtime multimodal routing** — `CoreRenderable::Blocks` variant, `PromptInput.blocks` and `PeerInput.blocks` fields, `extract_prompt()` returns `ContentInput` on both RPC and REST runtime executors.
- **Wire types** — `WireContentBlock` (no `source_path`), `WireContentInput`, `WireToolResultContent` in `meerkat-contracts`. Schema regenerated. Forward-compatible `Unknown` variant.
- **SDK multimodal prompts** — Python: `prompt: str | list[dict]` on all session methods. TypeScript: `prompt: string | ContentBlock[]`. Web SDK: `ContentInput` parsed at WASM bridge.
- **Hook `has_images` flag** — `HookToolResult.has_images` and `ToolExecutionCompleted.has_images` for downstream consumers.
- **Hook patch rebuild rule** — deterministic: strip text blocks, prepend patched text, append image blocks in original order.
- **Compaction image stripping** — `strip_images_for_compaction()` replaces images with `[image: {media_type}]` placeholders. `source_path` excluded from placeholders to prevent filesystem path leaks.
- **`Display` impl for `ContentBlock`** — delegates to `text_projection()`.
- **Dispatch-time tool gating** — hidden tools (via ToolScope external filter) are blocked at execution time, not just advertisement time.

### Changed

- **`ToolResult.content`** — `String` → `Vec<ContentBlock>` (breaking Rust API). `ToolResult::new()` still accepts `String`. Use `.text_content()` for string access.
- **`UserMessage.content`** — `String` → `Vec<ContentBlock>` (breaking Rust API). `UserMessage::text()` constructor for common case.
- **`BuiltinTool::call()` return** — `Result<Value, _>` → `Result<ToolOutput, _>` (breaking for custom tool implementors).
- **`CreateSessionRequest.prompt` / `StartTurnRequest.prompt`** — `String` → `ContentInput` (breaking, use `.into()` from String).
- **`AgentRunner::run()` / `run_with_events()`** — accept `ContentInput` instead of `String`.
- **`content_blocks_serde`** — only collapses single text block to string; multi-block text arrays serialize as arrays to preserve block boundaries.
- **`source_path`** — `#[serde(skip_serializing)]`: never persisted to session stores, only used in-memory for compaction re-read hints.

## [0.4.11] - 2026-03-15

### Fixed

- **RPC in-session model switching silently dropped** — `turn/start` model/provider/provider_params overrides were built in the handler but never reached the executor because `RuntimeTurnMetadata` did not carry those fields. Added model/provider/provider_params to `RuntimeTurnMetadata`, extracted `hot_swap_llm_client()`, and wired it into `apply_runtime_turn` so overrides propagate end-to-end.
- **MCP drain race in tests** — `set_inflight_calls_for_testing` could fire before the MCP router was ready. Now waits for `wait_until_ready` first.

## [0.4.10] - 2026-03-15

### Added

#### `meerkat-models` — Curated Model Catalog (#148)
- New leaf crate `meerkat-models` as the single source of truth for model defaults, allowlists, capability detection, and parameter schemas. Consolidates model data previously scattered across `config.rs`, `config_template.toml`, and client adapters.
- Catalog module with curated entries for all supported providers (Anthropic, OpenAI, Gemini) including default models, allowed model lists, and per-model parameter schemas.
- Profile module with provider-specific rules: per-model param schemas that document exactly which `provider_params` keys each adapter reads and processes (e.g., opus-4-6 gets adaptive thinking + effort + compaction; non-gpt-5 models don't advertise `reasoning_effort`).
- `models/catalog` endpoint on all surfaces: CLI (`rkat models catalog`), REST, RPC (`models/catalog`), and MCP Server.
- Wire types in `meerkat-contracts` (`ModelsCatalogResponse`) with SDK codegen.

#### Mid-Session Model/Provider Hot-Swap (#147)
- `Agent::replace_client()` swaps the LLM client on a live agent without rebuilding.
- `SessionAgent::replace_client()` trait method (default no-op) and `SessionService::set_session_client()` (default `Unsupported`).
- RPC `turn/start` now accepts `model`/`provider`/`provider_params` on materialized sessions, builds a new client via factory and hot-swaps before the turn.

### Fixed

#### Comms Interrupt Regressions (#147)
- **False wakes from non-actionable traffic** — raw `inbox_notify` woke `wait` on responses, acks, lifecycle traffic, and plain events. Added single-pass ingress classification in `meerkat-comms` with narrow `actionable_input_notify` that fires only for `ActionableMessage`/`ActionableRequest`. Untrusted items dropped at ingress with snapshot semantics.
- **Wait tool not interruptible on some dispatcher paths** — override and WASM dispatcher paths never wired wait interruption. Added `bind_wait_interrupt()` and `supports_wait_interrupt()` on `AgentToolDispatcher` trait with implementations on `CompositeDispatcher`, `ToolGateway`, and `FilteredToolDispatcher`. Factory probes before consuming bind and falls back gracefully.
- **Wait budget overshoot** — wait could overshoot `max_duration` by up to 1800s. `MAX_WAIT_SECONDS` reduced to 60s.
- **Trust state split between async and sync locks** — collapsed `tokio::sync::RwLock` and `parking_lot::RwLock` sidecar into single `Arc<parking_lot::RwLock<TrustedPeers>>` shared by Router, `IngressClassificationContext`, and `trusted_peers_shared()` callers. Mutations through any handle are immediately visible to classification.
- **ChildInproc trust is not inferred at construction** — `CommsBootstrap::prepare` now fails closed unless generated wiring installs parent trust through typed comms trust authority.
- **Lifecycle intent serialization** — `PeerAdded`/`PeerRetired` now serialize as `"mob.peer_added"`/`"mob.peer_retired"` via explicit `#[serde(rename)]`.
- **Host-mode hot loop spin** — legacy fallback falls through to `tokio::select!` when no work performed instead of unconditional continue that spins until budget exhaustion.
- **Legacy drain classification** — turn-boundary drain fallback now classifies by `InteractionContent` (peer lifecycle batching, response inline injection) instead of raw concat. Host-mode drain routes per-interaction with subscriber/tap/sink support.
- **Shared dispatcher consumed during bind** — factory now checks `Arc::strong_count` before `bind_wait_interrupt` and skips binding for shared dispatchers instead of consuming the caller's dispatcher.

### Changed

- **`meerkat-core` reads model defaults from catalog** — `ModelDefaults` no longer reads from `config_template.toml`; all model defaults come from `meerkat-models` catalog.
- **OpenAI adapter delegates detection to shared profiles** — capability detection logic moved from `meerkat-client` to `meerkat-models` profile rules.
- **Legacy delegated-agent validation uses catalog for fallback resolution** — the remaining `sub-agents` compatibility path in `meerkat-tools` now resolves fallback models via catalog instead of hardcoded strings.
- **Stale template defaults updated** — `gpt-4.1` → `gpt-5.2`, `gemini-1.5-pro` → `gemini-3-flash-preview`.
- **`SessionError::Unsupported` variant** — new error variant for capability negotiation across session service implementations.

## [0.4.9] - 2026-03-14

### Fixed

- **MCP tools invisible via RPC** — `start_turn_via_runtime()` (the V9 runtime path used by all RPC sessions) bypassed `apply_mcp_boundary()`, so MCP servers staged via `mcp/add` were never connected or made visible to agents. MCP tools were completely broken on the RPC surface since 0.4.7.
- **Wait tool not interruptible by peer comms** — `WaitTool` was created without interrupt support, so peer messages arriving during a `wait()` call queued silently until the wait completed. Added `WakeMode::InterruptYielding` policy for `peer_message`/`peer_request` while running, wired comms `inbox_notify` → `WaitTool` interrupt channel via factory bridge task. Agents now respond to peer messages during wait instead of blocking for up to the full requested duration.
- **Wait tool cap raised** from 300s to 1800s — sleep costs zero budget; the old 300s cap forced unnecessary LLM round-trips. Note: budget checks happen at loop boundaries, not during tool dispatch, so the cap balances responsiveness against overshoot risk in non-comms sessions.

### Changed

- **`WakeMode::InterruptYielding`** — new policy variant for peer inputs while running. Interrupts cooperative yielding points (e.g., wait tool) without cancelling active work or waking idle runtimes. Applied to `peer_message` and `peer_request` in `DefaultPolicyTable`.

## [0.4.8] - 2026-03-13

### Fixed

- **Cross-crate `include_str!` in facade** — `meerkat` crate referenced `../../meerkat-mob/skills/mob-communication/SKILL.md` which works in workspace builds but breaks when the crate is pulled from crates.io. Copied the skill file into the facade crate.
- **`meerkat-runtime` missing from crate publish order** — `meerkat-session` depends on `meerkat-runtime` but it wasn't in the CI publish sequence, causing registry publish failures.
- **Path-only dependencies break `cargo publish`** — 6 crates referenced `meerkat-runtime` via `path = "../meerkat-runtime"` without `workspace = true`, causing `cargo publish` to reject them. Fixed all to use workspace dependencies.
- **Release workflow publish decoupled from binary builds** — `publish_registries` job no longer depends on `build_binaries`, enabling manual dispatch to re-run publishing without rebuilding all 5 platforms.

## [0.4.7] - 2026-03-13

### Added

#### `meerkat-runtime` — Canonical Lifecycle Runtime (#140)
- New `meerkat-runtime` crate implementing the v9 Canonical Lifecycle and Execution Specification with a strict 3-layer model: Core (run primitives), Runtime (input lifecycle), and Surfaces (protocol adapters).
- 6 input type variants with `InputHeader`, `InputOrigin`, `PeerConvention`.
- `InputState` lifecycle: 9 states with validated transition table (`AppliedPendingConsumption` → `Queued` explicitly rejected).
- Policy engine: `DefaultPolicyTable` with `ApplyMode`, `WakeMode`, `QueueMode`, `ConsumePoint`, and `record_transcript`.
- `RuntimeState`: 7 states with strict transition table.
- Durability validation: derived forbidden for required input types.
- Coalescing/supersession: scope-based, cross-kind forbidden.
- `EphemeralRuntimeDriver` and `PersistentRuntimeDriver` (durable-before-ack via `RuntimeStore`).
- `InMemoryRuntimeStore` and `RedbRuntimeStore` backends.
- `SqliteRuntimeStore` — SQLite backend for runtime state persistence.
- `CommsInputBridge`: `InboxInteraction` → `PeerInput` conversion.
- `SessionServiceRuntimeExt` + `RuntimeSessionAdapter`: per-session driver registry.
- `MobRuntimeAdapter`: flow step delivery, member registration.
- Core lifecycle primitives in `meerkat-core/src/lifecycle/`: `RunPrimitive`, `RunEvent`, `RunBoundaryReceipt`, split executor boundary/interrupt handles, `CoreExecutor` trait, `RunId`/`InputId` newtypes.
- 222+ tests across the crate.

#### JSON-RPC Parity — 9 Wire-Feasible Gaps Closed (#138)
- **Deferred session creation**: `session/create` with `initial_turn: false` returns session ID without running a turn. `DeferredSession` class in Python and TypeScript SDKs.
- **Per-turn overrides**: `model`, `provider`, `max_tokens` overrides on `turn/start` for pending sessions.
- **Rich session responses**: `session/list` returns `WireSessionSummary` (timestamps, message counts, token totals) with pagination (`limit`/`offset`). `session/read` returns `WireSessionInfo` (timestamps, message counts, last assistant text).
- **Batch mob spawning**: `mob/spawn_many` with per-spec error reporting.
- **Scoped event streaming**: `scope_id`/`scope_path` preserved on `session/stream_event` notifications for delegated child-scope / flow-scope forwarding.
- **Persistent session recovery**: `turn/start` recovers persisted sessions after runtime restart, mirroring the REST recovery path. Archived sessions are rejected.
- **Callback tool protocol**: bidirectional JSON-RPC request/response for SDK-provided tool implementations. `ToolCallbackHandler` helpers in Python and TypeScript SDKs.
- 6 new RPC handler methods with legacy-mode guard.

#### Session History Across Surfaces (#142)
- Public session history (message listing) exposed on all surfaces: CLI `session history`, REST `GET /sessions/{id}/history`, RPC `session/history`, MCP `meerkat_session_history`.
- `SessionService` trait extended with history read capability.
- Wire types (`WireSessionHistory`, `WireSessionMessage`) added to `meerkat-contracts`.
- SDK codegen updated for Python and TypeScript.

#### SQLite Persistent Realm Backend (#143)
- `SqliteSessionStore` in `meerkat-store` — SQLite-backed session persistence for realm backends.
- SQLite is now the default persistent realm backend.
- Backend-owned persistence bundle pattern: realm backends own their persistence infrastructure rather than having it constructed externally.

### Changed

- **Persistence architecture** — persistence bundles are now opened from realm backends rather than assembled externally. Surfaces (CLI, REST, RPC) use the new bundle pattern for cleaner resource lifecycle.
- **Documentation** — refreshed persistence and session history docs across API reference, concepts, SDK guides, and architecture pages.

### Fixed

- **Host loop comms continuation** — idle host loop now triggers a continuation run when terminal comms responses (`Completed`/`Failed`) are delivered, instead of leaving agents unresponsive until the next external message. Scoped narrowly to terminal responses only; `Accepted` does not wake the host. Emits `RunStarted` before the continuation for correct stream lifecycle (#139).
- **Archived session recovery** — RPC and REST runtime recovery now rejects archived sessions instead of attempting to reconstruct them, preventing stale session resurrection.
- **Persistence helper panic** — avoided panic in persistence helper on unexpected backend state.
- **MCP tools schema test counts** — corrected test assertions after schema changes.
- **SQLite store clippy** — fixed clippy style issues in new sqlite stores.

## [0.4.6] - 2026-03-10

### Changed

- **Clean-cut comms/observability split** — removed mixed public interaction-stream APIs across Rust SDK, CLI, REST, RPC, MCP, WASM, and both Python/TypeScript SDKs. Public comms now exposes delivery (`inject`, `send_message`, `comms/send`) and explicit observation surfaces separately; interaction-scoped comms stream helpers remain runtime-internal only.
- **First-class mob and session parity across all SDKs** — Python SDK, TypeScript SDK, and Web SDK now expose explicit `Mob` and `Session` classes with typed mob lifecycle, member management, flow control, and event subscription methods. RPC surface adds dedicated `mob/*` methods (`mob/create`, `mob/list`, `mob/status`, `mob/members`, `mob/spawn`, `mob/retire`, `mob/respawn`, `mob/wire`, `mob/unwire`, `mob/lifecycle`, `mob/send`, `mob/events`, `mob/stream_open`, `mob/stream_close`, `mob/append_system_context`, `mob/flows`, `mob/flow_run`, `mob/flow_status`, `mob/flow_cancel`) as the canonical typed substrate for SDKs.
- **EventSubscription replaces CommsEventStream** — Python SDK exports `EventSubscription` instead of `CommsEventStream`/`CommsStreamEvent`. TypeScript SDK exports `EventSubscription<T>` (generic, async-iterable). Web SDK's `EventSubscription<T>` is now generic with a `parseEvents` callback.
- **Standalone session event streaming** — RPC adds `session/stream_open` and `session/stream_close` for explicit event stream lifecycle. Web SDK adds `Session.subscribe()` returning `EventSubscription<EventEnvelope>`.
- **Mob subscription methods are now async** — Web SDK `Mob.subscribe()` and `Mob.subscribeAll()` return `Promise<EventSubscription<T>>` instead of synchronous handles. `mob_member_subscribe` and `mob_subscribe_events` WASM exports are now async.
- **Mob events use numeric cursors** — `mob_events` WASM export takes a numeric `afterCursor` parameter instead of a string. Web SDK `Mob.events()` converts string cursors to numbers internally.
- **Mob create returns string directly** — `mob_create` WASM export returns a plain string mob ID instead of JSON. `mob_run_flow` similarly returns a plain string run ID.
- **Typed mob observation** — `Mob.subscribeAll()` returns `EventSubscription<AttributedEvent>` (source + profile + envelope) on Web SDK. `Mob.events()` returns `MobEvent[]` (cursor + timestamp + mob_id + kind). TypeScript RPC SDK uses `AttributedMobEvent` and `AgentEventEnvelope` types for mob/member subscriptions.
- **Web SDK types refined** — `SpawnResult` now has `status: 'ok' | 'error'` with optional `member_ref` and `error` fields. `MobMember` includes `member_ref`, `runtime_mode`, `state`, `wired_to`, and `labels`. `MobStatus` uses `state` field instead of `status` + `member_count`.

### Removed

- **`inject_and_subscribe`** — removed from WASM exports, Web SDK `Mob` class, and `MobWasmBindings` interface. Use `Mob.sendMessage()` + `Mob.subscribe()` separately.
- **`CommsEventStream` / `CommsStreamEvent`** — removed from Python SDK. Use `EventSubscription` instead.
- **`openCommsStream` / `sendAndStream`** — removed from TypeScript SDK `MeerkatClient` and `Session`. Use explicit mob/session observation subscriptions instead.

### Fixed

- **Stream termination and WASM subscription cleanup** — fixed event stream termination handling and subscription resource cleanup in the WASM runtime and RPC router.
- **Config runtime lock cleanup** — hardened lock cleanup in the config runtime to prevent leaked locks on error paths.
- **Mob-backed session routing** — hardened routing and control for mob-backed sessions, including proper session resolution for mob members.
- **MCP run handler without comms** — fixed MCP `meerkat_run` handler crash when the `comms` feature is not compiled in.
- **Pre-push unit hook stability** — serialized pre-push unit test runs across worktrees with a per-tree cache and one retry if `nextest` discovery hangs.

## [0.4.5] - 2026-03-07

### Fixed

- **Gemini tool schema validation** — `strip_gemini_function_parameters_unsupported_keywords` no longer strips property names from `properties` maps. A user-defined field named `"title"` was being removed as a JSON Schema keyword, causing Gemini to reject the schema with `required[1]: property is not defined`. The stripper now recurses into each property's schema individually without touching the properties map keys.
- **Example 033 sprite 404s** — sprite loader capped at 6 frames (00–05) instead of 8; eliminates 40 console 404s per page load.
- **API key leak in Vite bundle** — removed `define` block from example 033's `vite.config.ts` that baked raw `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` / `GEMINI_API_KEY` into the production bundle. Only `import.meta.env.VITE_*` (opt-in) pattern remains.
- **WASM not shipped in `@rkat/web`** — `build:wasm` script now removes wasm-pack's generated `.gitignore` (which contained `*`) so npm includes wasm artifacts in the published package.

### Added

#### Example 034: codemob-mcp — Implement Pack & User Mob CRUD
- **`implement` pack** — gated implementation with iterative review loop. Two comms-based agents (implementer + reviewer) iterate until the reviewer approves (max 3 rounds). Uses diverse models (claude-sonnet-4-6 + gpt-5.3-codex) for genuine review independence.
- **User mob CRUD tools** — `create_mob`, `get_mob`, `update_mob`, `delete_mob` MCP tools. User-created mobs stored as JSON under `.codemob-mcp/mobs/` and loaded dynamically into the pack registry without MCP restart. Both `comms` and `flow` execution modes supported.
- **Activity-based comms termination** — comms-based execution now tracks active agent turns via `RunStarted`/`RunCompleted` events instead of a fixed quiescence timeout. Agents can work for as long as needed (up to 1 hour hard cap); termination triggers when all agents are idle for a 30-second grace period.
- **Dynamic orchestrator routing** — comms initial message target and result capture now use the mob definition's orchestrator profile instead of hardcoded `"moderator"`.

## [0.4.4] - 2026-03-06

### Added

#### Session System-Context Control Plane (#121)
- `SessionServiceControlExt::append_system_context()` — inject runtime system context into a live session without rebuilding it.
- Staged appends are applied at the `CallingLlm` boundary just before the next model call.
- Idempotency enforced per session via `idempotency_key`. Duplicate keys return `Duplicate` status.
- Canonical system prompt remains the first `Message::System`; appended context follows as additional system messages.
- State carried through checkpoints, clones, forks, and persistence.
- Wired across all surfaces:
  - CLI: `session inject-context`
  - REST: `POST /sessions/{id}/system_context`
  - RPC: `session/inject_context`
  - WASM: `append_system_context(handle, request_json)`
  - Web SDK: `Session.appendSystemContext(options)`

#### Mob Member System-Context Control (#122)
- `mob_append_system_context(mob_id, meerkat_id, request_json)` WASM export — append system context to an individual mob member's session by resolving its live session through the mob roster.
- Web SDK: `Mob.appendSystemContext(meerkatId, options)` with `MobAppendSystemContextResult` type.
- Shared `resolve_mob_member_session_id()` helper deduplicates roster lookup logic with `mob_member_subscribe`.

#### Fire-and-Forget JS Tool Registration (#120)
- `register_js_tool(name, description, schema_json)` WASM export for tools that return `"acknowledged"` immediately without a JS callback.
- Agents get proper schema-validated tool calling; the host watches `ToolCallRequested` events in the stream and responds asynchronously via `mob_send_message`.
- Existing `register_tool_callback` (callback-based) unchanged — backwards compatible.
- Duplicate tool names are latest-wins across both registration modes.
- Web SDK: `MeerkatRuntime.registerFireAndForgetTool()` static method.

#### Structured Output Extraction Fix (#118)
- Structured-output extraction now unwraps provider-style named envelopes (e.g., `{"advisor": {...}}`) when the inner object matches the configured schema shape.
- `FlowStepSpec.output_format` option: `"json"` (default) or `"text"` to allow non-JSON agent outputs without parse failures.

#### Example 033: The Office — 10-Agent WASM Multi-Agent Demo (#123)
- Browser-based demo: 10 autonomous AI agents in a pixel-art office process events, coordinate via desk phone calls, store knowledge, and route actions through compliance.
- Demonstrates: mob orchestration in WASM, `autonomous_host` mode, inter-agent comms with visual phone arcs, fire-and-forget JS tools (`request_human_approval`, `upsert_record`, `revoke_access`, `restore_access`), system context injection for admin trust policy.
- Dieter Rams inspired UI: warm cream/copper chrome, tabbed log/records/graph panel, Cytoscape.js knowledge graph, floating approval panel, trust toggle (The Boss / Outsider).
- 6 pre-built scenarios: Client Escalation, Server Room Alert, Expense Report, Calendar Conflict, New Hire Onboarding, Security Breach.

#### Example 034: force-mcp — Multi-Agent Teams as MCP Tools (#119)
- Standalone MCP server exposing Meerkat mobs as `consult`, `deliberate`, and `list_packs` tools for Claude Code.
- 7 mobpacks: advisor, review, architect, brainstorm, red-team, panel (comms-driven), rct (full pipeline).
- MCP progress notifications for live progress bars during deliberation.

### Changed

- **Model names**: Updated model table — added `gpt-5.3-codex`, `gemini-3.1-pro-preview`, `gemini-3.1-flash-lite-preview`, `claude-sonnet-4-6`. Removed deprecated `o1-*`/`o3-*`/`o4-*` prefixes from provider inference (#117).
- **Demo server mode**: Diplomacy (031) and WebCM (032) demos support `?proxy=` query param for hosting behind `@rkat/web` proxy with server-side key injection (#116).
- **MCP readiness**: Aligned readiness waits with the real async connect budget; reduced adapter polling latency. Fixes flakiness under full-suite load (#121).

### Fixed

- Session context persistence: live persistent appends no longer mutate runtime state before durable save succeeds.
- Unknown session IDs no longer leak checkpointer gates.
- Pending-session promotion preserves injected context during first turn/start.
- Successful promotion no longer reports a false turn failure if replay staging has a post-create problem.
- Diplomacy demo map: filled Kosovo, North Macedonia, Albania gaps; Croatia/Slovenia render as Austria-Hungary; resolution mode cropping fix.
- Proxy CORS: `x-goog-api-key` added to allowed headers.

## [0.4.2] - 2026-03-04

### Added

#### `@rkat/web` npm Package
- New `sdks/web/` TypeScript wrapper around wasm_bindgen exports with idiomatic camelCase API.
- `MeerkatRuntime` class: `init()`, `initFromMobpack()`, `createMob()`, `createSession()`, version validation.
- `Mob` class: spawn, wire, retire, flows, event subscriptions.
- `Session` class: multi-turn agent loop, event polling.
- `EventSubscription` class: typed `poll()` / `close()` over WASM subscription handles.
- `registerTool()` static method for JS tool callback registration before init.
- TypeScript types aligned to exact Rust serde wire format (`AgentEvent`, `Profile`, `WiringRules`, `ToolConfig`).

#### Provider Proxy
- Node.js auth-injecting reverse proxy in `sdks/web/proxy/`.
- `npx @rkat/web proxy --port 3100` standalone CLI.
- Composable `createProxyHandler()` for integration into existing Node.js servers.
- Routes: `/anthropic/*`, `/openai/*`, `/gemini/*` with per-provider auth injection.
- CORS support, SSE streaming, `Accept-Encoding`/`Origin`/`Referer` header stripping.

#### Per-Provider Base URLs
- `anthropic_base_url`, `openai_base_url`, `gemini_base_url` on `Credentials`, `RuntimeConfig`, and `SessionConfig` in the WASM runtime.
- Backward-compatible: single `base_url` still works as fallback for the default model's provider.

#### MCP Server Loading
- `--wait-for-mcp` flag on `run`/`resume` blocks until all MCP servers finish connecting before first turn.
- Non-blocking parallel MCP server loading: servers connect in background, tools become available as each completes.
- Per-server `connect_timeout_secs` in `.rkat/mcp.toml` (default: 10s).
- `[MCP_PENDING]` system notice informs the LLM while servers are still connecting.

#### Mob Enhancements
- `SpawnMemberSpec.additional_instructions`: per-member system prompt additions, wired through `BuildAgentConfigParams` to `AgentBuildConfig`.
- `runtime_version()` wasm_bindgen export for JS/WASM version mismatch detection.

### Changed

- **Gemini auth**: use `x-goog-api-key` header instead of `?key=` query parameter in URL.
- **wasm32 clippy clean**: cfg-gated filesystem-only functions, removed dead imports, zero warnings with `-D warnings`.
- **CI**: added wasm32 clippy step to CI workflow.
- **Version parity**: web SDK added to `bump-sdk-versions.sh`, `verify-version-parity.sh` (6 files must now agree on version).
- **Release CI**: `@rkat/web` publish step added to release workflow, `meerkat-mob-pack` added to crate publish order.

### Fixed

- Backfill empty `Response.url` to prevent reqwest panic on wasm32 (#111).
- Proxy: strip `Accept-Encoding` from forwarded requests, `Content-Encoding`/`Transfer-Encoding` from responses (prevents `ERR_CONTENT_DECODING_FAILED`).
- Proxy: strip `Origin`/`Referer` headers to prevent Anthropic CORS rejection (#113).
- Gemini function schema lowering for type arrays and const values.
- Gemini `additionalProperties` normalization for nested schemas.
- Mob provider params propagation and Gemini reasoning deltas.
- Documentation accuracy audit: 20+ fixes across reference, guides, examples, and skills.

## [0.4.1] - 2026-02-28

### Added

#### WASM Browser Runtime
- `meerkat-web-runtime` crate with 25+ `wasm_bindgen` exports for browser deployment.
- 9 crates compile for `wasm32-unknown-unknown`: meerkat-store, meerkat-skills, meerkat-hooks, meerkat-comms, meerkat-tools, meerkat-session, meerkat (facade), meerkat-mob, meerkat-mob-mcp.
- Override-first resource injection: `AgentBuildConfig` accepts `tool_dispatcher_override`, `session_store_override`, `hook_engine_override`, `skill_engine_override` — bypasses filesystem resolution on wasm32.
- `AgentFactory::minimal()` filesystem-free factory constructor for browser environments.
- `CompositeDispatcher::new_wasm()` tool dispatcher without shell tools.
- `MobStorage::in_memory()` ephemeral mob storage for browser-hosted mobs.
- `FactoryAgentBuilder` default injection propagates wasm32-compatible resources to mob-spawned sessions automatically.
- Time compatibility layer (`meerkat_core::time_compat`) using `web-time` for `SystemTime`/`Instant` on wasm32.
- Anthropic client auto-adds `anthropic-dangerous-direct-browser-access` header on wasm32.

#### Tool Scoping and Runtime Tool Control
- `ToolScope` contract with `ToolFilter` enum (whitelist/blacklist) for per-turn tool visibility.
- Live MCP mutation: `mcp/add`, `mcp/remove`, `mcp/reload` RPC methods for runtime server provisioning.
- `tool_scope` field on `SessionBuildOptions` for session-wide tool visibility defaults.

#### Mob API Enhancements (Architecture Review)
- `Roster::session_id()` convenience accessor for direct session ID retrieval.
- `MobHandle::subscribe_agent_events()` per-member event subscription with point-in-time snapshots.
- `AttributedEvent` type for mob-level event sourcing with member attribution.
- Non-blocking `respawn` command (atomic retire + spawn).
- `SpawnPolicy` trait as extension point for auto-provisioning strategy on external turns.
- `MobEventRouter` independent async task merging per-member event streams.
- `inject_and_subscribe()` request-reply pattern for sync-like interactions over autonomous agents.

#### EventEnvelope Standardization
- Hard-cut canonical `EventEnvelope` contract: `{timestamp_ms, source_id, seq, event_id, payload}` across all surfaces.
- Strict `seq` ordering for replay and idempotency.
- Client-side malformed event guards with structured logging.

#### Schema Hardening
- Recursive `additionalProperties: false` injection at all nesting levels for Anthropic schema compliance.
- Handles arrays of objects, union types (`anyOf`), and deeply nested properties.
- Preserves user-provided `additionalProperties` settings.

#### Mobpack Archive Format
- `meerkat-mob-pack` crate: portable multi-agent deployment bundles.
- Pack/deploy/sign pipeline with Ed25519 signing, digest verification, and allowlist trust.
- WASM web build target support for browser deployment of mobpacks.

#### Documentation
- 10 new feature pages: sessions, tools, structured output, hooks, skills, memory, comms, mobs, mobpack, WASM.
- Examples gallery with 11 curated showcase applications.
- Universal surface tabs showing CLI, RPC, REST, MCP, Python, TypeScript, and Rust implementations.
- 20+ stale references fixed (version numbers, crate counts, CI descriptions, build matrix).

#### Examples
- Expanded to 31 polished examples (up from 27) covering all features and surfaces.
- All examples compile and exercise real features (validated in CI).
- WASM mini-diplomacy demo: 3-faction browser app with real mob orchestration.

#### CI/CD
- WASM compilation check job in CI workflow.
- `cargo fmt --all` auto-fix on pre-commit hook stage.
- `cargo build --workspace` added to pre-push hook stage.

### Changed
- CI parallelized to 8 jobs (~3.5 min): fmt-lint, test, test-minimal, test-feature-matrix-lib, test-feature-matrix-surface-checks, test-surface-smoke, audit, gate.
- Adopted `nextest` for faster parallel test execution across all CI jobs.
- Pedantic clippy with `-D warnings` enforced across full feature matrix.
- Toolchain updated to 1.93.1 with provenance tracking.
- Cross-surface parity: all 7 surfaces (CLI, RPC, REST, MCP, Python SDK, TypeScript SDK, Rust SDK) support EventEnvelope, tool scoping, live MCP controls, and mob feature-gating.
- Facade re-exports expanded: `ConfigStore`, `ConfigError`, `SessionServiceCommsExt`.
- CODEOWNERS file added for maintenance coverage.

### Fixed
- WASM32 runtime panics: `SystemTime`/`Instant` replaced with `web-time` shims, CORS headers for Anthropic, JSON schema validation restored.
- Mob spawn pipeline on WASM: `FactoryAgentBuilder` default injection ensures mob-spawned sessions inherit wasm32-compatible resources.
- Inproc comms enabled on wasm32 with override-first pattern.
- All 31 examples compile without warnings (non-exhaustive match, unused_mut, vec![] → array literals).
- Import ordering fixed for `cargo fmt` compliance across workspace.

## [0.4.0] - 2026-02-23

*(Historical cumulative detail: release stamping did not run for
v0.3.1-v0.3.4, so some provider and release-pipeline work below first shipped
in those tags. The release-local summaries below identify the first tagged
availability; this section preserves the original v0.4.0 notes.)*

### Added

#### Mobs (Multi-Agent Orchestration)
- Introduced first-class mob runtime (`meerkat-mob`) for built-in multi-agent orchestration.
- Added DAG-based flow engine with conditions, branching, fan-out/fan-in, and dependency-aware step execution.
- Added full mob lifecycle operations with in-memory and redb-backed persistence.
- Added parallel spawn provisioning/finalization paths to support large swarm initialization.
- Added autonomous-host default runtime mode with supervision and escalation behavior.
- Added dedicated mob MCP surface (`meerkat-mob-mcp`) and integrated mob tools into CLI run/resume workflows.
- **Consolidated mob tool surface from 19 → 12 tools** with clear mob-level (`mob_*`) and member-level (`meerkat_*`) taxonomy.
- **Gated mob tools behind opt-in `--enable-mob` / `-M` flag** (default: disabled). Mob tools no longer pollute regular agent sessions.
- Mob enablement persists in session metadata for deterministic resume.

#### CLI UX
- **Stdin pipe support**: `cat file.txt | rkat run "Summarize"` reads piped input as context. Supports chaining: `cat data | rkat run "Extract" | rkat run "Analyze"`.
- **Live event streaming**: `tail -f app.log | rkat run --host --stdin "Monitor"` reads stdin line-by-line as events (infinite pipes).
- **Default `run` subcommand**: `rkat "hello"` is equivalent to `rkat run "hello"`.
- **Compact session references**: `rkat resume last`, `rkat resume ~2`, `rkat resume 019c8b99` (short prefix, git-style).
- **`continue` command**: `rkat continue "keep going"` (alias: `rkat c`) resumes the most recent session.
- Session output shows compact 8-char ID instead of full UUID.

#### MobHandle SDK Renames
- `external_turn` → `send_message`, `list_meerkats` → `list_members`, `get_meerkat` → `get_member`.
- `spawn_member_ref*` → `spawn` / `spawn_with_backend` / `spawn_with_options`.
- `spawn_many_member_refs` → `spawn_many`. `spawn()` now returns `MemberRef` (old `SessionId` compat path removed).

#### Skills v2.1
- Added strict source-pinned skill identity model (`SkillKey`) and structured skill refs.
- Enforced explicit model-mediated skill activation (`load_skill`) and removed legacy fuzzy fallback behavior.
- Brought CLI and SDK parity for skills v2.1 references and diagnostics.

#### Docs and Examples
- Added comprehensive examples library (27 examples) covering providers, hooks, memory, comms, and mob coordination.
- Added design philosophy reference and updated architecture summaries.
- Rewrote README for clearer platform/surface positioning.

### Changed
- Added config CLI CAS UX (`--expected-generation`) and generation-aware responses.
- Added configurable compaction settings to config surface and runtime wiring.
- Documented and tested config merge/override semantics across scalar/list/map fields.
- Added deferred initial-turn policy support for session creation, used by mob spawns.
- Added session-wide event stream API parity across services/surfaces.
- TypeScript SDK package renamed from `@meerkat/sdk` to `@rkat/sdk`.
- Release pipeline now publishes all 18 Rust crates (added `meerkat-mob`, `meerkat-mob-mcp`, `rkat`).

### Fixed
- Fixed OpenAI streaming duplicate/replay edge cases and strengthened error mapping.
- Fixed skills source fallback and identity resolution bugs across CLI/SDK surfaces.
- Fixed multiple mob lifecycle correctness issues (ordering races, shutdown/startup sequencing, duplicate wire side-effects).
- Stabilized host-mode and provider-agnostic integration tests.
- Addressed workspace clippy blockers and CI/push gate regressions.
- Fixed release tooling portability (`sed`-portable hook path) and lock-step contract/package version handling.

## [0.3.4] - 2026-02-17

This tag was cut from the v0.3.2 mainline rather than from the sibling v0.3.3
tag, so its comparison link uses v0.3.2 as the exact ancestry base.

### Breaking

- The `meerkat` facade default feature set changed from Anthropic plus bundled
  subsystems to provider-only Anthropic, OpenAI, and Gemini. Storage, comms,
  MCP, sub-agent, and skill support now require explicit features.
- The Rust minimum supported version increased from 1.85 to 1.89.0.

### Added

- Added cross-platform release builds, registry dry-run and recovery flows,
  checksums and asset manifests, and Python and TypeScript SDK binary
  auto-download.

### Changed

- Server surfaces now compile all three default providers, and Unix-only comms
  mechanics are configuration-gated for cross-platform builds.

### Fixed

- Fixed OpenAI stream deduplication, replay, terminal, and error handling;
  fixed Anthropic missing terminal and streaming-error handling.
- Fixed Windows packaging, `WireRunResult.session_ref` SDK code generation,
  already-published registry recovery, and release-hook portability.

## [0.3.3] - 2026-02-16

### Breaking

- Session, config, backend, skills, hooks, comms, and sub-agent state became
  realm-scoped. Session references are realm-qualified, and config reads return
  the realm envelope rather than an unscoped config value.
- The `rkat rpc` subcommand was removed in favor of the dedicated `rkat-rpc`
  binary. TypeScript streaming types removed the unsent `sequence` and
  `contract_version` fields.

### Added

- Added realm manifests, backend pinning, lifecycle commands, isolation, and
  release-distribution workflow scaffolding.

### Changed

- REST, MCP, and RPC use persistent sessions by default, and RPC host-mode
  turns run in the background so the command plane remains responsive.

## [0.3.2] - 2026-02-15

This tag was cut from the v0.3.0 mainline rather than from the sibling v0.3.1
tag, so its comparison link uses v0.3.0 as the exact ancestry base.

### Fixed

- Removed orphaned OpenAI reasoning items before replay so Responses requests
  are not rejected.
- Enabled crate publication and made the release hook workspace-rooted and
  idempotent within one version.

## [0.3.1] - 2026-02-15

### Added

- Added `PeerMeta` descriptions and friendly labels across comms discovery,
  CLI, REST, MCP, RPC, Python, and TypeScript surfaces.

### Fixed

- Corrected the host-mode assertion and regenerated versioned schemas and SDK
  artifacts for the release.

## [0.3.0] - 2026-02-14

### Added

#### Comms Command Plane Redesign
- Canonical `send` and `peers` tools replacing 4 legacy tools (`send`, `send_request`, `send_response`, `peers`)
  - `send`: unified command dispatch with flat `kind` discriminator for all comms operations
  - `peers`: list all visible peers
- `comms/send` and `comms/peers` RPC methods with flat-schema validation
- `POST /comms/send` and `GET /comms/peers` REST endpoints
- Python SDK methods: `send()` and `peers()` replacing `push_event()`
- TypeScript SDK methods: `send()`, `send_and_stream()`, and `peers()` replacing `pushEvent()`
- Optional peer authentication with fallback to in-process peer context
- `TrustedAndInproc` trust source for hybrid peer resolution

#### Interaction-Scoped Event Streaming
- `EventTap` mechanism for scoped event subscription per interaction
- `SubscribableInjector` extending `EventInjector` with `inject_with_subscription()` for dedicated interaction streams
- `InteractionSubscription`, `InteractionId`, `InteractionContent`, and `ResponseStatus` types in meerkat-core
- Host-mode interaction FSM with scoped event delivery
- Terminal completion events (`InteractionComplete`, `InteractionFailed`) for stream lifecycle management

#### CD Infrastructure
- Version parity verification: `make verify-version-parity` enforces Rust workspace, Python SDK, TypeScript SDK, and contract version alignment as a CI gate
- Schema freshness check: `make verify-schema-freshness` detects stale committed schema artifacts
- `cargo-release` configuration with pre-release hook that bumps SDK versions, regenerates schemas, and verifies parity
- Release scripts: `scripts/verify-version-parity.sh`, `scripts/bump-sdk-versions.sh`, `scripts/release-hook.sh`, `scripts/verify-schema-freshness.sh`
- `make regen-schemas` target for re-emitting schemas and running SDK codegen
- `make release-preflight` for full pre-release checklist (CI + schema freshness)
- `make publish-dry-run` for cargo publish readiness checks across all crates

### Changed

#### Versioning (Breaking)
- Package version and contract version are now lock-stepped (both `0.3.0`)
- Contract version bumped to `0.3.0` reflecting comms API changes
- All schema artifacts and SDK generated types regenerated for contract version `0.3.0`

#### Comms API (Breaking)
- Comms tools reduced from 4 to 2: `send` (with `kind` discriminator) and `peers`
- RPC: `event/push` removed, replaced by `comms/send`
- REST: `POST /sessions/{id}/event` deprecated in favor of `POST /comms/send`
- SDK: `push_event()`/`pushEvent()` removed; use `send()`/`peers()` instead

#### Host Mode
- Strict state transitions via `.transition()` instead of raw assignment
- Interaction processing classified into individual vs batched modes
- Host drain state reset on all exit paths

#### Dependencies
- Removed vendored `hnsw_rs` (was unmodified upstream v0.3.3); now resolved from crates.io
- `verify-version-parity` wired into `make ci` pipeline

### Fixed
- `ToolUse` args deserialization robust under Message buffering with custom deserializer
- Idempotent `stream_close` preventing duplicate close errors
- Comms stream completion cleanup preventing reservation leaks
- Comms self-input guard preventing agents from responding to their own messages
- E2E test model names updated to canonical providers (`gpt-5.2`, `gemini-3-pro-preview`)
- Clippy fix: `.or_insert_with(Vec::new)` → `.or_default()` in `SessionProjector`

### Removed
- 4 legacy comms tools (`send`, `send_request`, `send_response`, `peers`) -- now return "Unknown tool"
- `event/push` RPC method
- `push_event()`/`pushEvent()` SDK methods
- `vendor/hnsw_rs/` directory and `[patch.crates-io]` section

## [0.2.0] - 2026-02-12

### Added

#### Contracts and Capabilities
- `meerkat-contracts` crate: single source of truth for all wire-facing types, capability model, error contracts, and schema emission
- `CapabilityId` enum with distributed `inventory`-based registration across feature-gated crates
- `CapabilityStatus` (Available, DisabledByPolicy, NotCompiled, NotSupportedByProtocol) for runtime status
- `WireError` canonical error envelope with `ErrorCode` projections to JSON-RPC codes, HTTP status, and CLI exit codes
- `ContractVersion` with semver compatibility checking (currently 0.1.0)
- Composable request fragments: `CoreCreateParams`, `StructuredOutputParams`, `CommsParams`, `HookParams`, `SkillsParams`
- Wire response types: `WireUsage`, `WireRunResult`, `WireEvent`, `WireSessionInfo`, `WireSessionSummary`
- Feature-gated `JsonSchema` derives on all wire types
- `emit-schemas` binary for deterministic schema artifact generation (`artifacts/schemas/`)
- `capabilities/get` endpoint on all four surfaces (CLI, REST, MCP Server, JSON-RPC)

#### Skills System
- `meerkat-skills` crate with skill sources (filesystem, embedded, in-memory, composite), parser, resolver, renderer, and engine
- Core skill contracts in `meerkat-core/src/skills/`: `SkillId`, `SkillScope`, `SkillDescriptor`, `SkillDocument`, `SkillError`, `SkillSource` and `SkillEngine` traits
- 8 embedded skills: `task-workflow`, `shell-patterns`, `sub-agent-orchestration` (legacy name), `multi-agent-comms`, `mcp-server-setup`, `hook-authoring`, `memory-retrieval`, `session-management`
- Skill inventory section injected into system prompt via `extra_sections` slot
- Per-turn skill injection via `<skill>` tagged blocks prepended to user messages
- `SkillsResolved` and `SkillResolutionFailed` agent events
- Filesystem skill sources: `.rkat/skills/` (project) and `~/.rkat/skills/` (user)

#### Python and TypeScript SDKs
- SDK codegen pipeline (`tools/sdk-codegen/`) reading from `artifacts/schemas/`
- Python SDK (`sdks/python/`): async MeerkatClient with subprocess lifecycle, capability gating, version checks
- TypeScript SDK (`sdks/typescript/`): MeerkatClient with subprocess lifecycle, capability gating, version checks
- Generated types committed (Python: dataclasses, TypeScript: interfaces)
- SDK error types: `MeerkatError`, `CapabilityUnavailableError`, `SessionNotFoundError`, `SkillNotFoundError`
- Python conformance tests (8 type/error tests)

#### SDK Builder
- Builder tool (`tools/sdk-builder/build.py`): resolves features, builds runtime, emits schemas, runs codegen, emits bundle manifest
- Profile presets: `profiles/minimal.toml`, `profiles/standard.toml`, `profiles/full.toml`
- Bundle manifest with source commit, features, contract version, hashes, timestamp

#### Hooks System
- `meerkat-hooks` crate with `DefaultHookEngine`
- 3 hook runtimes: in-process (Rust handlers), command (stdin/stdout JSON), HTTP (remote endpoints)
- 8 hook points: `run_started`, `run_completed`, `run_failed`, `pre_llm_request`, `post_llm_response`, `pre_tool_execution`, `post_tool_execution`, `turn_boundary`
- Guardrail semantics: first deny short-circuits, deny always wins over allow
- Patch semantics: foreground patches applied in `(priority ASC, registration_index ASC)` order
- Background hooks with observe-only pre-hooks and `HookPatchEnvelope` post-hooks
- Failure policies: observe defaults to fail-open, guardrail/rewrite default to fail-closed
- Per-run hook overrides via `HookRunOverrides` (add entries, disable hooks)

#### Legacy Sub-Agent Surface (pre-0.5)
- `agent_spawn` and `agent_fork` tools for parallel delegated child work
- `agent_status`, `agent_cancel`, `agent_list` management tools
- `SubAgentManager` with concurrency limits, nesting depth control, and budget allocation
- `ContextStrategy` for spawn context: `FullHistory`, `LastTurns(n)`, `Summary`, `Custom`
- `ToolAccessPolicy`: `Inherit`, `AllowList`, `DenyList` for delegated child-agent tool filtering
- `ForkBudgetPolicy`: `EqualSplit`, `Proportional`, `Fixed` for budget allocation
- Model allowlists per provider for delegated child-agent spawns

#### Comms (Inter-Agent Communication)
- `meerkat-comms` crate with `Router`, `Inbox`, `InprocRegistry`
- 3 transport backends: Unix Domain Sockets (UDS), TCP, in-process
- `Keypair`/`PubKey`/`Signature` identity system with Ed25519
- `TrustedPeers` trust model with peer verification
- `Envelope` wire format with `MessageKind` variants: `Message`, `Request`, `Response`, `Ack`
- Comms tools: `comms_send`, `comms_request`, `comms_response`, `comms_peers`
- Host mode for long-running agents that process comms messages

#### Memory and Compaction
- `meerkat-memory` crate with `HnswMemoryStore` (hnsw_rs + redb)
- `SimpleMemoryStore` for testing
- `MemoryStore` trait in meerkat-core: `index`, `search`, similarity scoring
- `memory_search` builtin tool for agent access to semantic memory
- Memory indexing of compaction discards wired into agent loop
- `DefaultCompactor` in meerkat-session: auto-compact at token threshold, LLM summary, history rebuild
- `CompactionConfig` for threshold tuning

#### Structured Output
- `OutputSchema` type with `MeerkatSchema`, name, strict mode, compat, and format options
- Schema validation and retry logic for structured output
- `SchemaWarning` for compilation issues
- Provider-specific schema adaptation (Anthropic, OpenAI, Gemini)

#### Session Management
- `SessionService` trait in meerkat-core: create, turn, interrupt, read, list, archive
- `EphemeralSessionService` (in-memory) and `PersistentSessionService` (redb-backed)
- `RedbEventStore` append-only event log
- `SessionProjector` materializing `.rkat/sessions/` files from events
- `RedbSessionStore` for session persistence
- All four surfaces (CLI, REST, MCP Server, JSON-RPC) route through `SessionService`

#### JSON-RPC Server
- `meerkat-rpc` crate with JSON-RPC 2.0 over JSONL stdin/stdout
- `SessionRuntime`: stateful agent manager with dedicated tokio tasks per session
- Methods: `initialize`, `session/create`, `session/list`, `session/read`, `session/archive`, `turn/start`, `turn/interrupt`, `config/get`, `config/set`, `config/patch`
- `session/event` notifications with `AgentEvent` payload during turns

#### Builtin Tools
- Task management: `task_create`, `task_update`, `task_get`, `task_list`
- Shell execution: `shell` (Nushell backend), `shell_jobs`, `shell_job_status`, `shell_job_cancel`
- Utility: `wait`, `datetime`
- Three-tier tool policy: `ToolPolicyLayer` soft policies, `EnforcedToolPolicy` hard constraints, per-tool `default_enabled()`

#### MCP Server Capabilities
- `meerkat-mcp-server` crate exposing `meerkat_run` and `meerkat_resume` as MCP tools
- `McpRouterAdapter` relocated from CLI to `meerkat-mcp` for all surfaces

#### Build Profiles
- Profile presets for controlling feature composition: `profiles/minimal.toml`, `profiles/standard.toml`, `profiles/full.toml`
- Profiles drive SDK builder feature resolution and bundle manifests

#### E2E Tests
- 21-scenario E2E smoke test suite across 5 surfaces (CLI, REST, MCP Server, RPC, SDK)
- Integration-real tests for process spawning and live APIs
- Fast test suite gating for CI (unit + integration-fast, skipping doctests)
- Kitchen-sink compound RPC test replacing mock-only coverage

#### Prompt Assembly
- Unified `assemble_system_prompt` with documented precedence: per-request override > config file > config inline > default + AGENTS.md
- `extra_sections` slot for skill inventory injection
- Config fields `agent.system_prompt`, `agent.system_prompt_file`, `agent.tool_instructions` fully wired

### Changed
- Project renamed from "raik" to "Meerkat" with CLI binary `rkat`
- `AgentFactory::build_agent()` is now the centralized agent construction pipeline for all surfaces
- `FactoryAgentBuilder` bridges `AgentFactory` into `SessionAgentBuilder` trait
- All wire types consolidated into `meerkat-contracts` (removed per-surface duplicates)
- Error handling unified via `WireError` with protocol-specific projections
- Helper functions deduplicated: `resolve_host_mode()` to meerkat-comms, `resolve_store_path()` to meerkat-store, `spawn_event_forwarder()` to facade
- OpenAI and Gemini added to default CLI features
- Test infrastructure stabilized: fast test target isolation, real E2E gating, pre-commit hook fixes for bin-only crates

### Changed - Feature defaults
- `meerkat-tools`: comms, mcp, and the legacy `sub-agents` compatibility feature are now optional features (default: on)
  - `--no-default-features` builds tools with zero optional deps
  - Features: `comms`, `mcp`, `sub-agents`
- `meerkat` facade: comms, mcp, and the legacy `sub-agents` compatibility feature are now optional features (default: on)
  - Features: `comms`, `mcp`, `sub-agents`
- `meerkat-rpc`: comms, mcp, mob, and the legacy `sub-agents` compatibility feature are optional features (default: on)
  - `--no-default-features` builds the minimal server surface
  - Features: `comms`, `mcp`, `mob`, `sub-agents`
- `meerkat-rest`: comms is opt-in (default: on), no comms code when disabled
- `meerkat-mcp-server`: comms is opt-in (default: on), no comms code when disabled
- `meerkat-cli`: comms and mcp are opt-in (default: on), all inline code cfg-gated
- `agent_spawn` tool: `host_mode` field removed from schema when comms feature is off

### Fixed
- Anthropic streaming: emit `ToolCallComplete` on `content_block_stop`
- SDK E2E tests: session list uses `session_id` not `id`
- Python SDK async issues and TypeScript SDK brought to feature parity
- `active_skill_ids` now collects from all skill sources (not just embedded)
- SDK builder memory-store feature resolution
- SDK builder feature forwarding and dead `usage_instructions` removal
- `CapabilityStatus` parsing in SDKs and `contract_version` field inclusion
- RPC `session/create` expanded to full `AgentBuildConfig` parity
- Provider schema lowering moved from core to adapters, removing provider leakage
- `thought_signature` removed from generic `ToolCall`/`ToolResult` (provider-specific only)
- Config-driven delegated child-agent compatibility policy with fail-closed validation
- Legacy `sub-agents` compatibility, comms, and memory enabled through RPC/SDK surfaces

### Removed
- Dead files in meerkat-core: `comms_runtime.rs`, `comms_bootstrap.rs`, `comms_config.rs`, `agent/comms.rs`
- Duplicate `LlmClientAdapter`/`DynLlmClientAdapter` in meerkat-tools (uses canonical from meerkat-client)
- Per-surface wire type definitions (replaced by `meerkat-contracts`)
- Duplicated helper functions across surface crates

## [0.1.0] - 2026-01-15

Initial development release.

[Unreleased]: https://github.com/lukacf/meerkat/compare/v0.8.33...HEAD
[0.8.33]: https://github.com/lukacf/meerkat/compare/v0.8.32...v0.8.33
[0.8.32]: https://github.com/lukacf/meerkat/compare/v0.8.31...v0.8.32
[0.8.31]: https://github.com/lukacf/meerkat/compare/v0.8.30...v0.8.31
[0.8.30]: https://github.com/lukacf/meerkat/compare/v0.8.29...v0.8.30
[0.8.29]: https://github.com/lukacf/meerkat/compare/v0.8.28...v0.8.29
[0.8.28]: https://github.com/lukacf/meerkat/compare/v0.8.27...v0.8.28
[0.8.27]: https://github.com/lukacf/meerkat/compare/v0.8.26...v0.8.27
[0.8.26]: https://github.com/lukacf/meerkat/compare/v0.8.25...v0.8.26
[0.8.25]: https://github.com/lukacf/meerkat/compare/v0.8.24...v0.8.25
[0.8.24]: https://github.com/lukacf/meerkat/compare/v0.8.23...v0.8.24
[0.8.23]: https://github.com/lukacf/meerkat/compare/v0.8.22...v0.8.23
[0.8.22]: https://github.com/lukacf/meerkat/compare/v0.8.21...v0.8.22
[0.8.21]: https://github.com/lukacf/meerkat/compare/v0.8.20...v0.8.21
[0.8.20]: https://github.com/lukacf/meerkat/compare/v0.8.18...v0.8.20
[0.8.18]: https://github.com/lukacf/meerkat/compare/v0.8.17...v0.8.18
[0.8.17]: https://github.com/lukacf/meerkat/compare/v0.8.16...v0.8.17
[0.8.16]: https://github.com/lukacf/meerkat/compare/v0.8.15...v0.8.16
[0.8.15]: https://github.com/lukacf/meerkat/compare/v0.8.14...v0.8.15
[0.8.14]: https://github.com/lukacf/meerkat/compare/v0.8.13...v0.8.14
[0.8.13]: https://github.com/lukacf/meerkat/compare/v0.8.12...v0.8.13
[0.8.12]: https://github.com/lukacf/meerkat/compare/v0.8.11...v0.8.12
[0.8.11]: https://github.com/lukacf/meerkat/compare/v0.8.10...v0.8.11
[0.8.10]: https://github.com/lukacf/meerkat/compare/v0.8.9...v0.8.10
[0.8.9]: https://github.com/lukacf/meerkat/compare/v0.8.8...v0.8.9
[0.8.8]: https://github.com/lukacf/meerkat/compare/v0.8.7...v0.8.8
[0.8.7]: https://github.com/lukacf/meerkat/compare/v0.8.6...v0.8.7
[0.8.6]: https://github.com/lukacf/meerkat/compare/v0.8.5...v0.8.6
[0.8.5]: https://github.com/lukacf/meerkat/compare/v0.8.4...v0.8.5
[0.8.4]: https://github.com/lukacf/meerkat/compare/v0.8.3...v0.8.4
[0.8.3]: https://github.com/lukacf/meerkat/compare/v0.8.2...v0.8.3
[0.8.2]: https://github.com/lukacf/meerkat/compare/v0.8.1...v0.8.2
[0.8.1]: https://github.com/lukacf/meerkat/compare/v0.8.0...v0.8.1
[0.8.0]: https://github.com/lukacf/meerkat/compare/v0.7.31...v0.8.0
[0.7.31]: https://github.com/lukacf/meerkat/compare/v0.7.30...v0.7.31
[0.7.30]: https://github.com/lukacf/meerkat/compare/v0.7.29...v0.7.30
[0.7.29]: https://github.com/lukacf/meerkat/compare/v0.7.28...v0.7.29
[0.7.28]: https://github.com/lukacf/meerkat/compare/v0.7.27...v0.7.28
[0.7.27]: https://github.com/lukacf/meerkat/compare/v0.7.26...v0.7.27
[0.7.26]: https://github.com/lukacf/meerkat/compare/v0.7.25...v0.7.26
[0.7.25]: https://github.com/lukacf/meerkat/compare/v0.7.24...v0.7.25
[0.7.24]: https://github.com/lukacf/meerkat/compare/v0.7.23...v0.7.24
[0.7.23]: https://github.com/lukacf/meerkat/compare/v0.7.22...v0.7.23
[0.7.22]: https://github.com/lukacf/meerkat/compare/v0.7.21...v0.7.22
[0.7.21]: https://github.com/lukacf/meerkat/compare/v0.7.20...v0.7.21
[0.7.20]: https://github.com/lukacf/meerkat/compare/v0.7.19...v0.7.20
[0.7.19]: https://github.com/lukacf/meerkat/compare/v0.7.18...v0.7.19
[0.7.18]: https://github.com/lukacf/meerkat/compare/v0.7.17...v0.7.18
[0.7.17]: https://github.com/lukacf/meerkat/compare/v0.7.16...v0.7.17
[0.7.16]: https://github.com/lukacf/meerkat/compare/v0.7.15...v0.7.16
[0.7.15]: https://github.com/lukacf/meerkat/compare/v0.7.14...v0.7.15
[0.7.14]: https://github.com/lukacf/meerkat/compare/v0.7.13...v0.7.14
[0.7.13]: https://github.com/lukacf/meerkat/compare/v0.7.12...v0.7.13
[0.7.12]: https://github.com/lukacf/meerkat/compare/v0.7.11...v0.7.12
[0.7.11]: https://github.com/lukacf/meerkat/compare/v0.7.10...v0.7.11
[0.7.10]: https://github.com/lukacf/meerkat/compare/v0.7.9...v0.7.10
[0.7.9]: https://github.com/lukacf/meerkat/compare/v0.7.8...v0.7.9
[0.7.8]: https://github.com/lukacf/meerkat/compare/v0.7.7...v0.7.8
[0.7.7]: https://github.com/lukacf/meerkat/compare/v0.7.6...v0.7.7
[0.7.6]: https://github.com/lukacf/meerkat/compare/v0.7.5...v0.7.6
[0.7.5]: https://github.com/lukacf/meerkat/compare/v0.7.4...v0.7.5
[0.7.4]: https://github.com/lukacf/meerkat/compare/v0.7.3...v0.7.4
[0.7.3]: https://github.com/lukacf/meerkat/compare/v0.7.2...v0.7.3
[0.7.2]: https://github.com/lukacf/meerkat/compare/v0.7.1...v0.7.2
[0.7.1]: https://github.com/lukacf/meerkat/compare/v0.7.0...v0.7.1
[0.7.0]: https://github.com/lukacf/meerkat/compare/alpha/v0.7.0-alpha.0...v0.7.0
[0.7.0-alpha.0]: https://github.com/lukacf/meerkat/releases/tag/alpha/v0.7.0-alpha.0
[0.6.34]: https://github.com/lukacf/meerkat/compare/v0.6.33...v0.6.34
[0.6.33]: https://github.com/lukacf/meerkat/compare/v0.6.32...v0.6.33
[0.6.32]: https://github.com/lukacf/meerkat/compare/v0.6.31...v0.6.32
[0.6.31]: https://github.com/lukacf/meerkat/compare/v0.6.30...v0.6.31
[0.6.30]: https://github.com/lukacf/meerkat/compare/v0.6.29...v0.6.30
[0.6.29]: https://github.com/lukacf/meerkat/compare/v0.6.28...v0.6.29
[0.6.28]: https://github.com/lukacf/meerkat/compare/v0.6.27...v0.6.28
[0.6.27]: https://github.com/lukacf/meerkat/compare/v0.6.26...v0.6.27
[0.6.26]: https://github.com/lukacf/meerkat/compare/v0.6.25...v0.6.26
[0.6.25]: https://github.com/lukacf/meerkat/compare/v0.6.24...v0.6.25
[0.6.24]: https://github.com/lukacf/meerkat/compare/v0.6.23...v0.6.24
[0.6.23]: https://github.com/lukacf/meerkat/compare/v0.6.22...v0.6.23
[0.6.22]: https://github.com/lukacf/meerkat/compare/v0.6.21...v0.6.22
[0.6.21]: https://github.com/lukacf/meerkat/compare/v0.6.20...v0.6.21
[0.6.20]: https://github.com/lukacf/meerkat/compare/v0.6.19...v0.6.20
[0.6.19]: https://github.com/lukacf/meerkat/compare/v0.6.18...v0.6.19
[0.6.18]: https://github.com/lukacf/meerkat/compare/v0.6.17...v0.6.18
[0.6.17]: https://github.com/lukacf/meerkat/compare/v0.6.16...v0.6.17
[0.6.16]: https://github.com/lukacf/meerkat/compare/v0.6.15...v0.6.16
[0.6.15]: https://github.com/lukacf/meerkat/compare/v0.6.14...v0.6.15
[0.6.14]: https://github.com/lukacf/meerkat/compare/v0.6.13...v0.6.14
[0.6.13]: https://github.com/lukacf/meerkat/compare/v0.6.12...v0.6.13
[0.6.12]: https://github.com/lukacf/meerkat/compare/v0.6.11...v0.6.12
[0.6.11]: https://github.com/lukacf/meerkat/compare/v0.6.10...v0.6.11
[0.6.10]: https://github.com/lukacf/meerkat/compare/v0.6.9...v0.6.10
[0.6.9]: https://github.com/lukacf/meerkat/compare/v0.6.8...v0.6.9
[0.6.8]: https://github.com/lukacf/meerkat/compare/v0.6.7...v0.6.8
[0.6.7]: https://github.com/lukacf/meerkat/compare/v0.6.6...v0.6.7
[0.6.6]: https://github.com/lukacf/meerkat/compare/v0.6.5...v0.6.6
[0.6.5]: https://github.com/lukacf/meerkat/compare/v0.6.4...v0.6.5
[0.6.4]: https://github.com/lukacf/meerkat/compare/v0.6.3...v0.6.4
[0.6.3]: https://github.com/lukacf/meerkat/compare/v0.6.2...v0.6.3
[0.6.2]: https://github.com/lukacf/meerkat/compare/v0.6.1...v0.6.2
[0.6.1]: https://github.com/lukacf/meerkat/compare/v0.6.0...v0.6.1
[0.6.0]: https://github.com/lukacf/meerkat/compare/v0.5.2...v0.6.0
[0.5.2]: https://github.com/lukacf/meerkat/compare/v0.5.1...v0.5.2
[0.5.1]: https://github.com/lukacf/meerkat/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/lukacf/meerkat/compare/v0.4.9...v0.5.0
[0.4.13]: https://github.com/lukacf/meerkat/compare/v0.4.12...v0.4.13
[0.4.12]: https://github.com/lukacf/meerkat/compare/v0.4.11...v0.4.12
[0.4.11]: https://github.com/lukacf/meerkat/compare/v0.4.10...v0.4.11
[0.4.10]: https://github.com/lukacf/meerkat/compare/v0.4.9...v0.4.10
[0.4.9]: https://github.com/lukacf/meerkat/compare/v0.4.8...v0.4.9
[0.4.8]: https://github.com/lukacf/meerkat/compare/v0.4.7...v0.4.8
[0.4.7]: https://github.com/lukacf/meerkat/compare/v0.4.6...v0.4.7
[0.4.6]: https://github.com/lukacf/meerkat/compare/v0.4.5...v0.4.6
[0.4.5]: https://github.com/lukacf/meerkat/compare/v0.4.4...v0.4.5
[0.4.4]: https://github.com/lukacf/meerkat/compare/v0.4.2...v0.4.4
[0.4.2]: https://github.com/lukacf/meerkat/compare/v0.4.1...v0.4.2
[0.4.1]: https://github.com/lukacf/meerkat/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/lukacf/meerkat/compare/v0.3.0...v0.4.0
[0.3.4]: https://github.com/lukacf/meerkat/compare/v0.3.2...v0.3.4
[0.3.3]: https://github.com/lukacf/meerkat/compare/v0.3.2...v0.3.3
[0.3.2]: https://github.com/lukacf/meerkat/compare/v0.3.0...v0.3.2
[0.3.1]: https://github.com/lukacf/meerkat/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/lukacf/meerkat/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/lukacf/meerkat/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/lukacf/meerkat/releases/tag/v0.1.0
