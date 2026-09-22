# Parallel Feature Lanes Plan

Status: Proposed
Date: 2026-04-26
Scope: Meerkat repository feature planning; excludes MobKit-only product packages

## Purpose

This plan groups the remote, durable, mob-backed, and multiplayer feature
requests by likely Meerkat repository ownership. The goal is to make it clear
which work can proceed in parallel without creating competing semantic owners.

The grouping follows the current layering stance:

```text
meerkat      = spartan runtime substrate
meerkat-mob  = rich multi-agent substrate
meerkat-mobkit = batteries-included application kit
```

This document covers `meerkat` and `meerkat-mob` work only. Runtime host
registry/discovery as an operational service, project/thread/domain presets,
console projections, and product-shaped workflows belong in MobKit unless they
are first reduced to generic runtime or mob contracts.

## Boundary Rules

- Core Meerkat exposes durable runtime facts, authority boundaries, placement,
  events, approvals, artifacts, metadata, and secure surface contracts.
- `meerkat-mob` may expose rich multi-agent coordination primitives because mob
  orchestration is its purpose.
- MobKit composes those primitives into useful application patterns.
- Surfaces are skins. RPC, REST, CLI, MCP, and SDKs project shared contracts;
  they do not own runtime or mob semantics.
- Machines own semantics where state transitions matter. Shells and stores own
  mechanics, persistence, and projection.

## Excluded From This Plan

The following feature areas should be planned in `~/src/meerkat-mobkit`, not in
this repository, unless a smaller generic contract is identified first:

- runtime host registry/discovery as a deployable registry service
- project, thread, team, or domain topology semantics
- opinionated console and SDK workflows
- project/domain enrollment helpers
- product-specific coordination flows

## Shared Contract Names To Settle Early

Several lanes can progress in parallel only if they agree on shared names and
reference shapes:

- `SurfaceMetadata` / `RuntimeMetadata`
- event envelope identity and cursor fields
- `PrincipalId`, grants, and acting-on-behalf-of fields
- artifact references
- approval references
- placement handles
- mob member identity and external binding references

These should be additive and explicitly typed. Labels and app context remain
opaque metadata and must not become authorization truth.

## Development Methodology

All implementation lanes should use test-driven development.

For each lane:

1. Write or update the contract tests first.
2. Add focused defensive tests for the failure modes most likely to regress.
3. Implement the smallest production change that satisfies the tests.
4. Run the lane's local test target before handing off.
5. Commit the completed lane work in its own worktree.

Defensive tests should cover invalid inputs, stale cursors, authority boundary
mistakes, reserved metadata spoofing, visibility leaks, duplicate/replayed
events, interrupted operations, and recovery paths where applicable. They are
not optional follow-up polish; they are part of the acceptance criteria for
each lane.

## Parallel Agent Workflow

Work should be split across agents running in separate worktrees. Each agent
owns one lane or a clearly bounded slice of a lane.

Agent rules:

- An agent must not edit files outside its assigned lane unless the main agent
  explicitly expands the scope.
- An agent must preserve existing user or agent changes in its worktree.
- An agent must run the relevant local tests for its lane.
- When an agent finishes, it must commit its work in that worktree.
- The commit message should name the lane and the contract being advanced.

The main agent is responsible for integration:

- create and maintain the integration branch `codex/obs-feature-requests`
- review completed agent commits before merging them
- integrate results into `codex/obs-feature-requests` without losing unrelated
  work
- resolve cross-lane contract conflicts
- run the final local verification suite
- open the final PR from `codex/obs-feature-requests` to `main`
- ensure the final criteria below are satisfied before the PR is considered
  ready

## Lane A: Runtime Host and Capability Surface

### Feature Requests

- runtime host introspection
- `runtime/host_info`
- runtime capabilities and health
- feature flags for replay, artifacts, approvals, external members, secure RPC,
  and placement

### Likely Affected Areas

- `meerkat-contracts`
- `meerkat-runtime`
- `meerkat-rpc`
- `meerkat-rest`
- `meerkat-cli`
- facade crate `meerkat`
- docs under `docs/api`, `docs/reference`, and `docs/architecture`

### Parallelization Notes

This is mostly a read-only projection lane and can start early. It should report
facts owned by runtime, auth, artifact, approval, mob, and comms subsystems. It
must not become a semantic authority for placement or topology.

## Lane B: Metadata Contract

### Feature Requests

- labels and app context, reframed as shared metadata
- metadata on sessions, turns/runs, mobs, members, flows, tool calls,
  approvals, artifacts, and events
- metadata query hooks where storage supports them

### Likely Affected Areas

- `meerkat-core`
- `meerkat-contracts`
- `meerkat-runtime`
- `meerkat-session`
- `meerkat-mob`
- `meerkat-rpc`
- `meerkat-rest`
- SDK/schema artifacts

