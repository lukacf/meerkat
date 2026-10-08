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

The completed-turn SQLite case now runs in two actual child processes. Process
A commits a governed turn and exits. Process B reopens the same database and
checks the original input, audit and transcript. Its trusted host reconstructs
and pins the controller using the configured provider binding and File token
owner, issues current grants and authenticates each fresh submission.

First submit a historical association through current ingress: the current
grant owner must refuse its old lineage before model or tool entry, with no
new durable input. Then submit fresh governed work. Its denied operation must
return feedback, its permitted sibling must execute once, and the new run must
complete while preserving the exact original transcript prefix and audit.
These are completed-turn recovery assertions, not resumption of interrupted
work or persistent controller administration. Missing current controller
custody still refuses reconstruction; serialized history cannot supply it.

Harness anchors:
`crates/meerkat-authorization/tests/native_governed_loop/e1_policy_control/stock_persistent/process_reopen.rs`,
`crates/meerkat-authorization/tests/native_governed_loop/e1_policy_control/stock_persistent/revalidation.rs`,
and `crates/meerkat-runtime/src/input_audit/tests.rs`. The
[implementation checkpoint](design/adr-001-implementation/implementation-progress.md)
records executed source and CI scope separately.

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

Keep confinement cases in explicit native platform lanes. The combined source
includes a limited Linux strict-Paths backend, MCP launch/reload fixtures, and
command-hook PreTool fixtures. These authored additions still need qualification
on the supported platforms. Successful Required Linux foreground/background
shell and monitor-recovery adapter cases remain open, as do factory-owned
nonwaiting MCP startup/reload feedback and full hook-lifecycle locality.

Unsupported platforms and requirement combinations must return typed local
refusal before entry. Required descendant termination remains unsupported;
correct refusal does not establish that guarantee. Keep source preparation and
executed evidence separate in the implementation checkpoint. These shell tests
do not establish all source, communication, permission, or surface coverage.

Harness anchors: `crates/meerkat-tools/src/builtin/shell/confinement_tests.rs`,
`crates/meerkat-tools/src/builtin/shell/tool.rs` (the
`accepted_kill_fence_retains_custody_until_descendant_exit_is_observed` test),
`crates/meerkat-sandbox/tests/bootstrap_integrity.rs`,
`crates/meerkat-sandbox/tests/process_confinement.rs`, and
`crates/meerkat-sandbox/tests/compiled_confinement.rs`.

## Next-slice source inventory

The acceptance source branch starts at consent checkpoint
`0dcddd279728ef5316981d58b8f89893c82822f6`. The rows below identify source
coverage and remaining acceptance work. They are not executed RED/PASS results;
this acceptance lane has run no Rust tests, builds, lint, code generation or
benchmarks. Keep actual commands, source identities and results in the existing
[implementation checkpoint](design/adr-001-implementation/implementation-progress.md).
Earlier native-loop or optimized-binary results do not qualify this source.

| Boundary | Existing source and required oracle | Status for this slice |
| --- | --- | --- |
| R2 reviewer Deny or Escalate in an actual Agent | `factory_review_denial_and_escalation_preserve_goal_sibling_and_model_continuation` in `src/work/tests/tool_dispatch/operation_review.rs`: typed review feedback, zero reviewed body entries, one permitted R1 sibling, unchanged user goal/session, next model request and one completion | New test source only; no observed RED or PASS. The prior direct-dispatch test does not cover this Agent boundary. |
| Changed authority, stale review, missing reviewer and tool timeout | The same existing suite includes actual factory controls, a narrow tool revocation, post-Allow entry mutation, worker custody and queued-observer lock controls | Existing source only for this checkpoint; qualification must use the completed integrated head. Test work owners and in-process model clients do not establish production ingress or provider HTTP coverage. |
| Required review on model/context operations | Shared HTTP and context append reject unsupported required review; the Agent has one retained-controller feedback path | Full operation-review support remains open. HTTP error conversion is not an actual Agent continuity test; feedback requires a usable retained controller and does not prove continuity after a second refusal. |
| Human consent and reviewer context | Exact original work, requester/executor, account/mandates and action-bound human consumption must come from their existing native owners | Still open; existing MobKit pending-approval projection and memory-only review do not establish these semantics or cold consumption. |
| Restart and shipping effect leaves | Retain the separate native admission/restart and real leaf controls, including no pre-entry effect and permitted sibling continuation | Do not inherit full restart, confinement or surface coverage from the review-capable test dispatcher. Unsupported leaf feedback is safe refusal, not completed leaf support. |

