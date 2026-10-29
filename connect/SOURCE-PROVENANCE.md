# Source provenance

Public Connect checkpoint IDs recorded below predate a publication-only Git email
metadata rewrite. [PUBLIC-SOURCE-COMMITS.json](PUBLIC-SOURCE-COMMITS.json) maps
them to published commit IDs with identical trees. Names, dates and messages
are unchanged; parent references follow the mapped history. Existing private
Toolkit checkpoint IDs are unaffected. Selected binary, SDK and resource bytes
are unchanged.

This repository is a clean source export from the reviewed Meerkat Toolkit
checkpoint `557c492a78145e223afc5bfa6fcea3e247b8bf4d`, with explicit release
projections. It does not include the private repository's Git history, coordination
messages, captured account data, raw live-case evidence, local build archives or
credentials. The release audit records every included file and projected hash.

Ordinary source tests and their synthetic fixtures are retained. Test certificates,
fixture keys and fixture identifiers are test material, not provider credentials.
Vendor source and third-party notices retain their attribution and license scope.
The unpublished Core codegen source comes from exact Core 0.9.0 revision
`a00bafaf1d862fb41112ff1f30312b350ad05246`; its manifest is projected to the public
dependency layout without changing its implementation source.

## Unreleased follow-up source

### Ordinary 0.9.4 and deferred shared setup

