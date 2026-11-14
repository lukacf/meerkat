---
name: meerkat-wasm
description: "Developer guide for the Meerkat WASM embedded runtime (meerkat-web-runtime) and the `@rkat/web` browser SDK. Covers wasm32 compilation, building/deploying WASM bundles, writing/auditing wasm_bindgen exports (sessions, mobs, comms, JS tool callbacks, external auth resolver, subscription handles), the auth_binding + registerExternalAuthResolver auth model, tokio_with_wasm aliasing, cfg-gating patterns, mobpack bootstrap, and common wasm32 gotchas. Use when touching meerkat-web-runtime, sdks/web, browser auth flows, or making a meerkat crate wasm32-compatible."
---

# Meerkat WASM Embedded Runtime

The `meerkat-web-runtime` crate compiles the full meerkat agent stack to `wasm32-unknown-unknown`. It is an embedded deployment target for mobpacks — the JavaScript equivalent of the Rust SDK. Not a protocol server like RPC/REST.

The crate is not published to crates.io (`publish = false`); browser hosts
consume it as the prebuilt WASM inside `@rkat/web`.

## Crate Compatibility Pattern

To make a meerkat crate compile for wasm32, apply this pattern:

**Cargo.toml** — gate platform-specific deps:
```toml
[target.'cfg(not(target_arch = "wasm32"))'.dependencies]
tokio = { workspace = true }
rusqlite = { workspace = true }

[target.'cfg(target_arch = "wasm32")'.dependencies]
tokio_with_wasm = { workspace = true }
```

**lib.rs** - tokio alias + module gating:
```rust
#[cfg(target_arch = "wasm32")]
pub mod tokio {
    pub use meerkat_core::time_compat::wasm as time;
    pub use meerkat_core::tokio::spawn;
    pub use meerkat_core::tokio::task;
    pub use tokio_with_wasm::alias::*;
}

#[cfg(not(target_arch = "wasm32"))]
mod filesystem_module;
```

**async_trait** — dual cfg_attr:
```rust
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
```

**Time types** — use `meerkat_core::time_compat::{SystemTime, Instant, Duration}`, never `std::time::*`.

**Tokio imports** in wasm32-visible code — add `#[cfg(target_arch = "wasm32")] use crate::tokio;` then existing `use tokio::...` paths resolve through the alias.

Use the shared core alias for WASM scheduling. Its time adapter owns the exact
JavaScript timer handle and cancels it when the Rust future is dropped. Direct
`tokio_with_wasm::alias::time` calls bypass that cleanup. The shared task alias
also implements `yield_now` through the same cancellable timer. Its `spawn` and
`task::JoinSet` box every future before handing it to `tokio_with_wasm`, which
keeps spawn-wrapper shadow-stack frames small; route spawns through the alias,
not `tokio_with_wasm::spawn`. The SDK's release build fails when a spawn-wrapper
frame exceeds 8 KiB (`sdks/web/scripts/wasm-frames.mjs`). Native code continues
to use Tokio. Lifecycle tests must exit naturally after teardown; forcing a
successful Node exit hides retained timer resources.

## Override-First Resource Injection

Browser bootstrap composes `AgentFactory::minimal().builtins(true)` with
`with_browser_runtime_profile()`; on wasm32 it also registers
`WasmExternalAuthResolver` under the `wasm_host` resolver id
(`with_external_auth_resolver`). `FactoryAgentBuilder` injects the browser tool
dispatcher and a `StoreAdapter` backed by `MemoryStore` into direct and mob
builds. The shared profile validates requested capabilities before construction.

| Resource | Browser composition |
|----------|---------------------|
| Tool dispatcher | `CompositeDispatcher::new_wasm()` plus runtime-registered JS tools |
| Session store | `StoreAdapter` backed by `meerkat_store::MemoryStore` |
| Hooks | Configured entries and hook-engine overrides are excluded by the profile |
| Skills | Factory-owned embedded sources; custom engines and repository discovery are excluded |

Embedded skill selection uses canonical `SkillKey` values. The profile permits
the builtin source and rejects external sources with `CAPABILITY_UNAVAILABLE`.
Resource injection does not bypass profile enforcement.

## Runtime mode on wasm32

WASM is a runtime-backed surface. Direct sessions and mob members share the
cached `MeerkatMachine` adapter and use canonical `SessionOwned` bindings:

```text
WASM -> MeerkatMachine -> prepared runtime bindings -> SessionOwned -> SessionService -> AgentFactory
```

