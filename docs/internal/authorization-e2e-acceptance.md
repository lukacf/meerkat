# Authorization E2E acceptance scenarios

These scenarios are intended to become a small number of high-density Turbo S
tests plus one macOS sandbox companion. They assert runtime effects and typed
outcomes, not model-authored explanations. Use deterministic loopback model
responses through real provider adapters, bounded waits, and observable effect
counters or files.

The full end state includes all protected operation owners and supported
surfaces. These compact scenarios are integration checkpoints, not exceptions
to coverage. They test concrete access boundaries and structural audit records;
they do not claim mechanical control of semantic information inside an LLM.

## 1. Refusal, recovery, and revocation

One admitted run requests a forbidden destructive operation, then a permitted
read. The controller returns a typed, redacted operation refusal to the model
and continues the same run. The destructive body is never entered, the read
runs exactly once, and the final answer completes. The protected audit record
associates both attempts with the same run and input; the denied attempt has no
fabricated successful entry/outcome.

In a second branch of the same suite, prepare an allowed operation and block it
at the final consequence check. Revoke its grant through the real owner, release
the barrier, and assert that the body is still not entered. A separately
authorized sibling succeeds. Reusing the captured context with changed
arguments or a retargeted resource is refused.

Harness anchors: `crates/meerkat-authorization/tests/native_governed_loop.rs`
and `crates/meerkat-authorization/src/work/tests/tool_dispatch.rs`.

## 2. Source and communication boundaries

Give subject A permission to read public source X and publish to private
destination Y. Cross the permissions deliberately: reading Y and publishing to
X must be refused before source bytes are fetched or destination state changes.
Then permit A to send to wired peer B, remove the A-B trust edge, and retry;
the receiver must not admit the second message. A still-authorized A-C message
is a positive control.

This suite is expected-red for source/publication enforcement until those
feature owners consume the shared authorization contract. The peer wire/unwire
case can run independently against the existing native control seam; do not
report it as source authorization coverage.

Harness anchors: `crates/meerkat-authorization/src/work/tests.rs` and
`tests/integration/tests/e2e_fast/multi_host_spawn.rs`.

## 3. Restart preserves evidence, not authority

Process A commits the real input/audit persistence record and exits. Process B
reloads it and verifies contributor identities, audit ordering, and history.
The runnable controller client and ingress context are absent. Governed
reconstruction without current controller custody must refuse before model or
tool entry. Include an ungoverned persistent-session control to prove the
fresh-process harness itself works.

This accepts the current refusal boundary; it does not claim successful
governed-session restoration.

Harness anchors: `crates/meerkat-runtime/src/input_audit/tests.rs`,
`crates/meerkat-runtime/src/meerkat_machine/local_authorization.rs`, and the
fresh-process fixture pattern in `tests/integration/tests/smoke_model_fallback.rs`.

## 4. Mutable executable inside an immutable sandbox

On macOS, bind a launch to an allowed mutable executable, then have a second
confined profile overwrite or atomically replace it. Prove the replacement
actually ran, while private-read, outside-write, and inherited-descriptor
canaries remain inaccessible. Include positive controls for allowed I/O, exact
environment, PID continuity, cancelled-wait followed by kill, and drop/reap.

The native shell checkpoint covers foreground calls, background jobs, and
recovered monitors. Run those real adapters as well as the sandbox crate.
Required mode must refuse an unsupported requirement before any target
effect, preserve the current host requirement across recovery, and reject a
tool/manager confinement mismatch. Required configuration must round-trip
without losing enforcement; malformed required values must never become
trusted-host execution.

Verify the explicit launch environment and resolved directory. Unconfigured
parent `HOME` must be absent, explicit variables must retain their values, and
monitor submission/checkpoint values must survive recovery. Include allowed
workspace I/O and a denied outside write followed by a permitted sibling
operation on the same shell owner.

Bind durable `ProcessCustody` and have the target read its own record as its
first effect. The recorded PID must match the target, with custody committed
before the gate opens. A successful kill dispatch must not remove the record
while a real descendant remains alive; only observed group exit permits
release. Exercise this on the production foreground owner, including timeout,
wait error, and cancellation paths. The accepted-kill custody retention
regression passes in the current shell checkpoint. Broader governed-path and
restart acceptance remain separate requirements.

Keep actual confinement cases in the macOS companion suite. Other platforms
must return the typed unsupported refusal rather than skip or launch directly.
Command-hook integration remains pending. These shell tests do not establish
all source, communication, permission, or surface coverage.

Harness anchors: `crates/meerkat-tools/src/builtin/shell/confinement_tests.rs`,
`crates/meerkat-tools/src/builtin/shell/tool.rs` (the
`accepted_kill_fence_retains_custody_until_descendant_exit_is_observed` test),
`crates/meerkat-sandbox/tests/bootstrap_integrity.rs`,
`crates/meerkat-sandbox/tests/process_confinement.rs`, and
`crates/meerkat-sandbox/tests/compiled_confinement.rs`.

## Registration and evidence rules

Register Turbo S cases in the authoritative catalog in
`tests/integration/src/e2e_lanes.rs`. Keep platform-specific coverage in an
explicit macOS lane. Missing setup is a failure, not a skip. Report source and
publication coverage as incomplete until their production adapters exist, and
keep successful governed restart out of acceptance until controller custody
has a defined persistence protocol.

Use the existing repository commands from the root for this native checkpoint:

```bash
./scripts/repo-cargo test -p meerkat-tools --lib --features integration-real-tests confinement_tests
./scripts/repo-cargo test -p meerkat-tools --lib accepted_kill_fence_retains_custody_until_descendant_exit_is_observed
./scripts/repo-cargo test -p meerkat-sandbox
```

The broader `make e2e-fast`, `make e2e-system`, and normal CI gates remain
necessary for integrated delivery. Passing the focused native tests is not
successful governed restart, full E2E registration, or CI acceptance.

After functional validation, use the repository's existing benchmark commands
in a reserved quiet window against the same candidate and an ungoverned
control. `make bench` runs workspace benchmarks; use `./scripts/repo-cargo
bench -p <owner> --bench <benchmark>` for an existing targeted benchmark.
Apply the accepted workload budgets to the default local profile. Keep raw
measurements and source identity, and report missing measurements as open;
compilation time or unit-test duration does not establish cheap authorization.