The ordinary 0.9.4 branch is extracted from the public mixed draft
[`ffeb622f9d06a0190ba87eb2290d67909ee1dfe1`](https://github.com/lukacf/meerkat-connect/commit/ffeb622f9d06a0190ba87eb2290d67909ee1dfe1).
That original branch and its complete history remain intact. Shared multi-caller
setup is retained separately for 0.9.5; the ordinary branch excludes its workflow,
membership, launch/delivery and per-waiter execution implementation. Ordinary
helper custody, cancellation, recovery and result authorization remain included.

The sections below are retained historical provenance for the mixed source.
Statements that a checkpoint adds shared code do not mean that code is included
in the 0.9.4 branch. Earlier test results retain their original source and artifact
scope. See [follow-up scope](docs/releases/follow-up.md#release-separation) for
the current branch boundary and remaining release checks.

### Shared preparation and source-owner currentness (work in progress)

Private Toolkit checkpoint `ddc3dc863a270d5190db5237788c7a43ec724b16` contributes
86 reviewed source/test paths. The native development graph is now
[Core 95343786](https://github.com/lukacf/meerkat/commit/953437864bbb21b135661a0e61a09b5aa1f6ba01)
and [Meta f04125c4](https://github.com/lukacf/meerkat-meta/commit/f04125c45e732b3ef05aab2ac2a0f642fc1b08a1),
with the exact native-candidate lock and matching SDK/codegen source. The root
portable-library lock has a separate scope and remains unchanged.

The candidate adds exact shared preparation membership, retained native attempt
custody and source-owner currentness corrections. A canonical disposition read
waits for its collection mutex; poisoned, invalid and cancellation-pending state
still refuses access. Temporary runtime diagnostics are excluded.

Sixteen earlier phase/currentness/close/poison controls and a separate SQLite
owner-reopen/cancel/complete control pass. The latest five disposition controls
pass after the held-lock regression failed on the prior production code.
The earlier clean shared SDK case failed: both Original finds succeed, then an
unattributed helper submission reports `ControllerReadinessUnavailable`. Only
A-helper progresses before the two-helper barrier times out. Independent
cancellation and sibling survival are unreached. The in-test cleanup check and
outer owners settle; that clean run exports no individual native receipt bodies.
Outcome `5d0ef152` and independent review `c9cd2812` preserve this failed status.

This is a draft source checkpoint. Shared SDK completion, a single pre-spawn
planning lead with durable per-waiter context delivery, installed gateway/reset
consumption and remote restart remain open. Earlier successful installed journeys
retain their original source selections. Published 0.9.0 assets are unchanged.

### Durable workflow storage (work in progress)

Private Toolkit checkpoint `8aed9b48ac1d3aaf045d134cc88196f7ffe80c3b` contributes
five source paths for exact workflow terms and waiter membership in the existing
SetupStore and outbox. Concurrent joins create one launch plus separate delivery
obligations. Exact replay does not duplicate effects; changed terms refuse.
A join racing final departure cannot reopen a sealed group. After reopening, an
unacknowledged launch needs reconciliation and a late waiter gets a fresh pending
delivery rather than inheriting an earlier result.

All five focused SQLite controls pass in outcome `6c8e61ce`, independently
reviewed in `f65e7fbe`. At that checkpoint native planning-lead launch was
unwired; later admission results are recorded below. Protected context delivery
and per-waiter operation authority remain open.
The same run's temporary diagnostic identifies helper A's SDK admission refusal
as `PolicyChanged`, with matching prepared/native sessions. Helper B progresses,
but the two-helper barrier still times out. The SDK remains failed; cleanup's
in-test assertion and outer command joins are not individual native receipt
proof. The diagnostic is excluded from source. Published assets are unchanged.

### Helper admission after a policy change

Private Toolkit checkpoint `0d3b1a6a54c9091100f63e30408fd5dfa1acab78` contributes
two production paths and five native fixture paths. A helper submission may retry
once after exact same-session `PolicyChanged` nonadmission. It preserves the
actual input, source, event key and original deadline. Uncertain acceptance,
other refusal categories and further policy changes do not trigger another
submission.

The corrected prior-code comparison `08d1bdbe` reaches an actual native
policy-change refusal and fails at the intended missing-admission assertion.
The earlier comparison `858fda8b` failed a fixture prerequisite and is not counted
as a qualified negative. The repaired run `74a1d715` passes all six focused
controls: classification, same-input reprepare, persistent refusal, lost accepted
reply, contributor retirement and original expiry. Independent review `707e318b`
confirms those focused results and their native source/effect assertions.

The same run's SDK scenario remains failed. Both Original finds succeed; helper
A's selected check returns `operation_authorization_unavailable`. A separate
generic helper admission refusal has no member or precise reason attribution.
The barrier expires before independent cancellation or sibling survival. The
in-test cleanup assertion and command/transport joins pass, without exported
individual native process receipt bodies. These results qualify the helper
repair, not shared SDK completion. Published 0.9.0 assets are unchanged.

### Latest SDK diagnostic and first planning-lead candidate

The later diagnostic outcome `ff0c8532`, independently reviewed in `5c03b653`,
records a different SDK failure: A's first Find returns
`operation_authorization_unavailable`. It does not identify the cause. B reaches
the held installer, then the two-requester barrier expires. Cancellation and
sibling survival remain unreached. Both actual native attempts close with their
direct children reaped at exit 0; the Python owner is gone and the retained worker
count is zero. Temporary diagnostics were restored. These observations do not
replace the distinct earlier failed runs above or establish shared SDK success.

The planning-lead source candidate was initially isolated from the public
checkpoint. Its first compile failed (`9e3626e5`, review `183c1879`) before any
control ran. A two-path correction fixed a missing import and used SetupStore's
existing public load-and-validation contract. The retry (`20e23085`, review
`f43e1160`) compiles and passes five focused custody controls using real local
grant authority with synthetic admitted-source acknowledgements.

The retry's first actual native launch control fails its fresh-admission
assertion. The returned variant was not recorded, so no cause is assigned; the
remaining five launch controls did not run. The fixture's cleanup assertion and
outer command joins pass, without exported inner shutdown receipt bodies. These
results do not establish actual G admission, protected context delivery or
physical MCP cleanup. Private documentation checkpoint
`b0f631451172e9905f70e1588fa4d49d226378ed` records this failed outcome.

### Corrected planning-input launch

Private Toolkit checkpoint `ee0c39810f433fee1bbc7e850237f1cf988fe9bd`
contributes 15 source/test paths for retained planning input, exact source and
grant ownership, first-producer selection, and native admission/reconciliation.
The initial diagnostic compile failed with a moved-value error before any test
ran (`4a302e04`). Its corrected diagnostic (`cda335ae`, review `a024d058`)
identified the actual launch failure: the ordinary configured-helper binding
check rejected the independently issued planning root before native admission.

The correction uses a typed planning-binding path through the same persistence
and publication owner. It validates the actual retained planning source,
controller, grant lineage, deadline and native session, while preserving the
ordinary configured-helper checks. A negative control uses a different valid
issued root, rather than malformed metadata.

Outcome `985706c4`, independently reviewed in `a6943d55`, compiles and passes
all seven selected native launch controls. They cover one fresh admission and
sealed waiter delivery obligations, refusal of the distinct issued root,
reconciliation of expired in-flight admission without resubmission, final-waiter
departure before admission, cancellation before handoff, exact first-producer
selection, and admission from a live B when A is unavailable. The earlier five
custody passes remain separate; they were not repeated in this run.

All seven in-test native cleanup checks pass and command/transport owners join.
Individual inner shutdown receipt bodies were not exported, so this is not
independent physical-process cleanup proof. The fixture uses the existing P/A/G
session: it proves native planning-input admission, not a new planning agent or
conversation isolation. The later dedicated fixture below qualifies fresh
composition separately; protected A/B context delivery, per-waiter effects and the
complete shared SDK journey remain open.
The cancellation/store/recovery successor below was not included in that
checkpoint. Temporary diagnostics and native dependency changes are
excluded. Published 0.9.0 assets remain unchanged.

### Workflow departure and pending cleanup

Private Toolkit checkpoint `ad7691458b2ec0ee796757b39de19a07a2122988`
contributes 18 Rust and five SDK source/test paths. Conditional
join atomically checks source, acquisition and workflow state. Cancellation
records source cancellation before departing membership, including when the
caller has lost its local handle. Final departure retains one close obligation.
The same recovery worker can revisit it after the caller returns, fencing future
planning operations and reconciling the exact native input without cancelling an
ambient shared run or treating uncertain admission as absence.

Outcome `0a362761`, independently reviewed in `8e1b6fc8`, compiles and passes all
12 selected controls: two SQLite transition controls, one independent membership
and delivery control, and nine native close/cancellation controls. They include
a queued join after cancellation, late publication refusal, lost acknowledgments,
pending active/unknown input, worker recovery and cancellation after owner reopen.
All 740 selected source rows remain exact. All 15 command owners and outer
transport join; native in-test cleanup predicates pass. Inner physical shutdown
receipt bodies were not exported. This does not qualify active-input shutdown or
remote-store conformance.

The three earlier attempts failed before tests: isolated workspace development
dependencies (`5491d423`), a missing runtime extension import (`9909614f`) and a
non-Send mutex guard across an await (`2ae081b7`). The successful successor scopes
that guard before awaiting the drain. The scratch-only Cargo membership and
development-dependency overlay is excluded from repository adoption.

SDK outcome `fe639c2e`, independently reviewed in `3e0d1b53`, passes five Python
and five TypeScript controls plus TypeScript compilation. It qualifies decoding,
identity checks and late responses without replay. The new cancellation state
preserves the distinction between durable cancellation and cleanup still pending.
Matching SDK generations are required; older closed decoders reject it and
bootstrap v1 has no version negotiation. That checkpoint did not qualify fresh
dedicated composition or terminal-result/expiry departure; their later focused
results are recorded below. Protected waiter-context delivery and the complete
shared SDK journey remain open. Published 0.9.0 assets are unchanged.

### Bounded SDK feedback controls

Private Toolkit checkpoint `3356cad1577bc9fa877f4816af6ca98fe1198cff` contributes two test paths.
The scripted agent may make one fresh local Find proposal after exact pre-entry
authorization feedback, preserving the request and original deadline. It does
not replay an entered effect or retry uncertain helper/MCP work.

Outcome `5ad14d9a` passes the two scripted controls but fails the first native
test because its call snapshot precedes the existing terminal wait. After moving
that wait, outcome `3dca5919` passes both native controls: one exact refused
operation followed by a fresh proposal produces one acquisition and one succeeded
work-admission effect; persistent refusal produces two refused operations and
zero such effects. Complete native audits export 21 and 11 rows. Independent
review `f87efb79` joins these with the two earlier scripted passes.

The full SDK case in that same outcome remains failed. B's fresh Find succeeds,
then its helper check reports `other_error`; A's check succeeds, but the shared
barrier expires. The exact error and full-case audit were not exported, so no
cause or MCP leaf entry is inferred. Two provider cleanup-thread panics remain;
the cleanup-empty predicate passes but raw inner receipts are absent. Outer
owners join and all 741 selected source rows remain exact. The earlier compiler
failure `57003027` and prior native/SDK failures remain failed. This qualifies
four controlled feedback tests, not shared setup completion or product automatic
retry. Published 0.9.0 assets are unchanged.

### Fresh dedicated reservation and terminal recovery

This source update adds 24 Rust paths plus the host-composition guide, preserving
the distinct private/public guide projections. Source application `db040f67`
uses public reservation manifest `b59714e5`, the sorted policy-row correction
`146b3956` and Bind source-class correction `5a429e4a`, with the reviewed terminal
and recovery changes. Temporary SDK/helper diagnostics, scratch Cargo changes,
protected owner files and the external locator are excluded.

The initial finite-ten record remains seven PASS and three FAIL across retained
outcomes `dafa1595`, `005dec93`, `22bb5cfe` and `459f3a0f`. The seven passes cover
terminal paging, membership retirement, expiry and later acquisition for the same
A, acknowledged-result recovery, selected canonical reads and uncertain close.
The first partial FINAL listed nine through its failed group, while its immutable
plan contained ten and two entered; independent review `5cc2f5c4` preserves that
count correction. No earlier passing control was repeated.

After focused corrections, outcome `61f87bde` and independent review `2d886dda`
record two PASS: public dedicated reservation and Bind. The dedicated control
joins a same-lifetime omitted-participant negative fixture, then runs a separate
positive fixture with fresh, distinct A/P/G sessions. It exercises public
reservation/bootstrap and exact native input restrictions. The Bind control
reaches pending-owner withdrawal, Failed replay, distinct new-source and
cancellation assertions. In-test shutdown predicates and all command owners pass;
raw inner receipt bodies were not exported. Both controls reuse the exact compiled
artifacts, without another Cargo invocation.

That checkpoint used an internal planning-source key. The later public-locator
and second-input qualification below extends the supplied-terms scope without
claiming external SDK completion or treating a historical locator as permission.

Earlier SDK outcome `ecd54ae4`, independently reviewed in `4912b0d5`, remains FAIL.
Helper B receives same-session PolicyChanged with prior matched reprepare count
0, then PolicyUnavailable with count 1. A's check and B's first Find each record
only authorization_unavailable for the exact operation, with no Prepared/Entry;
B's distinct fresh Find succeeds. The 57 before/57 after audit rows are identical
snapshots, not 114 distinct operations. The prior `005dec93` failure retains its
39/71 snapshots and earlier scope; it did not identify this helper tuple.

One scripted-provider thread panic and the shared-worker barrier failure remain.
Cleanup returns without panic and its strict empty predicate passes. The decoded
Python receipt records zero workers and zero physical entries. Native construction
3 is Installed and construction 4 Cancelled/RouterClosed, both with emitted
Closed/Reaped receipt projections. Original receipt files were not separately
collected; their hashes and decoded projections are retained in the log. Command,
transport and cache owners join, and all three temporary diagnostic paths are
restored or removed. PolicyUnavailable's producer remains unassigned.

That affected selection is two PASS/one SDK FAIL with none unentered. Across
the original ten identities, nine have scoped passing observations and SDK remains
failed; this is not one new ten-control cohort. Full shared completion, online
member creation, active generation retirement, protected waiter-context delivery,
gateway/reset and a new release remain unqualified. Published 0.9.0 assets are
unchanged. See the [development inventory](docs/releases/follow-up.md#unreleased-development-progress)
and [host composition guide](docs/site/guides/host-composition.mdx).

### Public locator and admitted-source rebind

This qualified follow-on includes seven toolkit-mobkit Rust paths and the
host-composition guide. It excludes the unfinished SDK planning fixtures, their
dev-dependency/lock changes, temporary diagnostics and gateway work.

Outcome `aa0b5f94` (review `09075318`) passes two SQLite source-locator controls
and fresh dedicated composition using the public locator. It preserves exact
scope, aliases and cancelled history without new permission or store writes.
Outcome `79f98890` (review `1e432a4f`) passes three admitted-source rebind controls
and one actual second-G-input control. Healthy rebind/replay preserves the
publication stamp, new admission still publishes, and cancellation or observed
poison refuses. Actual second input/readback/replay retains G's association and
P contributor; foreign and cancelled-P inputs refuse. Expiry and waiter-context
disclosure are not covered. Old-production comparison `344bad83` (review
`1bac4c53`) fails the expected stamp assertion, then restores corrected source.

Earlier full SDK outcome `d3105a29` fails an obsolete approval-body assertion after
both Finds/checks succeed and both installs are emitted. Cleanup's strict empty
assertion passes: zero live Python workers and both installed processes
Closed/Reaped. The physical-entry count of one is historical. Later provider
panics remain recorded and cancellation/result assertions are unentered.
Planning outcome `a023d72c` passes corrected root issuance but stops at Unsettled
held bootstrap; the awaited nested result was not exported. Its existing cleanup
assertion passes.

Latest pair `7646af61` compiles but the full SDK case fails at A's catalog-load
authorization refusal and helper barrier, before its typed-approval block. The
generic readiness event remains unattributed. Exact-artifact planning continuation
`ade4c091` returns NotHeld for original-b, without an inner construction reason.
Independent reviews `01a8aef5` and `06b70670` confirm both strict cleanup assertions,
source restoration and recorded owner joins; raw inner receipts are absent.
Neither SDK case qualifies shared completion.

On Meta `a8ff89dae54f6e48d28f68aacc6078e11c684959` with the same Core,
focused outcome `785dc246` (independent review `bc9a3e59`) passes both gateway
cleanup controls. The planning case now passes bootstrap and parent admission,
then fails at both actual Find calls; their result bodies were discarded by the
assertion. Focused diagnostic `238bbef4` compiles and exposes `access_denied` for
both calls, with `Tool 'find_and_install' is not allowed by policy`. Its assertion
message change is restored afterward. All three original selectors entered, the
planning cleanup-empty assertion passes, and diagnostic process owners join.
This does not identify the exact policy producer or erase the earlier NotHeld
result.

Actual gateway binary outcome `c48ded63` (review `59188a34`) and staging pass with
binary `e102d6f7`, refreshed resources `7f7d8a04` and exact matching SDK sources.
Installed outcome `9aae37e0` stops at a missing launcher case before gateway entry.
After the parent-only hookup correction, `59484d81` reaches native readiness but
fails before model/reset work in Forge's retained-source reader. Exact remote
metadata `547f5411` records the three source copies as `0644`; unchanged-reader
controls `f4166260` refuse those modes and accept the same bytes at `0600`.
Third attempt `f6d5f8a5` uses fresh private copies and the same staged product.
Authentication, preparation and policy validation pass, but the `meta/reset` HTTP
request times out before any model request. A later helper-spawn log reports
selected activation unavailable; their relationship is under diagnosis. Its 143
saved references validate and recorded process owners join. Overall shutdown
remains refused with exit 1; joined processes and native cleanup flags do not
convert it into success. Review `cef06797` retains that distinction for the prior
attempt. No complete installed reset journey or new release is qualified.

Fourth installed attempt `ee9d0d13` reuses that exact stage, binary, resources and
SDKs, with only reset observation changed from a 10-second limit to a 75-second
I/O cap within the existing case deadline. The one request returns in 11.207
seconds with a new helper session, generation 1 and ready identity readback.
Seven numeric observations show continuing CPU and read activity; they do not
attribute all elapsed time to resource verification. The reset reply includes
`stale_member_cleanup_skipped_after_identity_reset`. In the selected Meta source
that warning is unconditional on successful reset, whose implementation calls
the successor-reset bridge. It does not establish failed predecessor retirement.
Actual Find is admitted,
but replacement setup MCP attachment returns `Unavailable` and Search reports
`total_matches: 0`. Four model requests occur before the schema-tool assertion
fails. The old guard's disposition is not exported, so neither cancellation nor
poison is assigned as the cause. Semantic shutdown still fails; process owners
join. All 142 saved references independently validate.

The Core owner conditionally confirmed completed disclosure at an R1 Find-fenced
durable transfer commit, with recipient-only scope, immutable payload validation
and settled-attempt reconciliation. Later G/P consumption need not import A/B as
contributors. The selected host policy already authorizes Find at native R1; its
compiled R2 consequence label is separate and unchanged. At this earlier checkpoint, transfer and delivery
remain isolated review candidates. Their first compile (`8421e8d7`) fails on an
unlinked test import and a test admission-state type mismatch; zero of three
selected controls enter. All 50 saved references independently validate and
owners join. Minimal test corrections use declared `futures::FutureExt` and
compare full `WorkItemAdmissionState` instead of the admission receipt type.
Corrected outcome `cc9f1205` compiles and passes all three selected transfer
controls. Its separate planning SDK control fails after B's host and canonical
acquisition admissions succeed and the same correlation re-enters the host.
That places the failure downstream; it does not prove initial native Find-policy
refusal. A's later setup-facts denial follows the primary panic. All four controls
enter, the planning cleanup assertion passes, temporary diagnostics restore and
recorded command/transport/cache owners join. Automatic delivery is untested at
that checkpoint.
The separate automatic-delivery candidate retains slow pre-dispatch observation
before polling can cancel it, distinguishes typed refusals from pending work and
skips never-entered transfers in recovery. These are source-review outcomes,
not runtime qualification. Later waiter effects still recheck authority.
These development results do not change published 0.9.0 assets or supply a new
released selection.

### Retained context and automatic observation

Private Toolkit checkpoint `586ccf3d7eefb39e8d8b789ec36d14967f313f7f` contributes
14 toolkit-mobkit source/test paths from the exact clean source compiled in
`881f9741`, plus `context_transfer: None` in the existing Connect SDK test host.
This preserves the test host's default-off behavior without adopting the unfinished
planning fixture chain. Cargo manifests, locks, temporary diagnostics and gateway
candidates are excluded.

The implementation retains a Find-fenced exact context commit and the same native
delivery attempt when a polling observer drops. Existing coordinator/recovery
owners advance that work; later G/P consumption and waiter effects keep their own
authority checks. The earlier three transfer controls pass. Outcome `881f9741`
adds a passing slow-advance control, then fails a reset fixture's factory-call
count after actual replacement attachment. Exact-artifact continuation `507c8d83`
passes the other five reset controls and fails the planning case at a generic
native turn-result assertion. The five dependent automatic SDK delivery controls
remain unentered. Independent reviews `2c9ff2bc` and `0afcec60` verify exact source,
artifact reuse, saved references and joined outer owners.

The planning fixture had two corrected inputs: actual compiled setup policy
revision and a request deadline within the existing workflow deadline. The newer
failure does not export its exact native outcome or establish a Find refusal
cause. The SDK cleanup-empty assertion passes before the primary panic resumes;
a provider panic during cleanup remains recorded. The first reset test awaited
cleanup but skipped its final assertion after the primary panic. Its separate
reviewed correction checks Core's pre-factory PriorGuardUnsettled/NotSpawned
receipt and runs cleanup assertion on every exit; its later passing rerun is
recorded below.

This is a local implementation checkpoint. Native end-to-end automatic delivery,
full shared setup, installed reset and cold-process recovery remain unqualified.
Cold recovery needs exact retained planning inputs and actual current authority,
not only stored workflow membership or a content hash. No published 0.9.0 asset
or released dependency selection changes.

### Initial preparation cancellation

Source candidates `23036f3a` and `09403bcd` contribute the local-round attempt
implementation and two test paths. The parent test module excludes the unrelated
gateway cleanup module from the execution workspace. Initial cancellation may
restore unused preparation only for the exact native NotSpawned disposition;
a normally closed initial child may use the existing successor offer. Each later
begin still validates registration, session, descriptor and full resources.
Abandoned, abnormal, failed and offered-round cancellation remain nonreusable.

The five earlier affected controls pass in `507c8d83`. The corrected sixth passes
in `5111246e`, including actual pre-factory refusal, stale-session refusal,
successful successor attachment and strict cleanup. Independent review
`3e615867` validates all 72 references and joined owners. No installed reset
success is claimed.

The same pair's planning case passes Find, G identity and all turn checks, then
fails a live-ledger count of zero versus one. Source review confirms that Core
archives terminal rows from that ledger. A separate unqualified fixture correction
will count exact durable input identities and preserve replay checks. The earlier
turn failure and cleanup provider panic did not recur; they remain unresolved.
Temporary diagnostics, dependency changes and published 0.9.0 assets are excluded.

### r5 development observations

Development-only Core `ab4d62274a8dd759842de2a25b7ac97a77d358ea` and Meta
`67fa040723ec60568ded5bac3ed7ea82e459db6c` compile with the prior 556-package and
complete feature map unchanged. Outcome `4efac7a4`, independently reviewed in
`0867cc23`, fails the planning turn-settlement assertion with the displayed
reason "operation unavailable under current authorization" before the durable-input
checks. Core shares that text across ReprepareRequired, Denied and MalformedFacts.
The subtype and producer are lost in Meta's string outcome, and the result does
not prove the targeted r5 correction ran.

Outcome `77f562cf`, reviewed in `7612722e`, reuses the exact same compiled binary.
Both original Find calls, helper catalog loads, three exact schemas per helper
and selected checks pass. Two independent approvals and one held physical
preparation are observed. The SDK fixture incorrectly expects a ToolCall approval
owner and raw arguments; production uses the exact helper Session owner and the
reviewed proposal with tool/call identity. Its separate install error is still
unexplained. Both SDK cleanup-empty assertions pass, all temporary diagnostics
restore and recorded owners join. Shared completion is not qualified.

Later focused outcome `f1c8cfee` compiles and runs all eight selected cases on
the same r5 graph. All five explicit planning-source controls (`a63cd576` plus
the test-only UUID correction `879bc43a`) and both delivery-claim controls
(`c24818d6`) pass. The earlier parent-source compile `dd9bab43` entered no tests.
The SDK executable is now `68c5496f`, despite an unchanged filename suffix.

With corrected approval assertions, the full SDK passes exact helper Session
ownership, A cancellation and B install/job/report tool results. It times out on
the combined completion, B readback and original-task continuation condition at
`native_case.rs:1679`; the raw log does not isolate the missing condition. No
provider panic or install refusal appears. Strict cleanup-empty passes and all
recorded owners join. Shared completion and process restart remain unqualified.
The local planning-source checkpoint adopts the 11 paths from `a63cd576` with
`879bc43a`, as checked against the executed source.

The subsequent outcome `54b6c948` compiles the same r5 graph and passes all four
actual-store delivery claim/retry controls. Independent outcome review
`550f3a4a` verifies all 99 saved references and exact source restoration;
independent source review remains `0277c259`. This checkpoint adopts the exact two
source paths from `c24818d6` plus `d15e4ed2`. Only a definite pre-claim revision
conflict preserves the earlier successful NotApplied witness; foreign or older
attempts, ambiguous claim responses and generic Pending/RetrySafe state cannot
manufacture that evidence. The final delivery source SHA-256 is
`fa89f7f8f47c316bae358bf4f34081de40e9ae4682d7f99a5fed366ad6a3b5e2`,
and its test source is
`3824645b3375641cf7a8380387affa9a26f148ad758a6245765cbd3d29bfc139`.

That run's full SDK case still fails after 50.17 seconds. Its bounded retained
projection shows preparation completion, job succeeded and one original-task
continuation, but no B delivery acknowledgment. The native delegate returned
authorized once and admission Timeout once; no reconciliation return was
recorded. The report is unavailable at revision 2. The diagnostic does not add
a fresh timeout-time read or establish nonentry from an absent return. The SDK
executable is `3ac3d0ee`. Strict cleanup-empty passes, all recorded owners join,
and both temporary SDK diagnostic paths restore exactly. Planning, idle G and
installed reset are not selected. SDK fixture changes, development dependencies
and temporary diagnostics remain excluded from the local source checkpoint.
Historical positive acceptance stays separate from new operation authority.
The Core owner confirms that the exact historical governed-input lookup is absent
and has assigned the additive 0.9.4 query to Core for a reviewed development
pair. No consumable implementation is available yet. Cold recovery needs
that supported lookup and integration with the existing recovery worker. Missing
or error evidence remains pending. No private Core codec or second index is added.
These observations do not alter published 0.9.0 assets or the adopted dependency
selection. CI-triggering publication remains held for the Core/Meta lead.

### Selected workflow source custody

The exact G/A composition `59917d99` plus `59da025c` contributes 13 source/test
paths, including fixture corrections `37fc7d63` and `25f490b7`. The former rearms
the one-shot observation between P and A. The latter supplies the actual session
service through `MobBootstrapSpec.controller_pin_source` and checks exact runtime
and source-owner identity. Bridge-only configuration did not populate that field.

Outcome `e3ae16c4acdccc8021200545b39a20f08c817a719274d450b732d700fa3013a9`
passes compilation and all seven exact native controls on Core `ab4d6227` / Meta
`67fa0407`, using executable `d3b4308b`. Independent review `7c59a245` checks all
104 saved references, source bytes/modes, raw markers and joined command,
transport and cache owners. Strict in-test close assertions pass; no separate
raw inner receipts are exported. Prior failures `88cb3452`, `3aeabc15` and
`a1d4199c` remain part of the evidence.

The tests cover selected G/A historical custody after ordinary native resume,
exact input/history preservation, no model calls, a restricted G gate, no new
caller-to-G effect authority, refusal/no-retrofit cases and affected parent/fresh
planning behavior. This local source checkpoint does not include restored
workflow interest, cold context-transfer reconstruction, SDK fixture changes,
temporary diagnostics or a dependency change. It does not qualify the complete
shared SDK journey or alter published 0.9.0 assets.

### Explicit Node build plans

Private Toolkit checkpoint `07172f52e03b4a3c0ec9f383651bbfde4ebc2364` contributes 19 source/test
paths and the selected Node build, setup and host-composition documentation.
The agent supplies literal commands and input/output mappings; the host supplies
an exact toolchain profile. Existing installer, software review, job, cancellation
and readback owners retain their responsibilities. No Rust API or dependency-graph
change is needed. npm lifecycle scripts remain disabled.

Thirty-seven distinct focused controls passed across the retained attempts.
The original integrated attempts failed first on an omitted baseline license,
then a synthetic macOS fixture's Linux admission, and finally a missing archived
GitHub research module. Only the affected remainder reran; the production source
was unchanged. The last missing-member cause was established from source; its
saved child refusal was generic. The fixture now includes the actual module and
keeps its target substitution local to the test.

A separate actual Controller/installer/compiler case passed on Linux x64/glibc
2.39 with Node 24.19.0 (ABI 137). It generated a 16,056-byte addon, then a fresh
Node consumer loaded it and returned 42 for 19 + 23. Original stdout and parsed
receipt bindings agree with the ready file; replay retains the same job, readback
preserves bytes and the lifecycle-script marker stays absent. Controller and
command owners join normally. Direct-child waits and ELF shape are assertions in
the passed fixture, without a separate exported consumer PID or addon binary.

The production source capsule is `9f78fc8b`, with the final test-only starter
fixture `0bfaed6b`. Actual result `71a56c28` and independent review `3a7484b2`
record the compiler case; final starter result `506f8491` completes the focused
set. That direct-consumer result covers one local source package and target.
The later installed MCP slice below has its own matching product and evidence;
broad build compatibility and a hermetic toolchain remain unqualified. Published
0.9.0 assets are unchanged.

### Installed source-built MCP operation

Private Toolkit checkpoint `ffb04f81068746a93783a5b587d713c52d249af1` contributes
the supplied fixture, installed consumer and scoped qualification records.

On production source `4288d20a7613f03b3a4048446e7087bc8b4f30cd`, a matching rebuilt
gateway `7b14f790` and resources `b6585e9c` passed one installed supplied-package
case on Core `efd313d4` / Meta `669f54fa`. The ten-path test-only capsule
`3ce80944` adds the source fixture, consumer leaf and existing-verifier dispatch;
source review `2b39a16c` binds those bytes. The fixture is supplied user input,
not a bundled product resource. Native compilation retains warnings.

The real helper read host planning IDs, inspected exact schema/package bytes,
checked the explicit build, installed once and reconciled. The protected software
review retained the exact checked build payload before approval. A separate
service-access decision enabled the resumed Original to search/load `source_add`
and call it exactly once with left 19 and right 23. The compiled addon returned
`{"sum": 42}`; actual native call/result and Original/helper completion frames
are retained. The 16,056-byte addon `7ac64d9e` matches the original installer reply
and ready receipt; retained input bytes are unchanged and lifecycle scripts did
not run. No JavaScript arithmetic fallback supplies the result.

Result `6767debb` records two displayed service/catalog leaf revokes with
unavailable readbacks, certain login retirement, natural gateway exit 0 and
joined provider/readers/observed children/listener/outer owners. Independent
outcome review `d6c566f9` accepts this scoped result. Original final `5a0d4d8e` stays failed
because its offline summary compared `passed` with the checker's `PASS`; saved-only
correction `ed2550c0` used the existing successful execution and no runtime replay.

This is a controlled model schedule, supplied synthetic package and protected
Console API case on Linux x64/glibc 2.39, Node 24.19.0 / ABI 137. It establishes
installed compiled MCP use, not autonomous discovery, a real-provider strategy,
Browser rendering, reset/restart, authenticated/shared setup or a complete dynamic
toolchain closure. No private submitted-input receipt or universal descendant
closure is claimed; separately issued setup children are outside the two displayed
revokes. The existing exact review payload was checked, not a new panel summary.
Published 0.9.0 assets and broader requirement statuses are unchanged.

### Console source-build summary

Private Toolkit checkpoint `d30e7dc3970acb25a7b01ed30ba8e45c75b1ab66` makes the ordinary build
approval show the target, ordered literal command tokens and relative file
destinations to add or replace. Full toolchain/input hashes and limits remain in
Exact review details; protected decisions are unchanged. Three focused DOM
controls pass, including escaping, argument boundaries, a 256-input summary
bound and retained software-approval behavior. Result `919cadf5` and independent
review `f64956b5` bind the selected source and joined Node owner. These checks use
Node 25.9.0/jsdom 26.1.0. The earlier installed product did not include this UI
change; no Browser rendering or interaction is claimed.

### Selected installer cancellation

Private Toolkit checkpoint `a6cab1e3e059fb66cae22a90837f890fcf2a4bab` contributes the
following four source paths: the selected-job backend, the pinned installer and
two focused test files.

The selected-installer cancellation repair changes two production files and adds two test files. Its exact 20-control run passed with zero skips on Python 3.11.16 / MCP 2.2.0 (13.657 seconds): 14 new and six retained controls. Ten new controls use synthetic handles; real inert Python controls separately assert the installer's inner wait before selected-job settlement and outer-only cleanup after the 10-second grace expires. The inner wait is an assertion within the passed control, not a separately exported receipt. Selected source bytes remained unchanged; the run recorded joined outer owners and no remaining test threads. Partial output/replay semantics remain intact. No npm/compiler/package, arbitrary-descendant, MCP-transport or separate readback-validator qualification is added.


Source manifest `80ef7484`, final result `9a56eef2` and independent outcome
review `2ad3cd4a` bind the exact controls and selected source. The change preserves
the existing dependency graph. Published 0.9.0 assets remain unchanged.

### Retained local setup process

Private Toolkit checkpoint `940d5b679876a82a55fb04384f095c3415f73391` contributes three source paths:
the local-round exports, the retained owner/guard implementation and its focused
tests. The protocol child stays in the original `LocalRoundOwner` across errors.
`OwnedLocalRoundGuard` adds a move-owned asynchronous form; the existing borrowed
CLI API remains compatible. Borrowed guard Drop requests cancellation and leaves
the child handle in the owner for an explicit join.

Seven exact controls pass on public baseline `d0cfd48d` with unchanged Core
`efd313d4`, Meta `669f54fa` and native lock `3afe2492`. Six exercise actual
synthetic protocol children; one checks the bounded Ready parser. The process
controls require `console` and an explicit absolute `TOOLKIT_SHARED_TEST_PYTHON`,
and use only that interpreter's standard library. Result `e80d43d1` and independent
review `b225e641` bind source, executed binary and joined command owners. The
initial run compiled but retained six pre-child fixture failures; its corrected
private-directory fixture preserves the production verifier. Compile retains 55
warning headers.

This does not qualify production Python, native MCP, gateway/reset integration,
shared SDK use or cleanup after dropping an entire owner/runtime. Callers retain
begin/finish futures and explicitly retry cleanup after an OS wait failure. The
current CLI has no persistent-wait-error retry loop. Published 0.9.0 assets and
requirement statuses are unchanged.

### Warm public reacquisition

Development source adds an optional setup-enabled public review to the existing
configured host path. The continued task receives fresh setup children and its
own reviewed service grants before first binding. Its catalog scope explicitly
covers both named agents across both named sessions under the same audience and
task deadline. A later review can select an exact earlier checked job plus a new
job without reinstalling the earlier job, while issuing all fresh grants. Every
required native attachment must be Ready before continuation. Setup revocation
after admission preserves independent business grants.

R6 source `7a411015`, result `7d5d3de7` and independent review `8a79bc8e` join
28 distinct passing Rust controls and 49 unchanged JavaScript controls on Core
`efd313d4` / Meta `669f54fa`. Native A/helper/B/helper/C composition is distinct
from the checked-owner reuse control, whose package replies are mechanical.
The controls retain exact first-admission currentness, independent later target
checks, the reviewed 2x2 catalog scope, canonical partial-issuance observations,
stale-offer refusal, legacy preparation and wire compatibility. The final native
command and all transport owners joined; compilation retains 31 plain warning
headers and zero JSON compiler-warning rows.

Failed R1-R5 attempts remain retained: private type access, unsorted target pairs
and reconstructed borrowed authority, retained fixture dispatcher lifetime and a
bootstrap-only policy comparison, incorrect history/context expectations, then
using a current-turn witness for historical helper turns. The final correction
uses exact finalized input receipts and a settled-row reload. Authority and
revocation assertions and deadlines remain unchanged; these failures are not
reclassified as passing executions.

The existing host methods and new version boundaries are documented in
[host composition](docs/site/guides/host-composition.mdx#acquire-another-public-service-through-the-same-live-helper-development).
Forge source note `81fea62d` confirms the normal optional catalog selection;
it is not installed warm acceptance. Legacy singleton and v1 bytes remain exact;
the setup-enabled v2 and catalog-v2 records require matching readers. V43's
installed multi-service result remains separate. Installed repeat acceptance
is recorded below; Browser, helper-reset, authenticated-batch, performance and
release results remain outside this checkpoint.

Current-head composition `c897b204` preserves 33 warm afterimages and merges
only the shared test file onto adopted job-wait source `d42e093d`. Result
`5747e1d1` passes the exact bounded-wait guard and checked-job reuse control, with
source/lock equality and all owners joined. These two composition checks do not
increase the 28 distinct warm Rust count or repeat native/installed acceptance.
Compilation retains 55 plain warning headers and zero JSON compiler-warning rows.

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
This closes the installed compatibility regression. The following R9 case
separately qualifies installed repeat acquisition.

### Installed repeat acquisition

R9 completes the controlled installed A/helper/B/same-helper/C path on unchanged
production `3ecf0d5a`, gateway `80f956a8`, Core `efd313d4` and Meta
`669f54fa` with their retained matching SDK/resource inputs. B uses Hacker News
before requesting Wikipedia. The same helper installs Y once and reselects X's
checked job without reinstalling X. C receives fresh complete-set grants, uses
both integrations and answers in the same Original session. Native Console
events prove three completed Original runs and two helper runs.

Exactly five displayed service/catalog leaves are revoked with exact unavailable
readbacks. Login is fenced and drained, native shutdown acknowledges runtime
and host cleanup, and readers, providers, observed children, listeners and outer
owners join without escalation. One First-page action obtains a fresh access
offer without repeating a protected request. Setup children and persistent
policy are not included in those revokes.

Final aggregate `017a5eb0`, source review `6810c12c`, mechanics review
`79e7bbf8` and outcome review `c232d0bd` bind the exact case. The public verifier
and its 36 portable verifier controls are retained. Nine saved-artifact controls
separately validate the runner's observations. The model schedule is controlled;
this case does not prove autonomous strategy or Browser interaction. No native
controller-recovery notice occurred, so the recovery branch remains qualified
by source and synthetic controls only. Private native input receipts were not
requested. No helper-reset, restart, authenticated-batch, shared-setup,
performance, release or broader requirement closure is claimed.

R7's parser failure after C answered and R8's HTTP500/cancelled C caused by its
fixture's recovery-message rejection remain failed. R9 corrects those verifier
assumptions without weakening product authority or the existing success and
cleanup assertions. Product and native binaries are unchanged.

### V44 initial setup-enabled batch

V44 uses public `3ecf0d5a`, gateway `80f956a8` and the unchanged Core `efd313d4` /
Meta `669f54fa` graph in a fresh installation of the source-preserved application.
Actual prepared output confirms the helper catalog issuance, equal audiences and
distinct executors. One question produces two software decisions, one setup-v2
complete-set access decision and an automatic Original answer using Hacker News,
Wikipedia background and a Hacker News refresh. Both integrations were selected
in the first acquisition; V44 alone does not establish B-to-C or retained-X reuse.

Six tool errors recover automatically, including catalog refusals, busy help and
the exact reconciliation, metadata and member corrections. The initial refresh
refusal before builder entry also remains recorded. Saved results support the
requested answer fields; only those fields are unchanged on refresh, while a raw
comment-ID field changes. The Wikipedia background does not establish the exact
model release or official status, as the answer states.

Exactly three displayed service/catalog grants are revoked, logout returns 303,
normal Stop returns 200 and all owned processes, forwards and listeners close.
The separately approved persistent policy remains until superseded. Source-owned
setup children are not covered by those three revokes; no child-journal read or
grant-revocation sweep is claimed. Final aggregate `07b6e68c`, independent review
`0649ebf3`, continuation review `c8f7eec7` and cleanup source trace `32f2509d` bind
the 116 verified references. This is controlled Console API task completion,
without a Browser, performance, release or broader requirement-closure claim.

### Bounded setup-job observation

Development source adds optional integer `wait_ms` from 0 to 10000 to the existing
`setup_job` tool. Omitted or zero preserves immediate snapshots. Positive waits
re-read the exact job through the existing owner and hold no state lock across
the timer. Cancelling the observation joins any entered read but leaves the
installation running; explicit stop remains separate. Terminal observation is
not readiness or worker-cleanup evidence.

Source manifest `91d643b6`, final result `9fc144ba` and independent review
`55be2e29` join eight Python and four Rust passing controls on the unchanged Core
`efd313d4` / Meta `669f54fa` graph. Exact job/session/realm/epoch checks remain;
stop/reconcile retain their argument shapes. All command owners joined and
compiler warnings remain recorded. V43's saved trace contains 34 immediate job
reads. V44 records seven actual `wait_ms: 10000` call/result pairs during its
completed initial batch. Different work, operator review, polling and unmeasured
shared-host contention prevent a comparable latency claim. This establishes
usage, without qualifying performance, PLAT-030, shared-peer native isolation,
Browser use or a new release.

### Public-service batch checkpoint

Development source adds one reviewed set of 2-8 public MCP services to the existing
acquisition path. Checked jobs supply exact selected exports and operations; one
actual approval covers the set, catalog and continuation. Each member retains its
own parent/child grants and native attachment. All required members must be usable
before one Original continuation, and native admission rechecks the exact set.
Singleton wire bytes and independent member revocation remain covered. The
Core `efd313d4` / Meta `669f54fa` graph is unchanged. Exact source checkpoint
`21c967b9` contains 53 batch paths and the two separately qualified UI afterimages.

Final aggregate `bcafe131` joins 29 distinct passing Rust controls: 21 focused
(six codec, eight public-parser, five operator-approval and two partial-compensation)
and eight native (four batch and four singleton regressions). Repeated executions
are not added to this count. The actual batch controls cover both attachments
before one Original, exact member revocation with sibling/catalog use preserved,
queued admission refusal and second-attachment failure. Historical source-custody
reconstruction occurs after the admitted turn settles; it does not establish
process restart or a new operation after restoration. Captured contributor checks
do not establish later C admission. Attachment-observation retirement remains
distinct from physical child cleanup at normal fixture shutdown.

Both earlier native attempts stay failed: one hit its 60-second fixture deadline
while awaiting restoration across its own held turn; the next misclassified the
intentionally shared public refusal output and exhausted its bounded choices.
The passing fixture checks the exact public refusal after known member revocation,
zero entries to that server and actual sibling/catalog success. It does not infer
the internal refusal kind. Production guards and deadlines are unchanged, all
owned test/transport processes joined, and compiler warnings remain retained.

The separate review renderer preserves the status-read deadline and passes all
48 JavaScript controls; the exact prior renderer passes 46/48, with two meaningful
visible-detail failures. Independent UI review is `2c43cb33`; final panel SHA-256
is `7af2b61109a62a98727fdd573df5d1fab4e2df4f9882482fb8dc567a7358ff33`.
This is focused source/component evidence, not Browser interaction, CI or
released packaging. The separate V41 ordinary case used exact public `4fb67218`
and the same native graph. Its initial provider-route startup failure remains
recorded; normal model correction, Build and fresh Install then reached startup.
One question produced one acquisition and two successful software installs, but
no receipt reconciliation, access review, service grant, business call or answer.
Three public requests returned generic unavailability. The existing checked-owner
source identifies an unconfirmed matched receipt first and missing post-check
library metadata next; the visible errors did not disclose those reasons.
Final receipt `929ba6fa` and independent review `6264976c` retain the failed result
and confirmed cleanup. Normal Stop returned 200; application processes were
absent before the exact host received SIGTERM, and all owner sessions and
forwards joined. Feedback receipt `bcb8a30b` and review `8186217c` record four focused passing controls
and the unchanged correction regression failing at runtime when only the old
generic mapping is restored. These are actual checked-owner/software-review calls
with synthetic package replies; V41 is not reclassified by that patch.

### V43 ordinary multi-service completion

V43 uses public `be0df64cf94c54c1462961131bfa961999fe8071` and gateway
`40ab034b8445a96d6494499cad7f50902b0295b34b5594680915ff5a194f6122`, with
the same Core `efd313d4` / Meta `669f54fa` graph. The production installed
success/corruption pair passes without test-support features. Normal Forge
`3b24a887` refresh preserves the accepted neutral application's authored source;
Build `7ca94c8f-0282-419d-b3a5-a9f5fd3dda99`, source `95b8a68f`, installs into
a new destination and a fresh native namespace. No prior installed state or task
history is copied. Actual Original/helper bindings are independently checked.

One business question produces one acquisition of agent-selected Hacker News and
English Wikipedia jobs. Two software decisions and one exact complete-set access
decision precede two attachments, exact catalog acceptance, four successful
business reads and one automatic Original answer. Native all-member readiness
precedes continuation; public attachment-add events are not direct Ready receipts.
The trace retains actionable receipt, metadata and destination corrections.
Independent saved-result comparison verifies factual correspondence while
retaining the explicitly speculative story-2 connection and excess answer length.

Final aggregate `1cfbb80f` and independent review `140b773b` verify the ordinary
result and cleanup, including all 100 direct evidence references. Three exact
task-grant revocations return 200 with unavailable/nonrevocable readbacks; logout
returns 303 and one normal Stop returns 200. Application processes and listeners
are absent before the owned host receives SIGTERM; its child returns -15 and
supervisor joins 241. Both forward owners join 0, with no remaining owned process,
group or listener. Files and histories are preserved. The separately approved
persistent policy revision 2 remains until superseded.

This joins V40's outside-catalog case and the exact-set/native controls to close
SETUP-002's four proofs on the declared development profiles. It is controlled
Console API evidence, not Browser, authenticated batch or warm reacquisition
acceptance. V42 aggregate `1eb47b1a` retains its failed, uninstalled Build: generated
callbacks and a second report ledger led acceptance to request new protected
policy grants, which were not added. V41 and V42 remain failed predecessors.
Saved V43 local question-to-observed-answer time is 502.182 seconds including
review/polling; cross-host event timing has no measured clock-offset correction.
Shared-host compile overlap and clock boundaries prevent a performance guarantee
or PLAT-030 closure. Published 0.9.0 assets remain unchanged.

### Agent Plugins external-client checkpoint

One independently reviewed development case exports Microsoft's
[`microsoft-docs` 0.3.1](https://github.com/MicrosoftDocs/mcp/tree/f8ffde185dfd232dbf5d187c22ce299eadc3d583)
and uses unchanged [OpenHands SDK 1.49.2](https://github.com/OpenHands/software-agent-sdk/tree/d128a786ee2ee570eb23ff5862ec148b43cfad0b)
through its normal loader/runtime. The Linux exporter uses public `1779df8f`
plus one case-only public-API example; all 127 selected portable package/store/graph
paths match adopted public `070d3469`. The client runs on macOS with Python
3.12.13 and the pinned SDK dependency lock
`334b82b8688861bfa3f051f3cfca31ed5585101599212ff76399865117a29f18`.

All eight original donor files, including the CC-BY-4.0 notice, are retained
byte-identically. Two explicit canonical JSON additions select Agent Plugins
metadata and map `http` to `streamable-http` without changing the endpoint.
Staging is removed before export/use; all ten files are inspected. The actual
client loads three skills and calls `microsoft_docs_fetch` once, returning a
substantive 2,089-byte Azure Blob Storage overview. Result body SHA-256 is
`21f40ae2aa0780d7dc046b27993e5c4f427d459b4bf3a2dd2352b47482fa6dcd`.
Build/export and client exit naturally with observed owners joined; the SDK's
background GET-stream reconnect is not a repeated tool invocation. Final receipt
`ee02bead` and independent review `55496809` retain exact evidence.

Both earlier full-pack UMT cases at donor `b6eb997c` remain failed with zero tool
calls: unchanged `npx` execution encounters a bin without a shebang, and the
explicit Node launch encounters a missing published core entrypoint. Their
receipts are `875b385c` and `1585fddc`. No donor/package or SDK fix is hidden by the
Microsoft result. This qualifies one format-adapted pack/client operation, not
unchanged canonical donor format, immutable remote service code, complete-page or
`structuredContent` fidelity, remote session deletion acknowledgement, installed
Connect host adoption or a new release. A separate four-proof audit (`7925c86c`)
and canonical-loader addendum (`ed8aa088`) close PKG-007 for the declared canonical
Agent Plugins 1.0 skill/MCP subset, joining current ownership/refusal paths to
retained execution. Earlier evidence scopes and failures remain unchanged. D3
and PKG-006 stay open. See the
[portability reference](docs/site/reference/portability.mdx#qualified-external-client-example).

### Console status-read deadline

The development panel now applies one 30-second deadline across status response
headers and JSON body, aborting only the owned observation. It uses the existing
unavailable/First-page recovery and excludes late results. Protected action
requests and their polling fence are unchanged; timeout does not cancel setup or
establish any action outcome. Source packet `e0cb0da2` changes only the panel and
its existing JavaScript tests; panel SHA-256 is
`d400884c21408dad387c98e224ed08555996c8b69650fe5d3d00def29ec7ce2e`.

On Node 25.9.0, all 45 component controls pass, including the 40 earlier controls.
The same fixtures against the exact prior panel retain three behavioral failures.
Independent source/component review is `712b916f`. A separate unchanged-timer,
real-fetch loopback check expires held headers/body at 30.006/30.004 seconds,
observes both held sockets close before cleanup, and completes a fresh read in
6.843 milliseconds. Receipt `54e4e47a` records natural exit, joined servers/sockets
and absent process/group/listeners; independent outcome review is `9f8961aa`.
This is component/Node transport evidence, not Browser interaction, authentication,
native effects, installed packaging,
CI, complete UX-002 coverage or a release. Requirement status and 0.9.0 assets
remain unchanged.

### Native acquisition and recovery checkpoints

V40 completed the controlled Console API business journey using exact adopted
public `1779df8f6d95f34e17fd6b24c944d7d1a2ac69c7` (production `5c00e496`) and
ordinary Forge `3b24a887`, without overlays or a recovery selection. One original
question led to outside-catalog discovery, protected software/access decisions,
installation, attachment and automatic same-Original reads and answer. There were
no operator follow-ups, direct business calls or manual wakes. Independent
recomputation confirms EUR 2,900 to EUR 4,600 overdue, the EUR 1,700 increase and
Linden Works' EUR 1,300 contribution, including strict cutoff handling.
The task grant was revoked with unavailable/nonrevocable readback, logout returned
303 and the protected local session file is absent. One normal Stop returned
200. Application/gateway/supervisor absence preceded owned Forge-host SIGTERM;
all six relevant sessions and both forwards joined, with owned listeners absent.
Ledger and three sidecar identities were unchanged. Final independent review
`8bc7e8b1` confirms the answer and cleanup; physical receipt is `f270ccf6`.
Observed queries were reads; this is not independent write-prevention,
Browser/UI, helper-successor or installed recovery qualification.

V40 also qualifies one real native-addon MCP slice: `@database-mcp/sqlite` 0.4.2
with `better-sqlite3` 12.11.1 on Linux x64/glibc 2.39, Node 24.19.0 ABI 137.
Independent review `05ef0cb7` verified the 135-file acquisition inventory and joined the exact installed
addon, primary entrypoint and runtime to the already recorded adapter calls and answer, without another business probe.
This does not establish other targets, compilation-dependent addons or general
write prevention.


The retained approval inventory, protected Console adapter and independent local
SDK/inherited-terminal adapter compose 32 exact source/test afterimages in manifest
`ac6b1f76c538250ec278a9a83fca7281564b0ba427cd6af01780eaa6e8b0919a`.
The native Core `efd313d4` / Meta `669f54fa` graph is unchanged. Existing canonical
records, the source writer and actual native authority own recovery; there is no
second ledger, automatic startup revoke or Original re-admission. Published
sources and other approvals are preserved. Missing-parent and partial effects
remain unresolved, with policy observation and login uncertainty kept separate.
The [host guide](docs/site/guides/host-composition.mdx#recover-unpublished-service-approvals)
shows explicit Console and local SDK selection. There is no standalone realm recovery command.

The final affected eight-control run passes compile/list and all eight exact
verifier/recovery tests. It covers actual normal Console admission/completion,
retired and mismatched login, invalid signature/issuer, exact current operator
and administrative/member authority, and configured remote-operator refusal.
The preceding six-control run passed the four count/ownership/terminal repairs;
its two HTTP diagnostics returned the finite stale-durable-alias refusal. The
fixture correction uses native `restore_flow`, real status/session bindings and
actual completion instead of a raw member plus an invented runtime alias.
The original negative assertions and deadlines remain. `O_NOCTTY` prevents an
inherited PTY reopen from acquiring a controlling terminal, with a separate
session-leader regression. Retained unaffected controls and the repaired
40-case JavaScript suite remain applicable; this is not a repeated full-workspace
or CI run. The reviewed selector map joins 26 distinct passing Rust controls,
using the latest applicable exact result, not a sum of overlapping cohorts or
one final run of all 26. Final execution is
`83cd15e3dd8a052017205da4a0393fbfe7bc3689e107e29fbe1f88f2398489d8`; independent review is
`c65727b8c6450d8bcc1dce89c2a073802095a78a6e4d15ebde6b59a69b8c3cbb`; selector map is
`6b9696a91d4a4a4e6f26b6f1f164cb7cfa9a326727ab3a2000fe782e3dba7bfa`.
All command/transport owners joined. The final compile retains 27 rendered
warning lines.

The validation history remains intact: 39/40 UI before the same-login retirement
repair; five typed-ID compile errors; an 11/13 run before genuine Console source
admission and permitted test issuer; a typed terminal-error compile failure;
11/16 before count, fixture ownership, terminal and HTTP diagnosis; and two
private error-path compile errors before the public enum-path correction.
No assertion or production authority fence was weakened. Execution warnings and
failed runs are retained, without a warning-free claim. Detailed earlier source
identities remain in the [prior provenance checkpoint](https://github.com/lukacf/meerkat-connect/blob/740922ab8fbd833a9d2e3036f158bd6a91831af0/SOURCE-PROVENANCE.md) and execution records.

A separate ordinary development Rust SDK consumer now reopens the same installed
host using adopted public `070d3469`, the unchanged Core/Meta graph and one explicit
local-operator recovery selection. A fixture process prepares actual canonical
A/B approvals and fully closes before consumer startup; A is published and B is
deliberately unpublished. Public bootstrap/host APIs bind the restored owners,
without starting agents or declaring Ready. One displayed-row recovery action
appends exactly B's parent and child revocations. A, source-table rows and native
input history remain unchanged; all process/IPC/service owners close naturally.

The first producer run failed `AdmissionConflict` before consumer launch because
it admitted the same acquisition twice with different timestamps. A test-only
repair rereads the existing current acquisition. The same consumer artifact
`b0917377` and original 60-second deadline pass without retry. Final receipt is
`cf153b3e8d16f7f7e2287b13e087fa94b5ca175c7de508649aa8332ed0e1fe2c`;
independent outcome review is `dce04004eebc9741301fd9791d4a49c16c4a07ce2f3b782b36acfb5556f65fb1`.
Temporary workspace/member/lock scaffolding remains verification-only. This
qualifies public pre-agent SDK owner restoration/recovery, not a released
package, stock gateway, inherited terminal, Ready or actual process loss.
V40's public `1779df8f` package still excludes this implementation. Published
release assets are unchanged.

The official hidden in-app Browser fixture attempt stopped at
`Browser is not available: iab`, before creating a tab or performing any read,
action or effect. Normal fixture shutdown, process/listener absence and unchanged
product assets pass independent review (`6ded4eed`); blocked receipt is
`3ddce254`. No mouse or alternate driver was used. Panel ordering/presentation,
Browser login and deployed Console recovery remain unqualified.

Forge/MobkitInstaller's composed owner source `e33ed031` / `40119816` passes
38 independently reviewed controls (`53bf78ac`) for explicit optional selection
and exact cold-controller pin restoration. The earlier 12-control proposal,
pretest refusal and pytest cleanup warnings remain recorded. This is source and
fixture qualification, not installed adoption; generator defaults are unchanged.
Full controller reconstruction differs from retained gateway artifact startup.
No Core/Meta API change is required.

Returned-error service approval compensation comes from private checkpoint
`317806cc7b6e1d9e472cab1a31b0a67d2f572616`, exported as public
`5c00e4966049c1e994800a858255287e22f9c4f9`. Eight files add exact parent/child
cleanup attempts after joined approval failure, preserve published replay and
other approvals, and recheck grants before publication. Native refusal or
uncertainty remains unresolved. The unchanged Core `efd313d4` /
Meta `669f54fa` graph passes seven focused native/SQLite tests. A comparison using
only the prior approval body with the same fixtures compiles and fails at missing
revocations. Independent source and execution review passed, with all runners
and transport owners joined. This qualifies returned errors only; it does not
qualify process loss, cold recovery, installed Console behavior or logout recovery.
Historical V39 cleanup is recorded separately below. Login uncertainty stays retained and
tool-policy publication is not rolled back. Published assets are unchanged.

Generic startup recovery guidance comes from private checkpoint
`4cefacce44f38c5d7c599eb881c2fe9c6ed7704f`, exported as public
`4b165d9b4381da642bac497e368d1513b1ed72be`. The standalone prompt and existing
`setup_inspect_acquired` description ask agents to inspect relevant startup imports
and API calls in the exact failed closure and acquired replacement. The integrated
helper retains its host-selected profile. Independent review and AST comparison
confirm one description-only Python change, unchanged schema/dispatch and no new
capabilities or version pins; both apply checks pass. No runtime test is claimed.
The later V40 package includes these source bytes and completes original-task
use. That joint result does not isolate the prompt change as the sole cause.

The V39 controlled Console API case consumed the frozen package built from public
`05cc5058f77196a8bb591e3e793c9e69af91a41e`, excluding the separate UI change.
Both native attachments failed during initialize with exit code 1. The original
agent returned no figures and did not reach the successful-attachment context
path. Both observed grants were revoked; logout's 503 was followed by protected
session refusal and exact local session cleanup. Normal Stop, owned processes and
forwards are closed, with ledger and sidecar identities unchanged. Independent
review accepted this finite failure and physical closeout. A subsequent read-only
record join found additional unpublished native issuance without a revoke record
or the expected continuation source. Returned-error compensation is qualified
above. A separate native cleanup now appends exactly the two historical
revocations, preserving the prior journal prefix, identity/incarnation and runtime
bytes. The identical binary passed one scratch run before one canonical run;
actual custody was released and all processes joined, with independent review.
This does not qualify product restart recovery. Pre-cleanup usability was not
evaluated. Attachment diagnosis remains open.
Source/native and controlled installed context evidence below remains valid,
without reclassifying those failed cases; V40 later qualifies the controlled API
business journey above, while Browser UX remains separate.

The installed context fixture correction comes from private checkpoint
`4bae81d2da3a04caca57b5245c567356d171bc6c`, exported as public
`2e941711a343145f8f85d6f14206cc0b9916eb23`. It changes only the verifier and its
tests. The first installed run rejected canonical context at an obsolete pre-B
fixture predicate and never reached corruption testing. The correction requires
exact proposal-derived context before unchanged B while preserving authority,
catalog and completion assertions. Four focused fixture controls pass. The
installed rerun uses the same gateway/archive built from `05cc5058`, with unchanged
resources, library and SDK bytes; the separate UI change is excluded.

Independent review confirms one installed SDK public read with exact native B
completion and a discriminating resource-corruption refusal before model entry,
finalization or sentinel connection. Both gateways exit naturally, success with
0 and integrity refusal with 1, and their owners/readers/children are joined
without forced cleanup. The combined log retains an untagged Meta transport-reader
Future diagnostic, without assignment to a particular case. This qualifies that
controlled installed path, not general service readiness, scoped execution,
live authentication, browser UX or free-form business completion. The later V39
failure did not exercise the context path; published assets and earlier failed
results remain unchanged.

Protected Console action ordering comes from private checkpoint
`4f30a16945c98950b11b8dafd9fc5fc2a9d838a5`, exported as public
`e4802182cdcef2dd239c8edae1540ec85ecc7e0d`. Two exact source/test afterimages
reuse the shared submission flag to pause panel reads during a protected POST
and identify definite stale-offer nonentry at the access endpoint. Two matching
prior-source controls fail as expected; all 36 embedded-panel JavaScript controls
pass. Independent review confirmed the source and retained synthetic-transport
receipts. No mutation retry or native authorization rule changes. Other tabs can
still replace offers, and pending-fetch timeouts are unchanged. This qualifies
controlled presentation/transport behavior, not installed or browser execution.
The frozen V39 context package selects `05cc5058` below and excludes this UI source.

Approved-service continuation context comes from private checkpoint
`54bd16de7e6f256f5b18490f045c51d02afbd175`, exported as public
`05cc5058f77196a8bb591e3e793c9e69af91a41e`. Eight exact source/test afterimages
use the existing injected-context and whole-input binding mechanisms on the
unchanged Core `efd313d44807d3e93f6808a127fa157be84dee05` / Meta
`669f54fafafd82747c8cd9022e602a774cb4559d` pair. Four focused native controls
pass, covering actual controlled-provider projection, unchanged question, exact
replay, source-custody restoration and failure/cancellation boundaries. The
comparison retains the same corrected fixture and additive canonical-control
metadata with the prior continuation projection; it reaches native admission and
model entry, then fails the missing-context assertions. It is not untouched old
production or a permission refusal. The first compile failed before tests due to
a missing test trait import; R2 repairs only that import. Independent source and
execution review passed with joined owners. This does not qualify full process
restart, installed context use or a live-model business answer. Compiler warnings
remain recorded; no strict Clippy or whole-workspace formatting pass is claimed.

Before that correction, the fresh V38 controlled Console API case consumed the
Stage 2 diagnostic package below. It delivered an actual initialization failure,
then reached successful attachment after a fresh reviewed proposal. Original
resumed and searched its catalog, but returned no figures because the input
lacked separate model-facing completion and permission context. Both grants and
all owned processes were closed, with ledger and three sidecar identities
unchanged. This failed business outcome remains historical; blocked browser
navigation does not qualify browser UX. Published 0.9.0 assets remain unchanged.

Stage 2 native failure capture comes from private checkpoint
`32c709e7cdbf0c72533eefa552920deef1c2dad5`, exported as public
`57fbd17ddbb7175d16089d44527e8a9485a2db32`. Three source/test afterimages consume
the exact attempt observer from
[Core efd313d4](https://github.com/lukacf/meerkat/commit/efd313d44807d3e93f6808a127fa157be84dee05)
and [Meta 669f54fa](https://github.com/lukacf/meerkat-meta/commit/669f54fafafd82747c8cd9022e602a774cb4559d).
The existing native workspace and matching code generation select that Git pair;
portable root dependencies and released 0.9.0 assets are unchanged. Optional
finite metadata uses the remaining readiness deadline, and absence or timeout
preserves a timely failure. Existing source cancellation, replay and original-agent
delivery retain authority. Connect does not request or project raw stderr.
Five distinct controls pass: four retained scripted results and one corrected
actual native initialize-failure control. The same actual native fixture compiled and ran
against old production capture, failing at the missing persisted observation.
Independent source and execution review confirmed the scoped outcomes and joined
cleanup. The initial compile failure and later live-spine fixture-oracle failure
remain recorded; their corrections changed only tests. This qualifies actual native
failure capture and original-agent native input delivery, not a model answer,
service recovery or installed-host acceptance. Stage 1 below retains its original
source, synthetic evidence and earlier native pair.

Bounded attachment failure feedback comes from private checkpoint
`fdae40d584ddceba542a7d16d1369264d8b1e645`, exported as public
`49f072d3e4b8937d3cb4f7ee8b4a2578cc6140ad`. Eleven exact source/test
afterimages add an optional finite observation to the existing source
cancellation, setup result and original-agent continuation. The final source
manifest is `9102d3ba`; twelve distinct controls combine nine unchanged R3 passes
with three affected R4 passes. A corrected valid old-reader regression rejects
the added field. Earlier fixture and command failures remain recorded. Source
review preserves first-commit ownership, cancellation/admission precedence,
replay and exact delivery checks. The tests inject synthetic observations; at
this checkpoint the production native capture file is unchanged and supplies
no diagnostic. This is storage/delivery qualification on the prior development
tuple, not real-process diagnostic capture or business recovery.
Two-library Clippy completed at warning level with 95 visible diagnostics at
baseline source contexts. Existing aggregate variants may grow with the added
record field; no unchanged-size claim is made. Strict Clippy and whole-file
formatting remain failed with their scoped qualifications retained.

Normal-helper retained acquisition inspection comes from private checkpoint
`144a115ce0c7262378ce5809e84b4b8f9804000a`, exported as public
`157bad3faf0e1017008221e813f4e33cbc38f340`. Two exact source/test afterimages
and the operator guide correct eligibility for explicitly host-selected historical
jobs after matched reconciliation. Source manifest `f5a6f6a3` and execution
aggregate `ec42d7da` join 34 distinct passing local controls and one old-source
regression with Python and Node subcases. Actual MCP inspection uses authored
fixture acquisitions. Independent review found no blocking issue. Historical
status remains unconfirmed; current runtime/profile custody and before/after
readbacks are unchanged. No automatic cross-helper import, installed-host
adoption, native attachment or business recovery is qualified.

Retained guidance export comes from private checkpoint
`33034db5b582fc170c4c75d6481feb731378ce0a`, projected as public
`b1870fa0d3ebb5696830d32f8a15728106898585`. Five exact source, test and guide
afterimages add explicit saved-source skill export through the existing CLI and
portable writer. The final source manifest is `6d6644df`; the execution aggregate
is `028d6c30`. The corrected final source compiled and passed 24 distinct macOS
controls, including canonical inspection, SQLite retention, staging removal,
fresh export/readback and an independent file reader. Three old-source runtime
failures remain retained. Independent review rejected a formatting candidate
with duplicate imports; the corrected source received a new compile and the
same 24 tests. Changed-region formatting and scoped Clippy pass with the existing
command-only `too_many_arguments` exception. There is no strict Clippy or
whole-file formatting pass claim. No installed-host, live-provider or full
package interoperability qualification follows from these local controls.
Private plans, review records and runtime evidence remain outside this export.

Portable guidance discovery comes from private checkpoint
`8f3463552c1627b0503fbc12f5c5f69277fdc988`. Eleven exact source, test and guide
afterimages plus the projected portability guide add bounded directory inventory
to the existing inspection tool. The final source manifest is `215572e9`.
Eleven macOS Rust controls, 34 focused Python controls and one actual MCP 2.2
canonical-reader test pass. The latter recovers selected text after staging
removal using a scripted client and inert installation fixture. Three meaningful
old-source failures are retained. Linux's non-UTF8 filename control remains
unexecuted after macOS refused fixture creation. Scoped Clippy uses the existing
command-only `too_many_arguments` exception, with no strict-pass claim. Independent
source and execution review found no blocker within this scope. This does not
qualify installed-host adoption, service readiness or full package interoperability.

Portable executable export comes from private checkpoint
`4e388588bfb2c70457da2fa6f509dfe7ab5b2c98`, projected as public
`d938e80300ad050a30656933ce6b27193ee563b3`. Five exact source/test afterimages
and two projected guides preserve Unix executable intent, caller umask and
historical unrecorded permission observations. Three meaningful old-source
failures and 18 focused final controls include a generic stdio-client round trip
after source removal. Strict Clippy encountered unchanged baseline diagnostics;
a scoped pass used only the command exceptions `too_many_arguments` and
`err_expect`. The original failures remain retained. This is controlled local
interoperability, not third-party, Windows or full package-format qualification.

Frontend sender retirement comes from private checkpoint
`641302fcedc94075c492f31ca9c5322c4fefcf4e`, projected as public
`9347a98e72e6cb777cf131ef88730c913df61619`. Its six exact afterimages fence
local dispatch before awaited deactivation, including losing-activation cleanup,
and serialize lifecycle work per independently owned sender. Durable selection,
pending output and receipts from already-entered calls remain with their existing
owners. Thirteen meaningful old-source regressions are retained; the final seven
affected suites pass 198 tests. Independent source and retained-result review
found no blocking issue. Installed host adoption and live provider behavior are
not qualified by these local tests. Neither increment changes released assets.

The helper catalog successor increment comes from private Toolkit checkpoint
`f36ec7ca0ea97bd99289d5978282fe52a359bc89`, projected as public Connect
`11cf211b28e4d46b1c92e2424efbe6f4d0833ade`. It contains 28 reviewed source and synthetic
test files plus the host-composition guide section. The reviewed source manifest
has SHA-256 `51a46ca5eead2db234d411064180da17f3e2ca6614f63d04cece1f7f69a9c796`.
The export matches all 28 source afterimages. The latest fixture corrections are
test-only and leave production source unchanged.

The new `current_helper_successors` scope is an explicit host selection. Its
catalog-read-only parent remains separate from operation grants; the trusted
Connect binder supplies each child's exact application and current-session
domain. The parent alone does not enforce application-domain isolation. Omitted
scope retains the prior serialized request and captured-session behavior.
Existing grants are immutable, and retained work is not retargeted after reset.
Independent source review passed, and 57 distinct focused controls now pass,
including an actual native helper reset. Legacy mode refuses current catalog
authorization after reset; explicit successor mode reaches authorized Search and
Load dispatch. The inline-only test operation returns `not_deferred_eligible`,
so this establishes catalog authorization rather than successful deferred loading.
Earlier fixture failures remain retained. A separate five-file host-adapter patch
passed 188 focused controls, without installed successor qualification. Private host application source is excluded.

Separately, a fresh Console run on the existing default catalog path completed
four timely protected reviews and observed two distinct failed setup attempts.
Independent review joined both delivered failure inputs to separate completed
runs in the same original conversation. The original agent proposed a second
attempt after the first failure and returned no figures after the second, without
an operator follow-up or manual wakeup. This qualifies the second failed-attempt
continuation on the selected development tuple, not successful attachment or a
business answer. Both task grants were revoked. Normal Stop and exact owned
process/forward cleanup passed independent closeout; the ledger and known
sidecars were unchanged. This run did not consume the new successor catalog
source. Its startup cause remains unproved, and the proposed native attempt-guard
API has not landed. Published 0.9.0 assets remain unchanged.

Private SDK metadata checkpoint `24f686d5e76b3f379862f4b9cf16b1267c52ce1e`
maps to public `8a5728c79bb3dc19697b5c606e8d030756434052`. Three files select
Meta 0.9.1 for the Python native-host extra and optional TypeScript peer/root
lock; Toolkit and its separate Core TypeScript peer remain 0.9.0. Development
qualification selects Core `fa84878cfe1c0c3bc42ff77ea68ff649ca999c47` with Meta
`16cf534277c63f5a684abb62206fa690ccf74d72`. Source, resolved-graph, SDK build,
gateway assembly and controlled installed success/corruption checks passed.
The rebuilt Python wheel includes previously committed license and notice changes,
with no executable module delta from the prior qualified wheel. Those changes
retain their existing licensing provenance. The corrected fresh Forge guide
passed generated-policy checks, installation and startup, and its free-form
helper used supplied-schema inspection. Operator-missed software reviews
invalidated that task. A distinct ordinary-reset successor then failed setup
initialization and encountered catalog authority bound to the old helper session.
The initialization cause remains unproved; no implicit successor grant is added.
Both earlier attempts returned no figures and are closed. Private Forge source is outside
this export, and no published 0.9.0 artifact is changed.

Private direct-credential checkpoint `5759bf5da0744786154484ca5e44f04e47d0b82a`
maps to public `6c2fcbc71db4493cbe33d0c205b9cd669465dd50`. Its four exact files
add terminal credential inventory and protected removal through the existing
defining-realm owner, without setup/model composition or new grants. Independent
review joined 20 distinct GREEN controls and two meaningful old-source
regressions. Two test-only fixture corrections preserve explicit status
observation and exact launch-bound credential references; production bytes are
unchanged by those corrections. The normally emitted Linux CLI and matching
resources passed ordinary staging, followed by actual same-store inventory,
freshly approved exact-slot removal and `NotStored` readback. Configuration and
grants were unchanged; all three commands settled naturally without a model or
setup helper. This resolves local cleanup after one controlled Console GitHub
read, not provider-token revocation or free-form discovery qualification. Earlier
artifact, fixture, staging and bootstrap-cleanup failures remain retained. No
published 0.9.0 artifact changes.

The supplied-schema helper checkpoint is private Toolkit
`9614eabe9816a7bc20a54c6e37c884a523b60f1b`, projected as public Connect
`2e957bd1903c24fb6c73c45b70da88a5f2762a68`. The seven reviewed files add
bounded exact-byte schema hashing under the existing metadata-inspection opt-in,
matching installer guidance and focused controls. Independent review joined a
meaningful old-source failure, 20 affected Python passes and one native
caller-ceiling pass. The CLI operation-list expectation is source-reviewed only.
The matching Linux library, resources and gateway passed normal build/assembly
and the controlled installed success/corruption pair. One actual setup-helper
invocation returned the exact supplied-byte digest after native catalog loading;
the existing acquisition, original-agent read and corruption checks also passed.
Private test checkpoint `77601b947d37f1dce0713aa2a25ecfef66466164` maps to public
`9ed84fa53b09749695b90dc7fe44b2d30a74d0d1`. This qualifies installed helper
availability and hashing. A later free-form helper successfully used the same
operation; neither result establishes live-server schema equality or completed
business use. Initial transport failure and retained SDK cleanup diagnostics remain
separate from the observed natural process settlement. No release is published.

Before the catalog increment above, the follow-up branch imported reviewed source,
synthetic tests and usage guides from 33 private development checkpoints after
the release baseline, through
`24f686d5e76b3f379862f4b9cf16b1267c52ce1e`. These are file patches, not imported
private Git history. Private plans, coordination, live-case records, accounts and
build archives remain excluded. The public dependency layout, vendor notices,
catalog identities and published 0.9.0 assets are unchanged.

This source adds package compatibility and setup recovery improvements. Actual
development runs verified held installation completion and automatic return to
the original agent after an unavailable report. A later fresh Console run reached
protected access approval, then native attachment failed. Its service input was
cancelled before admission and the same original agent durably received the
failure result and opened a new setup. Its second selected integration reached
usable attachment and resumed the same original agent. Tool loading then failed
because the approved proposal lacked catalog permission, while the application
policy also lacked the selected tool-call grants. No database query or figures
followed. Grants and owned processes were cleaned up. These results establish
attachment and automatic continuation, not a completed business task.
The new optional-dependency path separately passed actual acquisition, offline
installation and direct use of `chokidar-cli@3.0.0` on Linux. That package test
does not qualify MCP attachment or the Console journey.

The Console and terminal expiry corrections keep the existing review future
polled during presentation without extending approval lifetimes or changing
grant authority. The old drivers failed the real-expiry regressions; 28 affected
checks, including those regressions, passed across the corrected scoped runs.
The combined development package passed normal production and its controlled
installed success/corruption checks, with retained transport and cleanup
diagnostics qualified by actual physical settlement. Fresh application startup
also passed. These are development results, not a new published artifact or a
completed business task. The actual fresh Console run also observed timely
settlement of an unapproved displayed review and an autonomous new request.

A separate three-file source increment adds exact raw-operation feedback to
current checked declared-secret-slot requests. Its old-source regression failed
as intended and all 15 targeted checks passed. The export preserves those exact
source files and synthetic controls; it does not include credentials or claim
live authenticated use.

The next ten-file source increment validates complete permission proposals at
the captured-original Console boundary. Private checkpoint
`7eece6ef2b4b71021967d3acecf4a3ac4ff99c63` maps to public source commit
`82206554644b9ed75896009e311b8e1366668020`, with the same reviewed afterimages.
It requires explicit catalog scope and exact exposed-tool coverage before the
existing protected whole-policy review. It does not create grants or change
trusted application and lower-level contracts. Independent review joined the
meaningful old-source failure and 28 distinct passing controls across retained
runs. Checked-request builder rejection is separate from the native positive,
which proves real deferred catalog loading and mapped inert invocation under the
reviewed service input. Earlier command, compile and fixture failures remain
recorded; production bytes were unchanged by their corrections. These are
controlled synthetic checks, not live authentication or a successful fresh
Console task. The combined credential-feedback and permission-guard gateway now
passes normal build/assembly and the controlled installed success/corruption pair.
The selected library, resources, released Core/Meta contracts and SDK bytes are
unchanged. Retained transport-reader and startup-cleanup diagnostics remain
separate from confirmed physical process settlement; no broad permission or
independently observed readiness claim follows from pair PASS. The successor
application also passed normal fresh startup and timely approval with complete
permissions. Native initialization failed, the service instruction was cancelled
before entry, and the original agent consumed the bounded failure result. No
database query or figures followed. Normal closeout and physical cleanup are
complete and independently reviewed. This does not establish a
package-specific startup cause or successful business use.

Private guidance checkpoint `b32f319d24943278a456039b199b91928b0b50ee` maps to
public source `7f43bde94999f73adca35b41c0b781e5b8d0a065`. Its one tool-description
change clarifies that a fresh setup proposal may remain within the original
request and current authority, with ordinary separate software/access reviews.
It does not permit replaying the failed instruction or reusing its approval.
Retained failure-body bytes are unchanged because existing reconciliation
compares full delivery content; this preserves that compatibility boundary.
Source review, normal gateway emission/assembly and the controlled installed pair
passed on unchanged library/resource/SDK selections. A fresh successor application
also completed normal installation and healthy Console startup, then created a
distinct setup proposal after attachment failure without replaying the failed
approval. The second initialization also failed, and the native owner refused
its failure continuation before admission. This historical V33 result predates the reviewed Core correction selected
above; it is not superseded by a later controlled pass. No
business query or useful answer followed; normal closeout and physical cleanup
passed independent review. This is not isolated causal proof of the wording. No schema, state, API, grant or
deadline changes accompany the wording.

Private test checkpoint `9cba4b43e8726465e315070e6a3157dd57ca2d6d` maps to
public `cc50658d55f0b9e4046c9f09a941ccf7f673ea8c`. Four test/fixture files extend
the existing persistent Reading management control and correct its stale exact
setup-operation expectation. The one ignored control passed against a truthfully
selected, normally staged CLI, with all existing native admission, reply and
shutdown assertions intact. The same exported application handler, service
instance, Store and database returned the nonempty stored result after frontend
removal and management-CLI exit. The first expectation failure and later old-CLI
identity refusal remain separate from this pass. This is controlled application
service survival, not native service-grant/model dispatch, live frontend use or
restart coverage. No production implementation or published artifact changes.

The [follow-up inventory](docs/releases/follow-up.md) retains the broader work.
These source changes are unreleased and do not change the evidence or claims
attached to the 0.9.0 binaries.

## Catalog identity and historical references

The adopted catalog, discovery catalog and frontend catalog preserve their exact
committed bytes and content identities. Some records contain `docs/verification`
or `docs/coordination` locators from the private development repository. Those
records and their historical acceptance data are not part of this public export.
A retained locator is provenance, not a promise of a downloadable public report
or a new live-service qualification.

Do not silently edit a hashed catalog to remove a historical locator. A changed
canonical description requires its corresponding new identity and verification.
Only unused local observation fields are removed from the source-observation
test data; URLs, revisions and recorded source hashes remain unchanged, and the
export audit records that exact projection.

## Distribution boundaries

The root licenses cover original Toolkit code as stated in each component.
The nine extracted Python modules are licensed under MIT OR
Apache-2.0 for their Toolkit copies, with exact provenance in
`sdk/python/LICENSE-EXTRACTED`. This does not change the original projects.
Upstream donor notices and optional dependency terms remain alongside their
source and retain their own scope.

A complete native source build, a packaged user distribution, account connectivity
and a completed user task are separate claims. The release notes identify the
accepted artifacts and outcomes. Historical private packaging defaults are not
portable public build prerequisites.

## Selected native packages

The retained CLI packages were built from public Connect
`2656d6777acb0ee1fcdb344fd9e7507aab3ec07b`, with the explicit public dependency
projection. The later `1bb3c234` checkpoint changes only a helper-policy test
fixture. The application gateway correction is public Connect
`52768b64c8e08e576d33f1d893f9401c8eb152ea`. Both matching gateway packages
passed their installed success and resource-corruption checks. It changes
application composition paths that ordinary CLI and stdio setup do not reach. The accepted CLI bytes and their original build
attribution remain unchanged, as do both SDKs and resource inventories.

The public source and macOS build use native registry lock
`9059ad9b7eb38f6869726f0b2c0f1cc7d500f9000ea54eb0ba023f6a46146517`.
The Linux build retains the earlier target projection
`39a9e0f51328aa3083d224cfeae052a39ce8016c8666c767bee7fbd624361825`.
The difference is the already selected `libc` dependency edge added for the
macOS-only store correction. Both projections select the same published Core
and Meta 0.9.0 package identities; their lock files are not byte-identical.

Both platform builds use Rust 1.97.0. macOS uses the optimized release profile;
Linux uses the exercised development profile with debug information disabled.
The selected resource inventories are:

| Platform | SHA-256 of `resources.json` |
| --- | --- |
| macOS arm64 | `cbdc8c838413a732ebe4dbcca5e43da388d1c653beca3cfce102574401b61075` |
| Linux x86_64 | `3280c10bef460edb19dba38099f97a115b7d49856e9dd9a092efa2a9097d5d5d` |

The final package checks use those matching inventories and binaries. They do
not transfer earlier live-account results to new executable bytes. The release
notes retain the actual package, Console and historical development outcomes.

## Included source checkpoints

The timeout and version checkpoint is `88a346721d3d3a78deb39d072151d10c06bac08a`.
Its reviewed timeout correction, fixture prerequisites and 0.9 package metadata
are included. The public dependency projection retains published Core and
Meta 0.9.0 with the exact included Core code generator.

The final licensing metadata checkpoint is
`e02d9a2135b1157f4d4a57e50f966dfa80e092c2`. Its notice and four module-docstring
updates leave the nine module implementations unchanged.

The packaging-only checkpoint
`79b529aff1d91ea45c49870a07c643b7168e3e27` adds the optional release version to
archive names, embedded README commands and checksum companions. It does not
change native implementations, internal prefixes or resource inventories.

The verifier-only checkpoint
`afb1b697b6d834510e56a111c0b016932e4b2e26` opens its selected event-source
fixture component before submitting the event. The existing native admission,
execution and shutdown assertions remain in place.

The application acquisition scope checkpoint
`3aabc80b0c344be4e2868dae370731ee1805da04` separates the canonical acquisition address
from the host-selected deployment used for standing service approval and attachment.
It includes the matching policy guards and focused regression tests.

The macOS store correction checkpoint
`c927f243b1ca2ac9bd69fbc401c683d20c0a491e` uses a one-byte open-description
lock on the same database inode, outside SQLite's lock range. It preserves
publication custody through aliases, clones and entered writes. Other platforms
retain their existing lock mechanism. Toolkit Store alone has one documented
macOS FFI exception to its default unsafe-code denial; the workspace prohibition
for other crates is unchanged. The public registry lock adds its already selected
`libc` dependency without changing any dependency version.

The verifier-only canonical-owner checkpoint
`be682d079b475401f66c683f276923ebc01d968e` makes the installed-product
fixture expect the canonical acquisition address returned by the host. It leaves
the separate deployment, service permission, route and expiry checks unchanged.

The application installation-readback checkpoint
`5cab4f4925bef91d35cdbb4c85120c01eb42b962` is included as clean-public commit
`10fbec40da17c96c61dd5c4045df605a0639993d`. It retains the exact selected software
for application service policies and reopens the typed policy authority for
installation readback. Human approvals keep their existing path; package equality,
parent-policy checks and currentness checks remain required. Historical application
records without the retained software selection are not migrated implicitly.

The captured-consumer guidance checkpoint
`5c01e362b1729a254bff95fd83b5c5328ac9d194` is included as clean-public commit
`c3be3e1d3c87a93f73cf643f03a812f5642c74b3`. After validating the current setup
task and actual tool call, a mismatched consumer selector returns the captured
native member identifier for the agent to submit explicitly. The request still
requires exact equality before access review. No display-name resolution,
automatic substitution or change to raw operation names is introduced.

The test-only diagnostic checkpoint
`69199a3827ae80da0e2c7cf2815e468583cd5d08`, clean-public commit
`9855309b7e218c1026fedcd3eba70667a22972e4`, adds bounded refusal diagnostics to
an existing native control. Its strict currentness assertion and runtime source
remain unchanged. It is not an additional gateway implementation change.

The successor test-only diagnostic checkpoint
`9450d08fbf04f2d236a1a621f97d15a9c8801de2`, clean-public commit
`ebbd8e44113d43128250e0801d35139f0a19323b`, further narrows that diagnostic
without changing production behavior.

The installed-public catalog fixture checkpoint
`acf56a194b10e838b7a0aaba2f38b5e6ab538efc`, clean-public commit
`ea3e0f5c23e495e5d81e46de77bca360c97354ae`, explicitly requests and approves
Original catalog access for the selected package check. It checks the complete
successor policy and actual native search, load and operation results. Other
fixture defaults remain unchanged. This is a verification-source change and does
not modify the gateway binary.

Its verifier successor `2466104c6644bdda636430571dad44ec9e389a65`,
clean-public commit `f1f2c487e6f78f0fc8509b2c1880a3e1e1c785e2`, declares that
standing policy selection explicitly, observes the same SDK exchange with bounded
waits, and requires exactly search, load and one public read in the native result.
The assembler checks those three call identities, arguments and result order.
The gateway binary remains unchanged.

The native audit diagnostic checkpoint
`9617555fac6b0166729712e827ca8db584b1c1d3`, clean-public commit
`07a29bfe79246c1face7393dfaa4bbfdf2bcaa88`, reads existing typed refusal
observations in one native test. It changes no production code or binary.

The package verifier checkpoint `d266fe1d7a39663583d10c209f3a59c49aea1207`,
clean-public commit `60b2751493887a8936f777033f8f93bb1073a70e`, models one
pre-tool controller recovery. Acceptance requires the exact input's persisted
`ReprepareRequired` audit, a distinct successful controller operation, and the
same final search, load and public-read result. It changes no gateway code.

The generic discovery guidance checkpoint is private Toolkit `22f3da82449201b749e1910d1238b8aadff45e98`, projected as public Connect `51a76961c7ff4fcf586f55c220304db1734661b8`. It updates the setup search tool description and installer instructions to explain short public searches, serial operation recovery and candidate-specific failures. The two files are selected library payloads, so the selected product packages require new library/resource digests and matching binaries. These source changes alone do not establish a successful discovery or installation.

The native test-provider correction is private Toolkit `3b63d781b95d066131cd85ecb7f01818178c9a25`, projected as public Connect `ecd46fad0331d9494532e2d5c4a31ce9ca4b01c9`. Two test-only modules now return typed authorization errors through the existing provider contract and advance scripted responses only after authorized output. This preserves Core-owned recovery and exact read assertions; it changes no production binary behavior.

The public-attachment test policy checkpoint is private Toolkit `52fd3307c52db03cb6c43c153ca013f85eb4497d`, projected as public Connect `449dd740252021d235620683a3be30eb9b972085`. The explicit public test variant reviews its one mapped read invocation through the existing complete-policy approval contract. Base fixture grants remain empty; the patch changes only two test modules and does not alter selected resources or production binaries.

The native test-helper compatibility checkpoint is private Toolkit `4cb1a888462786adf5f33289c7a5545227b2ddf8`, projected as public Connect `4656574ce247002944567b693a3cb2591ab185e3`. It restores the existing no-policy helper signature and delegates to the unchanged explicit-policy test implementation. Existing sibling test callers and all production artifacts remain unchanged.

The native human-policy-owner fixture checkpoint is private Toolkit `4d80b350636139fd2ce1922a7f222cd6816766b1`, projected as public Connect `5a9470cefde994245be9bb2b51bf1fcc3282a802`. The fresh public test selects the existing application policy-owner seam before host sharing and retains the exact human-approved full candidate for recovery. Other fixture entrypoints and all production binaries/resources remain unchanged.

The native tool-reprepare test checkpoint is private Toolkit `1882c68bee5086f478e0b780a076102dd2818139`, projected as public Connect `7cd9956966460ae5ef5ed942a9701d4cac3ef71c`. Its synthetic controller may choose one fresh call after operation-local refusal feedback. That candidate incorrectly required the whole refused operation to have no entry observations. Its execution showed enclosing adapter entries before a nested `ReprepareRequired`; the whole-operation non-entry claim is withdrawn. The control stopped before its independent endpoint, cancellation and replay assertions and is not accepted as passing. Attempted operations, enclosing dispatch observations and physical endpoint reads must be distinguished. This changes one test module, not native product behavior.

The layered native tool-audit test checkpoint is private Toolkit `183e3adcf2c052f3a4ec7438ac67aeb5ade2cdfd`, projected as public Connect `55093b2ddda5616667e9d74672ca34c8e92b3647`. It groups observations by exact operation identity while preserving typed refusal, result and controller ordering. The controlled endpoint must record zero reads before the sole fresh choice and exactly one read at completion; existing cancellation and replay checks remain. This corrects the preceding test assumption without changing production code. A later feedback-recovery fixture supersedes this intermediate control, as recorded below.

The setup-research public-search checkpoint is private Toolkit `ffec0982c59b09e0d9805c96f4fd0dd14e96d8c4`, projected as public Connect `bdd87cb1bd20d9a73e9e88bc80e66267c45a85c6`. An exact application policy grant for the dedicated helper can register Core's governed `web_search` tool on fresh and restored helper construction. Other tools, grants and explicit disable choices remain unchanged. This changes native implementation and requires matching binary emission; library, resources and SDK payloads are unchanged. Source review establishes the construction carrier and identity contract, not a successful search. The subsequent fresh Console task offered the tool, but its first call returned `operation_authorization_unavailable`. The task continued through registry research and finished unavailable without installation, access review or data read. The refusal code does not identify which authorization guard refused. Core confirmed separate unsupported stock builtin and nested-model composition seams in 0.9.0; category registration alone was not evidence of usable search. The ordinary MCP successor below removes this experimental selection.

The native feedback-recovery fixture checkpoint is private Toolkit `9ed2d359ac22d44db569da8462502cd580d8445e`, projected as public Connect `2f87a1322ac858521acefc4202af055bc2daffe9`. It follows the exact current input and actual tool feedback across bounded native run recovery, requires zero endpoint reads before any fresh choice and one before the answer, and binds the successful operations to the completed input. Earlier typed failed attempts may span native runs. This changes one test module. The selected native control passed its read-once, replay, cancellation and normal-shutdown assertions against this source. The successful run did not separately serialize its retry count or audit rows, so no particular recovery branch is claimed.

The installed-package startup-budget checkpoint is private Toolkit `013cb2c24c1cae02c8438122403307d2eafcf1c4`, projected as public Connect `e1ca2eed039a751a97f368a46306eefcb88d21e5`. The verifier uses its existing 900-second startup budget for the pre-accepted init request as well as preparation and post-accept settlement. The separate 60-second request default had cancelled two macOS attempts while offline preparation was still progressing; the same CLI preparation completed normally in about 77 seconds. Meta exposes this pre-accept budget as a host configuration choice. Production binaries, resources, ordinary RPC deadlines and all result assertions are unchanged; the affected macOS success and corruption checks passed with the selected host budget. The prior 60-second failures remain distinct, and the earlier Linux success used that stricter bound.

The ordinary public GitHub research checkpoint is private Toolkit `3c3575793e147a61f65d4e378750ab34d9ec2666`, projected as public Connect `2656d6777acb0ee1fcdb344fd9e7507aab3ec07b`. It replaces the experimental native search selection with `setup_search_github` and `setup_read_github` on the existing setup MCP server. The agent chooses public repository queries, document paths and follow-up steps; the host retains exact server configuration, finite operations, normal MCP target lineage and explicit helper grants. The worker uses anonymous direct GitHub API requests and no nested model call. An actual MCP worker proof searched a public repository and read its verified README without credentials. Changed setup payloads require matching library, resource and native binary successors; SDK archive bytes remain unchanged. This worker proof alone does not establish a complete Console task.

The helper-policy fixture checkpoint is private Toolkit `99f07c13db9aa3cde4cf1af33fb912cfee8ea8b4`, projected as public Connect `1bb3c234640ee3c59347c48dbae3ae34a41fbb3c`. It sorts the synthetic grants by their canonical tool names before constructing the test policy. The three new research controls then passed; the six unaffected controls had already passed. Only test code changes, so the selected native implementation remains the preceding `2656d677` checkpoint.

The application helper-projection checkpoint is private Toolkit `6a14bfa4f330988597f13177da7079b085061198`, projected as public Connect `52768b64c8e08e576d33f1d893f9401c8eb152ea`. A fresh Console attempt with the ordinary MCP research grants failed native host composition before login. Its retained preparation had enabled both research operations, while initial Rust compilation retained the base helper profile and server. The later Driver projection added the selected operations, so exact reserved-profile equality refused composition. This cause is derived from the emitted source and retained inputs; the failed process exposed only a generic admission error. The correction selects the policy-derived optional ceiling before capturing the definition, server and resource owner, and checks it against the same compiled policy during completion and restore. Full member, server and definition equality, default-off behavior and runtime policy/currentness remain required. No schema, stored authority, library or SDK payload changes. A dedicated compilation-to-Driver regression reproduced the reserved-profile refusal using the baseline compiler and reconstruction with an extracted unchanged Driver expression and compile scaffold. This was not an untouched all-old product build. The same regression passed on the correction, as did all nine helper controls. Both corrected gateway packages then passed their controlled installed success and resource-corruption checks, including actual catalog search, load, one public read and normal shutdown. Neither corrected run emitted recovery evidence; the prior macOS recovery result remains tied to its earlier binary. These results do not establish the separate real-model Console business journey.
