# ADR-001 implementation checkpoint

Checkpoint: 2026-10-01. This supplements the original static requirement
inventory. Its 111 requirements and 57 explicit cases remain open until their
actual owner, recovery and sink evidence is assembled. The components below
are prerequisites and bounded observations, not complete governed enforcement.

## Current integration checkpoint

The current lower native association checkpoint is clean at
`2356d0eccb6b6a43172e5d73474a22745f1a53ee`, on composed prerequisite parent
`8a6265fd6`. Contracts34 tests+3 compile-fail docs, combined host114 tests+3
compile-fail docs, strict package lint, stable-tree Cargo/Bazel/example metadata
and normal commit hooks passed. Toolkit independently accepted the frozen source.
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
its real-file regression tests remain unexecuted. Toolkit accepted the frozen
grant target-association source, and canonical generation is queued in the next
serialized lease. Clock and witness encryption sources have separate frozen
review candidates. The clock profile has a known idle-entry latency failure
against the unchanged budgets in [the acceptance plan](acceptance-plan.md).
Early demand refresh does not close that failure.

Elephant PR4 at `64e5a8d02eda56df6b34c9c448704e87eaabda23` has green hosted
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
`/tmp/adr-001-tlc-raw-root-verification-r1.json`, SHA-256
`097f6878404b80c24ea0845d83b390191fb47bb99adb0af5721952e7556dcacc`.
The GCP09:00-09:45UTC reservation produced an incomplete retention comparison;
render load and the unchanged50000-row deadline prevented full acceptance.
The detailed receipt and future resource conditions are in
[the critical path](critical-path.md).

## Earlier published checkpoints

