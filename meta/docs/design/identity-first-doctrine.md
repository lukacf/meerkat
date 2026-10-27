# Identity-First Doctrine

**Status:** DECIDED 2026-07-06 (D1 + D2 approved); D3 decided 2026-08-21 and
shipped in MobKit 0.8.19 on meerkat 0.8.25. This is the design doc the
2026-07-06 audit proved never existed — the coexistence of the two member
authorities had been an accident of chronology, ratified only diffusely in
code comments. This document is the recorded decision.

**Since Meta 0.9.4** Core's mob machine holds an identity's session,
generation and fence, and Meta keeps no continuity records or leases of its
own. The split between the two planes below is unchanged; where this record
names Meta's 0.9.3 identity substrate, read Core's member history instead.

## The two authorities

Meta has two member-lifecycle authorities. "Identity-first vs
session-based" is a false dichotomy — identity-first agents ARE session-owned
(the bridge creates sessions like everything else). The real split is which
authority owns the member's lifecycle:

The table describes Meta's intended allocation of responsibilities, not the
storage limits of the underlying mob/session substrate.

| | Mob plane (classic) | Identity plane |
|---|---|---|
| Owner | mob-actor roster (`MobHandle`) | `IdentityRuntime` |
| Key | member `AgentIdentity`/member identifier (a Meta-generated incarnation alias where applicable), not the per-runtime `AgentRuntimeId` | stable application `AgentIdentity` |
| Durability | ephemeral-worker usage policy | Core-held session and generation per identity, resume-first restore |
| Disposal | strict archive invariants (ask-20 class) | tolerant session-owned cleanup; a refused wake is answered typed |
| Stand-up | `spawn`/`ensure_member` (`SpawnMemberSpec`) | roster reconcile (`restore_flow`) |

Mob/session storage and member launch modes support persistence and resume;
this does not mean every worker is automatically resumed. Identity-plane
continuity and roster reconciliation remain the prescribed abstraction
for durable populations under D1.

History (verified): the mob plane predates identity-first (`ensure_member`
2026-03-06, roster API PR #37; identity-first PR #57, 2026-04-01, as an
additive layer). No deprecation was ever recorded — until now, deliberately:
**neither plane is deprecated.** They have different jobs.

## D1 — the dual-plane model (decided)

**Durable members live on the identity plane. Ephemeral workers live on the
mob plane.** We do not force workers onto identities and we do not build an
"identity-lite" tier.

Why: workers have no resume story by design — nobody wants a person-sweeper's
transcript restored across a restart — and per-worker continuity records +
leases are pure overhead in the hottest path of the largest deployment (the operator deployment:
~600 eternal identities, heavy worker churn, BigQuery-backed stores; every
reconcile would pay for a table of dead worker identities). The bug that made
"identity everywhere" look necessary (retire/respawn stranding never-ran
members) is a mob-plane defect with an upstream fix (ask 20), not an
architecture signal.

Consequences:

- The mob-plane machinery (`MobHandle`, `SpawnMemberSpec`, agent mob tools
  `mob_spawn_member`/`delegate`, `implicit_delegate_idle_retire`) is the
  **worker plane** — permanent, first-class, and additionally the substrate
  the identity bridge itself is built on. It is never "legacy to delete."
- What IS deprecated (later, phase 3) is using the mob plane for **durable
  populations**: the member-per-user pattern, long-lived coordinators on
  `ensure_member` (meerkat-fugue), etc. Docs stop teaching it now (phase 1).
- `force_cancel_member` (after upstream M1 fixes the SIGABRT) is a
  both-planes primitive: cancel-by-member-id covers workers; an
  identity-addressed wrapper covers durables. There is currently no
  identity-side cancel at all — that wrapper is part of this doctrine's
  follow-up work.

Deployment census at decision time (audit-verified):

- **The operator deployment** (production, 100+ users): dual-plane already — eternal fleet
  identity-first via the builder (roster + BigQuery-backed continuity and
  leases, LazyMaterialize); ALL worker churn on the mob plane (programmatic
  `spawn_worker_with_parent` + agent `mob_spawn_member`, idle-retire reaping).
  This doctrine formalizes the operator deployment's model; the operator deployment migrates nothing.
- **The downstream app**: identity-first end to end for durables (SDK roster provider,
  `reconcile_identity` on boot, console lifecycle buttons hidden); mob plane
  used only for agent-spawned helper churn. Already doctrine-conformant.