### Parallelization Notes

This is cross-cutting but can begin as an additive type introduction plus
surface pass-through. Indexing and query support can follow. Reserved Meerkat
keys must be protected so clients cannot spoof runtime-owned facts.

## Lane C: Durable Event Replay and Snapshots

### Feature Requests

- uniform `events/*` contract
- cursor replay
- latest cursor
- runtime/session snapshot
- typed event envelope with source, session, mob, run, artifact, approval, and
  metadata references
- reconnect semantics across clients

### Likely Affected Areas

- `meerkat-core`
- `meerkat-runtime`
- `meerkat-store`
- `meerkat-contracts`
- `meerkat-rpc`
- `meerkat-rest`
- `meerkat-cli`

### Parallelization Notes

This is foundational and should share envelope vocabulary with Lane B. It can
progress beside mob-specific replay as long as both lanes converge on cursor
and envelope contracts.

## Lane D: Mob Snapshot, Replay, and Ingress

### Feature Requests

- mob snapshot completeness
- mob event stream completeness
- mob-backed interaction and ingress ergonomics
- ensure mob by stable key or metadata
- ensure ingress/orchestrator member
- send user input to an ingress member
- transcript-friendly interaction events

### Likely Affected Areas

- `meerkat-mob`
- `meerkat-mob-mcp`
- `meerkat-contracts`
- `meerkat-rpc`
- `meerkat-rest`
- SDK/schema artifacts
- docs under `docs/guides/mobs.mdx`, `docs/api`, and `docs/reference`

### Parallelization Notes

This lane is primarily `meerkat-mob` owned. It can start from the existing
`MobEventStore` cursor model and `mob/snapshot` surface, then align with the
generic event replay lane as shared envelope work lands.

## Lane E: External Member V2

### Feature Requests

- tracked turn/event streams for peer-only members
- external member observation snapshots
- health and reachability
- artifact forwarding hooks
- approval forwarding hooks
- robust interrupt, retire, and destroy
- supervisor rebind and recovery
- cross-host wiring beyond in-process bridge sessions
- stable rebinding with the same `AgentIdentity`
- clear unavailable status

### Likely Affected Areas

- `meerkat-mob`
- `meerkat-comms`
- `meerkat-contracts`
- `meerkat-runtime`
- `meerkat-rpc`
- `meerkat-mob-mcp`

### Parallelization Notes

This is a deep mob/comms lane and should remain separate from basic mob
ingress. It touches identity, runtime binding, fences, supervisor bridge, and
comms trust. It will consume approval and artifact hooks when those lanes land,
but it can define forwarding seams before the full protocols are complete.

## Lane F: Durable Approvals

### Feature Requests

- durable approval records
- `approval/list`
- `approval/get`
- `approval/decide`
- `approval/stream`
- tool, session, mob, run, and external member ownership references
- requesting principal or agent identity
- decision actor and audit/provenance
- resumable blocked execution

### Likely Affected Areas

- `meerkat-core`
- `meerkat-runtime`
- `meerkat-machine-schema`
- `meerkat-machine-kernels`
- `meerkat-tools`
- `meerkat-mob`
- `meerkat-contracts`
- `meerkat-rpc`
- `meerkat-rest`

### Parallelization Notes

Approvals should be generic runtime contracts, with mob context layered on top.
If approval state controls execution progress, the state transition must be
machine-owned rather than surface-owned. This lane intersects strongly with
auth and visibility, but an initial single-principal durable protocol can land
before full multiplayer grants.

## Lane G: Artifacts

### Feature Requests

- stable artifact records and handles
- logs, command output, diffs, patches, generated files, test reports,
  screenshots/images, structured reports, and binary blobs
- `artifact/list`
- `artifact/get`
- `artifact/open`
- `artifact/download`
- artifact references in events
- producer, owner, provenance, metadata, and access policy fields

### Likely Affected Areas

- `meerkat-core`
- `meerkat-store` or a new artifact storage module
- `meerkat-runtime`
- `meerkat-tools`
- `meerkat-mob`
- `meerkat-contracts`
- `meerkat-rpc`
- `meerkat-rest`
- existing blob surface

### Parallelization Notes

This lane can proceed in parallel with approvals. It should reuse or generalize
the existing blob surface where possible instead of inventing a competing binary
transport. Tool output can gradually move from transcript-only rendering to
artifact-backed references.

## Lane H: Auth, Principals, Grants, and Visibility

### Feature Requests

- principal identity model
- grants
- acting-on-behalf-of relationships
- typed event audience and visibility
- policy-aware replay filtering
- approval authority
- separation of personal/private state from project-visible state

### Likely Affected Areas

