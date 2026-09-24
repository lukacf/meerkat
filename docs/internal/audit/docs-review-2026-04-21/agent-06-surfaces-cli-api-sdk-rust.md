# Agent 06 Findings

## Scope reviewed

- CLI surface docs
- REST, RPC, and MCP API docs
- Rust library docs
- Python SDK docs
- TypeScript SDK docs

## Files reviewed

- `docs/cli/commands.mdx`
- `docs/cli/configuration.mdx`
- `docs/api/mcp.mdx`
- `docs/api/rest.mdx`
- `docs/api/rpc.mdx`
- `docs/rust/overview.mdx`
- `docs/rust/tools-and-stores.mdx`
- `docs/rust/advanced.mdx`
- `docs/sdks/python/overview.mdx`
- `docs/sdks/python/reference.mdx`
- `docs/sdks/typescript/overview.mdx`
- `docs/sdks/typescript/reference.mdx`

## Findings

1. `docs/cli/commands.mdx` is missing current top-level CLI commands.

The “Common commands” table in `docs/cli/commands.mdx:44-59` does not mention `rkat realtime`, `rkat blob`, or `rkat auth`, even though the current CLI exposes them as first-class commands in `meerkat-cli/src/main.rs:1228-1309`. The root help text also advertises `run, realtime` and `session, blob, models, capabilities, doctor` in `meerkat-cli/src/main.rs:84`. This makes the command reference incomplete for the shipped binary.

2. `docs/cli/configuration.mdx` documents stale CLI exit codes.

The exit-code table in `docs/cli/configuration.mdx:126-143` lists codes `10`, `11`, `12`, `20`, `21`, `22`, `30`, `40`, `41`, and `42`. The current CLI only defines and returns `0`, `1`, and `2` in `meerkat-cli/src/main.rs:77-80,2104-2115`. The additional per-error codes are no longer emitted by this binary, so the page currently overstates the contract.

3. `docs/api/rest.mdx` understates the current public REST surface.

The endpoint overview in `docs/api/rest.mdx:51-79` is missing several live routes from the current router in `meerkat-rest/src/lib.rs:1083-1207`, including:

- `/sessions/{id}/system_context`
- `/sessions/{id}/peer-response-terminal`
- `/schedule/tools` and `/schedule/call`
- the full `/schedules/*` family
- `/realtime/open_info`, `/realtime/status`, `/realtime/capabilities`
- `/runtime/*` and `/input/*`
- `/auth/*` and `/realms/*`
- mob helper/member endpoints such as `/mob/{id}/spawn-helper`, `/fork-helper`, `/wait-kickoff`, member status, member cancel, and member respawn

As written, the page reads like the REST host only exposes session CRUD plus a few helper endpoints, which is no longer true.

4. `docs/api/rest.mdx` points readers at the wrong realm config path.

`docs/api/rest.mdx:105-113` says REST realm config lives under `.../meerkat/rest/realms/<realm>/config.toml`. In the implementation, REST resolves the shared realm locator, opens persistence under the normal realm root, and then reads `realm_paths.config_path` from that shared realm directory in `meerkat-rest/src/lib.rs:221-247`. The default shared state root is `.../meerkat/realms` per `meerkat-core/src/runtime_bootstrap.rs:74-80`. `rest_instance_root()` is `.../meerkat/rest` (`meerkat-rest/src/lib.rs:747-752`), but that is not the realm config location the docs describe.

5. The Python SDK docs are stale on both installation/runtime requirements and the external-event API shape.

`docs/sdks/python/overview.mdx:9-12` says the SDK has “Runtime dependencies: none”, but the published package declares `websockets>=12,<16` in `sdks/python/pyproject.toml:5-11`.

Separately, both Python docs describe `send_external_event` without the required event type:

- `docs/sdks/python/overview.mdx:64-67`
- `docs/sdks/python/reference.mdx:133-140`

The actual API is `send_external_event(session_id, event_type, payload, *, blocks=None)` in `sdks/python/meerkat/client.py:730-747`, with the session convenience wrapper matching that shape in `sdks/python/meerkat/session.py:245-257`. The docs currently imply a two-argument payload-only call that will not match the shipped client.

