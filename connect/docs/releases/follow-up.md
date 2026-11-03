# Follow-up scope

The first release retains the complete integration-library and agent-led
acquisition goals while limiting supported deployment breadth.

## Unreleased development progress

This development branch combines the ordinary Connect improvements and the
**shared setup development candidate**. Shared execution remains unfinished and
unqualified. The release separation and 0.9.4 checks below describe the released
base, not qualification of this combined candidate.

### Connect 0.9.5 release decision (29 October 2026)

Connect 0.9.5 targets the published Core 0.9.7 (`d920733a807b020e5de3b12ffefe6a8d7d547baf`)
and Meta 0.9.6 (`923305a634ba4b420464e51fd27403091c157659`) pair. The
verified release decision
accepts the approval limitations represented by canonical 333 and 59, provided
Connect implements and qualifies the recovery behavior and documents it. The
Core consent fixes continue for the next upstream release and are no longer a
prerequisite for this Connect release. This decision changes release scope; it
does not turn the existing failed native controls into passes.

On the selected pair, a pending approval can be withdrawn after an unrelated
authorization change, a graceful stop and resume, or a restart. Recovery must
surface that state and obtain fresh approval for a new request where supported.
The withdrawn approval must stay unusable. Recovery must not automatically
approve, replay an old operation, invent retained original custody or assume
that a missing completion proves no effect occurred. The final user-facing
instructions must distinguish a fresh request from continuation of the old one.

The planned Connect workaround lengthens its finite allowance for ordinary
work and controllers, including the corresponding collector allowance paths.
Helper Operations keep their canonical setup deadline. Native grants and
restriction lifetimes still bound every operation. Source authoring or a
longer local window alone does not establish human-paced approval recovery.

The final release source must pass the changed allowance and recovery controls,
as well as the other existing release criteria. Historical 207 remains
explicitly deferred and unqualified; historical 206 remains required. Preserve
the original 333/59 results and report the separately tested workaround without
claiming durable same-original recovery. All other canonical controls, additive
controls, Python checks, installed and product scenarios, Private Services,
ordinary rkat, Linux and platform artifact requirements remain in scope.

The `9b406a9` source has reviewed SDK/resource composition and 69 Python control
passes on this public pair. The subsequent `e35a4fe` cold-restoration correction
and any allowance/recovery changes require their own actual source-bound native
qualification. Earlier native results below concern other source selections.
Final hosted artifacts, the published Python wheel and portable release-input
fragments for Forge must be bound to the actual qualified release. No current
release or completed consumer qualification is claimed by this checkpoint.

### Reviewed member-Add authority

Source corrections `fbfc6e2` and `5377ae3` bind ordinary dynamic member Add to
the installed operator, ingress authority, member identity and audience. The
installation now carries its exact authentication and event domains, including
when it starts with no members. Add uses that installed pair; request fields or
seed routes cannot supply missing authority. Shared-workflow members retain
their separate canonical parent checks.

Legacy selections may still omit both domains without a migration; existing
static-roster restoration remains unchanged. A prerelease dynamic installation
without the pair must use a fresh install or new preparation containing the
actual installed domains before Add can work under the paired Toolkit.

These changes are source-reviewed. The first native build
failed on an unqualified audience type before any selected tests ran. Correction
`15c91f8` uses the canonical authorization-contract type; the corrected `45d1f4f` build passed.
The 20 focused component controls
then recorded **19 passes and 1 failure**: a fixture tried to read a retired
admission through an accessor that correctly denies current authority. The
source-reviewed, test-only correction `fdd9b32` uses the canonical observation
and still requires authorization denial. It and the `4c93dc3` retained-helper
admission-fence regression passed in the five-control native batch on `3a97427`.
All five selected controls completed successfully; the test build
took 83 seconds using the existing build lane. The original failed control and
the later root command's incorrect source-argument preflight remain retained.
That preflight stopped before Cargo; the corrected exact-source command produced
these results. No product rebuild or shared-setup qualification is implied.

Four affected Python verifier controls passed across focused corrective runs,
with the earlier failures retained. The actual helperless authority check is
recorded below. The `5f764b4` product, GitHub read and Console results precede
these authority corrections and do not qualify them.

### Earlier development-pair checkpoint

This retained checkpoint predates the public-pair release decision above. Its
references to current scope and pending gates describe that earlier selection.

Connect adopts a new development selection based on `17641d2`, including the
reviewed consumer source corrections. Its metadata-only resolution
selected 547 packages and 229 Core dependency edges with no external version
changes. This records dependency resolution, not compilation or consumer
acceptance. The selected inputs are Core `0d20d8cb09638c72df3207ccbcefc8d57103e23f` (0.9.7) and Meta owner source
`477a9ec3453e9c5e6bf1024438b6518760a5d0ea` (still package 0.9.5).

Meta is consumed through local dependency-only projection
`eceffb061db8acaf80fd5d011a4fa77441d25264`, tree
`49f32d24731561f1cbbf7a5640c31fe875f47004`. The
projection record
limits this change to 32 Core version selectors in two manifests. It is not a
final Meta 0.9.6 release re-pin. No consumer build, SDK/resource production or
runtime result from the prior tuple qualifies these inputs.

The latest completed Connect native results remain on `aef90ee3`, Core
`f1ae8b5a` and Meta `73f322d1`:

| Scope | Retained result |
| --- | --- |
| Python controls | 69 passed |
| Native test build | Passed for the three test targets |
| Portable controls | 280 passed, 6 failed; failed IDs 20, 57, 205, 331, 332 and 333 |
| Additive controls | 10 passed, 2 failed; both cold-wake controls failed |
| Five nonhistorical ignored controls | 1 passed, 4 failed; control 12 passed; 13, 18, 58 and 59 failed |

All three failed batches completed their selected controls. Their later source
corrections do not change those outcomes or establish a pass on the new tuple.
The separate historical 206 reader passed
on aef90 using genuine old snapshots. The
historical 207 producer capture failed:
its composed mandate exceeded the 4096-byte field bound
before the required consent capture. That older-source failure supplies neither
a genuine successful 207 capture nor a current reader pass; the bound is not
waived or replaced with synthetic history.

The full scope remains 334 canonical native controls (286 portable, 41 Linux,
7 ignored), 12 separate additive controls (5 publication, 2 boxing, 3 bootstrap,
2 cold wake), 69 Python controls, five installed controls, seven product scenarios
and four ordinary Private Services controls. Current canonical-G and routable producer evidence and
real historical 207 capture/reader acceptance remain required. Normal product,
installed-host and ordinary rkat artifacts must be emitted and bound separately
where their contracts require them. Private Services protocol 3 is not qualified
by acceptance-only hosts or newer protocol probes.