The new Agent regression is written before any acceptance-lane implementation.
Its first execution must retain the actual outcome: a compile/setup failure is
not a behavioral RED, and an immediate PASS means the missing coverage was
added without proving a current production defect. Keep the existing unchanged
review and genuine cancellation controls beside it.

When the GCP lead schedules this completed slice, its focused existing-target
command is:

```bash
./scripts/repo-cargo test --locked -p meerkat-authorization --lib \
  work::tests::tool_dispatch::operation_review::factory_review_denial_and_escalation_preserve_goal_sibling_and_model_continuation \
  -- --exact --nocapture --test-threads=1
```

The neighboring controls remain in the same `operation_review` module; select
that module through the same `--lib` target for its complete critical suite.
Neither command replaces integrated native-loop, provider, restart, surface or
normal CI qualification.

## Registration and evidence rules

Register Turbo S cases in the authoritative catalog in
`tests/integration/src/e2e_lanes.rs`. Keep platform-specific coverage in explicit
native platform lanes. Missing setup is a failure, not a skip. Report source and
publication coverage as incomplete until their production adapters exist, and
keep interrupted recovery and persistent controller administration open.
The completed-turn SQLite case above does not accept either of those paths.

Use the existing repository commands from the root for this native checkpoint:

```bash
./scripts/repo-cargo test -p meerkat-tools --lib --features integration-real-tests confinement_tests
./scripts/repo-cargo test -p meerkat-tools --lib accepted_kill_fence_retains_custody_until_descendant_exit_is_observed
./scripts/repo-cargo test -p meerkat-sandbox
MEERKAT_E1_EVIDENCE_DIR=/tmp/meerkat-e1 ./scripts/repo-cargo test -p meerkat-authorization --test native_governed_loop --features integration-real-tests e1_policy_control::shell_confinement::adr_e1_required_shell_syscall_denial_preserves_native_sibling_and_model_turn -- --ignored --exact --nocapture
```

The explicit macOS E1 case runs the stock factory-built shell through native
authenticated ingress, generated grants, real loopback model HTTP, ordered
tool feedback and protected input-row audit. Both shell calls are authorized;
the forbidden outside write is blocked by the OS after shell entry. Its
nonzero exit and stderr remain ordinary shell output, not a fabricated
prelaunch authorization refusal. The permitted sibling and next model turn
complete in the same run. This case does not bind durable process custody or
accept restart and other protected surfaces.

The broader `make e2e-fast`, `make e2e-system`, and normal CI gates remain
necessary for integrated delivery. The completed-turn SQLite result does not
establish interrupted recovery, full E2E registration, or CI acceptance.

The existing `native_cost` integration target includes two workload correctness
tests, profile/deadline controls, and three ignored timing selectors. Run the
correctness tests with:

```bash
./scripts/repo-cargo test --locked -p meerkat-authorization --test native_cost -- --test-threads=1
```

The fixtures compare actual native trusted and governed admission, model
boundaries, grants, file effects and audit. They include fresh admission,
continuing work and individual fenced tool calls. Deterministic model transport
and application resource mapping remain fixtures.

Before the quiet allocation, build the optimized target in each exact source
checkout selected by the execution owner. Use Cargo's emitted executable path
for `MEAN_BINARY` and `TAIL_BINARY`, respectively; they may have different source
and binary identities. Preserve each identity with its own raw output and
ordinary command logs. A build or correctness result for one identity does not
qualify measurements from another.

```bash
./scripts/repo-cargo test --locked --release -p meerkat-authorization \
  --test native_cost --no-run --message-format=json
```

The representative mean, tool tail and model tail share one original
1,200-second allocation, in that order, including setup, warmup, samples,
correctness oracles, cleanup and analyzer output. Immediately before each invocation, the execution
owner sets its `*_REMAINING_SECONDS` to a positive integer from that original
deadline, reserving time for the remaining work and postwork. If no positive
budget remains after that reservation, do not start the selector: zero disables
GNU `timeout`. Record the incomplete allocation as UNCERTAIN. Do not reset the
deadline, reduce sample counts, or substitute the withdrawn representative `tail`
profile. The internal deadline does not preempt synchronous work; use the existing
GNU `timeout` command and externally monitored process cleanup.

Use separate fresh `MEAN_RAW`, `TAIL_RAW` and `MODEL_RAW` paths. The unchanged
six-cell mean profile uses fixed W20/N32; the tool-only selector uses fixed
W100/N2000 at grant lineage depths 1 and 3. Run the analysis commands from the
integrated checkout, whose repository analyzer supports all three schemas:

