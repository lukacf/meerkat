# GPT-Live remote development handover

Status: incomplete implementation; preserved, unqualified WIP. No feature
release or final PR is complete.

## Why development is paused

The user's direct messages in coordinator local history, turns 761 and 762
(2026-09-14 11:22 and 11:28 UTC), requested: commit and push all work with
`no-verify`, write a handover, and move development off the laptop to a remote
agent because of airplane connectivity. These messages were absent from the
subsequent conversation summary and were recovered from the local session store.

The checkpoint's skipped hooks are authorized for this preservation operation
only. They are not qualification, permission to skip future release gates, or
proof that generated files are current.

The local implementation writer is held in interactive mode. Do not start a
second writer until an explicit, exclusive remote ownership transfer is recorded.
Do not reset, reimport, amend, force-push, or discard either checkpoint.

## Exact source to transfer

| Repository | Pushed branch | Exact commit | Tree |
|---|---|---|---|
| `lukacf/meerkat` | `luka-crnkovicfriis-abk-public-live-import-7d5b0bf3` | `87eb78a57e7156fa2d00d57bc4789e6c4bd64591` | `8a0586a6eb982512e7cbaa80497a912829c52e66` |
| `lukacf/oai-rt-rs` | `luka-crnkovicfriis-abk-live-session-identity-fence` | `b6ff6d505bd9ed7b8fed75124955c01e55ee6132` | `2dbbdcc305abff7af7d165e02a59cd54bfb4d323` |

Both remote branch heads were read back with `git ls-remote` on September 14.
Meerkat's worktree is clean at the checkpoint. Its parent is the previously
verified import `3d57894f3dc49efe0ced32d81381500315be6eea`.

Meerkat reflog records initial checkpoint `8ebc6edc8` at 14:10 CEST, amended
to `87eb78a57` at 14:25 CEST. The amendment changes only:

- `specs/compositions/meerkat_mob_seam/model.tla`
- `specs/machines/meerkat_machine/model.tla`

Their generation/semantic correctness is NOT certified. Preserve the amended
source and run normal fresh generation/drift checks when remote execution is
available. Five source hashes recorded in delta205 match the checkpoint.

The coordinator and sole implementation writer did not execute the checkpoint
in their retained tool history. Its subject, timestamp, and explicit
not-gated/no-verify message align with the recovered user preservation request;
this is not an independent attribution of the committing process.

## Ownership

- Coordinator: `7f79d593-ecaa-4537-bca6-60ee2dbdfbe4`.
- Current Meerkat writer: `928b363b-6345-4790-aa9c-1993461a1b72`;
  original app handle `7d5b0bf3-b7ab-4e66-b389-80231f45a527`.
- Its checkout is
  `/Users/luka/src/copilot-worktrees/meerkat/luka-crnkovicfriis-abk-fuzzy-barnacle`.
- Upstream correction owner: `94909ee0-20bc-4c60-b8d9-449c73374d12`;
  checkout
  `/Users/luka/src/copilot-worktrees/oai-rt-rs/luka-crnkovicfriis-abk-symmetrical-fortnight`.
- Old Meerkat writers `38621a57-e254-4944-ba6e-ca2fa6234cf9` and
  `540e9bc2-2927-4867-992e-708eb213fa8a`, and original library owners, are
  preserve-only. Never resume, edit, build, or archive them.

A remote worker must first verify the exact head/tree and receive explicit sole
writer ownership. The existing app account blocker may also prevent provisioning
a GitHub-hosted coding session. No remote successor has been started.

## Approved contract and evidence

The full approved V3.1 scope remains 191 spec IDs, 99 tasks, and F01-F29.
The historical 8/31 Gate0 snapshot is not current completion.

Original artifacts are under
`/Users/luka/.copilot/session-state/540e9bc2-2927-4867-992e-708eb213fa8a/files/`:

| Artifact | SHA-256 |
|---|---|
| `public-live-design.md` | `7578e4d099333193964ea50ff55840fcf65f8f9ed07172964e5a84fbcd4b5a35` |
| `.rct/spec.yaml` | `eba92ea83a36e3952ec102b33d56da71292b61c997b125a85992580696f63c32` |
| `.rct/plan.yaml` | `2c43ec6b344d3c00f5979b87096a2e616c17ee0025b6fb45eec6f2d4eea90129` |
| `.rct/checklist.yaml` | `8cd8061db4efec821f0a2c0f0241ea28200ed4bc58666111512fc534d1b1a5d6` |

Do not rewrite those immutable files. Explicit later coordinator/user
adjudications govern the extensions described below.

Current writer artifacts are under
`/Users/luka/.copilot/session-state/928b363b-6345-4790-aa9c-1993461a1b72/files/`:

| Artifact | SHA-256 |
|---|---|
| `public-live-current-writer-handoff-188.json` | `dfc992eb2ebc4a943725dc466a4f3dd31039638b0c886cb910a5988ca1ad8add` |
| `public-live-current-writer-handoff-189.json` | `8ce9bb1a0a182a51f8d154a5e26c04c6cca4a907364d9a37afc58fd7d05423aa` |
| `public-live-compaction-delta-190.json` | `ebc62457bc2eb2769485981b59cb5287f66d0a1a9c88ba4f33c974b93b15df77` |
| `public-live-writer-delta-205.json` | `0010f60d86debc837647990997bb48f7b985b723f02676c8fde9ed3dd372e31e` |
| `public-live-handover-delta-206.json` | `aff9a7559284e4fc8867c10d9736b7b45c20c583f1d8c7453a31df280d3e7ce8` |

Read delta206 first, then delta205 and the chain as needed. Delta205's statement that the freshness
proposal is unapproved is historical: the explicit adjudication below supersedes
that field. Delta190 supersedes delta189's unvalidated adapter status.

## Implementation boundary

Request owner version 25 and Transcript owner version 7 were current at delta205.
Significant real paths are implemented, but they do not yet constitute the
shipping public feature:

- Independent generated Live ledger, exact source reservation/admission,
  singleton staging, scoped physical model/tool claims, callback application,
  cancellation, held/unknown recovery, and archive ingress fencing.
- Public model catalog and neutral factory/provider adapters, distinct from
  private/legacy Realtime. No ordinary text model fallback.
- Exact transcript/usage accounting, per-channel reader ownership, same-pump
  drain, owned external close handoff, retained-adapter retry, and prevention
  of duplicate physical close.
- Identity-validated adapter observations, native provider-start and bounded
  nonterminal diagnostic receipt ownership, measured CAS, and direct pump
  ingress. Notifications are wakeups, not readiness/permission authority.
- Per-invocation `reserve_and_admit_client_source` composes the existing
  source-first freeze and ordinary admission. Replay/refusal/cancellation does
  not mint another executable handoff.

The stock public profile, full transport delegation/function/output/continuation
pipeline, and complete direct RTC lifecycle are NOT shipping or fully enabled.
Do not turn on the profile merely because a lower-level test passes.

### Post205, unvalidated attempt206

Current source adds coherent `ConfigDocumentObservation` /
`EffectiveConfigObservation`, a filesystem one-read implementation, activation
presence digests, and profile document-set observation. Existing legacy
`effective_config` behavior remains; unsupported coherent sources fail explicitly.
An optional facade `sha2` dependency was added.

Attempt206's metadata -> BUILD/MODULE refresh -> tests command was interrupted.
The retained lock log stops during Bazel startup and there is no test log.
Offline `check_bazel_module_lock_inputs.py` passed 52 recorded inputs on the
checkpoint, but that is NOT a completed Bazel lock gate or test pass.
Inspect the existing logs; resume only missing work, with no overlapping builds.
This observation slice is neither validated nor complete runtime currentness.