6. The TypeScript SDK docs misdescribe default realm behavior and the external-event signature, and they under-document the current exported surface.

`docs/sdks/typescript/overview.mdx:163` says that if `realmId` is omitted and `isolated` is not set, sessions are stored in “the default realm”. The actual server default is isolated mode: `rkat-rpc` builds `RealmSelection::Isolated` when no realm flags are supplied in `meerkat-rpc/src/main.rs:124-129`. The SDK also only forwards realm args when options are explicitly set in `sdks/typescript/src/client.ts:274-283`, so the documented “default realm” behavior is incorrect for the current client/server pair.

The same overview page also documents `sendExternalEvent(sessionId, payload, options?)` at `docs/sdks/typescript/overview.mdx:74`, but the actual method is `sendExternalEvent(sessionId, eventType, payload, options?)` in `sdks/typescript/src/client.ts:496-511`.

In addition, the TS docs still present a narrower surface than the package exports today. The package root exports realtime handles in `sdks/typescript/src/index.ts:35-42`, and the client exposes auth/realm wrappers in `sdks/typescript/src/client.ts:1992-2067`, but those surfaces are not covered in either TS doc page.

7. `docs/rust/tools-and-stores.mdx` contains multiple stale or non-compiling public-API examples.

There are several concrete mismatches on this page:

- `docs/rust/tools-and-stores.mdx:85-100` shows `ToolRegistry::register(tool_def, closure)`, but the actual API only registers schema definitions via `register(def)` / `register_many(defs)` in `meerkat-tools/src/registry.rs:15-30`.
- `docs/rust/tools-and-stores.mdx:103-123` imports `meerkat::ToolOutput`, but `ToolOutput` is not re-exported from the `meerkat` facade (`meerkat/src/lib.rs:242-245`). The enum currently lives in `meerkat-tools::builtin` at `meerkat-tools/src/builtin/mod.rs:61-68`.
- `docs/rust/tools-and-stores.mdx:177-195` implements `SessionStore` with `StoreError`, but the trait requires `SessionStoreError` in `meerkat-core/src/session_store.rs:60-74`.
- `docs/rust/tools-and-stores.mdx:198-215` claims that `AgentFactory::session_store()` makes `build_persistent_service(...)` persist through the custom store. In reality, the runtime-backed service persists through the `PersistenceBundle` store passed into `build_runtime_backed_service`, which becomes the `PersistentSessionService` store in `meerkat/src/surface/runtime_backed.rs:28-35` and `meerkat-session/src/persistent.rs:580-598`.
- `docs/rust/tools-and-stores.mdx:247-252` uses `McpServerConfig::http(...)`, but the current constructors are `streamable_http(...)` and `sse(...)` in `meerkat-core/src/mcp_config.rs:97-145`.

These examples are likely to fail for readers who copy them verbatim.

8. No material drift found in the remaining scoped pages.

I did not find contract-level inaccuracies in these files during this pass:

- `docs/api/mcp.mdx`
- `docs/api/rpc.mdx`
- `docs/rust/overview.mdx`
- `docs/rust/advanced.mdx`

Those pages looked aligned with the current binaries/contracts, aside from the broader SDK/REST gaps already called out above.

## Suggested follow-ups

- Update the CLI docs together: command inventory in `docs/cli/commands.mdx` and exit-code guidance in `docs/cli/configuration.mdx` should be regenerated from the current binary behavior to avoid further drift.
- Rework `docs/api/rest.mdx` from the router table in `meerkat-rest/src/lib.rs`, not a hand-maintained subset. The current page is missing too many routes to be safely partial without saying so.
- Refresh both SDK doc sets from the current exported clients. At minimum, cover realtime, auth/realm, blob, MCP live ops, and the correct external-event signatures in both Python and TypeScript.
- Replace the Rust `tools-and-stores` examples with compile-checked snippets or snippets derived from examples/tests. This page currently has the highest copy-paste failure risk in the audited set.