```bash
NATIVE_COST_RUN=approved-quiet-window NATIVE_COST_MEASUREMENT_PROFILE=fixed_mean_32 \
NATIVE_COST_WARMUP_PAIRS=20 NATIVE_COST_PAIRS=32 NATIVE_COST_OUTPUT="$MEAN_RAW" \
timeout --signal=KILL "${MEAN_REMAINING_SECONDS:?remaining shared allocation}s" "$MEAN_BINARY" \
  --exact representative::native_representative_matrix \
  --ignored --nocapture --test-threads=1

NATIVE_COST_RUN=approved-quiet-window NATIVE_COST_MEASUREMENT_PROFILE=tool_dispatch_tail \
NATIVE_COST_WARMUP_PAIRS=100 NATIVE_COST_PAIRS=2000 NATIVE_COST_OUTPUT="$TAIL_RAW" \
timeout --signal=KILL "${TAIL_REMAINING_SECONDS:?remaining shared allocation}s" "$TAIL_BINARY" \
  --exact representative::native_tool_dispatch_tail \
  --ignored --nocapture --test-threads=1

python3 scripts/analyze-native-cost.py "$MEAN_RAW" > "$MEAN_SUMMARY"
python3 scripts/analyze-native-cost.py "$TAIL_RAW" > "$TAIL_SUMMARY"
```

The existing matrix also has a `model_dispatch_tail` profile. Run it last with
a positive remaining budget from the same original deadline, using the emitted
optimized `MODEL_BINARY` for its exact integrated source:

```bash
NATIVE_COST_RUN=approved-quiet-window NATIVE_COST_MEASUREMENT_PROFILE=model_dispatch_tail \
NATIVE_COST_WARMUP_PAIRS=100 NATIVE_COST_PAIRS=2000 NATIVE_COST_OUTPUT="$MODEL_RAW" \
timeout --signal=KILL "${MODEL_REMAINING_SECONDS:?remaining shared allocation}s" "$MODEL_BINARY" \
  --exact native_cost_matrix --ignored --nocapture --test-threads=1

python3 scripts/analyze-native-cost.py "$MODEL_RAW" > "$MODEL_SUMMARY"
```

This profile uses only Boundaries cells at depths 1 and 3. Its schema 1 native
preparation spans include the instrumented Agent preparation through the
scripted provider's post-Entry currentness check, ending before transport and
Outcome. The older provider-only timer does not cover that preparation interval;
neither interval is real HTTP or time to a provider event.
The two request spans in each fixture are correlated. Partial output after a
graceful timeout preserves completed counts only, without quantiles or
acceptance. A hard kill may leave no output and therefore no known sample count.

The repository analyzer accepts model schema 1, representative schema 2 and
tool-tail schema 3 independently; never combine samples across outputs. The mean inference rules
remain in the [acceptance plan](design/adr-001-implementation/acceptance-plan.md).
Do not also run the unfiltered minimal matrix as part of this allocation.

The tail retains two runs per depth and measures full resolve/validate/fenced
tool dispatch, including the actual file read and audit. Its p99 is computed from
matched signed per-call differences, not by subtracting separate percentile
summaries. Calls within each retained run are correlated and accumulate audit
history. These are empirical estimates without independent tail confidence;
they exclude Agent scheduling and model preparation. Only the artificial held
model call and inactivity timers are disabled in the tail fixture; per-tool and
outer deadlines remain.

Require release-build and correctness evidence for each selected source before
measurement. The GCP lead owns the quiet exact-head allocation; machine-wide
competing build/test/benchmark load must be absent, not merely an idle named lane. The unchanged
targets are native added per-operation p99 strictly below 1 ms and
representative overhead at most 10 percent, under the acceptance plan's mean
inference rules. W20/N32 cannot establish p99. Keep each mode's raw costs:
governed-minus-trusted differences can cancel a regression common to both modes.

Before claiming reviewed-leaf performance, close the source coverage gap in
`FileTools::dispatch_resolved_with_context`: it currently bypasses
`reviewed_entry_ticket` and `enter_reviewed_effect`, and short Text prompts do
not exercise populated `CurrentTurnContent` Blocks. A bounded control using the
real leaf entry and populated content belongs in the existing fixture; it must
retain content through the leaf without adding a new measurement runner or
changing the fixed counts. Until then, those costs remain unmeasured.

Do not assume the combined mean-plus-tail work fits the allocation;
its actual complete execution must establish that. A timeout,
nonzero exit, incomplete raw data or output failure is UNCERTAIN, not acceptance;
an analyzer's empirical below-1-ms result alone is not full-profile acceptance.

These fixtures cover only a subset. They do not establish all warm/cold,
invalidation, concurrency, resource cardinality, streaming/audio or CPU,
allocation, lock and IO acceptance cells. Keep raw measurements and source
identity, and report missing measurements as open. Compilation time and
correctness-test duration do not establish cheap authorization.
