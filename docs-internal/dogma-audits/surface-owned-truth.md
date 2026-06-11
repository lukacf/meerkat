---
title: "Dogma Audit — Surface-Owned Truth"
description: "Workspace audit for Rule 6 (Surfaces Are Thin Over The Shared Runtime) / Surface Priesthood."
icon: "magnifying-glass"
---

# Dogma Audit: Surface-Owned Truth (Rule 6 / Surface Priesthood)

> **Postscript 2026-06-11:** point-in-time audit record (PR #754 head). Open/confirmed findings below were re-adjudicated and closed by the PR #759 campaign — see [`PR759-final-ledger.md`](PR759-final-ledger.md) (close-out: open set zero).

**Audit target:** PR #754 head `worktree-dogma-stringy` @ `32b4d13b0`
**Date:** 2026-06-05
**Dogma:** Rule 6 — *Surfaces Are Thin Over The Shared Runtime*; pattern *Surface Priesthood*.
**Test:** a surface (CLI/rkat, REST, RPC, MCP server, MCP agent tools, SDK python/typescript/web, WASM/browser,
examples) is a **translation boundary**, not a second runtime. It may format UX, normalize transport, choose
presentation, expose friendly names. It must not own/fabricate/advertise a fact the shared runtime should own — a
default, lifecycle/phase, handle, availability/feature/model support, or success/terminal class — nor silently
emulate runtime-backed behavior in standalone mode, nor advertise (static catalog / doc / SDK flag) something the
runtime cannot deliver. Correct shape: `surface syntax → typed request → shared helper → MeerkatMachine bindings →
SessionService → AgentFactory`.

## Method

Two agent workflows (same harness as the prior audits). (1) A 10-agent surface-centric map inventoried **167
surface sites** (38 suspect). (2) A 48-candidate verify pass: each candidate got an independent classifier that
**re-traced from real code** (Rust, TypeScript, *and* Python) to decide runtime-owned vs surface-fabricated; every
suspect faced a 3-lens adversarial refuter panel (lowering / honest-mode / advertise-vs-deliver), confirmed only at
≥2 non-refutes; a synthesis agent clustered survivors and re-verified line drift. Load-bearing claims were
hand-verified at the cited lines.

**Result:** 48 candidates → **11 confirmed** (7 Medium, 4 Low), **5 contested** (refuted or narrow), 32
legitimate. **5 root clusters. No High.**

## Bottom line

- **Widespread?** **No — narrow and decisively SDK-skewed.** 10 of ~15 findings live in the hand-written SDKs
  (Python/TypeScript/Web). The Rust product surfaces are largely clean: **CLI and REST have zero confirmed
  findings** (REST is repeatedly cited as the dogma-correct sibling), RPC/MCP carry only minor config-default and
  duplicated-seam items.
- **Systemic?** **Yes, but in one specific place:** the SDKs systematically **coalesce an absent/unknown
  runtime-owned status to a permissive class** (`?? "completed"` / `?? "Available"` / `else "staged"` / `?? ""`)
  instead of failing closed — asserting a class the runtime never confirmed. The same shape recurs ~8× across two
  SDK languages, often in paired twins.
- **No High / latent risk.** Every divergent branch is **dormant today**: the wire contracts are closed + required
  (no `skip_serializing_if`), so the fallback is dead-on-the-wire; or the fabricated literal happens to coincide
  with the canonical value. The harm is **latent contract-rot** that goes live on contract skew, a degraded/older
  server, a malformed response, or a new enum variant.
- **Complexity to clean?** **Medium, mechanically uniform.** ~10 SDK sites each become a 2–6 line strict-parse-or-
  throw; **no runtime/contract changes needed** (contracts are already closed and correct), and **the web SDK
  already provides the fail-closed reference implementation** (`parseWireHandlingMode` throws on unknown). One Rust
  finding (structured-output-retries default) crosses a contract boundary.

## Confirmed clusters

### C1 — SDK coalesces absent/unknown runtime status to a permissive class (the dominant root)
**`fabricated_success_or_terminal_class` · Medium · sdk-python + sdk-typescript · ids 25,27,28,29,30,31,33,35,37**

Every hand-written SDK status/terminal projection applies a `?? <permissive>` / `else <success>` coalesce when the
runtime-owned status field is absent or unrecognized, asserting a class the runtime never confirmed:
- **Python** (`sdks/python/meerkat/client.py`): capabilities → `"Available"` (`c.get("status","Available")` :471);
  mob/respawn → `"completed"` (:1817); mob/append_system_context collapses `applied|staged|duplicate` to
  `else "staged"` (:2330); mob/member_send substitutes the **client's requested** `handling_mode` when the
  runtime-confirmed mode is missing (:1582).
- **TypeScript** (`sdks/typescript/src/client.ts`): mob/snapshot → `String(result.status ?? "unknown")` (status
  not even in the enum); mob/list → `?? ""` empty-string lifecycle class; mob/respawn → `?? "completed"`;
  mob/status object-key-as-status reconstruction.

All the underlying contracts are **closed + required** (`WireMobRespawnOutcome`, `AppendSystemContextStatus`,
`WireHandlingMode`, `WireMobLifecycleStatus`, `CapabilityEntry.status`), so each branch is dead today but goes live
on skew — at which point a topology-restore *failure* reports `"completed"`, an unknown capability reports
`"Available"`, etc. **The web SDK is the proven-correct sibling:** `parseWireHandlingMode` (`sdks/web/src/mob.ts:294`)
validates against `WIRE_HANDLING_MODES` and `throw`s on unknown; `parseMobStatusResult` uses
`requireRecord`/`requireStringField`.

**Repair:** parse against the closed contract enum, accept only its variants, raise `MeerkatError('INVALID_RESPONSE')`
on absent/unrecognized (mirror the web SDK). Delete every `?? 'Available'|'completed'|'unknown'|''` and
`else 'staged'`; never substitute the client's request for an unconfirmed runtime value (finding 30). Fix paired
Python↔TS twins together. *Medium: ~10 sites across two languages, each 2–6 lines + a fail-closed test; reference
impl already in-tree.*

### C2 — Hand-written SDK types diverge from the already-generated closed wire contract
**`fabricated_default` · Low · sdk-web + sdk-typescript · ids 18,33,35,37 (overlaps C1)**

Root enabler beneath the status findings: hand-written SDK types ignore the generated-from-authority contract
types. `mob.ts` (18) renames the wire's required `role: ProfileName` to a hand-written `profile` noun and adds two
**phantom** fallback aliases (`member.profile` / `member.profile_name`) the `MobMemberListEntryWire` contract can
never emit. TS `mobSnapshot`/`listMobs` return loose `status: string` instead of consuming the generated
`MobSnapshotResult`/`MobStatusResult` closed `WireMobLifecycleStatus` union. **Repair:** bind the SDK roster/status
types to the already-generated wire contracts; delete phantom alias branches and the `Object.keys`-as-status
reconstruction. *Small; folds into C1.*

### C3 — Rust surface fabricates/duplicates a runtime-config-owned default
**`fabricated_default` · Medium · rpc + mcp-server · ids 1,4**

- **F1** (`meerkat-rpc/src/handlers/session.rs:291`): `default_model("anthropic").unwrap_or("claude-sonnet-4-5")`.
  **Verified:** `default_model("anthropic")` always returns `Some(DEFAULT_ANTHROPIC)` where
  `DEFAULT_ANTHROPIC = "claude-opus-4-8"` (`meerkat-core/src/model_profile/catalog.rs:128,238`) — so the literal
  is both **unreachable** *and* **disagrees** with the catalog default, and it hardcodes `"anthropic"` rather than
  the configured provider. **Repair:** delete the literals; treat absent `config_runtime` as an error (match REST's
  non-optional shape) or derive any fallback from `default_model(<configured provider>)`. *Trivial.*
- **F4** (`meerkat-mcp-server/src/lib.rs:260`): a surface-local `default_structured_output_retries() -> 2`
  duplicated in RPC (`handlers/session.rs:161`). **Repair:** flip `SessionBuildOptions.structured_output_retries`
  to `Option<u32>`, resolve `None` against config inside `AgentFactory`, delete both surface constants. *Medium —
  crosses the shared `SessionBuildOptions` contract.*

### C4 — Surface duplicates / locally composes a shared runtime mechanism
**`bypass_shared_seam` · Medium · mcp-server + mob-mcp · ids 9,11 (contested-but-real)**

- **F9** (`meerkat-mcp-server/.../runtime_ingress.rs:44`): MCP re-implements the pre-admission RAII seam
  (`McpRuntimePreAdmissionEntry`/`Registration`/`RegistrationLockLease`) **verbatim** instead of importing the
  shared `meerkat::session_runtime::admission` module whose doc names MCP as an intended consumer and which RPC
  already uses. Semantic facts lower correctly; only the *mechanism* is duplicated. **Repair:** delete the `Mcp*`
  types, implement `RuntimePreAdmissionRestore` exactly as RPC does.
- **F11** (`meerkat-mob-mcp/.../agent_tools.rs:1069`): `dispatch_mob_list` composes the per-mob admission verdict
  locally (`.filter(can_manage_mob)`) while every sibling mutating dispatch lowers the same observation through
  `resolve_current_mob_admission` into `MobMachine`. A real asymmetry with no list-admission seam (fail-closed,
  read-model-ish → Low). **Repair:** lower per-mob through `resolve_current_mob_admission`, or add a `MobMachine`
  list-visibility input.

### C5 — WASM/SDK advertises a JS affordance the runtime never exports
**`capability_advertise_vs_deliver_drift` · Medium · wasm + sdk-web · id 13**

**Verified clean hit.** `meerkat-web-runtime/src/lib.rs:53-54` advertises `comms_peers(session_id)` and
`comms_send(session_id, params_json)` under a "### Comms (placeholder)" doc heading — but **neither has any
`#[wasm_bindgen]`/`pub fn` anywhere** (the names exist only in the comment). `@rkat/web` then declares **both as
required** members of the `WasmModule` interface (`sdks/web/src/runtime.ts:153-154`), asserting the bundle exports
them. Any call would fail at runtime. **Repair:** delete the 2 doc lines + 2 SDK interface members (comms is
already reachable via config + agent tool dispatch + cross-mob wiring); or actually implement the exports.
*Trivial — pure removal.*

## Contested (refuted or correctly narrowed)

| id | Site | Disposition |
|---|---|---|
| 9 | `runtime_ingress.rs:44` | Real but narrow (duplicated mechanism, facts lower correctly) — kept in C4 as Low/Medium. |
| 11 | `agent_tools.rs:1069` | Real asymmetry but fail-closed read-model — kept in C4 as Low. |
| 18 | `mob.ts:894` | Role fact is runtime-owned; the phantom aliases are the real (Low) defect — folded into C2. |
| 36 | `client.ts:808` | `inject_context` status genuinely lowers from the runtime; the `?? ""` is the residual — borderline, refuted as live violation. |
| 37 | `client.ts:1299` | String-passthrough is legitimate translation; only the object-key reconstruction is suspect (C2). |

## Systemic read & clean path

The recurring root shape is **hand-written SDK projection code ignoring the closed, required, already-generated
wire contracts and coalescing the unknown into a permissive default**, instead of faithfully relaying the
runtime-owned status and failing closed. It is the SDK twin of "advertise more than the runtime delivers." The
Rust runtime/contract layer is clean — contracts are closed and documented ("branch on the typed variant, do not
re-derive meaning from free-form status text"); the violations are in the surfaces that ignore them.

**Ordered plan:**
1. **C1 SDK fail-closed sweep** (highest value, 8 of 8 terminal-class fabrications). Build a small
   parse-or-throw helper in each SDK; replace every `?? <permissive>`/`else <success>`; mirror the web SDK; add a
   fail-closed test per site. *No runtime/contract change.*
2. **C2 regenerate off-contract SDK types** — folds into step 1 (bind to generated types, delete phantom
   aliases + object-key reconstruction).
3. **C5 WASM phantom-comms cleanup** — trivial, do anytime (delete 2 doc lines + 2 SDK members).
4. **C3 Rust config-default cleanup** — F1 trivial (delete literals); F4 medium (`Option<u32>` on
   `SessionBuildOptions`, resolve in `AgentFactory`, delete both surface constants).
5. **C4 shared-seam re-use** — F9 import the shared admission module (RPC is the template); F11 lower list
   visibility through `resolve_current_mob_admission` or add a `MobMachine` input.

**Overall: medium, mechanically uniform, front-loaded on the SDK sweep.** No new authorities or machines strictly
required (one optional `MobMachine` list-visibility input for the dogma-purest F11). The bulk (10 of 15) is one
uniform SDK pattern with the fix template already in-tree.

## Cross-audit note

The Rust product surfaces are clean here precisely because they were already migrated to route through
`SessionService`/`AgentFactory`/`MeerkatMachine` with closed, generated contracts — the same discipline that made
Projection Promotion non-systemic. Surface-Owned Truth has retreated to the **last mile that isn't generated from
authority: the hand-written SDK projection code.** Generating the SDK status/roster projections from the closed
wire contracts (rather than hand-authoring them) would structurally close C1+C2 — the largest part of this dogma
aspect — at the source.