**First known repair after exclusive remote ownership is established:**
`meerkat-core/src/config_store/observation.rs` accidentally nests its new
`#[cfg(test)] mod tests` inside `observe_effective_config`'s `while` loop, around
line 91. Move it to module scope before validation. This is an explicitly
unfixed issue, not a test pass or permission to modify the held laptop checkout.
Delta206 includes all 13 post205 source/manifest/BUILD/lock hashes, the retained
metadata/lock log hashes, and the explicit source/index/ref/build stop receipt.

## Explicit currentness adjudication

Disk files are candidate observations, not a second permission owner. External
edits become runtime-effective when the owning host observes and durably
publishes/adopts/revokes them through the existing generated LiveRequest owner.
No instantaneous external-editor-write guarantee is claimed.

The approved implementation must:

1. Bind the complete observed realm chain, document presence, coherent per-file
   typed/raw material, profile, and activation. Do not fabricate an atomic
   multi-file read or pair typed values with different raw-presence bytes.
2. Observe/reobserve around awaited resolution at required new
   admission/effect/send interactions, not only at startup or manual refresh.
3. CAS the exact preparation-time owner generation/expected state. A stale
   candidate must not rebase onto a newer generation and resurrect revocation.
4. Fail explicitly on read, parse, coherence, or expiry failure; never reuse
   cached permission as a success fallback.
5. Publish the required durable fence/update before cooperating mutation,
   refresh, or revoke paths acknowledge runtime-effective success.
6. Preserve final physical/dequeue/expiry/executor/policy fences, source-first
   replay, cancellation, and actual/unknown effect outcomes. A started effect
   is never retroactively relabeled NotStarted.

Required tests include edits during awaited auth, stale publication losing to
revocation, acknowledged revocation blocking queued claims, external edits after
the final observation becoming effective at the next adoption, ancestor/presence
changes, malformed/read failures, expiry, cancellation/publication failure, cold
restart, and local/placed owner config divergence.

This clarifies the approved design's document reread / trusted reload contract
and durable effect-claim linearization; it is not a new manager or gate waiver.

The other explicit content clarification permits RefusedEmpty for a complete,
nonzero accepted whitespace-only interval, retaining its exact observations and
spent frontier. Missing/gapped/partial evidence is never Empty.

## Formal and failure obligations

TLC has a HARD 1200-second wall-clock maximum. The shared runner was independently
verified to kill/reap exact children and fail nonzero on timeout. Do not extend
the limit, automatically retry, escalate unbounded depth, or treat timeout as
PASS. Historical xtask71776/Java71873/wrapper71775 were stopped and confirmed gone;
that oversized attempt is interrupted, not green, and must not be restarted.

The writer found a current formal nonvacuity gap: LiveTranscript CI
`NatValues={1}` and Deep `{0,1,2}` cannot activate voice controls requiring more
than two slots. Passing that profile does NOT prove the new controls. Sound,
nonvacuous bounded witnesses/formal checks remain required.

All retained nextest LEAK results remain UNCLEAN. No root cause has been proved.
Excluding them from development selections is not qualification. In particular:

- Attempt172 reused-tool-ID native factory/core case.
- Attempt178 generated settlement without actual completion record.
- Attempt197 `arithmetic_fails_closed_in_either_dimension`.
- Attempt198 `numeric_maximum_value_is_not_a_substitute_for_maximum_encoded_width`.

There are additional earlier failures recorded in the owner logs. Do not replace
their evidence with a clean unrelated rerun, disable capture/leak detection,
change timeouts to hide them, or claim all assertions passed means a clean run.
Strict minimal-feature warnings and the complete native/WASM/SDK matrix remain
separate obligations.

## Upstream library correction and external PR blocker

Original `oai-rt-rs 0.5.0` at `90118cebd3442a1caa1071dee1954f00b30f959f`
was released and independently verified. Do not replace or retag it.