Use `meerkat::surface::materialize_ephemeral_runtime_session` for direct
creation and the shared runtime turn, interrupt, run-fenced stop
(`MeerkatMachine::stop_run`), and peer-wiring helpers for later operations.
Keep exact registration custody through actor attachment and
cancellation cleanup. Browser handle maps and subscriptions are private
mechanics; they never determine admission, lifecycle, or terminality.

`BrowserRuntimeProfile` in `meerkat-capabilities` owns browser limitations.
In-memory storage and page-lifetime execution do not change runtime authority.
Direct `keep_alive: true` sessions with `comms_name` support runtime peer ingress
and comms drain. Never construct `StandaloneEphemeral` on this surface.

Await bootstrap, session creation, state reads, interruption, and destruction.
The SDK's `Session.sessionId` is also a promise. Turn results and session state
project the generated canonical contracts; no browser-only status or identity
fields belong in those results.

## `@rkat/web` npm Package

The `sdks/web/` directory contains `@rkat/web` — a TypeScript wrapper around the wasm_bindgen exports with an idiomatic camelCase API. Ships with the pre-built WASM binary and a Node.js provider proxy.

**Key classes:** `MeerkatRuntime`, `Mob`, `Member` (from `mob.member(id)`), `Session`, `EventSubscription`, and `MeerkatError` (typed `code`, `message`, `data` from a rejected WASM call). Auth helpers live in `sdks/web/src/auth.ts` (`registerExternalAuthResolver`, `clearExternalAuthResolver`, `withAuthBinding`, `WASM_EXTERNAL_AUTH_RESOLVER_HANDLE`, lease and failure types). Its `Auth` class is a typed `auth/*` RPC client over a caller-supplied `AuthTransport` for RPC/REST hosts; it does not drive the WASM runtime.

Current release-line notes: `runtime_version()` and package compatibility
should match the workspace `Cargo.toml` version (the current release line is
`0.9.x`; do not hardcode a stale patch pin). Without an explicit `model`,
browser bootstrap uses the catalog default of the highest-priority configured
provider (Anthropic, then OpenAI, then Gemini) and reports it as
`InitResult.model`. The OpenAI and global catalog default is `gpt-6-astra`.
Confirm the proxy's API organization has access; otherwise select `gpt-5.5`
explicitly. Existing configured models are unchanged. Anthropic examples should
use the current `claude-opus-5-5` default; Gemini examples should explicitly pin
`gemini-3.8-flash`. A Gemini-only bootstrap without an explicit `model` selects
the catalog default, `gemini-4-argon`, and reports it as `InitResult.model`.
Keep these unless a test deliberately pins another model. WASM mob flows use the
same current identity-first mob runtime, including helper/fork/respawn
controls, run-fenced stop, and runtime-committed session projection behavior.

**Provider proxy** (`sdks/web/proxy/`): Node.js auth-injecting reverse proxy. Sits between browser WASM and LLM providers so API keys stay server-side. Strips `Origin`/`Referer`/`Accept-Encoding` headers from forwarded requests. The package bin is `rkat-web-proxy` (`proxy/cli.mjs`; `--port`, default `PORT` or 3100; from 0.9.15 `--host`, default `127.0.0.1`, and `--cors-origin`, default loopback origins only). It serves `/anthropic/*`, `/openai/*`, and `/gemini/*` for each of `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, and `GEMINI_API_KEY` that is set.

From 0.9.15 the standalone proxy listens on the loopback interface by default; `--host <address>` binds another address. Before it calls the provider it compares a request's `Origin` header with the allowed origin (loopback origins by default; `--cors-origin <origin>` names one, `*` allows any) and answers 403 for any other origin, preflights included; bound to a loopback address it also refuses a `Host` header that is not a loopback name or address. The proxy does not check who calls it in any other way: a request without an `Origin` header is forwarded (unless a browser marks it `Sec-Fetch-Site: cross-site` and the allowed origin is not `*`), so put a proxy that listens on another address behind your own authentication.

```bash
# Standalone proxy
ANTHROPIC_API_KEY=sk-... npx @rkat/web proxy --port 3100
```

For a server with Fetch-style routing, set `ANTHROPIC_API_KEY` in that server's
environment. Pass a `Request` and the path with the provider prefix removed,
then return the resulting `Response`. From 0.9.15 the handler applies the
same origin rule: name the site's origin with `allowOrigin` (loopback origins
only when it is not given, `'*'` for any); `loopbackHostOnly: true` adds the
`Host` check. It does not authenticate callers, so the server does that
before it calls the handler:

```typescript
import { createProxyHandler } from '@rkat/web/proxy';

const anthropicProxy = createProxyHandler('anthropic', {
  allowOrigin: 'https://app.example.com',
});

