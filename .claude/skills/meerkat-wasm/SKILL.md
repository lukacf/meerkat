---
name: meerkat-wasm
description: "Developer guide for the Meerkat WASM embedded runtime (meerkat-web-runtime) and the `@rkat/web` browser SDK. Covers wasm32 compilation, building/deploying WASM bundles, writing/auditing wasm_bindgen exports (sessions, mobs, comms, JS tool callbacks, external auth resolver, subscription handles), the auth_binding + registerExternalAuthResolver auth model, tokio_with_wasm aliasing, cfg-gating patterns, mobpack bootstrap, and common wasm32 gotchas. Use when touching meerkat-web-runtime, sdks/web, browser auth flows, or making a meerkat crate wasm32-compatible."
---

# Meerkat WASM Embedded Runtime

The `meerkat-web-runtime` crate compiles the full meerkat agent stack to `wasm32-unknown-unknown`. It is an embedded deployment target for mobpacks — the JavaScript equivalent of the Rust SDK. Not a protocol server like RPC/REST.

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

**lib.rs** — tokio alias + module gating:
```rust
#[cfg(target_arch = "wasm32")]
pub mod tokio { pub use tokio_with_wasm::alias::*; }

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

## Override-First Resource Injection

`AgentBuildConfig` has 4 override fields. When set, `build_agent()` uses them directly, skipping filesystem resolution. On wasm32, these are always set.

| Field | Skips | wasm32 default |
|-------|-------|---------------|
| `tool_dispatcher_override` | Shell/file/project tool resolution | `CompositeDispatcher::new_wasm()` |
| `session_store_override` | Feature-flag store creation | `StoreAdapter` backed by `meerkat_store::MemoryStore` |
| `hook_engine_override` | Filesystem hook config | `None` |
| `skill_engine_override` | Filesystem/git skill resolution | `None` |

`FactoryAgentBuilder` has `default_tool_dispatcher` and `default_session_store` — injected into ALL `build_agent()` calls including mob-spawned sessions.

## Runtime mode on wasm32

WASM is an embedded/standalone surface, not a runtime-backed one. When you are
working on wasm session creation paths:

- do not invent runtime-backed `SessionRuntimeBindings`
- prefer explicit `RuntimeBuildMode::StandaloneEphemeral`
- treat `prepare_bindings()` as a runtime-backed surface API, not a wasm default

This keeps the embedded contract honest: browser sessions use the in-memory
substrate intentionally, not as an accidental fallback from a missing runtime
binding.

## `@rkat/web` npm Package

The `sdks/web/` directory contains `@rkat/web` — a TypeScript wrapper around the wasm_bindgen exports with an idiomatic camelCase API. Ships with the pre-built WASM binary and a Node.js provider proxy.

**Key classes:** `MeerkatRuntime`, `Mob`, `Session`, `EventSubscription`. Auth helpers live in `sdks/web/src/auth.ts` (`registerExternalAuthResolver`, `clearExternalAuthResolver`, `withAuthBinding`, `Auth` types).

Current release-line notes: `runtime_version()` and package compatibility
should match the workspace `Cargo.toml` version (latest published release line
is `0.8.x`; do not hardcode a stale patch pin). The OpenAI and global catalog
default is `gpt-6-astra`. Confirm the proxy's API organization has access;
otherwise select `gpt-5.5` explicitly. Existing configured models are unchanged. Anthropic and
Gemini examples should use the current `claude-opus-5` and
`gemini-3.5-flash` defaults unless a test deliberately pins another model. WASM
mob flows use the same current
identity-first mob runtime, including helper/fork/respawn controls and
runtime-committed session projection behavior.

**Provider proxy** (`sdks/web/proxy/`): Node.js auth-injecting reverse proxy. Sits between browser WASM and LLM providers so API keys stay server-side. Strips `Origin`/`Referer`/`Accept-Encoding` headers from forwarded requests.

```bash
# Standalone proxy
ANTHROPIC_API_KEY=sk-... npx @rkat/web proxy --port 3100
```

For a server with Fetch-style routing, set `ANTHROPIC_API_KEY` in that server's
environment. Pass a `Request` and the path with the provider prefix removed,
then return the resulting `Response`:

```typescript
import { createProxyHandler } from '@rkat/web/proxy';

const anthropicProxy = createProxyHandler('anthropic');

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

