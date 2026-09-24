# Agent 05 Findings

## Scope reviewed

- Guides:
  - `docs/guides/comms.mdx`
  - `docs/guides/mobs.mdx`
  - `docs/guides/mobpack.mdx`
  - `docs/guides/realtime.mdx`
  - `docs/guides/scheduling.mdx`
- Examples:
  - `docs/examples/comms.mdx`
  - `docs/examples/mobs.mdx`
  - `docs/examples/mobpack.mdx`
  - `docs/examples/wasm.mdx`
- Implementation cross-checks:
  - mob RPC catalog and handlers
  - comms RPC handlers and MCP tool surface
  - wasm runtime exports
  - SDK call sites where they clarify current public lifecycle

## Files reviewed

- `docs/guides/comms.mdx`
- `docs/guides/mobs.mdx`
- `docs/guides/mobpack.mdx`
- `docs/guides/realtime.mdx`
- `docs/guides/scheduling.mdx`
- `docs/examples/comms.mdx`
- `docs/examples/mobs.mdx`
- `docs/examples/mobpack.mdx`
- `docs/examples/wasm.mdx`
- `meerkat-contracts/src/rpc_catalog.rs`
- `meerkat-rpc/src/handlers/comms.rs`
- `meerkat-rpc/src/handlers/mob.rs`
- `meerkat-mob-mcp/src/lib.rs`
- `meerkat-comms/src/mcp/tools.rs`
- `meerkat-web-runtime/src/lib.rs`
- `sdks/python/meerkat/client.py`
- `sdks/typescript/src/client.ts`

## Findings

### Mobs

1. `docs/guides/mobs.mdx` has a stale mob-tool inventory and no longer matches the current agent-side or host-side control plane. The table at `docs/guides/mobs.mdx:180-205` still presents a smaller older surface and leaves out current tools that users now need to understand lifecycle and readiness, including `mob_events`, `mob_respawn`, `mob_force_cancel`, `mob_wait_kickoff`, `mob_wait_ready`, and profile CRUD. On the host side it also never points readers at newer typed RPCs such as `mob/ensure_member`, `mob/reconcile`, `mob/list_members_matching`, `mob/destroy`, `mob/submit_work`, `mob/cancel_work`, `mob/cancel_all_work`, `mob/wait_kickoff`, and `mob/wait_ready`, all of which are now in the public RPC catalog. Current surfaces are defined in `meerkat-mob-mcp/src/lib.rs:1946-2108` and `meerkat-contracts/src/rpc_catalog.rs:340-429`.

2. `docs/examples/mobs.mdx` teaches `mob/member_send` as the host-side way to “send work” (`docs/examples/mobs.mdx:231-269`), but current behavior distinguishes ordinary member-directed content delivery from the newer work lane. `mob/member_send` just delivers content plus `handling_mode` (`meerkat-rpc/src/handlers/mob.rs:667-711`), while tracked work submission/cancellation now lives under `mob/submit_work`, `mob/cancel_work`, and `mob/cancel_all_work` (`meerkat-rpc/src/handlers/mob.rs:1206-1295`, `sdks/typescript/src/client.ts:1181-1212`, `sdks/python/meerkat/client.py:1761-1789`). Without a note about when to use `member_send` versus the work lane, host integrators can easily build on the wrong primitive and miss `work_ref`-based cancellation/idempotency.

### Comms

3. `docs/guides/comms.mdx` underspecifies the current tool contracts in ways that are user-visible. The guide says `send_message`, `send_request`, and `send_response` return only `{"status":"sent"}` (`docs/guides/comms.mdx:254-317`), but the current MCP tool surface returns `{"status":"sent","kind":"..."}` (`meerkat-comms/src/mcp/tools.rs:193-227`). The guide also omits the optional `handling_mode` override on `send_response`, which is part of the current tool contract (`meerkat-comms/src/mcp/tools.rs:165-178`) and matters because it controls whether the requester is interrupted immediately or sees the response at a later turn boundary. Separately, host-side `comms/send` already returns typed receipts such as `peer_message_sent`, `peer_request_sent`, and `peer_response_sent` with IDs and ACK state (`meerkat-rpc/src/handlers/comms.rs:56-94`), so the docs should distinguish the agent-tool receipt from the richer host receipt instead of collapsing both to “sent”.

4. No actionable drift found in `docs/examples/comms.mdx` beyond the guide-level contract gaps above. The examples still match the current `session/create`, `comms/send`, `comms/peers`, and `session/external_event` host surfaces.

### WASM

5. `docs/examples/wasm.mdx` uses the wrong credential/config shape for the current browser runtime. The example passes a singular `api_key` into both `init_runtime()` and `create_session()` (`docs/examples/wasm.mdx:31-54`), but the current wasm bootstrap expects provider-specific keys such as `anthropic_api_key`, `openai_api_key`, and `gemini_api_key` (`meerkat-web-runtime/src/lib.rs:1088-1098`, `meerkat-web-runtime/src/lib.rs:1145-1159`). More importantly, `create_session()` no longer accepts per-session API keys at all: credentials are loaded during runtime bootstrap, and session creation only accepts session config (`meerkat-web-runtime/src/lib.rs:1217-1221`, `meerkat-web-runtime/src/lib.rs:1312-1316`). As written, the sample is copy-paste broken for current code.

### Realtime

6. No actionable drift found in `docs/guides/realtime.mdx` during this pass. The guide’s capability-driven attachment model, `runtime/realtime_attachment_status*` coverage, and no-public-reconfigure-RPC caveat matched the current runtime and RPC surface closely enough that I did not flag remediation here.

### Scheduling

7. No actionable drift found in `docs/guides/scheduling.mdx`. The trigger types, target bindings, policy names, lifecycle phases, and agent-side tool names matched the current schedule types/tool surface on this branch.

### Mobpack

8. No actionable drift found in `docs/guides/mobpack.mdx` or `docs/examples/mobpack.mdx`. The documented `pack`, `inspect`, `validate`, `deploy`, `web build`, and trust-policy resolution flow still line up with the current CLI.

## Suggested follow-ups

1. Update `docs/guides/mobs.mdx` to separate:
   - current agent-side mob tools
   - current public host RPC/SDK methods
   - readiness/work-lane lifecycle guidance (`wait_*` and `submit_work`)

2. Update `docs/examples/mobs.mdx` so the “send work” section distinguishes:
   - `mob/member_send` for ordinary content delivery
   - `mob/submit_work` for tracked/cancellable work

3. Refresh `docs/guides/comms.mdx` to document:
   - actual agent-tool return payloads
   - optional `send_response.handling_mode`
   - the difference between MCP tool receipts and host `comms/send` receipts

4. Fix `docs/examples/wasm.mdx` snippets to use bootstrap-time provider keys and session-time config only, with no per-session `api_key` field.
