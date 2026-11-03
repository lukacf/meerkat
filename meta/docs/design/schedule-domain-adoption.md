# Schedule-domain seam adoption (meerkat 0.7.25 → mobkit 0.7.30)

Status: RESOLVED at the 0.7.25 pin bump (2026-07-09). The upstream fix
shipped as meerkat #860 ("SessionRuntime arms its own schedule firing host"):
the runtime that binds the `meerkat_schedule_*` tools now arms the firing
host for that same store at session creation / executor attach, so
tools-bound-but-undriven is no longer a representable topology. No composed
"schedule domain" seam API shipped — the single-owner guarantee landed
inside `SessionRuntime` instead, which meta does not use (no meerkat-rpc
dependency). Per the audit below, meta's gateways already bind tools +
host + watchdog from one `ScheduleService`, so NO meta code change was
needed. Stranded-data recovery also resolves upstream: an embedder on
0.7.25 gets a driver pointed at the previously-undriven store automatically
(the downstream app's pending occurrences fire on their upgrade, subject to catch-up
policy). Items 1–3 below stay as the design record; item 2's ordering
discipline remains good hygiene for any future build path.

## The upstream defect pair (context)

1. **Two-writer slot race.** `FactoryAgentBuilder.default_schedule_tools` has
   two writers in embedder processes that also stand up meerkat-rpc surfaces:
   the embedder (binding to its driven store) and
   `meerkat_rpc::SessionRuntime::{new,new_with_config_store}`, which
   unconditionally seed the slot with a dispatcher bound to the realm-bundle
   schedule store inside `sessions.sqlite3`
   (meerkat-rpc/src/session_runtime.rs ~1737/~1877 on meerkat main). Which
   binding a build sees depends on construction order; a reset/reprofiled
   member's rebuild can land on the stale realm-bundle binding.
2. **Undriven store accepting writes.** meerkat-rpc starts its firing host
   lazily (only from RPC schedule handlers / eagerly only in the Router), so a
   direct `SessionRuntime` embedder gets agent schedule tools bound to a store
   nothing drives — schedules plan occurrences no driver claims, silently.

meerkat 0.7.25 carries the root fix: a composed "schedule domain" seam that
binds the agent-facing schedule tools AND the firing host from the same
`ScheduleService` (single-owner binding), plus tools-coupled-to-firing-
authority (an undriven store is loud, never a silent write sink). Exact API
shape may shift while it lands — verify against the released crate.

## Meta audit (as of 0.7.29, meerkat =0.7.23 pins)

Both defects were checked against meta's gateways. Findings:

- **Single writer, attach-first — already holds.** The only
  `default_schedule_tools` writer in a meta process is
  `schedule_wiring::attach_schedule_tools_with_identity_targets`, called at
  `FactoryAgentBuilder` construction, before the builder is consumed into a
  session service and before any member build can run:
  - rpc_gateway.rs: persistent branch builder + attach (all persistent-branch
    builds, including callback builds, flow through this one builder).
  - meta_gateway.rs: same pattern in
    `build_persistent_session_service`.
  Meta does NOT depend on meerkat-rpc, so the second writer
  (`SessionRuntime` self-seeding) is unreachable in a meta-only process.
  The race bites deployments that ALSO construct meerkat-rpc surfaces
  against the same realm store (the downstream app).
- **Tools ↔ firing authority — already coupled.** Whenever tools are
  attached, `schedule_host_inputs` is `Some` and the gateway unconditionally
  spawns the firing host AND the claim watchdog from the SAME
  `ScheduleService` instance (rpc_gateway.rs step-9 preamble;
  `spawn_schedule_host` + `spawn_schedule_claim_watchdog`). The watchdog is
  the loudness affordance: "everything stays pending forever" becomes a
  row-level gateway-log diagnosis.
- **Ephemeral branch: consistent no-schedule posture.** No store is opened,
  no tools attached, no host spawned — there is no silent write sink because
  there is no write path.
- **Second store location.** Meta's gateways open
  `<state_dir>/schedule.sqlite` only. The realm-bundle schedule store inside
  `sessions.sqlite3` is never opened by meta; stranded rows there are
  written by non-meta surfaces sharing the state dir.

Net: meta's current wiring is correct by construction but only
*incidentally* — nothing prevents a future build path (or an embedder mixing
surfaces in-process) from reintroducing a second binding.

## Work items for the 0.7.25 pin bump (rides 0.7.30)

1. **Adopt the composed seam.** Replace the
   `attach_schedule_tools` slot-write + separate `spawn_schedule_host` pair
   with meerkat's schedule-domain API so tools and firing host are bound from
   one `ScheduleService` *structurally* (binding tools to store A while the
   driver runs store B becomes unrepresentable). Keep
   `MobIdentityScheduleToolDispatcher`, the internal-delivery mob host
   wrapper, the resumable-target repair, and the claim watchdog on top of the
   new seam.
2. **Exactly one reachable store.** After adoption, assert (debug log at
   boot) which store file backs the domain; keep `schedule.sqlite` as the
   canonical gateway location unless meerkat's seam dictates the realm
   bundle. If meerkat's `SessionRuntime` seeding change alters
   `FactoryAgentBuilder` slot semantics, re-verify no path re-seeds after
   attach.
3. **Stranded-data recovery.** Schedules already written into an undriven
   realm-bundle store (the downstream app: a security-domain agent's daily digest, ~30 pending
   occurrences in `sessions.sqlite3`) will not retroactively fire from the
   code fix. Ship the recovery matched to what 0.7.25 exposes: prefer a
   one-time migration of live schedules into the driven store; otherwise a
   point-a-driver-once affordance. Guard rails either way:
   - never blind-open a foreign `sessions.sqlite3` with
     `SqliteScheduleStore::open` (open runs migrations and would CREATE
     schedule tables in a file meta does not own — probe with a raw
     read-only connection first);
   - refuse auto-migration when another process may be driving the source
     store (double-firing risk); loud log + explicit opt-in flag/RPC.

## Non-goals

- No speculative meta-side migration before 0.7.25 lands (the upstream
  seam may ship its own pointing/migration mechanism; a parallel meta
  implementation risks colliding with it).
- No change to the ephemeral-gateway posture (no store → no tools → no host).