export function handleRequest(request: Request): Promise<Response> {
  const { pathname } = new URL(request.url);
  if (pathname !== '/anthropic' && !pathname.startsWith('/anthropic/')) {
    return Promise.resolve(new Response('Not found', { status: 404 }));
  }
  return anthropicProxy(request, pathname.slice('/anthropic'.length) || '/');
}
```

This is not Express middleware. A Node/Express host must adapt its request to
the Fetch API and stream the returned response back; `startProxy` in
`sdks/web/proxy/index.mjs` demonstrates that adapter.

WASM client uses per-provider base URLs to point at the proxy natively — no fetch override needed:
```typescript
const runtime = await MeerkatRuntime.init(wasm, {
  anthropicApiKey: 'proxy',  // dummy, proxy replaces it
  anthropicBaseUrl: 'http://localhost:3100/anthropic',
});
```

### Browser auth model

Stock `MeerkatRuntime.init()` and `initFromMobpack()` accept provider keys and
base URLs, not realm/binding configuration. They synthesize `InlineSecret`
bindings in the in-memory `global` realm. Use keys or a server-side auth proxy
with that stock bootstrap; registering a callback does not create a realm or
convert an `InlineSecret` binding to external auth. Native realm config files
are not loaded by these browser exports.

The external resolver API is a **custom Rust/WASM composition seam**. To use
it, the custom bootstrap must supply a binding whose credential source is
`CredentialSourceSpec::ExternalResolver { handle: "wasm_host" }`. Only then
does the factory call the registered host callback for a typed
`ExternalAuthLease`. The following is a callback-registration fragment, not
a runnable external-binding setup for the stock npm bundle:

```typescript
import { registerExternalAuthResolver } from '@rkat/web';
import * as wasm from '@rkat/web/wasm/meerkat_web_runtime.js';

await wasm.default(); // Load the wasm-pack web binary before invoking an export.
registerExternalAuthResolver(wasm, async (authBinding) => {
  const token = await myHostFetchToken(authBinding);
  return {
    kind: 'inline_secret',
    secret: token.accessToken,
    metadata: { account_id: token.accountId },
    expires_at: token.expiresAt,
  };
});
```

Register before building the externally bound session. First runtime
installation preserves a registration made after loading the binary, but
replacing an existing runtime clears it; re-register after that bootstrap.
`withAuthBinding(authBinding, config)` only sets the structural selector on
the config. It cannot install the binding required by a custom composition.

Notes:

- WASM synthesizes its api-key binding section under the reserved `global` realm (`GLOBAL_REALM_SLUG`) in `crates/meerkat-web-runtime/src/lib.rs`. A session built without an explicit `authBinding` uses `global` as its explicit head; this is a degenerate single-realm chain with no filesystem doc or parent edges. Do not synthesize under any other slug.
- `authBinding` is structural and supported on `runtime.createSession({...})`, `mob.spawnHelper(...)`, and `mob.forkHelper(...)`. Plain `mob.spawn([...])` specs do not currently carry an auth binding.
- `mob.spawnHelper(prompt, options)` and `mob.forkHelper(sourceMemberId, prompt, options)` options require `agentIdentity`, `resultLabel`, and `maxTextBytes` (wire `agent_identity` / `result_label` / `max_text_bytes`; the surface never allocates helper identity, so a missing `agent_identity` is rejected); `profileName` maps to wire `role_name`. The helper result carries required `output`, `tokens_used`, `agent_identity`, `member_ref`, `bounded_result`, `session_id`, `usage`, `turns`, and `tool_calls`, plus optional `retirement_error`.
- `RuntimeConfig` uses `anthropicApiKey` / `anthropicBaseUrl`, `openaiApiKey` / `openaiBaseUrl`, and `geminiApiKey` / `geminiBaseUrl`. Raw WASM uses `anthropic_api_key` / `anthropic_base_url`, `openai_api_key` / `openai_base_url`, and `gemini_api_key` / `gemini_base_url`. Generic `apiKey` / `baseUrl` compatibility fields are deleted at both runtime and session boundaries; per-session credentials are not accepted.
- `RuntimeConfig.mobpackTrust` (raw `mobpack_trust`: `policy` `strict` or `permissive`, plus `trusted_signers.signers` mapping signer id to Ed25519 public-key hex) is the runtime's pack trust store. It defaults to strict with no signers, so `initFromMobpack()` and raw `create_session` reject unsigned or unknown-signer packs (`untrusted_mobpack`); opt into `permissive` only for local demos. `inspect_mobpack` is read-only and skips trust verification.
- `session.stopRun(runId, reason)` and `mob.stopMemberRun(agentIdentity, runId, reason)` (raw `stop_session_run` / `mob_stop_member_run`) stop one exact run; read `runId` from the `run_started` event's `identity.run_id`. They resolve to a `WireRunStopReceipt` union on `outcome` (`stopped`, `not_current`, `not_stoppable`); a stale run id is `not_current`, never an error.
- Use `clearExternalAuthResolver(wasm)` from `@rkat/web`, or pass `JsValue::NULL` / `undefined` to the raw WASM export, to clear the registration.
- `register_tool_callback` registers promise-returning JS callbacks. `register_js_tool` registration is synchronous, but dispatch reports pending detached host work, not completion. The host observes `ToolCallRequested`, performs the action, and reports any actual completion/result through a later session/mob message. Both registrations require initialized runtime state; prefer the instance methods after `MeerkatRuntime.init(...)`.
- Await `destroy_runtime(expected_handle?)` or `runtime.destroy()` for canonical session and mob teardown. The SDK passes the opaque bootstrap `runtime_handle` so stale wrappers cannot destroy a replacement runtime. Successful teardown clears subscriptions and the resolver, then drops `RuntimeState`; failures preserve cleanup authority for retry. This does not undo already-dispatched external host side effects.

For repository smoke coverage, browser/WASM scenarios are owned by the Rust lane
harness in `tests/integration/src/e2e_lanes.rs`. Prefer `make e2e-live` or
`make e2e-smoke` over a manual `npm run smoke` in `tests/live_smoke/browser`
unless you are debugging the browser surface directly. Cargo is the default
backend; with BuildBuddy access, `MEERKAT_BUILDBUDDY=1 make e2e-live` and
`MEERKAT_BUILDBUDDY=1 make e2e-smoke` route through the matching Bazel/RBE
lanes.

**Version validation:** `runtime_version()` export returns `CARGO_PKG_VERSION`. `MeerkatRuntime.init()` and `initFromMobpack()` compare it with `EXPECTED_VERSION` in `sdks/web/src/runtime.ts` and throw on a mismatch, which catches stale cached WASM. `scripts/bump-sdk-versions.sh` writes that constant and `make verify-version-parity` gates it.

## Building

From the repository root, build the SDK's WASM artifact through its checked
build wrapper (`sdks/web/scripts/build-wasm.mjs`). wasm-pack runs plain
`cargo`, so give it the repo-cargo lane target the way `make test-sdk-web` does
(prefix `--print-env` with `RUST_LANE_ID=<lane>` to build in a separate lane):

```bash
CARGO_TARGET_DIR="$(./scripts/repo-cargo --print-env | sed -n 's/^CARGO_TARGET_DIR=//p')" \
  npm --prefix sdks/web run build:wasm