| Candidate | Exact source | Evidence and limits |
| --- | --- | --- |
| [Meerkat ADR PR 1352](https://github.com/lukacf/meerkat/pull/1352) | `97d3078e2e5cff9f0a49e3f46c67d2da7a9a0fe9` | Four requested bus reviewers accepted the frozen r6 design. Scoped PR source reviews and two Homecore record corrections are retained. Hosted CI gate passed at this head. The PR remains a draft; it changes documentation and archived-review hygiene, not runtime enforcement. |
| [Elephant prerequisites PR 4](https://github.com/lukacf/elephant/pull/4) | Published `e55531eb0e41f0c04ab356f9100aa7869197cc36`; local rebased source `3985dbe37f8ee842848c776860d5e6ecf94cc885` | Independent ABAC fixes, verified caller context and retained-work repair passed 2,031 local tests and strict lint at the recorded integration checkpoint. The narrow synthetic-fixture repair passed local full-history scanning, two negative controls and the real authenticated MCP regression. Normal publication hooks and all eight applicable hosted jobs passed in run 36809459432, including all-feature tests and full-history scanning. The workflow skipped E2E; no E2E result is implied. No governed endpoint is enabled. |

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
| Physical SQLite custody | `775234131` | Eighty-two SQLite and seventeen custody tests plus strict lint passed. The normal publication gate passed 12,044 workspace unit tests, then both integration attempts hit their 900-second deadline; the push failed. Timed-out tests are not passing evidence. The warm retry then failed two timing tests outside the custody diff: 12,042 unit tests passed, two failed and 17 were skipped. It did not publish. The log is `/tmp/adr-001-custody-push-warm-long-budget.log`; no skipped-hook success stamp is claimed. The GCP exact-tree unit 12,051, integration 2,759, HeadCanonical 9 and fast E2E 30 all passed. The report and archive were downloaded and independently checked, including all 16 embedded manifest entries and eight raw lane summaries; the archive SHA-256 is `a8e353f2a226b1a25bca2b99289c5d953577862280f7948913fd870f23477373`. These Linux lane results create no local hook stamp. Physical ownership is not antirollback authority. |
| Native retained connection adoption | `ff3af7b4e8940826b7a753b6e043b5115391d01b` | SQLite 88, session/schedule 82, runtime SQLite 107 and facade 5 tests passed, with strict four-package lint. Twelve reviewed source hashes survived rebase. Session, runtime and schedule share one physical connection; other realm writers remain unadopted. Uncertain commit and leaked transaction retire the shared handle before queued aliases enter. EVID-2 remains open. |
| Exact numeric model packets | Reviewed `88089558f24636134aee16d175bc5f1523f828d2`, rebased to `1877c81f4d36116de4a07e74753d69650ce4d6cf` | Generated numeric witness packets and actual reached model transitions. The normal publication retry with the actual MCP fixture failed three MobKit unit timing/lifecycle tests: 12,063 passed, three failed and 17 skipped. Integration did not run in that retry; no push occurred. The failed gate is frozen under `numeric-publication-880-with-fixture`. Rebase onto main `39cae9bec` preserves the two source files and includes the upstream fixture and truthful-stamp repairs. Current-base regeneration and a controlled publication attempt remain pending. Bounded models do not prove a production store or verifier. |
| Generated atomic composition and compiler repairs | `bd582768368599ab7a7651e58a9f55150dfd0fd9`, `802f18ac1b0e50ffb108f4123161937b3ed2f2cc` | The coherent generic stack is committed with a clean tree. 681 tests, 17 final fixture tests, strict all-target lint, normal hooks and drift for 15 machines/9 compositions passed. Conditional substitution and mixed phase/data repairs affect 12 canonical model files; the other 12 and Rust compatibility kernels remain byte-identical. A checksummed before/after packet containing 31 bounded and 12 separately classified deep configurations is with the GCP lead for controlled acceptance. The controlled comparison reports no regressions on completed rows: 31 bounded rows include 18 passes, seven unchanged unproven goals, three unchanged deadlocks, two incomplete and one unchanged rc151; 12 deep rows include six passes and six incomplete. Root verified archive 16942f40 and all 43 paired classifications/counts/coverage in the summary JSON. Raw per-row command/environment/log archive is separately requested; no universal model-success or local-rerun claim follows. |
| Generated atomic joins and keyed witness owners | `04defbcba8c9c2f4689e79fb90bd91fe540f16f0` | The canonical feature host, keyed owner machines and private generated join are committed. Sixty-four package tests, strict all-target lint, metadata checks and normal hooks passed. All 14 frozen witness traces reach their goals, including positive coverage on five refusals. The explicit `witness-control` feature test target exercises the actual host; the SQLite successor also adds a PR/main feature-unit suite. Accepted-row absence, mismatch, malformed bytes and unexpected occupancy invalidate cached custody; ordinary pre-acceptance storage failure remains distinct. Durable backend, authentication and recovery lifetime remain separate obligations. The subsequent actual-owner codec opt-in is committed as `386b64b328aef8eb000a7a2629b7a5bb61c132c0`: 70 package tests, strict all-target lint, selected generation/drift, metadata and normal hooks passed. An independent reviewer checked all 42 source/artifact hashes and eight gate logs. Untrusted snapshots remain data, not current authority. |
| Generated owner state codec | `230a16791595960d50e2a980e3dd2742f93c3ecf`, `6cf865d2551f4269370c7d763e71cc8ab7985536`, `6d24ac7a3f4fa4250e4fae8e5505db97a2020a81` | Explicit DSL opt-in emits strict, versioned untrusted snapshots without serializing live authority. Structural bindings and borrowed capped output are committed and independently source-reviewed. DSL 24, owner codec 14, principal 15 and schema 35 tests passed; all 17 non-opted owner expansions remain byte-identical to the coherent generic baseline. Combined strict all-target lint for DSL, proc macro, schema and core passed, as did formatting, Bazel freshness and normal hooks. The conservative principal source fingerprint can invalidate persisted snapshots after nonsemantic source changes; migration or refusal is required. |
| Physical witness owner backend | `ef8deacbc4fc2680e927aba263059eecd9483bc9` | The SQLite adapter uses the retained physical connection, exact generated snapshots, a namespace head CAS and one required-absent or existing request row in one synchronous transaction. Two independent source reviews exposed and closed TEMP shadowing, direct and indirect foreign triggers, inbound foreign keys, foreign indexes and conflict-classification gaps. The indirect-cascade finding was reproduced against real SQLite. After integrating the actual owners and codecs, 71 unit tests, 23 integration tests and one compile-fail doc test passed, including actual SQLite COMMIT refusal, post-check corruption and physical retirement. The first compile failure was a test-fixture static-lifetime declaration and is retained separately. The final adversarial review additionally reproduced main/TEMP FTS3 and FTS4 shadows of the table-valued foreign-key introspector. All four actual Rust regressions failed before repair, then passed after direct catalog collision refusal. Final tests passed 75 unit, 23 integration and one compile-fail doc case; the exact PR-unit nextest command passed 75/75. Strict all-target lint, WASM compilation, CI selection/selftests, final metadata and normal commit hooks passed. The initial stale-lock metadata failure remains alongside its successful refresh. Independent review is GREEN at the committed source hashes, with no governed activation claim. Eleven source hashes and 20 evidence files are frozen under `witness-sqlite-ef8deacbc`, manifest SHA-256 `89ec02bccca7f37d317115fdf344ad4df6904277bbce9b4ecea3f3d869e39817`. |
| Canonical grant feature host | `e9d6f02f349518c7520c37a25410ad38113c7e2d` | The isolated prerequisite is committed with normal hooks. Fifty-four feature tests, 297 schema/kernel tests, all five reached TLC witness goals, strict all-target lint, selected drift and metadata checks passed. The first restoration witness deadlock exposed an omitted expected-transition declaration; that failing receipt is retained and the canonical witness metadata was corrected without changing its reducer. This host preserves existing cached validation semantics. Stored content digests and an earlier Valid state are not current permission or constraint attenuation. The reviewed r7 durable handoff and owner-generated domain projection remain separate work. |
| Explicitly enrolled workload authentication | `8d580f52789161ab7bf80b3c4c1c70c8fcccdde8` | Clean normal-hook checkpoint. The exact source passed independent review, 369 focused/package tests including two compile-fail cases, strict all-target lint, full canonical drift (18 machines, 10 compositions), bounded TLC (62 generated, 26 distinct, depth 5), metadata and hooks. The finite NatValues={1} model does not prove higher-generation rotation; actual Rust rotation tests passed. Real Ed25519, strict field/wire binding, weak-key rejection, administrator-pinned bounded file access and FIFO cleanup are covered. The private observation proves cryptographic binding under selected enrollment, not independent currentness, measured time, replay, a human requester or permission. Frozen manifest `workload-authn-8d580f527/manifest.json`, SHA-256 `24d5b40743f3a20091112d983a18f96fd5e671fddcdb26f360ae09ffe9b4816c`. |
| Independent Elephant negotiation | `188e4437249b4bf76df9414388ec41dae5e1e303` | Elephant consumes the exact 14-case Meerkat negotiation corpus without a Meerkat dependency. Its 15 existing auth tests, two consumer tests and strict lint passed. Toolkit and a subagent accepted this exact bounded source. Raw ingress and actual protected sinks are not established by this normalized corpus. |
| Independent Elephant restrictions and information flow | `82198dbbe8b5a014dd292f8806a9e4993f547c56`, rebased to `e55531eb0e41f0c04ab356f9100aa7869197cc36` | An independent implementation executes all 26 shared cases, including 13 actual derivation steps. All 23 auth/negotiation/information tests, strict lint, formatting, docs, surface and version checks passed with before/after source hashes. A subagent and Toolkit accepted the bounded source and recorded evidence after diagnostic redaction was strengthened. Neither reviewer independently reran the checks or attested the complete build graph. Rebase onto the hosted-CI-green Elephant head preserved all eight reviewed source hashes; all 23 tests passed again with `--locked`. Resource-domain differential and actual authenticated sink integration remain open under A003. The reviewed conformance commits are integrated into the clean PR worktree at e55531e. The normal e555 publication hooks passed, including full workspace unit and strict lint, and the head was pushed. Fresh hosted CI did not start because main #3 introduced security conflicts. A separately reviewed semantic-union rebase onto exact 0ce109a is clean at 3985dbe; root verified all ten changed hashes and all eight preserved conformance files. Fresh focused tests passed 913/913 (seven existing ignored), and all 23 auth/conformance cases passed. Strict all-feature lint and normal publication gates remain pending. Only the earlier 5e8efec head has confirmed green hosted CI. |

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
four bus acceptances of the implementation protocol and its keyed-request
amendment. These are protocol verdicts. They do not close the implementation
requirements in the table or the complete requirement inventory.

## Executed runtime observation

The [baseline tracer](runtime-tracer.md) now ran successfully against published
Meerkat 0.8.49, with deliberate process loss after durable input admission,
recovery of the exact input without resend, actual helper delegation, parent
continuation and console delivery. Toolkit accepted its bounded diagnostic
evidence and the corrected explicit export allowlist. It does not prove
requester propagation, live authority, audience authorization or helper history
durability across restart. The old manifest that selected identity-key paths is
retained privately for audit and must not be used for packaging.

## Next acceptance boundary

The first governed vertical slice still must compose authenticated ingress and
durable work association, canonical grant/policy/resource generations, physical
custody with independent witness recovery, and exact entry/settlement evidence
at real model, tool, source and recipient boundaries. It must demonstrate both
allowed effects and physical absence of denied effects after revocation,
restart, missing evidence and uncertain outcome. MobKit-owned reads and streams
and independently enforcing Elephant operations must use those same contracts.

The full feature/surface inventory and deployment budgets remain required after
that slice. Homecore supplied a recorded workload of 20,000-50,000 work items for
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
not certify Homecore's deployment budget or create local successful-hook stamps.

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
- The lead conditionally accepts witness time at a fresh post-fence logical decision point with a historical durable reply. GL4 remains open until authoritative final release checks, fresh retry observations, uncertainty refusal, cleanup and exact pause/race fixtures pass. All four bus reviewers received the written clarification. The inspected GCP deployment lacks the proposed authenticated clock source; no daemon or host-clock change has been made.
- The numeric publication lane remains held for the lead's mob-flakes repair SHA. The separate main integration repairs do not authorize retry. No hook bypass, merged PR, production deployment or full ADR completion is claimed.

## 2026-10-01 adversarial continuation

- GF-4 revisions 5/6 closed restored-away entry recovery, positive expiry, explicit human-only Unknown continuation, predecessor qualification and reached model obligations in design. Lead/Toolkit exposed hidden adapter resends, incomplete final provider-wire binding, untagged output fanout and continuation lost-ack replay. Revision 7 received lead acceptance with final text conditions. Revision 8 (89250ecca1a51f8a5b36f302161d7fdba0d3607a0095427da0a5874a33457090) is with both reviewers, adding explicit reqwest no-redirect/no-retry construction, HTTP/2 probes, refusal of catalog-selected realtime text, and one generated continuation successor per Unknown predecessor, including overlapping-set and concurrent-command races. It binds final lowered wire bytes, forbids internal resends/redirects, selects buffered first-profile output after committed settlement, enumerates all consumers and reuses canonical admission idempotency for continuation. Native admission remains dependent on NativeWorkAnchorV1 and the live b+2b handoff; attempt source remains held. A surviving entry anchor plus restored Unissued local bytes does not establish non-disclosure.
- The generic single-owner preparation source is frozen uncompiled on d93 with 20 paths (manifest ea72dcda) and independent bounded source acceptance. It retains the actual persistence future through publication, poisons at bind to cover forgotten guards and opts in no production owner. Its 14-commit prerequisite stack is cleanly rebased onto main 456dc09cb at 8cd7d7a79, preserving upstream redaction. The successor source is independently GREEN on exact current-base manifest 5e53350c; root inspected the emitter and authorized scoped generation/execution, now running. Actual native runtime gate integration remains open.
- The additive observed-input join source passed two independent pre-build reviews. Canonical generation passed, codegen 149 tests and actual SQLite 108 tests passed without source repair; strict lint, drift/WASM/metadata and hooks are running. It supplies fresh mechanical post-fence observation and explicit rollback cleanup, not trusted time or release authority.
- Witness bootstrap revision 2 (27c90d3acc579978a6aa4175cb197a34d0ec8f9991d4170206fc75a3829c7d8a) closes two-sided commissioning, client-detected witness rollback and singular retirement in design. Revision 3 received conditional schema-partition acceptance. Revision 4 df967985fc0ce7010ed6e1cbc9d0717154a50afa6a218ab260bfad1b325eacc1 writes in the incident record, inner profile identifier, gated client high-water acceptance and explicit refusal transitions. It also requires a client-secret keyed plaintext commitment and client-authenticated outer custody envelope, closing the witness-admin dictionary-attack gap in the prior bare-hash proposal. Current-main prerequisite composition is starting; no schema or crypto producer is yet implemented. The operational service and actual clock producer remain unimplemented.
- Time clarification cf6c2398d7c83c801d8f03f7977c56ef9fde711a21af3dd18cdf151619e48639 has exact design acceptance from the lead, Homecore and Toolkit. OB3's current revision response remains pending. Those reviews establish no producer, sink or deployment qualification.
- Pure restriction contracts relocated byte-preservingly at ea8ae6ef840dbf2edd67ba42e669931e7c208c55 with focused gates and normal hooks. The isolated immutable child-derivation carrier passed 13 unit+3 compile-fail docs, 46 existing authorization+ 1 conformance tests, strict lint, WASM and strict metadata checks. A discriminating null-payload test repair and narrowly scoped lock repairs are in final source review (manifest 5855d806); normal commit passed at ac63171e261e33bd60c2a1564be530c265b05408 with a clean tree. Nine source and fifteen evidence files are frozen under derived-restrictions-ac63171e2, manifest fba4d286ce2609bdcd628d0cb41b45fb5bf82da422e095aedd766a3874768f40. No compiler/grant integration or runtime authority follows.

## Critical-path control, 2026-10-01 08:30 UTC

The [integration critical path](critical-path.md) now makes the first actual governed native prompt the next integrated acceptance milestone. Independent prerequisite breadth is subordinate to that path. Review is relative to accepted source checkpoints; only concrete integration defects reopen settled work. This changes scheduling, not the complete ADR scope or its open requirement/case inventory.

The Mac build queue is serialized after its already running jobs drain. Elephant's normal publication completed at exact `3985dbe37f8ee842848c776860d5e6ecf94cc885`: 2,052 workspace unit tests passed, 97 existing skips; all normal push hooks passed. The PR is mergeable and fresh hosted CI run `36835999238` started. Its full-history secret scan failed and is under read-only investigation; fresh CI is not green. A concrete SDKROOT difference explained the repeated native Clippy build. Future manual prevalidation will match the normal hook environment without adding a tooling project.

Generated preparation owns the next focused Mac lease. Root then reuses the now-idle `adr-observed-input` warmed cache for the single coherent integration gate. Observed-input is clean at `2bf69f187665c017ce932d6a715080746365db92`; package149/auth108, strict lint and focused drift passed. Its all-catalog drift was deliberately interrupted and WASM never started; both remain explicit integration gates.

The GCP lead reserved **09:00-09:45 UTC** for the unchanged retention benchmark controller and binaries. GCP agents must stop heavy work by 08:58 and launch no compiler/TLC/hook-running push during the reservation. Host idle and compiler-free preconditions remain unchanged; violation yields NOT_MEASURED rather than another blind probe. CoreNext's 88 comparison jobs have finished, with classification/raw evidence still pending.

GF-4 revision9 (`7d11682b067c38e54db57c78a1f1de4b59644849b5710510271ca62935dd779f`) has lead and Toolkit textual acceptance. It closes complete owner-derived frontier, surviving continuation/readmission claims, linear recovery chains and exact committed observation recovery. Source must distinguish a surviving committed outcome with pending evidence from Unknown. Attempt source review may proceed. Native admission edits wait for the lead's explicit handoff at turbo-live 2b's merge commit; unrelated main changes do not trigger repeated rebases.

Witness bootstrap outer revision5 (`c01fae20ea2f12349342d1d21aa6b08d3b3c90a509bb1a77d974457876f292e0`) is accepted for source. The neutral certificate-evidence framing is frozen before implementation. This does not qualify an inner proof, clock, key producer, current witness or runtime entry.

The root composition onto generic/main456 reached `c92544f6e3de`. Delta review found one real integration defect: the newly upstreamed current-metadata read used a pathname instead of the retained ConnectionSource. The one-line repair and real closed/poisoned/valid-replacement regression are independently source-reviewed; execution awaits the coherent integration lease. This finding is distinct from an already accepted component being reopened without evidence. Strict canonical Bazel/Cargo lock checks passed, with all 254 unrelated live-universe pins preserved.


## 2026-10-01 coherent checkpoint and first-path deltas

Composed current-base source8a6265fd6 now has focused execution evidence and canonical zero-drift generation, recorded in critical-path.md. The retired-custody metadata regression discriminates the repair: restoring the old pathname read fails the closed-custody case, and restoring the accepted source passes all90 SQLite tests. Combined authentication/witness130 tests and3 compile-fail docs, contracts13+3docs, DSL-core32, single-owner fixture15 and model2 passed. Strict lint/WASM and complete publication gates remain open. Required Cargo/example-lock consistency and strict Bazel metadata passed; all254 unrelated live dependency pins were preserved.

The lower immutable native association source is frozen at manifest3da7e0285e862ed618a00d122b39b3633d4f4a34ca9083280ad26ffdd85b2716 for Toolkit review. It contains complete original work/authentication/grant/source references, an exact qualified ingress key and bounded canonical binding bytes. These are caller-constructible data, with no accepted/current status. Existing protocol/information/evidence definitions move to the lower contracts crate with exact-type host re-exports; production definitions retain their original bytes. Real generated admission and surviving acknowledgment are still required.

Witness physical journal source is under independent review at9237565f86d270c40dd7b589bba38a27ef1053ac1ea6799f72ce188051b290d3. Its12 real-file tests and3 no-create custody tests are authored, not executed. The sealed native-work format r2,8022cdbe8968b3458a3b65b7ba956d1bf2254a20370e288696baecdaaa086e3d, has root source-contract acceptance f17154ec7f37f35f23c1686b78ce3c4a269efa9d8d902139501215873371e46b. Actual length and native receipt digest stay encrypted; encryption and commitment key generations are separate; public native receipt/head commitments are item-specific keyed values. Real historical key custody and authenticated service/client completion remain open.

Measured-time proposal r4,7c070013280e05ffb3a4f34c6b0b23ff4912f18a444954db839012bb644e3090, has lead conditional acceptance219359a1fffcff6b9d3ab7ed3d620735e62f99eaa8999ddf526e724ca059b5c3. Source work must add actual single-flight acquisition, preserve valid prior samples on failed refresh, account for all qualified host-discipline terms and declare the short sample lifetime/acquisition latency. A fresh stateless observer per demand bounds all possible NTP samples inside the acquisition bracket; reference timestamp changes do not prove fresh measurements. Deployment qualification and the model-entry latency budget remain open.

Elephant's identical-tree history repair at64e5a8d02eda56df6b34c9c448704e87eaabda23 now has green hosted CI36838043537, including the full-history secret scan and all configured required suites. E2E was skipped. GCP retention run2 remains incomplete:50000-row paired processing exceeded the existing deadline while unrelated render load was present. Partial20000-row data is retained with its host-load limits; it does not close full workload or Homecore deployment acceptance. See critical-path.md for resource decisions and actual timings. Full ADR implementation remains OPEN.


## 2026-10-01 bounded source review and clock budget disposition

The native association source has independent Toolkit acceptance, review8b2576b2b847c4b36d1111ba7199f85187e25441c8f4f018d21fe1fa282810cf, on frozen manifest3da7e0285e862ed618a00d122b39b3633d4f4a34ca9083280ad26ffdd85b2716. Root verified all review hashes and read the complete report. Contracts34 tests+3compile-fail docs and combined host114 tests+3compile-fail docs passed. Strict lint passed after one test-only missing semicolon was repaired; production bytes remained unchanged. Canonical metadata and normal commit are completing in INTEGRATION-02. Native acknowledgment/currentness remains open.

The accepted LinuxChronyNtsV1 r5 design has a known model-entry and turn-overhead budget failure after idle periods: conservative sample lifetime is seconds or tens of seconds and a fresh stateless authenticated acquisition takes multiple seconds. This is explicitly recorded in acceptance-plan.md as failure against unchanged numeric bounds, not merely missing measurement. Demand-triggered early refresh through the same owned single-flight worker is selected for active conversations and remains unexecuted. It does not resolve idle latency or qualify the deployment.

Witness key-custody source has bounded root acceptance096454ea8e480cceaf158e2021a994c3edeb564b444304f34789b26b6eedef97. The actual selected protected commissioning/client factory and administrative/recovery-root qualification remain open. Independent physical-journal reviewd49ddaea28539d81bb5079e2e3a4277b6752b90852c375d3847daa827d344d3d found NW-P2-1: commissioning retry could return success for an absent primary head with surviving child history or an existing head with corrupt indexes. The owner is repairing transaction-local inventory checks; the original RED checkpoint remains retained. Toolkit is independently reviewing the frozen grant/control target-association source at manifest618290b14e90445189915c7515d6a224a5c202cda289ebb9d4ae0c8c9189da9b. No new heavy agent build is authorized.