Final public qualification also requires the actual final upstream release
identities, SDK and resource composition, platform artifacts and their required
consumer execution. Linux remains with its existing owner and final-registry
gate. Source reviews, local dependency projections and old passes provide no
release or shared-setup qualification. See the [build guide](../../BUILDING.md#current-development-selection)
for the selected development inputs and artifact boundaries.

### Earlier 3f0/4be integration checkpoint

The following checkpoint is retained as historical evidence. Its references to
current source, running builds and pending controls describe 28 October at
18:27 UTC, before the aef90 results above; they are not the current selection.

Checkpoint: 28 October 2026, 18:27 UTC. Connect remains unreleased. The
[native manifest](../../native-candidate/Cargo.toml) selects Core
`f1ae8b5a47ba409acd7c165a130c2cf3674b8f0f` and Meta
`73f322d1f367d744b2945bd0264012fc312b7f0e`, with development package versions
0.9.6 and 0.9.5 respectively. The
actual graph review
binds frozen Connect `3f0cab32c5c4ee63a9a755d297b229da0732e10e`, 547 packages,
38 Core packages, one Meta package and 229 same-family Core edges, including
same-root codegen.

The SDK source/import stage
and fresh library/starter/resource composition
have root and independent review. All
67 Python component controls passed.
The original native test build is still running at this checkpoint. There is
no terminal build result, and the current native controls have not run.

Meta fix `094d06a1b6d73febf8e2d4cd652cd45797983368`, carried in the selected
`73f322d1` development source, addresses the owner-confirmed dormant governed-send
regression. It wakes a local member under the lifecycle lock before reading its
session binding and preparing its controller. Hosted identities are refused
before that wake. Connect's
[`ObservedBridge`](../../crates/toolkit-mobkit/src/private_originals/governed_submit_test_bridge.rs)
now forwards `wake_for_governed_input`; omitting the forwarding leaves the
trait's default no-op and can make a later controller wake fail with
`BindingChanged` when the member moves to a successor session. The source fix
is adopted; the affected Connect controls still need actual execution.

The preceding `4beab4f` candidate used the same Core source and Meta `9bb1e539`.
Its native test build passed,
and its five publication-wait regressions passed.
Those regressions cover `eb7d977`: wait only after typed `ObservationChanged`,
reread canonical state and retain one bounded rebuild budget. They do not
permit generic retries or replay effects.

The complete portable accounting
and independent output review
record **271 passes and 15 failures across all 286 controls on `4beab4f`**.
Every attempt is terminal, with no runner or postflight errors. Failed canonical
IDs are 20, 52, 53, 55, 57, 205, 312, 313 and 327-333. The original 17 attempts
and remaining 269 used the same three binaries; the prefix was not repeated.
These were the ordinary 32 MiB controls without a worker-stack override. The
previous `f448dad` pair recorded 268 passes and 18 failures; 54, 56 and 334 passed
on `4beab4f`. Neither historical result qualifies the current source.

The current frozen candidate includes these reviewed corrections and diagnostics:

| Controls | Current disposition; runtime verification remains pending |
| --- | --- |
| 20 | Native preparation refused with `Denied`/`Unknown` before the intended injected failure. `66e0613` adds safe failure diagnostics; the exact refusing branch is unresolved. |
| 52, 57 | Stack overflow. `97fdaab` boxes the operation before constructing the review-custody future. Its two regressions and original 32 MiB controls remain unrun on this candidate. |
| 53, 55 | `af4967d` fixes status-only HTTP rejection reads and awaits actual B roster retirement before the retained-session owner probe. Original owner/consent assertions remain. |
| 205 | `613b69e` selects the actual head-canonical durable store and hosting paths used by the predecessor and restart fixture. |
| 312, 313 | The reviewed dormant-helper mismatch is addressed by the adopted Meta fix and bridge forwarding above. The old logs did not retain the exact runtime refusal variant; the fixtures still exercise a dormant helper. |
| 327 | The Core owner classified publication currentness refusal as intended fail-closed behavior. `9e00128` orders actual `FreshAcceptancePromoted` observations and both provider-response holds before review entry to preserve the fixture's stable publication window. Both real admissions and the existing post-entry assertions remain. |
| 328 | The exact native error remains unclassified. `77969c0` retains failure-stage diagnostics without changing control flow or granting review reuse. |
| 329-332 | `d04ce3a` fixes the reviewed Find account declaration and proposal bounds for the actual original member. Grant, cancellation and graph assertions remain unchanged. |
| 333 | `0c05071` waits for actual canonical helper custody to become Promoted before the unchanged five-role and exact-record readback. Only validated Pending custody waits; other failures remain errors. |

The retained failing `4beab4f` binary's static review
found a conditional nine-frame polling reservation of 22,926,704 bytes
(21.8646 MiB). This is not a captured runtime stack or proof of the overflow's
cause. Stack resolution still requires the original 32 MiB runtime controls
and comparison of newly emitted frames. The separate 256 MiB diagnostic also
failed and is not an accepted workaround.

The release plan remains Core 0.9.7 followed by Meta 0.9.6. Both release leads
received the dormant-helper report
and subsequent fix-adoption and remaining-validation updates. The reviewed
Meta fix source is verified here; final published commits, crate checksums and
SDK identities for those releases are not. Development source validation does
not require waiting for publication and does not establish release qualification.

All 334 canonical native controls remain in scope: 286 portable, 41 Linux and
7 ignored controls requiring explicit execution. Five publication controls and
two boxing controls are additional. The
installed five-case successor
has root and independent source review, but still requires separately emitted
CLI and host-example binaries and actual execution. Ordinary test binaries do
not supply them. The
current Linux source map
preserves the final-registry gate; a development source relocation contract and
current Linux execution remain pending with the existing owner. Historical
206/207 inputs, genuine shared-G evidence and full product qualification remain
required. SDK/source checks, static analysis and old test passes cannot satisfy
those remaining obligations.

### Earlier b7b/05a integration checkpoint

The following checkpoint predates the current development selection and keeps
its original source and evidence scope.

Checkpoint: 2026-10-28. Connect remains unreleased. The
[native manifest](../../native-candidate/Cargo.toml) now selects imported Core
`b7b1aab196c943823d8d7b3980a1cb708cc594b8` / Meta
`05a9582b7a0656be5899f25388a899f2559e3929` through local paths. The
reviewed update
changes 39 Core paths and the Core label; versions, features and profiles are unchanged.
The actual locked dependency graph
now confirms 547 packages, including 38 Core packages at 0.9.6, one Meta package
at 0.9.5, same-root codegen and 229 edges into Core. No duplicate Core or Meta
source is selected. The [lock](../../native-candidate/Cargo.lock) remains
byte-identical to the previous c624/e18 lock. The
[build guide](../../BUILDING.md) records matching SDK source revisions.

The production compiler check passed in 27.6 minutes
on frozen c624/e18: Toolkit native-host and Connect library/CLI console compiled,
with warnings. Its production-only scope excludes the new test controls. The
later b7b/05a adoption and
13-file warning cleanup
are reviewed source changes, not newly compiled results. The cleanup preserves
record validation, custody/Drop behavior, test availability and the existing MCP
launch profile. The 2855-file source backup
records the prior complete source before these updates.

The latest completed native build is the immutable
custody v3 snapshot,
SHA-256 `286c28b92702c1076b4cba9414bc63c8857a44a302765fdffe309752d09d3e9f`.
Its three native test binaries compiled.
Completed results keep that exact source and dependency identity:

| Validation | Result | Scope |
| --- | --- | --- |
| Portable native batch | 252 passed, 8 failed; all 260 completed | Selected native controls on macOS; Linux and ignored partitions remain separate |
| Custody controls | 12 passed, 3 failed; all 15 completed | Native review lifetime and recovery controls |
| Python component controls | 67 passed | Local-session, shared stdio and shared preparation behavior |

The portable failures cover the real-G fixture lifetime (case 20), five Find
review-entry controls (52-56), concurrent planning cleanup (57), and stock
consent resume (205). The custody failures are one canonical-parent fixture
selection and two helper cleanup stack aborts. Source corrections exist for
the fixtures and Find declaration; their new runtime results remain pending.
The three stack aborts and stock resume failure retain their original evidence.

The newer reviewed source checkpoint
backs up all 2852 files. Its 25 task-source changes relative to frozen v3 exclude
the separately preserved user-owned `sdk/typescript/NATIVE-HOST.md` file.
The task increment includes the entered witness through both graph fences and all five helper
issuers, fixture corrections, same-owner consent delivery diagnostics and
review-lifetime documentation. Two new real R2 Find controls cover all five
helper grant roles and preservation of the first controller append after an
actual report-parent revocation. The latter also requires the stored
`completion_uncertain` effect, its complete first-attempt journal and unchanged
uncertainty on original-input replay. Source review found no need to change
production error projection. These additions are uncompiled and unrun.

Two further source-reviewed R2 controls
exercise actual Find cancellation before a graph step starts and inside an
already admitted step. They require the real inner future to drop and the same
native run to become Cancelled while the step is held, then join the original
physical worker. Queued refusal must be typed Denied; an admitted late graph
row must not become successful Find. This workflow branch retains its earlier
launch/delivery effects as Pending, unlike the helper partial-effect branch
above. No uncertainty or success outcome is fabricated.

The reviewed 332-case selection
contains 284 portable, 41 Linux and 7 ignored identities. It preserves all
previous selectors, binary-target case 94 and historical cases 206/207. The
reviewed 333-case successor
adds one source-reviewed authenticated R3 control. That control uses
the actual private login, JWT resolver, Console HTTP decision and retained native
entry to issue and read back five role grants. It does not claim account/public
method execution, business effects, R3 partial refusal or R3 cancellation.
All added controls are uncompiled and unrun. The
334-case selection
preserves every prior case and adds the latest-run stock consent control,
committed at `43fc01b` after root and independent source review. Two real parked
approvals must produce separate queued notice inputs without another prompt,
model call or MCP operation, followed by three actual terminal receipts. The
same commit adds a narrow installed Restore/compose constructor and weak source
readback for the cold fixture. Those historical row observations cannot restore
admitted custody and are not an atomic multi-row snapshot. Full case 205 remains
incomplete; this additional stock control does not satisfy it.

The release-lead update at 02:41 UTC
records adoption of b7b/05a and the remaining Connect acceptance work. The
exact Core bundle and complete source export
include the stock retained-resume owner as well as the existing entered API and
stack corrections. All 56 Core manifests and 58 Python SDK files are unchanged
from ced. The independent graph review
confirms the actual complete graph changes only its Core source root.
The upstream development report
records Core b7b's full-workspace compiler check passing with no warnings, while
broader test failures remain under investigation. Its separate 29-test Meta
storage-layout pre-pass used an earlier development pair. Neither report is
current Connect runtime or final release qualification.

The consumer source review
found a case 205 composition mismatch, confirmed by the upstream owner. Three
successive stock prompts supersede earlier destinations, so two earlier
approved notices cannot both resume. A new process also does not hold stock's
prior originals. Both are intended terminal refusals, not missing release
fixes. The existing full cold positive census remains unmet; its correction
must use actual durable original custody and supported retained-resume owners,
with genuine input admissions, owner-loss proof and terminal receipts. A stock
failure-only control cannot substitute for that census.

Source review also confirms the two graph cancellation hold points match the
native exclusion boundary. The Created expectation holds inside the authorized
callback after all native rechecks. It does not cover a slow in-exclusion
source read or waiting for SQLite's write lock. Those controls remain unrun.

These source dispositions do not replace the three unchanged stack reproducers
or current native, SDK and resource acceptance. Stack sweeps require fresh
processes, with any explicit `RKAT_WORKER_STACK_BYTES` set before the internal
runtime is first acquired. The
SDK/resource preparation source
has root and independent source review and is assembled for the 334-case
selection. Final committed-source, metadata and native-source bindings remain
pending; no SDK or resource production has run from it. The native runner still
needs its reviewed path-source successor before that preparation can proceed.
Existing producer reuse remains subject to its byte/source guards, and public
release-pair gates are unchanged.

The next candidate must validate all required native partitions and preserve
installed, historical and shared-G acceptance with exact binary, SDK, resource
and runner provenance. Existing Python passes and source reviews cannot stand
in for that work. Earlier checkpoints below retain their original scope and
are not the active validation queue.

### Earlier 204-control integration checkpoint

That frozen validation candidate was Connect
`9fa19817875965c9d826eaea6ed855d3d30b5b50`, a four-file test correction to the
retained validation checkpoint
`e07d5c8e39d62e32579864415095d0181b4e4e1c`. It includes GHE checked installation,
async Describe, the typed native receipt projection and the gateway's optional
review-policy defaults. It retains the earlier
integrated recovery source union
and reviewed Recorder correction.
The authoritative [native manifest](../../native-candidate/Cargo.toml) and
[lock](../../native-candidate/Cargo.lock) select the private composed Core
`fdfb7b8905a4769934caa945a49e19ede066fe8a` and unchanged Meta
`e8105172701a89224de534d5b49a636fc1b83c3c`. The
composition record
joins Core interim `3ca3ed83` and shared-callback source `5f61833d`. The required
settlement, unknown-outcome and async redirect-lookup implementation is present
in source. Exact private bundle/cache or command-scoped Git URL mapping is
required, as described in the [build guide](../../BUILDING.md). Public GitHub
availability and qualified installed artifacts are not established.

Earlier completed checks on the exact `e07d5c8e`/Core/Meta tuple:

- The library and binary compiler check
  passed for `toolkit-mobkit` and `meerkat-connect` with Console and native-host
  features. It did not emit executable binaries or run Rust tests.
- SDK materialization and imports
  passed: 1814 Connect, 58 Core and 95 Meta source files, with 16 Python module
  imports. Resource packaging
  verified all 15593 payload entries and archive/tree equality. These are
  packaging checks, not installed or provider acceptance.
- All 67 selected Python component controls
  passed for local-session, shared stdio and shared preparation behavior. They
  do not prove native Rust behavior or full recovery.

The `e07d5c8e` test-source compiler check failed
on four Connect test compatibility errors: private receipt-field access, a
temporary mutex-guard borrow, the consent service's required `Arc`, and an
ambiguous string conversion. The reviewed four-file correction
is frozen at `9fa19817875965c9d826eaea6ed855d3d30b5b50`, with the same Core,
Meta and locks. The failed run remains retained and did not execute native tests.
Fresh SDK materialization and imports
and complete resource packaging
passed on `9fa1981`, followed by all
67 selected Python controls.
The SDK command now uses the frozen source directory; the earlier preparation
refusal and its results are preserved separately.

The three selected native test binaries compiled successfully
and their exact bytes are retained separately from the mutable build outputs.
The current native selection
retains all 204 controls. Its run stopped after 17 passes and one failure,
leaving 186 unexecuted. The shared-install fixture supplies conflicting admission
timeouts: 30 seconds for its tools and the bootstrap's 10-second default.
The owner requires equality and refuses that configuration. The reviewed
test-only correction derives both from one limits value; it remains uncompiled
and unrun. The failed run also retains a socket error during cleanup, so clean
shutdown is not established for that attempt. The earlier `reserve.prepare`
refusal in its log is intentional negative coverage, not evidence of this
bootstrap failure.

After physical cleanup review, three disjoint diagnostic segments reused the
unchanged binaries and frozen source:

| Canonical controls | Passed | Failed | Unavailable |
| --- | --- | --- | --- |
| 19-58 | 27 | 11 | 2 |
| 60-123 | 62 | 2 | 0 |
| 124-204 | 79 | 2 | 0 |

Together with the original 18 executions, these account for 201 executed
controls: 185 passed and 16 failed. Controls 37 and 38 were unavailable; control
59 remains held because it shares the failed control 58 wrapper. No ordinary
controls remain unexecuted. The original failed result is retained unchanged,
and the disjoint diagnostic results do not qualify the full selection.

The source correction batch addresses the actual selected helper in control 20,
duplicate native consent-owner installation in 52-56, the missing 32 MiB libtest
child stack in 58/59, invalid CLI flag combinations in 94, and polling the same
retained discovery worker in 123. Control 123 timed out after 300 seconds; its
process was reaped and its group was absent afterward. That establishes physical
cleanup, not successful native shutdown. Startup controls 40-42 also need the
complete Python fixture dependency closure. Their correction preserves failed
child evidence instead of deleting it; the next recipe must select the complete
test interpreter rather than assume the minimal SDK environment supplies it.

Control 57 still refuses the second concurrent Find. Source-reviewed diagnostics
now distinguish outer publication changes, inner refusal and exhausted rebuild,
without logging inside an observation or changing the two-read limit. The
release lead and Installer identify that bound and generic error mapping as
Toolkit/Connect policy; the current evidence does not establish a Core/Meta
defect. Strict concurrent successes, retained bindings and the single-G checks
remain required. Generic unavailability does not authorize replaying admission
or preparation.

The final segment exposed two additional fixture issues. Control 139 requests
ordinary persistence for a realm its fixture already governed; its correction
supplies the actual governed bundle for the same realm and agent. Control 166
collapses three cleanup results and discards the body error, so its assertion
does not identify the failing owner or establish a native join failure. Its
source-reviewed diagnostic correction preserves the first body failure, all
three cleanup attempts and their individual errors, with detailed evidence in
the retained private fixture directory. The original cause remains unresolved.
All corrections described here remain uncompiled and unrun on the successor
candidate.

The retained macOS test listing also lacks selected controls 37 and 38:
`abandoned_physical_disposition_requires_real_native_and_controller_joins` and
`factory_installation_refuses_overwrite_and_resource_close_survives_caller_loss`.
Their entire test module was Linux-only even though these cases use Unix APIs.
The source correction
enables the module on Unix and retains the existing Linux guards around the
separate `/proc` census helper and test. Test bodies, deadlines and cleanup
assertions are unchanged. This correction remains uncompiled and unrun; the
original macOS binary cannot execute those two cases, and neither counts as a
pass.

The separate two v6 plus three v7 installed cases and canonical G
capture/recovery remain pending. The actual
consumer setup
and G setup
select this exact candidate and its fresh SDK/resources. Both have root and
independent source reviews,
with no runtime result. Forge still requires the matching normal
`meerkat-connect --provider-stdio` executable and Python SDK. Compiler checks
and test executables do not supply that product artifact.

The separate source checkpoint, `7d83e0885dead800caafd9ae7c9a11d062454f45`, adds a
reviewed reader for genuine older receiptless stores.
It is not part of the `e07d5c8e` or `9fa1981` candidates and remains unexecuted. The
ignored test uses the retained old Core `ce43` Staged and Consumed snapshots,
checks every fixture byte and mode, and opens fresh independent copies only.
It requires historical run identity, R5 settlement, durable receipt rereading
and repeated settlement under the same held claim. The Staged case expects
`TailConsumed` with `CompletedWithoutResult`; receiptless Consumed expects
`OutcomeUnknown` with runtime-termination scope. Both require `NoConsentOwner`.
This separate test does not prove consent completion, canonical G recovery or
physical adoption. See the [reader source](../../crates/meerkat-connect/src/session/receiptless_predecessor_tests.rs)
and [bound fixture](../../crates/meerkat-connect/src/session/receiptless_predecessor_fixture.json).

The later reviewed source batch `cd13978`
adds two complementary controls for the next candidate. Neither changes frozen
`9fa1981`, and both remain uncompiled and unrun:

- [Historical same-G recovery](../../crates/meerkat-connect/src/application_installed/historical_g_recovery.rs)
  requires a separate private producer running the old Core `ce43` / Meta `52bd`
  tuple. That producer must create real Staged and receiptless Consumed history
  for the same G, issuer and physical owners. The current reader then requires
  actual `OutcomeUnknown`, repeated settlement and full consent census before
  physical adoption, replacement and fresh authenticated fixture work. The
  producer source
  is reviewed but has no emitted artifact or execution result. It is a new
  private acceptance harness, not an unchanged historical release binary.
  Its fresh SDK imports, library, starter and resource inputs
  are prepared and verified; that preparation does not prove old-native capture
  or current recovery.
- [Multi-row routed consent census](../../crates/meerkat-connect/src/session/standalone/stock_submission_tests/routable_consent.rs)
  creates three actual R3 approval origins, admits two approval notices before
  process loss, then routes all three withdrawals through the native delivery
  owner. The production census must refuse until every notice is terminal,
  refuse each omitted delivery receipt, repeat without new obligations and
  preserve a foreign-scope row. This control does not claim installed G or
  physical adoption.

The GHE checkpoint at `109b89e8`
passed 80 focused Python checks; its three new Rust controls remain unrun.
Those checks do not establish authenticated GHE or installed acceptance. The
async Describe correction at `917c8e0f`
awaits the native redirect lookup and rechecks the original login before
returning either success or failure. Its two new Rust tests cover six cases and
remain unrun. Earlier SDK and native component results below retain their exact
source and execution scope.

The earlier native source-history check
passed for 100 source files and 93 selected controls. The
SDK source-closure check
passed for the same 100-file closure and 11 historical R4 source pairs.
These checks did not compile, run native controls, materialize SDKs or generate
resources. The historical reviewed recipe checkpoint
selects 199 native controls and five separate installed cases at Connect
`e6eedc55`, retaining the tested process-owner wrappers. It predates the GHE and
async Describe changes; the current 204-control selection and fresh SDK/resource
records above supersede it. Those source reviews are not runtime results. The build guide's
[SDK selection](../../BUILDING.md#sdks) distinguishes the published 0.9.5 npm
dependencies from development Git source and staged artifacts.

### Recorded foundation checkpoint

The following foundation records retain their original source identities and
then-pending work, including references to files that were still private at
that checkpoint. They do not supersede the current selection and limits above.

The Connect source checkpoint `2861e45` selects Core `ce43bec80ea866c6cb414c213463461c2a1f74a6`
and Meta development source `52bdd727623e4d7a16878b67eecbe14f430116e9`.
It includes the Python and TypeScript SDK receipt changes at `b575e01` and
the reviewed native service/machine lifetime correction at `2861e45`.
The native manifest and lock select that exact Meta revision; package versions
alone do not identify this development composition. The selected Meta source
includes the operation-review hook and durable Console consent-currentness
support. It still lacks the required R6 consent-notice composition and production
receipt settlement. It is not a qualified Connect release.

The reviewed prepared and installed composition
binds an explicitly selected persistent consent issuer and operation review to
the actual application owner before Meta constructs session factories. Installed
workflow wrapping forwards the same review. The stock installed executable keeps
its default `None` selection; trusted embedding selects the issuer explicitly.
Missing or changed selected issuer history refuses instead of creating a new
issuer. The R4 source batch at `95ae8a51`
adds the authenticated Console consent event path and its final Meta,
native-login and host-policy checks. Its native controls remain unexecuted.

The reviewed `fc8d70d` consent test batch
corrects the R4 control's result reader to consume the actual typed consent-attempt
notice. It also adds an explicit installed-gateway test with two cases: the
approved parked call transfers to its exact admitted notice input and executes
in a distinct resumed run, while another parked input with identical arguments
stays unchanged. The test requires actual receipt, custody, runtime origin,
operation result and completed-answer evidence. It never supplies a receipt or
run identity itself. These assertions have passed source review, not execution.

The integrated `7667f6f` Requeue control
adds two cases through the same installed native owner and live issuer. It
requires actual Stop, exact-registration Requeue, a retained queued notice and
same-handle Resume with a new registration; stale predecessor cleanup must
leave that successor intact. Each command checks the canonical CommandDriven
posture; Resume separately requires registration absence. The observer
does not retain public native ownership; its retained slot is cleared before
normal close. Response writes have a size bound and a ten-second deadline, and
all owner joins remain required. The
root review
and independent review
cover source only. Neither case has run; this is not cold custody restoration
or an exclusion of arbitrary external identity adoption.

The native lifetime correction at `2861e45`
retains the exact supplied service and its canonical machine together. It
rechecks that machine through the same service before native census reads.
The retained close task releases the configured factory's reference cycle on
completion, refusal, unwind, or cancellation before its first poll. Losing an
observer leaves the actual close task and recovery ownership intact. Reference
release does not turn a failure into success or prove native or physical cleanup;
this record contains no hosting claim. Six new and four existing regression
controls have passed source review and remain unexecuted. Durable issuer
association and the separate cold native/physical recovery join remain in progress.

The release lead's R6 ruling for 0.9.6
requires Core to record the real consent-notice admission receipt and
Meta to withdraw consent when its admitted input ends without a run. Connect
does not settle those receipts or add another delivery owner. The lead also
accepted legacy recovered-boundary disposition for 0.9.6: any supported legacy
receipt must explicitly retain an unknown outcome, never invent successful
completion. Final R5 v2.3.2 records the accepted
`CompletionOutcome::OutcomeUnknown { run_id }`, the `outcome_unknown` event
reason and the two durable-format additions. The final contract and its
documentation-only correction have been reviewed. The actual R5 and R6
implementation pair is still pending.
The new controls still require that actual pair and execution; cold
process restart cannot restore process-local executable consent.

The SDK change at `b575e01`
recognizes `outcome_unknown` and preserves the complete native receipt. Its
runless receipt keeps `run_id` null; the historical run belongs inside the
outcome. Exact source, original, input, owner, scope and recipient checks remain
in force. The selected protocol suites passed **90 Python and 113 TypeScript
tests**, including 18 new unknown-outcome cases. Core, Meta and Connect
TypeScript SDK compilation also passed against the exact selected source.
These are pipe/stream protocol results, not native R5 receipt production or
installed acceptance. Six reviewed Rust source/test files remain private until
the actual Core variant is available; unknown outcomes must not authorize a
successful closure or another send.

Receipt and `Transferred` custody alone also do not execute the retained call.
The resumed-spend source finding
confirms that Connect has no production `dispatch_retained_consent_attempt`
caller; its three calls are component tests. The installed controls require
Core to spend the exact transferred candidate at the resumed native boundary,
without re-issuing Find or borrowing another input's approval. The
accepted R6b ruling for 0.9.6
requires that exact native handoff and a single spend, with identical-argument
twins untouched and cold restart requiring fresh consent or owner-loss refusal.
Enrollment must also handle a transfer arriving after the resumed run starts;
seeding the run only at startup is insufficient.
Design, implementation and fresh review remain pending; this source has no
R6b implementation or runtime PASS. Receipt settlement and the Requeue source
review do not establish end-to-end completion.

Connect `70ae632` adds reviewed physical lifetime and abandonment-census source.
The live shared owner can join its native attempts, Controller and child before
producing a separate physical disposition from the complete durable census.
Entered work, including reads, missing identity and unclaimed native ownership
refuse that disposition. This does not establish complete input settlement,
consent recovery or cold adoption after losing the actual join owner. Full
predecessor settlement before a fresh serving registration still requires the
Core-owned recovery contract and Connect consumer integration. Unknown effects
remain unknown; ordinary historical `consent=None` rows cannot gain consent
retroactively.

The most recent native test build, on frozen `eecf67c`, failed
with two invalid `RealmId::new` calls in test fixtures. None of its 35 selected
controls ran. The corrected constructors and an installed issuer-file permission
fixture correction are included in the reviewed current source. The failed
build's only freshly emitted Toolkit test binary and source bytes remain retained;
the separate archive-mode correction
preserves its original failure evidence.

Frozen `95ae8a51` has since passed SDK materialization
and resource composition and archive verification.
The original library-packaging failure remains recorded; the successful correction
used the complete historical library input alongside the exact selected SDK.
These are packaging results for `95ae8a51`, not native or installed acceptance
for `b575e01` or the eventual 0.9.6 pair. Current native source has not yet been
type-checked or executed. The next consolidated candidate requires the
root-owned type check and test-binary build, matching SDK/resource evidence,
and all selected component and installed acceptance checks. Validation must
include both the real-route transfer and same-live-owner Requeue controls after
the upstream prerequisites are present. Earlier passes below retain their
original source identities.

### Earlier integration checkpoints

The following records describe their named historical candidates, including
then-pending work. Use the current status above for the active source selection
and remaining validation.

Earlier development source selected Core `0e4e5d58b7ca3157205bac3152a2ed3a5fbc4ef7`
and the published Meta merge `3d6c065ce69971ef49183728acc5fad2609266ae`.
Its resolved graph
changes only Meta's exact revision from the prior lock; its full source tree is
unchanged. Connect `7c85313` preserves unavailable authority through workflow
dispatch, and `60001d5` corrects the historical stop-marker oracle while adding a
real terminal-seed recovery probe and failure cleanup. Both corrections passed
source review and remain uncompiled and unexecuted in this combined candidate.

The earlier frozen Connect `3e06cede` validation selected Core
`0e4e5d58b7ca3157205bac3152a2ed3a5fbc4ef7` and Meta `a52502e6eba12961236d99142833961f94f208e0`. Its
actual resolved graph
contains one exact Core source across 38 packages and one Meta package, with no
external version or checksum changes. Two reviewed standalone regression tests
now cover preserved recovered inputs after refused materialization, intentional
stopping after live admission, and cleanup against an exact stale registration.
The native test build passed in 3204.68 seconds.
The 18-control run stopped with 15 passes, one failure and two controls not run.
Configured drain and both late-adoption cases passed, including all nine cleanup
observations, joined providers/readers and native shutdown acknowledgement without
forced cleanup. The recovered-input materialization control failed after its
`Abandoned::Stopped` assertion passed: it incorrectly assumed the earlier
`stop_requeue` marker must be absent. Exact Core source review shows abandonment
can retain that historical marker. A test correction must preserve terminal
disposition, native recovery behavior and exact cleanup custody; it is not yet
validated. The live-admission companion and public shared case were not reached.
Product, full two-caller and recovery qualification remain pending.

The frozen `3e06cede` Python SDK has passed materialization and import checks,
including dependency consistency and the new execution-identity API shape.
Resource packaging for that frozen source also passed:
all 15,593 retained payload entries match in bytes, modes, types and link targets,
and the archive is byte-identical. Payloads keep their historical source origin;
these checks do not qualify the native runtime, installed flows or public packages.

The published Meta tag and SDK metadata were independently read back.
Meta `v0.9.5` resolves to `3d6c065ce69971ef49183728acc5fad2609266ae`, with
the same source tree as the previously selected `a52502e6`. Existing native receipts retain
their actual Git identity. Connect `10e5b80` updates TypeScript Core/Meta
dependencies to the published 0.9.5 packages and accepts Cargo's two exact Git
repository spellings while preserving repository, version and commit checks.
Validation passed:
TypeScript compilation and 900 tests, plus 361 release-producer tests. One
TypeScript native-fixture case and one Linux case-sensitive-filesystem case
were skipped; these results do not establish either skipped qualification.
Connect's own version and public download instructions remain at their existing
release until the 0.9.5 release inputs and acceptance are complete.

The portable handoff audit
found that publication retains absolute producer paths inside the selection,
although its separate path map and public file inventory are portable. Final
Forge fragments need a reviewed portable projection and actual target/PyPI
pins, preserving the original attested bytes. Both release owners received this
finding and the native terminal result.

The earlier owner-composition audit
identified the missing member-factory review hook and consent owner-loss
settlement installation in released Meta 0.9.5. The current development source
above now includes the follow-up hooks and Connect's persistent issuer wiring;
full authenticated consent and pending/unknown-effect recovery remain unfinished.

The earlier isolated `3d6299ff` / Core `dd0ada78` / Meta `bedbdabc` candidate
passed all 13 selected native controls,
including the entered-turn drain regression, and its
product build and staging passed.
Its subsequent full two-caller run failed after 196.59 seconds.
A actually received `sum=5` from `selected_add`. B entered its native workflow
and received a successful selected-install result, then `setup_reconcile`
returned an access denial. The source trace narrows this to Connect's shared
workflow lookup returning `Unavailable`, which the approval adapter reports as
a generic policy denial. The mutex failure variant is not recorded; no Core or
Meta defect is established. All nine cleanup observations passed, the provider
joined, and native shutdown acknowledged exit zero without forced cleanup.
Full two-caller acceptance remains failed and recovery has not started.

The reviewed Connect correction
now waits for the existing retained-source mutex instead of refusing ordinary
contention, and preserves shared-reserve unavailability as a distinct error.
Poisoned state still refuses; all source, input and authorization checks remain
enforced.
Two native fixture controls cover contention and poison; the existing ignored
public-resource case also checks the real unadopted-source error mapping.
The contention and poison controls passed in the current native run. Its
public-resource case was not reached. Further canonical-read and workflow-wrapper
availability losses have a separate reviewed correction awaiting native validation;
they are not attributed to the historical installed failure's exact cause.

Both release owners received the terminal failure and qualification limits.
The [shutdown acceptance record](#graceful-shutdown-acceptance-before-enabling-a-drain-window)
retains the earlier native failures, exact passing evidence and SIGTERM limits.
These results do not qualify the new Core/Meta composition or a Connect release.

Earlier development source selected Core `dd0ada78` and Meta `6b79555bf1d064cace586e554c354e6c566b5482`
from the verified development source handoff.
This unpublished Meta composition includes durable-tail forwarding in both
session wrappers. Its Cargo manifests, lockfile and SDK sources are unchanged
from `a8840da7`; Connect's native lock changes only the Meta source identity.
For Connect `6ff5a88`, locked online and offline metadata passed
with the same 547-package graph, and exact SDK materialization and import checks passed.
The reviewed upstream receipt
reports Meta `6b79555b` passed its all-feature library/test type-check and
decorator verifier with Core `dd0ada78`; only the receipt and short compiler
tail are retained, not the full raw compiler log. Connect `6ff5a88` then
built the native test binaries in 2761.59 seconds and passed 43 of 44 controls.
The fresh-host Peer restoration control refused its static baseline member.
Source review identified a sibling-registration refresh that can temporarily
invalidate that shared policy observation. The Connect correction preserves
the accepted policy observation across registration-only metadata changes and
withholds only the changed member until its reservation and native witness
pass. Correction `6b184b6` passed all 51 selected native controls,
covering the preceding 44 controls and seven focused registration checks.
The product build passed,
but the both-caller installed case failed
on A's first `setup_state` call with `tool_unavailable`, after successful catalog
loading and before any native Prepared or Entry audit row for that call.
The retained evidence does not distinguish inner MCP plan resolution from
Connect workflow staging. Diagnostic source `265558d` passed its two affected
native controls and product build, but its installed case failed
at A's first `setup_state` with `access_denied`. This call reached native
Prepared and Entry; the fixed labels localize refusal to service-task
preparation. Source review found that the fixture submitted its LocalConsole
children through the application-event API, which does not retain a verified
Console login on the source. The subsequent Connect correction selects the child
through the existing private workflow owner, then prepares it during the actual
authenticated Console bind before first admission. It keeps the exact native
input and source-held login; correlation and historical evidence supply no new
authority. The two-caller fixture now uses this path and retains both protected
reviews and completion checks. That correction, changed SDK and focused controls
were unvalidated at this checkpoint. Both failed runs acknowledged native shutdown with exit
zero; readers, provider and descendants joined without forced cleanup. Full
installed flow, shared recovery and release qualification remain pending.
Older SDK receipts retain their original source identities.

Candidate `b97ac8d` retains Core `dd0ada78` and Meta `6b79555b`.
The installed SDK materialization
retains its actual `1a1b6f3` source identity, and the refreshed Node resources
retain their `5c54940` origin. The Connect correction waits for the existing
source writer during ordinary asynchronous follow-up, then rechecks cancellation
and the admitted parent before reserving an entry. It passed all eight native controls,
including queued cancellation and dropped-waiter cases. The gateway and CLI build passed.
The installed case then failed at A's service-access request,
after its selected check, exact protected software review, installation job and
retained installed readback succeeded. `a-selected-access` returned
`operation_refused` with `kind=denied` and `tool_invoked=false`; the precise
authorization refusal is under review. All cleanup checks passed, the provider
joined, and native shutdown acknowledged exit zero without forced cleanup.
This proves A's software installation, not its service grant or either caller's
completed business operation. Full installed flow and shared recovery remain
unqualified.

Candidate `45f8aa1` keeps those same Core and Meta selections.
It projects the selected workflow caller's origin for public-service requests,
preserving the registered target, exact call binding and delegated bounds.
The three focused native controls passed,
including the real installed policy projection and ordinary helper controls.
These controls call the operation policy owner directly; they do not prove
admission through the native grant policy. The gateway and CLI build passed,
but the installed case still refused A's service request
after successful software approval, installation and readback. Its retained G
authority basis selects the setup MCP target but has no public-service registered
target lineage. Native grant resolution requires that exact target before it
calls the operation policy owner, so the projection fix alone cannot admit the
request. Both callers' service use and shared recovery remain unqualified. All
cleanup checks passed, with joined provider and acknowledged native shutdown
at exit zero.

Runtime correction `01f7ff5` adds one deterministic public-request child
under G's already-selected root, with only the selected public action and the
same domain, processor, audience and lifetime bounds. It creates no new root.
Saved bindings retain their exact target references, including legacy bindings
that omitted the public target; replay does not issue a missing grant. Public
revocation remains local to public use, while fresh admission still checks all
selected targets. The correction has completed root and independent source review.
The test build passed in 1722.15 seconds,
and all five selected native controls passed.
They cover real native grant-policy target lookup, revocation, immutable replay,
MCP continuity and source isolation; they do not prove installed completion.
The gateway and CLI product build passed in 2247.61 seconds,
with both emitted binaries, packaged resources and all 2697 tracked source files
verified. The full installed two-caller case failed
at A's public-service request with `access_denied`. Its runtime audit
shows successful native preparation, then entry into tool dispatch and the
tool-execution policy before denial. That run did not identify the inner policy
branch. No public-service request was admitted; all nine cleanup checks
passed with acknowledged native shutdown at exit zero and a joined provider.
The SDK and resource bundles retain their actual `1a1b6f3` and `5c54940` source
identities. Full installed completion and shared recovery remain unqualified.

Diagnostic candidate `c72bb0a` adds failure-only static labels without changing
the policy checks. Its product build passed in 318.90 seconds,
with both binaries and all 2697 tracked source files verified. The unchanged
full two-caller reproduction failed in 149.46 seconds.
The diagnostic observation
locates the refusal at `service.prepare.reserve`, propagated through
`application.owned.prepared` and `public.application`. All nine cleanup checks
passed with normal shutdown and a joined provider. The retained canonical slot
contains four static members and two admitted class members. The selected
service policy required exactly the four static names, rejecting the complete
successor that preserves all six. The five native controls remain evidence for `01f7ff5`;
they do not qualify this diagnostic source or prove installed completion.

Correction `1c9c384` has completed source review.
It keeps the service recipient static and accounts for preserved class members
using their exact admission records from the same prepared canonical snapshot.
Every static member remains required; extra rows must match unique selected
classes and their complete materialized grants. Native publication still checks
the prepared record's revision and digest. Historical retirement evidence can
explain an old approval during explicit re-admission without restoring a removed
member's authority. No serialized policy or SDK contract changes.
The native build failed after 994.13 seconds
with `E0616`: the parent policy owner could not borrow the private `admission`
field. No controls ran. Correction `b10d911` makes only that field visible to
its parent module; the stored representation and policy checks are unchanged.
The retry built both test libraries in 2543.02 seconds.
Four of five selected controls passed.
The remaining Connect control stopped before its policy assertions: its fixture
passed unsorted member rows to the canonical policy constructor. Correction
`bcd3356` sorts the proposal's local row copy, preserving the input vectors and
their negative mutations.

Independent source review also found that retiring an unrelated class could
overwrite the sole latest retirement witness and make a valid re-admission's
durable canonical slot unreadable. Correction `a4c349e` has
completed source review.
It validates historical narrowing against exact class rows and canonical
admission state, including an exact pending reinstatement. It adds 12 recovery
cases covering three retirement orders, native acceptance before recovery, and
revocation. Candidate `bcd3356`, which contains this correction and the fixture
fix, built both test libraries in 1667.11 seconds
and passed all five selected controls.
The selected Toolkit regression executes the 12-case matrix and tampered
pending-state negatives. These are component results; they do not establish
installed completion or live shared recovery.

The prepared `b10d911` installed and recovery recipes are held. The
reviewed `bcd3356` product and two-caller successor
binds the actual five-control pass and has passed its SDK and resource
preflight. Its gateway and CLI build passed in 236.32 seconds,
with both emitted binaries, staging and all 2697 source files verified.
The unchanged full two-caller case failed in 152.76 seconds
at A's catalog-policy reservation, before B was launched. Source review found
that the fixture compared the service proposal against the static bootstrap
policy at revision 1, although the two completed class admissions had advanced
the canonical policy to revision 3 with six members. The failed proposal was
not retained before refusal, so its exact prior is inferred from the native
proposal-construction path. All nine cleanup checks passed, the provider joined,
and native shutdown acknowledged exit zero without forced cleanup.

Verifier correction `9186d72` captures independently observed current policy
bytes before approval, preserves the complete successor comparison and retains
refused proposals. Its three focused controls passed across two runs:
two initial passes and the corrected third pass, with the original path-fixture
error retained. The reviewed installed runner
uses an exact committed verifier export with the existing `bcd3356` binaries,
native controls, SDK and resources. It performs no product rebuild.

The full two-caller run failed in 341.22 seconds.
A's protected software review and native public-service grant succeeded; the
complete policy was retained at revision 4, and A observed `selected_add`
returning `{"sum":5}`. B then launched, but its first `find_and_install` call
returned `tool_unavailable`, before its software review or service grant.
A's final response was held pending B, so this is not proof of finalized caller
completion. All nine cleanup checks passed, the provider joined, and native
shutdown acknowledged exit zero without forced cleanup. The subsequent
canonical-state diagnosis
shows that terminal setup recovery departed A and sealed the shared cohort
590 ms before B's native preparation, while A's service response was still held.
The deadline was still in the future and waiter capacity remained available.
An earlier component-entry contention log does not establish the cause of B's
refusal.

Source correction `6886d39` retires terminal setup-call authority immediately
but retains an approved service's cohort membership until its exact native
input is finalized. Explicit cancellation can still depart independently.
The [host composition guide](../site/guides/host-composition.mdx) describes
this lifetime boundary. The first native test build failed
with `E0624`: the new regression called an existing test-only helper from a
sibling module. Correction `f85142c` exposes that helper only within
`private_originals` under `cfg(test)`; it changes no production behavior or
test assertions. Its native build passed in 1522.44 seconds,
and all five selected controls passed.
The gateway and CLI product build passed in 110.01 seconds,
with emitted binaries, staging and all 2698 tracked source files verified.

The full two-caller case failed in 193.47 seconds.
A's service grant and `selected_add` result succeeded. B then completed
`find_and_install` and its independent software review, but its public-service
request returned `access_denied`. A's final response remained held; all nine
cleanup checks passed and native shutdown acknowledged exit zero without force.
The source diagnosis
found that the verifier retained A's completed service-administration exchange
when launching B. The SDK requires explicit retirement before accepting the
next exchange. This conclusion follows the exact source and observed sequence;
the failed run did not log the inner refusal branch directly.

Verifier correction `3fd5179` retires the same successful joined exchange before
progressing, requires the identical terminal observation and released
reservation, and preserves the first retirement result separately from final
cleanup. Six focused offline controls passed,
including failure cases that prevent B from launching. These controls establish
verifier sequencing, not installed completion. The successor installed run failed
in 242.70 seconds
after both callers received grants and retired their exchanges successfully.
Its audit assertion rejected a repeated preparation observation for the same
operation before accepting either final continuation. All nine cleanup checks
passed; native shutdown acknowledged exit zero without force.

Verifier correction `19334bb` accepts canonical preparation refreshes while
requiring the same operation, run, input contributors, target, authority and
publication instance. Publication sequence may advance; the observed wall clock
need not increase. Every raw observation remains available, and malformed
counters, binding changes, missing successful dispatch or any refusal still fail.
Twenty affected offline tests and both retained native audit replays passed.
The replays preserve all seven observations for each caller and do not change
the failed installed result.

The fresh installed case failed in 224.18 seconds
before reaching the revised audit. B's Console request received HTTP acceptance,
then failed delivery while binding its authenticated source. Its setup child and
both target grants were issued, but its controller grant, final source association
and native input were absent. Retained evidence narrows the refusal window
without identifying the inner error. The adjacent plan-selection refusal belongs
to an intentional negative control. All nine cleanup checks passed, with joined
provider and acknowledged native shutdown at exit zero.

Source correction `a601f06` adds fixed-stage, finite-error diagnostics to that
binding window, including the distinction between changed and unavailable
publication observations before the existing error mapping. It preserves checks,
returned errors, issuance order and custody. Root and independent source reviews
passed.
All six existing behavioral controls passed in the 13-control native batch,
whose combined test build passed in 2126.03 seconds.
That batch passed 12 controls and failed the nonzero drain control described
below. These diagnostics improve failure evidence and do not establish a
behavioral fix or installed completion.

Full two-caller completion remains required before the
three recovery cases:
shared G respawn, cold shared restore, and cold restore with withheld P completion.
Component tests alone cannot qualify the installed path; full completion and
shared recovery remain unqualified.

The preceding completed native run used frozen candidate `068f515` with Core
`dd0ada78` and Meta `a8840da7`. Its test build passed in 219.47 seconds,
and all 44 selected native controls passed.
The gateway and CLI then built successfully,
but the installed case failed
in its ordinary member respawn prelude, before either shared caller reached
setup. That Meta revision wraps the persistent session service without forwarding
`MobSessionService::provisional_tail_stands`; Core refuses the resume because
the wrapper cannot establish whether a provisional tail stands. Both Meta's
pre-build and after-create wrappers omitted forwarding to their inner service.
The new source selection addresses that omission without a Connect fallback;
the failed installed result remains unchanged. Native shutdown acknowledged
completion; readers, provider
and descendants joined without forced cleanup. Shared setup and installed host
application acceptance remain unqualified.
Locked online and offline metadata passed,
but the preceding `b9c9234` compile check failed
on one test-only `SessionCreatedContext` type path, corrected in `d8c5da9`.
The earlier `d8c5da9` test build passed,
and all 29 selected controls ran: 23 passed and 6 failed.
After correcting fixture topology, `87568b0` compiled
and passed 27 of 29 controls.
The remaining failures exposed shutdown settlement through a closed admission
path and a cold test selecting an old receipt. Corrections `355468c` and
`1288df8` compiled successfully
and passed 35 of 36 native controls,
including both previously failing peer-delivery and cold-restore cases. The
new close regression failed during native shutdown with
`Mob(ActorCommandChannelClosed)`. Its fixture calls full shutdown twice;
reviewed test-only correction `068f515` joins both owners once, then checks the
same settled state. The corrected close regression passed in the `068f515`
batch, along with the collector, workflow-close and completed-receipt controls.
The earlier `1288df8` aggregate remains failed.
These component results do not qualify the
installed shared setup case: it must still return answers to both separately
authorized callers and settle shared work with joined cleanup.

Further source review found that the second caller's `setup_state` would fail
because the shared helper retained the first caller's software review.
Correction `6de10ab` permits an independently authorized new input to read setup
state without exposing or reusing the prior input's review. Same-input binding
mismatches still refuse, and retired setup owners refuse reads even when no
review is present. The existing installed case now checks each caller's own
review against its protected approval, with distinct approvals for the same
physical job. Its four affected offline verifier controls passed.
All eight native review-observation controls passed in the `068f515` batch.
The corrected installed case remains pending; the earlier `1288df8` run does not
include this correction.

The existing completion-recovery control was extended after its earlier pass to
cover a dropped metadata observation and a refused SQLite completion write.
That extended native control passed on `068f515`,
using the already emitted test binary without another build. It proves the
component retains rows and events on failure and recovers from its existing
resource; it does not establish installed cold recovery or installer replay.

The separate Linux candidate `c71c9a2` uses Core `0bf554a7` and Meta
`7df2e67f`. Its downloaded receipt
records a rebuilt gateway and verified development package. The
downloaded native and SDK log review
confirms 24 of 25 native controls passed, together with 112 Python and 48
TypeScript checks. The only native failure was the same double-close fixture
failure described above. The verified corrected-close addendum
records that one control passing on `c71c9a2` plus the reviewed single-close
patch, with the same Core/Meta pair. The prior 24 passes and corrected one cover
the 25 selections across retained runs; the other 24 were not rerun, and the original
aggregate remains failed. This addendum does not rebuild or qualify a product,
the Mac candidate, an installed host application journey, or a release.

The Linux Recorder control passed both normally and with the Core run guard
removed. It therefore remains a composed refusal check, not isolated coverage of
that guard. The separate Core guard evidence below retains its own source and
execution limits.

Earlier, Connect `a83868c` selected Core `85f7749` and Meta `f780dfa5`.
Locked online and offline metadata passed,
but its native test build failed
with `E0433`: a continuity test used `futures::FutureExt` although Connect already
depends on `futures-util`. None of the fourteen controls ran. The reviewed
one-line `futures_util::FutureExt` correction is included in `b9c9234` without a
dependency change. No product or installed result followed that failed build.

Earlier, frozen Connect `01a58c1`, with Core `0768728d` and Meta `f780dfa5`,
compiled successfully.
Its six selected native controls recorded **4 passes and 2 collector fixture failures**.
The aggregate remains failed. Source-reviewed correction `163fd93` preserves a
real empty-catalog connection through host join and selects the gateway's actual
head-canonical audit store, with child shutdown on assertion failure. Those two
collector controls passed in the `d8c5da9` batch above. The earlier failures
remain preserved; that `d8c5da9` batch remains failed because of the Peer rows.

The same `01a58c1` candidate also built its gateway and CLI successfully,
but its installed two-caller case failed.
The first caller's `setup_state` was denied before any completion callback.
Diagnostics located the refusal in service preparation: it looked up the late
workflow helper in the static installation roster. Source-reviewed correction
`ffe82497` selects the current workflow helper through its existing custody
check. The installed case remains the required regression and has not been
rerun with that correction.

Shutdown joined all six members, readers, provider and descendants without
forced cleanup, but could not establish shared association settlement. A
reviewed consumer is committed in `a83868c`. It uses Core's body-free receipt
observation while preserving the full census, identity and closure checks,
including a real finalized `CallbackPending` control that must remain unresolved.
Core `85f7749` has passed both upstream source reviews. Installed shared close
remains unproven; physical cleanup does not establish completed work or a
successful shared close.

Source correction `8f49f0e` adds the explicitly observed remote MCP requirement
contract while preserving the strict in-process owner contract. Its
changed Python module passed all 60 tests.
Review then found a gap those Python-only tests could not detect: native
acknowledgement of an absent observation made Python shutdown report a callback
reconciliation error. Correction `e8ed563`, accepted by the Core source reviewer,
acknowledges that
absence while still refusing the setup waiter. One new regression crosses the
actual Python/native pipe and requires clean callback shutdown. It and the seven
selected L88 native controls passed on `87568b0`, as controls 03-10.
Those eight controls also passed on `1288df8`. These focused results do not qualify
remote Add, caller-owned final publication, installed Forge setup or installed
shared execution. Earlier results below retain their original source and
acceptance limits.

Earlier, frozen Connect `9075408`, with Core `0a3cc302` and Meta `2c80ede4`,
compiled successfully.
Its 23 native controls completed with **22 passes and 1 failure**.
All six collector controls, five non-cold native-operation controls, six
targeted-workflow/observation controls and five parent/restoration controls
passed. The sole failure was
`native_target_preparation_restores_exact_grants_before_first_input`: retained
class-member restoration refused before the post-Restore inputs could run.
The aggregate result remains failed. Source-reviewed fixture correction
`edbc891` composes the existing native delivery and mob-tool owners before
restoration. The affected cold controls 07/08 then
both passed on frozen `3f930d4`.
These are component results, not installed shared-workflow recovery acceptance.

The warm two-caller installed continuation
is a separate validation partition on the same frozen `9075408`. Its
gateway and CLI product build passed,
but the installed case failed
at `a-selected-state`: local policy refused `setup_state`. Gateway shutdown also
reported a shared-resource settlement failure. The retained physical close receipt
shows that the resource closed; readers, provider and descendants joined without
force, but successful association settlement is not established. Policy
correction `910b086` is independently source-reviewed and committed. Its first
regression run recorded 7 passes and 1 fixture failure:
the test unwrapped optional member metadata that its waiter deliberately omits.
Test-only correction `0bed658` uses the retained parent identity as its distinct
comparison value. The ten-control rerun on frozen `85f3d71` passed,
including the corrected helper regression, close/association controls and the
selected-package cancellation coverage from `423a571`. Those cancellation
fixtures exercise approval custody and uncertain outcomes, not a physical
installer process. The gateway and CLI product build on `85f3d71` passed,
but its installed two-caller rerun failed
at `a-selected-state` with `access_denied` for `setup_state`. No original-completion
callback was recorded (`callbacks: []`). Native shutdown was refused, although
observed processes, readers and provider joined without force. The precise
refusal cause remains under diagnosis; installed shared execution remains
unqualified. Diagnostic-only `e05d463` adds fixed failure substages and does
not repair or override the refusal.
The recipe preserves the failed native aggregate and cold-control result;
this case does not qualify cold native-target recovery.
The later metadata change `eb16469` and fixture correction `edbc891` are outside
the earlier frozen `9075408` candidate.

The earlier gateway and CLI product build passed on `d7d7768`.
Its two-caller installed case failed
at `a-selected-state` with `operation_authorization_unavailable`, after G had
progressed beyond the earlier catalog refusal. Source review found that G's owner-domain
base grant did not record the explicit MCP connector constraint required by native
policy. Correction `79eb3c0` adds the separate exact MCP target lineage while
preserving the base grant and P's contributor ceiling. Its affected component
controls passed in the `9075408` batch above; installed shared execution and
successful shared cleanup remain unqualified.

The earlier helperless Add/use/cold Restore/use case passed on `45d1f4f`:
the installation started with zero members, held Add used its installed authority,
and a fresh process restored the same canonical admission and session. A fresh
tool callback and second native input completed. Both hosts and runtimes completed
cleanup; readers and descendants joined without force.

Its earlier path-spelling preflight failure
remains retained. The separate `45d1f4f` shared failure
at G's catalog search led to the implemented pre-admission parent preparation:
P now retains its independently selected operation ceiling before its first
input. A legacy controller-only parent is not upgraded during Restore.
The earlier `5f764b4` authenticated GitHub and Console passes retain their
separate limits below. [BUILDING.md](../../BUILDING.md) records the dependency
selection. These are development checks, not release qualification.

Producer correction `786812d` validates SDK and native versions against the
exact selected Core/Meta 0.9.4 or 0.9.5 pair; 72 focused controls passed.
Connect Rust and Python development metadata now selects 0.9.5; Python's
native-host extra requires `meerkat-meta==0.9.5`. TypeScript metadata and its npm
lock remain 0.9.4 pending genuine published dependency artifacts. Distribution
adoption still requires the final upstream artifacts, a regenerated npm lock
with actual integrity hashes, and registry installation verification. Metadata
alignment and producer checks do not qualify a release or installed behavior.

The registry check at 21:09 UTC on 25 October 2026
found Core `meerkat-sdk==0.9.5` on PyPI and `@rkat/sdk@0.9.5` on npm; both Meta
0.9.5 SDK endpoints still returned HTTP 404. The captured Core artifacts
match their registry hashes, including npm's SHA512 integrity. Python wheel and
source-archive package code agrees for all 20 files. Against the frozen Core
`dd0ada78` SDK, 16 package files are identical; the other four add release binary
hashes and the optional member-live `execution_identity` argument through the
client, generated model and mob wrapper. This source review
does not establish runtime compatibility. No published package was installed,
and the active SDK, native pair and TypeScript lock were not replaced.

### Native collaboration and Recorder gap

Committed source `fa889869` adds explicit pre-admission selection and installed
operation mapping for peer Message/Request, peer-directory reads and the caller's
own-identity Recorder Read/Write/Delete. The same private client exposes it after
fresh startup or Restore, including selection of actual Active members after
Add. See [host composition](../site/guides/host-composition.mdx#select-native-peer-and-recorder-operations-unreleased)
for the preparation sequence. Mob and Realm memory scopes remain excluded.

The five non-cold native-operation controls passed in the
`9075408` batch,
including exact selection/replay, sender policy, foreign-target refusal,
revocation/cancellation fences and issuance custody. Their sender assertions
distinguish native policy entry from delivery. The retained-selection cold
Restore control initially failed; both affected cold controls passed after the
fixture correction described above. Existing product results do not qualify
installed cold recovery or recipient behavior. The earlier
refusal on the host application at `853410a` remains valid historical evidence.

Outbound permission alone cannot establish delivery or a receiver's reply.
The separate host application receiver is reviewed and committed in `4bd9ab3`, with lifecycle
controls in `0763e34` and the existing-owner helper/L88 port in `cd4783a`. It selects
Core `0bf554a786` and Meta `7df2e67f`; Add retains standing authority for peer
replies and the receiver's own Recorder, without borrowing sender authority.
Core's admitted-original proof also covers governed Peer inputs after payload
retirement, preserving Prompt behavior.

The Core lead reports runtime and authority source GO and an earlier **12/12
GREEN** result. These are upstream-reported results. The host application's `cd4783a`
native library and test compilation failed:
receiver code used private `Input::header_mut`, and tests also used a private type
path and equality assertions requiring unavailable `Debug` implementations.
No native controls or product acceptance ran. Reviewed correction `043dd3d`
uses the public Input APIs and correct test types; its
source handoff reached the existing Linux owner.

The earlier Linux validation on `fc3a1f5`
built and verified the development gateway. Python passed 112 tests plus 12
subtests; TypeScript passed its 42-test and six-test groups. Native controls
recorded **32 passes and 6 failures out of 38**. All six receiver controls
stopped at their first Peer send: five reported `PeerNotFound`, and the
revocation check failed its expected-refusal assertion. Source review traced
the common setup failure to missing fixture topology. Receiver admission and
the later lifetime assertions were not reached. The Recorder fence mutation
was not attempted because its baseline failed before reaching the effect.

The fixture correction on the host application (`ba18cc3`) and main (`5f5dc51`) selects the exact
pair through the native wiring owner and requires cold restoration to preserve
the edge without rewiring. The Linux receipt and actual logs for `ba18cc3`
confirm four passes and the same two deeper failures as Mac `87568b0` above.
Its Recorder mutation survived: the test still passed after Core's run-custody
fence was removed. It proves refusal of a late Recorder effect through the
composed path, but does not isolate that fence. Source review identifies a
second possible refusal during re-preparation when the completed input's live
bindings have been archived; the mutation did not instrument that branch.
Isolated run-fence coverage belongs to Core's
`an_unused_peer_send_ticket_refuses_after_its_run_is_consumed` component test.
Its retained baseline passed,
and skipping the run-custody check made that test fail at its first ticket entry
with `Ok(ReviewedEntryCustody(Inert))`. These logs identify Core `aae0a37b91c3`
plus the supplied test source. The Core lead reports that the reviewed test was
replayed onto its unpublished release chain as `9705f777de`; it is outside the
current `0bf554a786` host application tuple. This cross-reference does not qualify that tuple
or an installed collaboration flow.

The configured-provider regression exercises two actual Added project members,
their standing grants, successful authorized Add and explicit post-settlement
topology refresh. Lead-member topology is a separate host application acceptance item:
when a host application build is installed, use its real lead-member seed and an actual project
Add, require native success and authorization, reconcile, then verify the exact
lead-member to project edge and absence of undeclared edges. This check proves
topology, not the lead member's peer-tool authority. Renaming the generic fixture's members
would not establish that coverage.

### Collector enrollment

Committed, source-reviewed `2b7e61d` publishes the existing approved collector
record to native service-original enrollment and limits installed operation
policy to its exact read-only MCP selection. It preserves uncertain cancellation
as unavailable and prevents collector IDs from masking ordinary cancellation.
All six collector policy/enrollment/grant/cancellation controls passed in the
`9075408` batch.
They do not qualify service-call dispatch or a live provider read.
[Host composition](../site/guides/host-composition.mdx#enroll-a-read-only-collector-unreleased)
describes the existing Rust entry points.

Source-reviewed `cf31faa` adds the finite Rust `CollectorOccurrence`
open/read/close API, documented by guide commit `2488348`. It preserves the
native `ServiceCallOutcome` result and audit introduced in Core `0768728`;
entry without an outcome remains unknown. This API compiled with Core `0768728d`
and Meta `f780dfa5` in the `01a58c1` batch above. That checkpoint selected Core
`dd0ada78` and Meta `6b79555b`, pending validation at the time. The two corrected public occurrence/audit
component controls passed in the `d8c5da9` and `87568b0` runs above.
It does not implement a collector runnable or an SDK surface.
These component results do not establish governed announcement delivery
(`S1-gov`), the late-owner Rust read seam or a completed scheduled Gmail flow.
Enrollment is not a delivery result or release qualification.

<details>
<summary>Completed checks, failures and historical source checkpoints</summary>

Every result below belongs to its named source and artifact. Historical
"pending" and "unrun" statements describe the checkpoint when they were written,
not the current validation queue. Later passes do not rewrite earlier failures
or qualify later source changes.

#### Completed product and component results

- **An actual authenticated GitHub read passed:** the packaged official connector run called `get_me` and the original native session continued with: "GitHub account lukacf (ID 16124748)." The model and issuer were controlled test components; the provider read was real. The outer result records successful test completion and removal of the isolated credential. The verifier also required exact test-grant revocation, unavailable readback and joined native cleanup. The existing GitHub credential store was unchanged. This does not repair or establish revocation of the separate grant from the earlier failed attempt below.
- **The corrected product built; three installed checks still failed:** the gateway and CLI build passed, then three installed checks failed. Console reopening reached a real structured tool result, but the verifier expected text. Helperless Restore reached a new tool-dispatch customization but waited for an `after_create` callback that cold recovery does not emit. Shared setup reached a later search that native authorization refused. Console and helperless cleanup acknowledged shutdown; shared cleanup refused shutdown and exited 1, although its processes and readers joined without forced cleanup. These are separate unresolved outcomes, not successful shared recovery.
- **The Console result oracle is corrected:** source `12175ec` requires the exact structured native result and schema, rejects malformed or mixed results, and passed 15 offline controls. The installed rerun passed: both sessions restored, panel reads caused no agent work, and a fresh ordinary Find reached an actual restored-helper schema result. Both native hosts acknowledged shutdown and all readers and descendants joined without force. This proves the stated reopen and tool-call path, not installation, service readiness, browser interaction or deployed authentication. The fixture correction does not change native behavior.
- **Focused regression coverage passed:** 20 Rust controls on `be1fccf` cover workflow deadline bounds, retained credential outcomes and adjacent cancellation, shutdown and installation behavior. Five later Rust controls on `5f764b4` cover official GitHub installation readback and credential selection. 66 offline verifier controls also passed. These component results do not qualify the installed recovery cases.
- **The Python environment failure is accounted for:** the earlier combined run passed 13 Console controls but failed importing a separate readback test because its environment had an older MCP API. Only the unexecuted test was rerun in the existing MCP 2.2 environment and passed.

- **Python source compatibility passed:** Connect 143, Core 239 and Meta 131 controls.
- **TypeScript builds and selected runtime tests passed:** Connect 205 and Core 9 session-event controls, Core/Connect builds and Connect typecheck; Core 95 and Meta 35 protocol controls. Meta build and typecheck passed; its initial missing test launcher was corrected for the protocol run.
- **The upstream type-test correction merged:** the original Core public-type test failed because its supported-key list omitted the existing `session_events` field. The isolated one-line correction passed the same type test. [Core PR #1944](https://github.com/lukacf/meerkat/pull/1944) merged on 24 October 2026. This test-only patch does not change Connect's selected native revision.
- **Matching ordinary `rkat` compiled:** the Core `0a3cc302` build completed successfully. The installed CLI refusal cases below use this binary; compilation alone is not installed-use evidence.
- **Native compilation and all 92 selected controls passed:** the `2a8b345` build emitted the required test binaries. The 92-control result includes cancellation, stale-authority, issuance recovery, correlated-target replay, retained-original admission fences, reviewed dynamic setup and cold continuation. These native component checks clear the prerequisite for installed testing; they do not establish full installed recovery or frontend acceptance.
- **The three prior fixture failures are corrected:** the cold final-close test now checks the cancelled setup agent's actual operational and relaunch refusal while preserving its historical records. Dynamic setup initializes its synthetic credential through the existing native lifecycle. Cold continuation joins the existing source recovery worker and requires its durable promotion before simulating custody loss. These corrections change fixtures, not production authority or readiness checks.
- **Corrected verifier controls passed:** 49 offline controls include requiring the original setup agent's exact finalized close receipt before outer shutdown can pass cold acceptance. These are verifier controls, not installed runtime acceptance.
- **The matching product built, but installed qualification failed:** the `2a8b345` gateway and CLI build passed. The seven packaged cases plus cold selected-installation case finished with **2 passes and 6 failures**. Both passes prove that denied or interrupted access preserves the actual installation without letting a fresh ordinary `rkat` use it. Four private Services cases stopped at setup catalog readiness. The Forge case selected a legacy protocol incompatible with its current fixture. Cold shared setup stopped before installation because the new caller's acquisition deadline exceeded its selected parent workflow deadline. The first command's noncanonical resource-path failure remains retained separately; correcting the path reused the same binaries.
- **Supplemental installed checks also failed:** all four cases completed. Shared setup and setup-agent respawn stopped at the first Find operation. Helperless Restore preserved the canonical identity and session, but the verifier incorrectly required a changed runtime ID. Console reopening preserved the sessions and passed idle panel reads; its later helper Send lacked a selected governed source route. Fixture corrections must preserve the native authority and Restore contracts. No successful second tool use, full shared workflow or cold installation recovery is established by these failures.
- **The earlier authenticated attempt failed before the read:** the GitHub attempt stopped at task access with `reprepare_required`, before `get_me`. Its outer result records successful removal of the isolated test credential. Native shutdown was acknowledged, descendants and readers joined, and no forced cleanup was needed. This does not establish provider read acceptance or grant revocation.
- **0.9.5 SDK package alignment remains open:** Connect's Python native-host extra and TypeScript dependency/lock metadata still select upstream 0.9.4. Passing source-selected SDK checks does not resolve that distribution gap or establish provider, installed-product or release qualification.

#### Earlier source checkpoints

The following records retain their original evidence and qualification limits.

The `40bbef9` native run
passed 89 of 92 controls. Its three failures were the later cold-close predicate,
missing synthetic credential lifecycle initialization, and missing source
recovery-worker enrollment described above. The successful `2a8b345` rerun
preserves that original failed result and its logs.

The `481416b` native run
passed 86 of 89 controls. Two fixtures expected one model request despite their
deliberate probe and final-answer sequence; correction `40bbef9` asserts the two
requests after the completed turn and retains exact native-input and replay
checks. Its third failure exposed an optional static cold-capture lookup that
rejected a current dynamic caller before warm submission. The same correction
lets that caller decline optional capture through the existing route authority;
it does not grant dynamic callers cold-recovery eligibility.

The earlier `e05f4c3` native run
passed 62 of 63 controls and stopped at cold launch observation. Its source
correction and final-close recovery are included in `481416b`. The subsequent
`168f52d` compilation failure
came from two test expressions using unsupported identifier traits; correction
`0ce4349` uses their existing equality and string accessors. Those failed
records remain intact despite the later successful compilation.

At the adoption checkpoint, the selected development pair was Core
`0a3cc302dc916763571f34044978ae5f7420f764` and Meta
`2c80ede4e66041f6dd05a2ff62518c2d8b14583b`. Connect's root patches selected that
Core for Meta's exact 0.9.5 dependencies. Native compilation, matching SDK
checks and installed acceptance had not yet run. The completed results above
record the later executions; this adoption note is not the current status.

The retained Core `60587099` / Meta `cc0751d4` results keep their original
source and artifact bindings. Native compilation on frozen `36819e8` passed.
Its 57 selected controls
recorded 52 passes, four executed failures and one unmatched test name. The
renamed test passed separately,
giving 53 distinct passes without changing the original failed report. Fixture
sequencing corrections for the four failures are committed as `0c283ef` and
`efde388`. The 62-control successor on frozen `efde388`
completed with 61 passes and one failure in the fresh post-restore probe fixture.
At that checkpoint, correction `de50fda` was committed and unrun. The
same-session hard-cancellation and useful-follow-up control in `c8a4f54` was
reviewed and unrun, awaiting the corrective pair. These results did not qualify
full installed acceptance.

- Python SDK compatibility: 143 existing Connect controls passed against the retained Core `60587099` / Meta `cc0751d4` SDK bytes.
- The original Core TypeScript build failed on exact Core `6058709`: `MobTurnStartParams` lacked `session_events`. Core SDK-only correction `e32996624532c41840d86892282848899b372ee9` adds that generated optional nullable field. The corrected SDK build, nine affected Core controls, Connect typecheck/build and 205 selected Connect TypeScript controls passed. These reused the unchanged Meta TypeScript build. That SDK correction did not change native Core `6058709`; these results do not qualify the newly selected SDK bytes, native runtime or provider use.
- Ordinary Core `6058709` rkat build passed. Installed use remains unrun.

The preceding native behavioral subset used `5c04a90`, selecting Core
`cdc7972c73eb027692d8d064d46a13ce527dd423` and Meta
`69318a77d03b2af1ff06c5fb55da19545ea0ccdc`, adopted by integration pin
`99f407c`, with native lock digest
`80b83052fff8fe897f125660318fe30e0bb35ad64726e53014e29a8b85cf4077`.
The source/API preflight
confirms the required hosted-MCP methods and selected Core revision. The
lock preflight
passed locked metadata resolution. The two-library build then failed with two
errors from one missing direct `uuid` dependency in Connect. Its toolkit test
binary was emitted and reused: 13 of 15 selected tests passed, while two failed
before their intended assertions because of fixture setup omissions. The 13
selected Connect tests could not run because their binary was not emitted.
Neither the enclosing build nor the partial test batch passed.

Corrections `b07d312` and `6e55e92` add the missing dependency, bind the test
fixture to its actual host, and use an identity already permitted by the test
policy. Source review, locked metadata for the dependency, and Rust syntax
parsing for the fixture changes passed. Successful compilation and rerunning the
affected tests remain pending. These corrections do not change the earlier
results or establish packaged acceptance.

Later no-run compilation on Core `3a57ae7` and Meta `c14eb25` exposed Connect's
ordered-collection use of `InputId`, a missing waiter-provenance argument, and
two fixture API mismatches. The latest retained build log
and earlier failed attempts retain those diagnostics. Source-reviewed fixes
preserve deterministic input serialization, use no helper provenance for waiter
rows, and correct the fixture assertions and transport access. They do not
establish a passing native batch. The selected development pair and its source-only
evidence are recorded in [BUILDING.md](../../BUILDING.md); changing that selection
does not qualify the Connect user journey.

The previous `038343bb00fc646bc245f6edb9e2ddd2ffdbfc84` candidate selected the
same Core and Meta `25b5ea164a51b8614710bb1fbaaee6dc2755d537`, with native lock
digest `a77c6894ad5aabaded097beadf9de55d2d91e0f441b19317b0e437a7fa6cd47d`.
Build 3766 is terminal, failed:
Cargo exited 101 after four distinct compiler errors. Three hosted-MCP methods
were absent (`credential_landed`, `resolve_mcp_auth_target` and
`member_mcp_custody`), and Connect's identity-error match omitted three new
variants. No behavioral tests or installed cases ran. The retained
build record
records the failed no-run compilation. Meta `69318a7` restores the facade
methods, and Connect `978c9a7` adds the fixed diagnostic labels. Neither source
correction converts that failed candidate into a passing result. Matching SDKs,
resources and native validation are still required for the successor pair.

Source checkpoint `3f52438` binds member MCP setup to the exact accepted
application requirements and rechecks them after asynchronous reads and before
grant issuance or finalization. Independent review caught two stale-read gaps
before this checkpoint. It preserves the existing human approval and grant
owners. Its three native controls passed on frozen `5c04a90`; the Python callback still
reports this capability unavailable. This is Rust groundwork, not a usable
Forge setup flow or an implementation of approval reuse across members.

Source checkpoint `c5bb99c` lets application setup omit a permanent helper,
including applications with no initial members and declared classes for later
members. It uses the existing preparation, held-completion and adoption owners;
helper-only fields and resources are absent. Independent review and Rust syntax
parsing passed. All seven focused native controls passed on frozen `36819e8`,
including the six Connect controls previously unrun on `5c04a90`. The empty
held-completion fixture stops before runtime adoption, later member creation and
process Restore. The existing supplied verifier now has a source-reviewed
`--helperless-restore` mode covering those missing boundaries: empty native Ready,
one actual Add and callback call, joined shutdown, then a second process using
the same installed selection, canonical admission and session for another audited
callback call. The mode and its four focused controls are source-reviewed, not
native execution evidence. Helperless startup does not by itself provide an
agent conversation for discovering integrations.

The previous packaged product source `cffd27785a547e436d87364e581f558348a07da5` is an
unreleased development candidate, not a completed shared-setup or release
qualification. It selects Core `05487bcf0de062a084570d6a49651106530f34d2`
and Meta `1f9f84157ff92d1d8f372453b75d6d1af9a4121a`, with native lock digest
`ffd367036758e436702980359ba781f2a60f8f9f8c0adc4a73bad5f7ad9a6c17`.
Use matching SDK and packaged-resource selections. Earlier pairs and results
below keep their original provenance; they are not executions of this head.
The combined gateway/CLI build passed. The original assembler rejected its
two explicit binary targets; correction `6fd7fbce` restaged the same emitted
bytes without another Rust build. The original failed staging record remains
retained. The corrected assembly controls passed 68 tests with one platform
skip. Successor verifiers are pinned separately from the compiled product.

| Area | Established evidence | Remaining qualification |
| --- | --- | --- |
| Ordinary dynamic members | Acceptance98 passed on Build97 (`c8109cb`): real late Add, same-session generation-changing respawn, `customize_build` and `after_create`, tool calls before and after rebuild, stale-scope refusal and joined cleanup. | This does not prove cold restore, shared G reconstruction, member MCP account access or every application customizer. |
| Shared commissioning and caller preparation | Rust, Python and TypeScript expose the host-selected path: retained parent and WorkGraph claim, independently authorized G, held Add and durable application listing ACK. | The complete two-caller installation/use/ordinary-continuation journey has not passed. Preparing a child does not submit it or complete acquisition. |
| Latest shared product attempt | The gated `cffd277` run, using verifier `1f431f6`, passed the exact bound replay checks but stopped when A's first `find_and_install` returned `operation_authorization_unavailable`. All nine cleanup checks passed with acknowledged shutdown and no forced cleanup. The earlier replay refusal remains retained. | Source-reviewed correction `eb97e70` makes the policy read wait for its existing map lock instead of treating contention as unavailable. The targeted control stopped during fixture setup on `36819e8`; its committed correction awaits rerun. Attribution of the actual installed failure to this defect remains unproven. B's later refusal occurred during teardown. Full workflow and same-session G respawn remain unqualified. |
| Shared observation and respawn corrections | `f278d99` preserves unavailable backend/grant reads and passed two focused SQLite/native controls. `2f8a1e7` reuses the bounded enclosing observation for binding checks and child preparation. `82abb7c` retains approval/session and workflow custody across same-session G respawn while replacing only the exact retired peer. | The corrected native pin-refresh control passed on frozen `5c04a90`; installed execution remains pending. A pure observation rebuild does not retry installation, issuance or submission. Cold reconstruction remains separate. |
| Selected GitHub preparation | The latest `cffd277` retry reached credential POST 202, `Stored`, one `get_me` grant and retained task publication, then stopped on a transient Console status 403. No `get_me` or native authenticated read occurred. All nine native cleanup checks passed without force. The failed result remains retained; separate exact-owner cleanup verified stored presence and removed the isolated secret. | The exact `get_me` grant leaf remains durably retained after its recorded expiry, 2026-10-23 16:51:42.252 UTC. Revocation was not performed; no fresh native authorization observation was made. The verifier correction passed source review and the affected offline suite (15 tests); neither authenticated-read acceptance nor complete grant cleanup has passed. The earlier request-policy failure remains retained. |
| Host preparation and fast checks | `fccbf42` forwards explicitly pinned families through product, starter and preparer. `63538e4` forwards bundled Node/npm only for selected Node families; omission preserves defaults. `0d62e43` reports an unavailable source-default distribution explicitly. On frozen `0d62e43`, 32 starter and 13 export tests passed, and the documentation check covered 50 pages. The unaffected resource-packaging suite (45 tests, one skipped) and 29 shared adapter tests retain their earlier passes. | Fast checks do not establish installed or provider acceptance; the earlier starter failures remain recorded. |
| Lightweight CI | The exact 15 contract controls selected by `e00f192` passed locally with Python 3.12. | This is local evidence, not a hosted CI result. |
| Private Services conversation | Source-reviewed `5f7b3a7` composes protected plain-service review, consuming native `Closed`, joined cleanup and the existing configuration/grant finalizer over the actual session and initial-attempt guard. Ten focused controls covering the initial-attempt owner, retirement and plain finalization passed on frozen `36819e8`. The source packet retains the original finalization selection. | All packaged cases remain unrun. Source-reviewed `1f947da` adds stopped-helper retirement and crashed-helper refusal cases, and corrects the existing handoff assertion. These cases interrupt pending review after installation; they do not cover mid-installer writes or an entered grant effect. Preparation/help smoke remains separate from actual installation, access finalization and a fresh ordinary `rkat` read. Services/ForgeServices remain unqualified. |
| Setup progress responsiveness | `f3d88cd` lets exact job and job-list reads use the Controller's existing short lock while effects remain serialized. All 22 affected Python controls passed, including concurrent own-job reads, read-entry closure and blocked effects. `89b9103` also removes discarded Node toolchain probes from shared state; five focused offline controls passed, preserving fresh ordinary planning and changed-tool refusal. | These establish local concurrency, isolation and avoided probe work. They do not establish deployed latency or native product acceptance. |
| Focused native validation | Both library test targets built on `0d62e43`. Six of seven selected controls passed, and the separate cross-application isolation control passed: seven of eight overall. The idle pin-refresh control failed with `Unavailable` during fixture restoration, before its pin-refresh assertions. | The batch remains failed. The later fixture correction `156c058` passed its exact pin-refresh control on frozen `5c04a90`; that later pass does not change the failed `0d62e43` batch. These results do not qualify full shared execution, authenticated GitHub use, installed G respawn or cold recovery. |
| Hosted MCP authentication | Registered completion, cancellation, retirement, retained joins, factory binding and redirect-base controls passed as rows within Controls80 (42/46 overall). Startup and cleanup SDK controls also passed. | Deployed authenticated-provider use remains unqualified. A failed start without proven ceremony disposition stays uncertain; no returned ID does not mean nothing was admitted. |
| Ordinary installed restart | The actual `cffd277` reopen case passed through the existing `Bootstrap`/`Restore` path. Both identities resumed with unchanged native sessions. Reopening the setup panel made zero model requests and no setup-task activity; old login refusal, new enrollment retirement and joined shutdown passed. The restored helper also failed MCP startup with `local_setup_previous_session_needs_review`: the old local session had no live round offer or retained selections after shutdown. | Source-reviewed correction `8dc6194` adds one-use reattachment for an orderly closed, empty ordinary round through the existing owners. Its five Python controls passed across two runs, and four native local-round controls passed on frozen `36819e8`. Corrected installed Restore remains unrun. The earlier case proves session and panel restoration only. Usable helper tools, post-restart task execution, interrupted work and cold shared-G recovery remain unqualified. |
| Selected workflow recovery | Source and effect-reference reconstruction are implemented. Controls93 passed five idle reconstruction, cancellation, revocation and writer-contention cases on `b263ba1`. Physical close/reattachment, approval observations and the installed custody association are connected in reviewed source. `44b87b7` adds exact settled P completion/leave attachment; its Python module passed 23 tests and 17 subtests. `4756d3a` adds exact cancelled-source cleanup through the existing cancellation owner and worker. | The original five native P controls later passed on `2a8b345` in the 92-control batch. The later completion-write-failure extension passed on `068f515`, as recorded in [current integration status](#current-integration-status). The full installed selected-P cold path remains unrun. The new cancellation path does not recreate an admitted original or resume effects. Idle reconstruction does not prove both live recovery owners converge after real Ready. Actual two-process recovery and selected-preparation replay remain unqualified. See the [remaining recovery work](#cold-shared-g-recovery). |

The reviewed acceptance-only successor `9d905b7` adds an opt-in shared G
same-session respawn mode to the existing verifier. Its 27 offline verifier
controls passed with the exact matched SDK environment, separately from the
45 starter/export checks above. Native respawn and full workflow acceptance
remain pending.

Verifier corrections `cfbf2ad` and `1f431f6` passed 10 GitHub and 31 shared
controls. They cover normalized read metadata, exact replay ordering, failure
wakeups and bounded waits. These fast checks did not predict the remaining
installed authorization and request-policy failures above. The subsequent policy correction passed 11 GitHub
controls, and four new reopen controls reject the observed helper startup failure
even when its enclosing notice says `completed`. The documentation check still
covers 50 pages. Actual product cases remain mandatory; absence of a startup
failure notice does not prove tool execution.

Verifier `bf6b2a5` now requires an actual `setup_inspect_schema` call after
ordinary Restore, its successful result received by the model, and a committed
answer on the same restored session and native run. The existing zero-work panel
checks remain separate. Nine offline verifier controls passed,
including missing evidence, replay, wrong arguments and mismatched session or
run refusal. The installed case has not run with this stronger verifier.

Source-reviewed Console correction `337149a` preserves typed current-task
`Unavailable` as HTTP 503 and real denial as 403, with a fixed-label diagnostic.
It adds no retry or authorization exception. Its focused status-mapping control
passed on frozen `36819e8`; corrected installed validation remains unrun.
The frozen `cffd277` product contains neither this correction nor `eb97e70` or
`8dc6194`. Their source review and focused controls do not replace execution of
the corrected packaged product.

Ordinary single-caller CLI and integrated application acquisition remain distinct
from shared setup. Sharing is an explicit host-selected optimization: the host
selects each participating original before its first submission. Later new
originals can join through that same owner; already-bound singleton originals
are not retrofitted. The supplied gateway opts in with
`--private-workflow-launch`; ordinary startup remains unchanged. Frontend
plugins still require the application's participant, audience, routing and
reply-delivery owners. These checks do not narrow the intended CLI or supported
Rust, JSON-RPC, Python and TypeScript surfaces to one test host.

The selected Meta pair includes `AgentCustomizerRequired` and actual
`after_create` invocation. Connect reports the actionable error when declared
member classes lack a gateway-composed customizer; ordinary apps may still omit
one. Acceptance98 qualifies the ordinary callback/rebuild path, replacing the
earlier ca261 callback-pending result. The implemented warm shared G correction
admits its exact Active/Resume successor using the same Driver's retained resource
owners, preserves the canonical approval binding and uses `check_session` instead
of repeating single-use `bind_session`. Its existing eight-peer lifetime bound
remains; unlimited respawn is not qualified. Resuming a Meta store written by
MobKit 0.8.9-0.8.28 remains
unsupported. Preserve user state and verify compatible saved definitions,
selected sources and their native owners; this older-store limitation is not
an additional 0.9.5 release gate.

The official GitHub `selected-reads` route carries its checked stdio launch,
declared credential slot and operation mapping through native installation and
readback, including Core's validated working directory. The pinned server reads
configuration there; it must not be removed or guessed. A packager must explicitly
select `official-github` in the pinned resource manifest. Payload presence alone
does not enable it. The failed `f278d99` attempt created no credential-persistence
proposal and left the existing credential store unchanged. The latest retry
established joined shutdown and separate removal of its exact isolated credential
slot, but actual use remains unverified. The exact durable `get_me` leaf remains
retained after the recorded expiry above; revocation was not performed. This is
retained-record evidence, not a fresh native authorization observation. Isolated
removal does not revoke the provider token. Packaging or
discovery of an authorized account cannot substitute for that result. Integrated
browser approval and logout remain separate acceptance.

Forge owns the trusted HTTPS base, callback mount, login composition and its
application's durable roster transaction. Connect supplies the native endpoint
and SDK handoff. Forge's regenerated host application must verify its declared
classes and actual customizer; a fixed-roster application has a different
requirement. Login completion, member reconnection, service use and cleanup
remain distinct observations. Ceremony disposal does not establish credential
removal or server unpublication. The selected Meta pair retains the known L12
persistent-factory failure paths; the reviewed upstream correction remains a
final-artifact prerequisite. Final upstream adoption, matching target artifacts
and SDKs, Forge application acceptance and publication remain separate release
work; retained development binaries must not be relabeled.

</details>

### Cold shared-G recovery

Cold shared-G recovery remains unqualified as an installed workflow. A new upstream
activation callback is not a prerequisite. The existing installed host binds the
native runtime and identity owners before ordinary restore, composes the actual
member build, then publishes native Ready and starts its retained recovery owner.
Fresh held-bootstrap finalization must not be repeated for an already adopted
installation. Source review first confirmed this path in Meta
`b2c628b9dd8276673f1e3ac2d40dc220f512f178`; subsequent installed composition
source uses the same existing owners. This is not a native recovery acceptance
result.

Cold recovery must reconstruct physical resource and approval custody from
owner-produced close observations. Canonical admission, session, workflow,
source and effect records can identify prior work; they do not alone establish
that its Controller jobs, native attempts or approval dispatches settled. Ready,
process exit and diagnostic attempt files cannot supply that disposition.

<details>
<summary>Historical cold-recovery implementation and test checkpoints</summary>

These records describe the evidence available at each named checkpoint.
Their unrun tests and pending reruns are historical; use the current integration
status above for later executed results. Installed acceptance remains separate.

The physical preparation in `0cf27c3` retains exact owner-produced close
snapshots and request history, then allows a one-use physical reattachment
claim. Independent source review passed, and 52 affected Python tests passed
against the actual setup Controller and local lease code. They cover configured
and agent-selected installations, changed requests, incomplete publication,
unknown work, stale claims and refusal to reuse old effect handles. These are
local controller tests, not native recovery or provider acceptance.

The installed composition preparation in `c2cd1f1` retains the existing native
guard owner and drains it before closing shared setup connections. Source
review caught and corrected that ordering. Its Rust tests and installed execution
were unrun at that checkpoint.

Approval-owner observation in `6d846c3` preserves unresolved predecessors while
identifying settled approval records. It is an additional source prerequisite,
not a reconstructed live approval owner or native cold-recovery result. Retained
old preparation and validation handles remain readback only; physical
reattachment does not authorize another dispatch through them.

The gated primitives in `bd2154c` now associate the exact completed artifact
with its canonical admission and owner-produced native, approval and physical
close observations. They retain the one-use physical restore and association
update task. Source review and seven focused controls on frozen `36819e8` passed.
Source-reviewed `cc9f852` connects the restored Active class restrictions before
native capture and reconstructs the late helper from its original admission and
server provenance. It attaches the empty workflow and current helper controller
together before publishing the recovered plan, including when no callers remain.
Of its six focused controls, three passed and three failed on `36819e8`; the
committed fixture corrections then awaited rerun. Legacy rows without original helper provenance
remain refused. The existing private-workflow launch option now selects ordinary
installed Restore; initialization and replacement remain refused. This opt-in
path is unqualified until actual two-process acceptance proves dynamic helper
restoration, a fresh B caller and restored helper tool use.
Connected source and static controls do not qualify cross-process recovery or
replay; saved approvals and process tokens do not become authorization on restart.

Source-reviewed `44b87b7` attaches exact settled P readback through the current
restored G and canonical source owners, using the existing completion and leave
transitions. It rechecks source and membership after asynchronous resolution;
old P cleanup cannot stop the successor caller's work or revive an old approval
or effect handle. The affected Python module passed 23 tests and 17 subtests.
`4756d3a` extends exact cleanup to cancelled cold sources through the existing
cancellation record, without reconstructing an admitted original. Public cancel
and the existing recovery worker both recheck source and P state before physical
leave. Five native controls across these changes were unrun at that checkpoint.

The installed verifier introduced in `b63acad` requires an actual selected package
installation, protected review, nonempty P, joined process close and Restore,
and a successful A cancellation acknowledgement before B's installation or
service effects. The current source keeps the canonical workflow open with A
until B joins; last-waiter departure seals it and cannot be followed by a fresh join.
Its 45 offline controls passed
after correcting a test-observation isolation error. The controls for corrected
join/departure ordering and actual installed run were unrun at that checkpoint.
This scenario keeps A live until Restore; the separate native
controls cover the already-cancelled path. Neither proves new completion
publication after a lost reply or recovery of arbitrary interrupted effects.

</details>

The bounded installed slice targets an owner-proven empty resource and a
fully settled resource with retained request history. Pending approvals, unknown
effects, interrupted inputs and unconfirmed cleanup retain their exact work and
remain unresolved until their owners reconcile them. Supporting those cases is
still part of the full recovery goal; the orderly slice does not redefine it as
empty-only restart. The admission-only shutdown fence and upstream native-close
corrections help preserve or distinguish work, but do not prove physical or
approval settlement. Neither complete cold recovery nor selected-preparation
replay has passed installed acceptance. See the
[host composition guidance](../site/guides/host-composition.mdx#restore-selected-workflow-sources-unreleased)
for the already implemented source boundary.

### Historical early 0.9.5 checkpoints

These records describe their named source snapshots, not outstanding blockers
on the current pair. Their original artifacts, failures and cleanup evidence
remain retained.

- G commissioning source review passed at `b24222584d42d624b786134a1982c84cb8dffaf2`.
  The SDK checkpoint `b63f02774cb25114e362393bd09aa8938d270026` recorded 162
  Python and 182 TypeScript tests passing, plus TypeScript type checking/build.
- Build70 at `6e38374a797bff7be78bbe02de190f9e3b30353b` passed. Acceptance71
  then failed with `HeldBootstrapRefused(NotHeld)` before protected service use.
  Its exit-1 cleanup error remains a failure, despite acknowledged shutdown and
  joined readers. Later customizer fixes and Acceptance98 do not change that record.
- The `c11faea5` check compiled selected native test targets without executing
  them. `ba5c2375` fixed member-class customizer composition; `06475b9` added the
  interim missing-customizer refusal. Their then-pending Meta diagnostic and
  callback prerequisites are supplied by the current pair.
- Selection `34c7e133` used Core `47308df9`/Meta `ca261564` and lock
  `dce48749bf0b6b1178117f0634d20bbacc9b0f58bb7bd02d308f55e7e1a8618f`.
  Hosted native startup `6aa6fa0d` and SDK startup `6a527739` recorded 213 Python
  and 201 TypeScript checks against their matching SDK selection. The optional
  Core TypeScript dependency in that SDK check was the published 0.9.4 package.
  Those checks did not establish a provider read or qualify the later pair.
- The GitHub metadata/readback checkpoint recorded 69 Python tests passing.
  The protected-read harness's offline controls and Resources74 package checks
  were separate from native installation and account acceptance.
- The private application-model-custody launcher successor was source-reviewed
  with one supported credential root and explicit Guide/application custody.
  That review alone did not establish execution of the packaged application.

### Graceful shutdown acceptance before enabling a drain window

The older packaged `cffd277` candidate selects Meta `1f9f841`, which exposes
`begin_close` and `join_close`, but no `stop_admitting` hook or active-turn grace
setting. Its `drain_timeout` covers event draining. Connect's tests at that
checkpoint cover typed refusal during an active turn and host closure separately;
neither qualifies draining behavior.

Source review confirms that upstream Meta `b2c628b9dd8276673f1e3ac2d40dc220f512f178`
has both the hook and `shutdown_in_flight_drain_ms` (0-300000 ms). Omission or
zero skips the drain phase. That review used a different Core selection from the older packaged product.
The current development pair is recorded in [BUILDING.md](../../BUILDING.md);
the hook's source availability and coherent dependency selection do not establish
native drain behavior.

The planned active-turn drain window retains `D = 0` as its default. Connect
commit `826baa5` implements the admission-only fence for M6b and has independent
source-review approval. It rejects new source submissions, ordinary and shared
member reservations, Add acceptance and recovery rounds while preserving entered
receipts, finalizers and late installed-host adoption. It does not reuse the
member-class close flag, which would also reject entered completion.

Four behavioral controls cover bound-source refusal with a held accepted
callback, an entered member finalizer with a refused fresh Add, refused shared
reservation, and parked recovery waking for full close. All four passed in the
retained `efde388` native run on Core `60587099` / Meta `cc0751d4` (controls
08-11), and again in the 92-control `2a8b345` result
on the then-selected Core `0a3cc302` / Meta `2c80ede4` pair. That batch also passed
direct retained-original-binder refusal and typed helper feedback to the same
entered turn. Later regression `4c93dc3` covers a real retained helper binder
paused before its first native capture: after admission stops, it refuses the
helper submission without changing the canonical helper row or entering native
execution, while the already-entered A turn completes. That exact control
passed on `3a97427`
in the five-control batch recorded above.

Connect `3d4f219` adds an explicitly ignored native acceptance control for late
Driver adoption with a real acquisition owner. It runs fenced and unfenced
cases through the actual private channel, finalizer and native Ready producer,
then checks the same recovery control and joined cleanup. Both cases
passed on `a601f06`
with zero model requests, native shutdown acknowledged at exit zero, and all
nine process/reader/descendant cleanup checks true. The test calls
the Driver admission fence before adoption to cover a defensive ordering; it
does not exercise the gateway's real nonzero drain phase or qualify SIGTERM.

Connect `ef7d764` adds a separately ignored control with a 30000 ms fixture-only
drain window. It holds a real entered Console response until Meta calls the
actual admission-stop hook, then requires the governed helper call's typed
refusal and the same native run's committed answer, input audit and joined
cleanup. Its first native execution failed in 104.45 seconds:
Meta reported `host_admission_open` and skipped the drain after one actual
Console model request entered. The reviewed call path
shows that selected Meta `6b79555` calls the host's full `begin_close` from its
stdin shutdown-handshake reader before reaching the drain phase. Connect's
probe then rejects the already-closed host. The native child acknowledged
cleanup and exited zero with all nine cleanup checks true, but the provider
fixture failed and no post-stop helper result or committed answer was proven.
The observer remains unchanged; this is a selected Meta ordering prerequisite.

The release owner received this failure and the subsequent Ready-ordering and
test-coverage findings. Its reviewed correction defers full close for an
initialized nonzero-drain SDK handshake, with the flag armed only after
successful Ready publication. The first-twin Connect `f59793f` / Meta `0df4b7f`
test build passed in 2544.34 seconds.
Its controls were explicitly not started
because the owner supplied the revised production patch before they ran.
That build is not a drain-control pass.

The reviewed final validation selection
is Connect `3d6299ff` / Meta `bedbdabcd279234e278481518fa9f2c3e248394e`, retaining
Core `dd0ada78`. Only the two native dependency files differ from `a601f06` by
the Meta source substitution. Locked offline metadata retained the same
547-package identities apart from Meta. Its test build passed in 2543.32 seconds,
and all 13 selected controls passed.
The unchanged control 13 ran first, followed by controls 1-12, using the exact
emitted binaries. The drain case made two model requests and committed the
same native input's answer after the helper refusal. Meta captured the Original
as completed, reported `drained`, and waited 393 ms within a 29999 ms remaining
window. The coordinator and provider joined; native shutdown acknowledged exit
zero without forced cleanup, and all nine cleanup observations passed. Both
late-adoption cases also passed with joined cleanup.

Root review
and independent review
verified the exact command records, source bindings and three lifecycle cases.
Both release recipients received the terminal result and its limits.
This qualifies the selected native controls on `3d6299ff`. The subsequent
product build passed, while full two-caller execution reached B's selected
installation and failed during reconcile. Recovery remains uninvoked; the
[current integration status](#current-integration-status) records that later
failure and its separate source scope.

The before-Ready test covers an already-observed close, not a forced
reservation/publication race. The owner reported a later test-only correction
to the claims and assertions, outside the selected `bed` revision, and recorded
stronger race coverage for 0.9.6. The acknowledged
postpublication/prestore immediate-close fallback remains a qualification
limit; source review does not prove that interleaving or SIGTERM behavior.

If a future qualified run completes before Meta captures its drain snapshot,
the result explicitly records completion without drain-capture evidence. An
empty `ended` list never proves the drain captured or waited for the run.

In the first `a601f06` / Meta `6b79555` batch, both controls used the same emitted
Connect test binary. Its independent review
retains the late-adoption passes and drain failure. The subsequent `3d6299ff`
native pass permits matching product validation; it does not establish installed
frontend adoption or SIGTERM qualification. In particular, a later SDK handshake
during a SIGTERM drain is outside the passing fixture's scope and requires its
own evidence. The frozen `cffd277` binaries contain none of this new source.

Keeping `D = 0` needs no new Connect implementation merely to expose this API.
Acceptance then requires a host-governed tool
call from an already draining turn after that hook. The call must receive its
correct answer or a typed refusal; it must never be silently dropped. Meta owns
its own drained-turn callback check under SIGTERM, which does not replace this
Connect host-side check. Connect has not enabled a nonzero drain window or
qualified M6b. The separately announced upstream change to preserve running
durable turns during ordinary `D = 0` shutdown is not established by this source
review and remains pending selection and verification.

### Startup failure diagnostics

The selected setup composer now retains a fixed failure stage and typed error
category in the existing native refusal. Application owners can distinguish
member selection, approval creation, session binding, review composition and
presenter transfer failures without exposing account details, private paths or
nested provider errors. These diagnostics do not change retry permission,
authorization or cleanup. The affected composer tests pass, including real
storage and channel failures, redaction and review retirement. This is an
unreleased correction, not new installed-application acceptance evidence.

### Release separation

<details>
<summary>Historical 0.9.4 release and development record</summary>

The following is the historical 0.9.4 release record. Its branch and dependency
selection do not describe the combined development head above. Statements about
current manifests, pending builds and release selection below belong to these
earlier checkpoints, not the current validation queue.

The `codex/connect-0.9.4` branch contains the ordinary CLI and Console work:
individual setup, agent-led acquisition, protected credentials, cancellation,
recovery, installation and continuation of the original task. Existing installed
integrations can still be reused when the caller is authorized.

Known limitation: Forge cannot update an existing Connect application in place in
0.9.4, including an application that completed setup and ran successfully.
Rebuilding an accepted application does not update the installed application;
the Stage/Apply path remains unsupported and refuses the change. The upstream
update producer and lineage correction is assigned to 0.9.5. Preserve the
existing installation and its saved state. Fresh application
installation and normal agent-led discovery, protected installation and
authorized use of integrations are separate supported paths; this limitation
does not prevent installing an integration into a running application.

Shared multi-caller setup belongs to the separate `codex/connect-0.9.5-shared-setup`
branch. It combines overlapping requests under one setup assistant while retaining
independent approval, access, cancellation and results for each caller. Deferring
that optimization does not remove individual setup or integration reuse. The
0.9.4 source excludes its workflow, enrollment and execution code rather than
shipping a disabled shared implementation.

The original mixed draft is preserved at
[`ffeb622f`](https://github.com/lukacf/meerkat-connect/commit/ffeb622f9d06a0190ba87eb2290d67909ee1dfe1).
Shared development stores are not supported downgrade inputs for 0.9.4. Do not
remove their membership fields to make them load; retain them for the shared
branch or use a separate ordinary development realm.

The merge gate is the released Core/Meta 0.9.4 dependency pair and matching SDKs,
with passing affected native controls and ordinary CLI and Console acceptance.
Execution records retain their exact inputs. A reviewed source comparison can
carry an earlier result when the exercised path is unchanged; new artifact
bindings still need verification, and the earlier result is not a new execution.
These checks can run before publication once the source pair is selected.
Shared execution and broader frontend/deployment coverage
do not gate that merge. The lightweight CI checks packaging, panel behavior and
documentation; its success does not establish native or installed acceptance.

Build the final library, resource inventory and SDK distributions from the
ordinary 0.9.4 branch. The installed preparer loads its setup code from the selected
library archive, so a new gateway combined with an older mixed archive would
still contain the deferred entry points. Preserve the existing source, artifact
and resource checks when selecting the ordinary release inputs.

The final Connect 0.9.4 Console functional check passed. The final candidate selects
published Core 0.9.4 registry packages, with only the unpublished
`meerkat-machine-codegen` crate taken from the exact
[Core release revision `e91835c5`](https://github.com/lukacf/meerkat/commit/e91835c5fea0170d0dfc1d297d70269234fd4a16).
Its Core and schema dependencies use those same registry packages.
[Meta `324f5ff3`](https://github.com/lukacf/meerkat-meta/commit/324f5ff3157424e722a402b5593faee69fc3d682)
contains the Console navigation and idle-state fixes. It merged as
[`9c13361f`](https://github.com/lukacf/meerkat-meta/commit/9c13361f9b103e941f419c1a6b0ac041ffe05489)
with an identical full source tree. The later published release is described
below. Its
[Python SDK](https://github.com/lukacf/meerkat-meta/tree/324f5ff3157424e722a402b5593faee69fc3d682/sdk/python)
and [TypeScript SDK](https://github.com/lukacf/meerkat-meta/tree/324f5ff3157424e722a402b5593faee69fc3d682/sdk/typescript)
trees are independently verified unchanged, so reviewed artifacts remain reusable
when their exact inputs match. The final gateway build, fresh installation, normal
setup navigation and protected setup completed. The original conversation resumed
automatically and returned to idle. One actual follow-up sent through the Console
returned exactly `OK` and returned to idle, with no reload or manual wake. Canonical
records contain exactly the setup request, its automatic continuation and the
follow-up, all finalized; no database operation ran and the database stayed unchanged.

The official Browser tool could not target the rendered setup iframe, so the
approvals were clicked in the same observed protected page opened in a separate
task tab. Normal sidebar discovery and rendering passed; an in-iframe approval
click was not exercised. Both test grants were revoked and normal Stop ended the
application while preserving its files. Logout remains unconfirmed: the single
Sign out navigation returned `ERR_BLOCKED_BY_CLIENT`. No bypass or retry was used.
Earlier business results retain their exact execution provenance; this check did
not repeat the ledger business task.

### Published upstream selection and remaining production

Meta 0.9.4 is published at
[`3c3af34f`](https://github.com/lukacf/meerkat-meta/commit/3c3af34f0a12bfab3d33183f65da9fd2578d58e3).
The scoped installer review preserves the Connect helper route and guarded login;
Console, embedded bundle, SDK and Connect startup sources match the accepted
candidate. New early Python ABI refusal and Debian venv installation handling
are deliberate fixes. The ordinary-app path cannot take over a Connect helper
plan. No repeated SQLite business task is required by that review.

The released Rust crates include a compaction-retention correction after the
accepted build. Current manifests select the published Core/Meta 0.9.4 packages,
with the existing exact-source exception for unpublished Core codegen. New final
candidate production must build and verify this final graph. The previous
`340b8620` gateway remains development acceptance evidence, not a final-release
binary. The R1 Forge handoff requires actual final candidates and their provenance.
The release owner selected direct Linux and macOS builds for 0.9.4, followed by
one `v0.9.4` prerelease that is not marked latest. After the target verdicts, the
same release can become full without changing its URLs or bytes. Earlier
development outputs cannot be relabeled as those new final candidates.

Current packaged Linux dependencies require CPython 3.11, and the retained native
Linux gateway requires glibc 2.39 or newer. Generic Python SDK compatibility does
not widen compiled-wheel ABI compatibility. The producer uses Python 3.11 to
match this selected closure. Remote product deployment and in-place application
updates remain follow-up work for 0.9.5. There is no 0.9.4.1 release in this plan.

### Retained development evidence

The records below describe the earlier mixed draft and its exact historical
inputs. Shared results belong to the deferred branch, and no earlier pass is a
qualification of the separated source. An earlier development selection used
[Core `9281f3b0`](https://github.com/lukacf/meerkat/commit/9281f3b05b846ff31f9d74a146b8020c507aa818)
and [Meta `2ea41584`](https://github.com/lukacf/meerkat-meta/commit/2ea415849867ad2cfddee4090a9685c9ec11457e)
with matching SDK source trees. Published 0.9.0 assets remain unchanged.
The [earlier development evidence](#earlier-development-evidence) retains source,
artifact and execution references with their narrower qualifications and failures.

| Result | Current evidence |
| --- | --- |
| Standalone CLI setup followed by normal agent use | Pass with a real model on derived development inputs: Hacker News setup, four normal reviews, /quit, then a fresh ordinary rkat session returns three actual stories. Canonical messages and finalized completion verify the answer. The optional post-run ATIF exporter fails separately. |
| Installed SDK acquisition/use | Pass: the same Original acquires Hacker News and reads one story. Corrupted resources are refused before setup. |
| Shared setup (Connect 0.9.5) | UUID-corrected compilation and two focused controls pass. Shared1 fails at helper A catalog load with authorization unavailable. One-helper shared execution remains isolated source implementation; full completion is unqualified. |
| Console approval/recovery | Actual producer loss and restart reaches protected recovery. Stale, foreign, retired-login and published-A refusals pass. B revoke returns parent unavailable, child not entered and uncertainty, with no persisted grant change. Exact native branch attribution is pending. |
| Ordinary live-model read | Pass: fresh setup normally authorizes the actual OpenAI route; a fresh ordinary rkat searches, loads and calls the installed HN operation. Read leaves config/grants unchanged; physical owners join. Final 0.9.4 artifact qualification remains pending. |

Shared setup is assigned to **Connect 0.9.5**. It lets overlapping requests share
one setup assistant while preserving each caller's approval, access, cancellation
and result. It is not a 0.9.4 release gate. Individual CLI/Console setup,
agent-led discovery and authorized reuse of installed integrations remain in
0.9.4, including fixes for ordinary isolation and Console restart/revoke.
The complete shared workflow remains an end-state requirement.

The one-helper shared workflow still needs production execution and per-original
completion wiring. Its current path delivers planning context, but does not call
the existing per-waiter operation staging method; setup-result lookup only
recognizes ordinary helpers. Policy refuses unstaged calls. This is an
implementation gap, so expanding planning tests alone cannot close it. Isolated
source candidates now address the per-caller execution/result paths; native bundle
staging still needs the lead-approved seam after actual target/account registration.
No duplicate registration or fabricated account is accepted.

Adopted source corrections have these affected results:

- Console status handling (`6cff5eb`) preserves unavailable reads as 503 and
  explicit denials as 403. The panel clears stale actions, retains page position
  and uses its existing scheduled refresh after 503; it does not retry protected
  actions. All 55 JavaScript controls and three affected Rust controls pass. The
  corrected development gateway builds. Its actual status/approval sequence now
  passes; the whole journey remains unqualified after a later model-oracle failure.
- The verifier (`3a41ea7`) permits only the exact 503 status-read response to remain
  pending within its existing deadline. Protected writes are never retried, and
  stateful readiness observations remain single-entry. All 33 offline controls
  pass. Actual corrected execution observes native `Changed` mapped to 503 and
  successful later reads without repeating protected submissions. The earlier
  failure's precise native cause remains unknown.
- Standalone CLI staging/assembly (`d14c71d`) now reuses the candidate producer,
  preserves production bytes before test compilation and consumes the existing
  independent shutdown review. The 240 offline controls pass, with one
  case-sensitive Linux filesystem check skipped locally. Actual candidate
  production, review, assembly and installed qualification remain pending.

The selection-writer-v2 correction is adopted after independent source review
(`9743bde`), using the existing async writer and preserving source/cancellation
checks. All three native writer controls pass. The combined batch has seven of
eight passing: B's native continuation is terminal, but the combined
preparation-completion, delivery-acknowledgement and fixture-finished condition
times out. Its unsatisfied term and the earlier binding failure's cause remain
unknown. Source restoration and physical process joins pass; the failed journey
remains unqualified. The subsequent real-model CLI journey now passes with
consistent route selection; earlier route failures remain historical evidence.

Release selection still needs exact Core/Meta/Toolkit 0.9.4 artifacts, matching
SDKs, the native graph and the actual selected Meta reader. Linux x86_64 and macOS
arm64 use the existing independent target orchestration; neither target has new
release qualification here. Exact commit selection permits an optional tag only
when it resolves to that commit, with no branch/HEAD fallback. Existing public
source, byte, archive, attestation and reader checks remain. Descriptive producer
paths do not force rebuilding an otherwise unchanged library artifact.

CLI candidate commands retain the same CI producer directory; `stage-cli` requires
the existing CI preflight. The lead confirms that the standalone CLI may use the
existing retained native producer, independent shutdown review and assembler,
with checksums, review record and assembler input inventory. Clean GitHub-hosted,
attested production remains required for every Forge input, including the CLI if
Forge consumes it. Standalone CLI CI migration is a 0.9.5 item; the permitted
retained route needs no new producer, assembler mode or cross-job transport.
Actual selected builds and consumer qualification remain
required; no CI run, successor publication or public readback is claimed. The released
breadth limits below remain separate from the service-read development journey.

### Earlier development evidence

An earlier shared-preparation checkpoint used
[Core 95343786](https://github.com/lukacf/meerkat/commit/953437864bbb21b135661a0e61a09b5aa1f6ba01)
and [Meta f04125c4](https://github.com/lukacf/meerkat-meta/commit/f04125c45e732b3ef05aab2ac2a0f642fc1b08a1).
It adds exact preparation membership and retained native/source ownership.
Sixteen earlier owner controls, one SQLite reopen/cancel/complete control and five
focused disposition controls pass. Five additional SQLite workflow controls
cover concurrent joins, replay/reopen, sealing and late delivery obligations.
Six helper admission controls now pass after a meaningful prior-code failure.
The correction retries once only when native policy-change refusal confirms
nonadmission, preserving the same input and deadline. Cancellation, expiry and
uncertain acceptance retain their distinct outcomes.

Four focused SDK feedback controls now pass, including two actual native cases:
one fresh Find after pre-entry refusal creates exactly one acquisition; persistent
refusal stops after two proposals with no acquisition. This is a test-agent
correction, not product retry policy.

Fresh public dedicated reservation and Bind retain their scoped passes, alongside
seven earlier terminal/recovery controls. The public source locator now passes
two SQLite controls and the affected dedicated-member test. Three rebind controls
and one actual second-G-input control also pass. An unchanged test on old
production fails the expected publication-stamp assertion; corrected source is
restored. Healthy admitted-source rebind preserves publication, while new
admission and native currentness checks remain required.

The earlier qualified follow-on includes seven toolkit-mobkit Rust paths and its
guide. A subsequent checkpoint added 14 toolkit-mobkit paths for retained
context transfer and automatic observation, plus a default-off field in the
existing SDK test host. It excludes the unfinished planning fixture chain,
dev-dependency/lock changes and temporary diagnostics.

The shared-workflow qualification report
retains the owner, SQLite, planning-admission, recovery and exact earlier failures.
Planning admission passes on Core `75a23265` / Meta `263ecad4` (r6). The full
shared setup and automatic context-delivery journey remains unqualified.

The selected development consumer uses the same Core with
[Meta r7](https://github.com/lukacf/meerkat-meta/commit/d66f444ef4c4bc50af6a18ac727791aaf21a4541).
It consumes actual SDK/stdio bootstrap completion under the existing host close
boundary and starts the retained recovery owner. Its audit-fixture compile error
is fixed. Three actual runs then failed at different runtime boundaries:

- In one, A's helper reaches installation, but B has no helper tool event. An
  unlabelled ControllerReadinessUnavailable lacks the session and reason needed
  to attribute the failure.
- In the next, both helpers load their catalog, but B's selected check returns
  operation_authorization_unavailable. A's check succeeds.
- In the latest, both checks pass. A's Install returns generic operation_refused
  before the requested cancellation. Native audit and the exact tool sequence
  correlate this with a pre-entry ReprepareRequired refusal. The scripted agent
  rejects the error; the other eight selected cases remain unentered.

All three preserve their failures and pass strict command cleanup. The latest
refusal does not explain the earlier failures, and r7 remains unadopted. Native
Rust retains the typed cause, but the ordinary model-facing error loses the
refusal kind. The Core lead confirms the gap and owns a native feedback correction
for 0.9.4; its reviewed revision is pending. Private audit remains diagnostic
evidence, not a model input or permission to retry an installation.
The Core owner now locates the latest refusal at prepared dispatch after two
successive publication changes, before policy-wrapped staging. The saved trace
cannot tell whether the final change was still in progress. The proposed wait
only applies in that case; it is not yet proof that the next revision fixes this
failure. The next native run must retain the new typed refusal evidence.

A separate admission-recovery gap remains: retaining an uncertain Original does
not schedule recovery. The reviewed #14 contract
(`e85a4dda`) now fixes the owner obligation. Only a Candidate enrolled in the
source owner's bounded settlement may answer AdmissionPending; unenrolled
candidates, including `retain_unresolved`, remain NotRetained. The owner must
specify its enrollment event, recovery trigger, cadence and step budget, stop and
join settlement and answer no candidate as Pending after close. Every promotion or permanent
withdrawal must announce source-custody change. Promotion requires verified native
admission of the exact input. Withdrawal is terminal across restart, preserves
the uncertain input and binding, prevents resurrection or a fresh duplicate, and
claims no native nonadmission, cancellation or settlement. Unreadable canonical
storage returns Unavailable, never guessed NotRetained or indefinite Pending.
Entered native operations remain separately owned until actual settlement or
close. The exact Core 0.9.4 API revision and Connect's durable-ordering
implementation remained pending at this earlier r7 checkpoint; the r9 adaptation
is described above. R7 startup alone did not close this gap.

The earlier matched r8 development offer was:
[Core `60509a43`](https://github.com/lukacf/meerkat/commit/60509a43e4fb42a0ccccdeca34218190e34db6e1)
and [Meta `c63e643e`](https://github.com/lukacf/meerkat-meta/commit/c63e643ee1bed5e64a33f375e57b85a21b64c8ed).
Its owner-domain API is identical to the reviewed offer, so reuse Connect's
23-path patch and two focused isolation/restore controls. Both controls now pass
in the r8 result described above.
Instances sharing one authority are one owner. Python and TypeScript SDK trees
are unchanged from r7. The only additional identified source adjustment routes
finalization ExternalEvent inputs through native `bind_host_outcome`; ordinary
Prompt inputs retain `bind_prompt`. Do not change the active r6 run or reopen
r8 state with r6: r8 writes state version 7.

The offer includes bootstrap completion and the r6 refusal trace. It explicitly
excludes admission-recovery #14, the host refusal observer, the additional
authorization-audit work and initialized Console-reset fixes. Those remain
upstream follow-up work at that checkpoint; r9 includes #14 and audit changes,
but still excludes the observer and initialized reset. An offered revision is not a passing Connect
consumer or release qualification.

Connect's preparation/store-binding, attempt schema and foreground persistence
are now committed locally as private `6ec07fc5` and public-follow-up `4abe7fcc`.
The production correction accepts the helper's exact canonical source key while
retaining store, scope and realm checks. The 49-case run recorded 47 passes and
two fixture failures; both affected cases now pass after corrections that retain
the child-identity, row-delta, replay, delivery and cleanup assertions. Together
with three unaffected adapter-construction passes, this qualifies 52 distinct
controls across the recorded runs, not one combined passing run. The original
failures remain recorded. The scoped 23-path commits include only this qualified
slice; nothing was pushed or released.

The first input remains recorded before Candidate staging, and exact retry
rebuilds current native authentication. The 15-control handoff run
(`db915f80`) now passes on r6: 14 native controls and one source-guard control.
It retains the exact native receipt through later callback failures, preserves
bounded same-input reprepare and never infers a new admission from a source row.
The six reviewed paths are committed locally as private `717de80b` and
public-follow-up `54a30ebc`; all recorded owners joined and source was restored.

The earlier combined helper and admission execution
(`c2d6fb3d`) compiles successfully and runs all 23 selected Toolkit controls:
20 pass and three fail. The failures are the three new helper cold-recovery
controls, all at the check that the old source, runtime, service, lifetime and
history owners have been released. They reach this check after the explicit
close/join assertions and before reopening. The logs do not identify which owner
survives. The outcome review
(`3121f7d8`) confirms the exact results, restored 800-file source, unchanged lock,
and 25 runner plus 23 child process groups absent. Process cleanup does not
prove the failed Rust-owner release condition. Both Driver controls remain
unlisted and unentered; the gateway, installed product and adoption stay held.
The currentness, original cold/wake/tamper and writer families all pass, together
with eleven retained helper controls.

The preceding recovery and writer-contention execution
(`25271039`) compiles both libraries and runs six Toolkit controls: five pass
and one fails. Cold completion wake, coherent-input tamper refusal, and all three
writer-contention controls pass. The first cold restoration control fails earlier
at original-task completion, before teardown or restoration. Its raw log does not
identify the native outcome; diagnosis remains open. Both Driver controls remain
unentered under the batch's prerequisite rule. The outcome review
(`87f960a0`) confirms these results and all recorded process joins.

A stale source-count assertion also interrupts the runner's final restoration
check. The original failed receipt remains unchanged. A separate
bookkeeping verification
(`5585a478`) verifies all 798 selected files, modes, unchanged native lock and
absent process groups under exclusive ownership. The correction derives the
count from the selected manifest. This qualifies cleanup only.

The preceding focused recovery run
(`ff8468cd`) completes all six selected checks: three pass and three fail.
Compilation and exact listing pass. The stopped helper publication, 33-entry
stopped traversal and cold failed-write controls pass. The two positive cold
restore controls fail on custody-state assertions; the tamper control reaches
native replay refusal and restore Conflict, then fails its no-custody assertion.
The independent contract review
(`faefb4a3`) confirms incorrect fixture expectations. The real issued-access bind
starts at `(false, false)`; restoration preserves its issuer and changes custody
to `(true, false)`. Tamper refusal must preserve the preexisting binding. The
fixture correction
(`280d528e`) is included in the preceding combined8 execution. In that run, the
wake and tamper controls pass; the first cold control fails before reaching
restoration. Both older batches remain failed, with no source adoption.

The independent outcome review
(`77c08279`) verifies all six raw results, eight absent runner groups, six absent
child groups, 797 restored source records, unchanged native lock, and joined
cache and outer transport. External cleanup does not establish assertions skipped
after a test-body failure. The first-round gates used by the two stopped-worker
controls do not explain the earlier live `Denied` and `Bind` observations.

The preceding 46-control run
(`49b05a21`) remains 41 pass and five fail. The earlier 43-control run
(`757f9db8`) remains 26 pass, 16 fail and one timeout; three compiler failures
also remain recorded. These results are separate runs, not one all-pass suite.

The asynchronous writer correction now passes its three focused runtime controls:
ordinary admission waits for the recovery CAS, a cancelled caller leaves the
admission operation owned, and source closure refuses a new Candidate after the
wait. These passes do not qualify the whole recovery composition or attribute the
earlier raw Bind failure.

The helper payload correction
(`2580b607`) has independent source and cleanup review. Its eleven retained
helper controls now pass; the three new cold controls fail before reopening.
Version 5 retains the exact prepared Input in the existing canonical helper row
before Candidate and uses native replay/current-authority checks. Legacy rows
without recoverable bytes stay unavailable. Full cold recovery remains unqualified.

A further admission race has an independently reviewed source correction: legitimate
custody receipt progress can change the canonical revision while the acquisition
host awaits admission. The correction permits only typed, reachable progress of
the same attempt, with all other source facts unchanged. Cancellation and actual
request changes still refuse admission. The source review
(`e111405d`) is now followed by three passing native controls: legitimate custody
progress succeeds, cancellation refuses, and substantive source change refuses.
These results do not attribute the historical Denied failure.

The conditional recovery adoption map remains held. The direct Connect
application driver separately needs its two cleanup controls, a fresh production
gateway and the installed public Hacker News success/resource-corruption pair.
The protected approval process-loss consumer now has reviewed cleanup and
approval-ordering source, but has not run. Source review or a component pass does
not qualify these installed consumers.

An earlier full SDK diagnostic gets through both helpers' catalog/check calls,
independent approvals, cancellation of A while B installs. Preparation completes
and the original-task script continues once, but B's delivery acknowledgment is
absent. The adapter's Timeout variant does not by itself prove an elapsed timeout.
The combined journey remains failed; its exact admission/reconciliation observations
and cleanup are retained in the report. It does not qualify current r7 behavior.

The installed idle-helper reset and the installed-product positive/resource-corruption
negative are qualified on the r6 development graph. The product case installs an
integration, loads it for the original agent and reads public Hacker News. The
corruption case refuses startup before a setup command. These use a scripted model
with real native execution; they do not establish free-form discovery or account
authentication. The exact evidence and earlier failures remain in the qualification
report. Their two verifier files are checkpointed locally as private `4ecfd6e2`
and public `742f281d`.

Reset after an initialized helper now progresses further. The healthy Initial
correction passes six actual native controls
with independent review.
It returns custody without repeating the full resource scan during healthy Initial
closure; exact native settlement and validation before every next use remain.
The controls require refusal of changed resources before another spawn and a real
Offered successor after restoring the bytes.

A fresh production gateway (`5b38d2df`) builds and stages with the complete resource
bundle. The installed reset rerun
(`d364b3f7`) runs both old Initial and replacement Offered transports, reaches the
schema tool in both phases, and records four native Begin/Settle completions and a
clean local drain. Initial finish returns within the same logged millisecond;
this is a callback observation, not an isolated benchmark.

The overall reset case still fails: reset and `/console/identities` report the new
helper, while the final `/console/experience` identity and sidebar report the old
helper. The original agent's identity is unchanged. The strict identity assertion
is retained. Independent outcome review
(`9ccdd210`) separates the observed transport success from the skipped final
historical-source and reader/listener checks. The gateway exits normally, its
provider joins and all outer command owners settle. The Meta lead's real-router
probe narrows the cause: the identity runtime and roster already contain the
successor, but the first `/console/experience` read serves a stale cached snapshot.
The planned 0.9.4 fix joins the current identity binding at request time while
preserving visibility and native liveness ownership. The reviewed development
pair is still pending; Connect retains the strict first-read assertion.

The two healthy runtime files are checkpointed locally as private `78721061` and
public `d54c71c9`, after independent review of the six controls and actual native
joins. The initialized-reset verifier remains unadopted until the identity fix
and complete installed pass. Published 0.9.0 assets remain unchanged.

Connect's `DomainBound::OwnedBy` adoption binds catalog, setup and
report/account/public parent grants to the actual application resource authority,
while retaining exact session/origin children. Distinct application principals
must come from authenticated installer/activation admission; paths or deployment
labels do not establish identity, and copies sharing one authority remain one
security principal. No new Core allocator or global registry is required.

The r8 checkpoint `12c4d2ff` records passing native cross-application isolation
and legacy-parent recovery controls on Core `60509a43` / Meta `c63e643e`.
The former exercises two authorities in one native grant domain, refused foreign
application/session access, empty-domain narrowing and durable reopen. The latter
refuses relabeling an old unrestricted issuance and requires a fresh owned-parent
ID while preserving historical grants and children. The raw r8 execution packet
no longer contains receipt or log files, so those recorded results cannot be
independently rechecked from that packet. The isolation test and exercised parent
constructors remain unchanged; the isolation control now passes on `0d62e43`
with the selected Core/Meta pair, as linked in the current native results above.
These controls do not establish application identity admission or migrate an
existing installation. Python/TypeScript readers preserve native bound objects;
new selections must retain the same native ownership and recovery contract.

The Core owner conditionally confirmed an R1 Find-fenced durable transfer commit
as a disclosure boundary, with recipient-only scope and exact payload/reconciliation
checks. Later waiter effects still require that waiter's authority. Three focused
controls pass for retained commit, cancelled delivery observation and stale absence
observation. The automatic recovery path now also passes a control that retains
slow pre-delivery work across dropped polls while another operation proceeds.
The first automatic SDK case now fails before its context assertions; four remain
unentered. The independent r6 planning-admission pass remains separate. This is
implementation progress, not native end-to-end delivery proof.

All six selected native cancellation controls now have passing observations. The
affected rerun checks Core's pre-factory refusal, stale-session refusal and
successful successor attachment. Initial preparation is reusable only after its
exact native cancellation proves no spawn or a normally closed child. Every new
attempt still validates current registration and full resources. The correction
and its tests are checkpointed locally; installed reset remains unqualified.

The durable-input fixture correction retains exact counts and replay checks, now
reached by the passing r6 planning-admission case. Automatic SDK delivery remains
unqualified.

Cold process recovery is separate from retained in-process task ownership.
Acknowledging historical native acceptance requires exact durable evidence,
without restoring operation authority. The Core owner confirms the missing
public query for the internally qualified input key and proposes an additive
0.9.4 API. That query is now available in the development-only
[Core r6 revision](https://github.com/lukacf/meerkat/commit/75a23265f6125f3ce61560ba8388c076a1caccf1),
paired with [Meta r6](https://github.com/lukacf/meerkat-meta/commit/263ecad41260a0b89c5dd0b8caa251164369be22).
Connect's recovery worker now reconciles exact historical acceptance after its
original host closes and the store reopens. An explicit host-selected private
reader and canonical paging find committed records, including workflows created
after startup. A blocked first read does not starve another selected record;
stop prevents new reads and join retains entered work. The query revives no actor,
grants no custody and authorizes no resubmission.

Three earlier controls and 16 affected checks pass with independent review on
this r6 pair. The first three include the real stdio driver reaching native Ready,
retired-recipient admission, store reopening, paging and stop/join. The remaining
checks cover changed envelopes, cancellation and negative historical evidence.
Full canonical transcript/input equality stays intact. The 29 source and
development-graph paths are checkpointed locally at
`af9dfad568a87ba135c4fbf5fa32a1064b357278`. This is component qualification using
host-selected readers and controlled committed uncertain records. An installed
abrupt-process-loss journey and full SDK continuation remain unqualified. Earlier
fixture failures are retained; the corrected checks read the authoritative runtime
snapshot and distinguish controlled source writes from recovery observation.
The supported in-process completion API remains separate. Restoring current
planning authority additionally needs exact host-selected inputs and current
member ownership. The explicit parent-source implementation is adopted locally
with five passing focused controls. The separate two-path delivery claim/retry
correction is adopted locally after four passing actual-store controls. Exact
proven nonadmission survives a definite pre-claim revision conflict; foreign or
older attempts, ambiguous responses and generic Pending/RetrySafe state cannot
supply that proof. Existing recovery owns eventual durable waiter/effect paging and
stop/join; missing evidence cannot authorize resubmission.
Selected setup-agent and caller source restoration now passes seven native
controls on that same r5 graph. After ordinary native resume, the exact selected
inputs regain native custody without resubmission, history changes or model calls.
The setup agent stays restricted and caller effects are not delegated. Foreign,
missing, changed, cancelled and unselected evidence is refused; parent and fresh
planning controls also pass. The 13 source/test paths include two fixture repairs:
separate parent/caller probe capture and actual bootstrap controller-pin wiring.
Earlier failed runs remain recorded. This result qualifies source custody; the
following result extends it to restored workflow interest. Automatic delivery and
a complete process-restart journey remain separate.

Restored interest and the affected recovery controls now pass all 18 distinct
native tests on r5, with independent review. The 16 selected source/test paths are
checkpointed locally at `ace0501abc90babaa60cb1af504f3e0d248dd5ea`.
The original failure compared outer
client Arc identity across repeated pins, while Meta's default wrapper allocates
a new outer client for each pin. Connect now validates exact association, input,
selection, route facts and current ownership inside the protected callback and
retains its exact returned controller. Runnable custody stays on the canonical
source; the workflow retains only route metadata. The tests cover canonical
paging, replay, cancellation, changed membership, retired-agent refusal and
worker stop/join behavior. Seven focused tests and the remaining eleven use the
same compiled source; cleanup assertions and command/transport ownership pass.
The earlier failed diagnostic remains recorded. No new native API was required;
full shared SDK delivery and process-restart completion remain open. The separate
narrow r6 execution above does not extend these r5 interest-restoration results.

The configured Reading frontend has a source-reviewed sender migration and
SQLite connection cleanup. Nine Reading Python tests pass; the separate egress
and sender run passes 299 tests, including real retained-connection closure,
rollback and receipt-preservation assertions. Its actual CLI consumer now stages
successfully but fails native startup with a cleanup wrapper. The gateway's
protocol-rejection wording also covers channel closure after an SDK timeout;
it does not prove a malformed request. The reviewed two-file repair uses the
existing 900-second installed startup bound, retains a 960-second first-phase
observer and preserves typed primary/cleanup causes. Later phases and cleanup
assertions are unchanged. The affected test rebuild passes; the CLI bytes remain
unchanged. A staging-only failure then occurs before native test entry. The
reviewed rerun reuses the previously successful CLI/resource stage while keeping
current test evidence separate from historical staging provenance. The actual
management case now passes in 230.381 seconds: setup exits while the same host
survives, pause/resume delivers queued input once, disable stops queued work and
a pending reply, and removal preserves the same nonempty service state. Three
native admissions occur. Native shutdown is acknowledged; SDK readers and all
application, provider and outer process owners join without escalation. The CLI
includes Core test-support features, and model/provider responses are controlled
loopback fixtures. Independent outcome review accepts the exact pass and cleanup;
the ten-path source composition is checkpointed locally at `58522d73`. This is
development integration evidence. Live provider delivery, native service-tool
authorization and Browser interaction remain unqualified. Earlier build, staging
and startup failures remain recorded. No push, CI or release change occurred.

The installer target is published Core/Meta 0.9.4, with Linux x86_64 qualification
first and macOS arm64 separately. The manual-only
[Linux candidate workflow](../../.github/workflows/connect-candidate.yml) and its
[input contract](../../release/CONNECT-CANDIDATE.md) are checkpointed after
independent source review. It builds Connect's SDK and starter from the selected
source and separates product execution from original-byte attestation. Required
release inputs remain unbound; no CI run or publication is selected. The earlier
r6 rehearsal remains local development
work: original records and embedded host paths are retained explicitly, without
a neutral-provenance or installed-qualification claim.

The candidate acceptance and publication helpers are now checkpointed locally
in public source at `3eb2a7ab`, with independent source and guide review. Fifty
controlled offline tests pass. Acceptance verifies original attested bytes and
uses the selected Meta reader; publication retains those bytes through upload,
checks the selected public repository/release and refuses conflicting assets.
Partial failure retains observations without claiming a complete inventory.
Real attestation, published Meta reader acceptance and public readback remain
unexecuted. These helpers do not authorize CI or publication.

[Source provenance](../../SOURCE-PROVENANCE.md#r5-development-observations)
records the current results separately from adopted source.

The planning-input source is now checkpointed. Seven focused native controls
pass after correcting a binding path that applied the configured helper's root
check to an independently issued planning grant. They cover admission, refusal
of another valid root, lost-reply reconciliation, cancellation and exact
first-producer selection. Five earlier local authority-custody controls also
pass. The original compile and launch failures remain recorded.

Those seven controls admit a real planning input in an existing session; the
later fresh dedicated-member proof above has its own scope. Protected per-waiter
context delivery, operation authority, shared SDK completion, gateway/reset and
remote restart remain open.

Workflow cancellation now retains cleanup after the cancelling caller returns.
All 12 selected native/store controls pass, covering cancellation racing a queued
join, independent membership, exact close reconciliation, lost local handles and
recovery through the existing worker. Five Python and five TypeScript adapter
controls also pass. The SDKs expose durable cancellation with cleanup pending;
consumers need matching SDK generations because older decoders reject that state.
Three earlier build failures remain recorded. Terminal-result/expiry departure
has the separate focused passes above; active planning-input shutdown and the
complete shared journey remain open.
[Source provenance](../../SOURCE-PROVENANCE.md#shared-preparation-and-source-owner-currentness-work-in-progress)
records the precise scope. Published 0.9.0 assets remain unchanged.

Explicit Node builds now use agent-authored commands and a host-selected toolchain
through the existing installation review, job and readback. Thirty-seven distinct
focused controls pass. A real Linux x64/glibc 2.39 case compiled a native addon with
Node 24.19.0 (ABI 137); a fresh consumer loaded it and returned 42 for 19 + 23.
Original installer output binds the receipt, replay reuses the checked job and
inert readback preserves the installed files. Three earlier fixture failures are
retained. A later matching installed product also completes a supplied source-built
MCP case: the helper checks, installs and reconciles the exact build; protected
software/access decisions lead to one native Original `source_add` call and its
actual result 42. Native completion, two displayed leaf revokes and recorded
physical cleanup passed independent review. This uses a controlled model schedule and synthetic package. General npm
compatibility, autonomous choice, Browser UX and hermeticity remain open. See
[source provenance](../../SOURCE-PROVENANCE.md#installed-source-built-mcp-operation).

Selected Python/Node installer cancellation now gives the pinned installer time to join its direct child before outer escalation. It retains partial output and the original unconfirmed job for observation; exact replay does not start another install. Fourteen new and six retained controls passed. This qualifies the cancellation and direct-child boundary, not npm/compiler/package execution or arbitrary descendant cleanup.

[Source provenance](../../SOURCE-PROVENANCE.md#selected-installer-cancellation) records the validation scope. Released 0.9.0 assets remain unchanged.

Local setup now keeps the protocol child in its original preparation owner
across errors and offers a move-owned asynchronous guard. Existing CLI calls
remain compatible; borrowed guard Drop requests cancellation while retaining the
handle needed for an explicit join. Seven focused controls pass on the current
Core/Meta graph, including cancellation, malformed replies, changed resources and
that same-child join. The initial fixture failure remains recorded.

This qualifies synthetic process custody. Native adapter, gateway/reset and
shared SDK acceptance remain separate, as does CLI recovery after a persistent
OS wait error. [Source provenance](../../SOURCE-PROVENANCE.md#retained-local-setup-process)
records the exact checkpoint and limits. Released 0.9.0 assets are unchanged.

An installed controlled Console API case now completes repeated acquisition in
one Original session. It first installs and uses Hacker News, requests Wikipedia
through the same surviving helper, then answers using both services. The earlier
Hacker News job is reused without reinstalling it; the new continuation receives
fresh grants for the complete service set. All three Original runs and both
helper runs complete. Exactly five displayed service/catalog grants are revoked,
login retirement is certain and native/process owners join.

This case uses a controlled model schedule with real native execution and public
service calls. It qualifies installed continuation and reuse, not autonomous
integration choice, Browser interaction, helper reset, process restart,
authenticated batches or concurrent shared setup. No controller-recovery notice
occurred, so the new recovery validator has source/control evidence only.
Separately issued setup children and persistent policy are outside the five
displayed revokes. Published 0.9.0 assets and requirement statuses are unchanged.

[Source provenance](../../SOURCE-PROVENANCE.md#installed-repeat-acquisition) records exact versions and evidence.

The fresh V40 journey now completes agent-led discovery of an outside-catalog
integration, protected installation/access and the original business answer
through the controlled Console API. The same Original reported overdue invoices
rising from EUR 2,900 to EUR 4,600, a EUR 1,700 increase led by Linden Works at
EUR 1,300. Independent recomputation confirms all figures and strict cutoff
handling. The question was submitted once, without operator follow-ups, direct
business calls or manual wakes.

V40 used exact public `1779df8f` with ordinary Forge generation and no overlays
or recovery selection. The task grant is unavailable after protected revocation;
logout, local session removal, normal Stop and physical cleanup passed independent
review. Owned sessions/forwards are joined and ledger/sidecar identities unchanged.
This qualifies the controlled API journey, not Browser/UI completion, installed
recovery or independent runtime write prevention. Published Connect 0.9.0 binaries
are unchanged. [Source provenance](../../SOURCE-PROVENANCE.md#unreleased-follow-up-source)
retains exact source identities, earlier failures and scoped validation.

Development source now supports 2-8 public MCP services in one acquisition.
The agent chooses the checked installation jobs and operations. One protected
access review covers the exact set; each service has separate grants and must
become natively usable before one continuation answers the original question.
Software installation keeps its existing reviews. Separately reviewed agent tool
permissions last until superseded and do not replace exact task grants. See the
[Console guide](../site/guides/request-an-integration.mdx#use-several-public-services-in-one-console-task-development).

Twenty-nine distinct Rust controls pass (21 focused and eight native). Restoration
coverage is settled historical source custody, not process restart or a new
post-restoration operation. The earlier fixture-order and public-refusal
classification failures remain recorded. The panel passes 48 JavaScript controls,
including visible per-service operations, deadlines,
remote endpoints and policy scope. A fresh controlled Console API case on public
`4fb67218` selected and installed both services after correcting the initial
provider route through normal setup. It made no receipt reconciliation call;
three public requests returned generic unavailability before access review.
Current library metadata was also absent because help was read before checking.
No service grant, business call or final answer resulted. Logout, normal Stop and
exact physical cleanup are independently closed. The feedback repair now passes
four focused controls; restoring only the old generic mapping makes the unchanged
correction regression fail at runtime. Agents can see the existing reconciliation
and fresh-help requirements.

V43 now completes the ordinary two-service case on public `be0df64c`. One business
request names Hacker News and English Wikipedia, without preselecting integration
IDs, profiles or strategies. The agent chooses and reconciles two installation
jobs, obtains one exact complete-set access decision, and the same Original
automatically uses both services and answers. Three exact task-grant revocations,
logout, normal Stop and physical owner cleanup pass independent review. The
separately reviewed policy successor remains until superseded.

The four-proof join with V40 and the native authority controls closes SETUP-002
for the declared development profiles. Story 2's Wikipedia relevance remains
explicitly uncertain; technically successful tools do not guarantee semantic
perfection. V41 remains failed before access, and V42 remains a failed, uninstalled
Build whose generated callback contract would have required new policy grants.
The policy was not widened. V43 uses a normal source-preserving refresh of the
accepted neutral application, with a fresh installation and native namespace.
Browser interaction, authenticated batches and warm later acquisition remain
separate. Other requirement statuses and released 0.9.0 artifacts are unchanged.

Development warm reacquisition now passes 28 distinct Rust and 49 JavaScript
controls. A host-selected configured capability lets the continued task use the
same live helper for a later public acquisition. The new review covers fresh
setup and service grants plus metadata access for both named agents across both
named sessions. The helper can select a current earlier job together with a new
job, but all grants for the next continued task are fresh. Native Ready and
independent-target checks, stale policy and recovery refusals, and checked-job
reuse pass; earlier R1-R5 fixture failures remain retained. All command owners
joined. See [host composition](../site/guides/host-composition.mdx#acquire-another-public-service-through-the-same-live-helper-development)
and [source provenance](../../SOURCE-PROVENANCE.md#warm-public-reacquisition).
Helper reset, Browser behavior and performance remain unqualified.
Requirement statuses and released artifacts are unchanged.

The first strict installed check on public `63c8d79a` failed after software
installation and receipt reconciliation. Its valid legacy profile gives the
Original and helper different audiences. Warm selection incorrectly compared
helper targets with the Original audience and returned a policy refusal before
public access review. The corruption case was not entered, and no Forge app or
business question was started. Normal native shutdown and physical cleanup
completed. This is a Connect compatibility regression, not a missing policy grant;
the failed execution remains retained.

The scoped correction validates targets against the actual helper audience,
then preserves legacy fallback for initially ineligible configurations. Seven
affected Rust controls pass, including actual cross-audience public preparation,
captured-scope refusal, partial recovery and both native repeat-acquisition cases.
Six are existing controls; the cross-audience control is new. Restoring only
the old production comparison compiles, then fails that new test at actual public
preparation with Conflict. No permissions or installed assertions were relaxed.

The fresh production gateway from public `3ecf0d5a` now passes the unchanged
strict installed success and resource-corruption pair. The success case reaches
the actual continued agent's durable consumed/completed receipt; corruption
refuses before negotiated entry. Native shutdown, readers, providers, children,
process groups and the build lock all settle. Independent review verifies the
exact source and artifact bindings. The gateway build retains 65 warning headers.
This closes the installed compatibility regression. The later R9 case
separately qualifies installed repeat acquisition.

V44 then completes an initial setup-enabled two-service batch through the
controlled Console API on that corrected production source. The host's actual
catalog/audience profile is eligible, but both services are selected up front,
so V44 alone does not establish B-to-C or retained-X reuse. Six tool errors recover
automatically before the Original answer. Exactly three displayed service/catalog
grants are revoked and physical cleanup joins all owners. Those revokes do not
cover separately issued setup children or reverse the persistent policy. See
[source provenance](../../SOURCE-PROVENANCE.md#v44-initial-setup-enabled-batch)
for the exact final evidence and answer limits. No additional requirement closes.

Concurrent shared setup remains in development and is not covered by this
installed repeat-acquisition result.

- **Bounded setup-job observation:** Development `setup_job` accepts optional
  integer `wait_ms` from 0 to 10000; omitted or zero keeps the immediate response.
  Cancelling the observation leaves installation running, and explicit stop stays
  separate. Eight Python and four Rust controls pass for exact-job custody and
  observation lifetime. V44 uses seven actual `wait_ms: 10000` requests during
  its completed initial batch. This establishes usage without a comparable
  latency claim, shared-peer isolation or readiness.
  See [source provenance](../../SOURCE-PROVENANCE.md#bounded-setup-job-observation).
- **External Agent Plugins consumption:** A format-adapted Microsoft
  `microsoft-docs` 0.3.1 pack now exports on Linux and runs through unchanged
  OpenHands SDK 1.49.2 on macOS. After staging removal, all eight original files
  and two canonical JSON values match; the actual client loads three skills and
  fetches a substantive 2,089-byte Azure Blob Storage overview with one tool call.
  Independent outcome review and natural local cleanup pass. Both earlier UMT
  initialization failures remain failed with zero tool calls. See the
  [portability example](../site/reference/portability.mdx#qualified-external-client-example)
  for the explicit format adaptation and limits. An independent four-proof audit
  now closes PKG-007 for the declared canonical Agent Plugins 1.0 skill/MCP
  import/export subset, including required-contract and malformed-entry refusals.
  D3 and PKG-006 remain open.
- **Retained approval recovery:** Development now includes one bounded inventory
  of existing parent/access/source records and reviewed exact cleanup through the
  existing native owner. Explicit Console operator policy requires the retained
  human plus current admin/member/login authority; independent local OS-operator
  SDK selection supports the inherited terminal's `/recover` command. Published
  sources and other approvals are preserved, and absent-parent or partial effects
  remain unresolved. Policy observation stays separate. The repaired UI suite
  passes 40 controls; a reviewed selector map joins 26 distinct passing Rust
  controls across native/SQLite, SDK, terminal and actual-verifier behavior. The fixture uses native
  identity restoration after actual HTTP diagnostics confirmed a stale alias.
  Inherited PTY adoption uses `O_NOCTTY`, with an isolated session-leader control.
  Earlier failed runs remain recorded. See the [host guide](../site/guides/host-composition.mdx#recover-unpublished-service-approvals)
  for explicit selection and use. A separate ordinary development Rust SDK consumer
  now reopens the same installed host after fixture preparation and owner closure.
  One public API recovery action revokes unpublished B's parent/child, preserving
  published A, canonical source rows and native input history. Exact journal
  readback and natural cleanup pass independent review. The first producer
  duplicate-admission failure remains failed; its repair preserves the unchanged
  consumer, authority and deadline. This is pre-agent SDK consumption, not a
  released distribution, stock gateway, terminal, Ready or real-crash proof.
  Browser login remains open; V40 excludes these changes. No standalone realm
  recovery command or automatic startup revoke is added. Forge/MobkitInstaller's optional
  selection and cold-controller pin correction passes 38 reviewed controls but
  has no installed adoption claim and changes no generator default.
- **Approval failure cleanup:** After an approval worker joins with an error,
  the source owner reconciles the exact result and attempts unpublished-grant
  revocation. Native refusal or uncertainty remains unresolved. An already
  published exact source and another approval's child are preserved; late
  publication rechecks grant eligibility. Seven focused native/SQLite tests pass,
  and the prior approval body fails the missing-revocations assertion with the
  same fixtures. Independent source and execution review passed. This covers
  returned errors, not process loss, cold recovery or installed Console behavior.
  Login uncertainty remains retained and policy is not rolled back. Historical
  failed-case cleanup remains independently verified; V40
  completes the separate original business answer.
- **Review expiry:** Console and terminal reviews continue polling their existing
  deadline while displayed. Focused checks and an actual expired Console review
  confirm settlement without extending approval lifetimes or changing authority.
- **Bounded status observations:** A stalled status read now expires after 30
  seconds across headers and body, aborting only that read and restoring explicit
  First-page recovery. Late results cannot replace the current view. All 45
  JavaScript controls pass; three prior-source regressions fail as expected. A
  real Node loopback check confirms both held requests close before teardown and
  a fresh read succeeds. Protected actions and their polling fence are unchanged.
  This is component/Node transport evidence, not Browser interaction, native
  effect completion or full UX-002 qualification.
- **Protected action ordering:** Panel reads pause while this document has a
  protected action pending, then refresh current state after settlement. The
  access endpoint's stale-offer 409 now reports definite nonentry without retrying
  the mutation. Two prior-source regressions and 36 controlled JavaScript checks
  pass with independent review. Other tabs can still replace an offer; this adds
  no cross-tab lock or claim that observation timeout cancels an action. V40
  includes these bytes and completes protected API actions. The official hidden in-app Browser was
  unavailable before any tab, read or UI action in a separate synthetic-transport
  fixture. The fixture stopped naturally with joined owners; no mouse or
  substitute driver was used. Actual Browser interaction remains unqualified.
- **Operation feedback:** Public and declared-secret-slot requests identify exact
  permitted raw tool names when a current checked selection receives invalid
  names. A controlled Console test now exercises the invalid-name feedback before
  credential review.
- **Usable permission proposals:** The Console fallback for a waiting original
  task rejects missing catalog scope or selected-tool coverage before protected
  review. Approval still covers the complete successor member policy until
  superseded and eligible catalog metadata throughout the original conversation.
  This explicit selection supplies the new task's catalog authority even when
  ordinary policy already permits the tools. Grants remain separately scoped and
  finite; trusted application delegation and lower-level APIs are unchanged.
- **Explicit helper catalog successors:** A new host opt-in retains a separate
  catalog-read-only parent and derives a child for the actual native helper
  session. Omitted scope retains the existing captured-session behavior. Existing
  grants and retained work keep their original bounds and targets. Source review
  passed, and 57 distinct focused controls now pass, including an actual native
  helper reset. The reset control contrasts legacy authorization refusal with
  authorized Search and Load in explicit successor mode. The inline-only test
  operation still returns `not_deferred_eligible`; this proves catalog
  authorization, not successful deferred loading. Host-adapter source checks
  passed separately, without installed successor qualification. See the
  [host guide](../site/guides/host-composition.mdx#helper-catalog-access-after-reset).
- **Target-bound npm dependencies:** Supported optional dependency graphs retain
  the full lock and archives while selecting compatible packages for the exact
  target. A pinned Linux `chokidar-cli@3.0.0` installation, performed offline with
  lifecycle scripts disabled, produced a fresh file event. Optional peers and
  arbitrary native payloads remain out of scope; this run qualifies Linux only.
- **Portable executables:** Explicitly included Unix scripts retain executable
  intent through export without copying unsafe mode bits or widening the caller's
  umask. Inspection detects permission drift even when bytes are unchanged and
  keeps historical unrecorded mode observations distinct. Eighteen focused
  controls pass, including use through a generic stdio client after source
  removal. Strict Clippy remains limited by two documented existing lint categories;
  scoped Clippy passes with command-only exceptions. This does not qualify
  third-party packages, Windows permissions or complete Agent Plugins support.
- **Discoverable plugin guidance:** The existing inspection tool can list one
  bounded package directory, letting an agent find unfamiliar skill and reference
  names before choosing explicit text reads. Inventory carries no source custody
  or execution authority. Eleven macOS Rust controls, 34 focused Python controls
  and one real MCP/canonical-reader recovery test pass. The latter uses a scripted
  client and inert installer, and recovers the same selected text after staging
  removal. Independent review passed. Linux's non-UTF8 fixture control remains
  unexecuted; scoped Clippy retains one existing lint exception. Installed-host
  and full package interoperability qualification remain separate.
- **Fresh recovery proposals:** `find_and_install` guidance permits a new reviewed
  proposal within the original request and current authority after a failed
  attempt. It does not require another chat authorization solely because setup
  failed, permit approval replay or force repeated attempts. Software and access
  reviews remain required. Retained failure payloads stay unchanged for delivery
  reconciliation. A fresh Console run demonstrated an autonomous new proposal
  with this guidance delivered, without establishing wording-only causality.
- **Retained failure feedback:** The existing source cancellation, replay and
  continuation path retains a bounded startup phase, fixed failure class and
  optional closed-process exit. The first committed observation remains immutable;
  delivery checks the exact source before resuming the original agent. Stage 2
  now reads optional metadata from the exact native attempt under the remaining
  readiness deadline. Missing or late metadata preserves a timely failure and
  legacy bytes. Five distinct controls pass, combining four retained scripted
  results with one corrected actual native initialize-failure control. The same
  actual native fixture reproduces missing capture on old production source;
  independent review confirmed the outcomes and joined cleanup. The earlier 12
  synthetic controls and valid old-reader regression remain recorded, along with
  fixture failures and scoped lint/format limitations. Raw stderr and arbitrary
  server messages are excluded. The latest controlled Console case now delivers
  an actual initialize/closed/code1 observation to the original agent before its
  fresh reviewed proposal. That installed observation does not establish
  historical failure causation or successful business recovery.
- **Startup recovery guidance:** After a known startup or attachment failure, guidance
  asks the agent to inspect the failed closure's relevant imports and startup API
  calls, then inspect those same edges in the exact acquired replacement before
  proposing installation. Both the standalone prompt and existing inspection-tool
  description carry it, preserving the integrated helper's host profile. Source
  review and AST comparison confirm unchanged schema, dispatch and authority. No
  package pins or capabilities are added. V40 consumes the refreshed package and
  completes original-task use; it does not isolate this guidance as the sole cause.
- **Approved-service continuation context:** A committed correction adds a
  checked attachment observation, approved operations and effective expiry to
  the existing native injected-context slot. The original reviewed question is
  unchanged; native authorization and exact-input replay keep their existing owners.
  Four focused native controls and independent source/execution review pass.
  The prior-projection comparison retains additive fixture-compatibility metadata
  and fails after native admission at missing-context assertions. The initial
  missing-import compile failure remains recorded. Restoration covers source
  custody, not full process restart. The observation promises neither future
  readiness nor filesystem isolation. The corrected package now completes a
  controlled installed SDK public read with exact native input completion and
  rejects resource corruption before model entry. Its first installed run failed
  an obsolete fixture predicate; a strict fixture-only repair reran the same
  product bytes successfully. Both cases exited naturally with joined cleanup.
  An untagged Meta transport-reader diagnostic remains recorded. V39 then failed
  both attachments before the context path. V40 now completes automatic
  same-Original business use; Browser UX remains unqualified.
- **Research across setup helpers:** A fresh normal helper can inspect an
  explicitly host-selected prior acquisition after exact matched reconciliation.
  Its historical status stays unconfirmed, and current owner, profile, runtime
  and retained-file checks remain required. One old-source regression and 34
  passing local controls include actual MCP inspection with fixture archives.
  This does not replay installation or qualify automatic cross-helper import,
  installed-host adoption or the complete business recovery journey. See the
  [inspection guide](../../sdk/consumer-setup/ACQUIRED-INSPECTION.md#custody-and-limits).
- **Retained guidance export:** The saved-recipe CLI exports explicitly selected
  public skill files after staging removal. It checks exact ownership, identity,
  source and paths, preserves plugin namespaces and requirements, and adds no MCP
  configuration, activation or execute bits. The corrected final source compiled
  and passed 24 focused macOS controls, including retained SQLite source,
  fresh-process export/readback and independent file-reader use. Three old-source
  runtime failures remain retained. Review rejected a duplicate-import formatting
  candidate; the corrected source was recompiled and retested. Changed-region
  formatting and scoped Clippy pass with one existing lint exception. Installed
  adoption and full package interoperability remain separate work. See the
  [operator guide](../../crates/toolkit-package/LOCAL-RECIPES.md#export-selected-retained-guidance).
- **Independent application roles:** A persistent Reading-host test confirms that
  the same exported service and store remain usable after frontend removal and
  management-CLI exit. Existing admission, reply and shutdown checks also pass.
  This establishes application service survival, not native service-grant or
  model-dispatch qualification, live frontend adoption or restart coverage.
- **Closed frontend senders:** New local deliveries stop before provider cleanup
  is awaited. Per-sender lifecycle serialization preserves independent senders,
  replacement attachments, durable owner selection, pending output and receipts
  from calls already in progress. Independent review and 198 affected tests pass,
  including a two-authority activation race and failure/cancellation cases. A
  separate, independently reviewed installed Python SDK consumer now qualifies
  two inert senders: A detaches before cleanup, entered work keeps its receipt,
  pending work spends no attempts, and B remains usable. The same nonempty Reading
  service fixture, store, export and database bytes stay unchanged; all owned
  tasks and the process are joined. Two fixture-only preparation refusals remain
  recorded. Actual host or Reading frontend adoption and live provider
  delivery remain pending.
- **Supplied schema evidence:** The optional metadata inspector computes a digest
  of exact supplied JSON bytes and rejects malformed or oversized input within
  the caller's existing permission ceiling. Its 20 Python checks and one native
  permission check pass. The matching Linux package also passes the installed
  success/corruption pair, including an actual controlled helper invocation after
  native catalog loading. A fresh free-form helper also used it successfully.
  This qualifies supplied-byte hashing, not runtime observation, schema
  validation, readiness or completed business use.

The previously assembled development package passed gateway assembly, controlled
installed success/corruption checks and fresh Console startup. Its focused
permission checks include native catalog loading and a mapped inert invocation.
Those results predate the new helper catalog successor source and do not qualify
that mode or a completed real-model business journey.

A controlled Console API test completed protected credential submission,
attachment and one authenticated GitHub account read by the same original agent.
Its task grant was revoked and application processes were closed. The new
`credentials status` and `credentials remove` commands passed 20 focused controls
and actual same-store `Stored` -> protected removal -> `NotStored` checks using
the normally emitted development CLI. Removal preserved service configuration
and grants without a model, setup helper or another credential fetch or account
read. This resolves local credential cleanup, not provider-token revocation or
full browser-UX and free-form discovery qualification.

Earlier operator-invalid runs, reset/catalog refusal, V37 attachment failures,
V38's missing-context answer failure and V39's initialize failures remain failed
and closed in [source provenance](../../SOURCE-PROVENANCE.md). V39's later exact
unpublished-grant cleanup is distinct from product recovery. V40 does not
reclassify those cases or prove their initialization causes. Paired native
setup-launch ownership and explicitly selected installed recovery remain separate
owner-coordinated work.

D3 also retains bounded Linux prebuilt native-addon load/read/write-refusal.
V40 adds real native-addon MCP acquisition, installation and original-task use:
`@database-mcp/sqlite` 0.4.2 / `better-sqlite3` 12.11.1 on Linux x64/glibc 2.39,
Node 24.19.0 ABI 137. Independent review matched the 135-file acquisition inventory, exact installed addon, primary entrypoint
and runtime to the recorded adapter calls and answer, without a new business probe.
Portable import with host-mediated same-session continuation and declared
Compose owner-conversion evidence on macOS and Linux also remain qualified. These are narrower than
general native compilation or accepted native application-profile migration.
The Microsoft/OpenHands case above adds one real, format-adapted Agent Plugins
client operation. PKG-007 is complete for the declared canonical skill/MCP
subset; additional client/format profiles remain unqualified. The saved-guidance export above
already closes the earlier retained-file export gap; no repeat of that proof is
needed. PKG-006 and the remaining D3 breadth stay open.

</details>

## Released 0.9.0 limits

| Area | Practical limit in 0.9.0 |
| --- | --- |
| Frontends and custom extensions | Source components and role contracts remain available for explicit host adoption. There is no general Telegram or Slack frontend migration promise, and no migration promise for any existing host. A persistent application must own identity, audience, routing and delivery. |
| Existing application upgrades | Application host support is scoped to fresh composition. Updating an existing application's selected gateway requires the host's replacement-selection wiring; automatic upgrade across arbitrary hosts remains follow-up work. If Fresh startup fails before producing completed setup and its finalizer, same-installation Restore/update has no admissible completion or replacement envelope. Preserve that failed installation and use the normal fresh path with an empty sibling target. |
| Forge-generated Console | The preserved-design refresh failed acceptance before Install and the original question. Forge must honor an explicit no-account-request brief in the initial helper policy, and omit unsupported optional analyst edges and delegation text while keeping peer tools denied. Complete fresh real-model task acceptance remains unproven. |
| Predecessor application stores | Adoption of an existing nonempty MobKit 0.8 store under authoritative Meta 0.9 startup is deferred when its composition manifest has only the legacy `created_by_mobkit` writer field. Fresh Meta 0.9 Console installations and Core realm/session CLI use are separate paths. |
| Provider onboarding | Some providers require an existing application, administrator consent or native credential. Universal registration and every remote/mobile/headless auth transport are not included. |
| Package formats and updates | Supported profiles and dependency closures are explicit. Arbitrary native addons, all MCPB/platform combinations and seamless migration or rollback are not promised. |
| Provider operations | Support covers declared operations and account profiles. It is not certification of every upstream API, MCP feature or end-to-end business workflow. |
| Remote deployment and shared workflows | Cloud/GitOps rollout, every durable store and broad unattended multi-user orchestration remain application-owned adoption work. |
| Live devices and modalities | Robot, voice, media and physical behavior need separate application and hardware acceptance. |
| Management and accessibility | Additional management parity, client modes, accessibility coverage and performance measurements require their own evidence. |
| Startup diagnostics | Released native admission exposes a generic composition refusal. The unreleased correction described above adds fixed failure stages and typed categories while withholding private inner error content. |

These limits do not permit broader grants, guessed accounts, hidden service
retries or claims that a configured frontend is running. Unsupported combinations
must report their missing requirement before performing the privileged effect.