Correction `b6ff6d5` fixes driver session identity binding/fencing and the
cancelled-close -> falsely clean receiver EOF defect found in independent review.
It passed retained independent review and root strict deterministic qualification:
246 Rust tests, 2 doctests, 6 Python tests; 2 preexisting billable opt-ins ignored.
The owner also ran four fresh bounded actual-provider probes: primary WS, WS fork,
RTC, RTC fork, with explicit identity, media/caption, and final-usage receipts.
Those prove library surface confidence, NOT Meerkat's four required quadrants.

The b6ff local package named `0.5.0` is only a candidate artifact. It must never
replace the released archive. The correction is pushed, but NOT released.
Meerkat remains on the published dependency; no private path/git override or
unpublished API consumption is authorized.

Git push works through the existing personal `lukacf` keyring using a
per-command helper. Copilot's native PR tool still uses an Enterprise Managed
User and returns HTTP403 even after the branch exists. The actual retry request
was `FADF:33F12A:131CC9D:196392F:6AA78D47`. No PR or exact-head CI existed at last
readback. The tool gave no shell-PR fallback authorization.

A legitimate app account relink, or a user-created actual PR, is needed.
Do not create the PR through a shell/browser workaround, reuse the original
library's narrow fast-forward exception, change global authentication, or
copy/display credentials. The user was unavailable when asked to repair the
integration. Once repaired, resume the same correction owner for actual PR/CI,
then obtain release authorization and verify the new registry artifact/consumer.

## Build and release discipline

Use Make for normal repo lanes and `./scripts/repo-cargo` for targeted Cargo.
Do not transfer laptop build caches. The existing local manual target belongs
only to writer928; authority/credit review caches are reserved.

After manifest/lock changes, refresh the actual Bazel metadata and module lock.
Stage freshly generated outputs before `make SHELL=/bin/bash agent-gate`:
SDK freshness compares the INDEX, not HEAD. Run normal hooks and qualified gates
before ordinary final commits; the airplane snapshot exception does not persist.

Remaining delivery includes all real provider/factory/surface activation,
callbacks/descendants/auxiliary paths, source-first delegation, function batch and
result/continuation/context ownership, config propagation and direct RTC cleanup;
all F01-F29, native/WASM/browser/SDK/governance, four actual Meerkat provider
quadrants, full candidate TurboS with all attempts retained, 100+ retirement
cycles, final adversarial PASS, and an actual exact-head-green mergeable PR to
main. Integrate the advanced baseline normally, preserving its private,
ordinal, and cache fixes.

Only then merge as authorized, run the canonical Meerkat release, implement and
qualify the matched MobKit consumer release, and verify both registries/artifacts.
Meerkat0.8.37 and MobKit0.8.34 are completed separate baselines, not GPT-Live
feature proof. Never retag or fold this feature into those releases. The publisher
requires the exact MAIN-PUSH semver artifact, not a manual preview.

Keep the coordinator continuation automation installed. Its instructions must
respect this off-laptop handover hold rather than repeatedly restarting local
implementation. Do not mark the overall task complete until both actual feature
releases are verified.

## Portable artifact bundle

`GPT-Live-remote-handover.tar.gz` accompanies this document. It contains the
approved design/spec/plan/checklist, writer handoff chain through206, all 678
top-level `public-live-*.log` development logs retained by writer928, selected
upstream correction/review/provider receipts, and a SHA-256 manifest. Build
caches, credentials, recordings, and the 3 MB Cargo metadata dump are excluded.
The metadata dump's hash and original local location remain in delta206.

Sources are already on the two named Git branches; the bundle is evidence and
instructions, not a replacement source tree. Extract it into an artifact
directory, verify `MANIFEST.sha256`, verify the repository heads/trees, and only
then begin the controlled remote continuation. All original laptop artifacts
remain preserved. Do not treat unpacking the bundle as authorizing a second
concurrent writer.