- **meerkat-studio**: currently all mob-plane via `meta_gateway`; migrates
  to identity semantics zero-flag-day via capabilities detection (their
  crews are definition-derived rosters; domain agents are single durable
  identities).
- **meerkat-fugue** (stale, pinned 0.6.52): durable coordinator on
  `ensure_member` — the anti-pattern; migration note owed when it revives.

## D2 — converge construction, merge binaries later (decided)

The K1/K2 field failures came from the two gateway binaries embedding the
same `UnifiedRuntime` with divergent wiring: `rpc_gateway` got the full
identity substrate, `meta_gateway` got none. The drift class is caused by
divergent **construction**, not by binary count.

**Decided:** extract one shared runtime-construction path — identity
substrate always built (the identity runtime and the stores it reads;
the roster may be empty) — and have both binaries call it. The actual binary
merge is a later packaging step (target: 0.8), because the two binaries have
deliberately different stdin contracts (`rpc_gateway`: stdin JSON-RPC loop;
`meta_gateway`: one init line, then stdin-EOF keep-serving that
meerkat-studio's persistent-helpers feature depends on).

Consumer contract to preserve through convergence AND the eventual merge
(from meerkat-studio, the primary console consumer):

1. stdin init handshake → `http_base_url`, `runtime_id`, `launch_state`
   (including `"resumed"`).
2. Resume-by-key (`tux-runtimes.json` registry) + stdin-EOF keep-serving.
3. Console surface: `/console/rpc`, `/console/experience`,
   `/console/modules`, the SSE streams, timeline/event cursors.
4. Roster materialization from definitions — a way to stand up N named
   members from a mob.toml/mobpack (identity roster or `ensure_member`
   re-dispatch, either is fine).
5. `meta/capabilities` advertising the identity RPC set — consumers gate
   on it and switch without a flag day.

## D3 - role migration is declared by the activation, never inferred (decided)

A durable member's role is part of its durable identity, not a config knob: the
stored role, the member's comms name and its mob-member binding are one fact.
A resume whose target role differs from the durable predecessor role is
therefore a restamp of identity, and meerkat 0.8.25 refuses it unless that
exact resume request declares the migration
(`MobError::MemberRoleMigrationRequired`, upstream
`meerkat-mob/src/build.rs`). Refusing is the correct default: the alternative
is an unintended profile edit silently restamping a live member's durable role,
comms name and binding, with nothing afterwards pointing at the edit as the
cause.

**Decided:** migration authority is supplied by the ACTIVATION, per exact
identity, and Meta never derives it. The host names it at the top level of
the gateway boot payload, as a sibling of `runtime_options`:

```json
"role_migrations": [{"identity": "domain:automation", "from_role": "domain"}]
```

The list is installed once on `MobSessionBridge`
(`with_role_migration_declarations`, `identity_first/bridge.rs`), which lives
for exactly one boot, and is read only by an exact-identity map lookup
(`declared_role_migration_in`). `resume_session` passes the looked-up
predecessor role straight into `MemberLaunchMode::Resume { resume_from_role,
.. }` and does nothing else with it. Nothing Meta reads can create, widen or
withdraw a declaration. Meta never infers or widens migration authority and
does not recover from a role-migration refusal by altering the declaration or
falling back to a fresh spawn. Separately, a custody- and durable-source-checked
`MemberAlreadyExists` collision repair can retry the unchanged resume spec;
that repair does not change migration authority.

The properties that make an activation-scoped carrier safe, each load-bearing:

- **Boot-scoped by construction.** Nothing persists the declarations, and
  Meta has no store that could reconstruct them. Dropping an entry from the
  next boot payload is how the authority goes away.
- **Exact identity, in both directions.** The map is keyed by `AgentIdentity`,
  so no prefix, suffix or case variant of a declared identity inherits
  authority. Widening one declaration into a family of members is precisely the
  failure the typed refusal exists to prevent.
- **Meerkat decides, Meta only carries.** The declared predecessor is
  re-verified against durable state and a mismatch refuses with
  `MobError::MemberRoleMigrationRejected`, so a mistyped `from_role` cannot
  authorize an unintended restamp.
- **A completed migration's declaration is inert.** Once the durable and
  requested roles agree, admission returns before it ever reads
  `resume_from_role`, so a declaration left in the payload after the migration
  landed does nothing; it is not a repeat restamp. That is why an IDENTICAL
  repeated declaration is accepted on purpose while a CONFLICTING pair for one
  identity is refused (`conflicting_role_migration_declaration`,
  `identity_first/types.rs`): a host proves inertness by carrying the same
  declaration into a later activation and showing that nothing moves. Nothing
  but map order could otherwise pick which predecessor becomes authority.
- **A bad payload refuses the boot on both binaries.** Arming nothing silently
  would resurface later as the original refusal with nothing naming the payload
  as the cause.
- **Only the identity plane carries it.** Every mob-plane resume path passes
  `resume_from_role: None`, so a role change reached through the worker plane
  still fails closed, which is the D1 split doing its job.

Both gateway binaries wire it, and their names differ by one word.
`rpc_gateway` reads it out of its untyped init params, refuses a malformed or
conflicting payload at init scope with `-32602` answered on the request id, and
then installs it inside the roster-provider branch. `meta_gateway` takes it
as a typed `InitParams.role_migrations` field and installs it inside the
identity-first block; its refusals both carry a null `id`, `-32602` when the
typed init params fail to deserialize and `-32603` from the conflict check,
which sits inside that same identity-first block. Under
`identity_first: false` that block never runs, so on that path nothing is armed
and a self-contradicting payload is not refused at all; `identity_first`
defaults to true. Declarations that reach `rpc_gateway` with no roster provider
likewise arm nothing, because there is no identity plane to migrate on.
Anything instructing an operator to launch a gateway must therefore name the
binary: the wrong one boots far enough to look like it worked.

Host side: `MetaBuilder.role_migrations([...])` accepts
`RoleMigrationDeclaration` dataclasses or plain dicts, validates both, raises
`ValueError` on a conflicting pair, and emits no `role_migrations` init-params
key at all when unused (`sdk/python/meerkat_meta/builder.py`,
`identity_first_models.py`, `runtime.py`). The TypeScript SDK's builder exposes
no equivalent setter, so a TypeScript host cannot authorize a role-migrating
resume today. The wire key names are pinned by one shared fixture,
`crates/meerkat-meta/tests/fixtures/role_migrations_init_params.json`, read by both
the Rust parser test (`identity_first::bridge` tests) and its Python twin
(`sdk/python/tests/test_role_migrations.py`), so a rename on either side goes
red instead of both staying green while a host arms nothing.

## Phases (1+2 ship together in 0.7.25, with meerkat 0.7.19)

- **Phase 0 (shipped, #240):** opt-in `identity_first: true` on
  `meta_gateway` + identity arms on the console member RPCs
  (`ensure_member` → roster upsert + reconcile; `retire_member` → tolerant
  identity retire; `respawn_member` → identity reset). Proof: retire/respawn
  of never-ran members succeeds — ask-20 class unreachable on this surface.
- **Phase 1 (0.7.25):** this document + skill + docs flip. Close the ask-20
  reachability gaps: identity arms in `rpc.rs` (the SDK stdin dispatcher has
  none), the identity-named console RPCs' mob-plane fallbacks (the
  UI-reachable path), console-aggregator disposal-tolerance parity.
- **Phase 2 (0.7.25):** shared gateway construction; `identity_first`
  default-on in `meta_gateway` (opt-out `identity_first: false` kept for
  one release, changelog-flagged); `meta/capabilities` advertises the
  identity set as baseline and frames the member set as the worker plane.
- **Phase 3 (0.8):** binary merge as packaging; deprecate mob-plane NAMES
  for durable use only. The worker plane remains.

**Operator deployment constraint (binding):** The operator deployment is the only system in serious production.
Every phase is additive to the library-builder path; the eternal-fleet path
and the worker plane are untouched; anything that could affect the operator deployment gets an
explicit migration note in the changelog before release.

## Related

- Audit + verification record: session memory `identity-first-audit`
  (2026-07-06 workflow, 10 agents, adversarially verified).
- Upstream asks: `docs/design/upstream-asks.md` — ask 20 (mob-plane disposal
  tolerance), M1 (cancel), asks 16–19 (schedule firing), all targeted at
  meerkat 0.7.19.
- The identity-first implementation contracts: `identity_first/contracts.rs`
  (CONTRACT-01..05), `identity_first/adapters.rs` (CONTRACT-08, REQ-27/28).
