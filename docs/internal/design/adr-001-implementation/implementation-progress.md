# ADR-001 implementation checkpoint

## Current delivery status, 2026-10-04 at 17:10 UTC

The native governed-flow checkpoint is published and CI-green at `00b0cab3`.
Its allocation/benchmark successor `b12d81e7` is still running normal push
hooks; the earlier CI result does not qualify that successor. Performance is
still unacceptable: the latest six-cell study missed every conditional mean
threshold, with fresh-turn overhead of 137.01 and 145.20 percent at grant
depths 1 and 3. Individual-operation p99 and full coverage remain open.

Console PR520 has advanced to `127331ff51605136da5264fe680cb9868b015bf2`,
tree `7daa82d6f0a359fdc7af52b0734c85c0e268a744`. It adds explicit access
inspection from group membership and refreshes the required embedded assets.
The initial CI attempt failed asset freshness; the successor passes that step.
[CI run 37217158682](https://github.com/lukacf/meerkat-mobkit/actions/runs/37217158682)
passed all 11 jobs, including browser and audio checks, in 37m19s. The focused
access-view tests passed 19 cases, and the
existing adapter suite passed 313. These remain Console projection tests;
MobKit's native dependency is still 0.8.50.

The preview safety baseline is committed at
`64b44ff9d0fb6d3118c324b20e0bcc4449ac6659`, tree
`aeceef3a8ed608b6fab842cf62f80e1595534a49`. Two preview tests and the existing
archival/recovery test passed with zero failures or ignores; normal commit
hooks passed. They cover isolated effects and full state, binding rejection,
retained poison, and cleanup. They do not prove that a poisoned preview invokes
cold validation. The production fork optimization remains held. The first
compile attempt used a stale macro artifact and ran no tests; rebuilding the
correct macro took 2m31s before the two preview cases passed in 0.04s.
Raw results are `costf9-preview-live-fork-baseline-r1.log`,
`costf9-preview-live-fork-baseline-r2.log` and
`costf9-preview-live-fork-archive-baseline-r1.log` in the retained evidence.

The separate push-range workflow fix `5ecab297` remains unpublished. Its
70m13s normal push passed Clippy, machine and hook checks, then failed one of
12,155 executed unit tests (17 skipped). The unchanged interrupted-harness
cleanup test observed FIFO closure before its immediate process-state check
reported exit. A bounded process-exit check with a still-running negative is
being prepared; the cause is not yet proven. Integration and E2E gates were
not reached. This failed attempt is recorded as validation time, not delivered
feature coverage.

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
A fresh typing-lag HOLD acknowledgment was absent; no new all-owner clearance
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
OB3 and Toolkit supplied current nonconflict replies, while fresh
lead/typing-lag acknowledgment was absent. No full-host acknowledgment is
claimed. Performance remains an unmet architecture gate, and further runtime
expansion remains held behind it.

The next candidate `f9f0cd55ef06156ebda74097cac3591ca538f2f4`, tree
`4ef17e5630398603de7accb6ccef7f9c9e1780d1`, shares the private immutable input
association across clones. Its new sharing/encoding test first failed on the
old representation; the repair passed all 58 contracts tests and strict
all-target Clippy. The optimized build passed in 453s, followed by both native
cost correctness selectors (0.04s and 8.74s). The executable SHA256 is
`18293a9065f91338a55bd93df24f176689bd120600b3c1220e2f772a65385664`.
OB3 and Toolkit source reviews found no blocker. The measured fixture, analyzer,
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
Fresh lead/typing-lag acknowledgment remained absent; this is sampled resource
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
`codex/security-shell-confinement`. The stock factory Required-shell E1 has
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

The bus monitor is active. Publication clearance and a quiet performance window
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
`/Users/luka/.codex/adr-001-evidence/custody-repair-20261003`.

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
`/Users/luka/.codex/adr-001-evidence/gcp-native-governed-jsonl-r7-20261002`,
seal `68c703166c141ad2b46fe6752ff459fb43dc828a265ca27263904968719ad45e`.
Schema R2 and module metadata acceptance are separate, source-qualified gates.
Clean checkpoint `11b3b77912bd7c6511f3d26f7f2a1b81087948e0` normally merges main
without an authorization-code change; it does not establish a new aggregate
runtime result.

The accepted R4 optimized build and two correctness smokes cover historical
source `455d0e3b6bb143a53017392b203df49af2b518cc`, not the final publication
candidate. Their receipt is
`/Users/luka/.codex/adr-001-evidence/gcp-native-cost-build-smoke-r4-20261002/root-acceptance.json`.
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
`/Users/luka/.codex/adr-001-evidence/gcp-native-s3-persistent-admin-green-r2-20261002`.
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
`codex/local-governed-default`. Its frozen audit/SDK checkpoint is unchanged:
266 selected Rust tests, 414 Python tests and 460 TypeScript tests passed, with
typed TypeScript compilation and the recorded schema/surface checks. Evidence
is retained under
`/Users/luka/.codex/adr-001-evidence/audit-sdk-green-20261001`, manifest
`e3e352ff860ef150aa1a6ee090a410870b9db84939ab2fe03ff3e1eaac52be33`.
This includes the storeless native model-tool-model fixture: denied delete,
feedback, permitted read and completion of the same run with native audit
assertions. Scripted provider results do not prove real-provider or persistent
coverage.

Publication is a separate extraction in `meerkat-authorization-publication`,
branch `codex/authorization-foundation`, based on main `9ebe09fa`. The first
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
by GCP, HomeCore and OB3 at 16:14 UTC. Toolkit verified the final
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
Full provider/source/Elephant/comms/delegation/surface support and measured cost
remain required. No benchmark or full-profile acceptance is inferred from these
package results or design reviews. Historical records below retain their
original scope and do not override this status.

## Earlier native and high-assurance checkpoints

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