```

`make test-sdk-web` runs the same WASM build (plus `build:ts`), then the SDK's
`npm test` and the packed-package smoke (`npm run test:packed`). The wrapper
owns the wasm32 flags (ambient rustflags are folded in ahead of
`--cfg getrandom_backend="wasm_js"` and the 8 MiB `-zstack-size`), the build
cache, and the output lock. Release builds use opt-level `s` with 256 codegen
units; the wrapper checks the linked stack and the spawn-frame budget.
`MEERKAT_WEB_WASM_PROFILE=dev|profiling`, `MEERKAT_WEB_WASM_OPT=0` (skip
wasm-opt), and `MEERKAT_WEB_WASM_FORCE_REBUILD=1` adjust a build. For custom
`wasm-pack` work, the crate path is `crates/meerkat-web-runtime`; `--out-dir` is
relative to that crate root, not the workspace root, and the build must carry
the same two rustflags (`docs/examples/wasm.mdx` shows the command).

**CRITICAL: wasm-pack creates a `.gitignore` with `*` inside the output directory.** `build-wasm.mjs` deletes it; a custom `wasm-pack` output must drop it before `npm publish` (or ship an `.npmignore`), otherwise npm excludes the WASM binary.

`make wasm-check` is the CI WASM gate: wasm32 `check` and `clippy -D warnings`
of `meerkat-web-runtime --all-targets` with
`RUSTFLAGS='--cfg getrandom_backend="wasm_js"'`. For targeted wasm32 work, pass
the same `RUSTFLAGS` to `./scripts/repo-cargo check -p <crate> --target
wasm32-unknown-unknown`. The optional BuildBuddy path exposes
`scripts/buildbuddy-dev wasm-check` and `scripts/buildbuddy-dev wasm-contract`
for the CI-equivalent WASM lanes.

`rkat mob web build <pack> -o <dir> --wasm <wasm-pack output dir or *_bg.wasm>
[--trust-policy permissive]` copies a prebuilt runtime (it never compiles one;
`--wasm` is required) into a static bundle whose `index.html` and
`meerkat-bootstrap.js` (`bootMobpack(opts)`) trust-verify the pack and call
`init_runtime`. The bundle does not create a mob or provide a prompt/transcript
UI.

## Common Gotchas

1. **Stale WASM binary**: Browser caches WASM aggressively. Nuke Playwright cache between rebuilds.
2. **cargo clean scope**: `cargo clean -p <crate>` only cleans native. wasm32 artifacts sit in `wasm32-unknown-unknown/` under the build's target dir (the repo-cargo lane's `CARGO_TARGET_DIR`, printed by `./scripts/repo-cargo --print-env`, not necessarily `target/`). Set `MEERKAT_WEB_WASM_FORCE_REBUILD=1` to rebuild the SDK bundle regardless of its cache.
3. **cfg inside async_trait**: May not propagate. Move cfg-gated logic to standalone functions outside the impl.
4. **Feature unioning**: Workspace `tokio = { features = ["full"] }` pulls mio which fails on wasm32. Each crate needs target-specific deps.
5. **MobBuilder**: A hand-built `MobBuilder` over the browser profile's in-memory session service requires `.allow_ephemeral_sessions(true)`; `MobMcpState` sets it from `supports_persistent_sessions()`. Members still receive `SessionOwned` bindings from the shared machine.
6. **Flow output format**: Without `output_format` or `expected_schema_ref`, a step defaults to text. With a schema and no explicit format, it defaults to JSON; explicit `output_format` selects the format. Require JSON-compatible output for JSON steps rather than assuming every flow output is JSON.
7. **Content shapes differ by export**: `start_turn` and the `initial_message` of `mob_respawn` take the tagged prompt (`{"text": ...}` or `{"blocks": [...]}`); `mob_spawn` spec `initial_message` and `mob_member_send` `content` take plain `ContentInput` (a string or a block array). The SDK serializes each one correctly.
8. **Release-build memory**: Cold wasm32 release compiles are memory-heavy. Local capped comparisons were OOM-killed at 4 and 2 Cargo jobs; with 1, the Cargo phase completed under a 13 GiB limit. The release job therefore sets `CARGO_BUILD_JOBS=1` (it bounds Cargo concurrency, not total memory or wasm-opt); do the same on small machines.

## Subsystem Availability on wasm32

Available: canonical session admission, keep-alive, cancellation, terminal
publication, agent loop, provider fetch transport, in-process comms, local mob
orchestration, browser tools and callbacks, tool scoping, embedded skills,
in-memory config and storage, compaction, multimodal content, and request-only
transient turn context.

`BrowserRuntimeProfile::require` is the availability authority. Its exclusions
include durable persistence, execution after page teardown, host processes,
configured hooks, runtime skill discovery, MCP clients, TCP/UDS comms, remote
member placement, file-based schema resolution (use an inline schema), and the
scheduler, WorkGraph, semantic memory, image generation, and fallback web
search services the browser composition does not install. Browser mobs also
offer no agent mob-management tools.
Requests for excluded capabilities return `CAPABILITY_UNAVAILABLE` with typed
`profile`, `capability`, and `clearing_action` data. Consult the profile rather
than adding surface-local availability tables or refusal text.

## Key Files

For detailed API surface and architecture, load: `references/api_surface.md`.

- `crates/meerkat-web-runtime/src/lib.rs` - wasm_bindgen exports (bootstrap, sessions, run stop, peer wiring, mob, subscriptions)
- `crates/meerkat-web-runtime/src/external_auth.rs` - `register_external_auth_resolver`, `has_external_auth_resolver`, `WasmExternalAuthResolver`
- `crates/meerkat-capabilities/src/browser_profile.rs` - `BrowserRuntimeProfile` exclusions and clearing actions
- `sdks/web/src/runtime.ts` - `MeerkatRuntime` class (TypeScript wrapper entry point)
- `sdks/web/src/session.ts` / `sdks/web/src/mob.ts` - `Session` and `MeerkatError`; `Mob` and `Member`
- `sdks/web/src/auth.ts` - external-auth helpers and the `Auth` RPC client
- `sdks/web/scripts/build-wasm.mjs` - WASM build wrapper (flags, cache, lock, stack and spawn-frame checks)
- `sdks/web/proxy/index.mjs` - Node.js provider proxy
- `crates/meerkat/src/factory.rs` - `build_agent()` with overrides, `AgentFactory::minimal()`
- `crates/meerkat/src/service_factory.rs` - `FactoryAgentBuilder` with default injection
