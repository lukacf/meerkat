
# RCT Methodology + Agent Gates (Master Protocol)

This repository follows **RCT**: a contract-first, test-driven methodology designed for AI-assisted development where the dominant risks are:
- **representation mismatch** (wire/storage/tool formats don’t behave like assumed), and
- **integration hell** (independently “perfect” components don’t fit together).

RCT is **stack-agnostic** and applies to backend systems, UI-heavy apps, data pipelines, infra, and mixed projects.

---

## 1) Core Principle

**Representations first. Behavior second. Internals last.**

If core data cannot reliably round-trip across the project’s real boundaries (API, DB, browser state, files, vendor tools), the spec/architecture must be challenged before implementation proceeds.

---

## 2) Definitions

### 2.1 Representation Boundary
Any surface where data changes form and assumptions can break, e.g.:
- API JSON ↔ typed structures ↔ validation
- DB rows ↔ ORM/client ↔ typed models
- file formats ↔ parsers ↔ schemas
- localStorage/indexedDB ↔ app state ↔ migrations
- CLI subprocess output ↔ parsers ↔ session IDs
- message queues ↔ payload envelopes ↔ consumers

### 2.2 RCT (Representation Contract Tests)
A small suite of tests that prove:
- round-trip compatibility across boundaries
- stable encoding rules (IDs, enums, optional fields)
- migration/version safety
- deterministic behavior of external tools and contracts

RCT is intentionally **small and ruthless** (≈10–50 tests). It is a permanent guardrail to prevent massive refactors later.

---

## 3) The Gates (Order of Work)

### Gate 0 — RCT MUST be green
Before writing meaningful E2E/integration tests or implementing features, establish RCT for the core nouns and critical boundaries.

Gate 0 must validate (as applicable):
- serialization/encoding strategy for enums/IDs/links
- optional field semantics (NULL vs unset/NONE) and normalization rules
- migration/versioning behavior
- minimal persistence round-trip (write → read → equals)
- external contract shapes (CLI JSON output, API schemas, etc.)

**If Gate 0 fails:** stop and revise the spec or representation strategy. Do not proceed.

> RCT should remain small and deterministic. It should run fast and be executed frequently.

---

### Gate 1 — E2E scenarios are written and executable (red is OK)
Define E2E scenarios as black-box “Given / When / Then” flows using only real external interfaces:
- HTTP/CLI/UI automation/etc.

E2E tests may be red, but must:
- start reliably
- fail for expected reasons (not “couldn’t boot,” “connection refused,” missing env)

---

### Gate 2 — Integration choke-point tests are written and executable (red is OK)
Write integration tests that validate wiring between subsystems and enforce cross-component invariants.

Integration tests are not “unit tests that hit a DB.”
They must prove **cross-component behavior** (at least two subsystems).

Common choke points:
- tenancy routing, sessions, authz
- migrations + startup
- outbox/streams/cursors
- partition routing (if applicable)
- cross-tool continuity (if applicable)
- UI ↔ API ↔ state/cache (for frontend)

---

### Gate 3 — Unit tests are written as needed
Unit tests accelerate local development, but must support getting integration/E2E green.
Unit tests do not replace integration/E2E.

---

## 4) Implementation Order (Bottom-Up)

Once Gates 0–2 exist:
1) implement smallest internals to satisfy unit tests (if needed)
2) prioritize making integration tests pass (main work)
3) finally make E2E scenarios pass (system truth)

**Rule:** the system is “working” only when E2E scenarios pass.

---

## 5) What Counts as an Integration Test (Strict)

An integration test must:
- exercise at least **two** subsystems (API/storage/policy/migrations/queue/UI/etc.)
- assert at least one cross-component invariant (not just “it returns 200”)
- use real boundaries where possible (real DB/tool, or a contract-faithful stub)

Examples:
- API write emits outbox event in the same transaction
- authz/redaction affects totals/cursors correctly
- CLI session ID persists and resume uses it
- migration applies and preserves existing data

---

## 6) PR Acceptance Rules (Simple)

A PR is acceptable when:
- Gate 0 RCT remains green (or is updated and green)
- at least one integration or E2E test improves (red → green) for meaningful changes
- no agent gate has an unresolved hard veto with evidence

“Unit-only PRs” are discouraged unless they directly unblock an integration test.

---

## 7) Agent Gates (Review Board)

AI-assisted development needs review governance. We use **agent gates**: specialized reviewers with narrow veto scopes.

