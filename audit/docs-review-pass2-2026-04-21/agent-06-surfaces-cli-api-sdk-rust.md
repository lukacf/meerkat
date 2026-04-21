# Agent 06 Findings

## Scope reviewed

- CLI surface docs for command/config behavior
- Public API docs for MCP, REST, and JSON-RPC
- Rust library docs for overview, tools/stores, and advanced embedding
- Python and TypeScript SDK overview/reference docs, with parity checks against current SDK code

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

1. `docs/cli/configuration.mdx:126-134` has a stale exit-code contract. The page says exit code `2` means “Invalid params or CLI contract error,” but the current CLI reserves `2` for graceful budget exhaustion and returns `1` for ordinary command/runtime failures. The active constants and exit mapping are in `meerkat-cli/src/main.rs:77-80` and `meerkat-cli/src/main.rs:2102-2115`. This is user-visible drift for scripts/CI that branch on exit codes.

2. `docs/api/mcp.mdx:37-69` presents the MCP host tool list as if it were the always-available `rkat-mcp` surface, but the actual tool inventory is both incomplete and incorrectly gated.
   The docs omit default/base tools that are currently shipped, including `meerkat_skills` and `meerkat_blob_get` from `meerkat-mcp-server/src/lib.rs:1205-1228`, plus the schedule tools added unconditionally by `tools_list()` in `meerkat-mcp-server/src/lib.rs:1316-1318`.
   The docs also list mob/realtime tools as general surface tools, but those are only added when the `mob` feature is compiled in (`meerkat-mcp-server/src/lib.rs:1320-1327`), while the crate’s default features are only `comms` and `schedule` (`meerkat-mcp-server/Cargo.toml:47-51`). The realtime MCP tools specifically come from `meerkat_mob_mcp::public_tools_list()` (`meerkat-mob-mcp/src/public_mcp.rs:504-520`), so the current page can make default `rkat-mcp` builds look richer than they are.

3. `docs/api/rest.mdx:104-112` documents `POST /mob/{id}/members/{agent_identity}/realtime/attach` and `/realtime/detach` as active mob endpoints, but the handlers are explicitly marked unavailable and always return a bad-request error because realtime is now session-scoped. See `meerkat-rest/src/lib.rs:1996-2018`. This is a concrete REST contract mismatch.

4. `docs/api/rpc.mdx:1-24` documents `rkat-rpc` as a stdio-only surface, but the current binary also supports `--tcp <addr>` and `--realtime-ws <addr>` listener modes. Those flags are part of the public CLI in `meerkat-rpc/src/main.rs:37-46`. Even if stdio remains the primary mode, the page title, description, and getting-started section currently hide a supported transport mode from app/IDE integrators.

5. `docs/rust/advanced.mdx:45-53` contains an `AgentBuilder` example that does not compile as written. `AgentBuilder::build(...)` is async (`meerkat-core/src/agent/builder.rs:214-220`), so the snippet needs `.await`, and the resulting agent must be mutable for direct run APIs because `run`/`run_with_events` take `&mut self` (`meerkat-core/src/agent/runner.rs:823-834`). This is a sharp edge on the “expert-level” page where readers are most likely to copy-paste.

6. The Python SDK docs materially understate the public client surface, which creates parity drift against the actual SDK.
   `docs/sdks/python/overview.mdx:53-119` and `docs/sdks/python/reference.mdx:14-128` cover the session/config/schedule/mob core, but the current client also exposes realm and auth wrappers (`list_realms`, `get_realm`, `list_auth_profiles`, `get_auth_profile`, `create_auth_profile`, `delete_auth_profile`, `test_auth_profile`, `auth_login_*`, `auth_provision_api_key`, `auth_status`, `auth_logout`) in `sdks/python/meerkat/client.py:340-492`.
   The docs also miss public session/control helpers such as `send_peer_response_terminal` (`sdks/python/meerkat/client.py:749-765`), blob/skill/MCP surfaces (`sdks/python/meerkat/client.py:801-807`, `sdks/python/meerkat/client.py:925-1010`), and realtime/runtime helpers (`sdks/python/meerkat/client.py:2117-2189`).

7. The TypeScript SDK docs have the same parity problem, although with a different omission pattern.
   `docs/sdks/typescript/overview.mdx:65-98` and `docs/sdks/typescript/reference.mdx:58-64` mention the core surface plus realtime/auth wrappers, but they still omit several public `MeerkatClient` methods: `sendPeerResponseTerminal` (`sdks/typescript/src/client.ts:514-528`), `mcpAdd`/`mcpRemove`/`mcpReload` and their snake_case aliases (`sdks/typescript/src/client.ts:619-648`), `listSkills`/`inspectSkill`/`getBlob` (`sdks/typescript/src/client.ts:652-675`), runtime/input helpers (`sdks/typescript/src/client.ts:1845-1983`), and realm wrappers (`sdks/typescript/src/client.ts:1992-2065`). As written, the docs make TS and Python look narrower and less aligned with the current RPC catalog than they actually are.

8. No material contract drift found in `docs/cli/commands.mdx`, `docs/rust/overview.mdx`, or `docs/rust/tools-and-stores.mdx` during this pass. I checked command names/examples against the current CLI and Rust facade exports and did not find another concrete mismatch worth flagging beyond the items above.

## Suggested follow-ups

- Update `docs/cli/configuration.mdx` to reflect the actual CLI exit-code semantics, especially that exit code `2` is now budget exhaustion rather than generic invalid params.
- Split `docs/api/mcp.mdx` into “always present”, “feature-gated”, and “default build vs `mob` build” tool sections, and add the currently omitted blob/skills/schedule tools.
- Mark the REST mob realtime attach/detach routes as unavailable/deprecated, or remove them from the endpoint overview entirely.
- Expand `docs/api/rpc.mdx` so it documents the supported TCP and optional realtime-WebSocket listener flags in addition to stdio.
- Fix the `AgentBuilder` Rust snippet so it compiles (`let mut agent = ...build(...).await;`) and re-scan direct-agent examples for mutability/async correctness.
- Add SDK “additional wrappers” sections for both Python and TypeScript that enumerate auth, realm, MCP live ops, blob/skills, runtime/realtime, and peer-response helpers so the docs match the exported public surface.