- `meerkat-auth-core`
- `meerkat-machine-schema`
- `meerkat-runtime`
- `meerkat-core`
- `meerkat-contracts`
- `meerkat-rpc`
- `meerkat-rest`
- `meerkat-mob`

### Parallelization Notes

This lane is high-risk and should move deliberately. It should define typed
principal and grant contracts before enforcing replay filtering everywhere.
Labels may project product audiences, but policy must key off canonical
principal and visibility handles.

## Lane I: Secure Remote RPC

### Feature Requests

- production-safe remote RPC control
- explicit remote enablement
- authentication
- encryption
- pairing or scoped token strategy
- revocation
- audit identity
- local-only defaults

### Likely Affected Areas

- `meerkat-rpc`
- `meerkat-comms`
- `meerkat-auth-core`
- `meerkat-contracts`
- `meerkat-cli`
- docs under `docs/api` and `docs/reference`

### Parallelization Notes

Transport hardening can start independently from mobs. It should eventually
consume principal and grant types from Lane H, but safe defaults and explicit
remote enablement can land first.

## Lane J: Placement Handles

### Feature Requests

- environment, worktree, and sandbox placement handles
- host id
- working root
- allowed roots
- worktree id/path
- shell policy
- environment variable policy
- credential profile
- container/sandbox id
- placement metadata in tool calls and events

### Likely Affected Areas

- `meerkat-core`
- `meerkat-runtime`
- `meerkat-tools`
- `meerkat-session`
- `meerkat-contracts`
- `meerkat-rpc`
- `meerkat-rest`
- `meerkat-mob`

### Parallelization Notes

This can begin as a structured execution-context contract for shell/tool
execution. Mob support should layer on by assigning member-level placement
without making `cwd` the semantic authority.

## Lane K: Mob Coordination Primitives

### Feature Requests

- `WorkIntent`
- `ResourceClaim`
- advisory, soft reservation, and exclusive claim types
- active-work queries
- overlap/advisory events
- generic mob task-board extension

### Likely Affected Areas

- `meerkat-mob`
- `meerkat-contracts`
- `meerkat-mob-mcp`
- `meerkat-rpc`
- `meerkat-machine-schema`, if coordination state becomes machine-owned

### Parallelization Notes

Given `meerkat-mob`'s richer design philosophy, generic coordination records
fit here if they stay product-neutral. MobKit can later wrap these records into
project, domain, and team workflows.

## Suggested Waves

### Wave 1: Surface Facts and Basic Ergonomics

- Lane A: Runtime Host and Capability Surface
- Lane B: Metadata Contract
- Lane D: basic Mob Snapshot, Replay, and Ingress
- Lane J: Placement Handles

This wave establishes the handles and projections that later features will
reference.

### Wave 2: Durable Objects and Replay

- Lane C: Durable Event Replay and Snapshots
- Lane G: Artifacts
- Lane F: Durable Approvals

This wave makes long-running remote work reconstructible and auditable.

### Wave 3: Remote and Multiplayer Authority

- Lane E: External Member V2
- Lane H: Auth, Principals, Grants, and Visibility
- Lane I: Secure Remote RPC

This wave enables production remote execution, cross-host members, and
multi-client or multiplayer visibility boundaries.

### Wave 4: Rich Mob Coordination

- Lane K: Mob Coordination Primitives

This wave adds generic mob-level work coordination after replay, metadata,
visibility, artifacts, and approvals have stable reference types.

## Coordination Risks

- Metadata must remain opaque and non-authoritative.
- Runtime host introspection must not become topology authority.
- Event replay and mob replay must not fork into incompatible cursor/envelope
  models.
- Approvals that pause or resume execution need machine-owned state.
- Artifacts should not duplicate blob transport unless the blob surface is
  insufficient after review.
- External member work must preserve the separation between `AgentIdentity`,
  `AgentRuntimeId`, `FenceToken`, runtime binding, and bridge transport.
- Placement handles must describe execution context without making filesystem
  paths semantic identity.

## Success Criteria

- Each lane has a clear primary crate owner and surface projection plan.
- Two or more lanes can be implemented concurrently without editing the same
  semantic owner.
- Downstream clients can build durable remote and mob-backed experiences using
  typed Meerkat handles.
- MobKit can add useful project and console workflows without forcing those
  nouns into Meerkat core.

## Final PR Criteria

The total output of this effort should be a pull request from
`codex/obs-feature-requests` to `main`.

The PR is not ready until:

- no violations of
  [`meerkat-runtime-dogma.md`](meerkat-runtime-dogma.md) are
  introduced in any shape or form
- every lane has contract-first tests and defensive tests for its high-risk
  failure modes
- all local tests pass
- local e2e smoke tests pass
- local e2e live tests pass
- CI is green for the PR
- completed parallel-agent work has been reviewed, integrated, and committed on
  `codex/obs-feature-requests`