### 7.1 Universal Gates (Always Enabled)

1) **RCT Guardian** (Hard veto)
Blocks only if:
- a representation boundary changed without updated RCT
- serialization/migration contract is untested or regressed
- NULL/unset semantics drifted
- DB/wire/tool contract changed without tests pinning required fields

2) **Integration Sheriff** (Hard veto)
Blocks only if:
- PR touches a known choke point without integration coverage
- wiring invariants are violated (routing, authz, cursor stability, partitions, streams, etc.)
- changes increase the integration gap (more moving parts, no system proof)

3) **Spec Auditor** (Hard veto, narrow)
Blocks only if:
- MUST/REQUIRED behavior in spec is missing or contradicted
- contract drift occurred without updating the spec
- user-facing behavior deviates from defined scenarios

### 7.2 Spec-Derived Gates (Post-Spec Generated)

Beyond the universal triad, additional reviewer agents are **selected or generated after the spec exists**.

Rationale:
- Once the spec is written, we can identify which risks dominate (security, performance, UX, concurrency, migrations, etc.).
- We can also define **narrow, tailored gate agents** for specific subsystems or invariants (e.g., “Cursor & Pagination Gate”, “Migration Safety Gate”, “Browser Automation Gate”, “Rate Limiting Gate”, “Vector Partition Gate”, “Outbox Ordering Gate”).
- These spec-derived agents improve integration and correctness by focusing review attention on the exact seams where systems typically fail.

Spec-derived gates are created in one of two ways:
1) **Selected** from a library of standard gates (security, ops, performance, UX, concurrency, code health).
2) **Generated** from the spec as narrow “micro-gates” that each enforce one critical invariant area.

All spec-derived gates must follow the same governance rules:
- Narrow veto scope (hard veto only within scope)
- Evidence-based blocking (test failing/missing, or spec clause + reproduction)
- Veto budget per PR
- Everything else is non-blocking

#### Examples of Standard Gates
- Security & Privacy Gate
- Ops/Deployability Gate
- Performance/Scale Gate
- UX/A11y Gate
- Concurrency/Ordering Gate
- Code Health Gate (soft; blocks only for clear production risk)

#### Examples of Spec-Generated Micro-Gates
- Migration Safety Gate (schema changes, rollback, backward compatibility)
- Cursor Stability Gate (pagination correctness under filtering)
- Policy/Redaction Gate (no leakage + envelope correctness)
- Session Continuity Gate (cross-tool continuity rules)
- Partition Routing Gate (strict retrieval under compartmentalization)
- Extension Contract Gate (manifests, ref rewriting, routing)

Spec-derived gates are recorded in `docs/review-board.yaml` under `enabled_gates`,
and must be updated when the spec evolves.


---

## 8) Checks and Balances (Anti-Pedantry Rules)

To prevent deadlock and uncontrolled pedantry:

1) **All hard vetoes must cite evidence**
A reviewer can block only if they cite:
- a failing/ missing test tied to a gate (RCT/integration/E2E), OR
- a spec clause + deterministic reproduction

2) **Test-or-nonblocking**
If a concern cannot be expressed as:
- a required test, or
- a spec violation with reproduction
then it is non-blocking feedback.

3) **Veto budget**
Each gate has a maximum number of blocking issues per PR (e.g., 2–3).
Extra comments are non-blocking.

4) **Arbiter**
There is always an arbiter (human or designated agent) who:
- enforces reviewer scope,
- resolves conflicts,
- can override soft vetoes,
- can require reviewers to convert feedback into tests.

---

## 9) Reporting Discipline (for AI agents)

When claiming progress, always state:
- which gate you worked on (0/1/2/3)
- which tests you added/changed
- which tests moved red → green
- which representation boundary was exercised

Avoid “platonic completion” claims based on code quality alone.
Passing system-level tests is the source of truth.

---

## 10) Generating the Agent Board from Spec + Repo (Minimal)

We do not interview the user. The board is derived from:
- `docs/spec.md` (normative requirements, invariants, external contracts)
- repo signals (language/tooling manifests, CI config, test frameworks)
- existing test layout (`tests/`, `e2e/`, `integration/`, etc.)

### 10.1 Detection Rules (Heuristics)
Start with the **universal triad** (always enabled):
- RCT Guardian
- Integration Sheriff
- Spec Auditor

Then enable optional gates if the spec/repo indicates relevance:

**Security & Privacy Gate** if spec mentions:
- auth/authz/roles/scopes/permissions
- redaction/PII/encryption/secrets
- policy/tenancy/isolation

**Ops/Deployability Gate** if spec mentions:
- migrations/schema changes/versioning
- backup/restore/durability
- Docker/K8s/deploy/rollback
- health/readiness probes

**Performance/Scale Gate** if spec mentions:
- latency targets / throughput
- indexing/vector search/cache
- large datasets, streaming, pagination/cursors

**UX/A11y Gate** if repo contains UI automation frameworks or spec mentions:
- UI flows / user journeys / forms
- accessibility / keyboard navigation
- responsive/mobile/browser support

**Concurrency/Ordering Gate** if spec mentions:
- workers/queues/outbox/streams
- monotonic sequences, ordering guarantees
- retries, idempotency under concurrency

### 10.2 Board Output Artifact
Generate and commit a small board config (example):

`docs/review-board.yaml`
```yaml
version: 1
universal_gates:
  - RCT_GUARDIAN
  - INTEGRATION_SHERIFF
  - SPEC_AUDITOR

optional_gates:
  - SECURITY_PRIVACY
  - OPS_DEPLOY
  - PERF_SCALE
  - UX_A11Y
  - CONCURRENCY_ORDERING

enabled_gates:
  - RCT_GUARDIAN
  - INTEGRATION_SHERIFF
  - SPEC_AUDITOR
  - OPS_DEPLOY
  - CONCURRENCY_ORDERING

veto_budget:
  hard_veto_max: 3
  soft_veto_max: 0
````

---

## 11) Reviewer Output Format (Blocking Issue Template)

All reviewer agents MUST output a structured verdict.

### 11.1 Required Output Schema (Human + Machine Readable)

Each gate reviewer must output:

* `verdict`: `APPROVE` | `BLOCK`
* `gate`: gate name
* `blocking`: list of blocking issues (may be empty if APPROVE)
* `non_blocking`: list of suggestions (optional)

### 11.2 Blocking Issue Format (Strict)

A blocking issue MUST include:

* a unique `id`
* a short `claim`
* `evidence_type`: `TEST_MISSING` | `TEST_FAILING` | `SPEC_VIOLATION`
* concrete `evidence`
* a minimal actionable `fix`

Example:

```yaml
verdict: BLOCK
gate: INTEGRATION_SHERIFF
blocking:
  - id: INT-001
    claim: "Writes succeed but outbox event is not emitted transactionally"
    evidence_type: TEST_FAILING
    evidence: "tests/integration/test_outbox_tx.rs::test_write_emits_event FAILED"
    fix: "Emit system_event insert in same transaction wrapper used by write endpoint"
non_blocking:
  - id: NB-002
    note: "Consider cursor format versioning for forward compatibility"
```

### 11.3 Evidence Rules (Anti-Pedantry)

A reviewer may BLOCK only if:

* `TEST_FAILING`: cites a failing test by name/path, OR
* `TEST_MISSING`: cites a choke point and the required test name to add, OR
* `SPEC_VIOLATION`: cites a specific spec clause and deterministic repro steps

If it cannot be expressed as above, it is **non-blocking**.

### 11.4 Veto Budget Rule

Each gate may include at most `hard_veto_max` blocking issues.
All additional concerns must be non-blocking.

### 11.5 Arbiter Rule

If reviewers disagree or go out of scope:

* the arbiter can reject a blocking issue as “out of gate scope”
* the arbiter can require the reviewer to reframe as a test or spec clause
* soft vetoes cannot block merges

---

## 12) Choke Point Matrix (Integration Sheriff Input)

Integration Sheriff uses a living list of choke points.

`docs/choke-points.yaml` (example)

```yaml
- id: CP-ROUTING
  description: "Tenant/project routing consistent across read/write"
  required_tests:
    - "tests/integration/test_routing.rs::test_ns_db_space_isolation"

- id: CP-OUTBOX
  description: "Every write emits outbox event, cursor resume works"
  required_tests:
    - "tests/integration/test_outbox_tx.rs::test_write_emits_event"
    - "tests/integration/test_outbox_stream.rs::test_cursor_resume"

- id: CP-RCT-BOUNDARY
  description: "Serialization + persistence boundary round-trips"
  required_tests:
    - "tests/rct/test_roundtrip.rs::test_entity_roundtrip"
```

Sheriff may block only if a PR touches a choke point and does not update/pass the required tests.

