---
title: "Dogma Audit — Terminal Truth Laundering"
description: "Workspace audit for Rule 8 (Terminality and Faults Are Explicit) / Split Terminality."
icon: "magnifying-glass"
---

# Dogma Audit: Terminal Truth Laundering (Rule 8 / Split Terminality)

> **Postscript 2026-06-11:** point-in-time audit record (PR #756 head). Open/confirmed findings below were re-adjudicated and closed by the PR #759 campaign — see [`PR759-final-ledger.md`](PR759-final-ledger.md) (close-out: open set zero).

**Audit target:** PR #756 head `worktree-third-dogma` @ `688f56442`
**Date:** 2026-06-05
**Dogma:** Rule 8 — *Terminality and Faults Are Explicit*; pattern *Split Terminality*.
**Test:** a failure / timeout / cancellation / dropped delivery / malformed data / partial execution / unknown
state must not be converted into a benign terminal class (success, empty, None, not-found, destroyed, default,
generic string error) that loses the typed fault. *"One condition, one ending. Success must prove itself. A logged
fault is not a handled fault. Cancellation is a transition, not an interruption."* A site is a real violation only
when the typed fault is **lost**, the caller **cannot distinguish** it from genuine success/absence, **and** the
lost fact has semantic consequence.

## Method

Two agent workflows (same harness as the prior audits). (1) A 10-agent locus map inventoried **197 sites** — and
**over-flagged badly** (142 marked suspect, 72%; one agent flagged 29/29). (2) A 142-candidate verify pass: each
candidate got an independent classifier that **re-traced what the caller receives on failure and whether the
authoritative outcome is preserved elsewhere**; every suspect faced a 3-lens adversarial refuter panel
(fault-preserved / single-meaning / canonical-terminal), confirmed at ≥2 non-refutes; a synthesis agent clustered
survivors. Load-bearing claims (both High + the config cluster + a key *cleared* item) were hand-verified.

**Result:** 142 candidates → **16 confirmed, 7 contested, 119 legitimate**. **11 root clusters, 2 High.** The
adversarial panels did the heavy lifting: **119/142 refuted**, including the entire `let _ = tap_emit(...)`
event-tap "dropped-delivery" cluster (35 sites the map flagged) — correctly cleared as best-effort observability
whose authoritative terminal truth is carried by the return value / machine state / persisted event.

## Bottom line

- **Widespread?** **No — once adversarially filtered.** The raw suspect count was huge, but the codebase's typed-error
  discipline (`?` propagation, typed `ToolError`/`SessionError`/`LlmError` with `error_code()`) holds in the vast
  majority. Only **~16 genuine** violations remain, and ~6 of those are bounded/Low. The map's 142 was 88% noise.
- **Systemic?** **Yes — one shape.** A fault is dropped at a leaf (`.ok()?` / `unwrap_or_default()` / `let _ =` /
  swallow-to-`tracing::error!`+return) instead of propagating the typed fault — **and the same condition is handled
  correctly by a sibling** (Anthropic `StreamParseError` vs OpenAI silent drop; `config/get` vs `capabilities/get`;
  the service re-persist vs the loop's best-effort save). The dogma-correct pattern almost always already exists nearby.
- **Live impact?** Mostly latent (requires a corrupt config, a malformed provider chunk, or an authority-invariant
  breach to bite) — but **two are High** because they corrupt the agent turn outcome and the ops barrier.
- **Complexity?** **Medium**, most clusters small/mechanical (copy the sibling's typed propagation). No new authorities.

## High-severity clusters (hand-verified)

### H1 — Provider streaming seam drops malformed/truncated output into synthetic Success
**`malformed_to_default` + `split_terminality` · High · anthropic/openai/llm-core · members 43,46,50**

The **same condition terminates two ways across provider parsers** — the canonical Rule-8 violation:
- **id 50:** `ToolCallBuffer::try_complete()` returns `Option`, and `serde_json::from_str(&self.args_json).ok()?`
  (`meerkat-llm-core/src/types.rs:445`) silently drops malformed tool-args after `finish_reason == tool_calls` → the
  intended tool call **vanishes** and the turn emits `Done{Success}`. Anthropic, for the identical corruption,
  returns typed `LlmError::StreamParseError` (`meerkat-anthropic/src/client.rs:1638`).
- **id 46:** Anthropic clean-EOF-without `message_stop` synthesizes `Done{Success{EndTurn}}` (`client.rs:1423-1432`),
  pre-empting `ensure_terminal_done`'s `IncompleteResponse` and dropping any in-flight `tool_use`; OpenAI/Gemini don't.
- **id 43:** `parse_sse_line` `.ok()`-drops a malformed real `data:` event → content truncation laundered to Success.

**Repair:** one canonical terminal for malformed/truncated output, matching Anthropic's `StreamParseError`: make
`try_complete -> Result<ToolCall, LlmError>` (map serde failure to `StreamParseError`, carve out empty/no-name);
give `parse_sse_line` a typed parse-failure distinction; delete the synthetic-Success branch and let
`ensure_terminal_done` classify EOF as `IncompleteResponse`. *Medium — typed faults already exist; streaming code is test-heavy.*

### H2 — ops_lifecycle authority faults swallowed (op reported complete + waiter hangs)
**`split_terminality` · High · meerkat-runtime · members 97,101**

- **id 101:** `maybe_satisfy_wait(&mut self)` (returns nothing) swallows `try_satisfy_wait_all_authority()` errors —
  an `OpsLifecycleError::Internal` (authority-invariant corruption) — into `tracing::error! + return`
  (`ops_lifecycle.rs:1762-1768`). Its caller `finalize_terminal` returns `Ok(())`, so `complete/fail/cancel_operation`
  report the op **fully terminal** while the wait-all barrier's oneshot never fires → the waiting consumer **hangs**.
- **id 97:** completion-cursor poison laundered to `None` (`ops_lifecycle.rs:3149`).

**Repair:** `maybe_satisfy_wait -> Result<(), OpsLifecycleError>`, `?`-propagate from `finalize_terminal`, and route
the barrier-satisfaction failure to a typed terminal for the op + the waiter (don't report complete on authority
corruption). *Medium — local to one file with a prevailing propagation convention.*

## Medium / Low clusters

| Cluster | Sig | Cx | Evidence |
|---|---|---|---|
| **Config-read handlers launder `ConfigError` → `Config::default()`** (114-117) | malformed_to_default | small | `config_store.get().await.unwrap_or_default()` at REST `lib.rs:3253` (`/capabilities`, returns bare `Json` — can't even propagate), `:3315` (`/models`), RPC `router.rs:1706` (`capabilities/get`), `:1734` (`models/catalog`). Benign absence is already `Ok(default)` inside the store, so every `Err` is a TRUE fault → success payload with phantom capabilities/empty catalog. Correct sibling exists: `handlers/config.rs:165` `match … Ok/Err`. **Verified.** |
| **In-loop durable-projection writes swallow `Err` then emit success** (17,12) | split_terminality | medium | `publish_committed_visible_set`'s typed `AgentError` dropped to `warn!`, then `AgentEvent::ToolConfigChanged{boundary_applied}` + "change applied" notice emitted; durable tool-visibility projection (recovery source) diverges from in-memory authority. |
| **Realm lease reader launders corrupt-but-live lease to absent — and deletes the evidence** (110,111) | malformed_to_default | small | `inspect_realm_leases_in` has no unparseable channel; a lease `*.json` failing serde lands in neither `active` nor `stale`, and under `cleanup_stale=true` (every prune/delete caller) is **auto-deleted** — defeating the `delete_realm` "active leases block destructive prune" safety guard. **Safety consequence.** |
| **Cleanup/rotation drops typed `Err` with `let _ =`** (2,78) | error_swallowed_to_success | small | `rotate_auth_lease_auth_binding`'s `let _ = handle.release_lease(&previous_key)` drops the release outcome during LLM hot-swap while the acquire branch maps to `AgentError` (asymmetric); stale lease may conflict with the new bind. |
| **Broadcast subscription silently drops `Lagged` events to pure observers** (122) | dropped_observation | medium | `subscribe_pending_session_events` on `RecvError::Lagged(n)` `continue`s, dropping n `AgentEvent`s (incl. `RunCompleted`/`RunFailed`) with no record; asymmetric with the existing `NotificationQueueOverflow` modeling. Pure observers can't reconstruct terminal truth. |
| **Python SDK post-connect tool registration laundered to success** (133) | error_swallowed_to_success | small | `_register_tool_with_server` wraps `tools/register` in `except Exception: pass` — catches protocol errors / server rejection, not just shutdown; tool silently never registered. |
| **RPC success envelope fabricates `result:null` on serialization failure** (120) | error_swallowed_to_success | trivial | `RpcResponse::success`'s outer `to_raw_value` failure fabricates `{result: "null", error: None}` — a valid JSON-RPC **success** carrying null, on the authoritative reply channel. |
| **Store enumeration drops faults to empty/none** (107,108,104,105) | error_to_empty | small/Low | `read_all_metadata_sidecars` drops a malformed `.meta` to warn+continue → incomplete `list()` (bounded: canonical `<id>.jsonl` still read directly). 104/105 (`SessionIndex` trait) verified **dead** (zero callers) → delete. |

## Contested / cleared (validates panel quality)

The map's two most prominent agent-loop flags were **correctly refuted**: **id 1** `save_session_best_effort`
(`meerkat-core/src/agent/state.rs:255`) swallows the store save — but it's a redundant optimization: the persistent
service re-persists with `?`-propagation (`persist_full_session_or_discard_live`) and `RunResult` never claims
persistence (**verified**). The **entire 35-site `let _ = tap_emit(...)` event-tap cluster** was refuted — best-effort
observability whose authoritative terminal outcome is the return value / machine state / persisted event, not the
dropped event. (Worth keeping an eye on: this is only legitimate *because* observers don't reconstruct terminal truth
from those events — cluster 122 is the one place that assumption breaks.)

## Systemic read & clean path

The recurring shape: **a leaf drops a typed fault** (`.ok()?` / `unwrap_or_default()` / `let _ =` /
`tracing::error!`+`return`) **into a benign terminal**, while a sibling handles the same condition correctly. The fix
is uniform: **propagate the typed fault that already exists, matching the sibling.** Because the typed faults and the
correct conventions are already present, no new authorities are needed.

**Ordered plan:**
1. **H1 provider seam** (highest blast — corrupts the agent turn): `try_complete -> Result`, typed `parse_sse_line`
   skip, delete the synthetic-Success branch. Unifies malformed-output terminality across all three providers.
2. **H2 ops_lifecycle**: `maybe_satisfy_wait -> Result`, `?`-propagate; don't report op-complete on authority corruption.
3. **Config handlers (114-117)**: replace `unwrap_or_default()` with the `handlers/config.rs` propagate-or-error
   pattern; make `/capabilities` return `Result<_, ApiError>`. *(small, mechanical)*
4. **Realm lease (110,111)**: add an `unparseable` channel; never auto-delete a corrupt-but-live lease (safety).
5. **The small set**: RPC envelope typed-error (120), cleanup `let _ =` → `?` (2,78), SDK `except: pass` → propagate
   (133), `Lagged` → typed gap observation (122), delete dead `SessionIndex` (104,105).

**Overall: medium**, front-loaded on the two Highs; the long tail is mechanical "copy the sibling's typed propagation."

## Cross-audit note

This is the fifth and final-style aspect, and it confirms the through-line of all five: **the typed/authoritative
core is well-governed; residual violations are leaves that drop a fault the surrounding system already models.** The
strongest evidence is *split terminality itself* — the violations are precisely where one path (Anthropic
`StreamParseError`, `config/get`, the persistent service) does it right and a twin (OpenAI `.ok()?`,
`capabilities/get`, the loop best-effort save) launders. Three audits now converge on consolidation targets: one
default-model owner (R6/R7/R9), one backend-kind owner (R4/R7), and here **one canonical terminal per condition**
across the provider parsers. The biggest *structural* lever remains R9's: the missing rerun-and-diff/parity gates
that would catch this class of drift before it lands.
