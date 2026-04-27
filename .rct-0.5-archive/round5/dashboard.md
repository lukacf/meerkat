# Round 5 Dashboard

Source of truth for Round 5 milestone state.

- Spec source: [dogma-violations.md](/Users/luka/.codex/dogma-violations.md)
- Tracking root: [.rct/round5](/Users/luka/.codex/worktrees/ec4d/meerkat/.rct/round5)
- Integration branch: `codex/dogma-round5-integration`
- Current workspace branch: `codex/fix-plenty-of-dogma-violations`

## Rules

- Only the orchestrator edits this file.
- Agents do not run `cargo`.
- Agents may request compile probes; probes are logged in [integration.md](/Users/luka/.codex/worktrees/ec4d/meerkat/.rct/round5/integration.md).
- No compatibility shim may appear in any handoff commit.

## Status Legend

- `draft`
- `plan-blocked`
- `plan-approved`
- `implementing`
- `probe-requested`
- `probe-complete`
- `handoff-ready`
- `static-blocked`
- `static-approved`
- `build-blocked`
- `build-approved`
- `final-green`

## Milestones

| Lane | Branch | Milestone | Rows | Status | Blocked By | Changed Areas | Last Handoff Commit | Next Action |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `A` | `codex/dogma-A-runtime-authority` | `A1a` | `DOGMA-08`, `DOGMA-09` | `final-green` | none | runtime loop, ops lifecycle | working tree handoff | done |
| `A` | `codex/dogma-A-runtime-authority` | `A1b` | `DOGMA-13` | `final-green` | `A1a` | core agent turn-state authority | working tree handoff | done |
| `B` | `codex/dogma-B-public-surfaces` | `B1` | `DOGMA-10`, `DOGMA-11`, `DOGMA-21`, `DOGMA-27` | `final-green` | none | catalogs, REST/RPC/MCP, SDKs | working tree handoff | done |
| `B` | `codex/dogma-B-public-surfaces` | `B2` | `DOGMA-26` | `final-green` | none | realtime status parity | working tree handoff | done |
| `C` | `codex/dogma-C-comms-trust` | `C1` | `DOGMA-12`, `DOGMA-19` | `final-green` | none | comms trust and rollback | working tree handoff | done |
| `D1` | `codex/dogma-D1-auth-identity` | `D1-1` | `DOGMA-15`, `DOGMA-17` | `final-green` | none | auth identity contract | working tree handoff | done |
| `D2` | `codex/dogma-D2-auth-policy` | `D2-1` | `DOGMA-25` | `final-green` | none | effective capability truth | working tree handoff | done |
| `D2` | `codex/dogma-D2-auth-policy` | `D2-2` | `DOGMA-16`, `DOGMA-24` | `final-green` | none | authorizers and policy behavior | working tree handoff | done |
| `EG` | `codex/dogma-EG-session-schedule` | `EG1a` | `DOGMA-20` | `final-green` | none | websocket rotation bridge rebuild | working tree handoff | done |
| `EG` | `codex/dogma-EG-session-schedule` | `EG1b-a` | `DOGMA-14` | `final-green` | none | recovery fallback correctness | working tree handoff | done |
| `EG` | `codex/dogma-EG-session-schedule` | `EG1b-b` | `DOGMA-23` | `final-green` | none | schedule terminal model | working tree handoff | done |
| `EG` | `codex/dogma-EG-session-schedule` | `EG2` | `DOGMA-18` | `final-green` | none | default skill-source identity | working tree handoff | done |

## Shared File Ownership

| File / Area | First Owner | Second Owner | Rule |
| --- | --- | --- | --- |
| `meerkat/src/factory.rs` | `D1` | `D2` | `D1` owns auth binding identity plumbing. `D2` owns effective-capability seeding and auth lease/policy follow-through. `B` and `EG` do not touch this file without explicit reassignment. |
| `meerkat-rest/src/lib.rs` | `B1` | `EG2` | `B1` owns session rename, `connection_ref`, and late-cancel surface work. `EG2` may touch only default skill-source canonicalization after `B1` is green. |
| `meerkat-rpc/src/session_runtime.rs` | `B1` | `EG2` | `B1` owns session create and public surface plumbing. `EG2` owns row `18` default-skill-source registry follow-up only after `B1`. |
| `meerkat-rpc/src/handlers/session.rs` | `B1` | `EG2` | Same sequencing as above. |
| `meerkat-rpc/src/realtime_ws.rs` | `EG1a` | `B2` | `EG1a` owns bridge/session rotation fix. `B2` may touch only status projection parity after `EG1a` is green. |
| `meerkat-openai/src/runtime/mod.rs` | `D1` | `D2` | `D1` introduces canonical binding identity contract if needed here. `D2` owns dynamic-authorizer and policy behavior. |
| `meerkat-anthropic/src/runtime/mod.rs` | `D1` | `D2` | Same as OpenAI runtime. |
| `meerkat-gemini/src/runtime/mod.rs` | `D1` | `D2` | Same as OpenAI runtime. |
| `meerkat-contracts/src/rest_catalog.rs` | `B1` | none | Sole owner. |
| `meerkat-contracts/src/rpc_catalog.rs` | `B1` | none | Sole owner. |
| `meerkat-contracts/src/capability/registry.rs` | `D2` | none | Sole owner. |
| `meerkat-skills/src/engine.rs` | `D2` | none | Sole owner. |
| `meerkat-skills/src/resolve.rs` | `EG2` | none | Sole owner. |
| `artifacts/schemas/*` | orchestrator | none | Regen only on `INT`. |
| SDK wrapper freshness / generated types | orchestrator | none | Regen only on `INT`. |

## Recommended Integration Order

1. `D1-1`
2. `A1a`
3. `C1`
4. `B1`
5. `EG1a`
6. `D2-1`
7. `D2-2`
8. `B2`
9. `A1b`
10. `EG1b`
11. `EG2`