- WASM synthesizes its api-key binding section under the reserved `global` realm (`GLOBAL_REALM_SLUG`) in `meerkat-web-runtime/src/lib.rs`. A session built without an explicit `authBinding` uses `global` as its explicit head; this is a degenerate single-realm chain with no filesystem doc or parent edges. Do not synthesize under any other slug.
- `authBinding` is structural and supported on `runtime.createSession({...})`, `mob.spawnHelper(...)`, and `mob.forkHelper(...)`. Plain `mob.spawn([...])` specs do not currently carry an auth binding.
- `mob.spawnHelper(...)` and `mob.forkHelper(...)` options require `resultLabel` and `maxTextBytes` (wire `result_label` / `max_text_bytes`); the helper result carries required `output`, `tokens_used`, `agent_identity`, `member_ref`, `bounded_result`, `session_id`, `usage`, `turns`, and `tool_calls`, plus optional `retirement_error`.
- `RuntimeConfig` uses `anthropicApiKey` / `anthropicBaseUrl`, `openaiApiKey` / `openaiBaseUrl`, and `geminiApiKey` / `geminiBaseUrl`. Raw WASM uses `anthropic_api_key` / `anthropic_base_url`, `openai_api_key` / `openai_base_url`, and `gemini_api_key` / `gemini_base_url`. Generic `apiKey` / `baseUrl` compatibility fields are deleted at both runtime and session boundaries; per-session credentials are not accepted.
- Use `clearExternalAuthResolver(wasm)` from `@rkat/web`, or pass `JsValue::NULL` / `undefined` to the raw WASM export, to clear the registration.
- `register_tool_callback` registers promise-returning JS callbacks. `register_js_tool` registration is synchronous, but dispatch reports pending detached host work, not completion. The host observes `ToolCallRequested`, performs the action, and reports any actual completion/result through a later session/mob message. Both registrations require initialized runtime state; prefer the instance methods after `MeerkatRuntime.init(...)`.
- `destroy_runtime` clears subscriptions and the resolver and drops `RuntimeState`, invalidating its browser-local handles. Call it on host teardown; it does not certify durable cleanup or cancel already-dispatched external side effects.

For repository smoke coverage, browser/WASM scenarios are owned by the Rust lane
harness in `tests/integration/src/e2e_lanes.rs`. Prefer
`make e2e-live` or `make e2e-smoke` over manual `npm run smoke` unless you are
debugging the browser surface directly. Cargo is the default backend; with
BuildBuddy access, `MEERKAT_BUILDBUDDY=1 make e2e-live` and
`MEERKAT_BUILDBUDDY=1 make e2e-smoke` route through the matching Bazel/RBE
lanes.

**Version validation:** `runtime_version()` export returns `CARGO_PKG_VERSION`. `MeerkatRuntime.init()` checks it against the expected version to catch stale cached WASM.

## Building

```bash
RUSTFLAGS='--cfg getrandom_backend="wasm_js"' \
  wasm-pack build meerkat-web-runtime --target web --out-dir <dir>
```

**CRITICAL: `--out-dir` is relative to CRATE root (`meerkat-web-runtime/`), not workspace root.**

**CRITICAL: wasm-pack creates a `.gitignore` with `*` inside the output directory.** Delete it before `npm publish` or add an `.npmignore` to the package — otherwise npm excludes the WASM binary.

Verify targeted wasm32 work with `./scripts/repo-cargo check -p <crate>
--target wasm32-unknown-unknown`. Clippy for the runtime remains
`./scripts/repo-cargo clippy -p meerkat-web-runtime --target
wasm32-unknown-unknown -- -D warnings` when debugging locally. The optional
BuildBuddy path exposes `scripts/buildbuddy-dev wasm-check` and
`scripts/buildbuddy-dev wasm-contract` for the CI-equivalent WASM lanes.

## Common Gotchas

1. **Stale WASM binary**: Browser caches WASM aggressively. Nuke Playwright cache between rebuilds.
2. **cargo clean scope**: `cargo clean -p <crate>` only cleans native. Use `rm -rf target/wasm32-unknown-unknown`.
3. **cfg inside async_trait**: May not propagate. Move cfg-gated logic to standalone functions outside the impl.
4. **Feature unioning**: Workspace `tokio = { features = ["full"] }` pulls mio which fails on wasm32. Each crate needs target-specific deps.
5. **MobBuilder**: Requires `.allow_ephemeral_sessions(true)` for the embedded ephemeral substrate used in WASM.
6. **Flow output format**: Without `output_format` or `expected_schema_ref`, a step defaults to text. With a schema and no explicit format, it defaults to JSON; explicit `output_format` selects the format. Require JSON-compatible output for JSON steps rather than assuming every flow output is JSON.

## Subsystem Availability on wasm32

Full: agent loop, LLM providers (browser fetch), sessions (ephemeral), comms (inproc), mob orchestration (in-memory), tools (task tools + `datetime` + comms/skill surfaces; no shell and no filesystem-mutating builtins), tool scoping (`ToolScope` + per-turn overlays), skills (embedded), hooks (in-process), config (in-memory), compaction, multimodal content (`ContentInput`/`ContentBlock` parsing at WASM bridge).

Excluded: shell tools, filesystem-mutating builtins such as `apply_patch`, filesystem persistence, MCP client (rmcp), network comms (TCP/UDS).

## Key Files

For detailed API surface and architecture, load: `references/api_surface.md`.

- `meerkat-web-runtime/src/lib.rs` — wasm_bindgen exports (bootstrap, sessions, mob, subscriptions, comms)
- `sdks/web/src/runtime.ts` — `MeerkatRuntime` class (TypeScript wrapper entry point)
- `sdks/web/proxy/index.mjs` — Node.js provider proxy
- `meerkat/src/factory.rs` — `build_agent()` with overrides, `AgentFactory::minimal()`
- `meerkat/src/service_factory.rs` — `FactoryAgentBuilder` with default injection
