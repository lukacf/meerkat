# ADR-001 implementation checkpoint

## Durable decisions, 2026-10-30

0.9.8 adds durable decisions beside the in-run consent kind, which is
unchanged: `ApprovalLifecycleMachine`, `approvals.json` and their audits keep
their bytes. One record-scoped `DurableDecisionMachine` per decision owns its
hold and sealed binding, the presentation of its subject, the attested human
or reviewer decision bound to one subject digest, the execution permit and the
executed fact, expiry, withdrawal and the owed requester notice. A permit is
authorized only before the deadline, and an approved decision with a permit
outstanding waits for its settlement instead of expiring. The owner is
`meerkat_runtime::decision::DurableDecisionService`, stateless over its own
store (`approval_decisions.sqlite3` beside `approvals.json`, created by the
first hold, read lazily). Core adds no wire. The hand-written TLC audit
`specs/machines/durable_decision/durable_decision_audit.{tla,sh}` runs in shard
`audits-j`. Open: the operation owners' commits under the permit, the host's
decision surfaces and notice delivery, and routing the decisions file through
a host's own store.

## Outcome summary, 2026-10-08 at 03:27 UTC

The 0.9 coordinator leads delivery. This summary separates completed outcomes
from remaining acceptance; the dated entries below retain their exact evidence
and limitations. The full [acceptance plan](acceptance-plan.md), interpreted
through the [local default](../adr-001-local-governed-default.md) and
[review/consent amendment](../adr-001-model-review-and-consent.md), remains the
completion contract.

| Area | Evidence and status | Remaining work |
| --- | --- | --- |
| Published native checkpoint | PR1730 merged as `8ca50a1a459eef6a79fd8bc33cd7d067b9b1f9bf`; its tested tree, required CI gate and 11-scenario Live gate are recorded below. | This checkpoint does not establish full surface, recovery or performance acceptance. |
| Next integration candidate | Clean `4d561ef06` passes all six commands in its selected qualification: seven-package all-target/all-feature check, 18 restart test entries, four confinement contracts, selected Clippy and schema/SDK codegen freshness. A separate SDK log records 524 passes and one network-test skip, including both refusal controls; that log has no exact source header. | Selected qualification completes at 03:26:30 UTC. The prior required Linux lane failed all 28 attempted positives; supported-host acceptance remains open. The current evidence does not establish a whole repository gate, successful push, PR CI result or publication. |
| Native overhead | Clean H5c `9bdcf7e55` passes selected release Clippy. The completed H4/H5 depth-1 FreshAdmission screen preserves workload/counters across 160 rows. Added mean falls from 1.381721 ms to 1.111662 ms, a 19.55% reduction; H5 governed mean is 4.285891 ms against 3.174229 ms trusted. | H5's observed +35.021% remains above the <=10% target. The original six-cell representative matrix and separate dispatch p99 <1 ms measurement remain unaccepted. Admission through the first model request is about 73% of this screen's remaining gap. H6 benefit remains unmeasured. |
| Sandbox and hooks | The scoped factory, run-ID and notice-bound corrections have the evidence recorded below. | Protected native hook audit Q3 and the required supported-Linux positive remain open under their existing owners, outside the finite .52 cut. |
| Review and consent | The Codex source comparison and accepted model-review direction are recorded in the amendment. The three-document handoff from 01:18:38 UTC is byte-identically integrated at `3666b44c7bd630070e1d90b5f2f317821412015c`. | Current selected action account and required mandate/policy explanations still need full coverage from actual owners. Direction and source review do not substitute for adapter acceptance. |
| Voice WorkGraph recovery | Three parallel source reviews of `cdcf70aa13c159f4bc09b783220afeb9709ec516` confirm replacement-claim retry retention and identify missing durable item/workerless custody, dropped retry inventory and unconfirmed cleanup loss. | These are open under the existing owner, outside .52. Same-runtime coordinator tests do not establish cold reconstruction. A separate released-result invariant conflict predates this delta. |
| Full coverage | The local, inexpensive default and operation-local feedback remain required across the full system. The R2 source-review sequence through `f9a63f8dc` is clear: canonical pending-removal restore, the version barrier, null-map rejection, migration dirty tracking and the prepared-cell regression are corrected. | Generation and executed qualification remain pending under the existing author and scheduled lane. Remaining operation families, surfaces, recovery boundaries and consumer acceptance retain their own completion evidence. An interim release or one passing scenario closes only its stated scope. |

No new acceptance gate is introduced by this summary. Source-only reviews and
prepared tests remain distinct from executed passes. Performance work stays in
the existing scheduled build and measurement lane; current publication scope
does not redefine final 0.9 completion.

### Integration qualification result, 2026-10-08 at 01:03 UTC

The existing normal-push attempt tested tree
`afeff1855baf6b723d48e9c9f83fbf53c5ebd208`.
Its complete raw log is
(operator-retained path) on GCP, SHA-256
`6d1d056b3f4e5115b119904d639d49f98e1fc1ca1dbffb09e1ea757f6ce8e129`.
The attempt ran from 23:02:06 UTC on October 7 to 01:03:11 UTC on October 8,
2h 1m 5s in total. The workspace build hit its 900-second timeout; the existing
automatic retry then completed compilation in 7m 10s. Reusable compiled output
means the first 900 seconds cannot all be classified as wasted work.

The unit summary records 13,417 passed, 25 skipped and 322.381s. Integration
records 3,293 passed, 31 failed, 90 skipped and 705.320s. Its failures are:

- Eight cold-restart cases cannot materialize a session because another runtime
  owner still serves it.
- Twenty-one sandbox cases encounter `BackendUnavailable` while requiring an
  available backend. This result does not establish a confinement escape or a
  supported Linux positive pass.
- The confinement terminal-class ordinal contract reports actual 11 versus
  expected 9.
- The metadata construction contract finds `RuntimeTurnMetadata` constructed
  in `meerkat-runtime/src/input.rs:462` outside its permitted source site.

The cold-restart failures involve real retention cycles already present at
baseline `9dcccd93fb3289fff78cb61df3ac77585c308c90`; the earlier fixture-only
classification was too strong. The existing baseline run now reproduces the
same eight failures: 8/16 passed, exit 100, 5.251s test time and 3m29s wall.
Its raw `coldrestart-9dcc-attempt1.log` has SHA-256
`57277a525a4d51097fcb0d96861cd608fbbca02dec3470bdbb02ce3ff3bed39e`.
The header records the baseline HEAD and an empty unstaged diff, not a complete
clean-tree identity. Together with immutable ownership comparison, this
supports the existing owner's fixture correction without a production Weak
retrofit. The broader retention cycles remain unrepaired.

Committed correction `25dd3890e61fd328345f7b83150b36f99c80b64d` is source-clear
for its two test files. Local history/compaction lifetimes await canonical
registration teardown and service shutdown. JSONL uses distinct writer and
reader processes and retains its exact queued-input and recovery assertions.
These cover graceful local reopen and process exit respectively, not an abrupt
kill during a write. The existing correction run against 25dd plus staged
fingerprint `fd98ec80d5ac` now passes the all-target/all-feature workspace check
and all 17 selected restart tests, with exit 0. The restart test time is 9.195s
after 3m28s compilation; its count includes two child-entry helper tests. Both
actual cross-process parent cases passed. The metadata constructor target also
passes its one test, exit 0. These results were read from GCP
(operator-retained path); the partial 01:44:15 UTC
snapshot has SHA-256
`cd4ec118c6be96ed281d22c665cd963af0963a8da1379200f28546f37e9f4975`.
That snapshot has no final END marker. It is executed correction evidence,
not a clean-head qualification or completed whole-gate result.

The subsequent 02:00:35 UTC snapshot of that same run also records 187 MCP
passes: 182 library cases and five `session_expiry_replay` cases, exit 0,
4.928s test time after 2m 54s compilation. Its six skips correspond to five
eligible-Linux confinement positives and the explicitly ignored real-subprocess
timeout case. This does not establish supported Linux confinement. The mob
`member_upcall`/`upcall` selection passes all 31 cases, exit 0, with 2,554
excluded; those tests take 0.189s after 15m 06s compilation. Their typed outcome,
settlement, fencing and replay coverage does not qualify the whole mob surface.
This partial raw snapshot has SHA-256
`5f9d87aa4cbf7ec8a71a67489264ec85563b101749eb26c69ab8a8abe4dfee53`.
The final log ends at 02:14:08 UTC after 52m17s total wall time. Selected
all-target/all-feature Clippy passes, exit 0, after 14m22s compilation. The
subsequent `make test-sdk-suites` stops in Python: 788 passed, one failed and
13 skipped in 1.46s, command exit 2. The failing
`test_generated_mcp_setup_fields_keep_closed_optional_native_reason` finds
`Any` where the generated `confinement_refusal` field must retain the closed
optional native reason. TypeScript is not reached by that command. Together
with the earlier ordinal failure, this is a completed red correction batch.
Its complete 91,366-byte raw log has SHA-256
`69fdf9c7498f27eeac3b08781ad7da9f1b9ac8e16d4c2ae18d8d877ba1236d8b`
and is retained as `integration-25dd-completed-20261008.log` in the root's
October 7 review artifact prefix. The original header remains 25dd plus staged
`fd98ec80d5ac`; later source fixes do not change this executed identity.

The earlier test-only `23c6b93f430ae9ef5906be0a894a896acb3bf370` correction was
not accepted: it masked the shifted published PR1730 prefix. Main
`6d052fac8679a338c6439f6f7b731e15188939ea` already appended OutcomeUncertain=15.
The selected contract preserves all of 0-15, then appends ReviewUnsatisfied=16,
ReviewUnavailable=17 and HookLaunchRefused=18. This supersedes the earlier
illustrative order for unpublished additions. The 25dd staged assertion still
expected OutcomeUncertain=17 with actual 18 and failed, three passed/one failed,
exit 100. Successor `bacbec67f`, retained in `5aaf15074`, now changes the actual
enum and its generated projections to the selected mapping. That correction
is source-clear and all four contract cases pass in the new run. No changed
JSON names or corrupted stored binary record is claimed. The committed
continuation constructor correction
`83a38634ec700de6f2490ad2e6825d389494c46a` is behavior-preserving and source-clear:
it moves the seed into the existing runtime-loop seam while preserving defaults
and later admission checks. Existing owners retain execution and integration.

The original integration lane returned 100, and its push ended with exit 1. The later
HeadCanonical and e2e-fast commands are sequential and were not reached.
Source diagnosis and any subsequent correction remain separate from this
executed result; no new runner, approval gate or publication authority is added.

At successor `3894912c46a3bc29403b1d37b84120ddef6459ce`, a focused SDK source
review confirms a separate direct parse/rewrite gap. Public
`MeerkatClient.parseSessionMessage` accepts system-notice raw blocks without
their shared validator, while rewrite serialization trusts those accepted
bytes. The ruling is to reuse the existing notice/refusal validation in that
public path, preserving exact valid/null/absent round trips and streaming
`malformed_event` retention with permitted sibling continuation. Moving only
the failing test to history would leave the direct path open. The MCP fixture's
omitted server ID, phase and persisted fields are native optional/defaulted
fields, not a separate schema defect. This source review is retained as
`sdk-parser-3894912-ruling-r1.md`; it is not an executed corrected SDK pass.

The clean successor `4d561ef06d1e040f40b7e0b1bb8ad0271d8baaed`, tree
`bdd28e06558cffe6310dd41adbfc6e381ed23251`, contains parser correction
`4e3839f5db0585e0c3b04f74f38555ee5abe2555`. Two independent source reviews
confirm that the public entry now invokes the shared notice validator before
projection, preserves valid/null/absent raw data and optional MCP fields, and
retains malformed-event feedback with permitted sibling continuation. The
negative assertion now requires a confinement-refusal-specific error. The
subsequent `sdk-ts-attempt1.log` records 524 passes, no failures and one release
asset network-test skip in 5.132s, including both relevant refusal controls.
Its SHA-256 is
`e37cc87f204f3f6f4900aee3cbd747a9f57f6bc3e390e77f694f1fd6be97a81a`.
That completed SDK log has no HEAD/dirty header or shell RC, so it is not
assigned an exact-head qualification. A separate existing sequence starts on
clean `4d561ef06` at 03:07:13 UTC. By 03:19:44 UTC its seven-package
all-target/all-feature check passes after 11m14s, its two restart binaries pass
all 18 entries after 2.62s compilation and 7.439s execution, and its four
confinement contracts pass after 0.10s compilation and 0.004s execution.
All three commands record RC 0; the checkout still has the same clean HEAD and
tree. The restart count includes three process-child helper entries. The
confinement controls retain exact wire causes, released ordinals and local
feedback settlement companions; they are not supported-Linux sandbox positives.
The same sequence completes at 03:26:30 UTC after 19m17s wall time. Clippy
for `meerkat` and `meerkat-rest`, all targets/features with warnings denied,
passes after 7m49s; schema and SDK codegen freshness each record RC 0. All six
selected commands have explicit successful results. A 03:26:39 UTC read still
finds the same clean HEAD/tree and confirms the parent process has ended. The
complete log has SHA-256
`f4e97a9edde643242a6cef2e6d4960e3c2f3ec11f4bf0f8e94e52c78cacb8d12`,
superseding the partial log for this sequence's completion status. This is
selected qualification, not a whole repository gate, normal push or CI pass.
The scoped source review is `integration-4d561ef-source-review-r1.md`; the
unrelated main merge is outside that review. Existing owners retain normal
qualification and publication.

The next existing run starts at 02:24:06 UTC on `5aaf15074` plus tracked diff
`25d666af16b9ae6c0ebf3e7e0fb7d6f8ae8b9fb90cdfe94373f4f376ad61d1de`.
Its 18 restart passes include three actual cross-process parents and three
ordinary-run child helpers. Test time is 8.129s after 3m29s compilation. The
new failed-compaction-attempt case preserves exact persistence/retry assertions;
it does not prove cold decoding of a completed rewrite or permission continuity.
Stronger teardown now rejects a false canonical-unregister result. A stale
file-wide same-process comment needs narrowing. The four confinement contracts
pass in 0.004s after 1m31s compilation; five ordinary sandbox controls pass in
0.007s after 28.42s compilation, with 26 skipped. These are scoped results.

Schema freshness and SDK-codegen freshness pass. Python records 789 passed,
13 skipped in 1.17s, exit 0. TypeScript records 523 passed, one failed and one
skipped in 5.130s, command exit 2; the failure is the already reviewed
`parseSessionMessage` missing-exception case. The final selected Clippy step
fails, exit 101: `cold_restart_resume_after_compaction.rs:350:41` introduces a
panic forbidden by the target's lint policy. The existing run ends at
02:48:46 UTC after 24m40s total wall time. Its complete 224,019-byte raw log is
retained as `integration-5aaf-completed-20261008.log` in the existing review
artifact prefix, SHA-256
`1dac9104c3698351b2570037a3ee128ac8cbb68f769ac35411d39a9eda24e2c6`.
This is a completed red correction batch, not a clean-head qualification.

The existing successor run starts at 02:49:51 UTC on clean
`c2b3c8503578d1d7a8d5f487ddf50b56b5c9170e`, tree
`d2dbb94ec7543c2824f72288c4fa21582cbf5c0d`. Read-only comparison against the
reviewed dirty predecessor finds byte-identical `cold_restart_resume.rs` and
only formatting plus a test-file `clippy::panic` allowance in the compaction
test. At the 02:55:19 UTC snapshot, both restart binaries pass nine entrypoints
each, 18 total and zero skipped, exit 0. Three are subprocess helper entries,
not additional independent scenarios. Compilation takes 2.59s and the tests
7.740s. Selected all-target/all-feature Clippy remains live with no result or
END marker. This is a clean-start selected-test pass, not whole-gate completion;
the stale same-process comment and unrelated lane/parser findings remain.
Raw (operator-retained path) snapshot SHA-256 is
`7f489c9cc809ce9018ef1c79bdbfc49c0f5b24e92f10dcd1803def9a4f0885e9`.
The existing run is retained and has not been restarted by this review.

That same run completes at 03:04:23 UTC after 14m32s. Selected Clippy passes,
exit 0, after 14m20s compilation; the 18-test selection above remains green.
The 03:05:04 UTC read confirms the checkout still has the same clean HEAD and
tree. This closes the scoped restart lint failure. The complete 8,894-byte log,
`integration-c2b3-completed-20261008.log`, has SHA-256
`f66919a1b337cc2eb1e09520c3d9f97cf8dc93cbc109bec2fb3acf983af8ccc1`
and supersedes the partial snapshot for run status. The command only contains
the two restart targets and selected Clippy; it does not rerun SDK, required
Linux, publication or remaining whole-gate qualification.

The existing ADR ruling accepts GCP's proposal to route the named Linux
backend-dependent cases through `make test-mcp-confinement-positive`, provided
the existing command explicitly executes all moved cases alongside its seven
MCP and hook cases. At this source, the affected sandbox set contains five
compiled-confinement and sixteen process-confinement cases. Required-lane
selection must account for the exact expected cases, without counting helper
subprocess entrypoints; unavailable, skipped or missing cases cannot qualify.
Only the Linux ordinary-test selection changes. Assertions and macOS coverage
remain intact. Backend-independent refusals and the no-namespace locality case
remain ordinary tests. The mixed unsupported-dimensions case must retain its
pure refusal controls there while its supported-compile control joins the
required positive lane. No host policy change or automatic permissive fallback
is authorized. This routing decision supplies no supported Linux positive pass;
that evidence and Q3 remain open under their existing owners. The captured
25dd working source had no routing delta. Successor `5aaf15074` implements the
exact 21 sandbox + five MCP + two hook selection, preserves assertions and
splits the mixed supported-compile/pure-refusal control. Its Makefile still
initializes aggregate status only after successful fixture preparation, so a
setup failure can be lost if the suites later pass; initialize status first
and retain setup failure while continuing the suite attempts. This is the only
new source residual in that routing delta. The new run actually attempts all
28 positives: all fail, command exit 2. Twenty-six failures explicitly show
BackendUnavailable; two report assertion failures without the actual refusal
cause. These results establish attempted selection, not eligible-host positive
acceptance or the underlying host cause. The detailed source rulings are
`integration-5aaf-ordinal-linux-review-r1.md` and
`cold-restart-5aaf-delta-review-r1.md` in the existing review artifact prefix.

The fixture-status defect is source-clear in
`bbf71ecb2c8eb4dc552ca8280b9c0b9d50eaa3e2`, retained by `4d561ef06`:
status is initialized before fixture setup, setup failure is recorded, and all
three suites remain unconditional without resetting that failure. Selectors
and required counts are unchanged. The same commit narrows the stale restart
caveat and replaces the explicit panic closure with an expect retaining the
phase/error tuple, removing the file-wide panic allowance. These test/command
source corrections do not supply an eligible-Linux positive result or extend
the earlier c2b3 execution to the successor.

Two follow-on design/source reviews remain outside .52 and add no build lane.
For a downstream consumer, its pinned audit
and MobKit cached main `39a48ca03c2cca21affc248fed5527eeb4245189` confirm there
is no single stock destination contract for the full persistent-host setup
journey. The ruling is an application adapter over the actual configuration,
approval, lifecycle, native admission and delivery owners. Console composition
does not make its embedding application the destination; route/delivery
projections do not prove persistent configuration or a permitted reply. The
retained `connect-host-ruling-r1.md` links both independent detailed reviews.

For save-cost Lever A, opt-in new writes with the proposed default off and eager
resolution are coherent, but the proposal's lazy-decode premise is stale at
main `6d052fac8679a338c6439f6f7b731e15188939ea`. Referenced bodies must resolve
at the existing authoritative decode/materialization boundary before validated
Session construction. Requested-ID binding, atomic optional first-migration
fallback and unchanged-body reference reuse are required; disabling new writes
does not disable reading committed v2 data. `save-cost-leverA-design-ruling-r1.md`
retains the source evidence. A separate second-eye check found no material
error in that ruling.

Lever B `9fcf3221011e42852a29d64b4f60e30e4887d5be` is source-clear for its
exact prepared-boundary readback after commit error. It reuses acknowledgement
and distinguishes unresolved infrastructure from permission refusal. Existing
tests need to retain the pre-retry committed target and assert typed executor
stop plus actual fatalization. `head-canonical-9fcf-review-r1.md` records those
bounded oracles; reported 440 passes and Clippy remain unverified raw claims.
Neither lever establishes overhead acceptance or the remaining caller coverage.
All named review artifacts are retained in the root's October 7 review artifact prefix.

The unbuilt R2 staged-witness candidate
`93afe928db74f23e0c78415c583121edf6ff9d75` has two further source findings.
Keeping Session version 4 lets its parent reader ignore the new staged map:
an active-A/staged-B document can resume against mounted A, then lose B when
the old builder saves its shared-map interpretation. Mounted B instead causes
old restore to refuse. Use the existing generated envelope version gate for
new writes and explicit legacy migration; a Breaking note cannot prevent that
silent reinterpretation. Separately, Stage can legally retain active x/A after
a catalog refresh to x/B while staging All or another name. The replacement
guard then rejects that pending state because it demands a same-name staged
witness for stale active A. Preserve active authority while making restore
accept the pending states the owner can actually produce. An existing negative
test explicitly encodes that wrong refusal and must become the legal
Stage/snapshot/restore/commit control. The authored DSL also duplicates an
existing helper. The original-map legacy split and present-empty staged-map
handling are sound. Stage's canonical update frame and focused pre/post
assertions suffice for noninterference without a stored prior-state mirror.
`r2-staged-witness-93afe-ruling-r1.md` retains the ruling and links all three
independent source reviews in the same review artifact prefix. Existing authors retain
correction and execution; no generated drift, TLC or test pass is claimed for
this WIP.

The unbuilt `f9162e50e0bff958f6de22bb00926470999390d7` successor closes the
canonical pending-removal defect and duplicate helper. Its All/Allow{y} controls
now drive actual Stage, snapshot, fresh restore and Commit; the parent-v4
version fixture is byte-exact. Authored envelope v5 is the proper downgrade
barrier, with original-version migration at all three reader entries. Two
handwritten reader defects were found: explicit null passed the v5
field-presence check and decoded as legacy None; HeadCanonical migration
changes the ordinary map before installing the old baseline, which clears
dirty tracking. Successor `8134d9a23e74cba9022107ff919fcc77ec10da65` closes the
null source defect by requiring an actual JSON object, keeping empty maps
authoritative and rejecting legacy declarations. It adds null/empty controls
to the existing reader test. Successor
`e408ad7c46b8183305978121018cebfc52894468` closes the HeadCanonical source
defect: the helper reports a real rewrite and the changed visibility key is
marked after installing the actual old baseline. The full-snapshot path and
null correction remain intact. Its new regression initially passed an ordinary
delta-only prepared successor to a full-snapshot materializer. Successor
`f9a63f8dcd97d4298e504d32a3823b54dc8cac7f` corrects that pure oracle: it
requires a written visibility mutation containing the exact split cell and an
object-valued staged map, then checks the successor version. The pre-fix code
omits that mutation, so this test discriminates the reported defect. Its renamed
test and comment correctly limit the claim to preparation/materialization;
durable commit/reopen is not claimed. Full-map assertions do not populate an
inherited witness, a coverage limit. No source blocker remains in this reviewed
sequence. Generated Rust is still v4 at these pins pending planned generation,
and no execution is claimed. `r2-staged-witness-f916-ruling-r1.md`, its original
reviews and correction addenda retain the history; the final
`r2-staged-witness-f9a63-test-r1.md` supersedes the open test finding. Its SHA-256
is `4294a0f1a6778f7e5b7890f62b473570bedbc2f75b47055b89ce01924929783f`.
The existing author owns scheduled generation and qualification, with no
additional review ACK required.

H5 source review found iterable non-set peers accepted by the new map-key
comparison. The captured H5b correction uses `BTreeSet::iter` and closes that
Seq/Option widening while preserving operand order and single evaluation. Its
existing doctest run ended at 01:25:41 UTC with exit 0: one valid-set example and
two compile-fail controls passed. Wall time was 371 seconds, including 6m 00s
compilation; aggregate doctest time was 0.92s. The raw GCP log
(operator-retained path) has SHA-256
`e945efeac870d3649ed8be5ab997ccf509b30a520434394ddba8d592ea1facec`.
Its header is `897827ff36381a277c8970d08ea9f1532d3dba3c` plus five dirty
entries, not a clean committed candidate. The header identifies the tracked
diff but does not independently hash the new untracked doctest file.

A further source check finds a separate common-key-type gap in both
keys-to-keys and keys-to-set comparisons. Supported named Rust types can define
cross-type `PartialEq`: iterator equality then accepts distinct key types that
owned `BTreeSet` equality rejected. This is a source-derived reachable type
shape, not an executed example or observed production use. The existing helper
must retain a common key-type constraint or conservatively preserve the owned
comparison; no broad typechecker or new runner is required. The Seq/Option
finding stays closed. Quantifier/count, operand order and strict-read/mutation
boundaries retain their scoped source clearance. The earlier reported 3,706
tests, drift and Clippy results belong to the owner's precommit tree. The later
H5b selected-test log has now been independently read: all 3,706 tests passed,
with 1,773 skipped, exit 0 at 01:53:55 UTC. Wall time was 28m 14s, including
23m 33s compilation and 280.263s of tests. Raw GCP
(operator-retained path) has SHA-256
`ff35cde2f90b10e416a1122ee05d2613211c38a9f046fb74ae379cddec2023df`.
Its header still identifies `897827ff36381a277c8970d08ea9f1532d3dba3c` plus
five dirty entries and tracked diff `8f4e54489645c61f0e05f4e15a8bb0c6613fe441d865fc05433d4d77f4308797`.
This is executed functional evidence, not clean-head qualification, a common
key-type negative control or an overhead measurement. Its subsequent existing
generation-drift command passed all 17 machines and nine compositions at
01:56:58 UTC, exit 0, in 183s including 1m 09s compilation. Raw
`H5b-drift.log` has SHA-256
`a2c925bd6a5b7f4025c9293f4b2f2d0403838bb76c2390636a0b04c423651716`.
The same script's selected release Clippy command subsequently passed with
warnings denied, exit 0 at 02:11:03 UTC, in 14m 05s. Raw `H5b-clippy.log` has
SHA-256 `31b1bec0a5ea95801966edde2ffc46578ada50866408194a949e9aa6dae4b24b`.
The four existing H5b steps took 51m 33s in total. Their headers retain the same
dirty-source identity; completion does not close the separate common-key-type
finding or establish an accepted performance result.

H5c at `dbb6b9d78c97d5622c7ce519d1d46504981a346e` plus three-file diff
`c69d0ff2e8707e1300913fe6a0cbbfda667b023da150cd1db866735a91de1d31` closes
the common-key-type gap. One generic K constrains both iterator items to &K
for keys/keys and keys/set, including Eq/Neq, retaining ordered single
evaluation. The narrow source review requests no further source gate.
Doctests pass two positive and three compile-fail cases, exit 0 at 02:33:43 UTC:
343s wall, including 5m31s compilation and 0.49s doctest phase. Raw log SHA-256:
`7f7508887bb05d56d3fd311fcf45f1963f55202d776ce0b10ad1d25ce02c4fea`.
The existing focused release selection then passes all 30 tests, exit 0 at
02:38:07 UTC: 264s wall, 4m23s compilation, 0.009s test time. Its 27 excluded
binaries are not claimed as executed. Raw log SHA-256:
`9ff7041e99633a6468d1c7795b67e4a44fb8dacfa87b745447233546d9a05bc3`.
Together these two steps take 10m07s. This is executed correction evidence on
retained dirty source. The subsequent clean freeze
`9bdcf7e553dd3e9196e5b0c4674bf0c8d816d309`, tree
`b35894d665e5801f8a3316fe4cf46b803402b826`, contains byte-identical production
correction and doctests; the lowering assertion changes only formatting.
Read-only source comparison establishes that correspondence; the earlier five
and 30 passes retain their dirty-source identity. The clean candidate's release
`native_cost` build then completes successfully in 481s. Its retained header
records binary SHA-256
`2e5ec2262c1c36a34d9a5fabb0cb78e00bd7c062e8661252de81b02271799db4`.
`native_cost_correctness` and
`representative::native_representative_correctness` each pass one selected test,
in 0.03s and 3.68s respectively, with 18 filtered per selection. These are
correctness controls, not latency measurements. At the 02:56:10 UTC process
snapshot, the existing owner's selected release Clippy is live, and its parent
command queues `compare3.sh h4-h5-r1 8` next. No H5 pair result is present in
the bounded directory snapshot. The retained `h5c-perf-evidence-20261008T0256.md` and
raw build/test logs distinguish this queued comparison from completed full
six-cell overhead acceptance. The existing owner retains that measurement.

The same H5c sequence completes release Clippy at 03:07:52 UTC, exit 0, after
847s wall time (14m06s compilation). The queued H4/H5 fast pair then completes
with all 16 processes successful. Both benchmark source files are byte-identical
between the retained H4/H5 build heads. All 160 raw rows contain 256 input rows,
four reads and two model requests; audit counts are zero for trusted and 18 for
governed in both arms. The shared benchmark's normal-completion, exact physical
call IDs, fresh-input consumption, retained old rows and unchanged-file
assertions remain active. Each arm supplies 40 trusted and 40 governed samples.

The recomputed H4 trusted/governed means are 3.225817/4.607538 ms (+42.833%);
H5 gives 3.174229/4.285891 ms (+35.021%). The added mean falls 0.270059 ms,
or 19.55%, and the governed mean falls 6.98%. Completion accounts for about
0.186 ms of the smaller gap and admission through the first model request for
about 0.084 ms. The latter still contributes about 0.814 ms, or 73%, of H5's
remaining gap. These are observed phase intervals, not function-level savings
or confidence estimates; overlapping admission-return time is not added again.

The actual selection is only `representative::native_fresh_pair_fast`, depth 1
FreshAdmission with five pairs per process. It does not execute the original
depths 1/3 by FreshAdmission/ContinuingSegment/IndividualFencedTools matrix,
its fixed warmup/measurement profile, or the separate individual-dispatch tail
measurement. The observed +35.021% itself remains above the <=10% target.
Recorded load of 16.92 at start and 15.12 at end is retained without treating
load alone as proof of a quiet host or grounds to discard the observation.
`h5c-pair-completed-review-r1.md` retains the raw-row and source review;
`H5c-clippy-completed-20261008.log` has SHA-256
`73daae89ef75fb9010fc693cc1294c67fb5961b8fb835b933f78c34e8d00b67b`, and
`h4-h5-pair-completed-20261008.log` has SHA-256
`06b441cdcbe9338ddbcae4ef6294003b64910d6b009f4d193e99d76d0827538e`.
No new benchmark or acceptance gate was introduced by this review.

H6 R2 is viable as a design with ordinary dependency policy: the inspected
MPL-2.0 allowance and Rust versions do not create a new human approval gate.
Its implementation must cover macro consumers, logical map and set escapes,
physical keyed reads, existing authority/recovery checks and pre-change fixture
compatibility. The corrected 0.18-0.2 ms estimate remains a hypothesis from
pre-H5 governed-minus-trusted profiling, not measured removable copy cost.
Dependency-graph checks, implementation, scheduling and qualification remain
with the existing owners; the unproposed Auth-A elimination remains outside it.

### Voice WorkGraph restart source review

Candidate `cdcf70aa13c159f4bc09b783220afeb9709ec516`, tree
`c0d9865b7496526cd6b3af83cd07c749cb084488`, was reviewed against direct parent
`2575736dd8e8127b598dd7fd8bb41d8e87715ae2`. The new native item association
and replacement-claim retry retention are coherent in memory. Actual lifecycle
persistence still uses `LiveBridgeRecoveryImage`, whose schema, capture and
restore omit the association and enumerate only worker identities. Real cold
reconstruction therefore loses item links and workerless operation custody.
The new tests reuse the same runtime when recreating the coordinator and cannot
expose that omission. The existing durable image needs this custody; a new store
or item IDs inferred from labels would not repair the authority boundary.

Two separate cleanup gaps remain. Missing workerless observations are absent
from the final unavailable check, so the driver can silently remove them from
its retry inventory. A pre-commit Record refusal followed by two unconfirmed
item closes drops the sole local association before it enters pending custody;
no native recorded association exists to recover that open item. Existing
owners must retain truthful settlement debt until closure is confirmed.

Workerless cancellation also lacks a current-association comparison under its
existing mutation gate. A warm snapshot A followed by Replace(A,B) can cancel
the operation associated with B while settling A. This is a public-wrapper
currentness gap, not a demonstrated cold-process race: cold restore removes
the old channel binding. An ordinary persistent Record error marks durability
reload-required, and worker admission uses that same gate. Physical serial
execution after an uncertain commit is therefore not established. Replace's
already-current-next branch proves desired-state idempotency, not the supplied
historical previous relation.

The released-result invariant conflict is byte-identical in parent and head.
Completed, release, close and terminal-custody restart can remove eligibility
while the invariant still requires every retained release to be eligible.
Preserve confirmed Completed release history and InterruptedByClose, keep
revoked results ineligible, and retain all current delivery and custody checks.
Historical receipt replay is distinct from new delivery permission. These
source findings were delivered to both leads; the owner's reported 189 tests,
Clippy, drift, TLC and seam passes have not been independently read as raw
results. No cold-window pass, publication or .52 inclusion is established.

## Current delivery status, 2026-10-07

The full [acceptance plan](acceptance-plan.md), read through the accepted
[local default](../adr-001-local-governed-default.md) and
[review/consent amendment](../adr-001-model-review-and-consent.md), remains open.
A focused pass or delivered patch does not complete another operation family,
surface or recovery boundary. The 0.9 coordinator leads the work; the existing
GCP owners retain native integration, qualification and publication.

Coordinator cutoff decision 31 fixes Core 0.8.52 to the existing migration API,
merged destroy and current integration/web-search batch. The GCP lead records
and freezes its exact inventory. Remaining D1 surfaces, platform-layer 0.9 work, other downstream
and optional integration work do not enlarge that cut; regressions in included work
and applicable native correctness/performance requirements still apply. Existing
publication authority 29 remains in force. This is scope control, not evidence
that a candidate was qualified or published.
The coordinator's applicability clarification of decision 31 permits that finite
interim publication while the six inherited performance misses remain explicitly
open for final 0.9 acceptance. Its inventory excludes the optimization branch;
new serious regressions in included work still require correction. This does not
relax the final mean <=10% or dispatch p99 <1ms requirements. The required Linux
positive lane and its assertions must not be silently weakened; any interim
source exclusion remains explicit and leaves that acceptance open.

The published checkpoint is now merged: PR1730 became
`8ca50a1a459eef6a79fd8bc33cd7d067b9b1f9bf` at 04:02:54 UTC on 2026-10-07.
Its tree `0814ac6d004fc7183986ed7e5c18a1a430b83c07` exactly matches the tested
`dab5f9865` head and Live checkout. The independently inspected PR checks record
46 successes and 5 skips; the required CI gate and the 11-scenario Live gate
passed. This is the merged functional checkpoint, not full profile/recovery
coverage or accepted overhead. The following implementation batch has the two
reviewer scenarios passing and its feature-gating correction independently
source-reviewed with targeted Clippy, the 33-package check and Clippy, and the
license gate passed. Detailed source and execution boundaries follow.

The first complete native cost study has now been independently checked. Its
fresh-admission whole-turn means miss the 10% target at both depths: +136.18%
and +151.06%, with conditional simultaneous ratio intervals wholly above 1.10.
Measured tool and full native model-preparation added p99 estimates are below
1 ms in their bounded subsets. Host activity overlapped the mean run, so quiet
qualification remains unresolved; neither that overlap nor the tail estimates
erase the recorded mean misses. Cheap-operation and full performance acceptance
remain open. The detailed result and next action are recorded below.
The earlier fast G1/G3 comparison reduces depth-1 diagnostic mean overhead from
58.65% to 41.18%, still above the target. The optimization now has a clean
committed-head run of 512 selected tests passing. That selection's 743s wall
time includes 12m21s compilation and a 0.641s test summary; it does not provide
new representative performance acceptance.

H1 successor `d45a2c8aa52e7f7e102bf328cf78fbc4a2499ccc` adds only the
requested optional-inequality and snapshot-isolation controls to the reviewed
optimization. Its completed clean-head log records all 3,579 selected tests
passing, 1,895 skipped and exit 0 at 20:36:26 UTC. The previously pending
remote-spawn resend-budget case passes in 120.032s. Release compilation takes
21m41s, the test summary 279.591s and the whole command 1,582s. The existing
`make machine-check-drift` then passes for 17 machines and nine compositions,
exit 0 after 205s, including 1m34s compilation. Release Clippy for the eight
selected packages completes at 20:54:25 with exit 0 after 14m34s, on the same
clean head. These are functional, drift and lint results, not overhead
acceptance. Retained completed-log SHA-256 values
are `ff17bd1a86e3854db3269cd4360ba7af38ce4f5b58856c484b0baf49ea4060b0`
for tests and `f6253c631aa0b767d7048462517485240d5d63bdae2eb65732b78430cc3a99af`
for drift. The completed Clippy log has SHA-256
`242af49afa13d8064504ec5d124c3c956db215979532e44cda0d4c3698b8ba62`.

The subsequent H1 fast comparison completes at 21:08:51 UTC. Its clean build
at `d822dd8873dbf2bc202e8ef38d6158ac4a4d527a` has exactly the same tree as
qualified d45a2c8aa. Eight alternating pairs contain 40 trusted and 40 governed
observations per arm, all 16 process exits 0. Independent calculation of the
160 retained CSV rows gives H1 means of 3.173775ms trusted and 4.590425ms
governed: a 1.416650ms difference and 44.6361% overhead. The paired G1/G3 arm
at clean `34716033ced5c6dfbb7b73f0095b8912c0631fe7` records 3.362700ms and
4.712500ms, a 1.349800ms difference and 40.1404% overhead. H1's governed mean
is 2.59% lower, but its measured authorization overhead is not lower. This
small comparison ran under concurrent compilation and remains diagnostic;
it neither replaces the original six-cell study nor establishes dispatch p99.
The original final thresholds remain unaccepted. H2 work started later in a
separate dirty source state and is not attributed to this H1 result. CSV
SHA-256 is `562b2f8baf42f78e4a03f05c10b7fe3b6a1dd5251b6c4c0af416493b793075cb`.

The later H3 optimization at `75bee8e3dee3864d272fbb80a5df1e84a59ad6ae`,
directly after H2 `f7e84b2f0babac7dffdaf62bfba0819b108f14f3`, is independently
source-clear. It retains exact full and batch encodings in each immutable
association, outside serialization and equality; it caches no permission or
currentness decision. Runtime binding still returns owned strings. Its clean
committed-head selection passes all 175 tests, with 1,932 skipped and exit 0,
at 22:46:23 UTC. This includes both new encoding controls and existing retained
authorization/currentness controls. Compilation takes 11m16s, test bodies
0.120s and the full command 677s. Completed raw-log SHA-256 is
`a82daa3fcb488beac4419c6aeae30ee06522dec84d0399a0b3543f26ec6809aa`.
The separate passing precursor Clippy log is not bound to this exact committed
patch. This optimization remains outside the finite Core 0.8.52 publication
inventory.

The completed H2/H3 fast comparison contains 40 trusted and 40 governed
observations per arm, across eight rotating rounds and 16 successful processes.
Root independently matched all 160 retained raw observations to the derived
table. H2 means are 3.240535700ms trusted and 4.591901325ms governed, with
41.7019% overhead. H3 means are 3.314164575ms and 4.649486750ms, with
40.2914% overhead. H3's governed mean is 1.2541% higher; the slightly lower
overhead ratio does not establish faster governed operations. The exact clean
H3 executable build takes 488s and exits 0. Its byte hash is separately bound
to the source header, not substituted for a Git identity.

The contiguous phase partition places 0.880750ms (65.96%) of H3's 1.335322175ms
mean difference before the first provider poll and 0.247852700ms (18.56%) in
completion. This supports prioritizing the existing four-input admission/run-start
path, without identifying a redundant check or dominant subroutine. The separate
admission-return measurement overlaps and must not be added. Scripted-model
phases include real native authorization, not an artificial provider delay.
These depth-one observations ran under idle scheduling with recorded host load;
they establish neither significance nor a quiet window. The original six-cell
study and final mean/p99 requirements remain open. The derived-table SHA-256 is
`2361c222469b5c76d3eab9d272134a75f8fd1720345fd5b8847585cb955d0aba`.

The accepted-row digest optimization is now committed at
`5c1ef93a843a232cf0bd18d569cd4691c3932f83`, tree
`d347ced1036343dd2507e454e405e558836319ce`. Source clearance carries from
the reviewed five-file WIP: four files are identical and the fifth changes
only a line wrap. It reuses the original immutable submitted-input digest;
binding, currentness and recovery checks remain in place. The clean-head
runtime selection passes all 271 tests, with 2,237 skipped and exit 0 at
23:53:21 UTC. Compilation takes 9m53s, test bodies 0.514s and the full command
594s. This includes the accepted-row digest, submitted-input currentness and
persistent-row controls. Raw-log SHA-256 is
`3b94e5851b3fd4f0e285e1eaa5736f86f89c400c94f8638bda25270f339ac080`.
The earlier passing Clippy belongs to its dirty precursor, not this clean
commit. The benchmark executable then builds successfully in 410s, with both
existing exact correctness selections passing: native correctness in 0.03s and
representative correctness in 3.63s. These are correctness checks, not accepted
timings. The text-only persistent fixture still does not establish actual image
externalization. No additional source-review or execution lane is needed.

The H3/H4 screen completes by 00:01:03 UTC on 2026-10-08: all 16 processes exit
0, with 160 retained observations across eight rotating rounds. H4 means are
3.080233100ms trusted and 4.327913150ms governed, a 1.247680050ms difference
and 40.5060% overhead. H3 means are 3.956221850ms and 4.260595800ms, but its
apparent 7.6936% overhead is distorted by a retained 37.932919ms trusted sample.
H3 trusted/governed medians are 2.901111500ms/4.177861500ms; H4 medians are
2.923410000ms/4.301903000ms. No sample is discarded. H4's governed mean is
1.5800% higher in this screen, which establishes neither improvement nor a
statistically significant regression.

This depth-one fresh-admission diagnostic contains 252 old rows and four new
contributors, with two model requests, four final reads and 18 governed audit
records per observation. Its phase partition is contiguous; the overlapping
admission-return timing is not added. Host load was 9.67/10.03/13.48 at the
start and 8.72/9.79/13.31 at the end under idle scheduling. It is not the full
representative timing matrix or evidence of a quiet window. The existing owner
now has completed correctness and comparison results to guide its next change;
the original representative mean <=10% and p99 requirements remain open.

### Follow-up qualification and review, 2026-10-07

The public Live S98 failure in [issue 1832](https://github.com/lukacf/meerkat/issues/1832)
is independently confirmed in the [attempt-3 CI log](https://github.com/lukacf/meerkat/actions/runs/37671167052/attempts/3)
at `6db8ee8fb52bfae5d45c2b1c7041a7e9c8c9fed1`: one selected test fails, zero
pass, 112 are filtered out, exit 101 after an 81.142s command and 78.04s test
body. Playback terminal projection times out; error cleanup reports provider
close and canonical drain unconfirmed. The issue attributes the intermittent
stall to the observation path during ExistingMember delegation; that cause is
owner-reported, not independently reproduced here. Other successful attempts
do not close this failure. Existing diagnostics and runtime owners retain the
fix; this supplies no new live recovery or close/drain acceptance.

The later Notification qualification ended with RPC 6 passed/2 failed and
native `stock_persistent` 9 passed/1 failed. These are failures, independent of
the earlier merged checkpoint. The amended candidate is
`f22bfc7cbf12c0790045445fe9323057142eb1a5`. The existing owner deliberately
checked out clean baseline `aa55e0082` for comparison. Its completed
`base-two-tests-attempt1.log` records RPC reconstruction failing with
`session_served_elsewhere` (RC 100, 12m07s build, 0.330s test body), while native
whole-blob fresh-work-after-reopen passes (RC 0, 3m28s build, 0.689s body).
The original log ends normally and its wrapper is gone. This establishes that
the RPC failure also occurs on baseline; it neither qualifies the candidate
nor explains the separate Notification failure. Source review found that normal live-actor
discard signals shutdown but drops the task handle without awaiting exit;
runtime-loop completion alone does not prove release of the session actor's
hosting claim. The correction remains with the existing lifecycle owner.

MobKit gate-ID commit `e3d637c5f2f34abc4fdf941b8c4b1d3e662951c1` prevents newly
minted cross-boot ID reuse, but restored v1 gates can still resolve unrelated
legacy promotions. Both RPC dispatch maps also classify `IdsUnavailable` as
invalid input. A reviewed test-only patch (`a856efebb0d4`) exercises actual v1
restore and approval, rejection and escalation notices against exact retained
SQLite promotion custody. It is delivered for owner execution, not an executed
failing test or an accepted repair. The owner's successor
`6c678a0714544ac87e46adb55ef6811e6c1f3142` is source-clear for these corrections:
the legacy guard returns before store access, and sync/async RPC and Console
retain typed infrastructure errors. Its own restored-v1 approval, rejection and
escalation regression overlaps the delivered test, which need not be added
again. The 71-test and Clippy passes are owner-reported; root has not read their
raw results. The later `6c678a07..61ef5bdb` successor is independently
source-clear: it strengthens the same regression with exact SQLite stage-token,
batch and creation-time equality after each decision, plus comments. PR 624's
completed CI test job `112859344966` independently records 3,458 passed and
159 skipped in 243.072s, including the strengthened test. Its tested merge
`cc4f1297a9c6e88c4300b9bff21eaa3e09e9d177` and reviewed head
`61ef5bdbaf4f8a4c08a867a679217558203bced8` have the same tree,
`5055c960739820f1313628518917f9c65224434c`. The final inspection reports all
11 checks successful, including the gate. GitHub subsequently confirms the PR
merged at 15:59:47 UTC as `2b08bb6005ce983c1cb1fd1e479a9571471cf976`, with
that same reviewed and tested tree. Root and independent review find no source
composition delta; this is merge evidence, not a new merged-main test run.
Root also read the completed voice and console job logs on that merge. The
console log includes 771 Vitest passes and the existing browser E2E passes;
its authorization-feedback browser control uses a mock backend. This is
observed CI evidence for this PR, not full native authorization acceptance,
a release or concurrent steward-WIP claim.

A second reviewed test-only patch (`2621e5b91756`) extends the actual native R3
dispatch scenario through `HumanConsentRoute::present`, checking raw arguments,
prepared facts, the dispatcher plan and historical policy observation while
retaining local refusal and permitted sibling execution. It is adopted in
clean commit `ccccbc345083015c8c0a25259e264b4b40268a0e`, with only formatting
differences from the proposal. The independently read `nb-test3.log` identifies
that head and records the actual R3 scenario passing: 1 passed, 137 filtered,
RC 0, 3m28s build and 0.00s test body. Its log SHA-256 is
`962204606c85ce7f5fd5845a9feefa987d3aa251975cdd935084d3d0aa96af3c`.
The matching Clippy run later failed with three test-style diagnostics. Clean
successor `f8c9facd55b5033c810880d0000731541e0ae73d` corrects those three lines
without weakening the assertions. Independently read `nb-test4.log` identifies
that head and records 1 passed/137 filtered, RC 0, 4.27s build and 0.00s body.
`nb-clippy4.log` identifies the same head and finishes in 3.38s with RC 0. It
does not echo command flags, so this result does not establish a broader Clippy
selection. Selected-account and mandate context remain incomplete. The next
context commit `6547b21d47130a59063b293866fd9ee897a30770`, tree
`f0446890cb6442246d66f375f88752bdd5085b55`, adds bounded passive owner material,
but independent source review found four integration gaps. Unsupported owner
interfaces still default to not applicable; the actual native accepted-work
wrapper does not forward its invocation owner's mandate; the shipping model
reviewer does not serialize the new account/mandate fields; and deadline
fallback presents previously read material without checking currentness after
the suspended review. The later R2 allow and consent-entry checks remain intact,
so the last finding establishes stale presentation, not a permission bypass.
R1 avoids the new read and protected-observation errors remain distinct. These
findings were delivered to the existing owners and remain uncorrected on the
clean head. The exact-head context run now records 41 authorization and 93 core
tests passing, then 80 runtime tests passing and 17 failing, RC 101. Sixteen
failures report admission `ControllerReadinessUnavailable { reason: Busy }`;
one times out awaiting the real row commit. The new context-outcome and
changed-context controls pass, but the cause of the runtime failures is not
established. Compile time was 4m42s. Exact-head Clippy finishes in 5m02s with
RC 0; its flags are not retained. These results do not qualify the context slice.
The independently read baseline `nb-base-runtime-probe1.log`, headed
`f8c9facd5`, records 78 passed and 16 failed, RC 101, with 4m43s compilation and
0.11s test bodies. Fifteen failures explicitly report admission `Busy`; the
foreign-owner control only prints a failed `CredentialUnusable` expectation,
without the actual result. This establishes pre-existing admission failures,
not the precise competing holder or every candidate failure's cause. The
baseline log SHA-256 is
`fe2c324662d6bc0c1acfdacd07b4f238e968cbe87a18f1743cc2df03edbcbb6b`.
A three-file test-only correction (`f42ee22e6536`) is source-clear after root
and independent review and delivered for existing-owner adoption. It derives
each configured fixture's credential from its existing prompt ID, preserves
intentional within-fixture sharing, uses the existing isolated generic host,
and exposes early admission completion while waiting for the real row write.
The existing store gate is released before failure. Global input/replay
semantics and production custody are unchanged. Static forward and reverse
patch checks pass; no Rust execution or proved repair is claimed.

The committed context successor `f060cff13694fcd8e23f570909740e88ec9f8be7`,
tree `322e328905a8acf528f9200e9d51e6c7e4cdd544`, is source-clear for all four
findings after root and independent review (`0a841f37f090`). Unsupported
defaults return unavailable; the actual native wrapper forwards the installed
owner mandate with existing membership/publication checks; the real reviewer
serializes bounded account and mandate fields; and human fallback checks the
retained decision before presentation without re-preparing stale material.
New inherited-default and deadline controls assert no affected entry and a
permitted R1 sibling. The real HTTP reviewer fixture checks the outgoing JSON,
while its owners supply test material; it does not prove every host's policy
derivation or audience. These are source controls, not fresh executed passes.
The runtime-fixture correction and coherent qualification remain separate.
Independent static application confirms the unchanged correction
`f42ee22e6536` applies and reverses cleanly on exact `f060`; none of its three
fixture changes is already adopted there. This adds no executed test result.

Retained f060-labelled logs now supply partial execution evidence in intake
`ab341d0f6ab5`: reviewer tests pass 17 and fail three URL assertions, native
model-reviewer tests pass seven, native owner tests pass one and fail one with
`ControllerReadinessUnavailable::Busy`, and authorization/core/tools selections
pass 44/93/9. The passing controls include installed-owner mandate forwarding,
unsupported work-owner context, and refusal of stale material at the R2 deadline
before human presentation. The three HTTP assertion failures also occur on the
f8 baseline; they do not establish a new context regression or a corrected
transport pass. Clippy finishes RC 0 in 5m36s. These logs omit full commands and
invocation-time clean-source evidence, so attribution follows their head markers
and named tests. The runtime fixture correction is still absent from committed
f060.

Clean direct successor `184782da37f6b8bcbe230235bbdc3f5ace36a887`, tree
`1489cfeb69267c1a4bc03a49c08cdc3203a2b133`, corrects only the governed HTTP
fixture's backend origin. Independent source review (`20c59ddd6b6f`) confirms
the canonical endpoint, request/context and negative assertions are retained.
The actual operation-reviewer selection now passes all 20 tests, with 486
filtered, RC 0, 4m18s compilation and 0.01s bodies. Raw log SHA-256 is
`c19f687fb0fc6206718093eaa4020ab953b4e06876643ccbe5095665e659de19`.
All three earlier URL failures explicitly pass. The still-live owner wrapper
establishes the command; the log itself identifies the exact successor head.
The later complete Clippy log also identifies that successor and ends RC 0
after 5m24s; SHA-256 is
`f3542fecc46c61700db41c0eb372e695f87f5dcc10ddb611f3486de7b4a08490`.
At that source the runtime isolation patch remained unadopted and its separate
Busy result remained open. These 20 passes close the fixture's HTTP failures,
not coherent full qualification.

Direct successor `318f6977c3ecddb4e35e373d09e537ed7eaa5acf`, tree
`a47e2c7cf185cacdb382b35899c4f300566680dc`, adopts the three-file fixture patch
`f42ee22e6536` exactly. Root and independent review confirm new exact-head,
clean-source logs: 95 runtime passes, 146 library passes, 20 reviewer passes and
7 native-loop passes, all RC 0. Both generic-host controls and the actual
row-commit custody control pass, closing the separate fixture failure. Targeted
five-package all-target Clippy also ends RC 0. The runtime and Clippy raw hashes
are `fcaa04b3a444b309987b22d936bb013b39f5851a279b65e761868aa85fe6a18b`
and `d937cd926f9993d3576a8a95ea9ec83bafe699a9c5efc3ef52f2078c744bc812`.
All commands are retained. Their compilation and lint time totals 12m43s, while
the test bodies total about 1.1s. Review `2ef0519f0e4d` records adoption and
scope; these results do not establish full host/account policy, audience,
surface, restart or performance acceptance. The later consent-persistence
source is a separate delta.

That persistence delta is `3e73d38cf096190bbc9ac5890eaab5bbe9f7fb99`, tree
`a3dfcfdf78eb702862bf1a1d03385b3487e31769`. Its confirmed lint-only successor
`58d9d9690f185f5abd28db299850a63b8e4b76e8`, tree
`ebcb0a6625ec06e6c25e721c05d266dbf622385f`, has seven targeted passes and Clippy
RC 0, with exact clean-head commands retained. Root and independent review
(`b42bbf44e0f4`) still find two P2 durability defects. A failed post-rename
directory sync returns unknown commitment, but reconciliation/reopen accept
visible bytes without resolving that barrier. The non-Unix directory-sync path
also returns success without performing a barrier. Preserve the existing owner
and OS-lock domain; establish persistence or remain unavailable. Newly created
ancestor directories require a bounded creation guarantee. CAS, typed
infrastructure outcomes, retired executable custody and the store-free R1 path
remain intact; no permission bypass is claimed.
Private expected-RED patch `74c84208e22b` drives the real generic approval service
through unknown commitment, continued failed-barrier unavailability and exact
history recovery without repeating the decision. Root source review and static
forward/reverse byte checks pass. Rust execution and live-consent no-refund
coverage are not claimed; the latter retains its existing core test. Both
findings and this small existing-module test are delivered to the current owner.

The supplied regression is subsequently adopted at
`49c7007002960d66ade07c42754f199be7e9ecd2` with formatting only. Its actual
run records seven passes and one intended failure: reconciliation returns the
visible unsynced Approved row (RC 101, 3.35s compile, 0.01s bodies). The fix
`54ff5c0dfc828dffeaf1c939f56d79ffd1517a0b`, tree
`520859adffc7061368fda1cd8a16d84d67b8ba30`, then records 136 store library
passes, 85 approval-filtered core passes and store all-target Clippy RC 0.
The exact-head logs retain commands and dirty=0. Compilation is 0.12s, 2m35s
and 2.75s respectively; test bodies are 1.68s and 0.30s. Raw test hashes are
`12ed7c84c7a509cd18c3690d9f5e5d6d4bc050fa4aac9c2f14af0d9a8d596f3a`,
`79f43f7ebd6d1a6baf3aed44486b6929fa7d4e2640c620d546c14c3ec2d1c32e`
and `63b85ddf2d1a7ae583eafadf2ed42373daebf7b38ce32459e1e650b36bac8bc0`;
Clippy is `8a2c4bef0582b3b2e6d0a09d7d5fb09ca7964c23a287dffaa1fce635a1809254`.

Review `ce0e01259178` clears single-instance reconciliation and explicit
non-Unix refusal, but finds two remaining durability defects. An already-open
second store skips the writer's instance-local unknown flag, and qualified
reads release the lock before reading potentially replaced bytes. Separately,
a failed ancestor-directory sync leaves visible directories; retry treats
existence as durability and skips the unfinished barrier. Root independently
confirms both. The existing GCP owner is already correcting the shared-file
read path; no duplicate implementation or concurrency test is introduced.
The distinct ancestor retry control is delivered as existing-fixture patch
`bdc021037d0e11f24100fc690caea82583813ff1534f6b35312c58a656e826e6`.
It holds the failed ancestor's parent barrier across reopen, then verifies
recovery and an actual approval write/read after restoring it. Static
forward/reverse checks pass; the expected failure remains source-derived,
with no Rust execution claimed.
Changelog-only successor `3c5247d2ce533981b3ec6dda6979a744e8dc2394` accurately
marks non-Unix FileApprovalStore refusal as Breaking. That correction does not
provide native Windows durability or full persistence acceptance.

Committed marker successor `f25e1cfd2bba0950296d356b4e3cf52da92b482c`,
tree `c928af068996c10d59e2d860e8760877e3612599`, closes the shared-file
uncertainty and unlocked-read findings. Root and independent review
`0d354a2a5e62` find that `read_locked` instead introduces blocking
`lock_shared`. The existing held-lock test calls `load_all` before releasing
its own exclusive lock, so the new path waits before reaching that release.
Use nonblocking shared acquisition with existing `Contended`, retaining the
marker recheck, barrier and exact-record read under one held lock. The new
race fixture must verify contention before releasing the writer, then exact
recovery. Ancestor retry remains unchanged at this commit; staged successor
bytes were excluded. No new qualification log was available, and the earlier
54ff passes do not qualify f25. The correction is sent to both existing leads.

The actual nonblocking successor
`a888bc469ad001292a9e4ca2052b203132b16fe8`, tree
`a8b1682112b4f1edbf6406566d2685e2f2dd55d3`, closes that regression.
Root and independent review `bb91f278922d` confirm immediate `Contended`
with the marker, barrier and exact-row read still protected by the lock.
Exact clean-head logs record 138 store passes, 85 approval-filtered core
passes and store Clippy RC 0. Compilation is 3.58s, 0.11s and 3.57s;
test bodies are 1.69s and 0.35s. The earlier f25 log actually records the
held-lock test running over 60 seconds, without a terminal footer; the
owner reports terminating its process group. Ancestor test-only successor
`b25a203021de33f6adec2dcd0f6bc18ff130f2e8` is separate and does not yet
establish that correction or its execution.

Ancestor fix `160c0a47016c7b2db6e22cc455fdea2c85385809`, tree
`efbe046256b645b0e68a69717459d8f85c6545a5`, closes the ordinary-path
retry in source and exact raw execution. Review `ce137bc3f6fb` confirms b25
adopts the supplied test with wrapping only: its RED is the intended retry
returning Ok (10 PASS/1 FAIL, RC 101). At 160c, 139 store tests and 85 core
approval tests pass, and store Clippy returns RC 0. Compile/check durations
are 4.04s, 0.11s and 2.91s; test bodies are 1.70s and 0.30s.
The public path contract still permits symlinked parents while the new loop
walks lexical absolute parents, which can omit a resolved target ancestor.
That separate gap remains with the existing owner. Required ancestor read or
sync failure must remain typed store unavailability, with the access requirement
documented; preexistence does not waive a barrier. The owner is separately
correcting initial-load contention recovery and persisted-RPC non-Unix wording.

Successor `4c02c36aa6ea580348348387c2d72c13d588251b`, tree
`e01ee21d97e268526b373892d38301f5765d58ca`, closes those three source issues.
Review `98d35689c688` verifies component-by-component parent-symlink ancestry,
the initial open-time durable-history handoff and explicit persisted-RPC platform
and ancestor-access wording. Initial history can be stale after another owner's
commit; it restores no executable consent, and current `get`/conditional writes
remain mandatory. Qualify the public read-lock documentation for this first-load
exception. Exact clean-head logs record 141 store and 85 approval tests passing,
plus strict store Clippy exit 0; compilation/check took 3.82s, 0.11s and 2.95s,
with test bodies of 1.67s and 0.30s. A remaining P2 is the final approval-file
symlink: the walk covers its parent path, while open follows the file itself
without establishing the target's ancestry. The existing owner must cover that
case or explicitly refuse final-file aliases before accepting history. The
parent-symlink pass does not qualify it.

The final-file alias correction is source-clear at
`bf3290f5268979cbdf8f8e972888a32a7c7e6d4e`, tree
`8f5f847130d6fbac8f44d1bc00711c9ed2277adb`, in review `583c829bf506`.
Its parent `43c99a167` merges 4c with integration `eb99fac99`; only the exact
two-file correction is reviewed here. Open rejects both existing and dangling
final symlinks under its lock before accepting history, and the new regression
verifies that no dangling target is created. `get` now discards the earlier
one-time open snapshot; the nonempty two-instance control proves the following
load cannot revert to it. Public rustdoc explains that initial-load exception.
Exact clean-head logs record 143 store passes, 85 approval-filtered core passes
and strict store Clippy exit 0. Cargo durations are 3.46s, 0.11s and 2.93s;
test bodies are 1.70s and 0.30s. Raw SHA-256 values are
`1a8ecb7a0305d313686c2669f372fc9b6bd17890d87acd4de7fd985f1c0307ed`,
`6e6de14b4c0bdebfe54ba4ce81aa79ba55e938106e1d51a7286a9e54e2fb5917`
and `71cd7c5d454f2940a002007450086dda43ef9c3bff70aa13289cecf443c82f75`.
These Linux default-feature selections close the observed correction; total
wall time, process-crash behavior and full consent acceptance are not inferred.

Documentation successors `3530fe402` and `6efc91b89` faithfully adopt the reviewed
integration guide and corrected owner-context acceptance scenarios on the clean
`consent-nb` branch. Review `c2c0b74bc01f` reproduces the guide merge exactly and
verifies unchanged acceptance oracles. The native account-aware scenarios remain
unimplemented/unexecuted host-integration obligations; documentation adoption
does not supply that acceptance.

The reviewed web-search observation patch (`60a47d74d447`) is adopted in clean
owner commit `5a4c5b16f1b5cd42bd74878552f51e0b478505c1`, directly above
`68bdea27`. Independent review found the two-file change matches the proposal
apart from test formatting, retaining the separate controller/helper binding
and observation-only parent event publication. Independently read exact-head
logs now record 89 selected tests passing across five crates and another
8 factory unit plus 11 factory integration tests passing. These include the
observation-only parent event regression and the distinct controller/helper
binding control. Both runs end RC 0; builds took 1m19s and 2m34s, with test
bodies at 0.00-0.07s per binary. The exact-head check and owner-named Clippy logs
also finish RC 0 (1m40s and 2m47s). Invocation flags are not retained in those
logs, so no broader feature or Clippy selection is inferred.

The direct builtin-free successor `a259d03b5f350900619719888d6f4d727ebfb7d5`,
tree `bede9fd7d89a0351e3d18ae562cbf9d79ce45557`, is source-clear after root and
independent review (`73cab691db54`). Explicitly enabled fallback search now
composes when builtins, shell and image generation are disabled. The empty
builtin allowlist, independent image predicate and native-search fallback
Disable remain intact. New factory controls assert visibility, closed named
tool families and one actual executor invocation; the paired Inherit control
asserts absence and zero invocations of its supplied executor. It does not
prove no executor object is constructed. Retained raw logs, independently read
in follow-up `82ef409da4df`, confirm six factory passes including the new Enable
and Inherit cases, one image-control pass, package check RC 0 and targeted strict
Clippy RC 0. Their recorded interval is 15:40:19Z-15:49:27Z, with an exact head
before each command and a clean starting tree. The Bazel-named log establishes
BUILD-file generation/freshness, not a Bazel build or test. No executed RED,
full-workspace qualification or performance result follows. The current-release
The adapter consumer trial remains separate.

Realm-config candidate `4bfef90b64bf86adedaf944a1cb5b6ab9ef86913`, tree
`718d6ab2e60900d81699d411bf7f04511bb6f99b`, clears the two prior e58 source
findings in review `e99992bd4074`. Explicit realm login validates the whole MCP
list before selection. Mobpack accurately omits realm servers until real member
surface authority is integrated; it adds no unbound handle. Unchanged D2
presence-preserving patch and concurrency behavior is not reopened. Retained
raw results confirm 216 targeted passes and one real CLI realm-MCP pass, plus
check/Clippy completion. Detailed command flags and invocation-time source state
remain owner-attributed. Both D3 reverted-production controls actually fail;
their wrapper's RC 0 is not the failing nextest exit status.

The existing config owner now has the B write ruling: unrelated valid writes
may retain exactly unchanged invalid legacy MCP entries, while new or changed
invalid entries are refused. Compare full typed entries against the persisted
own document under the existing final write/CAS owner, not incoming patch keys
or composed parent values. Update surface prevalidation and final error mapping
together so caller-invalid candidates retain caller-error classes and real
store failures remain distinct. MCP resolution stays strict. The committed
4bf source still has whole-document refusal; no B implementation or executed
B acceptance is claimed. RPC/REST opt-in, scoped OAuth, member-bound mobpack
staging and presence-preserving read/replace remain explicit full-scope work.

The B implementation is now committed at
`c0f7f09d2bff67cd18548ed6f3d737f01408b284`, tree
`a604f3ea12c7b25b88d53a2713a654dd17bf0341`. Root and independent review clear
its exact-entry comparison, current own-document validation, repair/removal and
strict resolution behavior. One P2 remains in its shared RPC/REST/MCP response
mapping: `ConfigError::Validation` can originate while loading an existing
unwired `[agent] provider_params` setting, before any caller candidate exists.
Mapping every such variant to caller-invalid misclassifies config reads and
valid unrelated patches. Preserve error origin in the typed owner path; the B
comparator needs no redesign. Private expected-RED router patch `a8bb0e9e720d`
uses the real file load, checks server-error versus caller-error classification,
and preserves bytes/generation. The owner has now executed that exact adopted
router file on c0f7: zero passed, one failed, nextest RC 100, after 4m17s
compilation and a 0.028s test summary. The first stored-document `config/get`
assertion fails as intended: actual -32602, required -32603. Later patch,
restored-candidate, byte and generation assertions were not reached. Raw log
SHA-256 is `437ea3e9c9ec305970188e45aaa26acff9c8646f8b14617bd7b5bc2f1e9e0319`;
review `ef98fd31fd77` records the exact file adoption and command. This reproduces
the error-origin defect; it is not a repaired or full-surface qualification.

Hook lifecycle source review at `f8c9facd55b5033c810880d0000731541e0ae73d`
found two remaining locality gaps. Optional foreground Observe confinement
refusals reach `HookLaunchRefused` events but not model notices; the existing
real model-tool-model fixture does not inspect that feedback. Independently
source-reviewed patch `0fbe1b74d0d2` is delivered to the existing owner, with a
separately applicable test-first facade patch `6e5d12a39847`. It adds typed
model notices from the existing optional-refusal report and checks the actual
recorded requests and retained completion notice. Variable identifiers are
bounded independently; command, argument and environment data are excluded.
No RED, GREEN, lint or live-provider result is claimed for this private patch.
Mandatory Pre/PostLlm and other lifecycle launch
refusals still reach whole-run failure, including an existing test that expects
that error. Full completion requires keeping the dependent operation withheld
while retaining unrelated work and local typed feedback. Making a mandatory
prerequisite optional would not satisfy that requirement. The inspected
PreTool/PostTool paths preserve operation-local confinement settlement and keep
genuine protected-observation failures distinct. This is source evidence, not
new hook execution or qualification. The subsequent lifecycle plan
`483b441d9a71` proposes a nonfatal refused-run terminal. Review rejected that
shape: ending the run and allowing a later run still violates the same-run
feedback requirement. The existing authorized controller must receive the local
result and continue permitted work without bypassing the blocked prerequisite.
The plan's PostLlm publication promise also needs correction because ordinary
streamed text can reach callers before that hook. No new terminal or generated
state change is justified merely to rename permission-triggered termination.
The revised r2 plan (`a6110d6bae52`), based on `907b3ce6a107`, uses same-run
controller feedback. Review `26c974b50966` is delivered with five corrections:
scope must govern actual hook execution, effective topology must be validated
before entry, request purpose must be separate from sticky controller routing,
tests must exercise actual mandatory launch refusal, and PostLlm feedback must
name only the subsequent effects withheld. Unsupported All-scope model and
RunStarted/TurnBoundary mandatory topologies require explicit activation errors
and migration guidance, preserving the previous valid configuration. This is an
interim coverage gap. No existing hook event establishes an authorized rejected
raw-output reader; retain bounded protected audit metadata instead of adding a
new content store. These are plan/source rulings, not executed locality.
A root and independently reviewed migration subsection is now in the existing
authorization integration guide, supplied as patch `387c23153386`. It labels
the correction as planned, preserves implicit All semantics and requires an
explicit owner change for affected configurations. It does not invent shipped
configuration syntax or present unsupported lifecycle coverage as complete.
The coordinator's record shows completed consumer configuration intake:
The pilot host and the autonomous host at their live revisions configure no
affected native hooks. Their `setup_hook`, `SessionHook` and `on_error` seams
are distinct. No current consumer migration is required; this is applicability
evidence, not runtime acceptance of the planned correction.

Candidate `29e28e9b36455ff1ce2a3fc4433088ef6c75a498`, tree
`ad57f31d140d9e580a7e3a297e26f87bafeb6054`, implements the shared scope
predicate, effective topology checks when an engine exists, distinct request
purpose, bounded feedback recursion and truthful PostLlm future-effect
withholding. Root and independent reviews (`3687ee34e460`, `6b5dca83d9f9`)
identify three remaining corrections: the normal factory drops override-only
hooks when base entries are empty; feedback serializes a null run ID from the
unstamped invocation clone; and provider-native names/list lengths remain
unbounded in the supposedly bounded notice. The source-reviewed private patch
`4f2e2830fd0d` adds a no-hook positive and override-only factory admission
regression in the existing fixture. It is expected RED, not executed.

SystemNotice remains appropriate for safe model feedback. Ordinary transcript
push and transient hook events do not establish required protected audit.
Reuse the existing input-row observation custody and transaction for bounded
typed hook facts; exact hook binding and observation-failure handling remain
missing. No new raw-output or audit store is justified. Owner-reported core
passes/Clippy are not independently inspected execution here, and the two
reported positive-confinement failures remain open. Migration docs at
`8fac7c917c5d42cd4fe65362eabef3c4d2db6d49` adopt the earlier subsection;
reviewed follow-up patch `b03d4b5b36e9` names candidate status and actual
`model_operations = "all"` / `"ordinary_only"` syntax without a release or
qualification claim. This hook slice is outside the finite Core 0.8.52 cut.

Later hook `6fbe763a097ba8c67d4c1d49d73ffc5e72c61f77`, tree
`80cd1a2a441d960284ad2d23e032ac299d8cdfd8`, is source-clear only for
retaining the admitted topology and recovering exact admitted ordinary gates.
Reviews `03ac45bda31e` and `b0c992b503a9` preserve the no-recursion and
future-effect withholding conclusions. This delta does not change the normal
factory gap, null feedback run attribution, metadata bounds or protected-audit
gap; the existing override-only RED is not adopted. New core-loop fixtures
are authored, not independently executed evidence.

The test-only successor `dbed2fd9531b93e5ec1b40fd7bafc5d90f0625dc`, tree
`2a82437f5b5de705154d2153149413a9fffcca03`, requires the correct typed
TurnLimitReached result for the explicitly capped fixture. Review `5444e5fb306d`
verifies that the one-controller-request assertion and uncapped continuation
remain intact. Exact-head logs record 13 selected passes, 2,077 skipped and
exit 0 after 2m32s compilation and a 0.783s summary. Raw SHA-256 is
`b701d57c02eefc738a0a81f791bc3070b3a2c5d7bb15a300f794a604e5a9b11f`.
The broader parent's two command-hook custody failures remain outside that
focused pass. A later raw read records workspace/all-targets/all-features
Clippy exit 0 at 17:59:03 UTC after 14m51s. Its header pins dbed but does not
separately record dirty state. Raw SHA-256 is
`6b04e6912201682c5b8892ddabbc122c992756d3a4237414bba01552d0ffb864`.
This does not accept permission-triggered termination or close the unchanged
production findings.

The normal-factory regression is subsequently adopted in test-only
`89104046adf52ead878840e7131e6faf3aac2ad9`, tree
`8a6749fa1cbcce4dbfcca7701e637a8d33c03caa`. Its test body matches the
delivered 4f2e2830 patch with only two formatting reflows. The corrected exact
test selection executes RED: zero passes, one failure and 15 skipped, exit 100
at 18:32:36 UTC. The no-hook positive completes first; the failing assertion
then observes a successful model response instead of `HookConfigInvalid` for
the override-only mandatory topology. Later no-publication and zero-request
assertions are not reached. Preparation takes 0.18s and the test summary 0.020s;
raw SHA-256 is `3af58d5e6dcedd97bb96f36e159ce6b7c17461103d549fef0a58e2f8d694823d`.
An earlier invocation spent 3m33s compiling but selected zero tests because its
filter used the wrong name, then returned exit 4. It is not failing-regression
evidence. The committed test and actual RED make the existing factory defect
actionable; no production correction or wider hook acceptance is established.

The production successor `a5be1ef3a78a2687309821743e3d5f6d4cc38c08`, with
docs-only head `dea26b748`, closes the factory composition, actual run attribution
and notice-bound findings in source review `5e7fc75b9102`. The existing regression
now passes, alongside engine-less admission, local continuation and PostLlm
withholding controls. The retained broad log records 2,419 passes, three failures
and 1,104 skips after 7m24s compilation and 193.107s test bodies; workspace strict
Clippy passes after 14m41s. The failures are the known reviewer URL fixture and
two required-confinement positives. This closes those three source findings and
the factory RED-to-GREEN transition, not full hook or confinement acceptance.

Protected-hook audit now has a source recommendation against clean
`dea26b748124d57bdd35a95b108a4a66e5fa154c`, independently reviewed in
`9038d32e47d6` and `4fc03bbdb758` and sent to both leads. Record bounded typed
withholding through retained work and the existing native input-row sink.
Reserve the actual candidate operation ID before PreLlm and carry it into the
request if sent; no extra authorization preparation, invented provider facts,
sidecar or transaction is needed. A feedback request is a distinct operation
and cannot supply the original audit record. Stage the observation before
feedback; a pending append becomes durable only through the existing input-row
transaction. PostLlm must retain the actual
responding request ID across controller recovery, preserving prior transport
outcomes and effects while recording only the later local work withheld.
Observation failure remains typed infrastructure with local operation handling.

The existing input-state format owner must version the new closed audit kind,
preserve supported old records and declare older-reader rejection of new rows.
Unknown audit records must not be silently dropped. Existing hook, native-audit
and persistence tests should cover absent feedback, replacement-request identity
and failed observation. This is a proposed owner correction, not an implemented
or executed audit path; its interim exclusion from Core 0.8.52 remains explicit.

The implemented Q3 candidate `50ce03bb614e2f75ed8dc93f0eacd5c0750dd3f7`
preserves that source design: it reserves the actual candidate ID before PreLlm,
retains the responding authorization ID across controller recovery, and appends
bounded typed observations through the existing input-row sink. The existing
format owner advances to version 6 with explicit supported migrations. This is
source review, not executed native hook-to-SQLite/restart acceptance.

One high-priority locality defect exists at that head: failure of the audit-only append maps
to typed `ObservationUnavailable`, but both ordinary hook callers then invoke
`terminalize_fatal_error`, ending the current run. The new fixture expects that
termination even though controller preparation remains healthy. The existing
owner received the correction: a one-shot append failure must preserve the
typed infrastructure result, withheld operation and known prior effects while
allowing independently permitted controller work to complete in the same run.
No false audit success or automatic retry is allowed. Persistent infrastructure
loss is a separate case. Review also found a stale runtime test that rejects
current version 6 while expecting version 5 diagnostics, and changelog wording
that overstates general input-row unknown-field rejection. These are source
findings; no new tests were executed during review.

Direct successor `3aab2261a0d2f6dfa6df65d0f5e4aab5822614ce` changes only
the stale version test: rejected versions are now 2 and 7, and the expected
current-version diagnostic is 6. This closes that source finding while
preserving supported historical records. The operation-local observation
failure and changelog findings remain open at that successor; no execution
result is added.

The later clean correction `d0067f6367fc0a515b39d89149d6a1bff1e16f57`,
tree `9b6c7ace66f2ad5756df2a7c3b4871f2c339c4eb`, closes both remaining source
findings. It attempts the protected append once, keeps the affected operation
withheld, reports `protected_record: unavailable` truthfully, and allocates
separate controller feedback. PreLlm and PostLlm both repoll instead of ending
the run for this isolated append failure. The new one-shot core fixture follows
the failed append with a successful later observation and controller completion;
it asserts distinct operations and no retry or false audit success. The
changelog now scopes unknown-field rejection to the closed audit structures.
Root and independent source reviews found no blocker in this delta.

The completed exact-head qualification now records the new one-shot append
failure regression passing in 0.396s. The combined core-library and hook-contract
selection records 2,129 passes and one failure after 7m44s compilation and
4.645s test bodies, exit 100. The all-target/all-feature package check passes
after 5m08s; workspace strict Clippy passes after 15m07s. Both existing drift
checks pass, with 51 protocol artifacts, 17 machines and nine compositions
current. These d006 results replace the earlier pending observation and ancestor
lint attribution. The retained 2,031 runtime passes still belong to ancestor
`3aab2261`, not d006.

The sole d006 failure is the required-confinement positive
`required_command_hook_enters_with_exact_argv_cwd_environment_and_custody`.
The model continued, but its guarded tool dispatched zero times instead of
once. The later source diagnosis narrows the cause: an ordinary entered-script
failure would fail `agent.run()` before this assertion, whereas confinement or
custody/setup launch refusal becomes local feedback and permits the observed
continuation. The fixture stops before examining that feedback, events and the
entry marker, so the exact refusal remains unknown. The existing test should
collect its recorded `guarded-call` result and `HookLaunchRefused`, `HookFailed`
and `HookStarted` events before asserting the unchanged expected dispatch count
of one. Its current event loop discards `HookFailed`. This needs no new runner
or production instrumentation. An actual ancestor `50ce03bb` run contains the
same assertion failure. A separate custody test in that ancestor run reports
`BackendUnavailable`; that does not prove this case's cause or a particular
host limitation. Keep this failed positive distinct from the new passing
append-locality control. No new execution, native custody/restart, full Q3 or
positive confinement acceptance is inferred.

Platform-layer operator-panel commit `d4a212db8c13` is source-clear in review
`0c98c5ce9405`. Raw path forwarding does not reproduce Part C's normalized-path
reparse defect, and restored panels resolve IDs against current configuration.
The iframe is explicitly trusted operator code. The flow-editor HTML still
needs the same framing policy as the console; separate secret-entry framing
must remain intact. Documentation needs byte-limit/whitespace precision.
Owner test passes remain reported, and actual browser/restored-layout execution
is not established by that earlier review.

Part D successor `b4fcd5fd36c741ebdcae38baafffb06ad781b8fa`, tree
`483f13352be27550a6979a1847918a1d3143d71f`, closes those source findings
(`67207caff012`). Rust and JS reject raw/encoded dot path segments, whitespace
and controls while counting UTF-8 bytes and preserving the literal path. Both
flow-editor HTML routes receive the same-origin framing header; secret-entry
policy is unchanged. New tests inspect the actual router headers and the panel
component's current-ID configuration on rerender. The tampered-layout control
does not exercise full dock restoration. Reported Rust 6/6, Vitest 780,
OperatorPanel 5/5 and Clippy 0 remain owner-attributed; no raw logs were read.

The later operator-panel origin ruling accepts same-origin scripts/forms only
as trusted application composition. That sandbox combination does not isolate
the parent console. An approval-card URL must resolve to an exact current
configured panel reference, including query and fragment, then open by panel
ID. Dynamic review parameters require an explicit bounded typed contract. Parent
`frame-src 'self'` does not replace each framed response's `frame-ancestors`
policy, endpoint authorization or CSRF checks. Intentional secret-entry
`frame-ancestors 'none'` remains intact. The reviewed b4 source did not implement
that proposed sandbox/CSP combination or URL-opening route, so this ruling adds
no implementation or browser acceptance. Coordinator decision 33 selects a
bounded existing-author port after privacy 0.8.49; its exact source and
qualification remain separate.

The follow-up navigation ruling accepts
`allow-top-navigation-by-user-activation` for that trusted panel. The flag
does not restrict the destination origin or path; trusted application code
must own the same-origin secret-entry route and bounded identifiers. The pilot host
PR 449 at `a848aa010b2cab5977dfb70210648e8b2a3960ee` is source-compatible:
its intent form and entry link use `_top`, the service constructs the relative
entry URL, and successful and refused secret-entry HTML keeps
`frame-ancestors 'none'` and `DENY`. Paired-device and unsafe-request origin
checks remain in force. Navigation grants no secret or approval authority.
This is a focused source and standards ruling, not browser qualification of
the pending MobKit port.

Part C review-link successor `43b5cf145980a1298c8f415781d8ae41e75fbb10`, tree
`c0f785aa4f9b8d816592e08ee5c48e91665e834f`, is also source-clear against
`5ddf8161` (`85a2626fb884`). It checks the canonical single-slash path after
joining and no longer trims incoming links before validation. Direct runtime,
RPC and restore use the same rule. The changed controls preserve audit/no-pending
refusal and successful restore, not whole-state immutability: direct evaluation
already allocates an action ID. Owner 29/29 and Clippy 0 remain reported.
Neither presentation change establishes native consent authority, deployment
acceptance or integration into the finite Core 0.8.52 cut.

Platform-layer Part B scope review (`861bd500207e`) is qualified GO with corrections.
The proposal's old core reference lacks the native consent branch's existing
full-record conditional commits, cross-process lock, typed unknown outcomes
and reconciliation. B1 must consume that existing owner implementation; file
and parent-directory synchronization remain missing at `6547b21d`. An unchanged
memory row after a store error does not prove that disk did not commit, and
visible bytes alone do not resolve uncertain durability. Keep this work on
actual approval transitions, with no R1 durable-write dependency.

B2 must compose `BoundOperationReview::with_human_consent` through
`AgentFactory::with_operation_review`, sharing the same live `ApprovalService`
with its authenticated decision ingress. Use `decide_consent` and the native
route's current eligibility evidence; generic `decide` rejects native consent
rows. Separate `approvals.view`/`approvals.decide` from generic gating grants and
authorize details before projection. Reopened records provide history, not live
candidate authority. A committed decision is distinct from notification and
physical entry.

The separate B3 generic correction can proceed with the existing platform-layer owner.
At `6f805c3b`, R3 sends before pending/audit insertion; refusals remove and
reinsert rows, changing retention order rather than permanently deleting them.
Decision notifications already follow the complete in-memory transition.
Commit affected state before external publication and keep uncertain delivery
separate from decision outcome. Snapshot export supplies no durability. If
foreign work leaves the mutex, exact state checks must prevent late receipts
from resurrecting settled rows. Preserve expiry and the original escalation
deadline. Root and independent adversarial review agree on this bounded source
direction; no implementation, new owner assignment or executed acceptance
is inferred.

The resulting B3 repair `c10cf3bd92bb2266eaf8f8183731fc4b208b64fd`, tree
`988c274fd3ad69c969e44b62c122741f02d83020`, is runtime source-clear after
root and two independent reviews (`1b17f4cf4d78`). It inserts the pending row
and creation audit before sending, records notification disposition separately,
and retains unknown delivery without retry or resurrection. The existing owner
reserves all four evaluation IDs before mutation; decision validation preserves
pending order, and one timestamp preserves the inherited escalation deadline.
The creation audit's delivery ID is now always null; a later acknowledged
notification supplies the separate reference. Qualify the documentation's
unchanged-on-refusal statement after already-due timeout settlements. These
are in-memory transitions and delivery-owner reports, not durable commits or
independently observed recipient receipt.

The three B3 controls are executed RED on `889c8136`. Exact-c10 raw logs then
record 2,483 passes, eight skips and exit 0 for the normal selection, followed
by Clippy exit 0. The separate ignored selection records nine passes and one
failure, with both new notification controls passing. Its `e2e_1201` pending
versus empty assertion also fails on the `206acff4` baseline; this does not
qualify the whole ignored suite or establish the failure's precise cause.
Final log SHA-256 is
`49beb2ee1faaedd106bfaa1e2febcf468fe2afb6b632c037773242f97b5a1dad`.
Normal compilation was 2.14s and bodies 36.473s; ignored compilation was 0.16s
and bodies 1.107s; Clippy took 2m20s. The tests inspect final state and do not
observe the instant of external send or execute ambiguous physical delivery.
The later A3 correction and B3 rebase qualification are recorded below; the
ignored-suite failure remains open.

Platform-layer approver-identity commit `cb623c72c98c9a7ed4420447ac5ffc3af164041d`,
tree `d1123eff060cd8816f7fa57dfb6120ba21075340`, has one material source finding
in review `b14e12076faa`. A request retains an immutable access view, copies its
held approver IDs, then waits for the gate owner. Access revocation can commit
while it waits, yet the gate mutation still trusts those old IDs. Refreshing a
new request does not close this in-flight race. Current permission, origin and
approver checks must remain ordered with the final mutation through the existing
owners, with external callbacks outside that reservation. Identity selection,
self-approval aliases, first-decision behavior and audit attribution are otherwise
source-clear within this generic-gating domain. Owner test counts remain reported;
real JWT HTTP revocation acceptance is not established. The finding is delivered
for the existing owner's correction; no competing production implementation.
The subsequently observed clean `cc4ec96be4be47635fd1ef3010e2d37f83f2b142`
adds escalation routing but does not change the access controller, HTTP handler
or observer placement relevant to this finding. It does not close A1 currentness.

The subsequent audit-stream delta `3c0fd67ec352d4cad47f21873ce5d19ee54f66d4`,
tree `4e033506c677fa934cf21a89b77ca367cbe4f22c`, also leaves A1 unchanged.
Root and two independent reviews (`c3094e3f3588`) found two new corrections:
the stream omits origin-attribute priming, so a label/role-hidden agent can
produce a data-free activity wake under broad allow; and console audit reads
silently treat malformed cursors as initial reads instead of the standard RPC
invalid-params result. Reuse the existing attribute projection and parameter
parser. Valid retained-cursor ordering is source-clear. Restored retained IDs
can remain valid across restart; documentation must not promise a gap merely
because their minting incarnation changed. New tests do not establish actual
eviction, lag or idle timeout/revocation delivery. No new execution is claimed.
The focused test-only patch `e45ab0f9ec8c` has independent static clearance and
exact forward/reverse application checks against `3c0`. It opens the real router
as the first console request with a cold controller and a live label-hidden
member, verifies actual audit creation without a hidden wake, then requires the
visible sibling's complete data-free frame. Cleanup precedes expected-RED
assertions. It is supplied for existing-owner adoption, unexecuted; JWT,
in-flight revocation and later-spawn coverage remain separate.

Committed A3 correction `206acff4d27d0d13f33232e51211f321c9f62454`, tree
`d9f41d55bef0eea5a943f681aa8cf3c1eafa0753`, closes the cold-cache and cursor
findings by source review `2a325c2da5d2`. It primes the existing attribute cache,
derives the access view after the last await, treats lag as a wake requiring
filtered audit reread, and shares the strict cursor parser. The real cold-cache
router regression is adopted. Follow-up review `5324328002d6` independently
reads exact-206 raw qualification: 2,481 passes, six skips, tests exit 0 and
Clippy exit 0. Test preparation took 0.17s, test summary 36.634s and Clippy
2m23s. Raw SHA-256 is
`855371cda3ad7e06b29ce26d479603dbe2383e8805f5adab00bf73f14bbdb9f8`.
These results do not cover the remaining disabled-controller case.

One introduced P2 remains: the stream discards an installed controller when it
is currently disabled. Later enabling that same controller cannot revoke or
restrict the existing stream. Retain the live controller and cover disabled-open
then enabled-deny through the same stream; the existing priming helper already
handles disabled policy. External cursors advancing only over visible entries
are a reasonable privacy tradeoff within the bounded audit retention. An evicted
cursor can still produce a gap resync, so hidden-burst silence is conditional on
retention. Remove the obsolete documented `lagged` frame reason. The periodic
check refreshes policy and approval expiry, not JWT authentication; owner waits
also prevent treating its interval as a hard latency bound. These focused
corrections are with the existing owner, without reopening A1.

Those focused A3 corrections are subsequently source-clear at
`6b44010f80cd8ed3e9a3f3a9136908dec215d83b`, tree
`071b365ec7216ab85b3e9eb1bfcc9f97c5ea0108`, in review `e6be0ebb43a3`.
The installed controller is retained, disabled priming remains cheap, and the
current view still follows all awaited reads. The new same-body stream control
fails against the reverted filter line with actual `gating.changed` instead of
expected `gating.access_revoked`, then passes on exact 6b. Raw review
`11647a324c26` records 2,482 passes, six skips and tests exit 0, with 2m26s
preparation and 36.589s test bodies, then Clippy exit 0 after 2m25s. Final raw
SHA-256 is `0ccb58edb8fa05c4cf5a67ef3083087aa0e4769cac05656c368916eda5ba2bf6`.
The RED header identifies the old head with the new test and old filter line;
it is not a clean-parent run. This covers revocation on a wake in the existing
stream, not idle-tick timing, token reauthentication or a new native permission
path. The documentation now states those limits and the retained-cursor gap.

Rebased B3 `a864506c4468beb71f47f60198f3c63f40a148a4`, tree
`47a9aec043bda721f254579a48ed4e8e1c70a06d`, is semantically unchanged from
c10 beyond that three-file A3 correction: both B3 commits compare equal, and
the actual tree delta matches A3. New raw results record 2,484 passes, eight
skips and default exit 0 after 2m26s preparation and 36.909s bodies. The separate
ignored selection still records nine passes, one failure and exit 100, with the
same earlier E2E-1201 pending-versus-empty assertion. Clippy exits 0 after 2m22s;
its log does not retain full flags. Raw SHA-256 is
`61c408d3c06d3149855fca995a3c20f07e9646b683c449facbff74edc71dc2aa`.
These are new exact-head selection results, not an all-green ignored suite.

The bounded B3 follow-up `75c5e50040ffecc4e58551dbfadd310614aaefdc`, tree
`b7e7ce4ecfbb31bc83abfa9f295297673e942b16`, is source-clear in independent
review `138d3b1353dc` and root inspection. Missing notification modules or
routing failure now emit `approval_notification: not_sent` after the pending
entry and `pending_created` commit. The observation retains the exact action,
pending and origin identities, consumes the already reserved fourth ID and
keeps the existing 512-record audit bound. It does not retry, resurrect or
renew the gate. The new missing-module control exercises the real runtime and
checks retained pending state plus audit ordering; it does not establish
delivery, restart or every routing-failure case.

The exact clean-head raw log records 2,485 passes, eight skips and default exit
0, including that new control. Compilation took 2m33s and test bodies 38.921s.
The separate ignored selection remains nine passes, one E2E-1201 failure and
38 skips, exit 100; the unchanged assertion observes Mallory's pending entry
instead of the expected settled timeout. Clippy exits 0 after 2m24s, without
full flags in the log. Raw SHA-256 is
`170103b9290ca58a9e4711b1f7b803721a9d7e76a08526e07bfc6dfac3ed71e8`.
This clears the narrow notification correction, not the whole ignored suite
or native human-consent integration. The branch remains outside the interim cut.

Private regression patch `2c185360d323` is independently source-reviewed and
delivered to the existing owner. It uses production JWT authentication and the
real runtime handler: retain Bob's authenticated view, revoke only his role
holding while preserving `gating.decide`, require refusal without changing the
complete gate snapshot or observers, then let freshly authenticated Carol
settle the same gate once with exact audit attribution. The empty runtime is
stopped before expected-RED assertions. Static forward/reverse application and
exact preimage/proposed-file checks pass against `cb623`; no Rust execution is
claimed. This deterministic authenticator-plus-handler seam does not establish
full HTTP routing or atomic ordering across a later handler await. The repair
must also release access guards before timeout and decision observer callbacks,
which can otherwise deadlock if they edit access.
The existing access state read guard can order that short synchronous transition
against configuration publication without taking the broader mutation mutex or
waiting behind configuration-file persistence. Final origin checks also need
ordering against the separate attribute cache. Derive the fresh view under the
retained guard without recursively acquiring it, and emit all reachable timeout
and decision notices afterward. This source-reviewed repair direction is sent
to both leads; it adds no cross-store durability guarantee or executed fix.

Committed regression `ef29972854b29bdf0b776d87ef2da8820f70eaf2`, tree
`d11c1e0bc7aca73cb955fc096e7ec726e75ba67b`, adopts that test with formatting
only. Its retained actual run is RED: zero passed, one failed, exit 100, after
2m23s compilation and a 0.029s summary. The intended post-cleanup assertion
shows the revoked role still approving. Raw SHA-256 is
`9585f477ca457bcdcf72d0401879baf8169fe03658d5e5b8695541fbad82df75`.
Review `9a5450e56720` distinguishes this committed regression from the separate
dirty repair run. This is an executed defect reproduction, not fix acceptance.

The committed repair `8de3d7a558dd23bb2b4ed2723b5fd0819e103f38`, tree
`fd0217a4758b62cfd4cc68865d1d92241761bd9e`, is source-clear for the authenticated
console decision path after root and independent review (`e3595de9f66b`). It
checks current permission, origin and held IDs after acquiring the gate owner,
then retains the real configuration and attribute read guards through the
mutation. Notices deliver after both guards drop. The exact-head raw log records
2,324 passes, six skips and test exit 0, including the revoked-role regression;
its 35.947s test summary follows a warm 0.17s compile. Raw SHA-256 is
`b43e8fd1de58a56a4f81c32e5d567f49c44610a0c68b96bfce8c0c39fefe1e25`.
The following Clippy fails exit 101 on the obsolete private observer `notify`
method. Full command arguments are absent from that log. This is functional
evidence and scoped source clearance, not all-gates green or cold-cache stream
acceptance. The stream regression remains unadopted in the inspected committed
refs. Legacy direct methods retain their existing notification behavior; the
post-guard guarantee applies to this new authenticated path. An authorization
refusal does not undo already elapsed timeout settlement.

The final amended repair `6d64858a262f5df3bc06dafbb58be6b67273295d`, tree
`60c238332ec393addd193e6aa38fafa6fb1affef`, removes only that unused method
relative to 8de. Review `8ecee584aed5` clears the delta. Its new exact-head raw
log records 2,324 passes, six skips and test exit 0 after 2m26s compilation and
36.338s test bodies, followed by Clippy exit 0 after 2m18s. The log retains
paired Core `11cdce1f0dccba9f2922689bc8051fd43ed28565`; full command arguments
are absent. Raw SHA-256 is
`0a1292849c544de9412b89ef58f1cbba152b27047e29e1dbdb4e5f63e961abe4`.
This closes the observed repair and lint failure. The separate signed-router
patch `38925fab464b` uses existing HTTP/live-runtime fixtures to check current
principal/role forwarding, forged-field refusal with unchanged state, and
correct approval attribution. It is prepared and statically reviewed, not
executed; it adds no duplicate revocation race harness. The separate comment
patch `8bf764c9b9d4` qualifies already-applied timeout settlements. Both are
supplied to the existing owner without reopening the 6d source verdict.

Those two patches are subsequently adopted at
`9ad3354fa7d377c2fa1eb6528e710cf077c20682`, tree
`c0c2ed7b844d8e092e90315f9f96295a1ae6fd70`. Review `82c0d98ff4af` verifies
formatting-only changes to the signed-router test and the exact comment patch.
The independently read owner log records the exact head, `clean=0`, paired Core
`11cdce1f0dccba9f2922689bc8051fd43ed28565`, and full command arguments. It
records 2,457 passes, six skips across 13 binaries and test exit 0, including
the new signed-router test at 0.199s. Compilation took 2m34s, test bodies
36.820s, and the following Clippy exited 0 after 2m26s. Raw SHA-256 is
`4c20ca708626e35857ce57d93914ddc3a3aef3de826e5dddd0e6d4c7032213b8`.
This establishes signed fixture JWT coverage through the actual HTTP router and
runtime. It does not establish a live identity-provider deployment or new
concurrency coverage. The later A3 count is a separate result.

Notification fixture patch `afcf0afb620b` restores committed-store message
equality around unavailable delivery on exact `f22bfc7c`. The existing no-turn
initializer supplies the missing stored session without starting a model turn.
Clean owner commit `056dbe9fe73fe93f87e5af12a8ec0a6f416d6015` adopts the exact
proposed RPC blob `7b2abfff1b3e51c35db677a3c3434a46915cc3ee`, independently
verified after commit. The new assertions remain unexecuted and do not repair
actor teardown. Independent source reviews locate the join in the existing external
post-stop cleanup, retaining the exact task across caller cancellation and
preserving ordinary nonblocking discard. The existing gated compaction-abort
fixture can hold the actor after its command receiver closes, providing a
deterministic test that terminal cleanup waits for actual hosting release.
Source-reviewed private patch `8e44d65b5e76` adds that expected-RED control in
the existing facade test module. It drives actual persistent unregister cleanup,
observes the cleanup future while the actor is held during two control calls,
checks early hosting refusal and later reconstruction, and preserves an exact
nonempty transcript without starting a model turn. Review corrected its initial
module placement and cooperative-scheduling assumptions before delivery. No
Rust execution is claimed. This distinct-owner, same-process fixture does not
establish separate-process restart or Notification authorization.

The actor-teardown WIP on committed control
`cd21407d2c857c29ee32500336be0463fdb6f9e6` receives a narrower source ruling
in review `843ccdde47bc`. Explicit awaited `try_shutdown` is the legitimate
same-process handoff boundary; Drop only signals, and the hosting claim remains
with the actual actor until it exits. Do not add a second best-effort shutdown
after an already asserted typed shutdown result. The proposed external join can
currently be skipped when an earlier nonjoining callback marks cleanup complete.
Separate callback completion from waiting for the exact actor, without repeating
the callback or joining while a finalization boundary, map/mutation lock, or the
actor itself is held. Retain the join across caller cancellation. Callback
failure must not manufacture hosting release. The existing two-command actor-exit
control remains the appropriate test; no new execution or final WIP acceptance
is claimed.

That exact adopted actor-exit control subsequently executes RED on clean
`cd21407d2c857c29ee32500336be0463fdb6f9e6`: zero passes, one failure and
506 skipped, exit 100 at 18:16:56 UTC. The assertion observes cleanup returning
while the original actor is still inside its first abort. Compilation took
4m32s and the test summary 0.369s. Raw SHA-256 is
`f20fb54aa319ee0d888f0d11167e89414c0f8974afa9ab722dd45302fa7b4be9`;
review `a6dbde7557b9` retains the selection and source limits. At the subsequent
source snapshot only CHANGELOG was modified, so that checkout did not provide
a corrected join implementation to review. This does not establish stopped or
lost work; the executed baseline failure is distinct from repair acceptance.

The next owner run, `teardown-green-attempt1.log`, still fails that actor-exit
oracle. Its header identifies cd214 plus eight dirty files at 18:17:50 UTC,
without hashing those dirty bytes. The five-package all-target/all-feature
check passes in 8m50s. The facade selection then records 35 passes, five failures
and 467 skips, exit 100, after 4m20s compilation and 0.434s test bodies. Besides
the original actor-exit assertion, three governed-reviewer cases stop at a
duplicated `/v1/v1/responses` fixture endpoint, and required command-hook custody
reports `BackendUnavailable`. Those four failures have no baseline attribution
from this log alone. The subsequent session selection has no terminal result
in the retained snapshot. Review `baa3671d7944` retains the raw log, SHA-256
`53f427f3f969d421ca86931aa731507f486fb82187e59aaa671468240af48ba9`.
The independent 18:30 source snapshot contains the same semantic join WIP,
including the callback-complete early-return bypass. Passing compilation does
not close that source finding or the failing actor-exit control.

Attempts 2 and 3 subsequently pass the actor-exit oracle in 0.402s and 0.365s.
Attempt 3 records 36 facade passes/four failures and 31 session passes. Its
header names cd214 plus seven dirty files, without hashing the tested bytes.
At 19:40 UTC, independent review `5e7fc75b9102` still finds two incomplete
paths in unchanged working diff `251fee7f098b`: prior boundary-owned callback
completion lets later external cleanup skip the join, and cancelled exact-actor
cleanup can return on an absent actor without awaiting its retained join.
The passing oracle covers neither path. The new exact-registration shutdown
ordering and ordinary joins are source improvements; they do not close these
remaining completion gaps. Three redundant shutdown calls also weaken first-call
completion evidence, while another fixture now gracefully unregisters the
durable Stopped state it intends to test after a crash. These corrections remain
with the existing owner; no additional test runner is needed.

The 19:51 UTC capture of the same attempt adds 258 RPC passes, four failures
and 447 skips (exit 100), followed by 89 runtime post-stop/unregister passes
and 1,941 skips (exit 0). All four RPC failures stop at the unavailable
`mcp-test-server` fixture before their behavior assertions. RPC compilation took
10m37s and its test summary 1.976s; runtime compilation took 3m02s and its
summary 2.286s. Review `0eeb6f4a2fd3` retains the raw result anchors. The same log
subsequently records five-package all-target/all-feature strict Clippy at exit 0
after 10m46s and three-package WASM Clippy at exit 0 after 3m16s. Committed
`33039d38dc603a09c2139e759d130485aadf7ec9` preserves the inspected production
behavior with formatting changes only; both caller gaps and the two test-quality
findings remain. Later owner work is separate from this committed verdict. These
results do not establish complete teardown qualification.

Successor `ed17752665198fd865a37d7b0671d89705d1a068` corrects all four scoped
findings in source; clean merge `833b3e955` preserves those changes. Review
`e0da985d512c` verifies independent completed-cleanup joins, cancellation-safe
exact-actor selection and the corrected fixture premises. Two focused regression
patches are independently reviewed and delivered, but remain unexecuted. The
existing attempt 4 names clean 833b. Its 20:49 UTC capture records the
five-package all-target/all-feature check passing after 8m50s. Four selections
complete with exit 0: facade `jsonl-store` 32 passed/475 skipped, session
discard/shutdown/unregister 31 passed/404 skipped, and RPC
session-runtime/session-executor 271 passed/438 skipped, plus runtime
post-stop/unregister 89 passed/1,941 skipped. Their respective compilation times
are 4m26s, 3m26s, 13m02s and 4m38s; test summaries take 0.375s, 0.012s,
1.934s and 2.289s. These are 423 total passes; only the 89 runtime cases are
new since the 20:36 capture. The named no-turn reconstruction/cancellation and
projection-halt/event-cursor controls pass. The sequence subsequently completes
native all-target/all-feature Clippy after 13m09s and three-package WASM Clippy
after 3m06s, both exit 0, ending at 20:54:04. Completed raw log SHA-256 is
`469abcfa1951bf63b72acc2dff1a156f55491f04e7b27afe437c50150eace105`.
These selections do not rerun the earlier all-feature custody and reviewer
fixtures. The log begins clean at 833b but has no source/status fields at END.
The subsequent 20:59 dirty capture contains the two delivered regression tests,
a test-only helper and a comment correction, plus deliberate RED mutations for
the completed-cleanup and absent-slot retry defects. Its full-index diff hash
`bf5b64328c6bbc650e2162cf93b75e8ca4d19bcdb134bdb3a3e0cd3a73bfd575`
matches attempt 1's `a2364c281b12` header when object IDs use the owner's
nine-character abbreviation. The runtime control fails at its intended
early-return assertion, exit 100. The first facade attempt instead fails during
replacement setup before its assertions, so it does not establish that RED.
With the fixture corrected, attempt 2 reaches and fails the intended absent-slot
exit assertion, exit 100 at 21:03:22. Its raw log SHA-256 is
`4ac64cedba9d08f92e42cbd0f9bc22bb7e0fbf6baf99cbd6ce61fff6df7f6479`.

Commit `9dcf2081da69431bfd65509b846cb25735bbafcb`, followed by integration
merge `7311b68b71823bd8622b84baa2801aa03bc6d16a`, adopts those controls with
both production mutations removed. The fixture correction explicitly retires
actor A's actual runtime registration before materializing a new same-ID
actor B registration; the durable session authority is retained. All
early-return observations are retained before that step; stale A
cleanup must still leave B's exact witness and liveness intact. It neither
fabricates a resume receipt nor removes the replacement oracle. Source review
accepts that correction. Attempt 5 starts at the clean merged head at 21:04:36.
The completed log records the five-package all-target/all-feature check passing
after 7m33s. Runtime records 99 passes and 1,932 skips after 3m12s compilation
and 2.285s test bodies, including the new completed-boundary control. Facade,
governed reviewer and process custody record 42 passes and 466 skips after
2m56s compilation and 0.408s test bodies. Both exact-actor cleanup controls pass.
Session discard/shutdown/unregister records 31 passes and 404 skips after
2m00s compilation and 0.013s test bodies. RPC session-runtime/session-executor
records 271 passes and 438 skips after 11m06s compilation and 2.456s test bodies.
These are 443 total passes, with each selection exiting 0. Native strict Clippy
passes after 11m07s and three-package WASM Clippy after 57.87s. The sequence ends
at 21:43:40, 39m04s after its start. Completed raw log SHA-256 is
`f299c5183ca420bf172909ab0753920aa8a6e13b143668d084b3d1f889cbdc5e`.
These results belong to recorded clean 7311. The later observed clean owner
checkout `4e7ce0be0d1c0af678be151027c94e8490f44ed2` is not qualified by them.
The prior 423 passes and lint results retain their separate attempt-4 attribution.
MCP and ephemeral cleanup still have concrete unjoined actor paths. RPC's
logical-session join has narrower identity guarantees, and boundary-held
rollback now indirectly joins through unregister, invalidating its old no-wait
comment. Follow-up review of clean 833b finds no current production actor cycle:
the persistent facade's eight rollback callers pass the non-boundary path, while
boundary-held Mob and ephemeral rollback retain handles whose new exit callback
is still empty. Committed Mob attachment retirement releases the boundary before
waiting. No replacement interference or rollback deadlock was demonstrated.
The existing Mob/ephemeral exit-coverage work must account for those retained
boundary callers when adding a real join. This preserves the uncovered exit
paths and corrects the generic contract caveat without adding an acceptance gate.

A separate native budget-terminal defect has an executed RED at
`f390e3bcb70d3106abb862b3c17d623636e31620`: the runtime produces a typed
`BudgetExhausted` result but its handwritten successful-commit checks reject that
machine outcome. The retained log records one failure and 705 skips, exit 100,
after 14m22s compilation and a 0.202s test body. Its parent-head dirty diff
matches the committed RED exactly; this is source-equivalent evidence, not a
clean committed-head rerun. Source ruling `e0af4c12c7a8` selects a distinct
private budget commit effect while retaining the generated public `Completed`
delivery class and unchanged budget cause in `RunResult`. Exact terminal-run
ownership must be checked before `RunCompleted`, which overwrites that field.
Preserve the actual failed/budget terminal tuple, ordinary input consumption
and finalization recovery. The existing author retains implementation and tests;
no GREEN or release inclusion is established here. This orderly budget stop is
separate from permission refusal, which remains operation-local feedback.

The committed successor `11b9d30eb8794dd1fd02f34bc90825ced9c223d8`, tree
`0c3adfd031327adf01dfb8d8d2fc14c428d45c03`, has completed bounded source
review with no concrete production defect found. It captures phase, outcome,
cause and terminal run before previewing `RunCompleted`, carries a distinct
private budget effect, and rechecks the live coherent budget terminal before
durable persistence. Generated public delivery and the typed result cause stay
unchanged. The exact-head package check now passes after 7m41s. The RPC
regression compiles in 11m30s but fails after 0.69s, exit 101, on its expectation
that the new assistant reply is durable. Earlier typed-budget and input-commit
assertions pass; the later Consumed-row and following-turn assertions are not
reached. Unchanged Core source charges provider usage, observes exhaustion and
returns before constructing or appending that response. The fresh result is
therefore expected to be empty by source, but the test did not record its text.
Correct this oracle around charged usage, the exact durable user input and
absence of uncommitted assistant text while retaining the over-budget stimulus.
This failure does not show persistence losing an already-committed reply.

The runtime library selection records eight passes and one failure after 4m28s
compilation and 0.01s test bodies, exit 101. The successful budget commit,
one-consumption/receipt, terminal-coherence and time-budget exclusion controls
pass. The failing atomic-apply fixture retries the same persistent driver and
finds its input already Consumed. Existing persistent terminal handling explicitly
requires cold reload after commit failure; it does not roll back the live shell.
This direct fixture also bypasses registration and has no shared durability-health
gate. Its in-place retry premise is invalid. Keep the injected failure and check
unchanged durable truth; use the existing registered-machine fixture to check
reload-required authority and rejection of later mutation. No new production
rollback or automatic retry is justified by this failure.

The independent `core_apply_terminal_truth` selection passes all three tests
after 0.27s compilation and 0.04s test bodies. Exact `11b9` runtime/RPC all-target
strict Clippy subsequently completes in 9m31s, exit 0, at 22:13:33 UTC. The
completed raw log has SHA-256
`7d44354e1d76bc35a0984b34f15d3dcce7caf82ff61e186a9318af592f41750e`;
the earlier failed test targets remain failures. Receipt assertions
remain machine-level coverage, not an RPC disk-restart test. The failed retry
helper passes no terminal result candidate, so it also does not establish
post-commit finalization recovery. That separate scenario remains in existing
completion tests. These first-attempt results did not establish GREEN, benchmark
acceptance or release inclusion.

Test-only successor `4185915b2a050cfb58fbd402113868e0123cb0e3`, tree
`d0638bd4ee2b6c7305ddffb2793350db09c488c9`, removes the two invalid
expectations. Independent source review finds two remaining assertion gaps in
the revised bodies: the RPC test only comments on reply absence, and the failed
atomic-commit test permits the already accepted durable input row to disappear.
Assert the rejected reply is absent and the fresh result text is empty; require
the durable row to remain present with its pre-failure state. Preserve the
existing stimulus, failure cause, budget terminal and continuation controls.
Describe this failure as an atomic terminal-boundary commit failure, distinct
from finalization after a successful commit. No production rollback is proposed.

The subsequent completed attempt records clean `418591` at every command
boundary and ends at 22:31:10 UTC. The RPC budget regression passes, now reaching
and passing its durable Consumed-row and following-turn assertions. The runtime
selection passes all nine tests, and `core_apply_terminal_truth` passes all
three. Their compilation times are 3m38s, 1m44s and 0.13s; test bodies take
1.15s, 0.01s and 0.04s. Package check and strict runtime/RPC all-target Clippy
also pass after 1m58s and 9m25s. The completed raw log has SHA-256
`31d19dfd7f1f10995f005daad2a8bb88f9b0ea208e6f5c39f1072ee93a96f726`.
Existing machine and protocol drift checks then pass on the same clean head at
22:34:43 UTC: 17 machines, nine compositions and 51 protocol artifacts are
current, and the post-check worktree is clean. Drift-log SHA-256 is
`c9aae83ff0dbaa0b6e832e36171bea84d8496fed4b16c1b01874116214803409`.
These are executed passes for the existing test bodies. The two assertion gaps
above remained open at this checkpoint; the run does not establish registered recovery, post-commit
finalization retry, performance acceptance or full platform completion.

Test-only successor `984f342ec95def1e8b69335acfb49ba17f7c3d93`, tree
`8a1afbf75156ba594b9c7bb2b687feb6d236793a`, closes both assertion gaps in
source and executed tests. The RPC body now requires rejected reply absence,
empty fresh-result text and actual charged usage, while preserving consumption
and following-turn controls. The atomic-failure body requires the same durable row
to exist before and after the fault, preserving its typed phase and terminal outcome. Its new completion
test commits the real typed budget result before faulting the receipt-finalization
write, then recovers the exact durable result without executor re-entry or
duplicate consumption. Both the direct-driver and cold-reconstruction branches
pass. The fixture has no shared durability-health handle or completion waiter;
this proves the bounded durable recovery path, not registered in-place retry,
waiter delivery or an RPC disk-restart scenario.

The exact clean-head attempt records one RPC, ten runtime and three terminal-truth
tests passing, all exit 0, by 22:47:20 UTC. Compilation takes 3m22s, 1m43s and
0.14s respectively; test bodies take 1.21s, 0.22s and 0.04s. Package check also
passes in 59.78s. Strict runtime/RPC all-target Clippy then passes in 9m27s;
the full attempt ends at 22:56:48 UTC, 15m36s after it started. Completed-log
SHA-256 is `55e722878263a7990ac120b5d9310797872b14eafc9a823889c6041eabf267cf`.
No new drift result appears in that log. The three-file test-only delta does not
justify a new request to repeat unchanged generation checks; earlier drift
execution retains its original source attribution. Coherent integration,
performance, full coverage and publication acceptance remain separate.

The test-only successor `1ab498a3359c52dbe40433f524b4cafaaa800a44` adds a
completed first turn followed by a second-turn budget terminal. It requires an
Ok typed BudgetExhausted result, exactly two Consumed durable rows, the retained
first reply and second prompt, and absence of the rejected second reply. The
clean-head log records both selected RPC tests passing and RPC Clippy exit 0,
ending at 23:11:14 UTC. Build/test time is 3m18s/1.35s; Clippy takes 8m51s.
Cumulative usage and the second model-call count are fixture premises rather
than direct assertions, so premature second-turn exhaustion remains an oracle
limit. This is a test-only addition to accepted 984 behavior. Raw-log SHA-256 is
`de62f0aebc35760c8d5cb6ef20ce66bbe6b8dab5715c14f39b0892d85bccafdc`.

Released application-policy resume paths have two additional source findings.
Both candidate `1c83b02aaccab2924a9847da929e41b7840973e2` and the Core 0.8.51
release at `1d5d844c8a75a70cbe90e62673dca5cd3621744e` omit the live host policy
registry during ordinary cold mob-member reconciliation. Factory-backed members
with a persisted Provider binding can therefore fail to resume. Explicit child
resume can also lose a current tighter application binding when the factory
replaces it with the older durable binding because explicit override intent was
not retained. These are application consequence-policy defects; this review does
not establish a bypass of independent native tool-access restrictions or a
deployed reproduction.

Coordinator decision 36 accepts the bounded local correction for Core 0.8.52
under the existing owner. Preserve optional current policy intent through local
spawn and deferred-resume recipes: None keeps the durable binding on resume;
Some applies the explicit binding and its existing override mask. Always forward
the current host registry. Document the public Rust field migration to Some;
the stored concrete binding does not require a new schema for this correction.
Actual post-factory cold and explicit-resume denied/permitted controls remain
required, without treating their proposed bodies as executed results. Placed
resume still loses optional intent through its concrete portable overlay and
needs separate coverage under the existing placement owner. The broader child
storage and root-ownership feature remains a later slice. This decision adds
release scope, not implementation, execution or release acceptance.

Local correction `7828e1cdf3af4d107921de58290b445fcc1274a9`, tree
`d5188efd6a0886b20500f15d7a83532769bf8175`, now has independent source
clearance for the intent helper, lifecycle propagation and test assertions.
It repairs the existing caller handoffs without changing durable schemas.
Validated desired-member material intentionally becomes Some(binding), including
Unmanaged, because it already represents a selected policy. Four new real-factory
cold/explicit controls exercise physical denied/permitted dispatch and completed
turns. The warm control verifies registry presence and no override bit in the
actual rebuild request. Explicit Provider-to-Unmanaged resume lacks a
discriminating control; its unconditional Some handling is source-correct.

The earlier broad verification fingerprint `2c4b1fab18f8` has 13 child-policy
passes and one identity-setup failure, while child/restore 15, Mob 347, check and
Clippy pass. Later GREEN `da8005cb455d` passes child policy 14 and warm revival
1. Its retained patch independently matches the reviewed candidate except for
one test assignment's line wrap. This is source-linked focused evidence, not a
literal clean-head run or a wholly green broad attempt. RED `6c4ac21e2bb1`
records the five intended missing-registry/ignored-policy symptoms; its mutation
patch is not retained. The existing hooked push and candidate CI remain with
GCP. The GREEN log SHA-256 is
`53ff7f8cfa8ad1da7441938b6549e58b7739cba2ecc5d2e8c0c8e14b8f41eec2`.

These tests replace service objects within one process and retain the in-memory
RuntimeStore. Existing already-live reseating performs no rebuild and does not
apply a changed explicit policy/registry. Hot reconfiguration and the documented
placed-member intent gap are not fixed by this local correction. Final platform,
performance and publication acceptance remain separate.

The post-.52 child-mob r2 design review accepts canonical owner observation:
Committed permits normal activation; Archived permits the existing destroy
path without starting members; Absent/error retains visible, unactivated child
debt and healthy siblings. A plain activation-hook error cannot represent this:
it currently escapes before management and aborts the whole restore. The
existing builder/lifecycle seam needs a typed withheld outcome. Already-admitted
destroy recovery continues independently of new owner uncertainty.

The same review selects one explicit stand_down join/report for ordinary and
failed-bootstrap cleanup. Its lifecycle owner must close admission and task
registration together, settle in-flight creation and descendant tasks, preserve
actual shutdown outcomes and retain the root claim through cancelled callers or
failed-object drop. Cleanup bypasses ensure_restored and preserves durable child
data. Expected child debt is a per-child result; a genuinely fatal partial
bootstrap uses that same cleanup path, without claiming atomic reversal of
already executed effects. This is reviewed design, not implemented or executed
recovery. The coordinator's post-.52 scope and D1 publication dependency remain
unchanged.

The interim MobKit WIP on `07674b63dbcc`, tracked diff
`fa8be49c92242a9a238fb8298b82fc6c3f5dbc905a44b5a992bbeffea4953d0d`,
cannot release the child-root claim after only a clean actor snapshot. Two
independent source reviews confirm that retained state still owns persistent
custody sweeps and an admitted council can publish a newly created child after
that snapshot. Keep the existing process-retained claim and report retained ownership
until Core's state-owned close/drain is implemented. Same-process AlreadyClaimed
is the truthful interim limitation. Durable opt-in does not make unjoined work
settled. A later call must also preserve the meaning of earlier unsettled debt:
that version's closed-channel-as-success branch could discard pending
unregister/effect debt and release the claim. Core already joins ordinary
concurrent shutdown requests; that is not an additional finding. These are
source rulings, with no new execution or .52 release gate.

The follow-up two-variant contract is acceptable: HeldUntilProcessExit means
the observed child actors are clean but the Core state drain is unavailable;
Retained reports specific or unknown child debt. Both retain the real claim.
Actor-only settlement must not make the overall cleanup-completed attestation
true while state-owned work remains undrained. Identical repeated diagnostic
text is not required: unknown/unsettled remains truthful and keeps ownership.
Core already retains its actual shutdown report privately; a second authority
ledger is unnecessary. The new ownership and process-lifetime test bodies are
source-clear. The later tracked diff
`48a05d5d17dc5c0d5dd14d18ac9f4951b70d1866a40cbd9d11f588859eda4c59`
corrects the overall cleanup attestation and removes the clean-by-MobId cache;
a replacement or closed actor cannot inherit its predecessor's clean result.
Its native-report projection includes only non-clean members, and the test now
requires the held worker while excluding the clean sibling. No blocking source
finding remains in this delta. An explicit native-report prefix assertion is
optional test hardening. The owner may retain stable historical diagnostics and
its equality assertion; neither is a required contract or release authority.
The untracked storage and test file SHA-256 values are respectively
`2cea50b57150e62bb031730111f9e57c5e5383bef9060bbe735670b84efc8782`
and `6d7ab486c1ba761b5d34928cc152896b95d0c87dbb0c9ea8b39e575b62ac57dc`.
These source changes remain unexecuted pending the owner's Core repin.

Explicit ProcessBound remains a supported long-term choice. The definition-based
`.persistent_state()` convenience builder currently supplies in-memory mob
storage, so durable-child support through that path remains a later coverage
item. Its durable composition needs actual custody and matching provenance
before that behavior is advertised. The legacy `.mob_spec(spec)` path already
preserves a supplied durable composition; constructor names alone do not choose
child durability. Root provenance is required when forwarding root participant
custody, not universally for every composition. Keep the convenience-surface
gap visible without silently changing the default or inventing a new storage
authority. New untracked WIP files are individually identified in the review
capture; the tracked diff hash alone does not identify them.

Review of the voice WorkGraph recovery proposal at source `35971af39c80`
found that releasing unknown work to Open would authorize another execution,
and generic caller-writable evidence could not establish the item-to-operation
mutation binding. Retired snapshots remain available to the existing exact
startup inventory; an indiscriminate session sweep is not justified. The review
was delivered to the existing lead with current-policy completion and missing
result evidence kept distinct. This is design feedback, not an implemented or
executed recovery slice.

The later voice save-cost and durability r6 proposals (`d4153a5f065e` and
`de5ae42487d1`) were retrieved and reviewed against source `c6225dd4a818` and
the unchanged relevant behavior in integration `9dcccd93fb32`. Review
`6f0a0456bce92` is delivered to both leads. It retains staging under the existing
owner but requires exact replay-byte identity instead of the semantic transcript
digest, full body accounting including the anchor, separate assistant admission
and terminal durability, and reconciliation after commit-then-error or cancelled
acknowledgement. An unconditional newest-edge exception makes a byte cap soft.
HeadCanonical already has the bounded suffix/head commit; a WholeBlob suffix
needs an actual atomic base/cursor/writer contract, with crash/reopen coverage
in both persistence profiles.
These are source findings. The proposals do not independently establish the
incident's causal timing, a threefold speedup or executed durability acceptance.

The native Notification working-copy fixture at blob
`3665965ac602a0a8c77cfaf1ab32ee9b94abd294` is source-clear for exact-emission
settlement checks. It derives the request digest from the actual notification,
requires one matching audit record, and checks the actual appended system
message or its absence in the same committed snapshot. Cold settlement retains
the emitted job ID. The earlier blocked-pass assertion remains runtime-level,
not per-job classification evidence. These new assertions are unexecuted.

Shared-string commit `6227a77a022022447161ac8d91da81568bcb0d37` and clean
successor `c3aee75985d8d240bd6440e603dcbdd58527cf48` correct the validator
fixture's diagnostic lookup, but still expose a physical shared-value map at
whole-map escape sites. `B-tests3.log` records 26 passed/0 skipped (2.34s build,
0.010s test bodies). The log lacks exact command/source and explicit exit-code
headers, so it is narrow test evidence rather than qualification of that head.
The existing owner's first `B2-tests.log`
ends with 157 passed, one failed and 1,910 skipped after an 11m24s build and
0.089s test bodies. The new `shared_value_maps` fixture fails at line 195;
`B2-clippy.log` also records an owned-comparison diagnostic in that fixture.
The first log lacks an exact command, numeric exit-code receipt and compiled
working-copy digest, so these counts are not exact-source qualification. The
second focused schema run is now complete: the corrected probe passes one test
with zero skips after a 5m41s build and 0.007s test summary. Clippy2 finishes in
3m09s without diagnostics, but its flags and numeric exit receipt are absent.
These executions precede clean commits `f10a3b8ca888cd4802ab64c1817fdcbe2c82de8f`
and `0c3b3dcdccdab4fbb00a230601186bef15c4a21a`; their built-source association
remains owner-attributed. Root and an independent reviewer clear the committed
generator/test/Bazel delta. Whole-map helper/effect types and exact payload/count
assertions are covered; the borrowed getter's physical receiver is source-clear
but that branch is not exercised by the typed probe. The single-probe pass does
not replace the earlier whole selection's failure. Private duplicate editing
has stopped. No accepted quiet overhead result follows from these corrections.

The subsequent committed-head qualification at full `0c3b3dcdccdab4fbb00a230601186bef15c4a21a`
now passes the complete targeted selection: 158 passed, 1,910 skipped, RC 0.
Root and an independent reviewer read the retained `B2-head-tests.log` and
`B2-head-clippy.log`. Both headers record that full head, `dirty=0` and an empty
diff digest. The exact release nextest filter selects 26 DSL-core tests, the
shared-value schema probe and 131 runtime tests; the formerly failed probe
passes. Compilation took 11m29s and the test summary 0.087s. Release Clippy for
the same three packages' libraries/tests with `-D warnings` also ends RC 0,
with 4m39s Cargo time and 280s wrapper time. Test and Clippy raw SHA-256 values
are `e392370176d5524d221396347f03c09c4d6baad6dab31633dd5d16fc766786c6`
and `260f594f4d3bb86a76d187cffc114bfd424832b4cc92df24d8eaa668a7caf9e4`.
The source attribution rests on those retained execution headers; no executable
hash is recorded. This closes the missing successful complete targeted rerun,
not full workspace qualification, G1/G3 implementation or performance acceptance.

The first G1/G3 qualification attempt is a terminal compilation failure, not a
test result: `G1G3-tests.log` ends RC 101 after 782 seconds, 16:08:00Z-16:21:02Z.
Its header identifies base `0c3b3dcd`, 11 dirty paths and diff SHA-256
`a83308f01b2556e48e4deea058c486dfab4442002f68e7e99ef35430dff6ea18`.
E0507 at `crates/meerkat-authorization/src/grants/tests.rs:727` attempts to move
`parent.grantee` out of the new dereferenced record. The exact retained raw log
SHA-256 is `7e8c8dc4d7e1ed06e51d3d09adc4d40603046bcf8aab67ef9f6310d9b7ab779d`.
No duplicate build or author was started. Prior B2 passes remain valid for
their source and scope.

The corrected owner attempt `G1G3-tests2.log` records 512 PASS, 1932 skipped
and RC 0 across the selected six-package release lane. It took 13 seconds,
16:25:18Z-16:25:31Z, including 11.07s compilation and 0.731s test bodies.
Its header retains the exact command, the same `0c3b3dcd` base and 11 dirty
paths, with corrected diff SHA-256
`a347b4ab3b65265a52e9492b465c9edfde93e315c7adb65260faa5eab85f8890`.
Raw log SHA-256 is
`bf5b2f3dd658534f4dade5d9f69fa0171eca26609af012e537be1cb4f139c888`.
This is identified work-in-progress execution, not a committed-source review,
Clippy pass or accepted overhead measurement. The earlier failed attempt remains
retained; no matching live job was observed at 16:33:06Z.

The first subsequent G1/G3 Clippy attempt fails RC 101 on a generator semicolon
lint after two seconds. The second ends RC 101 at 16:54:10Z after 8m24s, with
two auto-deref lints in the grant tests; raw SHA-256 is
`3d14cb72abc223ea1d74479554ad52556826b441685bb674f3baf5a740247b72`.
The third invocation now ends RC 0 at 17:01:40Z after 349s. Its raw log
SHA-256 is `9a969156b024a025c1ef5ab21bb1c890c24f61df6716febd89d0aed4162b9ac5`
and retains the exact eight-package release library/test Clippy command. Its
working-copy diff is
`76de347eae0887b51de072bdb1c4a280e8f1126363e5dc28f2ab35be26828b4f`,
so the earlier 512-pass receipt is not a pass for these later bytes. Stable
source capture at 17:05:23Z is independently reviewed: G1 shared grant records
and logical operand borrowing (`73067bf5f792`) and G3 exact successful JSON
hash reuse (`df2575cf366f`) are source-clear. Full retained-value equality,
mutation isolation, wire shape and native permission/publication/clock/entry
checks remain. Rust struct-literal construction of GrantRecord changes despite
preserved serialization. No committed successor or new paired measurement was
present at that intake. Existing performance misses remain open under
the coordinator's interim-scope ruling; source clearance and Clippy do not
establish the optimization's net benefit.

The subsequent committed G1 `a25062f4f36ee16955b42ffe2dc060a11e0a9b0c`
and G3 `34716033ced5c6dfbb7b73f0095b8912c0631fe7` differ from reviewed
76de only in formatting. The exact clean candidate release build ends RC 0
after 503s (Cargo 8m22s); both existing native-cost correctness selectors pass
with bodies of 0.03s and 3.88s. Intake `c463a3b741b0` retains the build
header, Cargo success/artifact record and terminal selector logs. It does not
transfer the earlier 512 passes to this candidate. At 17:23:40Z the existing
wrapper and Cargo process were still building the clean 0c3 baseline; their
actual command and working directory were read. No new paired measurement is
available at that snapshot, so it does not establish representative mean or
dispatch-p99 acceptance. A later paired diagnostic is recorded below.

The delivered G1/G3 comparison is independently checked in review
`87d9aed37121`: all 16 runs pass with RC 0, and all 160 CSV rows match the
raw JSON values. Eight alternating rounds provide 40 trusted and 40 governed
samples per arm for the depth-1 FreshAdmission diagnostic. Baseline and
candidate build headers bind clean-start 0c3 and 347 source, with terminal
build durations of 504s and 503s; binary hashes remain builder-recorded.
Recomputed ratio-of-means overhead improves from 58.6484% to 41.1761%, with
governed means of 4.922806ms and 4.339194ms and trusted means of 3.102967ms
and 3.073604ms. Recorded one-minute load is 14.05-15.32; values near 22 are
longer load averages. Measured process walls total 12.18s and 11.77s,
excluding builds. This supersedes the no-timing snapshot but remains a loaded
single-cell diagnostic MISS, without original-matrix or dispatch-p99
qualification. Phase differences are observations, not causal attribution to
individual internal operations.

Changelog-only successor `74a3930dcfaabfb5fa43eeada5e1c5e1a26f984a`, tree
`1d5ab4b7a569768f66d6f4bb070f79826e80eb0d`, documents the existing Rust
source breaks without changing production behavior. Review `65eae6c9362f`
verifies its exact clean-head run: 512 selected tests pass across 14 binaries,
1,932 tests are skipped, and the command exits 0. The six-package release
selection includes the shared-value and exact serialization controls, but no
Mob tests. Wall time is 743s, with 12m21s preparation and a 0.641s test summary.
Raw SHA-256 is
`da17bbc31fad9468d8263968c63af6ef6483f5841a9ae453d02e0dde1a1d9b83`.
The later independently read Clippy log pins the same head with dirty=0 and
records exit 0 at 17:55:16 UTC after 849s. The release lib/tests selection
includes eight packages, including Mob and code generation. Raw SHA-256 is
`ed1402d7b6e3f1ab80a6c24d3c526b122c2b78447cda526ca05ad1d87d7f6a4e`.
Its package list is not Mob test execution and supplies no new performance
acceptance.

The later r3 frame-pointer profile at owner-attributed `0c3b3dcdc` is complete.
Its hash-verified JSON contains ten alternating pairs; the raw log records one
test passed, 33,440 total samples and 58.09% diagnostic overhead. Only 858
samples lie inside the measured windows. The reported 3.31/5.27 ms are nominal
sample-derived averages, and inclusive symbol costs overlap. The build receipt
records success after 8m15s; exact binary-to-recording attribution and outer
perf RC 0 remain partly owner-supported. This instrumented cell does not
replace the original six mean misses or qualify the overhead requirement.
Source ruling `5e140de8711d`, delivered to both leads after independent reviews,
allows immutable generated-owner grant representation and borrowed logical
operands, retaining every current permission, publication, clock and entry
check. It rejects substituting V1 binary bytes into the existing JSON-based
retained-work identity. The narrower alternative reuses the full-association
constructor's exact successful JSON bytes or their SHA-256 privately, preserving
old identity/resume compatibility and current-owner checks. No lineage-verdict
cache, implementation pass, additional run or new approval gate is inferred.

Linux confinement commit `eac074660aed1955a903a9c1eabe59994e5d48c7` provides
`make test-mcp-confinement-positive`, selecting all five supported-native cases
with the real MCP fixture and no selected-test skips. Ordinary MCP execution
passed 170 tests; explicit positive execution failed all five selected tests.
Four failures print `BackendUnavailable`; the fifth only prints an assertion
mismatch. Clean CI successor `913085d8c94557bf1b581adef130b328c88842e0`, tree
`112aa57ec39cf98f121e4d741bc91386344758c3`, now wires the existing target into
the active PR/main workflow on Rust changes, requires its result in the gate,
and includes it in main attestation. Fixture-only changes select the lane.
Independent source review clears that wiring and the exact-head classifier
self-test log ends with RC 0. The only retained positive execution remains the
five failures above; the workflow is not an executed supported-host pass. The
backend directly uses Linux namespaces, mounts/chroot and seccomp, so neither
a bubblewrap-only allowance nor the `ubuntu-latest` label establishes runner
eligibility. The existing owner must obtain five actual positive passes on an
eligible host; ordinary negative-path passes do not close that boundary.
The 19:56 UTC intake finds the owner clean at merge `13b352d8e816`, with that
wiring unchanged. Its exact branch still has no remote ref, PR or workflow run.
The inspected `stack-1730-r6` integration head `6ebddc23cad5` does not contain
the positive-lane workflow. A newer integration log only enumerates two custody
tests; it executes none. Review `e9667cb4b622` retains these source and log
comparisons. Publishing/integrating the existing wiring and executing its five
cases on an eligible host remain with the existing owners.
The coordinator's record shows its exclusion from the finite 0.8.52 cut;
required positive confinement acceptance remains open for full 0.9.

At integration `6ebddc23cad5d41ef02e3d18df5ef16f36099711`, the ordinary
Linux-only ignores extend to two required command-hook positives. Both exact
names are added to the existing explicit positive target; their bodies and
assertions are unchanged, and macOS selection is unaffected. The target runs
five MCP cases first and the two command-hook cases second. Make stops if the
first command fails, so this source selection does not establish seven executed
cases. No new positive execution or CI inclusion is claimed; the finite 0.8.52
exclusion and full-0.9 acceptance requirement remain unchanged.

On 2026-10-06, native candidate `18abf7414123262b7334289cf5c6415c6be6235a` passed its normal
hooked push and reached [PR1730](https://github.com/lukacf/meerkat/pull/1730) at
20:06:13 UTC. The retained log, independently read after completion, records
all normal gates passed, including Clippy, machine/protocol generation drift,
canonical TLC, governance and the workspace deterministic unit, integration
and e2e gate. The full push took 1h44m20s; this is gate wall time, not test-body
or authorization latency. Log: (operator-retained path).
GitHub confirmed that exact head and reported a conflict with main after PR1801.
The existing sole assembler prepared merge `0af71cc604353618f6a726246b244b17accd426e`;
its resolution is source-clear, with the reviewed runtime and regression files
preserved. That source passed 15 targeted tests and Clippy, verified from its
retained raw logs. The published successor is
`76ef4d58d5b64413cdf20486d46523779d6435f6`, combining that source with
`c2c38fe9deb3b7ff83100193771fc4e3746b519a`. Its composition is source-clear.
The prerequisite #1796 normal push completed successfully at 21:34:45 UTC,
confirmed in its original log and then by GitHub's published `c2c38fe9d` head.
The original `76ef4d58d` push completed successfully at 23:28:57 UTC. Its
independently read log records `rc=0` and the published branch update; all normal
gates passed, including the workspace deterministic gate. The recorded start
was 21:35:07 UTC, so the full hooked push took 1h53m50s. Log:
(operator-retained path). This is gate wall time, not
authorization latency.

GitHub confirms the exact `76ef4d58d` head but reports `CONFLICTING` and `DIRTY`;
only the skipped Renovate lock-sync check is present. Required hosted CI has
not started. At 23:33 UTC the GCP lead identified conflicts with the newly
merged provider no-redirect change #1815 (`589d98736`) in four LLM-client files.
The existing publication owner prepared merge
`ff4def3c2b8ea51e97220c12811952414d6a8c0a`, with 800 library tests including
six redirect cases reported passed by the GCP lead. The prerequisite CI-hygiene
push `f45c05d20a9a395fe630589d0f0e82783849576c` completed at
2026-10-07 00:12:53 UTC with `rc=0`, independently read in its original log.
The repaired candidate's normal hooked push completed successfully at 01:24:42
UTC. Its original log, (operator-retained path), independently
records `rc=0`, all normal gates passed and the published branch update. Start
was 00:13:15 UTC, making the full hooked push 1h11m27s; this is gate wall time,
not authorization latency. GitHub confirms the exact `ff4def3c2` head and reports
it mergeable. Hosted CI run `37556967082` has started, but its completed
[Format + governance job](https://github.com/lukacf/meerkat/actions/runs/37556967082/job/112585316411)
failed because `meerkat-sandbox`, `meerkat-authorization-contracts` and
`meerkat-authorization` do not package `LICENSE-MIT` and `LICENSE-APACHE`, verified
in the raw job log. The workflow completed with failure at 01:49:14 UTC. All its
executed build and test jobs passed, including authorization and store-restart integration,
Governed JSONL, the listed Clippy jobs, generation ratchets, both bounded TLC
jobs, closure, SDK suites, gpt-live-replay and Example web suites. The final
CI gate's raw log confirms the failed prerequisite is Format + governance.
Its existing timing measurement reports 1,453 seconds (24m13s) of execution
critical path and 1,458 seconds to terminal, with gpt-live-replay the longest
lane. These are CI timings, not authorization overhead. Release-semver also
passed. The separate [Live gate](https://github.com/lukacf/meerkat/actions/runs/37556966863)
completed successfully at 01:59:19 UTC for the published `ff4def3c2` head. Its
independently read job log records 11 of 11 test targets passed and `live gate:
GREEN`. The reported 1,392.771 seconds elapsed and 1,324.50 seconds critical
path are build/test timings, not authorization overhead. These selected live
scenarios do not establish complete feature or platform coverage.
The packaging repair `dab5f9865678eb33f0a88e5aab68264bca17a6cc` subsequently
passed its full normal hooked push at 03:04:46 UTC. The original log,
(operator-retained path), independently records every normal
gate passed, including the workspace deterministic gate, the branch update and
`END ... rc=0`. Start was 01:58:30 UTC, so the full push took 1h06m16s; this is
gate wall time, not authorization latency. GitHub confirms that exact published
head. The root-verified main [hosted CI](https://github.com/lukacf/meerkat/actions/runs/37565064711)
has completed successfully on `dab5f9865`, including Format + governance and the
remaining Clippy jobs. The separately required Live gate initially failed;
its third attempt passed as recorded below.
The [Live gate](https://github.com/lukacf/meerkat/actions/runs/37565064708/job/112610775825)
is terminal at 03:20:32 UTC: five scenarios passed, three failed (S101, S103,
S105), and three were provider-degraded voids (S100, S104, S106), neither passes
nor failures. S101 reports two missed readouts; S103 reports a 90-second
barge-in/correction fixture-end timeout; S105 reports a 60-second `fork_recall`
input/audio timeout. These observations do not establish a root cause or a native
ABAC regression. The raw log is retained at
(operator-retained path).
The actual CI checkout is merge `fae933a45dd6dcf99b2c19deb2a8f63938eff30a`;
root independently verified through GitHub that it and published `dab5f9865`
have the same tree `0814ac6d004fc7183986ed7e5c18a1a430b83c07`.

[Live attempt 2](https://github.com/lukacf/meerkat/actions/runs/37565064708/job/112615865282)
also failed, ending at 03:41:35 UTC: ten scenarios passed, S99 failed and none
were void. Its raw assertion reports missing fresh matching transcript/decoded
speech for the current fixture; the same CI checkout/tree was independently
verified. GCP subsequently identified a superseded Daffodil answer instead of
the typed Marigold answer and classified it as preexisting issue 1800. That is
existing-owner classification, separate from the raw failure and not an accepted
pass. Reported rates of 7/223 overall and about 1/41 on comparable heads have
different denominators and remain owner evidence.

[Live attempt 3](https://github.com/lukacf/meerkat/actions/runs/37565064708/job/112620225018)
completed successfully at 03:59:04 UTC. Its independently read raw log records
11 of 11 tests passed and `live gate: GREEN`; actual checkout remains `fae933a45`,
with the previously verified tree shared by published `dab5f9865`. Earlier
attempts remain recorded as failures. This closes the observed Live-gate failure
for that published checkpoint, without establishing full governed feature coverage
or authorization overhead. PR1730 subsequently merged as `8ca50a1a4` at 04:02:54
UTC. Its parents are `589d98736a98ab6932127a915b46e8b584124877` and published
`dab5f9865`; GitHub independently confirms the identical tree above. This
establishes merge provenance, not a new merged-main CI run. GCP reports the
code generation guard push finished successfully at 04:08:38 UTC and benchmark builds
resumed while other pushes remained held. The later completed measurement and
its acceptance limits are recorded below.

Read-only protocol review found two preparation mismatches. The preserved
`157a73e75` binary requires at least 2000 representative pairs and cannot run the
planned W20/N32 study. The historical 355-second W20/N32 run belongs to
`761ce0d1b`, not that binary. The current six-cell mean already pairs TrustedHost
and LocalGoverned with alternating order, so historical157 is unnecessary for
its within-candidate comparison and remains nonmatched historical context.
The staged tail overlay `3dd2539613b3634d92663aaed08ae2e827dd3bcf`, tree
`bfd165863ae69dae20baa2646f5d41123bfc1c22`, is directly above `dab5` but omits the
accepted `50ede58a1` full native model span. The existing owner was directed to
restore that coherent measurement delta before the applicable build, retaining
the existing fixture/analyzer and tool-tail extension. The accepted order remains
mean, tool tail, model tail under one non-resetting 1200-second budget, with no
sample reduction. If the model measurement cannot run or finish, completed
mean/tool results remain useful and full model p99 stays incomplete. The
provider-only timer is not full model p99; optional reviewer network latency
remains separate. Benchmark source overlays and the merged runtime base must
both be identified; this source review is not an applicability, build or timing
pass.

The mean artifact is now independently qualified from retained raw logs at
(operator-retained path). Its exact source is
`dab5f9865` and the merged tree above. The recorded locked release no-run build
completed at 04:16:17 UTC (`rc=0`, Cargo elapsed 27m29s); its recorded start was
03:48:47, so this interval includes the separately reported hold/resume window
and is not CPU time or authorization latency. The preserved binary's SHA256 is
`25b6b196de7c702a252b5864d0e0e548d78d38b98fdab59f52e2dd69695fde6a`.
`native_cost_correctness` and `representative::native_representative_correctness`
each passed one case with eight filtered out; test bodies took 0.03s and 8.39s.
These are readiness controls, not timing acceptance.

The existing owner stopped the obsolete tail build before it produced a binary
and integrated the accepted model span as
`bdc389349fd990a50565d4eff7e1a189f7380d60`, tree
`19d1e9f07d74637cf1d057f3e6b8195bcb85350f`, above `3dd253961`. Independent source
review cleared the integration: seven named fixture/core/analyzer files match
`50ede` byte-for-byte, the Agent observer hunk is identical, and the sole BUILD
difference removes a test source absent from this tree. The tool-tail extension
is preserved. The manifest is `meerkat-authorization/Cargo.toml`, enabling core
`test-support`. Keeping the `dab5` progress narrative resolves the documentation
conflict; unrelated complete-branch history is not a build prerequisite. GCP
reported the replacement tail build started at 04:23:51. Its terminal raw logs
are now independently verified at
(operator-retained path): the same locked
release no-run command passed at 04:31:57.777 UTC (`rc=0`, Cargo 8m05s, wall
8m06s). Both correctness selectors passed one case with 17 filtered out, in
0.03s and 8.28s; the runner completed at 04:32:06.275. The preserved binary's
SHA256 is `57214d859d0ac852078e2aae7716fd685a9f437d0fe1c0edf10f09e3f3e45032`.
Original runner/watcher handles were absent at 04:34:22. The records inspected
through that time contained no measurement START.

Source inspection confirms `native_cost_correctness` already exercises the
full native model timer in both modes and at depths 1 and 3, requiring two
completed spans and no open span. The model-tail profile reuses that sample
path, and its analyzer rejects missing/malformed spans and incomplete pairs.
No additional readiness selector or run is needed. Readiness is complete;
measurement and accepted performance remain separate requirements.

GCP announced START effective at 04:44:10 UTC and subsequently announced END.
The actual measurement records run from 04:44:22.190 through 04:49:19.159 UTC:
296.969 seconds within the single 1200-second allocation, with no benchmark
timeout or kill indicated. Mean, tool tail and full native model tail each
completed with `rc=0`, in 269.70s, 3.31s and 23.58s. Mean uses the exact `dab5`
binary above; both tails use the `bdc389349` overlay and its separately qualified
binary. These source identities must not be conflated.

Raw files, selector logs, steps, observer output and the executed script are
retained at (operator-retained path).
Independently calculated raw SHA256 values match the execution record:

- Mean: `b0e7a69f44a7d3bee18d4b97a2cec044911daaf0423596be772527dcbff7c16c`.
- Tool tail: `bea478cb129b8ff89ad39394138891bcb93b52d54e9dc06a03b12974d1b18e3b`.
- Model tail: `6f49a7307d9f2224f47c8b5c906a0a3a39d7d921d9b530fdc481a538a18e46db`.

Reanalysis with the accepted repository analyzer exactly reproduces all three
owner summaries. The representative profile has W20/N32, all 384 rows, six
complete cells and 16 opposite-order blocks per cell. Every cell is a conditional
MISS under the accepted simultaneous Fieller method; the overall analyzer result
remains UNPROVEN. Only fresh admission is a whole-turn comparison.

| Unit / depth | Trusted mean (ms) | Governed mean (ms) | Added overhead | Conditional ratio interval |
|---|---:|---:|---:|---:|
| Whole turn / 1 | 12.381 | 29.240 | 136.18% | 2.3346-2.3896 |
| Whole turn / 3 | 14.360 | 36.051 | 151.06% | 2.4797-2.5420 |
| Continuing segment / 1 | 7.527 | 17.763 | 135.99% | 2.3090-2.4112 |
| Continuing segment / 3 | 8.256 | 20.145 | 144.00% | 2.3496-2.5318 |
| Four direct calls summed / 1 | 0.0767 | 0.4278 | 457.56% | 5.3061-5.8703 |
| Four direct calls summed / 3 | 0.0855 | 0.9303 | 987.84% | 10.3821-11.4238 |

Both tail suites have W100/N2000, 8000 complete rows and zero failures/timeouts.
Matched signed tool-call deltas have p99 72.248 microseconds at depth 1 and
204.527 microseconds at depth 3. Full native model-preparation deltas have p99
150.826 and 180.491 microseconds, respectively, with 4000 paired spans per depth.
Separate model-request ordinals are also below 1 ms. These are empirical
correlated estimates, without independent tail confidence. The model span
excludes earlier admission, real provider serialization/header refresh, network
and Outcome auditing; optional reviewer latency is separate. Provider-only
`model_authorization_ns` and instrumented whole-turn p99 remain diagnostics.

The observer contains 59 five-second samples, with load1 1.23-1.92 (median 1.40).
It records Python activity at 04:44:42/47 and unattended-upgrade/dracut/apt-check
at 04:46:32/37 during the mean step. These sampled records do not establish exact
duration, causal effect, stationarity or absence of unsampled work. Raw pairs
have no timestamps with which to assign affected cells. Preserve the conditional
misses and the quiet-host limitation together; do not infer that maintenance
explains the measured ratios.

The fast diagnostic selector now reuses the representative depth-1 fresh
admission fixture at `6d54980486a4f7d486b84fec646beb9977a3e577`, above
`bdddb6b19fa253086ee8214667407513eef5f28f`. Each sample retains 252 old rows,
four fresh contributors, two scripted model requests, four real reads and the
same completion, audit and teardown checks. Only cells and repetitions are
reduced; the W20/N32 acceptance selection is unchanged. The independently
hashed retained base binary is
`675a97f7a08439db05804b03f0180aba8c4940236755277ffbde4dae6f261821`;
compiler records show optimization level 3 and disabled debug assertions.

Three five-pair runs produced 30 raw samples. Independently checked logs report
one passing selector per run, with test-framework elapsed times of 3.49s,
3.48s and 3.42s in the order below. Separate process wall time is not established
by those log footers. Build metadata records 511s cold and 12s incremental.
Both existing correctness selectors passed one case with 18 filtered out.
Raw JSON and logs are retained at
(operator-retained path) and on GCP under
(operator-retained path).

| Pair order | Trusted mean (ms) | Governed mean (ms) | Ratio-of-means overhead |
|---|---:|---:|---:|
| Alternating | 12.282580 | 30.178173 | 145.699% |
| Trusted first | 12.454281 | 29.906065 | 140.127% |
| Governed first | 12.353037 | 30.039129 | 143.172% |

Every sample has 256 rows, two model requests and four reads, with 18 audit
records in governed mode and zero in trusted mode. All 30 phase partitions
reconstruct from raw timestamps and sum to the measured interval. Admission
return can overlap already-started runtime work; its separately reported
interval is excluded from that partition. No synchronization barrier was added
to force event order.

Pooling 15 samples per mode, the largest descriptive phase-median difference
is 8.539ms from admission to the first model request; tools-to-second-model and
completion differences are 3.985ms and 3.249ms. These windows include multiple
owners and scheduling, so they identify where to profile, not authentication
or snapshot cost. Separately
computed medians are not additive. Measured intervals total about 0.212s per
run, while fixture setup alone takes 3.172-3.244s; a whole-process profile would
mostly measure setup. The owner reported concurrent gate activity and load
around 7-9. Five pairs per order neither establish order independence nor
replace the original six-cell acceptance study.

The subsequent base/opt1 comparison has 120 independently checked raw rows
from 12 successful invocations, six per binary and five pairs per invocation:
30 samples per mode per binary. Binary order was `ABBAABBAABBA`, three quartets,
with no warmup or discarded samples. All rows retain depth 1, fresh admission,
256 rows, four reads, two model requests and 18 governed audit records or zero
trusted records. Every phase sum equals the measured interval, from fresh
admission through native completion; setup, verification and teardown are
excluded. The potentially overlapping admission-return interval remains
outside the phase sum.

| Binary / statistic | Trusted (ms) | Governed (ms) | Overhead |
|---|---:|---:|---:|
| Base mean | 14.8177 | 31.6194 | 113.39% |
| Opt1 mean | 8.8457 | 20.9190 | 136.49% |
| Base median | 12.6316 | 30.6205 | 142.41% |
| Opt1 median | 8.5299 | 20.5920 | 141.41% |

Across these runs, governed time fell 33.84% by mean and 32.75% by median; the
signed governed-minus-trusted gap fell 28.14% and 32.95%, respectively. Trusted time
fell proportionally more, so ratio-of-means overhead increased. These are
descriptive comparisons at sampled load 8.68-12.13, not proof of causality or
order independence. Four correctness logs each record one PASS.

Opt1's snapshot change on `3b894dcbc646cd1ea00034e7e0785022bc042d90`
semantically matches reviewed diff `e9cd62e2`; the current `5c0e` diff differs
only in Git headers and hunk labels. It projects under the shared authority mutex, forks
healthy authority for generated terminality, and cold-recovers poisoned
authority. A full-state clone remains in the fork.

Opt1v2 is committed as `47c8bd11e0a55899dd73e26ded2dd36fe27a971f`, tree
`fdc7bf9ce57cad087bd882812816ad3e4c89aeb5`, on the same `3b894dcbc` parent.
The independently read
(operator-retained path)
(SHA-256 `29c6b53f14363760c90a0b2c6ef188975a64ce70fef1d1028c3fd7d7eafda161`)
records 132 selected runtime tests passed, 1899 skipped, an 11m36s build and
0.656s test execution. The new
`snapshot_leaves_live_state_unchanged_and_poison_recovery_matches_healthy_fork`
control passed. The run-to-commit association comes from the owner receipt;
the log has no commit marker. Independently, committed `turn_state.rs` and
`handles/mod.rs` byte-match the reviewed source. This selected runtime result
does not establish performance acceptance.

The separate opt2 admission correction is committed as
`706081ef78dd7095ae9c283a6df468bf7c12df12`, tree
`a9055b034964f5588a0253ae7a95f0143da297ae`. Independently read
(operator-retained path)
records 122 PASS, 1910 skipped, a 9m45s build and 0.084s test execution. The
run-to-source association remains owner-attributed; the log has no commit or
binary identity. Its eight-case final-entry control covers direct/common entry,
wake/no-wake and allow/denied-after-preview using the ephemeral driver: one
final authentication, no denied row, and exact admitted input/association.
It does not exercise durable-driver post-await entry or process recovery.
The selection differs from opt1v2's 132 tests; neither is a full runtime run.
No accepted opt2 savings or performance qualification is established.

The later paired comparison, independently checked under
(operator-retained path), has 18 successful invocations
and 180 rows: 30 trusted and 30 governed samples per label. Every row retains
depth 1, 256 rows, four reads, two model requests and 18 governed audit records
or zero trusted records. Nonnegative phases sum exactly to the measured span;
the overlapping admission-return interval remains excluded.

| Binary label | Trusted mean (ms) | Governed mean (ms) | Ratio-of-means overhead | Process-wall mean (s) |
|---|---:|---:|---:|---:|
| Base | 12.394848 | 30.202869 | 143.6728% | 3.4800 |
| Opt1 V1 | 8.453203 | 20.123319 | 138.0556% | 3.4017 |
| Opt2 V2 plus authentication change | 8.590053 | 20.296437 | 136.2784% | 3.3717 |

Opt2's governed span is 0.8603% higher than opt1 V1 here, so this record shows
no added measured-span improvement. There is no standalone V2 comparison;
the combined changes cannot isolate authentication cost. Label-to-executed-binary
mapping remains owner-receipt evidence because the run records omit absolute
command arguments. Process-wall time includes work outside the measured span.
With sampled load 11.81-24, no warmup or discarded samples and only one cell,
these descriptive results do not replace the original six-cell acceptance MISS.

Opt3 `b74eb56c05f2dd01009a97be9127f3a9a3a0886d`, tree
`ddf5454a986c3040a484f0bde3e63cf6ed00b643`, is independently source-clear.
Its three read projections preserve existing authority locks, poison handling
and error mappings while avoiding full-state copies. Independently read records
show 346 runtime PASS/1686 skipped (9m43s build, 6.828s bodies) and both existing
cost correctness selectors passing (0.03s and 7.39s bodies; 6m43s build).
The retained executable and opt3 copy both hash to
`db51e29475bd032168beaa208d1d03843ef0ccac75b02ed6d626563c82ba76d7`.
Logs precede the commit and omit invoking source/binary identity; execution
association remains owner-attributed WIP evidence.

The new comparison under (operator-retained path)
contains 18 successful invocations and 180 consistent rows, 30 per mode/binary.
Workload and seven-phase sums are unchanged. Sampled load is 4.62-12.82.

| Binary label | Trusted mean (ms) | Governed mean (ms) | Ratio-of-means overhead |
|---|---:|---:|---:|
| Base | 12.489457 | 30.243386 | 142.1513% |
| Opt2 | 8.509268 | 20.713825 | 143.4266% |
| Opt3 | 7.691784 | 18.431894 | 139.6309% |

Opt3's governed mean improves 11.0165% over this opt2 comparison; every ratio
still misses 10%. This one-cell diagnostic leaves six-cell acceptance unchanged.
The r2 evidence archive has verified SHA-256
`be579b755cb9ac68a5fd7068809088db91686f4c59e88eac965e5b54735e5837`.
Its profile manifest adds declared clock, commands, windows and file hashes for
earlier profiles. Actual recording metadata and opt3 invocation linkage remain
unverified; these limits do not require another diagnostic run.

The first macro-side Arc-COW checkpoint is committed as
`d06a60f700b987e672c6c616c8911885585b1040`, tree
`ff116a8832a96b2e4e90f212acf6c1c425d1d027`, on opt3. Independent source review
found no new blocker in the 31 selected retained-input collections: reads retain
logical values, mutations detach explicitly, and authority tokens and prepared
base equality retain their existing meanings. This establishes the first
generator's source checkpoint; the subsequent schema-driven kernel parity
checkpoint is recorded below. Canonical and persisted formats and public
`Input`/`Effect` types remain unchanged in this slice. The public Rust state-field
representation changes to `Arc` collections, with explicit caller migration.

The retained runtime log records 2034 PASS, zero skipped, a 9m53s build and
9.965s test execution. It includes retained-input-only stale-commit rejection and
fork/snapshot independence. Both existing cost correctness selectors separately
passed, with 0.03s and 4.73s test bodies after a 7m59s release build.
Execution-to-source association remains owner attributed. That full 2034-test
result belongs to `d06a60f70`; it was not rerun on the successor.

The final parity checkpoint is
`2192f46930cee94a6d1b6b1be134421ccf10eaa6`, tree
`4ba579600a160ec06bb4636b2130c30451386c70`, on `6daed5ea8`. Independent source
review clears the state-only `shared_fields` schema metadata, renderer cfg
correction, all 31 generated collection types and initializers, and kernel
`Arc::make_mut` caller migration. Logical artifacts, TLA+ specs/mappings and
public event/effect shapes remain unchanged. The same-value write control is
now incorporated: both collections detach while logical state remains equal,
prepared commit succeeds and the live owner token is preserved. The stale-base
control additionally requires exact `{a,b}` contents, excluding prepared row `p`.

Independently read retained logs show 38 focused runtime PASS/1997 skipped,
including that discriminator, and 508 four-package PASS/2 skipped. Their builds
took 11m06s and 27.83s; test bodies took 0.022s and 42.765s. Machine drift reports
17 machines/nine compositions up to date, protocol drift reports 51 matching
artifacts, and the later strict six-package all-target Clippy log finishes
without diagnostics in 1.57s. That command did not select all features. The
logs precede the commit and omit source/binary digests, so successful results are
independently read but their association with final committed bytes remains
owner-attributed. The earlier Clippy failure is retained. These are source and
functional checkpoints, not a performance verdict or another validation lane.

The r3 archive has independently verified SHA-256
`5d9f2f03aeddf7cd05b5edda6732cffb853567cc1e0ce47f9eb3ebe624695c82`.
Its 18 successful invocations contain 180 consistent rows, 30 per mode/binary,
with the same depth-1 workload, audit counts and seven-phase sums. Sampled host
load is 3.85-8.38. Invocation-to-binary mapping remains owner-attributed because
the run records omit absolute commands.

| Binary label | Trusted mean (ms) | Governed mean (ms) | Ratio-of-means overhead | Process-wall mean (s) |
|---|---:|---:|---:|---:|
| Base | 12.468179 | 30.617157 | 145.5624% | 3.5283 |
| Opt3 | 7.824748 | 18.789386 | 140.1277% | 3.1217 |
| COW1 | 2.906216 | 5.955366 | 104.9182% | 1.6933 |

COW1's governed mean is about 68.3% lower than opt3 and 80.5% lower than base
in this comparison. The remaining governed-minus-trusted gap is 3.0491ms, of
which the admission-through-first-model-request phase accounts for 2.3072ms.
These are descriptive diagnostic results, not isolated function costs or quiet
host acceptance. Every ratio still misses 10%; the original six-cell study and
added native authorization p99 requirement remain unchanged.

The downloaded evidence archive (operator-retained path)
has independently verified SHA-256
`d92c4504cfd3ddcd693198c7f471a818efc78d1f55821309920761454373a0e2`.
Independent measured-window filtering reproduces 3160 governed and 1297 trusted
samples, with snapshot-inclusive shares of 54.75% and 50.89%. Separately retained
post-opt1 admission windows contain 1231 governed and 487 trusted samples:
state-clone-inclusive shares are 51.58% and 42.71%, while the native
`authenticate_work_with_credential` share is 4.47% governed. These overlapping
inclusive stack shares are not wall-time percentages or isolated authentication
cost. Window filtering and counts are verified; retained recording clock,
event and binary linkage remain unqualified. The supplied `CLOCK_MONOTONIC`
command is owner prose, not retained recording metadata. This is one diagnostic
cell, not acceptance. The original six-cell study still misses the <=10%
threshold; its workload, thresholds and recorded misses remain unchanged.
The normal checkpoint publication route remains unchanged.

The retained CI logs identify the actual checkout as merge
`a839a7e8ca2d78cfec99cbec2c8b38772e954f15`. It and the published `ff4def3c2`
head have the independently verified identical tree
`8c33c364a44a274857c8f8580314f12825c45eaf`. The passing counts support these
bounded scopes, not complete authorization or recovery coverage:

| Retained job | Executed coverage and limit |
|---|---|
| [Authorization](https://github.com/lukacf/meerkat/actions/runs/37556967082/job/112585654983) | 23 passed: 16 heterogeneous native-loop selectors, including decoder and Agent collector controls, plus 7 cost-fixture correctness controls. The two timing measurements were skipped. The native group includes actual separate-process completed-turn reconstruction with fresh current authority; it does not restore interrupted work or live consent. |
| [Governed JSONL](https://github.com/lukacf/meerkat/actions/runs/37556967082/job/112585655130) | 3 passed through the exported JSONL entry, fixed commissioned callbacks and actual native owners. They cover local refusal, permitted siblings, controller feedback and admission/setup negatives. The trusted-host fixture uses in-memory stores and a private HTTP provider; it does not qualify authenticated remote-human ingress, process recovery or all RPC methods. |
| [Store/restart](https://github.com/lukacf/meerkat/actions/runs/37556967082/job/112585654984) | 36 passed, none skipped, across five binaries. Generic storage/lifecycle coverage includes a real completed-turn subprocess history control and seven structural source tests. This job does not install the governed authority/consent owner or prove interrupted governed recovery. |
| [Live S97-S107](https://github.com/lukacf/meerkat/actions/runs/37556966863/job/112585314910) | 11 passed public GPT Live compatibility scenarios, including positive credential binding, delegation, transcript continuity and channel lifecycle. They do not supply ABAC refusal/revocation or Required sandbox negative controls; same-process channel reopen is not governed process recovery. |

Source-to-log mappings are retained in `native-jsonl-coverage.md`,
`store-restart-coverage.md` and `source-coverage.md` under
(operator-retained path). None supplies accepted
authorization overhead measurements.

At earlier next-boundary candidate `b01d3e87f8e24ce2e73df34482791e66a550ebda`, tree
`1a53b12e9230c0ed2f4b2c740cc009f1289a7795`, the GCP owner reports check 5 green
across 33 crates. The independently read `tests-attempt2.log` under
(operator-retained path) records 249 passed and 10 failed
out of 259 selected tests. Six failures in delivered fixtures have three
source-reviewed, test-only corrections delivered to GCP for that exact candidate:
retain zero-based original-turn attribution in two background-feedback checks;
read the failed run's actual owned Session and scope the source-denial oracle to
its exact operation in three reviewer cases; and implement the existing typed
notice-append method on the live-cancellation fixture's DummyAgent. These fixes
preserve the negative, cancellation and idempotency oracles and change no
production behavior. No post-fix executed pass is established. The other four
failures remain with their existing GCP owners; this result adds no new gate.

The next-boundary compile correction `f3ee6b4472176e3fc0dd43e9c4ef37830949989c`
is independently source-clear: it preserves typed local MalformedFacts feedback
and lossless RuntimeInput audit identity. Its Session test is a serde round-trip,
not an actual store or process reopen. Generation and drift at `c5a999e16` remain
owner-reported. Later raw logs under (operator-retained path) were
independently read: `clippy-attempt5.log` records Cargo Finished without diagnostics
at `4ddd29557a6bed690264138530a65eccafb4c4c5` (02:45:25-02:45:29), covering all
33 `check-crates.txt` packages with all targets/features and `-D warnings`.
`check-attempt6.log` records Cargo Finished without diagnostics at
`27d3c821e722394aca38d9381178186ebcb6334c` (02:49:02-03:00:11), superseding the
interim compile blockers. The clean checkout has tree
`1c9ce1d77772567b805297ec0f3ea4088aaa2727`. Clippy attempt 5 belongs to the
earlier `4ddd` head; check 5 belonged to `b01`, not `4ddd`. The separately read
`clippy-touched-attempt1.log` at `27d3` records Cargo Finished without diagnostics
and `rc=0` from 03:10:59 to 03:19:32 UTC (8m33s). Its exact package selection is
unverified because the script/list later changed; the current script cannot
establish that run's all-feature or full-scope coverage. The missing historical
selection alone does not justify a new rerun.

The independently read `rerun-attempt1.log` is terminal on its recorded `27d3`
head: 23 passed, 3 failed and 4,637 skipped across 26 selected tests. All 10 earlier
failures were selected; 7 passed. The three failures in that run were:

- `native_original_source_observation_failure_preserves_infrastructure_and_fresh_work`
- `native_reviewer_model_observation_failure_preserves_source_sibling_and_fresh_work`
- `committed_snapshot_revalidation_appends_only_when_unchanged`

The two reviewer cases fail their observation-versus-denial assertion at
`model_reviewer.rs:809`. A root-authored, independently source-reviewed test-only
correction now scopes the negative to the actual failed source/model operation
ID, retaining the typed infrastructure, sibling, zero-reviewer-HTTP and fresh-work
controls. Its later integration and newly reached fixture failure are recorded
below. The raw failure log does not identify the other refusal or establish a
production misclassification. The B7 case was explicitly selected and failed fresh-work
admission at `parallel.rs:1876`; its later targeted correction is recorded below.
Compilation took 9m42s, selected test execution 1.007s, and total wall time
9m44s (03:00:34-03:10:18 UTC). The footer's `rc=0` is not proof of success:
nextest explicitly reports three failures. This five-package filtered rerun
is not the full earlier 259-test selection or following-boundary qualification.
Companions still await the lead's formatted committed-base handoff.

The B7 test-only correction at `15ab46b5be34de9c6bb2ebb8106d7d59c1f33065`, tree
`df0635fe45e6d79fed2fbd3ffaba2ce674d1046c`, subsequently passed its original
`b7test-attempt1.log`: one passed, 465 skipped, `rc=0`, from 03:21:04 to
03:30:58 UTC. Compilation took 9m53s, test execution 0.070s and total wall time
9m54s. Root and independent source review cleared the `+27/-4` fixture change:
it preserves the original snapshot-mismatch negatives and exactly-one-append
assertion. Its independent fresh evidence is in a second session, not
same-session continuation, Agent execution or restart recovery. Raw files are
retained under (operator-retained path). This targeted pass does not
replace the earlier 23/26 history. The later Clippy log at the same `15ab` head
records `Checking meerkat-mob-mcp`, Cargo Finished without diagnostics and
`rc=0`, 03:31:19-03:40:47 UTC (9m28s wall, 9m27s compilation). A contemporaneous
read of the mutable script/list showed that single package with all targets,
all features, locked dependencies and warnings denied. The raw log does not
record the command flags, and no immutable script/list copy was saved; that
inspection cannot be promoted to pinned historical invocation proof. The run
precedes the reviewer fixture commit. This attribution limit requires no
additional qualification run.

The reviewer audit-scope correction was integrated exactly at
`6f9efce8e4c84624ed2489882b249aa3b5b55b41`, tree
`583ae8537daad444cc1dd8631eaa70b6c6ad77d5`. Both targeted cases now reach a different
fixture panic in `wire_text` at `e1_policy_control.rs:517`. The first run used
only the authorization package's default features: 0/2 passed, 24 skipped,
3m25s compilation and 0.068s test execution. B7 passed again in the second
command; total wall time was 3m26s, with aggregate `rc=100`. An all-feature rerun
also failed 0/2 (1m56s compilation, 0.066s execution), and a backtrace rerun
reused compilation in 0.14s and failed at the same call chain. All three raw
logs are retained under (operator-retained path).

Pinned source ordering identifies the new failure as the fresh-prompt search at
`model_reviewer.rs:930-931`: it incorrectly passes every user-role provider
message, including tool-result envelopes, to a strict text-only helper. The
backtrace confirms `wire_text` through the exercise's iterator predicate without
source line numbers. Earlier infrastructure, sibling, exact-operation audit,
fresh completion, distinct-run and fresh-attribution assertions were reached
successfully. Trailing checks for no reviewer/source retry, no reviewer HTTP,
unchanged tool effects and unregister completion were not reached. A minimal
fixture-only correction, patch `6414c3c9`, preserves strict tool-result decoding
and requires exact complete prompt text while skipping non-text messages. Root
and independent review cleared that source and delivered it to the existing GCP
owner. Its subsequent targeted execution is recorded below. No production
correction or full following-boundary pass is established.

The correction was integrated as `0b059e545213b455b4f2c14dd3f33e735655a144`, tree
`a2cdc6645c00a820a1f9007392229b9342daa1f7`, directly after `6f9`. Independent source
review confirms only formatting differences from the accepted patch. The raw
`final3all-attempt2.log` records both reviewer cases passing, 24 skipped and
`rc=0`, from 03:53:56 to 03:54:02 UTC. The echoed command selects the authorization
package, `native_governed_loop`, all features and those two exact tests.
Compilation took 5.33s and test execution 0.069s. The trailing no-retry,
no-reviewer-HTTP, unchanged-effects and unregister checks now passed. B7 was
explicitly skipped using its prior result, not rerun at this head. Raw logs are
retained under (operator-retained path).

The following authorization Clippy run failed at that same head, from 03:54:15
to 03:57:45 UTC (`rc=101`). Its log echoes the exact package, all-target/all-feature,
locked and warnings-denied command. Two dependency runtime methods were unused:
`staged_retained_work` at `driver/ephemeral.rs:675` and
`LiveBridgeOutcomeAuthority::new` at `live_execution.rs:1299`. The same warnings
were visible in the earlier targeted test logs. Static review finds both methods
unconditionally compiled while all their callers require `live`. The authorization
package's all-feature selection does not enable `live` on this runtime dependency.
The recommended correction gates only those two private methods, preserving their
owners and stored state. Enabling another feature merely to hide the warnings
would leave the valid non-live configuration broken. Existing GCP owners retain
correction and execution; that failed run is retained separately from the
correction below.

The existing owner committed the two-method correction as
`91420da279f0c94a0f35ab49b89e1e3c10d8a02e`, tree
`a3bfc857ec4fceeb486356367e154b2ff1087c7d`, directly after `0b059`. Root and
independent review cleared the exact two added `cfg(feature = "live")` gates;
fields, stored state, public authority type and test constructor are unchanged.
The same authorization-only Clippy command then passed on `91420`, with no
diagnostics, from 03:58:39 to 04:00:52 UTC (`rc=0`, Cargo 2m12s). Its exact header
and raw output are retained at
(operator-retained path).
The existing 33-package final check passed on that head from 04:01:01 to
04:10:54 UTC (`rc=0`, Cargo 9m52s, wall 9m53s), with locked dependencies, all
targets/features and no diagnostics. Its full retained log is
(operator-retained path).
The subsequent 33-package Clippy passed on the same head from 04:11:03 to
04:25:45 UTC (`rc=0`, Cargo 14m41s, wall 14m42s), with locked dependencies, all
targets/features, warnings denied and no diagnostics. The same-head license
gate passed from 04:26:20 to 04:26:29, confirming every release crate packages
its required license texts. Both terminal raw logs are retained beside the
check log. The two reviewer test passes remain attributed to `0b059`, not
relabeled as a test run on `91420`.

GCP delivered the next-boundary `91420` bundle and received the landing
disposition for one integration branch/PR, `integ/next-boundary-0852`, after
merging actual main `8ca50a1a4` under the existing quiet-window/push order.
Ordinary hooked push and normal CI remain required. No competing PR or extra
local replay is requested. The earlier `b01` selection below remains 249/259,
not an all-green full targeted run; later corrections retain their own exact
heads and scopes. Bundle delivery and landing disposition do not establish
publication or final combined qualification.

Static selection review at `91420` confirms both reviewer regressions are
ordinary non-ignored tests selected by the existing authorization integration
suite, normal CI and local fast/integration commands. Their focused qualification
commands are not the sole regression path; no new selector or validation runner
is needed. This proves selection in source, not another executed CI pass.
A separate local review of the exact `91420` bundle corrected the
[review-context coverage description](../adr-001-model-review-and-consent.md):
complete typed text/JSON/known-notice appends and separately supplied raw tool
arguments are supported. Media, unresolved references, operation-input originals
and current selected-account/mandate/policy explanations remain open. This is a
source/documentation correction, not additional execution or full approval
coverage.

Read-only build diagnosis found package/feature selection changes between these
runs; identical jobs/debug/incremental settings do not make their dependency
variants identical. No cache deletion or changed build lane was established,
and the share of compile time caused by feature changes is unmeasured. Future
reruns preserving one acceptance configuration should keep its package/features
stable and narrow the test selector. No additional run is required solely to
qualify this diagnosis.

The additional compile companion `b6666138ded0dea2fad5282de6b031d8d5756288`
is independently source-clear. Widening only `SpawnGate::launch_parts` to Linux
and macOS matches its already enabled callers and sandbox dispatch. The borrowed
Unix descriptor/token body, custody and platform implementations are unchanged.
This does not establish a Linux compilation or a qualified following-boundary base.

Independent r759 source reviews at that exact combined head found two bounded
gaps. Mandatory PostTool hook infrastructure failure returns before the completed
tool batch commits, losing already-entered sibling results, effects and async
obligations. The GCP lead assigned the existing live-path owner one fix and a
critical regression test first. Deliberate permission/confinement refusals retain
their accepted local behavior; background and committed-observation Guardrails
remain explicitly unsupported. Separately, the reviewer context read checks
current authority but presents historical original-input associations without
the current selected-account, mandate or policy-edit explanation. That context
must come from its actual owners on the optional review path, with no added R1
traversal or inferred identity in SDK/Console.

The PostTool repair `c1a6751637675778eceb296d7afdd909c202b5f7`, tree
`89a1038987e42450dee223c5148fc6f45805e397`, is independently source-clear
against parent `c43d25ad0`. It retains the exact typed hook infrastructure
failure, withholds the affected raw publication and commits entered sibling
results, effects and async registration before returning that infrastructure
error. It uses the existing deferred batch owner and selects the first deferred
failure in tool-call order. The new Agent regression has meaningful retention
oracles; its fresh `resume_session` fixture seeds default build state only.
The owner reports reverted-fix RED and 213 passing tests; this source review did
not independently execute or inspect those raw results. Test-only successor
`bebb6fd8842c8c91b46d78b2410eb0f1b670384d` is independently source-clear for
actual Agent staging, Session serde, callback application and resume to the exact
hook infrastructure error, with sibling publication retained and no body/hook
replay or extra model request. The owner's reported 2/2 result is not
independently verified. Exact applied-receipt preservation and callback redelivery
are not asserted; non-transcript effects, detached async obligations and settlement
diagnostics are covered separately, not through this mixed callback case. The
serde round trip is not durable-store or process recovery. These limits do not
reopen the accepted production repair or qualify its interim parent as the
following-boundary base.

The recovery reviewer specified two additional subprocess controls: interruption
with a pending human request, and interruption after consumed consent and target
entry but before observed completion. They retain fresh-current-authority and
permitted-sibling controls, and distinguish uncertain effects from denial. These
are proposed scenarios, not authored tests or executed failures. This combined
head still has process-local R2 and no qualified R3 route; the existing consent
owner's pending interfaces are needed to bind honest native fixtures. Its
separate branch and earlier focused tests are not superseded by this source
observation. Completed-turn reopen remains the only separately qualified process
boundary. The source reviews add no new runner, authority store or qualification
gate, and the following-base hold remains in force.

The descendant-custody follow-on found a separate command-hook cancellation and
timeout gap at the same head. Both Required and TrustedHost launches return only
the direct-child handle to the invocation; timeout cleanup can finish on leader
exit without establishing remaining group containment. The host watcher retains
truthful recovery evidence but does not terminate survivors while its host is
alive. The existing shell/job/monitor group owner supplies the reference pattern.
This finding concerns the owned process group, not the stronger full-descendant
termination requirement, which both compiled backends explicitly reject. It is
queued for the existing owner, separate from MCP graceful shutdown and the
already assigned PostTool settlement correction. No process probe or execution
was performed.

| Area | Verified evidence and remaining limit |
|---|---|
| Native human consent | Exact `885554aed055c42e332850c9b5a518036ee82e94` passed 218 focused tests with 2,748 skipped, including seven consent cases. These are library and fenced-dispatch controls, not a full Agent flow or separate-process consent recovery. The raw log is (operator-retained path); compilation took 3m16s and test bodies 1.045s. |
| Human-consent successor | R4 `ae05b0011e0defb476fcfccd1aa8538252fd8903`, tree `cd64ad7b80f349f9f1c8cdbab32fa2e3ed7d75a4`, is independently source-clear for the three r3 corrections. Decision publication follows the store-capable exit and rechecks authority/context, approver eligibility, deadline/expiry and reconciliation; unpublished Approved history cannot be spent. Failed withdrawal preserves typed Unreconciled without false notices or reusable custody. Review-only withdrawal settlement now travels through existing owned reports; successful-entry reports retain it through held-worker custody, without a later service-call nudge. Coherent readback may precede a concurrent withdrawal; it promises neither newest-at-return state nor effect authority. R3 entry completes store/exit work before final checks; R2 entry remains store-free and rechecks after exit. A refused committed spend stays Consumed with no refund. Approval references, linked one-use legality, Denied human rejection and restored history without executable custody retain their accepted scope. Combined held-worker/deferred-write-failure coverage remains open, not a demonstrated source defect. Core 81/81, authorization/store 68/68 and Clippy remain owner-reported, with no independently read raw logs or separate RED run on r3. The stack remains following-boundary only, outside the current freeze. Original-run carry, authenticated host admission, current presentation, delivery/wake, restored live consent and interrupted-process recovery remain incomplete. |
| Reviewer context and failure feedback | Native original-input context (`15dee74b` patch), supported typed text-appends (`c35c4945`) and successful reviewer HTTP followed by protected observation failure (`ac4ab4ef`) are independently source-reviewed and delivered. Original-input reads retain their actual work owner and independently checked source permission; unsupported media/reference context remains unavailable. Protected observation failure remains infrastructure failure, with no target effect or automatic retry and with permitted sibling continuation. The new fixtures are unexecuted. |
| Recovery | Same-process evidence retains its stated scope. The separately recorded `adc79830f` subprocess fixture proves completed-turn reconstruction followed by fresh governed work under newly installed current grants. It does not prove interrupted continuation, restored live consent or automatic recovery across all surfaces. |
| Live-channel revocation | Production `dc095580750b14eb417aeaaf40fca33551c93888` and wording follow-up `efcf0e067` are independently source-reviewed. The correction removes revoked experimental, staged and playback state while preserving entered execution and settlement of previously authorized delivery. It refuses new `AuthorizeLiveDelegationResultDelivery` and `EnqueueLiveContextRow` admission; revocation does not rewrite an already-started effect as absent. The owner reports 107/107 runtime controls and the focused 1/1 protocol-drift control; those results remain owner-reported. This closes the scoped source finding, not broader recovery or deployment-wide qualification. |
| SDK and Console | Feedback `151d9d62`, optional reference `074bf586`, reference display `774c72a4`, typed gating-result patches and public TypeScript subscription fixtures (`75e087fc`) are source-delivered. MCP setup-reason propagation is source-clear in native Python/TypeScript (`a6faf991`) and stock/shared Console projection (`769d4938`). Point-aware PostTool Console feedback (`9bb2fb59`) is source-reviewed against the accepted chain. Only a canonical per-call result establishes withholding; a bare Observe-hook refusal does not. The sole assembler must regenerate native schema/types; integrated execution and publication remain pending. An approval reference or displayed refusal does not establish action execution. |
| Confinement | Linux `baebee65`, MCP `f7e21419` and command-hook `356acc80` source are queued with the sole assembler. The hook patch binds the exact launch and preserves durable custody plus local PreTool refusal. Durable shell-job refusal codes (`a2c19339`) and typed MCP setup notices (`d67f08ed`) are source-clear. The accepted factory-owned asynchronous MCP feedback fixtures (`5adf304b`) cover actual adapter refresh and next-model projection. Foreground Observe locality source and tests (`fcb6bb39`) supersede the earlier `b14bd9f8` test-only draft. PostTool locality production (`9e205e51`) and strengthened controls (`cc34ea37`) extend the earlier tests (`1b2c5ce2`). The source-reviewed path withholds only the affected publication while preserving entered effects, sibling results, async registration, deferred visibility and settlement diagnostics. It adds a hook-specific refusal instead of claiming the tool never ran; canonical schema/SDK regeneration and execution remain pending. Post-commit Observe continuation and retained refusal diagnostics (`ea6beabd` patch) are source-clear with two new engine-level tests, including a later observer failure. These tests do not establish host/model delivery or durable audit. All these additions are unexecuted. Mandatory Guardrail disposition, full background and post-commit feedback, descendant termination and remaining platform/launch coverage stay open. |
| Background Observe feedback | Engine and active-Agent source (`b4861fc7`) is independently source-clear on the accepted PostTool chain. Scheduling pressure and retained completion facts reach the shared provider text projection with bounded original attribution and fixed failure reasons; concurrent PreTool preparation remains parallel, and notices are retained before event awaits. DefaultHookEngine enforces retention bounds; custom engines must uphold the contract. Factory controls (`d990b39d`) cover a real background engine with injected refusal, exact run attribution, once-only next-request feedback, deterministic concurrency pressure, permitted tool completion and no extra model request. These controls are authored, not compiled or executed; they do not establish native ABAC or OS enforcement. The separately reviewed late-fact bridge below extends this route. Full post-commit feedback, graceful teardown and process-loss recovery remain open. No new external producer mandate or qualification gate is implied. |
| Late background Observe delivery | Frozen source `4c910591` extends the existing PostCommitHookDispatcher and same-session transcript writer. It retains old engine work across configuration changes, uses one passive identity per native completion ledger across clones, and holds immutable pending notices until the actual writer succeeds. An incoming wrapper still becomes the current execution adapter. Unsupported or temporarily unknown lifecycle status can reject a proposed replacement while preserving the installed configuration; a custom engine with no background work can explicitly report supported empty status. The current attachment and turn-finalization boundary govern idle writes, with paced failure retry and no new model input or no-hook polling timer. Root and independent TS review found no remaining source blocker. The `cabb75f5` companion authors actual A/B engine retention and both WholeBlob/HeadCanonical writer failure, uncertain-commit and live cancellation retries. The suspected writer cancellation defect was withdrawn after tracing existing durable-authority resynchronization; production persistent writer code is unchanged. All new cases remain uncompiled and unexecuted. This is process-local retention and a bounded source checkpoint, not cold-restart custody, graceful shutdown, OS enforcement, full coverage or performance acceptance. |
| Following post-commit feedback | Reference source `f06fe323` and clone/idle controls `ae4f3fd1` are source-reviewed, unexecuted and held for the lead's formatted committed base. The source-only consumer control `06842cf1` targets the missing dispatcher-to-notice feedback path; it is an authored expected-failing case, not an executed failure. The GCP lead accepted bounded capacity reservation and one immutable session-level scheduling-pressure fact in a reserved slot of the same pending owner. Exhaustion stops new optional observers until that notice settles and capacity is rechecked. It does not fabricate per-observation audit, change `dispatch()` errors, replay skipped observers or add a queue, model call or per-operation fsync. Implementation and integrated qualification remain pending. |
| MCP session expiry | Accepted `0f29653787800af941a4d388af2398e9c8c5fd9c` retains scoped source clearance for request-local disposition, exact owner transfer through `into_protocol` and attached-session GET expiry. Integrated successor `a7a1aca5da24bd3cd7f3c04382e1e11d50d88f1d`, tree `32e5f9d699775cf9e288cf5e677408a571974e59`, merges redirect `50d26dc86` and replay `45d3d5cf0` through `a9a470275`. Independent source review confirms the own-POST versus background-GET distinction and the corrected GET405 control, which now waits for client-side classification. A GET404 marks expiry without relabeling an in-flight call; later calls are refused unsent. No session reinitialization/replay or rejected first-GET-404 heuristic is introduced. Owner-reported check, targeted tests, Clippy and docs-check have not been verified against raw logs. Full Agent/provider/mob coverage and combined qualification remain open. |
| Credential-bearing redirects | The integrated `a7a1aca5d` implementation is source-clear for response-observable, request-local redirect classification: a failed call with a response evidencing a followed or stopped redirect becomes typed `OutcomeUncertain`. An own session-attached POST404 remains separately typed. A no-response transport failure after a followed redirect can still become ordinary `ToolCallFailed`; this does not prove no effect or authorize retry. Wording-only successor `3f6595433b42bb2c38611255a631d99ffe461a5f` is independently source-clear: public docs, CHANGELOG and Rust comments now qualify returned-response evidence, disclose the no-response residual and replace universal sent-once wording. It changes no runtime behavior; owner-reported docs-check and precommit formatting were not independently verified. Stopped-redirect diagnostics and the skills-helper attribution are corrected. Ordinary same-origin routing does not promise one physical POST or exactly-once effects, and pre-handler routing remains a deployment property. Same origin does not authorize a different operation or resource, and failed client construction has no permissive fallback. Protected exact-destination calls retain no redirects. The review was sent to both leads; existing owner qualification remains reported rather than raw-log verified. |
| MCP graceful shutdown | Corrected successor `2b2e6ace9779e3d5e717689909b9178b8b8648fe`, tree `deef9060554ef3d410ff2fb8db8f4d7cd523ea76`, is independently source-clear. It preserves all refusal/no-entry/sanitization checks, maps the permitted sibling to its own exposed tool name and asserts its MCP provenance; helper cfgs now match existing Linux-only callers without excluding a Mac test. On the clean native arm64 Mac successor, the changed confinement case passes (1 PASS/179 unselected), and strict MCP all-target/all-feature Clippy exits 0. The test build/body/wall take 13.49s/0.078s/15.23s; Clippy check/wall take 8.39s/8.75s. The same warm target, four-job limit and original execution window were retained. Source remained clean and owned groups drained. The earlier 19 passing selected cases remain attributed to `429d247bd`: ten graceful-close, two unsupported/refusal, three kqueue and four supported-native controls. They were unchanged and not rerun or counted as 20 fresh passes on the successor. The prior `ToolNotFound("echo")` was a fixture collision with the correctly retained old server; the four strict lint diagnostic groups were test-helper cfg mismatches. Their failed logs and the separate initial missing-fixture setup failure remain retained in (operator-retained path), alongside the successful correction logs. These requested Mac findings are closed; this is not a benchmark or full platform acceptance. Earlier `c789c15e7` Linux evidence remains check rc0, 185 MCP PASS/6 skipped and one CLI policy PASS; its five supported-native cases stop at `UnsupportedRequirement`. At subsequent native arm64 Mac head `4fd1aa8da76d18ea3df38962413bf218508e2a05`, tree `ea20fdafe6c7f26f4b5b16466f21fbe247cf1f92`, both router shutdown-cancellation tests passed (2 PASS/180 unselected), with 7.32s compilation, 0.073s test execution and 8.636s wall time, using four build jobs and one test thread. Root independently read `router-tests.log` and `result.json` under (operator-retained path); the raw log SHA-256 is `2a53d1d65dc7655928c321640f92a76e6dc726394928507de11bd79c26bbca85`. Source remained clean and the command's owned process group was empty afterward. These controls establish `Abandoned` for cancellation of a router-owned close and `Reaped`/`ECHILD` for joined failed-connect custody. They close those two router paths, not full process-group or descendant termination or full platform/confinement acceptance. The earlier 19 plus 1 passing cases were not rerun at this head. |
| Continuation hosting composition | The waiting-member repoint production correction `d3a152c34` is independently source-clear: address-backed waits refresh through the existing resolver on store ticks and attachment/settlement wakes, without requiring a new delivery commit. Test-only successor `30d0deff6` closes both fixture findings: generation is checked while A still owns the resolved address, before B becomes eligible; the attachment case uses a process-local delivery store without a watcher and verifies awaiting-only state. Both retain exact single-B admission and empty-backlog assertions. Stable session routing, shared-CAS generation, fingerprinted v2-to-v3 schema compatibility and the callback waiter retain their prior scoped clearance. Hosting and repoint controls use same-process custody and a recording admission sink; owner-reported RED/PASS results do not establish process recovery or combined qualification. |
| Credential fallback | Default-chain correction `6a4c41987..bc27a4e5a` is independently source-clear. Google retains route/client failure types until transient lease observation, avoiding credential-unusable classification. The injected HostAuth client remains a trusted-host precondition. The focused default-chain control does not itself observe a live lease transition or notification absence; owner-reported 95 tests and scoped lint remain separate from integrated acceptance. |
| Platform-layer adoption | R5 `02911ef1` is independently source-clear for the three remaining production corrections and revised controls: explicitly established predecessor realms survive scratch/persistent builder reopen and reject conflicting declarations; Linux process lookup failures remain unknown; preexisting journal stages are preserved, with cleanup limited to this invocation's stage. The real builder tests assert the backend's actual realm, and the staging controls reach a post-stage failure and preserve exact prior bytes. They use an in-memory backend and do not establish remote durable-data or separate-process recovery. Test-only `d01f37b2` retains its scoped clearance for the autonomous host's direct injected-store plus scratch topology. Owner-reported targeted reruns and Clippy remain separate from combined qualification; full adoption, power-loss behavior and persisted jobs/outbox continuity remain unqualified. |
| Integration corrections | Hosting-lock failure correction `72ebb4092`, base test/lint repair `e06ea02a0` and expression-grouping correction `45f368428` are independently source-clear. Their bounded owner test results do not qualify the combined native path. Redirect round 3 `ce438d85e` is independently source-clear; 133/133 tests and Clippy are owner-reported only. The stock SQLite Notification case failed twice at the same fresh-work boundary on the `dde89`/`e06` lineage, including isolated low-load execution; nine sibling cases passed. Source tracing identifies a fixture incompatibility: `claude-sonnet-4-6` rejects the permitted mid-conversation System append before transport. Fixture patch `be24dad3` was integrated as `95e3c3accdf736fae130a2a577955abfb1a88cea`: it selects catalog-supported `claude-opus-4-8` only for Notification, reports early completion directly and preserves the 20-second wait, four HTTP requests and effect/marker assertions. The retained `rerun-attempt1.log` records `stock_sqlite_notification_without_authority_cannot_append_context_or_block_fresh_work` PASS in 0.995s at `27d3c821e722394aca38d9381178186ebcb6334c`, tree `1c9ce1d77772567b805297ec0f3ea4088aaa2727`; the overall run still failed with 23 PASS/3 FAIL. Both corrected fixture files remain byte-identical through `a08ae0d796cd46095bf0221e813783276e32d223` (blobs `e023f930ab3245f1ce89fa82d7bd67514f4b684e` and `12afebd4920c6ef846bae73e234a5b0831559b8b`). This closes the bounded SQLite Notification refusal and fresh-work-after-reopen scenario, not active-turn interruption or positively authorized Notification acceptance, and does not waive unsupported-model behavior. |
| Delegation | R2 `294525bf6f0aa38133f75ce2fcfb08ae9503a6b6` carries opaque inherited authority through service/factory construction and clamps profile tooling to ParentOwned visibility. Final witness correction `579dd4b53ac2dfe2b515ec0073380628b807f1ed`, tree `7676f49bafe03a176e3abd6c1440ab4542b8cde5`, is independently source-clear: conflicting names are removed from an incoming Allow ceiling, retained identities are not overwritten, and a non-Allow conflict is refused. The final regression replaces the original empty-ceiling case with a matching permitted sibling, asserting both witnesses, the exact narrowed ceiling and actual visible projection. Independently read raw logs in (operator-retained path) show the earlier RED as 1 FAIL/1 PASS with `EXIT=100`, then 3 core PASS and Clippy with `EXIT=0`; exact source-to-execution mapping uses the owner receipt. Mounted-name restriction remains conservative for late additions. Hidden-tool locality differs by base: older R2 `579dd4b53` still terminalizes the child, while current native `b74eb56c0` already preserves known hidden-tool `AccessDenied` locally in both shipping precheck paths (`agent/state.rs:7055` and `agent/runner.rs:1200`). The existing delegation fixtures must be composed with that implementation, updated and executed before claiming child continuation or fresh queued-work acceptance. This is scoped evidence, not full governed-continuation qualification or backport acceptance. |
| Respawn preservation | Revised design r3 (SHA-256 prefix `9f07ce89`) is scoped design-clear. Present canonical All is distinct from absent metadata and read errors; unknown authority must refuse locally before predecessor retirement. The proposed MobSessionService operation reads the exact retained predecessor through `PersistentSessionService::observe_authoritative_session_body`, preserving the committed-body seam and post-customization restrictions for ordinary and explicit successors. The opaque handoff issuer and generated DSL mapping remain unimplemented. Predecessor/fence checks must remain, but fences do not substitute for a coherent owner snapshot of the authority being handed off. The design must neither revive the predecessor nor expose a raw-state authority constructor. No implemented preservation or executed recovery acceptance is established. |
| MCP account selection | Production through `7d5a33e83` retains scoped source clearance. Test/docs successor `b8cb065191ad2fc1182b42f1bfdf5c643c275bb9` is independently source-clear: the actual materialized router must remain unchanged after each Known/Discover/Unverified refusal, and the plain-config control must stage successfully. Legacy Known disconnect-first wording now applies to a new login, not ordinary stored-credential use. Production still refuses account fields before session lookup/staging, propagates pending-flow retirement errors before fresh admission and restricts mode construction to configuration. Failed-retirement fault coverage remains open; the new fixture does not perform login, apply the staged server or prove disk-write behavior. Installation-scoped storage remains an incremental checkpoint: actual realm identity, native binding-use admission, attributable administration/audit and portable-profile coverage are still required. Host RPC/REST configuration remains ambient administration, not authenticated local-actor evidence. The injected client-failure test does not prove zero network attempts or unchanged refresh lifecycle. Reported tests and Clippy remain owner evidence; no raw correction-run log was independently read, and combined CI/full acceptance remain separate. |
| Performance | The earlier independently checked depth-1 COW1 diagnostic records trusted/governed means of 2.9062/5.9554ms and 104.92% ratio-of-means overhead. Its governed mean is about 68.3% below opt3 in the same comparison. Macro-side COW and schema-driven renderer parity are source-clear through `2192f4693`, with the separately attributed functional/drift evidence above. The existing owner is proceeding to profiling and attribution. This one-cell comparison does not replace the unchanged six-cell <=10% acceptance study, which remains MISS. Profile window filtering and earlier sample counts are independently verified; retained recording clock/event/binary linkage remains unqualified. No accepted performance result qualifies the integrated candidate; added native authorization p99 below 1 ms and representative turn overhead at most 10 percent remain required. Optional model-review latency is measured separately from the local default. |

The generated native authorization compatibility delta
`27adbf0c39509ce82f40f651fb4c35c2d9fab20e`, tree
`51bb523840e938967c4c44ff1b4ab20b73d456d4`, exposes the existing
`outcome_uncertain` terminal kind in schemas and SDK unions. Review
`65deaed7d778` finds a concrete TypeScript integration defect: the exhaustive
settlement-parser table omits the new value, producing a source-visible type
mismatch and rejecting that value at runtime. Add it to the existing table and
settlement parser control. Python derives its validation domain from the updated
Literal. No compiler, parser test or generation was executed by this review.

Existing named encodings remain unchanged, but older closed enum readers and
exhaustive consumers can reject the addition. The unchanged WireToolErrorClass
is a separate contract. The existing Rust Breaking note needs generated
SDK/schema and closed-reader detail, without inventing a new versioning gate.
The combined Rust source also inserts the variant before AuthorizationRefused,
shifting later implicit ordinals and contradicting the existing preservation
test/comment. That insertion predates the generated commit; the existing owner
must reconcile the intended ordinal contract. Neither unchanged version fields
nor minor-version handshake compatibility prove old-reader acceptance. These
source findings do not establish execution or publication.

Native-auth successor `edfa78c9a40b32bbee73358a24c87071e242b5b7`, tree
`ca7a25bc63c7b9ae89c67d0cca078baabee5e4ca`, corrects the TypeScript table and
parser control, appends `OutcomeUncertain` after the released variants, and
expands the existing compatibility note. However, its regenerated Python type
changes from one flat `Literal` to a union of the 15 prior literals and the new
literal. The unchanged settlement validator compares a string directly with
the union's top-level `get_args`, which contains the two literal aliases.
Consequently every nonempty settlement companion would reject `failure_kind`,
including prior values. This new source defect supersedes the earlier Python
clearance, which applied to 27ad's flat literal shape. Root and an independent
SDK reviewer confirmed the exact type and validator and notified both leads.
The existing owner must retain closed-domain validation while supporting the
generated shape, with prior-value, new-value and unknown-value controls in the
existing parser tests. This is not an executed Python failure or a new runner;
the candidate is not yet source-clear for integration.

Retained clean-edfa results nevertheless establish the bounded existing Rust
and TypeScript checks: 48 selected Rust passes, targeted check and Clippy exit
0, and TypeScript 522 passed/one skipped with exit 0. The raw TS summary does
not identify the skip or prove it was pre-existing. Rust compilation, check and
lint consume about 39 minutes while each selected Rust target reports 0.00s
test execution. The exact-head schema invocation leaves no generated changes;
the captured SDK-generation log belongs to a dirty 27ad ancestor without a
retained diff identity. No Python parser execution or successor qualification
is established by those results. GCP independently identified the same Python
regression and retained the existing author for its correction; the edfa
candidate was superseded pending that correction.

The committed correction `2f80919ea3c8661d34b3c98ec7af65d4a5d52af2`, tree
`2f120a22f6af24eecddbcd64dca3571929bdce31`, closes the Python P1 in source.
Independent SDK and schema reviews confirm unchanged ordinals and wire names,
the same 16-value domain in all 24 schemas, restoration of a flat generated
literal, and recursive literal extraction in the actual closed-domain validator.
New controls cover accepted prior/new kinds and rejected unknown kinds at that
validator, plus the formerly broken union shape. Existing full parser/rewrite
controls remain intact. The new controls do not establish a full roundtrip
specifically pairing `outcome_uncertain` with physical `unknown`; source retains
those independent fields without new retry authority. That limit does not
reopen the corrected membership defect.

The completed qualification starts clean at that exact head at 20:43:11 UTC
and ends clean at 20:48:35. Core/contracts check, all-target Clippy and 19 Rust
contract tests pass. The unfiltered Python SDK command passes 776 tests with 13
skipped; TypeScript passes 522 with one skipped. Source inspection establishes
that Python's default discovery includes the five new validator cases, but its
retained summary has no individual test or skip identities. The full TS log
explicitly passes the settlement-companion case and identifies its skip as the
existing opt-in published-asset network check. Generated files remain unchanged;
separate generator exit codes were suppressed by the owner script. This closes
the correction's source and recorded qualification findings; normal delivery
remains with the existing owner. The earlier 48 Rust passes retain their edfa
attribution. The completed run takes 5m24s, including 5m12s of reported Rust
check, compilation and lint time. These are validation timings, not operational
overhead measurements. Raw qualification log SHA-256:
`2e4e2cfa766775284628dc85cd3007f8de351bc8775e42c929fed17817a12b4e`.

The separate credential-route checkpoint is delivered:
[PR 1827](https://github.com/lukacf/meerkat/pull/1827) merged into main at
2026-10-07 18:13:55 UTC, head `bbef5a611782c8e3630827968e22706431290d7e`,
merge `2585e59b755bacd3e2e340b414ca237daa88fa86`. The current GitHub rollup
records 31 successful and seven skipped checks, with CI and Live gates passing;
the sole failure is Mintlify Deployment after the merge. This is an observed
merge with named gate results, not an all-green rollup or a full authorization
acceptance result. Review `295f12e8d936` retains the exact API snapshot.

That PR does not contain or supersede the generated 27ad candidate. Immutable
source comparison finds neither branch head is the other's ancestor; their
merge base is `863b1def07ab150e2d436540a7e539d81b468758`. The merged enum and
generated TypeScript union omit `outcome_uncertain`, so the candidate's parser
and ordinal contradictions remain separate owner corrections. The nominated
finite .52 integration, R2 and consent branch names have no remote PR/ref in
the bounded delivery lookup at 18:21 UTC. This records delivery state without
adding a publication gate or asserting anything about differently named work.

The later R2 integration candidate
`bba5126e14e7400d1e526aee7c12978c0b37800b`, tree
`1d172b7e32eace05ea6308f6cb655a2e208cec90`, exposes incomplete retained
ceiling evidence. Ruling `9dc260ae8561` distinguishes the invalid positive
facade fixture from actual producer loss. The valid fixture needs its original
identity witness and the narrowed deny union. Independently inspected DSL
staging replaces the shared witness map, and committing All clears it while
retaining inherited names. R2 therefore remains with the existing m3 owner
for the already planned build/turn/resume regression and generated-owner
correction; fixture repair alone is insufficient. Preserve witnesses required
by live inherited/active/staged filters, reject conflicting identity and retire
obsolete ordinary-filter witnesses. Keep missing-evidence errors distinct from
an intentional empty allow-list.

The subsequent independently read main-based regression uses
`863b1def07ab150e2d436540a7e539d81b468758` plus a dirty factory test. It records
zero passes, one failure and exit 100: the first post-turn assertion observes
an empty inherited witness map. Resume is not reached. Compilation took 4m09s
and the selected test 0.041s. Raw SHA-256 is
`f6ead06126fec4349197a9572153a9fb3dafcf26f1a4967c93a9ed4964cfebea`;
the header identifies the dirty file but does not hash its fixture bytes.
Review `fd496f159424` also establishes a supported direct native construction
route: a factory-provided parent composition authority can issue genuine opaque
inherited authority, and `AgentBuildConfig` carries it into the actual
SessionOwned runtime. Ordinary `SessionBuildOptions` conversion drops that
carrier, a separate limitation. This is an executed main-based witness-retention
defect with a source-reachable native route, not evidence of deployed fleet
impact, unauthorized tool entry or post-fix acceptance. The existing m3 owner
continues the same correction.

That regression is now committed as the test-only
`5e859797148cfd29ee5618ed38407e9801841ddc`. Review `a7ffffc0802e` also reads
the separate R2 `bba` plus staged-fixture RED: zero passes, one failure and
exit 100 at the post-turn empty-witness assertion, before resume. Its 3m57s
build and 0.045s test body are distinct from the earlier main run. The
production correction remains changing WIP; neither this committed test nor
incomplete code-generation output establishes a passing repair or respawn.

The subsequent committed correction
`0c1b734f57cebdeecc9d5521584280f3237f22a4`, tree
`f0f573c5318aea66aee66c61e9766816959159a1`, fixes inherited-witness deletion
for valid admitted state. Independent DSL, facade and execution reviews agree;
the five generated phase projections match the changed Stage/Commit rules.
Exact clean-head raw facade results record 484 passes, zero skips and exit 0
after 1m26s compilation and a 1.886s test summary. Raw SHA-256 is
`c46fd5739c2cbbc5e407bd523ae38bf3d66383bf4bf0c346050ba4492535a336`.
This reaches a completed first turn and fresh-runtime reconstruction; the test
only asserts that resumed construction succeeds. Exact resumed ceiling/witness
assertions should strengthen that existing fixture. No second resumed turn,
physical tool entry, durable-store reopen or process restart is established.
The broader owner qualification remains in progress at this inspection.

A later single raw-log read at 18:07:02 UTC establishes three additional
exact-clean-0c commands at exit 0: core visibility 80 passes/1,941 skipped,
runtime visibility 49 passes/2,362 skipped, and schema 307 passes/zero skipped.
Their compilation times are 2m24s, 4m29s and 1m23s; test summaries are 0.409s,
0.173s and 1.123s. Report `00c413c2a452` retains full commands and raw hashes.
Mob visibility was then compiling under the original qualification wrapper;
later serial mob-mcp, Clippy and drift logs were absent. Those selections are
not complete qualification or resumed tool-execution acceptance.

A further bounded read at 18:30:27 UTC records two more exact clean-0c
commands at exit 0: Mob visibility 27 passes/3,060 skipped, and Mob MCP
391 passes/one skipped. Compilation took 15m15s and 7m56s; test summaries
were 0.254s and 23.048s. Review `6b7ec2658469` retains the commands and raw
hashes `5f959618fd7e1b40473bea2f58fee117e63550dd4ec6dfbe5d87df24daafd7ef`
and `bf2e6a60f12ef8222654f092d27b559fffab7f52eb89ab37dda0a3a76ff13151`.
The original wrapper PID 1912218 is verified live, now running Clippy through
PIDs 2619427/2619555; Clippy has no terminal result and drift checks have not
started in that capture. Source remains unchanged, so these passes do not close
the two identity corrections or strengthen the resume oracle below.

Review of 0c identified two pre-existing gaps distinct from the repaired deletion.
The builder merges incoming witnesses before validating retained evidence, so
an overlapping incoming name can fill a lost saved witness. Validate the saved
ceiling first and add the same-name variant to the existing negative fixture.
Separately, after same-name tool identity refresh, staging can replace the shared
witness used by an active-only Allow filter before its revision commits. The
new conflict guard protects inherited names only. Preserve active identity and
visibility until the boundary, accounting for the old/new catalog transition,
and cover it in the existing owner/ToolScope tests. This is source-derived
premature visibility, not demonstrated physical entry. The normal Deny sequence
does not demonstrate stage-induced denial removal. Core missing-evidence refusal
is typed, but the unchanged facade converts it to Config(String). These findings
remain with the existing owner and introduce no separate build campaign or
global publication hold.

The GCP lead initially assigned both remaining R2 identity corrections and
the stronger resume assertions to the existing m3 owner within the finite
0.8.52 slice. Respawn follows R2; actor-teardown corrections remain with their
existing owner. This is execution sequencing, not a new completed result.

Clean `0a18f371015c9e3254f40094f1869fa566bf0675` now resolves the retained
witness backfill and resumed-state assertion findings in source review
`5e7fc75b9102`. The original ceiling is validated before incoming witnesses
merge; the same-name negative control checks unchanged owner state. Resume
asserts exact ceiling and witnesses in both the session and runtime owner.
Existing clean-head logs record 1,338 selected passes, scoped strict Clippy and
both drift checks at exit 0. The later 19:51 capture records machine verification
at exit 0, with Cargo-backed machine kernel/owner tests explicitly skipped.
Review `0eeb6f4a2fd3` retains the complete log and exact command; only its executed
scope is credited. This does not establish a second resumed turn, physical tool
entry, durable-store reopen or separate-process recovery. The active-only
Stage-before-Commit identity issue remains unfixed. GCP's later 18:39 scope
update lists it as an interim 0.8.52 known issue; full 0.9 coverage still requires
its correction. The coordinator remains the project and release-scope owner.

The same ruling accepts proven peer-only absence of local session authority,
but rejects inferring an unrestricted predecessor from a failed or absent
session read. `BeginSpawnExec.inherited_tool_filter_present=false` records only
the incoming handoff's absence; it can coexist with restrictions already in a
resumed session and says nothing about inherited launch policy. The existing
durable owner must establish the predecessor's actual restrictions or their
absence. Otherwise refuse only the respawn operation as unavailable, preserving
the member and unrelated work. No new mirror, witness-backfill mechanism,
duplicate author, build or global pause is introduced.

The more specific Option A review at `f6a3a31e` rejects treating retirement's
current-state classification as proof that a generation never had a local
session. The internal stored-event codec does retain the publicly skipped
backend reference. However, canonical recovery can add a session later, and
retirement can remove its current binding while keeping exact runtime and peer
identity. Reuse sufficient existing actor-owned generation history, with current
runtime/fence checks after reads; missing evidence remains local unavailable
feedback. No new shadow boolean or per-respawn full-journal scan is mandated.
The adversarial control is peer-only spawn, later local binding, release, then
respawn. Proven local-session absence still cannot discard inherited launch
policy. This source ruling does not execute or approve the current respawn WIP
and remains outside the finite 0.8.52 critical path.

Later `97bcd901a02d215ce47c9b728088eb44b153afe3` resolves that history defect
using the journaled generation roster, independently reviewed in `354050f4e08f`.
Retirement release cannot erase its retained Bound(S) fact. A standalone public
bind/unbind choreography is not exposed and is no longer required. The projection
and peer-only controls establish the discriminator; full F6 handoff acceptance
uses the six existing persistent/ephemeral respawn scenarios in
`delegate_profile_family_clamp.rs` plus the existing missing-evidence refusal.
These are identified test sources, not new executed passes or a change to .52.

The subsequent cold-restart question at `a59075b1350bc7a067fa97fdcab5641fb0e37461`
confirms a practical consequence of that rule. The existing partial-resume
fixture drops the old service and deletes the missing member's compatibility
projection, RuntimeStore body, authority, catalog and provisional tail. No
remaining recovery owner can establish its predecessor restrictions. Respawn
therefore returns `RespawnPredecessorRestrictionsUnavailable` before retirement
and retains the exact Broken member. Update that existing scenario to assert
the typed refusal, no successor publication, continued usable siblings and
continued absence of trust in the old endpoint. Document this concrete
lost-authority case. An explicit retirement and fresh spawn remain separate
ordinary operations under current authorization, policy and inherited ceilings;
an acknowledgement cannot recreate the lost restrictions as unrestricted.
Root and independent source review accept this consequence without a new repair
API, harness or acceptance gate. Execution remains with the existing owner and
outside .52.

The successor `55d13f283599558ebe3f844342a9f0aab00569d9`, tree
`8ca76e1b968fdadb1510e7c4bdb7a7530a67b900`, implements that scenario split.
Independent reviews find no material defect in its test, fixture and changelog
delta; the actor change is comments only. The negative control preserves the
exact Broken member after typed refusal. The separate unit control awaits a
healthy sibling's completed internal turn. The real-factory positive reconstructs
the service and reaches successful replacement and reciprocal peer wiring.

Its clean-head attempt9 log records 63 selected Mob tests passing, 3,027 skipped
and exit 0, including the surviving-session positive in 0.421s and lost-session
refusal in 0.764s. Compilation takes 11m09s and the test summary 1.990s. Mob
Clippy also exits 0 after 13m07s. Both new scenarios use 2 MiB worker stacks;
this is not transcript size. Their service reconstruction retains the same
in-memory RuntimeStore, and their default profiles do not add evidence of
nontrivial inherited restrictions. The cold negative's sibling assertion proves
delivery and persisted input, while the separate unit assertion waits for turn
completion. These limits preserve the existing separate-process and restriction
coverage requirements without adding a new gate to this test correction.
The parent `60b12fd77bb1a13276f46408272ee14df64ca016` independently has 9 core,
1 session, 62 Mob and 275 Mob-MCP selected passes plus broad Clippy; those runs
are not relabeled as executed at the successor. The completed attempt9 Mob log
has SHA-256 `6cf935c9d2e3fc04f29928b423b82405a76b41f8fb805fc9717c311bd5120887`,
and its Clippy log has
`bd330ccb83ef6473fd9944d0f5a6cb45b8e7d336e8303b87480b2e15ab88e2ed`.

The related MobKit memory-quarantine candidate
`2c69f52261fee25dca2f06499773739791d29869` is source-clear for the retained
privacy and atomic-invalidation findings. The final two-file correction proves
that the actual viewer lacks memory-read and quarantine-review grants while
retaining the timeline canary assertions. Its two touched tests passed with
28 skipped, and targeted Clippy passed; the inspected raw logs are
(operator-retained path) and
(operator-retained path). Source attribution
comes from the owner's final-head record; those logs do not independently prove
a clean pre-run tree. The unchanged production implementation retains the
previously reviewed 423-test and 13-console-flow evidence. Installation,
publication and CI remain with their existing owners.

Earlier results below remain attributed to their exact source and tested
boundary. Their dated operational status does not supersede this section.

## Earlier source-assembly status, 2026-10-06

The full [acceptance plan](acceptance-plan.md), read through the accepted
[local default](../adr-001-local-governed-default.md) and
[review/consent amendment](../adr-001-model-review-and-consent.md), remains open.
A focused pass or delivered patch does not complete another operation family,
surface or recovery boundary. The 0.9 coordinator leads the work; the existing
GCP owners retain native integration, qualification and publication.

Native candidate `18abf7414123262b7334289cf5c6415c6be6235a` passed its normal
hooked push and reached [PR1730](https://github.com/lukacf/meerkat/pull/1730) at
20:06:13 UTC. The retained log, independently read after completion, records
all normal gates passed, including Clippy, machine/protocol generation drift,
canonical TLC, governance and the workspace deterministic unit, integration
and e2e gate. The full push took 1h44m20s; this is gate wall time, not test-body
or authorization latency. Log: (operator-retained path).
GitHub confirms that exact head but reports a conflict with main after PR1801.
The existing sole assembler prepared merge `0af71cc604353618f6a726246b244b17accd426e`;
its resolution is source-clear, with the reviewed runtime and regression files
preserved. That source passed 15 targeted tests and Clippy, verified from its
retained raw logs. The next prepared merge is
`76ef4d58d5b64413cdf20486d46523779d6435f6`, combining that source with
`c2c38fe9deb3b7ff83100193771fc4e3746b519a`. Its composition is source-clear.
The prerequisite #1796 normal push completed successfully at 21:34:45 UTC,
confirmed in its original log and then by GitHub's published `c2c38fe9d` head.
Its CI and Live gate were running at the next check; release semver readiness
had passed. #1730 still published `18abf7414`, and no Actions runs were present
for `76ef4d58d`.
At 23:18 UTC, the original `76ef4d58d` push was still active after
1h43m19s. Its retained log shows Clippy, machine/protocol drift, canonical TLC
and governance gates passed; the workspace deterministic gate was still active.
That observation establishes neither terminal push success nor successor CI.
Earlier passes remain attached to their original heads. Qualification,
publication, hosted CI and accepted performance measurements for the combined
successor remain open.

| Area | Verified evidence and remaining limit |
|---|---|
| Native human consent | Exact `885554aed055c42e332850c9b5a518036ee82e94` passed 218 focused tests with 2,748 skipped, including seven consent cases. These are library and fenced-dispatch controls, not a full Agent flow or separate-process consent recovery. The raw log is (operator-retained path); compilation took 3m16s and test bodies 1.045s. |
| Human-consent successor | Source review at `5568d2bc01968e12595601917a435fa9fd980c45` is not clear. The existing consent implementation owner has a reviewed repair plan for typed store commitment/reconciliation, callback-free entry reservations, current approver eligibility and machine-owned consent association/use. Compile companion `5d257c610` is source-clear; that does not qualify the pending repairs. Original-run closure/carry, real authenticated host admission, qualified presentation, delivery/wake and restored live-consent custody remain incomplete. |
| Reviewer context and failure feedback | Native original-input context (`15dee74b` patch), supported typed text-appends (`c35c4945`) and successful reviewer HTTP followed by protected observation failure (`ac4ab4ef`) are independently source-reviewed and delivered. Original-input reads retain their actual work owner and independently checked source permission; unsupported media/reference context remains unavailable. Protected observation failure remains infrastructure failure, with no target effect or automatic retry and with permitted sibling continuation. The new fixtures are unexecuted. |
| Recovery | Same-process evidence retains its stated scope. The separately recorded `adc79830f` subprocess fixture proves completed-turn reconstruction followed by fresh governed work under newly installed current grants. It does not prove interrupted continuation, restored live consent or automatic recovery across all surfaces. |
| SDK and Console | Feedback `151d9d62`, optional reference `074bf586`, reference display `774c72a4`, typed gating-result patches and public TypeScript subscription fixtures (`75e087fc`) are source-delivered. MCP setup-reason propagation is source-clear in native Python/TypeScript (`a6faf991`) and stock/shared Console projection (`769d4938`). Point-aware PostTool Console feedback (`9bb2fb59`) is source-reviewed against the accepted chain. Only a canonical per-call result establishes withholding; a bare Observe-hook refusal does not. The sole assembler must regenerate native schema/types; integrated execution and publication remain pending. An approval reference or displayed refusal does not establish action execution. |
| Confinement | Linux `baebee65`, MCP `f7e21419` and command-hook `356acc80` source are queued with the sole assembler. The hook patch binds the exact launch and preserves durable custody plus local PreTool refusal. Durable shell-job refusal codes (`a2c19339`) and typed MCP setup notices (`d67f08ed`) are source-clear. The accepted factory-owned asynchronous MCP feedback fixtures (`5adf304b`) cover actual adapter refresh and next-model projection. Foreground Observe locality source and tests (`fcb6bb39`) supersede the earlier `b14bd9f8` test-only draft. PostTool locality production (`9e205e51`) and strengthened controls (`cc34ea37`) extend the earlier tests (`1b2c5ce2`). The source-reviewed path withholds only the affected publication while preserving entered effects, sibling results, async registration, deferred visibility and settlement diagnostics. It adds a hook-specific refusal instead of claiming the tool never ran; canonical schema/SDK regeneration and execution remain pending. Post-commit Observe continuation and retained refusal diagnostics (`ea6beabd` patch) are source-clear with two new engine-level tests, including a later observer failure. These tests do not establish host/model delivery or durable audit. All these additions are unexecuted. Mandatory Guardrail disposition, full background and post-commit feedback, descendant termination and remaining platform/launch coverage stay open. |
| Background Observe feedback | Engine and active-Agent source (`b4861fc7`) is independently source-clear on the accepted PostTool chain. Scheduling pressure and retained completion facts reach the shared provider text projection with bounded original attribution and fixed failure reasons; concurrent PreTool preparation remains parallel, and notices are retained before event awaits. DefaultHookEngine enforces retention bounds; custom engines must uphold the contract. Factory controls (`d990b39d`) cover a real background engine with injected refusal, exact run attribution, once-only next-request feedback, deterministic concurrency pressure, permitted tool completion and no extra model request. These controls are authored, not compiled or executed; they do not establish native ABAC or OS enforcement. The separately reviewed late-fact bridge below extends this route. Full post-commit feedback, graceful teardown and process-loss recovery remain open. No new external producer mandate or qualification gate is implied. |
| Late background Observe delivery | Frozen source `4c910591` extends the existing PostCommitHookDispatcher and same-session transcript writer. It retains old engine work across configuration changes, uses one passive identity per native completion ledger across clones, and holds immutable pending notices until the actual writer succeeds. An incoming wrapper still becomes the current execution adapter. Unsupported or temporarily unknown lifecycle status can reject a proposed replacement while preserving the installed configuration; a custom engine with no background work can explicitly report supported empty status. The current attachment and turn-finalization boundary govern idle writes, with paced failure retry and no new model input or no-hook polling timer. Root and independent TS review found no remaining source blocker. The `cabb75f5` companion authors actual A/B engine retention and both WholeBlob/HeadCanonical writer failure, uncertain-commit and live cancellation retries. The suspected writer cancellation defect was withdrawn after tracing existing durable-authority resynchronization; production persistent writer code is unchanged. All new cases remain uncompiled and unexecuted. This is process-local retention and a bounded source checkpoint, not cold-restart custody, graceful shutdown, OS enforcement, full coverage or performance acceptance. |
| Integration corrections | Hosting-lock failure correction `72ebb4092`, base test/lint repair `e06ea02a0` and expression-grouping correction `45f368428` are independently source-clear. Their bounded owner test results do not qualify the combined native path. Redirect round 3 `ce438d85e` is independently source-clear; 133/133 tests and Clippy are owner-reported only. The stock SQLite Notification case failed twice at the same fresh-work boundary on the `dde89`/`e06` lineage, including isolated low-load execution; nine sibling cases passed. Source tracing identifies a fixture incompatibility: `claude-sonnet-4-6` rejects the permitted mid-conversation System append before transport. Prepared fixture patch `be24dad3` selects catalog-supported `claude-opus-4-8` only for Notification and reports early completion directly. It preserves the 20-second wait, four HTTP requests and effect/marker assertions. The patch is unexecuted and does not waive unsupported-model behavior. |
| Delegation | Source review confirms that the service build-options conversion drops inherited visibility authority on main `2ae325a50` and v0.8.51. The old assumption that default InheritParent already survives this path is withdrawn. The existing owner must carry opaque authority in both directions, refuse missing required inheritance locally and preserve stricter durable ceilings on resume. Captured visibility is not a current native grant. The reported default InheritParent execution test failed during construction because a WorkGraph namespace grant was missing, so it demonstrated no later tool execution. The independent source finding that the authority carrier is lost remains confirmed; no fix or execution pass is claimed. Successor/respawn predecessor-policy persistence remains separately open. |
| MCP account selection | The account-selection fold `5e4b8955a` and no-permissive-client-fallback correction `3e540968e` are source-clear within the reviewed scope. Installation-scoped storage is an incremental checkpoint; actual realm identity, native binding-use admission, attributable administration/audit and portable-profile coverage remain required. Runtime host-management configuration does not establish an authenticated local actor. The external `meerkat_mcp_add` management tool can carry full config if exposed; the lead selected a typed no-write refusal for account fields on that path, with source and qualification pending. The injected client-failure test does not prove zero network attempts or unchanged refresh lifecycle. Owner-reported suites remain separate from CI and full acceptance. |
| Performance | No accepted performance result qualifies the integrated candidate. Added native authorization p99 below 1 ms and representative turn overhead at most 10 percent remain required. Earlier small-fixture measurements and conditional mean results do not establish these combined gates. Optional model-review latency is measured separately from the local default. |

The related MobKit memory-quarantine candidate
`2c69f52261fee25dca2f06499773739791d29869` is source-clear for the retained
privacy and atomic-invalidation findings. The final two-file correction proves
that the actual viewer lacks memory-read and quarantine-review grants while
retaining the timeline canary assertions. Its two touched tests passed with
28 skipped, and targeted Clippy passed; the inspected raw logs are
(operator-retained path) and
(operator-retained path). Source attribution
comes from the owner's final-head record; those logs do not independently prove
a clean pre-run tree. The unchanged production implementation retains the
previously reviewed 423-test and 13-console-flow evidence. Installation,
publication and CI remain with their existing owners.

Earlier results below remain attributed to their exact source and tested
boundary. Their dated operational status does not supersede this section.

## Earlier source-assembly status, 2026-10-06

The user-authorized `COMPLETE-BRANCH-FIRST-06` direction now governs this work:
finish the coherent implementation branch, its critical regression source and
independent review before starting the next builds, tests, benchmarks or
qualification push. Source commits and reviewed dependencies may stack before
main. The already running r5 push was allowed to finish. This changes sequencing,
not full-surface scope, normal hooks, CI or consumer acceptance requirements.
The coordinator's `SOURCE-GENERATION-AS-IMPLEMENTATION-07` exception permits
the existing GCP build lane to build the minimum exact-source generator and
produce canonical generated sources after source review. It does not authorize
tests, lint, model checking, benchmarks or qualification pushes. Mac work remains
source-only; normal formatting and commit hooks are permitted.

The working branch is (operator-retained branch) at
(operator-retained path), based on
`4ae693198f2fe2aa207557babac1b6482710eede`. Source checkpoint
`8fe1abcd585821f1cc4c56bc89682a272d52635b`, tree
`2dadcba349d400dc816199fe797d1796dadfadba`, integrates persistent controller
custody and governed context append. Normal commit hooks passed. Its dependency
bundle was transferred to the GCP lead and the assigned producer bridge owners.
Audit checkpoint `1ecfb6c76e4345e98d36fb5869152499fda0cf27`, tree
`d70c3711b28c3390eee3a52caca2d817d91650af`, adds exact policy-publication
observations. Normal formatting and commit hooks passed; its verified dependency
bundle was delivered to GCP. The branch also contains `e144d3ec27823389406f2ae0f1e1be9422414539`,
a clean cherry-pick of the publication owner's `796971dfadbea6231932a64bd20e460935f8f026`
example-lock and RPC test-import repair. That cherry-pick did not rerun hooks.
Source checkpoint `50ede58a1fbd69d0ba6005a2a34459aa0c5fd009`, tree
`5440e6a71876d3da45e3028c56c0010cda881e51`, adds the full native model-preparation
cost fixture and strict analysis controls described below. Normal commit hooks
passed, including canonical BUILD freshness after adding the context-append
test source. Its verified bundle was delivered to the lead and assigned GCP
owners. Documentation commit `f4c676314` also records the inspected Codex
context-change cancellation path and the requirement to settle it locally.
Implementation continues above these source checkpoints.
No build, test, benchmark or qualification push has run on this complete branch.

| Included source | State in the complete branch |
|---|---|
| Cold-admission recovery and documentation, `5a45ed0043c81b5b334ac974187e209d180a3091` | Integrated. Prior targeted execution is recorded below and does not qualify this combined tree. |
| Persistent controller-administration regressions, `425b214e0a073232725c3460aa93b8d419c61857` | Integrated; unexecuted. SQLite and InMemory backend custody source is now included, with critical regression source. |
| Notification source/audience regression, `17164c682b326bba160a8c8e32a6d12b902e8ad8` | Extended with persisted raw-unavailable audit, independent invocation/source/destination revocation, positive and duplicate appends, and permitted context in fresh model requests after reopen. Uncompiled and unexecuted. |
| Native cost benchmark and bounded runbook, `cb8fe0f6a1e912a0269ec578af9553e05fe4790c` including `a0d59214708500df568bbd74070c9929c0040461` | Integrated. Full native model-preparation timing and strict analysis source are now included; execution and accepted overhead evidence remain required. |

The remaining batch includes persistent controller and credential continuity;
owner-bound context source/audience admission across live, detached, ephemeral
and staged surfaces; typed local recipient outcomes and multi-subscription
delivery; and the assigned owners' native consent/reviewer, original-work carry
and surface-coverage changes. The coordinator assigned the consent implementation owner the native
review/consent source against `50ede58a1`; root retains design review and integration.
Root integrates the public typed delivery errors
and per-recipient behavior. The mob flake-repair continuation batch owns the shared
durable refused-delivery primitive and version gate; this branch stacks on that
exact source rather than duplicating it. Issues #1497, #1646, #1741 and #1807
remain in the cross-owner integration scope. Source preparation is not acceptance.

The source checkpoint above that integration adds the following behavior:

- Controller administration holds the actual SQLite writer or InMemory lock,
  native admission custody and original-work observations through mutation.
  Its explicit administrative scan visits canonical lifecycle/input records,
  includes orphan and terminal-current-run rows, and has no lifetime history cap.
  Explicit credential publication/removal uses this path; normal acquisition,
  refresh and cached command credential reads do not enter an administration scan.
- Context append has process-bound authenticated and prepared APIs through the
  existing native owner. Invocation, source and destination checks share one
  current publication, run again after queue waits when stale, and settle one
  operation without manufacturing an input or borrowing the latest run.
- The Session owns immutable protected control observations in its existing
  metadata commit. Refusal records no System message. Positive append and its
  observation commit together. Typed delivery errors distinguish refusal,
  authorization unavailable, observation failure and other infrastructure failure.
  Public history and provider messages do not expose the protected audit cells.

These are source-level claims, not executed results. Independent review fixed
the historical-row cap, ordinary acquisition overreach, repeated control-entry
hazards and three test-oracle/path errors before the checkpoint. Full integration
still needs the assigned Mob SendCommand, LiveResponses and job-delivery producer
bridges, retained-work continuation custody and multi-recipient settlement.
Raw governed notification refusal alone is not positive delivery coverage.
The audit checkpoint carries the actual local publication instance and sequence
through coherent preparation, final reprepare and Session settlement, including
denials. The instance identifies the synchronization owner, not a fabricated
policy principal or durable policy-document revision. Missing owner, clock or
coherent reads do not invent an observation. The existing current-check path
still reads the retained local stamp; it adds no per-operation UUID, hash,
registry or owner scan. Critical unit and real native-service assertions are
authored. Independent source review passed after correcting observation type
paths to their actual core module; no compilation or tests have run.

The model timing source uses a per-Agent callback compiled only for tests or
`test-support`. It starts before CallingLlm notice refresh and stops after the
native Entry observation and second currentness check, before scripted transport
and Outcome auditing. The real factory, Agent preparation, authorization owners
and native audit/effect assertions remain in the fixture. The new
`native_model_preparation_ns` contains exactly two ordered spans; the older
provider-only diagnostic retains its distinct meaning. Neither covers actual
provider serialization, header refresh or network time. Independent source
review found no blocker in the measurement boundaries or ordinary-build gating.

The existing `native_cost_matrix` now supports `model_dispatch_tail`, selecting
only boundary instrumentation at depths 1 and 3 with fixed W100/N2000. The
coordinator's shared 1,200-second allocation runs representative mean first,
tool tail second and model tail last. It never resets the deadline or reduces
counts. A model sample timeout retains completed raw samples and fails; the
analyzer reports incomplete counts without timing summaries. A hard process
kill can prevent output entirely, leaving counts unavailable. Critical recorder,
settings and parser tests are authored; none have run. This adds no accepted
benchmark result or full-profile performance claim.

Delivery review found that one committed inbox row can contain several recipients.
A whole-row refusal would misreport an earlier success or discard later permitted
recipients. The two independently reviewed dependencies below are now integrated
through `877bf4df2953a8b442a9c581d2605c24ffc31a28`, tree
`d6fa035ea1f494293181f694f412be9e9f0bbde1`. Normal merge commit hooks passed.
The runtime authority and declarations match the source dependency exactly;
the job facade and regression files match `fc49005f9`. Its recipient path
supersedes C1's intermediate job-side whole-row refusal path. Native whole-row
refusal and legacy envelope handling remain available for other delivery owners.
Canonical generated recipient bodies from `841af4551e00431f1cd1d1055ca1fc25099af59b`
are now integrated. Independent source reviews found no divergence between the
declarations, generated kernel/specifications and native consumers. Executable
qualification remains pending.

| Delivery dependency | Source and remaining integration |
|---|---|
| Existing runtime delivery authority, `db3d15f8a1964f664b10abdf65ffc6c02d993c13`, tree `ad1c5c8304698e0d7ea67ba2c98ccda0e8f50fd3` | Immutable per-recipient manifests and outcomes, truthful `Mixed` group status, exact row binding, version-3 envelope fencing, and critical schema/runtime/SQLite/CAS test source. Normal hooks passed. The GCP lane produced the canonical outputs now included in this branch. |
| Job projector, `fc49005f935e93888a8e30fa239ad66d38395220`, tree `e66d8b944e80dcffa694d9587950d42b7818a9b9` | Complete producer manifest extraction, independent settlement, retry skipping, and paging that counts local settlements without reporting them as applied. Normal hooks passed. Source integration with its generated runtime dependency is complete; execution remains pending. |

The minimum GCP generator build of `08391bdd1` stopped after 74 seconds at an
unsupported DSL `else if`. The reviewed `db3d15f8a` repair uses nested conditional
expressions with identical predicates and branch order. On that repaired source,
the same warm GCP lane completed machine generation in 2m45.76s, protocol generation
in 1m46.20s and BUILD generation in 12.78s, ending at 12:08:10 UTC on 2026-10-06.
Only machine generation changed files. The upstream generated transport commit
disabled hooks; its later check-only commands are not normal commit-hook evidence.
The integration here uses normal commit hooks. No runtime tests, model checking
or benchmarks ran in the generation lane. One combined source qualification
remains with the coordinator after final integration.
The continuation-owner C2-C10 stack and consent source remain separate
dependencies, not claims made by this delivery integration.

The runtime dependency stacks on the lifecycle owner's `5cdde0cc9` and copies
the exact test-only `5cdde0cc9..3f8e6de7e` refusal-reason repeat regression;
`3f8e6de7e` is not its ancestor. No machine behavior was changed for that copy.
The lifecycle owner still supplies the continuation-kind enrollment rejection
test when that row kind is available. Only the composed A/B/C model-checker
witness is deferred for the separately owned map-literal generator support;
real-kernel A/B/C tests and classifier invariants remain in this source.

Critical tests cover allowed A, denied/unavailable B, allowed C and a later job;
infrastructure/observation failures remain retryable, and prior successes must
survive retry without replay. The paging test uses 257 real jobs so a page of
256 local refusals cannot strand the final permitted job. SQLite cases drop all
runtime store/inbox/applier handles before reopening; the producer job store
remains in memory, so these do not claim a full process restart. No new test has
executed. Injected sink errors precede the synthetic effect, so these are not
evidence for retrying an unknown effect. Durable per-recipient disposition stays
with the existing delivery authority. Effect execution and settlement commit
remain separate; a missing settlement does not establish safe replay.

The native review/consent design has two independent source reviews. Corrections
require an explicit non-executing R3 path until qualified human support exists,
currentness at physical entry after preparation, exact owner-bound attempts,
truthful cancellation causes, and native enforcement when a required reviewer
is missing. The first review-path checkpoint does not complete Slice A's human
decision, shared consumption, notices, host projections or restart obligations.
All critical new tests, backend durability behavior and overhead remain unqualified.

## Published native checkpoint

The 0.9 coordinator leads the cross-repository work. Native
[PR1730](https://github.com/lukacf/meerkat/pull/1730) is the current publication
checkpoint. Commit `24efa210844e640c6907babb485cc745118c8801`, tree
`c10c045bca384e0b64bb7460bbaf8dfe3b2454cb`, completed the normal hooked
fast-forward push on GCP at 05:35:01.899 UTC on 2026-10-06. The command exited
zero after 2h05m53s, with no skipped hooks, bypass or force push. The owner
verified the remote branch at the same commit and the source checkout clean.

All normal hook stages passed, including strict workspace/all-target/all-feature
Clippy, machine verification, generated/Bazel freshness, and the deterministic
unit, integration, HeadCanonical cold-restart and E2E-fast gate. The successful
runner does not retain per-lane counts, so this checkpoint claims no new test
totals. The raw push log is retained locally at
(operator-retained path) and on GCP
at (operator-retained path), with SHA256
`db5499442ed2689acc64df4eb65f72ecf1557a9a8708367fc014d376d9247ec2`.

Frozen successor `4ae693198f2fe2aa207557babac1b6482710eede`, tree
`99434c3a32f55c3450d66e30aef36769d7e85232`, integrates main through
`d28ca32e54f66547d4a3c7f6f5cef58558bf5b25`. It preserves governed refusal
continuation, read-only restrictions across direct and resolved entry
repreparation, prepared provider dispatch and the incoming delivery controls.
Its only change from `a3b2646110e854aba852a14e239e332a2fdaa07a` initializes two
missing fields in the pending-callback test fixture; production source is
unchanged.

Exact `a3b264611` passed all 16 Linux-selected `native_governed_loop` tests, with zero failures,
ignored or filtered cases, in 0.81s test time and 3m20.45s including compilation.
Both workspace all-target checks, including the supplementary `--keep-going`
check, failed solely on that fixture's E0063. The additional core test command
was stopped before execution. Raw source, results and timings are retained
under (operator-retained path).

Exact `4ae693198` then passed the locked workspace all-target check in 1m22.19s,
both direct/resolved read-only repreparation tests in 2m21.63s, and the prepared
OpenAI dispatch test across public OpenAI, ChatGPT and Azure backends in
2m44.18s. All three selected tests passed with zero failures or ignored cases;
these command times include compilation. The checkout was clean and unchanged
before and after the 08:07:14-08:13:42 UTC run on 2026-10-06. Root read the seven
raw source, log and timing files, and the monitor independently verified full
remote/local hashes. They are retained under
(operator-retained path). The 16-case result
remains attributed to a3; that suite was not rerun for the test-fixture-only
repair.

The owner reported the superseded c6a normal push stopped at 08:14:10 UTC, with
its owned process group 3042014 drained and the remote still at `24efa210`.
Its pre-runtime gates, unit lane and integration lane had passed, but
`cold_restart_mob_resume` was compiling when stopped; this was not a completed
push. The normal hooked r5 push started at 08:14:20.468 UTC using an explicit
refspec with the lease tied to `24efa210`; the source update is a fast-forward.
The r5 push finished successfully at 10:17:14.196 UTC, exit zero, after
2h02m53s. Every normal hook passed, including the deterministic unit,
integration and E2E gate; there were no lane timeouts or skipped hooks. The
runner printed no per-lane test counts. The owner verified the remote at exact
`4ae693198f2fe2aa207557babac1b6482710eede` at 10:17:14.599 UTC and the source
checkout remained clean. Root read the raw log, command record and timing
result after the monitor verified their transferred hashes. They are retained
under (operator-retained path) and
(operator-retained path); `push-raw.log` has SHA256
`dd5a755bb9329b7701e8e824bee743cf8e3b2465f45a984d2319150149fe784f`.
GCP owns hosted CI and merge; push success does not establish either. No
accepted overhead benchmark or full-surface completion is claimed.

Hosted CI run `37448920637` failed its Format + governance job at the example
lock check on 2026-10-06 at 10:20:42 UTC. Root read the
[failed job log](https://github.com/lukacf/meerkat/actions/runs/37448920637/job/112220509633).
The root Cargo.lock resolved under `--locked`; the locks for
`examples/034-codemob-mcp` and `examples/035-mdm-tux-rs` required updates on
GitHub's synthetic merge `fbd1e4e47881`, whose parents are main `d28ca32e54`
and PR head `4ae693198`. The CI aggregate therefore failed. The separate Live
gate passed, which does not override the failed lock gate. The concrete failure
was sent to the release lead/coordinator. The coordinator confirmed that the
existing GCP publication lead owns this and a separately reported missing
`Duration` compile error as one correction. Root subsequently read the
[Governed JSONL failure](https://github.com/lukacf/meerkat/actions/runs/37448920637/job/112220959789):
RPC test compilation failed with E0425/E0433 on the unimported `Duration`, before
test execution. The lead's repair is stacked locally as described above. Its
normal GCP push started at 11:06:01 UTC with log (operator-retained path); no terminal
push result or new green hosted CI result is claimed here. No duplicate repair,
retry or qualification push was started here.

The separate accepted cold-admission/docs follow-up is
`aa4395c845948f97f44570f654f03c5e454cc9fb`, tree
`bfce5db6767760bb5403a6d07dda38408d697703`. Its tested change at
`adc79830f67cc004bbb474460d7428dc671b8636` extends the existing separate-process
SQLite case: current-grant input receives typed `LeaseAbsent` readiness before
credential-owner restoration, with zero HTTP/tool entry, no new live or durable
input, and unchanged prior input audit, committed document and decoded token.
The same input then completes the governed model-tool-model path after canonical
restoration; the old-grant denial and permitted sibling controls remain.

The exact `stock_sqlite_separate_process_reopen_runs_fresh_governed_model_tool_model_work`
selector passed on GCP on 2026-10-06, from 07:13:42 to 07:17:03 UTC: one passed,
zero failed or ignored, 15 filtered, 0.79s test body and 3m21.04s total command
time including compilation. Source and tree were unchanged and clean before
and after. Root read the source, raw result and timing files and verified their
hashes after transfer. The raw log is
(operator-retained path), also retained at
(operator-retained path), SHA256
`56b50326025dc5c8b18aa9ad9388070088d0801f8f5a9a8301564b7a7773bf2c`.
This is targeted acceptance, not full follow-up CI or performance evidence.
Its r5 integration at `5a45ed0043c81b5b334ac974187e209d180a3091` is now included
in the complete branch. The separate follow-up push was cancelled under the
complete-branch sequencing decision.

Documentation commits `92bd9a087ca3c08d7e6e59bd90c08bec72fe3fb5` and
`b8b9045f0940d27b2f4a3a1ba584f53d183d0108` correct absent approval API claims,
align consent commit semantics with the accepted amendment, and explain native
governance at CLI and shared-configuration entry points. The public documentation
checks passed for 120 pages and 54 tests; normal applicable commit hooks passed.
These notes add no runtime behavior. GCP retains compilation and publication
ownership. Qualification of the complete source and then its quiet performance
window replace the earlier per-fragment execution plan. The 0.8.52 cut remains
a resource consideration, not a source prerequisite or an additional idle wait.

Persistent-administration preparation at
`425b214e0a073232725c3460aa93b8d419c61857` is integrated in the complete branch.
Its source-reviewed tests are uncompiled and unexecuted; the earlier
cold-admission pass does not qualify them. The implementation must satisfy
credential removal/replacement and detached durable-original controls before
the combined branch is submitted for qualification.

## Earlier qualification history

The previous integrated baseline is
`64349561d898d0a9e3ad279fa1a7d286ff7b302f`, tree
`2041118ca2f545541a1a05cccccb908765307589`. It integrates main through
`98e26901ba24c0b2154192e17590cdb3773eb3b5`, the custody fixture correction,
the accepted model-review/consent ADR, and a test-only OAuth fixture migration.
Documentation commit `c982b3b52ca7f0dff3d25328bb73caeeb3baf3e1` additionally
refreshed this record and the Codex source comparison. At the coordinator's
direction, commit `c25ba2969b1c4e66be05e1f8150cc56c173958a8` integrated main
through `be2932374793ae84d3b844129cf58229509a1f0c`, including connector OAuth.
The current successor also integrates actual main merge
`74b6c7473797cb904f554c8c2cc1bb7386ec3c66`, containing the upstream memory-test
correction described below. That correction changes no production behavior.
Qualification of that integrated source, `f3ebf2a02e19330aa926b61a205fcd62466ca2fc`,
failed as described below. Its fixture successor completed GCP qualification
with two test failures; earlier published results did not qualify it. Luka's
explicit 2026-10-05 instruction in the 0.9 coordinator chat temporarily removed
GitHub CI as a blocker during the Actions outage. Those historical CI omissions
remain recorded as skipped, never passed.

MobKit's approval display checkpoint [PR594](https://github.com/lukacf/meerkat-mobkit/pull/594)
merged as `e09a3c33e25c96a29cff30ec2923eaccc7a9c7ba` at 22:15:37 UTC.
Its final source passed 726 console tests and all 14 existing static, build,
freshness and inventory commands. Inventory checks are not E2E executions.
The operator guide [PR603](https://github.com/lukacf/meerkat-mobkit/pull/603)
merged as `62bef663cdbae8d59340d5d85d09650b698bf318` at 22:20:43 UTC after
normal applicable documentation hooks. Hosted CI for both merges was skipped
under the explicit outage exception. These changes display existing decisions
and document their limits; they do not implement the native model reviewer or
establish release/deployment acceptance.

The Mac integration commit `eff3fcddf2dd3e86816d3cd02b2a986276958a8a`, tree
`91f03eca38fc1c3c732d297998330af9915ebe9a`, integrates main `851fd7de9`, including PR1749's retained
member lineage and PR1760's in-memory ops lifecycle deadlock repair. The store
merge preserves native custody and epoch retirement while placing ops snapshots
and retirement fences behind the incoming separate short-lived mutex. CI keeps
both governed JSONL and upstream TLC requirements; changelog entries from both
parents remain. A source review also migrated the newly added CLI and retained
metadata fixtures to the fallible native constructors. These are source-level
integration findings, not executed test passes. Normal merge commit hooks passed
in 11.55 seconds. Existing GCP `c7f42abb` results remain baseline evidence;
the final normal Mac gate must qualify the integrated successor. Routine
unrelated main changes do not require repeating this integration.

Clock times in the qualification history below are UTC on 2026-10-05.

The GCP run lasted from 20:39:20 to 21:05:56 UTC on the exact `f3ebf2a02`
source. Workspace library, integration and E2E inventories stopped during
compilation and ran zero tests. Three fixtures had not adopted the fallible
`PersistenceBundle` or `MobMcpState` constructors: the shared delivery-inbox
fixture, the stuck member-retirement fixture and the profile tool-denial fixture.
The separate `cold_restart_mob_resume` target passed all nine tests with zero
skips in 2.751 seconds of test execution; its command took 7m49s. That result
does not qualify the inventories that did not run. Raw command output and timing
remain under (operator-retained path) on GCP.

The redundant Mac normal push was stopped after the deterministic compile
failures were reported. Its process exited 143 and its owned group was confirmed
absent at 21:05:58 UTC; no push occurred. The fixture successor `970fa7a0181ae8aa1fdb05b4024794c387efe21e`, tree
`2beafcd1907974dedf1ac291e921187f43c6a0de`, handled constructor failure in those
three fixtures and three analogous live/replay fixtures, including fallible mob
composition. Its local workspace all-targets check with the replay feature
exited 101 after 1424.17 seconds and ran no tests. It found one remaining setup
mismatch: the profile tool-denial fixture also needed to handle the fallible
`wire_mob_tools` result. All nine other calls already handled that result. Raw
output is `envelope-main-fixture-all-targets-check.log` in the local evidence
directory below.

The sole GCP qualification owner applied the fixture changes as `feebf5f327`,
whose tree is identical to local `970fa7a01`, and added the final positive-fixture
unwrap as `c7f42abbd2fb9bb3df0b724a83f6df8ce3659681`, tree
`c3d5133b9b68c8a31d0fa2d3ca3af711017b0f0e`. Root verified the returned bundle
and the one-line difference against local `970fa7a01`. The GCP owner reports
format checking and the default-feature workspace all-targets check passed;
the latter ran from 21:59:47 to 22:10:37 UTC. That command did not include the
replay feature. The four test commands completed at 22:42:16 UTC:

- Workspace library: 13,021 passed, one failed and 19 skipped. Command wall time
  was 1,114.63 seconds; test execution took 380.644 seconds. The live-close test
  failed its independently started clocks' 150 ms lower bound at 149.894422 ms
  after its `Ok(None)` assertion passed. This unchanged upstream timing oracle
  is tracked by [issue1779](https://github.com/lukacf/meerkat/issues/1779), with
  its correction owned separately. There was no retry.
- Workspace integration: 3,098 passed, one failed and 83 skipped. Command wall
  time was 454.53 seconds; test execution took 452.847 seconds. The transcript
  retention fixture expected version 3 after an explicit write, conflicting
  with this checkpoint's version 4 writer. This is an integration regression
  in the test expectation, not a passing inventory.
- Cold restart: all nine passed, zero skipped. Command wall time was 325.06
  seconds; test execution took 2.845 seconds.
- E2E fast lane: all 30 passed, zero skipped. Command wall time was 3.66 seconds;
  test execution took 2.576 seconds.

All command results and source identity are retained under
(operator-retained path), copied and hash-verified
against the GCP owner's raw receipts. The GCP transport commit skipped commit
hooks; its explicit format check and normal Mac hooks are separate evidence.
These results do not qualify the later main integration or establish performance.

The normal Mac push of `eff3fcdd` started at 22:28:59 UTC and was stopped at
22:44:25 UTC after the deterministic version mismatch was reported. It exited
143 after 925.78 seconds; its owned process group subsequently drained and no
push occurred. Preliminary checks and formatting passed. The lint hook was
still building prerequisites, so lint and later gates did not pass. Raw output
is `envelope-main-851-normal-push.log` in the local evidence directory above.
The narrow successor corrects the retention test to assert a version 3 input,
a version 4 write, and exact equality of every other JSON field. It changes
neither the historical fixture nor production behavior; its focused execution
passed the affected test, with five unrelated tests unselected. The command
took 112.52 seconds, including 1m51s compilation and 0.040 seconds of test
execution. Independent source review confirmed that all history, message,
metadata and subsequent retention assertions remain intact. The focused pass
does not qualify the broad candidate. Raw output is
`envelope-retention-v4-fixture-green.log` in the same local evidence directory.

The retention correction committed as
`1394677f2768fdaa49290f3746aa8214596daf22`, tree
`e5bbf083fe8c7c04446b35ec3393028ed51d4262`. Its normal push started at
22:53:41 UTC and exited 1 after 1,664.44 seconds; terminal state was observed
at 23:21:43 UTC and the owned process group drained. Preliminary checks and
formatting passed. Strict Clippy rejected the runtime admission closure at
`meerkat-session/src/ephemeral.rs:6422`: its error tuple had grown to at least
128 bytes. Later gates did not execute and no push occurred. Raw output is
`envelope-main-retention-normal-push.log` in the same evidence directory.

The narrow repair boxes the owned `ToolError::HookDenied.denial` and
`AgentError::PolicyIndeterminate.failure` payloads. These nested error variants enlarged
`SessionError`; the admission guard and closure are unchanged from main.
Typed classification, serialized hook denial shape and settlement diagnostics
remain intact. The allocation occurs only when constructing those failures.
The public Rust fields now contain `Box` values, so downstream explicit
constructors and owned payload consumers must adapt. Two independent reviews
cleared this source delta. Scoped strict Clippy for every `meerkat-session`
target passed with warnings denied in 168.70 seconds, starting at 23:35:21 UTC.
The command exited 0 and its owned process group drained. Raw output is
`envelope-error-payload-clippy.log` in the same evidence directory. All six
selected denial, settlement and admission controls passed across four binaries;
5,059 other tests were not run. That command started at 23:38:55 UTC, exited 0
after 1,068.96 seconds, and its owned process group drained. Compilation took
17m46s; test execution took 2.113 seconds. The controls preserve same-turn
siblings after refusal, exact hook payload roundtrips, the original infrastructure
failure class, joined admission capacity and the terminal carrier. Raw output is
`envelope-error-payload-focused-green.log`. Normal publication qualification
remains pending; these scoped passes do not qualify the broad candidate.

That repair committed as `e89d3cae0c6be29e49852ba01e2ab87055b54838`, tree
`33a7e23fe8b63540aa5947abe7f629959782a015`, after normal applicable commit
hooks passed in 10.32 seconds. Its normal push started at 23:58:57 UTC on
2026-10-05 and exited 1 after 3,152.13 seconds. Terminal state and full process
group drain were observed at 00:51:34 UTC on 2026-10-06. Preliminary checks and
formatting passed. Broad strict Clippy found two test-only `ToolResult` literals
in `meerkat-mob-mcp/src/live_delegation/peer_replies.rs` that omitted the added
`settlement_failures` field. Later gates did not execute and no push occurred;
the remote branch remained at `afb79ea29d8cdf30689345d884b43bc6c5baa428`.
Raw output is `envelope-error-payload-normal-push.log` in the same evidence
directory. The successor initializes both fixtures with an empty diagnostics
vector, matching the existing constructors and preserving all positive and
negative assertions. Independent source review confirmed that neither fixture
represents a settlement failure. A whole-workspace source sweep, including
live/replay fixtures, examples and the core result alias, found no other
incomplete literals. Production code is unchanged. Normal publication
qualification remains pending.

That fixture correction committed as `fcd5115487897a252cadb408d2ee989fc935ceb6`,
tree `765df7331470c1dfb29edc15c8846f5075045586`, after normal applicable commit
hooks passed in 10.08 seconds. GCP's locked workspace all-target/all-feature
strict Clippy passed on that exact clean source in 1,065.83 seconds, from
00:58:14 to 01:16:00 UTC on 2026-10-06. Its raw log, timing and pre/post source
identity are retained under `adr-precheck-fcd51154/` in the local evidence
directory. This is compile/lint evidence, not runtime test execution.

The normal Mac push started at 00:57:44 UTC on 2026-10-06 and exited 1 after
1,254.81 seconds. Its terminal state and full process group drain were observed
at 01:18:43 UTC. Preliminary checks, formatting and broad strict Clippy passed.
Machine verification then found a stale hand-written run-start hold audit call:
the generated admission operator now takes twelve arguments, including optional
authority binding and batch key, while the audit still supplied ten. TLC stopped
at semantic analysis. No push occurred and later runtime test gates did not run.
Raw output is `envelope-peer-replies-normal-push.log`.

The successor supplies `None` for both added arguments, preserving this audit's
ordinary unbound input. Generated models, production code, guards, properties,
witnesses and bounds are unchanged. Two independent source reviews confirmed
the mapping. The existing `run_start_hold_audit.sh 12` command passed in 58.37
seconds, including safety and all seven required reachability witnesses. Its
optional mutant checks were not selected. Raw output is
`envelope-run-start-hold-green.log`. Normal publication qualification remains
pending. A full source sweep against integrated main found 66 changed admission
signatures across the native and composition models; all 69 corresponding calls
in the 35 tracked TLA files now supply twelve arguments, with no stale aliases
or qualified references. This sweep is source review, not additional TLC runs.

The Mac candidate adds one test-build correction to that GCP source:
`StdioChildCustody::spawned_pid` is compiled only for Linux tests, matching its
only caller. The Mac compile check emitted an unused-method warning for its
former all-platform test configuration, which conflicts with the normal strict
lint policy. Both predicates are true for Linux tests, so their helper and
caller behavior are unchanged. This is a source-level conclusion; the final
normal Mac gate still must execute. The recorded GCP source remains unchanged.

The connector merge needed four integration repairs: preserve typed stale
preparation, use coordinated mode-constrained logout, adapt the generated
refresh transition return type, and reuse the held lifecycle guard during cold
credential recovery. The mode check and credential clear share the existing
coordinator transaction and guard; foreign-mode credentials remain untouched.
Two tests were written first using a fresh runtime owner over a real committed
credential. Both failed on the intended five-second deadlock timeout. That RED
command took 185.39 seconds, including 2m54s compilation and 10.031 seconds of
test execution. After guard reuse, eight selected controls passed across two
binaries in 14.36 seconds, including 13.59 seconds compilation and 0.071 seconds
of test execution. They cover cold status/bearer, foreign-row logout protection,
failed-clear status, scope/account refresh refusals and infrastructure error
classification. Raw logs are
(operator-retained path) and
(operator-retained path).
These are focused integration results, not full candidate qualification.

The native integration regression was executed before its repair: acquisition
incorrectly accepted a builder-seeded runtime owner from a different store.
After the repair, all eight selected tests passed across four binaries. They
cover persistent owner/store conflicts, the typed conflicting-owner outcome,
ephemeral service ownership, and queued work surviving model-visible permission
refusal. The green command took 688.77 seconds, including 11m26s compilation;
the test bodies took 0.737 seconds. Its raw log is
(operator-retained path).

The auth-core test library initially failed to compile because its refresh tests
still used the removed `BrowserOpener` fixture. A test-only migration to the
host-driven login API retained the refresh cases. The targeted
`mcp_oauth::tests::ce_mcp_stale_error_roundtrip_remains_typed` then passed,
including the assertion that stale preparation is not a permission refusal.
The command took 36.61 seconds, including 34.94 seconds compilation and a
0.017-second test body. Its raw log is
(operator-retained path).
Normal commit hooks passed. These nine selected passes do not establish complete
surface coverage or replace the pending normal publication gate.

The combined runtime-correctness candidate
`e27338d97be661c1eb4e7670809020b91cba37f7`, tree
`c92a3ef7ceded9e1d475bfa7ba4916a537604383`, failed its second normal push at
19:05:07 UTC after 6474.52 seconds. Lint, machine, generated-header, Bazel
freshness and workspace compilation passed. Both unit attempts timed out at
900 seconds. The first ran 7,070 of 12,407 selected tests: 7,068 passed, one
memory assertion failed and one test was terminated; 14 were skipped and
5,337 selected tests never ran. The retry ran 6,891: 6,890 passed, one was
terminated, 14 were skipped and 5,516 selected tests never ran. Integration,
HeadCanonical and E2E did not start, and no push occurred. The raw log is
(operator-retained path).

The real memory failure also applies to the native candidate: the source is
identical. The released-v2 purge/reopen test had confirmed three durable rows,
then required approximate HNSW search to return all three and received two.
The local trial correction retained all earlier migration, staging, finalize
and search checks, verified the exact three durable texts through scoped
enumeration, and checked that lazy rebuild indexes three points. It also checked
nonempty, nonblank reopened search results. The affected test passed in the root
worktree's own target in 118.03 seconds, including 1m57s compilation and a
0.033-second test body; 65 unrelated tests were unselected. Its raw log is
(operator-retained path).
That targeted pass does not qualify the failed broad inventory. GCP independently
reproduced the assertion on current main and owns the upstream correction in
[PR1759](https://github.com/lukacf/meerkat/pull/1759), inspected at
`915fdab04714e318a8ccd148de7a4f934507196d`. The PR uses the same exact index-count
and durable-text checks, preserving the earlier search assertions. Root withdrew
its duplicate hunk and retained it at
(operator-retained path).
GCP reported runner capacity as the cause of cancelled CI jobs. Under Luka's
outage exception, its owner merged PR1759 at 20:29:08 UTC as `74b6c7473`, with
CI explicitly skipped and local evidence recorded on the PR. The actual merge
is integrated here. Fresh candidate qualification remains pending; the earlier
local trial is not an executed pass of the upstream variant.

The combined cohort's owned processes drained. PR1730 has the next normal push,
using the existing isolated
detached target and a 1,800-second unit timeout
within the unchanged two-hour overall bound. Publication does not require
merging the separate runtime-correctness branch first. One broad test cohort
remains active at a time because socket and timing tests share host resources;
independent source review and documentation continue in parallel.

The [model-review and consent amendment](../adr-001-model-review-and-consent.md)
incorporates the inspected Codex implementation and the accepted host deltas.
It keeps default authorization local, treats optional reviewer unavailability
separately from denial, and requires operation-local feedback with continued
sibling and queued work. A reviewer cannot grant missing native permission.
This is accepted design direction, not implemented reviewer, consent-consumption
or host-integration functionality. Approval-store atomicity remains tracked by
[issue1741](https://github.com/lukacf/meerkat/issues/1741).

Performance acceptance remains open. No measurement qualifies the current
integrated candidate. The historical six-cell study below missed every
conditional mean threshold; fresh-turn overhead was 137.01 and 145.20 percent
at grant depths 1 and 3. The required added authorization p99 below 1 ms per
operation and representative overhead at most 10 percent remain unchanged.
The accepted W20/N32 representative study follows functional qualification and
optimized correctness at the earliest actual quiet GCP window. Platform-layer publication
is not a prerequisite, and no idle window is reserved. It must finish within
20 minutes. It measures conditional mean ratios, not
individual-operation p99; the latter still needs separate valid evidence.

Full implementation remains incomplete under the
[acceptance plan](acceptance-plan.md). Required follow-ons include interrupted
recovery, persistent authority administration, principal/peer/callback carry,
all surface and operation-family activation, the independent consumer integration, supported
OS confinement, model review and human consent, and measured overhead. Narrow
native or Console checkpoints do not satisfy those remaining requirements.
The dated sections below preserve historical evidence and do not supersede
this current delivery status.

## Historical delivery status, 2026-10-04 at 21:03 UTC

Native PR1634 is published at `b12d81e71e57bfeb2c3bc677c21c782f57cc3df6`,
tree `1f9553187e12d0a7f91ac0cdd0ff54e56e658c77`. All normal push hooks,
including the workspace unit, integration and E2E gate, passed in 117m48s,
ending at 17:52:00 UTC. Successful hook output does not retain individual
lane counts; no counts are inferred from that result. The raw log is
`publicb12d-normal-push-r1.log` in the retained evidence directory.
[Fresh CI37222144847](https://github.com/lukacf/meerkat/actions/runs/37222144847)
passed 38 jobs with four intentional skips in 22m13s; its final gate completed
at 18:14:19 UTC. The exact three governed JSONL cases passed with zero failures
or ignores in 0.21s test bodies. [Semver readiness](https://github.com/lukacf/meerkat/actions/runs/37222144843)
also passed. Raw final CI JSON and governed JSONL logs use the `publicb12d-`
prefix in the retained evidence directory. Predecessor `00b0cab3` remains
separately CI-green. Performance is
still unacceptable: the latest six-cell study missed every conditional mean
threshold, with fresh-turn overhead of 137.01 and 145.20 percent at grant
depths 1 and 3. Individual-operation p99 and full coverage remain open.

Console [PR520](https://github.com/lukacf/meerkat-mobkit/pull/520) is published
at `b997bdcc5bcab37f7080215099c1d5be1307b892`, tree
`2b9e7d232dbce80c82e58b0824279c74575e52d0`. Local queue/send-attempt suites
passed 102 cases and the adapter suite passed 317, all without skips. Normal
commit, embedded freshness and applicable push hooks passed. However,
[CI37230079692](https://github.com/lukacf/meerkat-mobkit/actions/runs/37230079692)
failed: nine jobs passed, Console failed and the aggregate gate failed. The
5,000-turn send-streaming browser scenario timed out. A source reproduction
finds anonymous historical tool results appended after the assistant snapshot's
observation boundary, leaving the queue busy after the current run completes.
The real gateway can emit this ordering too. Repair and fresh CI remain open.
Predecessor `86e0cdf0a17d9727c90ba036a8b9dbca97c521b8` passed all 11 CI jobs
in 39m34s. These are Console projection results; MobKit still pins Meerkat 0.8.50.

The preview safety baseline is committed at
`64b44ff9d0fb6d3118c324b20e0bcc4449ac6659`, tree
`aeceef3a8ed608b6fab842cf62f80e1595534a49`. Two preview tests and the existing
archival/recovery test passed with zero failures or ignores; normal commit
hooks passed. They cover isolated effects and full state, binding rejection,
retained poison, and cleanup. They do not prove that a poisoned preview invokes
cold validation. The first
compile attempt used a stale macro artifact and ran no tests; rebuilding the
correct macro took 2m31s before the two preview cases passed in 0.04s.
Raw results are `costf9-preview-live-fork-baseline-r1.log`,
`costf9-preview-live-fork-baseline-r2.log` and
`costf9-preview-live-fork-archive-baseline-r1.log` in the retained evidence.

The reviewed production preview change is now under qualification. Healthy
locks use the generated authority's existing isolated fork; poisoned locks
retain cold recovery validation. On GCP, the same two preview tests passed
with zero failures or ignores after 11m13s of optimized compilation. Three
existing provisional-transition tests and the archival/recovery control also
passed, with test bodies reported as 0.00s. Raw logs use the
`native34f-preview-` prefix in the retained evidence directory. Both optimized
native-workload correctness tests then passed with zero failures or ignores in
8.05s, after a 6m18s rebuild. These eight controls cover local refusal feedback,
permitted effects, revocation, queued-work cancellation, isolation and cleanup.
Candidate `4cffdf3752574a886a0024bfe70f59dde3b5d254` failed its normal
push after 41m26s in Clippy. E0275 occurred while proving that the existing
CLI RPC-deployment test future is Send; later test gates and fresh CI were
not reached. A reviewed test-only explicit Send boundary then passed the
same workspace, all-target, all-feature Clippy command in 7.58s using the
retained warm target. The existing CLI RPC-deployment scenario then passed
with zero failures in 0.158s after a 9m24s compile, finishing at 19:37:46 UTC.
The test-only repair is committed in candidate
`d6f2369fab6caed654caa992ffe9423d1e3736a4`. Its normal push failed after
66m21s at 20:46:42 UTC, before integration/E2E: 12,796 of 12,798 unit tests
passed, two OpenAI authorization tests failed, and 22 were skipped. One success
fixture received two HTTP requests; the transport-failure fixture did not see
a transport error. The failure tests release an ephemeral port before using
it, allowing another parallel test to receive the request. A repair retains
port ownership through an actual request followed by transport closure across
OpenAI, Anthropic and Gemini. All 39 selected authorization tests passed
across the three provider binaries after 2m33s of compilation, with 0.07s test
bodies and 617 unrelated tests excluded by the selection. This includes both
previously failed cases. The scoped raw log is
(operator-retained path).
Normal publication and fresh CI for this repair remain open. The full failed push log is
`publicd6f-normal-push-r1.log` in the retained evidence directory.
No production allocation or recursion-limit change is part of the CLI repair.
The adapter consumer's bounded preview source review found no blocker; it does not supply
missing execution or performance evidence.
Raw logs are `public4cff-normal-push-r1.log` and
`native4cff-cli-send-clippy-r1.log` and `native4cff-cli-send-scenario-r1.log`
in the retained evidence directory.
No new performance result is claimed; timing requires fresh GCP clearance
after both active release tags.

The separate push-range workflow repair is published in
[PR1682](https://github.com/lukacf/meerkat/pull/1682) at
`032547f2ab26462842fb1d113bd73eb5d510d0b1`. Its complete normal push passed
in 126m22s, ending at 20:59:08 UTC, including the deterministic unit,
integration and E2E gate. Individual counts are not retained in successful
hook output and are not inferred. Fresh
[CI37234271914](https://github.com/lukacf/meerkat/actions/runs/37234271914)
is running.
It follows `5ecab297`, whose 70m13s push passed Clippy and machine checks,
then failed one of 12,155 unit tests (17 skipped). Two new controls reproduced
the cleanup observer's assumption that FIFO closure implies process exit.
Both passed after a bounded exit-observation repair; the two original real
process timeout/interruption controls also passed in 5.015s. The repair
changes test observation, not production cleanup. Normal commit hooks passed.
Two earlier 600s and 840s attempts expired during compilation before tests
ran; the completed baseline then took 8m47s to compile and the repair 1.91s.
Raw results are retained under
(operator-retained path).
That first-branch push conservatively validated the whole tree because
its source ref was `HEAD`; future first pushes will name the source branch
explicitly so the existing dispatcher can select its proven comparison base.

The representative benchmark now defaults to the approved fixed W20/N32
profile and rejects the withdrawn tail profile or conflicting count overrides.
Both profile tests first failed against the prior helper, then passed after the
repair; all three existing deadline controls also passed. Initial compilation
took 3m19s, the repair recompile took 2.63s, and test bodies took 0.03s total.
The six measurement cells, workload oracles, output format, 1,200-second budget
and separate small-matrix profile remain unchanged. Raw `red.log`, `green.log`
and `deadline.log` are retained in
(operator-retained path). This qualifies profile
selection, not performance. Strict scoped Clippy passed in 3m21s and the
repository analyzer self-test passed all 20 cases in 1.419s. The correction is
committed at `ce8d57524897fe656df8c9944ea13d4913c82f8f`, tree
`535415075d176351885843d763628804363ffbf7`, with normal commit hooks passed.
Its publication is pending.

## Publication gate, 2026-10-04

Native [PR1634](https://github.com/lukacf/meerkat/pull/1634) is published at
`00b0cab3e3480efac149c91f0823fa687b862639`, tree
`858a61c033b266086a0cf313d27b588b23247e6a`. All installed normal push hooks
passed without bypass in 118m17s, ending at 14:58:02 UTC
(`public00b0-normal-push.log`). Exact-head
[CI run 37211222497](https://github.com/lukacf/meerkat/actions/runs/37211222497)
passed 38 jobs with four intentional skips in 15m56s, ending at 15:14:04 UTC.
[Semver readiness](https://github.com/lukacf/meerkat/actions/runs/37211222385)
also passed. The governed JSONL job ran all three required cases with zero
failures or ignores in 0.21s test bodies. PR readback confirmed the exact head
and clean merge state. Raw evidence is `public00b0-ci-37211222497-final.json`,
`public00b0-governed-jsonl-ci.log` and
`public00b0-pr1634-ci-green-readback.json`. No merge or release is recorded.

Published predecessor `7defcdd7fa069782b12c9073bd3b128a08147d6d` passed all
normal hooks in 79m48s and
[CI run 37196269479](https://github.com/lukacf/meerkat/actions/runs/37196269479)
in 17m27s with 38 successful jobs and four intentional skips. Its retained
logs remain historical evidence for that exact source.

Published predecessor `c18c619bf2d140663eb0a78d17d7122d9ac1a5ef`, tree
`f81231a73c2f4ba5befa1cf2575fe1834016c77f`. Its normal GCP push passed all
required hooks in 4,078s wall time and completed at 06:17:43 UTC
(`native-c18-gcp-normal-push-green-r1.log`). Fresh
[CI run 37182383940](https://github.com/lukacf/meerkat/actions/runs/37182383940)
completed successfully on exact c18: 38 successful jobs and 4 intentional skips,
with its final gate at 06:39:26 UTC, 21m42s from creation. The exact-head
[semver run 37182383935](https://github.com/lukacf/meerkat/actions/runs/37182383935)
also passed. Independent PR readback confirmed exact c18 and clean merge state.
The final API record is `native-c18-ci-37182383940-final.json`, SHA256
`8577ababd8d3d8d40a7b2aeec66960f672c642d27c5a635c2487115822eb20a8`.
No merge or release is recorded.

The preceding `51cd489916387039121e751e595670b92a0d4b65` publication passed all
required hooks in 3,833s (`push-51.log`). Its
[CI run 37177141203](https://github.com/lukacf/meerkat/actions/runs/37177141203)
also passed with 38 successful jobs and 4 intentional skips. These remain the
source-qualified predecessor results, rather than the current head's evidence.

The previous `157a73e75aed677c5f5a37d20cd07fe53710c216` publication passed normal
hooks in 4,726s (`native-157-gcp-normal-push-green-r1.log`). Its
[CI run 37170072061](https://github.com/lukacf/meerkat/actions/runs/37170072061)
completed with 35 successful jobs. Format + governance failed because two
standalone example locks were stale; Generation ratchets failed only poster
freshness in two HTML outputs, after canonical coverage and typed content
checks passed. The dependent CI gate failed too. Those results remain historical
for the previous head; they do not describe CI-green predecessor c18.

The preceding repair is `51cd489916387039121e751e595670b92a0d4b65`, tree
`f365afd3b3bc18f14c70da35db3615ce97e5fc7c`. Its only four changed files are the
two example locks and two poster outputs. Existing dependency identities/pins
are preserved; existing canonical metadata refresh and lock checks passed.
The unchanged Node poster generator produced the reviewed bytes and passed
byte idempotence. The GCP publication owner then passed the existing
example/root-lock checks and full compiled-alphabet poster gate before the
normal successor publication. Published `51` has identical Rust bytes to `157`.
Additional runtime production integration remains held.

Console [PR520](https://github.com/lukacf/meerkat-mobkit/pull/520) is published
at `db369979a28bef472ddae729b9f5003e0bec4aeb`, tree
`6c76fbd96eeaf92cb40ac93e4d776fd782b062dc`. Normal hooks passed and
[CI run 37206031337](https://github.com/lukacf/meerkat-mobkit/actions/runs/37206031337)
passed all 11 jobs in 40m27s, ending at 14:13:42 UTC. The complete browser suite
passed. The repaired canonical-send scenario waits for its exact RPC response
before checking the refreshed access state; its identity, member and pending
state assertions remain. PR readback confirmed the exact head and clean merge
state. Raw evidence is `consoledb369-ci-37206031337-final.json`,
`consoledb369-ci-browser-job.log` and
`consoledb369-pr520-ci-green-readback.json`.

The preceding c973 source passed Python20/TypeScript44 event tests and SDK build,
but its CI browser scenario failed; that failure remains retained. Earlier e26
passed all 11 CI jobs. These packet and browser results qualify projection and
access UX, not integrated native-plus-Console enforcement. MobKit still needs
the explicit native dependency and authenticated ingress integration.

Docs-only predecessor `37ca84df0fb3b6c956501374183b3e0866880db6` passed its normal
push and all 11 jobs in [CI run 37188101522](https://github.com/lukacf/meerkat-mobkit/actions/runs/37188101522)
(32m20s elapsed). Only the Console guide and changelog differ from `17`.
The preceding
`17fe7af8b2b374402d0e198f44879c2891446ac8` is the ordinary merge of published
`fc0509cfe` and main `fa233b756`, whose five-file delta added Python init-timeout
SDK/tests/docs and retained both changelog entries.
The four incoming Python stand-in gateway tests passed (2.57s bodies).
The earlier tree-qualified Console/component/build and four mock browser
scenarios remain valid; these establish projection/access UX, not integrated
native-plus-Console governance. The fc0509 normal push passed all 2,183
workspace library tests (2 existing skips, 396.504s bodies). The 17 push passed
applicable hygiene hooks; Rust/unit hooks correctly selected no files for its
five-file Python/docs/changelog delta, rather than repeating that test result.
Fresh [CI run 37172144001](https://github.com/lukacf/meerkat-mobkit/actions/runs/37172144001)
passed all 11 jobs at exact 17, including the final gate at 03:24:41 UTC
(36m53s elapsed from run creation). This remains the predecessor's CI result,
separate from current e26's completed 11-job result.

The published SDK adapter exports known hook reasons plus an explicit unknown
wrapper retaining the exact original reason/code. Known TypeScript variants
keep discriminant narrowing, malformed known reasons do not fall back to legacy
strings, and explicit-null HookDenied payloads survive. The existing TypeScript
build/public typecheck and 500 tests, Python type/parser file (482 tests) and Web
SDK typecheck/96 tests passed at their recorded checkpoints. These are contract
compatibility results. The existing CI planner's 15.1-minute runtime unit model
preserves the 16-minute budget; it is not measured authorization overhead or a
hosted-job completion guarantee.

The exact published `157` optimized cost binary is prepared:
`native_cost-8340d2a996464c6b`,
SHA256 `03d524567eea7aff409d228dc9e1916912536b7a97b3d8dd777cccf562a20dd3`.
The optimized build passed in 472s (`native-157-optimized-build-correctness-r1.log`).
`native_cost_correctness` passed 1 test in 0.04s bodies
(`native-157-cost-correctness-r1.log`); `representative::native_representative_correctness`
passed 1 test in 11.44s bodies (`native-157-representative-correctness-r1.log`).
Those correctness results are separate from build duration and timing.

The first actual small timing run completed at 03:22:42 UTC: the existing test
exited 0 with 1 case in 46.91s bodies (`native-small157-r1.log`). Its four cells
contain 16,000 valid samples, with 100 warmup pairs and 2,000 measured matched
pairs per cell. Independent review confirmed sample/order cardinality, effect
and audit counts, and the arithmetic (`native-small157-r1-independent-review.log`).
The existing analyzer's summary is `native-small157-r1-summary.json`.
The uninstrumented `turn_only` results are:

| Grant lineage depth | Trusted p50 turn (ms) | Local p50 turn (ms) | Signed paired added-turn p99 (ms) | Turn overhead, ratio of means |
| --- | --- | --- | --- | --- |
| 1 | 0.919699 | 2.005658 | +1.228443 | +117.33 percent |
| 3 | 0.797036 | 2.285658 | +1.643655 | +189.37 percent |

Added-turn quantiles use signed Local-minus-Trusted matched samples and empirical
nearest rank; overhead is `100 * (sum(Local) / sum(Trusted) - 1)`. These turns
exceed 10 percent in this small fixture and require investigation; the
representative-turn gate remains unproven. The added whole-turn p99 is not the under-1-ms per-operation
metric. The instrumented boundary cells are diagnostics, with no individual
full-tool timing samples. One small quiet run does not establish the full
representative/platform budget result or full acceptance.

Root owned the explicitly allocated 45-minute small quiet window. The GCP
resource owner confirmed no build-monitor violation from START through DONE
and released the lease at 03:24:56 UTC. Post-run observations showed 100 percent
CPU idle, zero iowait and no swap, with writeback activity in the final sample;
this is not an all-IO-zero claim. That 45-minute allocation is historical.
Luka rejected the former 6-7-hour representative allocation: every future
benchmark attempt must finish in less than 20 minutes total, including setup,
warmup, measured work, all oracles, cleanup and output. Full surface and
performance acceptance remain open, including under 1 ms p99 added authorization
per operation and at most 10 percent representative turn overhead.

The first attribution-only profile completed at 04:41:19 UTC within the explicitly
reserved five-minute quiet window. The existing optimized small fixture passed
1 case in 48.41s; perf captured 5,109 CPU-clock samples with zero lost samples
(`native-small157-profile-r1.log`, `perf157-native-small-r1.data`). The resource
owner confirmed no build-monitor violation through DONE and released the host.
Allocation, copying and formatting appear in the self samples, but application
call stacks are incomplete and both execution modes plus setup were profiled.
This cannot assign the governed delta to an owner or qualify the performance
budget. Profile-perturbed timings do not replace the uninstrumented results.

The second attribution-only capture completed at 06:31:32 UTC using the same
verified `157` binary; only the DWARF user-stack capture changed from 8,192 to
32,768 bytes. The existing small test passed 1 case in 51.24s; perf recorded
5,447 samples and zero reported loss (`native-small157-profile-r2.log`). The
resource owner's external monitor confirmed no build violation from START at
06:26:50 through DONE, and the lease was released at 06:34:11. The larger stack
did not establish useful application-owner attribution improvement: major
allocation caller trees still stop in libc. Both modes, setup and capture costs
remain mixed, so this is not an incremental governed-cost or optimization result.
The separate exact-byte hex production candidate is committed at
`f4671b7c5f190d0a165cffe87e43f86845a6c718`, with only the reviewed 4-line/3-line
encoding change. Seven input-authority tests, the canonical encoding control
and both native cost correctness selectors passed. These qualify encoding and
correctness; the later comparison below is separate from published c18.
The fixed-mean harness is committed locally at
`761ce0d1b25454daf49bfbafb59d85d5f3c66f8b`: four deadline/profile controls
failed behaviorally before repair (9m21s compile, 0.28s bodies) and then passed
(6.51s compile, 0.05s bodies). Its full ordinary target passed six cases with
two intentional ignored timing matrices (0.45s compile, 216.25s debug bodies;
`native-fixed-mean-harness-all-ordinary-r1.log`). The repository-owned analyzer's
fixed-profile extension passed 20 pure-Python tests. These qualify the harness
and analysis contract; neither ignored matrix ran as part of those checks.

The prerequisite small comparison of preserved `157` and `f467` completed in
91s, including both runs, output and repository analysis. Each raw retained
16,000 samples, four cells, 100 warmups and 2,000 matched pairs per cell;
independent review reproduced the summaries and effect/audit counts. Signed
added means fell 18.50/14.59 percent at lineage depths 1/3, meeting the declared
GO screen for representative investigation only. Trusted controls rose
9.09/17.68 percent, while Local means fell only about 6.3/3.8 percent;
candidate whole-turn overhead remained 94.46/144.09 percent. This establishes
neither causal optimization gain nor cheap-default acceptance
(`native157-small-comparison-r1.log`, `nativef467-small-comparison-r1.log`).
The preceding quiet queue consumed 53m40s; waiting, the 79m48s publication push
and the 91s measurement are separate costs.

Exact clean `761` optimized qualification then passed: build 6m24s, small
correctness 1/1 in 0.03s, representative correctness 1/1 in 11.04s, four profile
controls in 0.04s and analyzer 20/20 in 1.709s. The initial wrong control filter
selected zero tests and is retained as setup failure, not qualification.
The fixed W20/N32 representative attempt ran on the exact optimized `761`
fixture from START 12:23:07 to DONE 12:29:02 UTC, 355s elapsed, below the
1,200s hard limit. The existing test passed 1 case in 354.84s bodies
(`native761-fixed-mean32-r1.log`). Raw schema2 records complete fixed_mean_32,
20 warmup pairs, 32 measured pairs per cell, zero failures/timeouts and 384
measured samples across all six depth/workload cells
(`raw761-native-fixed-mean32-r1.json`, SHA256
`8483a11f3283adc11f84464bc437c695dc83964ce90699da0ed79aa06e9ddafe`).
The repository analyzer's summary is `native761-fixed-mean32-r1-summary.json`,
SHA256 `5071c7e5b5dca4d75cfc3ca6a0dcfe8dbdad6851b4efe79be040eb253b9391df`.

| Unit | Grant lineage depth | Trusted mean (ms) | Local mean (ms) | Signed added mean (ms) | Ratio-of-means overhead | Conditional result |
| --- | --- | --- | --- | --- | --- | --- |
| Fresh-admission whole turn | 1 | 13.221447 | 33.108947 | +19.887500 | +150.42 percent | MISS |
| Fresh-admission whole turn | 3 | 15.840237 | 41.495532 | +25.655295 | +161.96 percent | MISS |
| Continuing segment | 1 | 8.052576 | 18.678672 | +10.626096 | +131.96 percent | MISS |
| Continuing segment | 3 | 8.712939 | 22.311535 | +13.598597 | +156.07 percent | MISS |
| Sum of four fenced calls | 1 | 0.068328 | 0.432402 | +0.364074 | +532.83 percent | MISS |
| Sum of four fenced calls | 3 | 0.088986 | 0.979219 | +0.890233 | +1000.42 percent | MISS |

Each cell uses 16 adjacent opposite-order blocks. The six paired Fieller
mean-ratio intervals use family alpha .05, conditional on stationary, independent,
approximately bivariate-normal blocks. All six intervals are above the 1.10
threshold; fresh-admission ratio intervals are [2.494751, 2.513664] and
[2.570860, 2.669374] at depths 1 and 3. This is a conditional mean-study cheap-turn
failure, not a passing performance result. Full performance acceptance remains
UNPROVEN. Only fresh admission measures whole turns; continuing segments and
summed correlated direct calls are diagnostic units, not individual-operation
p99 measurements. No small-N tail or full-platform acceptance follows.

The final external observer retained 72 complete overlapping five-second
frames covering 12:23:03-12:29:06 UTC around the 12:23:07-12:29:02 measurement.
No unexpected heavy process was observed. Known PID3381992 was zombie ZN;
gate PID3381990 was suspended TN at 0.0 percent CPU. CPU idle ranged from
99.34 to 99.77 percent; maximum iowait/steal were zero, minimum available memory
was 729,390 MiB and swap was zero (`native761-fixed-mean32-r1-monitor.log`, SHA256
`2f1e3e7e132efd43541f04168bba8e7832d11fd993935cd6e36f31d560f8c1f9`).
These samples establish observed resource quiet under the prior reservation.
A fresh HOLD acknowledgment from the typing-latency owner was absent; no new all-owner clearance
is claimed. Root owns measurement and the GCP resource owner owns host quiet
qualification. The less-than-20-minute total attempt bound includes setup,
warmup, measured work, oracles, cleanup and output. Functional PASS, sampled
quiet and the conditional MISS do not establish causal optimization gain or
full performance acceptance.

The next optimized candidate `a08b46c22eb178ed41a321f2831ef0424bfb23a8`, tree
`e04e777ed0ce3546e41612e0ec7a96119fc64445`, removes the intermediate replay JSON
buffer and a redundant full-state preview copy. It preserves all owner checks
and the measured fixture. Its release build took 385s, and both existing cost
correctness selectors passed (0.04s and 10.48s). The emitted binary SHA256 is
`5da51a8c5a24ca7902bd8530e354be38e5518671a8b0859a1f52287b05ee8ac7`;
profile and feature selection match optimized761.

The unchanged W20/N32 study ran from 15:15:01 to 15:20:43 UTC: 342s total,
341.59s test bodies, 384 complete measured samples, no failures or timeouts.
All six conditional mean comparisons again missed the 10 percent threshold.

| Unit | Lineage depth | Trusted mean (ms) | Local mean (ms) | Added mean (ms) | Overhead | Result |
| --- | --- | --- | --- | --- | --- | --- |
| Fresh whole turn | 1 | 12.887888 | 32.329614 | 19.441726 | 150.85 percent | MISS |
| Fresh whole turn | 3 | 15.747978 | 40.822693 | 25.074715 | 159.22 percent | MISS |
| Continuing segment | 1 | 8.037428 | 18.776226 | 10.738798 | 133.61 percent | MISS |
| Continuing segment | 3 | 8.918356 | 22.747428 | 13.829072 | 155.06 percent | MISS |
| Sum of four direct calls | 1 | 0.076086 | 0.441294 | 0.365208 | 480.00 percent | MISS |
| Sum of four direct calls | 3 | 0.083527 | 0.966538 | 0.883011 | 1057.16 percent | MISS |

Fresh added means were only 2.24/2.26 percent lower than the separate761 run;
continuing added means increased 1.06/1.69 percent. These descriptive changes
establish neither causal nor material improvement. The unchanged conditional
mean model and six-comparison limits still apply; direct sums are diagnostics,
not per-operation p99 acceptance. Raw `rawa08-native-fixed-mean32-r1.json` has
SHA256 `f8dd00ed9abbbe188483291fd2342a32d31591383a268dfddecb2b449cb0f18e`;
`nativea08-fixed-mean32-r1-summary.json` has SHA256
`069bd2e3bc7bd13445f6125d2b7cbd3ef9809e1a560d54a9452e391b255c4c15`.
Local reanalysis reproduced the summary bytes. The external five-second
observer retained 68 complete frames overlapping the measurement, covering
15:15:00-15:20:43 UTC. They showed no competing compiler, 99.33-99.43 percent
CPU idle and zero iowait/steal/swap.
Its raw log is `quiet-a08-fixed-mean32-r1-monitor.log`, SHA256
`7957dbadc462f3d2881182ca169eb9a42618601226e82b48bd48ba3f5287fda7`.
The autonomous host and the adapter consumer supplied current nonconflict replies, while fresh
acknowledgment from the lead and the typing-latency owner was absent. No full-host acknowledgment is
claimed. Performance remains an unmet architecture gate, and further runtime
expansion remains held behind it.

The next candidate `f9f0cd55ef06156ebda74097cac3591ca538f2f4`, tree
`4ef17e5630398603de7accb6ccef7f9c9e1780d1`, shares the private immutable input
association across clones. Its new sharing/encoding test first failed on the
old representation; the repair passed all 58 contracts tests and strict
all-target Clippy. The optimized build passed in 453s, followed by both native
cost correctness selectors (0.04s and 8.74s). The executable SHA256 is
`18293a9065f91338a55bd93df24f176689bd120600b3c1220e2f772a65385664`.
The autonomous host and the adapter consumer source reviews found no blocker. The measured fixture, analyzer,
features, build profile and canonical encoding are unchanged from a08.

Its W20/N32 study ran from 15:43:16 to 15:47:59 UTC: 283s total, 283.10s
reported test bodies, all 384 measured samples complete and no failures or
timeouts. All six conditional mean comparisons still missed the threshold.

| Unit | Lineage depth | Trusted mean (ms) | Local mean (ms) | Added mean (ms) | Overhead | Result |
| --- | --- | --- | --- | --- | --- | --- |
| Fresh whole turn | 1 | 12.830315 | 30.408860 | 17.578546 | 137.01 percent | MISS |
| Fresh whole turn | 3 | 15.042932 | 36.884818 | 21.841885 | 145.20 percent | MISS |
| Continuing segment | 1 | 8.041696 | 17.584395 | 9.542699 | 118.67 percent | MISS |
| Continuing segment | 3 | 8.719318 | 21.029911 | 12.310593 | 141.19 percent | MISS |
| Sum of four direct calls | 1 | 0.073372 | 0.432092 | 0.358720 | 488.90 percent | MISS |
| Sum of four direct calls | 3 | 0.076322 | 0.922383 | 0.846061 | 1108.54 percent | MISS |

Fresh added means were 9.58/12.89 percent lower than the separate a08 run;
continuing added means were 11.14/10.98 percent lower. These are descriptive
comparisons between separate runs, not a causal estimate or performance
acceptance. The same conditional mean assumptions and scope limits apply.
Raw `rawf9f0-native-fixed-mean32-r1.json` has SHA256
`952f2c5ce7de889757f2d2f66daab6a12d753581799e3c7c98ba4bc66f3dff30`;
`nativef9f0-fixed-mean32-r1-summary.json` has SHA256
`162bfbb49c0fe40f9c4b35d23c16e74cf0dfb0aad072ff070f26b266ed384bc2`.
Local reanalysis reproduced the summary bytes. The external observer retained
57 complete five-second frames covering 15:43:16-15:48:03 UTC, with
99.34-99.79 percent CPU idle, zero iowait/steal/swap and no unexpected matching
heavy process. Minimum available memory was 729,546 MiB. The retained Cargo
process was a zero-CPU zombie and its parent remained suspended. Raw
`quiet-f9f0-fixed-mean32-r1-monitor.log` has SHA256
`4399342d870895bdb808459cf7aceb4ccd0b0c8461e2d89d9c116b0293304aa0`.
Fresh acknowledgment from the lead and the typing-latency owner remained absent; this is sampled resource
qualification, not all-owner clearance. Source and measurement remain separate
from published native00b0 CI; the combined successor must pass its own normal gates.

The fixture-only `685e336d3b0984f72dab5517e6617bd366ce2b7a` checkpoint passed
input-authority 9 and controller-custody 5 tests under ordinary parallelism,
with no failures or ignores; post-format passes and applicable normal hooks
also passed. It isolates the actual retained credential selection in two test
files. The earlier four Busy failures remain separate from these 14 passes;
this is not production, a full-runtime suite or persistent administration.

The backend-only scaffold's initial 14-test batch reported 2 passed and 12
failed. The runtime E0502 counter-borrow compile failure ran no test bodies;
its narrow repair preceded the actual 8-test runtime RED
(`persistent-backend-runtime-red-r1.log`, `persistent-backend-runtime-red-r2.log`;
2m16s compile, 0.16s bodies). Strict-fence/profile/schema selectors each ran two
cases; the no-conversion profile case failed at fence setup before checking WAL.
The two added catalog controls then reported 1 pass and 1 failure after 1.27s
compilation, 0.03s bodies: foreign/retired compatibility passed, while the old
normalizer accepted oversized owned SQL. Those 16 controls reported 3 passes
and 13 failures. The two canonical grant-lock probes then both failed after
2m28s compilation and 5.01s bodies
(`persistent-controller-grant-locks-red-r1.log`): mutation waited while the actual
publication or grant-owner lock remained held instead of returning Unavailable.
The initial backend and nonwaiting-mutation selections executed 18 critical
tests: 3 passed, 15 failed and none ignored. That is the historical RED result;
the earlier counter-borrow compile failure remains separate from executed bodies.

The local nonwaiting grant repair is committed at
`e108e2053768554bdc8a4948a8429f0e1650ca2b`, tree
`5b934671599491734102a9cc22296e8f49455fa1`. It changes only the two canonical
publication/grant owners and the existing grant tests. Native-custody revocation
tries the actual publication and generated-owner locks; Busy returns Unavailable,
while the generated Revoke decision and publication/result semantics remain.
The first repaired authorization library run passed all 97 tests after 18.72s
compilation, with 0.06s bodies. Strict library Clippy passed in 1m24s before the
normal commit's whitespace-only Rust formatting. The first normal commit attempt
stopped after that formatter changed a file; the second passed. Post-format
library execution again passed all 97 tests, with no failures or ignores, after
21.61s compilation and 0.06s bodies. Both formerly failing grant-lock probes and
the publication/grant invariant companions passed
(`persistent-controller-nonwaiting-grants-green-r1.log`,
`persistent-controller-nonwaiting-clippy-r1.log`,
`persistent-controller-nonwaiting-normal-commit-r1.log`,
`persistent-controller-nonwaiting-normal-commit-r2.log`,
`persistent-controller-nonwaiting-grants-green-postfmt-r1.log`).

The public repair commit `ec7196736c8c43677724c1c2eb6fb135c121623b` is included
in published, CI-green `00b0` above, along with the status and public-guide
updates. The two grant-lock REDs are resolved and published;
the separate 16 backend controls remain at 3 passes and 13 failures. No backend
production repair or native/grant assembly has been accepted, and persistent
public Try remains unsupported. This narrow lock repair does not qualify full
persistent controller administration or performance.

Historical failure/preparation logs remain in the existing evidence directory.
The first Web SDK exhaustive-event check omitted HookLaunchRefused, while its
11m43s WASM compile/17m00s complete build did not qualify overhead. The no-SQLite
WASM push of 0576 failed after 43.5 minutes; the 9a7 Linux push failed after 34.4
minutes on unsupported-target fields/imports and the missing E2E confinement
field. Those narrow cfg/fixture repairs preceded published `157`; earlier failed
pushes did not publish. Console's GH007 metadata rejection and Python 3.14
process-fixture failure remain separate from its subsequent successful normal
publication with installed Python 3.12. Earlier d1/fca optimized preparation and
the 22:30-23:00 UTC NOT_MEASURED window remain historical evidence; they are not
substitutes for the exact `157` binary or current timing.


### Published tests-only restart checkpoint

Published c18 is based on `51` with fixture/catalog changes, the canonical BUILD
source-list refresh and checkpoint documentation. It promotes the six existing
deterministic default-feature acceptance cases to ordinary tests and adds one
real separate-process SQLite case. Published `157` retains its historical six
ignore annotations; c18's results are separate and do not establish an
automatically governed surface profile.

The first cold run failed at fixture discovery: raw SqliteSessionStore listing
returned zero sessions before the reader started (0.62s compile, 0.66s body;
`restart-acceptance51-cold-process-r1.log`). The fixture now discovers the real
session through the existing public RuntimeStore catalog and typed session ID;
it still validates the actual committed WholeBlob, input and protected audit.
This was a fixture owner-selection error, not an established recovery defect.
The repaired exact cold selector passed 1 test (16.84s recompile, 1.85s body;
`restart-acceptance51-cold-process-r2.log`). The directly executed default
native_governed_loop target then passed all 16 tests, with 0 ignored and 0 filtered out (0.60s compile,
4.85s bodies; `restart-acceptance51-ordinary-r2.log`). Unchanged native_cost
debug correctness also passed both ordinary cases (6.61s compile, 159.51s bodies,
2 timing cases ignored; `restart-acceptance51-cost-correctness-r1.log`), making
18 ordinary cases across the two authorization integration targets.

The writer and reader are actual separate processes using the real SQLite and
File token owners. The reader starts with a cold generated credential registry,
restores the writer's marked credential through the existing lifecycle owner,
installs newly issued current host grants and obtains the reconstructed actor's
current controller pin. Reconstruction performs no model/tool work. A fresh
input/run then receives exact local delete refusal, one permitted callback read,
two loopback model requests and same-run completion. Reader and parent verify
the original protected row/audit and exact transcript prefix remain unchanged.
The strengthened negative control explicitly lets current authentication accept
an attempt carrying the writer's historical association and the reader's actual
fresh controller pin. Current grant authority must then return typed Denied,
with zero HTTP/tool entry and no accepted native or durable input row. Independent
document loads verify transcript preservation; stored digest agreement with
document authority is enforced by the existing store loader.
The revised full target passed all 16 tests with no ignored or filtered cases
(14.87s compile, 2.05s bodies;
`restart-acceptance51-ordinary-antifallback-r1.log`).

The three existing E1 smoke-wrapper controls passed (7.06s warm compile, 0.00s
bodies; `restart-acceptance51-wrapper-controls-r2.log`) after a missing test-only
OutputPolicy import was fixed. The initial broader dependency build consumed
about 39 minutes before that compile error. These controls are not the execution
of the outer smoke wrapper. Documentation checks passed after the status edits.
The Make agent gate stopped before Clippy on two existing lane-doctor failures:
archive-mode nextest workflow scanning and the Rust selector module parser
selftest. Its shell also cannot source the Bash backend helper under /bin/sh.
The documented Cargo changed-path gate passed all selected governance and
Clippy checks (`restart-acceptance51-cargo-agent-gate-r1.log`); Clippy finished
its dev-profile check in 23m49s. This does not make the failed Make agent gate green.

The first Make e2e-smoke attempt was deliberately stopped during redundant
dev-profile materialization (exit 143;
`restart-acceptance51-outer-e1-prebuilt-r1.log`). The existing prebuilt outer E1
wrapper then passed 1 selected case (15.88s leaf compile, 0.72s outer body),
running the already-built native case with 1 inner pass in 0.63s
(`restart-acceptance51-outer-e1-prebuilt-r2.log`). Its existing
`restart-e1-artifact-manifest.json` records the artifact selection. These are
outer/inner wrapper results for E1, not a new execution of all 16 native cases.

This qualifies completed-turn reconstruction followed by new work in the
explicit embedding fixture. The HTTP Bearer header is synthetic and independent
of restored secret bytes; the read is a callback rather than filesystem/source
access. It does not qualify interrupted replay, provider secret refresh/rotation,
persistent controller/grant administration, automatic constructors, full surface
coverage or performance. The changed-path governance/Clippy and selected outer
wrapper results are qualified separately above. Normal c18 publication and
exact-head CI are green; these recorded unit/integration and CI inventories do
not supply an independently verified native_governed_loop exact-selector count.
The earlier failed full Make invocation remains historical. No runtime production
change is included in this restart delta.
The earlier separate public-entry/notification candidate was committed at
`c9361659b3e09a147eba0e3b863550b2d9952521`, tree
`8053319b92ba75913e3c761d615965c8a392ce57`. The first execution on `643` passed
two cases and failed the new public case: its fixture read the durable accepted
row while in-flight audit was still held by the actual native owner. The normal
fixture-only repair reads that audit through the existing runtime adapter and
retains final durable audit assertions. The same three no-default/local-authorization
RPC cases then passed, with no ignores, 553 filtered cases and 0.03s bodies after
a 9m18s optimized rebuild (`publicc936-governed-jsonl-r2.log`). The new case
executes exported JSONL setup/serve, two actual model requests, zero denied
delete callbacks, one permitted read callback, same-run feedback/completion,
protected native audit and ordered public event notifications. The negative
unsupported/ungoverned entry and existing refusal case passed too. Normal commit
hooks passed. Its source-reviewed local successor was
`f687e95d6c2611491e102a192429e7c8558b2c6a`, tree
`da4bce4bddba21698fbce883aae9cb0d556e3ba1`. Its same three cases passed with
0 failed, 0 ignored and 553 filtered in 0.04s bodies after a 9m06s release build
(`publicf687-governed-jsonl-r1.log`, SHA256
`4298fa4acb389ccf1f7631370ae3885e2996aeaae4392dab479a8108e04cc1fd`).
This includes the source-reviewed cross-operation model-tool-model audit ordering,
without imposing serial order on siblings. Published `7def` retains those
exact test bytes and adds no production behavior. Its fresh CI Governed JSONL
job 111418874872 listed and ran all three exact names: 3 passed, 0 failed,
0 ignored and 553 filtered, with 0.21s bodies
(`public7def-governed-jsonl-ci-job.log`). These exact-head results are separate
from c18's two-selector evidence and do not establish integrated
native-plus-Console acceptance.

## Current local shell and hook checkpoint, 2026-10-03

The native shell continuation is committed locally through
`6cfa8cbbbe428ef3d51b4b8bdf8098c3725ec632`, tree
`c23875f3f0479c384cc8c79dced0695185e9957d`, on
(operator-retained branch). The stock factory Required-shell E1 has
executed authenticated ingress, actual OS refusal after command entry,
permitted sibling execution, ordered feedback, a second loopback model request
and completion of the same run. Typed mechanical refusal causes remain distinct
from permission decisions and ordinary IO across shell and member upcall paths.
The core pre-tool launch-refusal slice has behavioral RED/GREEN for all five
confinement causes and ordinary pre-entry IO. Its normal commit hooks, broader
Clippy, schema/SDK freshness and 18 wire regressions passed. The broader Clippy
command spent 43m01s compiling/checking; the wire test body took 0.01s after
2m53s compilation. Raw results remain in the existing evidence directory below.
These results describe the local shell/hook checkpoints before publication.
The publication section above records CI-green `51` and the measured small
fixture. Native command-hook integration, full surface coverage and the
representative/per-operation performance gates remain open.

The explicit pre-tool policy-denial slice is committed locally at
`9a3a80964b486e86416d1cab99448d5e90184150`, tree
`fde1829da8c2975154f43c8def3cf73645223b61`. Its behavioral
RED aborted the run with HookDenied; after repair, five selected pre-tool tests
and thirteen hook contract tests passed. Eleven member-upcall controls then
passed in 0.01s after 15m54s compile. Adversarial review found existing decision
and event decoders collapsed a present JSON null payload into absence. Both
transport regressions reproduced the defect; their repair passed all fifteen
hook contracts. Schema/SDK generation, SDK freshness, event inventory, docs and
Bazel generation checks passed. Final core/Mob all-targets, all-features Clippy
passed in 26m36s. All twelve existing hook behavior controls passed (37.00s
compile, 0.16s body). Final schema freshness and normal commit hooks passed.
Native command-hook confinement, protected hook
refusal audit and other hook-point locality remain incomplete.

The review channel monitor is active. Publication clearance and a quiet performance window
are specific scheduling dependencies, not a halt on independent work. Root owns
the serial Rust lane while agents prepare and review child-process cleanup,
physical SQLite custody, documentation and existing E2E registration. SQLite
ordinary admission reproduced `UnsupportedScope` before entry (4m09s compile,
0.04s body). Five actual backend tests reproduced the missing custody owner
(6m41s compile, 0.10s body). The reviewed physical owner then passed those five
controls (3m09s compile, 0.14s body) and nine memory/fault/open/path controls
(0.58s compile, 0.10s body). The actual governed SQLite model-tool-model flow
passed (2m12s compile, 0.66s body); its memory-backed control also passed
(0.87s compile, 0.09s body). Seven backend controls, including independent
process exclusion/release and a proven same-file case alias, passed
(48.13s compile, 0.14s body). The existing hard-link rejection control passed
(0.50s compile, 0.04s body). The same-process close/reopen test reached teardown
and failed because the default service-machine composition retained the old
owners. A diagnostic run confirmed one machine, two session-store and three
runtime-store references after explicit teardown. Fresh release inspection
found upstream `27f8de8a6` already fixed that cycle. This candidate imports its
Weak host and RPC callers, retaining current fallible custody boundaries.
The separate returned-guard lifetime test reproduced premature service release
(21.46s compile, 0.04s body); a small tuple retaining the service passed all
eleven reconfigure controls (25.65s compile, 0.03s body).
The actual close/reopen scenario then passed (10.69s compile, 0.56s body), as
did all three stock memory/SQLite/reopen controls (0.48s compile, 0.60s body).
Targeted runtime all-target Clippy passed in 4m59s. Host/RPC library Clippy
passed in 13m15s. Normal commit hooks passed at
`2dd9d80d53f8be2d94bb5b553c903377f263ee0f`, tree
`8c266d2d5355917f5c7996b12c3ea6f5566a129b`.
Separate process restart,
persistent controller administration and additional-platform acceptance remain
open. The source-reviewed canonical service-slot adaptation is a separate next
slice, not included in this checkpoint.

The fresh-work tests-only slice executes authenticated work after physical
close/reopen. Its first run failed at credential setup (5.23s compile, 0.34s
body): a newly created empty token vault requested release of the still-live
credential registry. This was a fixture ownership error, not a native recovery
defect. The corrected fixture retains the original actual host token vault and
verifies its marked credential through the existing status owner without
republishing or changing lifecycle state. The fresh model-tool-model run passed
(3.82s compile, 0.62s body), with distinct input/run/tool IDs, exact protected
audit, local denial feedback and a permitted sibling. All four stock controls
passed; the final existing native-loop target passed nine tests with six existing
acceptance cases explicitly ignored (0.23s compile, 0.88s body). Targeted test
Clippy passed in 5m31s. The slice is committed locally at
`249b13147f78af6c615b3bf0347e71f962ab33a3`, tree
`26972407da9ae3e702084dd36cbd344b2cb1a4de`; normal commit hooks passed.
On that clean commit, all six acceptance cases were then explicitly executed
and passed (0.90s compile, 0.32s bodies). Raw results are retained in
`native7899-current-ignored-scenarios-r1.log` under the existing evidence
directory below. Publication remains pending. This is same-process
current-owner work, not process restart or restored controller authority.

[Meerkat issue 1618](https://github.com/lukacf/meerkat/issues/1618) is part of
full default-profile coverage: effective per-identity policy must survive fresh,
restored, child and scheduled/delegated paths, refuse peer/reply/cross-mob sends
before delivery, and retain permitted private memory, schedule and scoped
callbacks. Initial source inspection confirms MobKit DurableAgentSpec,
AgentBuildDraft and build_spawn_spec lack this binding. Native SessionBuildOptions
already carries tool_access_policy and application_tool_policy, so these existing
owners should be composed. Remote member upcalls already conjoin explicit child
policy with the parent; the separate local agent-tools resolver still accepts
explicit overrides without that ceiling. Current model-tool spawn/fork/delegate
paths pass an inherited policy; the unchecked explicit branch is a contract gap,
not an established model-reachable widening exploit. Python customization can also drop
the missing binding, and direct host comms handles need a stated authority scope
before claiming all-route restriction.
No integrated acceptance for this issue is claimed.

## Recorded checkpoint, 2026-10-03

The latest recorded normal native push was
`9c996bac0b0bba7b709339858c155c57e5523eea`. It failed after 2,319.07s on
integration-test compilation and lints, before the machine and broad
deterministic test gates. The caller/lint repair is pending validation. Two
earlier focused repair gates passed 26 and 13 native tests respectively;
those 39 passes do not establish a successful normal push or qualify the
pending repair. Exact sources and raw results are retained under
(operator-retained path).

The 13-test acceptance gate includes real loopback model/tool/model continuation
with an exported alias to the same governed owner. Its negative setup refuses
an ordinary ungoverned bundle before provider calls or catalog rows; sharing the
same already-governed owner is valid.

Separate Console readiness response compatibility passed 69 UI/state and 70
transport tests. The existing contracts typecheck reported zero new errors and
41 documented baseline diagnostics. The source and its guide were committed
through `bf7930c34207f60458c3e351f1dbdb8cce368c18`; the installed MobKit gateway
does not yet emit the native readiness code. The Web SDK reason-preservation
fix passed 96 isolated JS tests. It and its CI unit-route correction are included
in this repair candidate, awaiting normal gates. These client results do not
establish integrated gateway or native runtime acceptance.

At that recorded push, physical SQLite custody was unimplemented and the held
C7 tests and temporary API scaffold were unexecuted. The current local physical
custody and same-process reopen results are recorded above. C4 durable grant,
controller and credential restoration remains unimplemented. There is no native implementation PR with
green CI or accepted overhead measurement. Full ABAC, sandbox and supported
surface/platform coverage remain required.

## Historical first-native-path acceptance, 2026-10-02

The native publication source at this checkpoint was
`5c833bdbdbfb7106019a81da0dec60f19a3f8bc0`.
Its initial focused attempt stopped at a mixed-cache bridge-symbol link failure
after 262.30s, exit 101; zero tests ran. Removing the cross-source target/lane
overrides allowed the same command on the same source to exit 0 in 273.34s:
53 selected tests passed in 5.942s of test execution; 4,710 tests were outside
the selection. Root matched all eight former failures to unique PASS lines.
That candidate also passed both non-default-feature governed JSONL tests
in 0.08s of test execution, 452.55s command time: local refusal followed by
permitted execution in the same run, and rejection of shared bundles and
unsupported wire setup. Four earlier functional passes on ancestor `2210e70b6`
remain source-qualified; its MCP and live-barge-in selections have not been
rerun on this candidate. There is no current native implementation PR, green PR CI, or accepted
overhead result. The full ABAC and sandbox objective remains open; these narrow
checkpoints do not remove persistence/restart or broader surface/platform coverage.

Earlier, root and independent raw review accepted the narrow R7 gate: 14 selected,
14 PASS, zero failures, ignored tests or timeouts, on source
`f7e871a46296a2c494cb748780db86957675d39e`. This covers 11 custody/cancellation
controls, two governed JSONL tests and one MCP unsupported-profile control;
historical gate counts are not included.

The first supported slice is fixed-host, single-connection JSONL with stock
memory-backed persistence for the process lifetime. Real loopback model HTTP,
model-visible tool refusal, a permitted callback and same-run completion pass
with retained actor/seed/context and stored history/audit. The MCP control
proves refusal before setup, not governed MCP support. See the
[native embedding guide](../../../rust/native-authorization.mdx) for the exact
realm, selected-client and fixed-registry configuration.

R7 evidence is retained under
(operator-retained path),
seal `68c703166c141ad2b46fe6752ff459fb43dc828a265ca27263904968719ad45e`.
Schema R2 and module metadata acceptance are separate, source-qualified gates.
Clean checkpoint `11b3b77912bd7c6511f3d26f7f2a1b81087948e0` normally merges main
without an authorization-code change; it does not establish a new aggregate
runtime result.

The accepted R4 optimized build and two correctness smokes cover historical
source `455d0e3b6bb143a53017392b203df49af2b518cc`, not the final publication
candidate. Their receipt is
(operator-retained path).
Before a fresh measurement grant, the final publication candidate must have
its own exact-source optimized build and both correctness smokes. Performance
remains unproven. Window `W-minimal-20261002-1` was not granted and was cancelled
at 2026-10-02 11:19:43 UTC; zero measurements ran. Measurement ordering awaits
the owner's cost-timing clarification. No publication-first sequence or new
benchmark window is established by this record; low-overhead acceptance remains
required.

Separately, 47 Console backend tests passed on
`13e1847720a5e1ae0654137da724f085fcf68bbc`. The reviewed UI repair patch
`081bf002dc1e80c08b72a139600e9cd092e84597dee0f9597dc78aa0006993f2`
is committed remotely as `77d9eb0f0eb67e5058b41cdc56ff211de9003968`, tree
`299b015fa48937db6a7fb09f90982157c31a5fbd`; normal commit hooks passed in 0.16s.
The local private checkout retains the same patch on `13e184772`. The repair
passed 20 component tests and the real checked-save/recovery browser scenario,
including three exact writes, no read-only writes, and recovery after one
deliberate failed protected read. This is acceptance of existing MobKit access
administration, not native governed work, SQLite/restart, or JWT ingress.
MobKit is version 0.8.45 with Meerkat dependencies pinned to 0.8.50. PR520's old green CI does
not qualify the reviewed repair; current publication and CI are separate gates.

The earlier publication plan targeted `release/0.8.51`, reserved tip
`178f137543d532820cbe1f9013a25de93975de10`, and held other queued PRs except the
no-wedge fix, TLA precedence fix and demonstrated mandatory tip CI blockers.
That reservation was released at 2026-10-02 19:52 UTC. Development continues on
its own branches; full-feature integration remains held. Neither a native
checkpoint PR nor a Console PR520 milestone merge has occurred.
Regenerated source was committed at `5c833bdb`; its normal commit hooks passed.
The focused repairs and non-default-feature governed JSONL pair passed on that
source. Normal push hooks and required PR CI were pending at that checkpoint.
Source preparation is not a native PR or CI result.

The separate persistent controller-administration extension passed eight focused
tests on `8f65c1364395f7f91c477134b859069b64e2132b`: the unchanged real JSONL
scenario, four native custody controls and three memory-store controls. Root and
independent raw review accepted matching source/binary identities and all eight
passes, with no ignored tests or timeouts. The initial candidate on `c20172`
had returned `Unavailable` instead of `ControllerInUse`; the repair joins the
initial empty Idle checkpoint to the actual live owner without adding hot-path
persistence. The original failure and diagnostic evidence remain preserved.
The accepted result is under
(operator-retained path).
At that acceptance checkpoint, this source candidate was outside the first
publication checkpoint. It covers loaded owners with the stock memory store,
not SQLite or process restart; later integration needs its own source-qualified
validation. SQLite/restart,
broader surfaces, performance and full five-story/37-checkpoint ADR coverage
remain required next slices. Earlier
records retain their historical source and scope; this section supersedes
only their first-native-path and publication status.

## Scope change, 2026-10-01

The owner explicitly approved replacing the previous first-profile requirements
with the [local governed default](../adr-001-local-governed-default.md). Its
r8 candidate has four-reviewer design acceptance. The old 111-requirement/57-case
inventory must be dispositioned against this amendment; it is no longer a
blanket requirement to finish parked high-assurance work before delivery.
Existing source/test claims below retain their exact historical scope. They
do not establish an implemented local governed path.

## Earlier integration and publication checkpoint, 2026-10-01

The integrated donor remains `meerkat-native-governed-m1` on
(operator-retained branch). Its frozen audit/SDK checkpoint is unchanged:
266 selected Rust tests, 414 Python tests and 460 TypeScript tests passed, with
typed TypeScript compilation and the recorded schema/surface checks. Evidence
is retained under
(operator-retained path), manifest
`e3e352ff860ef150aa1a6ee090a410870b9db84939ab2fe03ff3e1eaac52be33`.
This includes the storeless native model-tool-model fixture: denied delete,
feedback, permitted read and completion of the same run with native audit
assertions. Scripted provider results do not prove real-provider or persistent
coverage.

Publication is a separate extraction in `meerkat-authorization-publication`,
branch (operator-retained branch), based on main `9ebe09fa`. The first
slice preserves qualified principals, pure contracts and the generated grant
owner without runtime enablement. It has passed 42 contracts tests plus six
documentation tests and 291 authorization/schema tests. Canonical machine
generation and release metadata passed; strict lint, packaging and WASM remain
pending. Missing RPC, MCP and constructor closure
in the next checks-and-audit slice is being addressed in its extraction plan;
the larger donor is not an independently publishable audit-only package.

The [UX/DX r5 design](../adr-001-authorization-ux-dx.md), SHA-256
`d87179cecf50154d8abff97bbfa23aeab445dad40f4fef82dd344f143b2b4ab0`, and
[Turbo S r7 design](../adr-001-turbos-e2e-scenarios.md), SHA-256
`659c2674cd981410c24b910506fddd6cede524ba26ebe30c9bc9a29f3670ba7c`, are design
artifacts. The E2E plan has five stories and 37 named checkpoints, budgeted at
17 live calls per attempt or 34 with one outer retry. These are not executed
coverage. Root reports that final packet
`b17de27b1043db434d4ef9c1b6df40fd07291637a9e9b94f8ab032a00246ceaa` was accepted
by GCP, the pilot host and the autonomous host at 16:14 UTC. The adapter consumer verified the final
two-paragraph delta, with closure relayed by GCP. All four reviewers have
closed the UX r5 and E2E r7 designs; this is not execution coverage.

[Draft MobKit PR 520](https://github.com/lukacf/meerkat-mobkit/pull/520) contains
the first console slice using existing contracts, not new native producers.
Its CI asset failure was diagnosed as CommonJS symlink-path pollution. A tested
repair is pushed at `8ebe2cb9ecb826e8fb1eab00ac0fd02e449d4cf8`; local freshness
passed and new CI run `36891393790` is pending. No green CI, merge or deployment
is claimed.

The [critical path](critical-path.md) retains the full implementation scope.
The four controller-continuity RED cases, account/credential custody and the
controller-setup fixture repair remain for the next checkpoint. Optional OS
confinement, consent and persistent/detached production changes remain held;
post-effect live diagnostics and interrupted-callback recovery are still open.
Full provider/source/independent consumer/comms/delegation/surface support and measured cost
remain required. No benchmark or full-profile acceptance is inferred from these
package results or design reviews. Historical records below retain their
original scope and do not override this status.

## Earlier native and high-assurance checkpoints

The current lower native association checkpoint is clean at
`2356d0eccb6b6a43172e5d73474a22745f1a53ee`, on composed prerequisite parent
`8a6265fd6`. Contracts34 tests+3 compile-fail docs, combined host114 tests+3
compile-fail docs, strict package lint, stable-tree Cargo/Bazel/example metadata
and normal commit hooks passed. The adapter consumer independently accepted the frozen source.
INTEGRATION-02 drained at09:52:06UTC. Real governed native admission, witnessed
acknowledgment and physical dispatch remain unimplemented; no native governed
request has passed. Full WASM/publication gates remain open.

The parent combines retained SQLite custody, authenticated workload
observations, transaction-fenced input construction, the grant host, pure
contracts, checked child restriction data and generated single-owner
preparation. Its canonical generation has zero drift, and the retained-custody
metadata regression passed with a discriminating failed pathname-reopen negative
control. SQLite90, combined authorization130+3docs, contracts13+3docs,
DSL-core32, single-owner fixture15 and model2 passed. These are prerequisite
checks, not the integrated governed path.

Witness journal commissioning repair has independent bounded source acceptance;
its real-file regression tests remain unexecuted. The adapter consumer accepted the frozen
grant target-association source, and canonical generation is queued in the next
serialized lease. Clock and witness encryption sources have separate frozen
review candidates. The clock profile has a known idle-entry latency failure
against the unchanged budgets in [the acceptance plan](acceptance-plan.md).
Early demand refresh does not close that failure.

The independent consumer PR4 at `64e5a8d02eda56df6b34c9c448704e87eaabda23` has green hosted
CI run36838043537, including full-history scanning and all configured required
suites. E2E was skipped. The history repair preserves the previously reviewed
final tree and its2,052 unit-test results. Earlier failed evidence remains
retained. No merge or deployment occurred.

Root independently checked all 86 atomic-composition and 88 CoreNext raw
log/result entries, including commands, JVM options, mappings and reported state
counts. Atomic classifications are unchanged. CoreNext retains 13 positive
bounded witness goals, seven unproven goals and six deadlocks; its mob-seam CI
row changes from timeout to out-of-memory, while the adaptive CI/deep passes
have only one distinct state and are vacuous. This is not universal model
success or an end-to-end performance improvement. The raw verification is
(operator-retained path), SHA-256
`097f6878404b80c24ea0845d83b390191fb47bb99adb0af5721952e7556dcacc`.
The GCP09:00-09:45UTC reservation produced an incomplete retention comparison;
render load and the unchanged50000-row deadline prevented full acceptance.
The detailed receipt and future resource conditions are in
[the critical path](critical-path.md).

## Earlier published checkpoints

| Candidate | Exact source | Evidence and limits |
| --- | --- | --- |
| [Meerkat ADR PR 1352](https://github.com/lukacf/meerkat/pull/1352) | `97d3078e2e5cff9f0a49e3f46c67d2da7a9a0fe9` | Four requested project reviewers accepted the frozen r6 design. Scoped PR source reviews and two pilot host record corrections are retained. Hosted CI gate passed at this head. The PR remains a draft; it changes documentation and archived-review hygiene, not runtime enforcement. |
| The independent consumer prerequisites change | Published `e55531eb0e41f0c04ab356f9100aa7869197cc36`; local rebased source `3985dbe37f8ee842848c776860d5e6ecf94cc885` | Independent ABAC fixes, verified caller context and retained-work repair passed 2,031 local tests and strict lint at the recorded integration checkpoint. The narrow synthetic-fixture repair passed local full-history scanning, two negative controls and the real authenticated MCP regression. Normal publication hooks and all eight applicable hosted jobs passed in run 36809459432, including all-feature tests and full-history scanning. The workflow skipped E2E; no E2E result is implied. No governed endpoint is enabled. |

The ADR publication explicitly disclosed the failing unchanged broad local
workspace gate and the docs-only publication's `SKIP=cargo-test`. Other hooks
passed. The dispatcher incorrectly wrote a full-success stamp for this partial
run; the exact stamps were archived and removed so ordinary later pushes cannot
reuse them. Independent reviewers confirmed that defect in the unchanged
dispatcher, and its owner has the finding. Hosted CI results are separate
evidence, not a claim that the skipped local gate passed.

## Independently scoped implementation evidence

| Component | Candidate | Bounded evidence |
| --- | --- | --- |
| Qualified canonical principals | `07788de70f8453bcb3a6583418927bb265a9a3e4` | Rebased on main `7ace16ed9`; owned source/schema/fixture bytes preserved. Eleven principal tests and six wire tests passed after rebase. Earlier exact production checkpoint passed the declared semver and WASM checks. Qualification is identity syntax, not authentication. |
| Pure authorization contracts | `0955991d5` | Restrictions, conjunction, evidence encoding, information dependencies and negotiation. Forty-six unit tests plus the shared negotiation consumer passed before a byte-preserving rebase; strict lint and Bazel freshness were checked. No live permit or operation owner is supplied. |
| Physical SQLite custody | `775234131` | Eighty-two SQLite and seventeen custody tests plus strict lint passed. The normal publication gate passed 12,044 workspace unit tests, then both integration attempts hit their 900-second deadline; the push failed. Timed-out tests are not passing evidence. The warm retry then failed two timing tests outside the custody diff: 12,042 unit tests passed, two failed and 17 were skipped. It did not publish. The log is (operator-retained path); no skipped-hook success stamp is claimed. The GCP exact-tree unit 12,051, integration 2,759, HeadCanonical 9 and fast E2E 30 all passed. The report and archive were downloaded and independently checked, including all 16 embedded manifest entries and eight raw lane summaries; the archive SHA-256 is `a8e353f2a226b1a25bca2b99289c5d953577862280f7948913fd870f23477373`. These Linux lane results create no local hook stamp. Physical ownership is not antirollback authority. |
| Native retained connection adoption | `ff3af7b4e8940826b7a753b6e043b5115391d01b` | SQLite 88, session/schedule 82, runtime SQLite 107 and facade 5 tests passed, with strict four-package lint. Twelve reviewed source hashes survived rebase. Session, runtime and schedule share one physical connection; other realm writers remain unadopted. Uncertain commit and leaked transaction retire the shared handle before queued aliases enter. EVID-2 remains open. |
| Exact numeric model packets | Reviewed `88089558f24636134aee16d175bc5f1523f828d2`, rebased to `1877c81f4d36116de4a07e74753d69650ce4d6cf` | Generated numeric witness packets and actual reached model transitions. The normal publication retry with the actual MCP fixture failed three MobKit unit timing/lifecycle tests: 12,063 passed, three failed and 17 skipped. Integration did not run in that retry; no push occurred. The failed gate is frozen under `numeric-publication-880-with-fixture`. Rebase onto main `39cae9bec` preserves the two source files and includes the upstream fixture and truthful-stamp repairs. Current-base regeneration and a controlled publication attempt remain pending. Bounded models do not prove a production store or verifier. |
| Generated atomic composition and compiler repairs | `bd582768368599ab7a7651e58a9f55150dfd0fd9`, `802f18ac1b0e50ffb108f4123161937b3ed2f2cc` | The coherent generic stack is committed with a clean tree. 681 tests, 17 final fixture tests, strict all-target lint, normal hooks and drift for 15 machines/9 compositions passed. Conditional substitution and mixed phase/data repairs affect 12 canonical model files; the other 12 and Rust compatibility kernels remain byte-identical. A checksummed before/after packet containing 31 bounded and 12 separately classified deep configurations is with the GCP lead for controlled acceptance. The controlled comparison reports no regressions on completed rows: 31 bounded rows include 18 passes, seven unchanged unproven goals, three unchanged deadlocks, two incomplete and one unchanged rc151; 12 deep rows include six passes and six incomplete. Root verified archive 16942f40 and all 43 paired classifications/counts/coverage in the summary JSON. Raw per-row command/environment/log archive is separately requested; no universal model-success or local-rerun claim follows. |
| Generated atomic joins and keyed witness owners | `04defbcba8c9c2f4689e79fb90bd91fe540f16f0` | The canonical feature host, keyed owner machines and private generated join are committed. Sixty-four package tests, strict all-target lint, metadata checks and normal hooks passed. All 14 frozen witness traces reach their goals, including positive coverage on five refusals. The explicit `witness-control` feature test target exercises the actual host; the SQLite successor also adds a PR/main feature-unit suite. Accepted-row absence, mismatch, malformed bytes and unexpected occupancy invalidate cached custody; ordinary pre-acceptance storage failure remains distinct. Durable backend, authentication and recovery lifetime remain separate obligations. The subsequent actual-owner codec opt-in is committed as `386b64b328aef8eb000a7a2629b7a5bb61c132c0`: 70 package tests, strict all-target lint, selected generation/drift, metadata and normal hooks passed. An independent reviewer checked all 42 source/artifact hashes and eight gate logs. Untrusted snapshots remain data, not current authority. |
| Generated owner state codec | `230a16791595960d50e2a980e3dd2742f93c3ecf`, `6cf865d2551f4269370c7d763e71cc8ab7985536`, `6d24ac7a3f4fa4250e4fae8e5505db97a2020a81` | Explicit DSL opt-in emits strict, versioned untrusted snapshots without serializing live authority. Structural bindings and borrowed capped output are committed and independently source-reviewed. DSL 24, owner codec 14, principal 15 and schema 35 tests passed; all 17 non-opted owner expansions remain byte-identical to the coherent generic baseline. Combined strict all-target lint for DSL, proc macro, schema and core passed, as did formatting, Bazel freshness and normal hooks. The conservative principal source fingerprint can invalidate persisted snapshots after nonsemantic source changes; migration or refusal is required. |
| Physical witness owner backend | `ef8deacbc4fc2680e927aba263059eecd9483bc9` | The SQLite adapter uses the retained physical connection, exact generated snapshots, a namespace head CAS and one required-absent or existing request row in one synchronous transaction. Two independent source reviews exposed and closed TEMP shadowing, direct and indirect foreign triggers, inbound foreign keys, foreign indexes and conflict-classification gaps. The indirect-cascade finding was reproduced against real SQLite. After integrating the actual owners and codecs, 71 unit tests, 23 integration tests and one compile-fail doc test passed, including actual SQLite COMMIT refusal, post-check corruption and physical retirement. The first compile failure was a test-fixture static-lifetime declaration and is retained separately. The final adversarial review additionally reproduced main/TEMP FTS3 and FTS4 shadows of the table-valued foreign-key introspector. All four actual Rust regressions failed before repair, then passed after direct catalog collision refusal. Final tests passed 75 unit, 23 integration and one compile-fail doc case; the exact PR-unit nextest command passed 75/75. Strict all-target lint, WASM compilation, CI selection/selftests, final metadata and normal commit hooks passed. The initial stale-lock metadata failure remains alongside its successful refresh. Independent review is GREEN at the committed source hashes, with no governed activation claim. Eleven source hashes and 20 evidence files are frozen under `witness-sqlite-ef8deacbc`, manifest SHA-256 `89ec02bccca7f37d317115fdf344ad4df6904277bbce9b4ecea3f3d869e39817`. |
| Canonical grant feature host | `e9d6f02f349518c7520c37a25410ad38113c7e2d` | The isolated prerequisite is committed with normal hooks. Fifty-four feature tests, 297 schema/kernel tests, all five reached TLC witness goals, strict all-target lint, selected drift and metadata checks passed. The first restoration witness deadlock exposed an omitted expected-transition declaration; that failing receipt is retained and the canonical witness metadata was corrected without changing its reducer. This host preserves existing cached validation semantics. Stored content digests and an earlier Valid state are not current permission or constraint attenuation. The reviewed r7 durable handoff and owner-generated domain projection remain separate work. |
| Explicitly enrolled workload authentication | `8d580f52789161ab7bf80b3c4c1c70c8fcccdde8` | Clean normal-hook checkpoint. The exact source passed independent review, 369 focused/package tests including two compile-fail cases, strict all-target lint, full canonical drift (18 machines, 10 compositions), bounded TLC (62 generated, 26 distinct, depth 5), metadata and hooks. The finite NatValues={1} model does not prove higher-generation rotation; actual Rust rotation tests passed. Real Ed25519, strict field/wire binding, weak-key rejection, administrator-pinned bounded file access and FIFO cleanup are covered. The private observation proves cryptographic binding under selected enrollment, not independent currentness, measured time, replay, a human requester or permission. Frozen manifest `workload-authn-8d580f527/manifest.json`, SHA-256 `24d5b40743f3a20091112d983a18f96fd5e671fddcdb26f360ae09ffe9b4816c`. |
| Independent consumer negotiation | `188e4437249b4bf76df9414388ec41dae5e1e303` | The independent consumer consumes the exact 14-case Meerkat negotiation corpus without a Meerkat dependency. Its 15 existing auth tests, two consumer tests and strict lint passed. The adapter consumer and a subagent accepted this exact bounded source. Raw ingress and actual protected sinks are not established by this normalized corpus. |
| Independent consumer restrictions and information flow | `82198dbbe8b5a014dd292f8806a9e4993f547c56`, rebased to `e55531eb0e41f0c04ab356f9100aa7869197cc36` | An independent implementation executes all 26 shared cases, including 13 actual derivation steps. All 23 auth/negotiation/information tests, strict lint, formatting, docs, surface and version checks passed with before/after source hashes. A subagent and the adapter consumer accepted the bounded source and recorded evidence after diagnostic redaction was strengthened. Neither reviewer independently reran the checks or attested the complete build graph. Rebase onto the hosted-CI-green the independent consumer head preserved all eight reviewed source hashes; all 23 tests passed again with `--locked`. Resource-domain differential and actual authenticated sink integration remain open under A003. The reviewed conformance commits are integrated into the clean PR worktree at e55531e. The normal e555 publication hooks passed, including full workspace unit and strict lint, and the head was pushed. Fresh hosted CI did not start because main #3 introduced security conflicts. A separately reviewed semantic-union rebase onto exact 0ce109a is clean at 3985dbe; root verified all ten changed hashes and all eight preserved conformance files. Fresh focused tests passed 913/913 (seven existing ignored), and all 23 auth/conformance cases passed. Strict all-feature lint and normal publication gates remain pending. Only the earlier 5e8efec head has confirmed green hosted CI. |

GCP additionally found that ordinary composition `CoreNext` consumes queued
packets by sampling argument domains again. This can disable valid out-of-domain
packets and explode initial action enumeration. A separate successor candidate
changes consumption to use actual queued payloads, retains explicit forced
Boolean guards, and rejects undeclared trigger bindings. Injection sampling and
owner-feedback routes remain separate. Independent source review is green and
all 140 code-generation tests passed, with two existing diagnostics ignored. An
identical extending harness deadlocks with the old generated model (one state)
and reaches the goal with the candidate (seven generated, four distinct states),
including positive coverage on both queued revisions. After consuming the coherent
generic dependency, all 144 code-generation tests passed with the same two
ignored diagnostics; the exact probe model, harness and configuration remain
byte-identical. The historical before-probe predates the generic merge; it is not
claimed as a fresh probe at the exact comparison base. Candidate `73359c9fad528b3b7eefada1fa3c772779bcc005`
is committed with a clean tree. Combined strict all-target lint, normal all-codegen,
all-drift and normal hooks passed. Independent source/operator review verified
that only CoreNext and existing fairness call expressions changed in nine canonical
models. The frozen 35-bounded/9-deep before/after packet has been sent to the GCP
lead. With explicit root agreement, CoreNext now runs while the release compiler batch prevents retention measurements; retention still requires a later coordinated compiler-free and TLC-free window. Its archive
SHA-256 is `1a81c6c7636b0ef235d5ccbc123b570c60c74ed1f557016bd9c77e2ee110d179`.
Canonical controlled TLC acceptance remains pending.

The [owner composition review record](owner-composition-review.md) records all
four project-reviewer acceptances of the implementation protocol and its keyed-request
amendment. These are protocol verdicts. They do not close the implementation
requirements in the table or the complete requirement inventory.

## Executed runtime observation

The [baseline tracer](runtime-tracer.md) now ran successfully against published
Meerkat 0.8.49, with deliberate process loss after durable input admission,
recovery of the exact input without resend, actual helper delegation, parent
continuation and console delivery. The adapter consumer accepted its bounded diagnostic
evidence and the corrected explicit export allowlist. It does not prove
requester propagation, live authority, audience authorization or helper history
durability across restart. The old manifest that selected identity-key paths is
retained privately for audit and must not be used for packaging.

## Historical next acceptance boundary (2026-10-01)

This section records the plan at that date. See the current checkpoint above
for completed acceptance and remaining work, and [Native authorization](/rust/native-authorization)
for the supported local profile. The earlier independent-witness proposal below
is not a requirement of the local default.

The first governed vertical slice still must compose authenticated ingress and
durable work association, canonical grant/policy/resource generations, physical
custody with independent witness recovery, and exact entry/settlement evidence
at real model, tool, source and recipient boundaries. It must demonstrate both
allowed effects and physical absence of denied effects after revocation,
restart, missing evidence and uncertain outcome. MobKit-owned reads and streams
and independently enforcing the independent consumer operations must use those same contracts.

The full feature/surface inventory and deployment budgets remain required after
that slice. The pilot host supplied a recorded workload of 20,000-50,000 work items for
retention-cost comparison. A local native in-memory adapter benchmark cannot
establish latency on its 16 GiB deployment under swap; matching-environment
performance evidence remains a release requirement.

The first Mac retention run is `NOT_MEASURED`: both 1,000-row functional pilots
passed, but concurrent compiler load prevented admission of any 20,000- or
50,000-row measurement cell. It supplies no accepted latency or overhead figure.
The first GCP retention run is also `NOT_MEASURED`: both functional pilots
passed, but no measurement cell obtained the required compiler-free window.
All eight large cells remain unmeasured; observed idle CPU alone did not satisfy
the admission criterion. Linux time and compiler-process adaptations are retained
with the raw receipts. A coordinated quiet window is requested. These results do
not certify the pilot host's deployment budget or create local successful-hook stamps.

The GCP principal tree passed 12,083 unit, nine HeadCanonical and 30 fast E2E
tests. Its full integration lane failed two MCP fixture-dependent tests, with
2,769 passed. The unmodified base failed the same two tests; supplying the actual
fixture executable made all 12 focused tests pass on both trees. The original
full lane remains failed. The upstream fixture-publication repair is merged;
new publication attempts will consume that repair instead of relabeling the old
failed lane.

## 2026-10-01 implementation coordination update

- The lead assigned GF-4 native InputId association, dedup and physical-attempt integration to this lane on main e7f7d948 or later. A read-only exact source proposal was sent before reserved DSL edits. Adversarial review has already required durable one-time release handoff, authority-scoped supersession, and selection of the actual runtime prepared-boundary integration; the generic atomic join cannot simply absorb MeerkatMachine signals. No governed source or activation claim follows from proposal ownership.
- Canonical state projection is committed at d93f251ba4d81453ccec5ff5481417232754a2c1. All 106 unique focused tests, strict lint, 19 unchanged non-opted owner expansions and metadata/hooks passed. The ten directly affected fixture tests reran after a test-only lint repair. Production framing stayed byte-identical to the reviewed candidate. Full snapshot identity changes when opting in, and old snapshots refuse explicitly. This is a projection prerequisite, not grant validity.
- The combined witness/authentication integration is clean at 0a29f1dbd after normal cherry-pick hooks and canonical lock refresh. All 124 combined-feature tests passed, including three compile-fail docs. Final strict all-target lint, combined-feature WASM compilation and metadata checks all passed. The immutable combined archive manifest is bd48ecab04394efa60eb53151f6e06c31f9655542036eba9ccf9644e6c909cb8. Existing cryptographic and SQLite production source hashes are retained; no operational service/currentness is implemented by composing the features.
- The accepted frozen r7 protocol documents are committed locally at 61ac05c2d, followed by the bounded native-tracer companion 75bd40ef3 and explicit post-fence time clarification e23829ed2 and its corrected revision 90477194b. Docs checks and normal commit hooks passed. These commits remain unpublished; PR #1352 still exposes 97d3078e with its earlier green CI.
- The lead conditionally accepts witness time at a fresh post-fence logical decision point with a historical durable reply. GL4 remains open until authoritative final release checks, fresh retry observations, uncertainty refusal, cleanup and exact pause/race fixtures pass. All four project reviewers received the written clarification. The inspected GCP deployment lacks the proposed authenticated clock source; no daemon or host-clock change has been made.
- The numeric publication lane remains held for the lead's mob flake-repair batch SHA. The separate main integration repairs do not authorize retry. No hook bypass, merged PR, production deployment or full ADR completion is claimed.

## 2026-10-01 adversarial continuation

- GF-4 revisions 5/6 closed restored-away entry recovery, positive expiry, explicit human-only Unknown continuation, predecessor qualification and reached model obligations in design. Lead and adapter consumer exposed hidden adapter resends, incomplete final provider-wire binding, untagged output fanout and continuation lost-ack replay. Revision 7 received lead acceptance with final text conditions. Revision 8 (89250ecca1a51f8a5b36f302161d7fdba0d3607a0095427da0a5874a33457090) is with both reviewers, adding explicit reqwest no-redirect/no-retry construction, HTTP/2 probes, refusal of catalog-selected realtime text, and one generated continuation successor per Unknown predecessor, including overlapping-set and concurrent-command races. It binds final lowered wire bytes, forbids internal resends/redirects, selects buffered first-profile output after committed settlement, enumerates all consumers and reuses canonical admission idempotency for continuation. Native admission remains dependent on NativeWorkAnchorV1 and the live b+2b handoff; attempt source remains held. A surviving entry anchor plus restored Unissued local bytes does not establish non-disclosure.
- The generic single-owner preparation source is frozen uncompiled on d93 with 20 paths (manifest ea72dcda) and independent bounded source acceptance. It retains the actual persistence future through publication, poisons at bind to cover forgotten guards and opts in no production owner. Its 14-commit prerequisite stack is cleanly rebased onto main 456dc09cb at 8cd7d7a79, preserving upstream redaction. The successor source is independently GREEN on exact current-base manifest 5e53350c; root inspected the emitter and authorized scoped generation/execution, now running. Actual native runtime gate integration remains open.
- The additive observed-input join source passed two independent pre-build reviews. Canonical generation passed, codegen 149 tests and actual SQLite 108 tests passed without source repair; strict lint, drift/WASM/metadata and hooks are running. It supplies fresh mechanical post-fence observation and explicit rollback cleanup, not trusted time or release authority.
- Witness bootstrap revision 2 (27c90d3acc579978a6aa4175cb197a34d0ec8f9991d4170206fc75a3829c7d8a) closes two-sided commissioning, client-detected witness rollback and singular retirement in design. Revision 3 received conditional schema-partition acceptance. Revision 4 df967985fc0ce7010ed6e1cbc9d0717154a50afa6a218ab260bfad1b325eacc1 writes in the incident record, inner profile identifier, gated client high-water acceptance and explicit refusal transitions. It also requires a client-secret keyed plaintext commitment and client-authenticated outer custody envelope, closing the witness-admin dictionary-attack gap in the prior bare-hash proposal. Current-main prerequisite composition is starting; no schema or crypto producer is yet implemented. The operational service and actual clock producer remain unimplemented.
- Time clarification cf6c2398d7c83c801d8f03f7977c56ef9fde711a21af3dd18cdf151619e48639 has exact design acceptance from the lead, the pilot host and the adapter consumer. The autonomous host's current revision response remains pending. Those reviews establish no producer, sink or deployment qualification.
- Pure restriction contracts relocated byte-preservingly at ea8ae6ef840dbf2edd67ba42e669931e7c208c55 with focused gates and normal hooks. The isolated immutable child-derivation carrier passed 13 unit+3 compile-fail docs, 46 existing authorization+ 1 conformance tests, strict lint, WASM and strict metadata checks. A discriminating null-payload test repair and narrowly scoped lock repairs are in final source review (manifest 5855d806); normal commit passed at ac63171e261e33bd60c2a1564be530c265b05408 with a clean tree. Nine source and fifteen evidence files are frozen under derived-restrictions-ac63171e2, manifest fba4d286ce2609bdcd628d0cb41b45fb5bf82da422e095aedd766a3874768f40. No compiler/grant integration or runtime authority follows.

## Critical-path control, 2026-10-01 08:30 UTC

The [integration critical path](critical-path.md) now makes the first actual governed native prompt the next integrated acceptance milestone. Independent prerequisite breadth is subordinate to that path. Review is relative to accepted source checkpoints; only concrete integration defects reopen settled work. This changes scheduling, not the complete ADR scope or its open requirement/case inventory.

The Mac build queue is serialized after its already running jobs drain. The independent consumer's normal publication completed at exact `3985dbe37f8ee842848c776860d5e6ecf94cc885`: 2,052 workspace unit tests passed, 97 existing skips; all normal push hooks passed. The PR is mergeable and fresh hosted CI run `36835999238` started. Its full-history secret scan failed and is under read-only investigation; fresh CI is not green. A concrete SDKROOT difference explained the repeated native Clippy build. Future manual prevalidation will match the normal hook environment without adding a tooling project.

Generated preparation owns the next focused Mac lease. Root then reuses the now-idle `adr-observed-input` warmed cache for the single coherent integration gate. Observed-input is clean at `2bf69f187665c017ce932d6a715080746365db92`; package149/auth108, strict lint and focused drift passed. Its all-catalog drift was deliberately interrupted and WASM never started; both remain explicit integration gates.

The GCP lead reserved **09:00-09:45 UTC** for the unchanged retention benchmark controller and binaries. GCP agents must stop heavy work by 08:58 and launch no compiler/TLC/hook-running push during the reservation. Host idle and compiler-free preconditions remain unchanged; violation yields NOT_MEASURED rather than another blind probe. CoreNext's 88 comparison jobs have finished, with classification/raw evidence still pending.

GF-4 revision9 (`7d11682b067c38e54db57c78a1f1de4b59644849b5710510271ca62935dd779f`) has lead and the adapter consumer textual acceptance. It closes complete owner-derived frontier, surviving continuation/readmission claims, linear recovery chains and exact committed observation recovery. Source must distinguish a surviving committed outcome with pending evidence from Unknown. Attempt source review may proceed. Native admission edits wait for the lead's explicit handoff at the second live-path branch's merge commit; unrelated main changes do not trigger repeated rebases.

Witness bootstrap outer revision5 (`c01fae20ea2f12349342d1d21aa6b08d3b3c90a509bb1a77d974457876f292e0`) is accepted for source. The neutral certificate-evidence framing is frozen before implementation. This does not qualify an inner proof, clock, key producer, current witness or runtime entry.

The root composition onto generic/main456 reached `c92544f6e3de`. Delta review found one real integration defect: the newly upstreamed current-metadata read used a pathname instead of the retained ConnectionSource. The one-line repair and real closed/poisoned/valid-replacement regression are independently source-reviewed; execution awaits the coherent integration lease. This finding is distinct from an already accepted component being reopened without evidence. Strict canonical Bazel/Cargo lock checks passed, with all 254 unrelated live-universe pins preserved.


## 2026-10-01 coherent checkpoint and first-path deltas

Composed current-base source8a6265fd6 now has focused execution evidence and canonical zero-drift generation, recorded in critical-path.md. The retired-custody metadata regression discriminates the repair: restoring the old pathname read fails the closed-custody case, and restoring the accepted source passes all90 SQLite tests. Combined authentication/witness130 tests and3 compile-fail docs, contracts13+3docs, DSL-core32, single-owner fixture15 and model2 passed. Strict lint/WASM and complete publication gates remain open. Required Cargo/example-lock consistency and strict Bazel metadata passed; all254 unrelated live dependency pins were preserved.

The lower immutable native association source is frozen at manifest3da7e0285e862ed618a00d122b39b3633d4f4a34ca9083280ad26ffdd85b2716 for the adapter consumer review. It contains complete original work/authentication/grant/source references, an exact qualified ingress key and bounded canonical binding bytes. These are caller-constructible data, with no accepted/current status. Existing protocol/information/evidence definitions move to the lower contracts crate with exact-type host re-exports; production definitions retain their original bytes. Real generated admission and surviving acknowledgment are still required.

Witness physical journal source is under independent review at9237565f86d270c40dd7b589bba38a27ef1053ac1ea6799f72ce188051b290d3. Its12 real-file tests and3 no-create custody tests are authored, not executed. The sealed native-work format r2,8022cdbe8968b3458a3b65b7ba956d1bf2254a20370e288696baecdaaa086e3d, has root source-contract acceptance f17154ec7f37f35f23c1686b78ce3c4a269efa9d8d902139501215873371e46b. Actual length and native receipt digest stay encrypted; encryption and commitment key generations are separate; public native receipt/head commitments are item-specific keyed values. Real historical key custody and authenticated service/client completion remain open.

Measured-time proposal r4,7c070013280e05ffb3a4f34c6b0b23ff4912f18a444954db839012bb644e3090, has lead conditional acceptance219359a1fffcff6b9d3ab7ed3d620735e62f99eaa8999ddf526e724ca059b5c3. Source work must add actual single-flight acquisition, preserve valid prior samples on failed refresh, account for all qualified host-discipline terms and declare the short sample lifetime/acquisition latency. A fresh stateless observer per demand bounds all possible NTP samples inside the acquisition bracket; reference timestamp changes do not prove fresh measurements. Deployment qualification and the model-entry latency budget remain open.

The independent consumer's identical-tree history repair at64e5a8d02eda56df6b34c9c448704e87eaabda23 now has green hosted CI36838043537, including the full-history secret scan and all configured required suites. E2E was skipped. GCP retention run2 remains incomplete:50000-row paired processing exceeded the existing deadline while unrelated render load was present. Partial20000-row data is retained with its host-load limits; it does not close full workload or the pilot host deployment acceptance. See critical-path.md for resource decisions and actual timings. Full ADR implementation remains OPEN.


## 2026-10-01 bounded source review and clock budget disposition

The native association source has independent adapter consumer acceptance, review8b2576b2b847c4b36d1111ba7199f85187e25441c8f4f018d21fe1fa282810cf, on frozen manifest3da7e0285e862ed618a00d122b39b3633d4f4a34ca9083280ad26ffdd85b2716. Root verified all review hashes and read the complete report. Contracts34 tests+3compile-fail docs and combined host114 tests+3compile-fail docs passed. Strict lint passed after one test-only missing semicolon was repaired; production bytes remained unchanged. Canonical metadata and normal commit are completing in INTEGRATION-02. Native acknowledgment/currentness remains open.

The accepted LinuxChronyNtsV1 r5 design has a known model-entry and turn-overhead budget failure after idle periods: conservative sample lifetime is seconds or tens of seconds and a fresh stateless authenticated acquisition takes multiple seconds. This is explicitly recorded in acceptance-plan.md as failure against unchanged numeric bounds, not merely missing measurement. Demand-triggered early refresh through the same owned single-flight worker is selected for active conversations and remains unexecuted. It does not resolve idle latency or qualify the deployment.

Witness key-custody source has bounded root acceptance096454ea8e480cceaf158e2021a994c3edeb564b444304f34789b26b6eedef97. The actual selected protected commissioning/client factory and administrative/recovery-root qualification remain open. Independent physical-journal reviewd49ddaea28539d81bb5079e2e3a4277b6752b90852c375d3847daa827d344d3d found NW-P2-1: commissioning retry could return success for an absent primary head with surviving child history or an existing head with corrupt indexes. The owner is repairing transaction-local inventory checks; the original RED checkpoint remains retained. The adapter consumer is independently reviewing the frozen grant/control target-association source at manifest618290b14e90445189915c7515d6a224a5c202cda289ebb9d4ae0c8c9189da9b. No new heavy agent build is authorized.
