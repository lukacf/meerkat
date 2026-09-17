# Async voice context: urgent WIP handoff

Saved at the user's request before anticipated internet loss. This checkpoint is
**incomplete and unqualified**. The user authorized `--no-verify` for the push,
not a merge, release, version bump, or another paid run.

## Preserved checkpoints

- Async branch: `luka-crnkovicfriis-abk-async-voice-context`.
- Exact failed paid-run source: `73a0b869afb872d16b39d9de2ef5bd74e1c4f8a1`.
- Its backup ref: `async-context-paid-73a0b869a`.
- Ancestry: async -> context-first `55f0a1f6` -> human `a3bd77d7` ->
  repair `2f04d8bf`; original `d7318919` is preserved.
- Earlier qualified checkpoints are retained locally as
  `async-context-checkpoint-4b95f72a1` and `async-context-family-2c4a05abe`.
- No release was performed. The new seams are development work, not a claim
  about the already-published v0.8.39 release.

## What was qualified at 73

The opt-in `LiveContextBootstrapMode::Concurrent` opens actual media independently
of bounded caller summary generation; `BeforeOpen` remains the default. Reserved
canonical history is separate from provider-acknowledged coverage. Summary and
causal reassertions use native quiet thinking, not instructions or commentary.
Receipt-validated custody exposes preparation status separately from media state.
Existing member identity and text model remain unchanged.

The 73 family passed 164 focused combined tests, 15 offline harness contracts,
strict native/WASM Clippy, canonical generation/authority/lock gates, bounded TLC,
and rebase review. Earlier checkpoint 4b95 passed the full Make gate (6037 tests).
Disk exhaustion interrupted one lint run; only verified idle task-owned build
lanes were cleaned, and the exact lint command subsequently passed.

## Paid S99: failed, no retry authorization

Exactly one authorized attempt ran on frozen 73, with synthetic WAV input and no
microphone. It failed after 143.36 seconds at the first fresh historical-vault
recall after summary acknowledgement.

Reached: actual active media and decoded non-silent voice while generation was
held; pre-summary typed and spoken correction setting; call-linked `pwd`, exit 0,
correct project directory; real summary generation (3178 ms, 4738 input / 217
output tokens, 580 bytes); provider thinking acknowledgement.

Not reached: post-summary Cobalt/Marigold retention, second-job close, third-job
late-completion isolation. Do not report those as real-media passes.

The log reported 69 thinking acknowledgements, but that does **not** establish
the recall failure's cause. Raw expected phrase, captured snapshot, summary text,
fragment payloads, and failing native input/output text were not retained.
The exact synthetic TempDir prefix had no remaining directories. Therefore
omitted summary fact, ordering, provider answer, and speech/oracle normalization
cannot be retrospectively distinguished.

Redacted paid log on the original machine:
`/Users/luka/.copilot/session-state/ddf8be20-2d38-47d0-a808-140f4b03e666/files/s99-paid-73a0b869.log`.
No further paid calls, threshold relaxation, canned summary, fake user turn, or
generic-notice promotion is authorized.

## Current unqualified changes

1. **Confirmed independent lifecycle defect:** a generated RED test,
   `bootstrap_ack_does_not_reassert_fresh_already_heard_live_output`, proves that
   73 reasserts fresh post-ACK native output because it tests preparation-map
   presence for the channel lifetime. This is not proof of the paid failure cause.
2. **Approved observation-provenance repair, still in progress:** generated
   MeerkatMachine owns channel/incarnation-scoped, replay-stable observation
   admission order and the exact summary-ACK cut. Native admission and ACK-cut
   recording must be ordered before asynchronous fanout, without source/store/
   model I/O or control-consumer waits. Core/SessionDocument/sidecars carry opaque
   provenance only; delayed pre-ACK materialization must still reconcile, while
   fresh post-ACK output must not echo.
3. **Main-side wiring is WIP:** GPT adapter intake/segments, bootstrap ACK routing,
   `LiveTranscriptIdentity`, shared projection, and RPC projection forwarding.
   It has not been compiled against the final Core/runtime contracts. Potential
   remaining work includes wrapper routing, durable replay, argument alignment,
   default/no-OpenAI forwarding, race tests, and generated artifacts.
4. **Test-only evidence journal:** `gpt_live_evidence.rs`, S99/browser support,
   and a provider recorder gated by the existing `test-realtime-fixtures`
   feature. The evidence owner reported 21 harness and 37 provider offline tests
   plus focused Clippy passing before the latest combined carrier changes.
   Revalidate the complete WIP; do not transfer that result to untested edits.

The journal is intended to survive failure/cancellation outside TempDir under
`target/e2e-live-audio-artifacts/s99/<run>/journal.jsonl`, recording bounded typed
synthetic facts/witnesses, real summary, ordered thinking fragments/ACK IDs,
native transcript timestamps, audio windows, stages, and local scope ordering.
It must fail visibly on capacity loss and redact known credentials. Never retain
raw HTTP/headers/auth, SDP, capability receipts, token stores, reasoning, or
unrelated transcripts.

## Required continuation

- Finish and compile the owner/Core provenance contracts and all facade/RPC
  forwarding; preserve readable V1/V2 sidecars as explicitly unsequenced, never
  synthesize ordinal zero or derive order from canonical indices/strings/time.
- Preserve replay identity, deferred rows, ACK/intake races, wrong-channel and
  stale-incarnation rejection, close/reopen fencing, strict mode, and unmeasured
  transcript provenance. Core must not own optional bootstrap policy.
- Regenerate canonical machine/protocol artifacts and run targeted Core,
  Session, Runtime, facade/RPC, sidecar, WASM, parity, and governance gates.
- Independently review the actual repair and evidence journal. Produce a new
  immutable qualified full-family SHA before requesting any new paid attempt.
- Root's consumer may need exact compiled default/no-OpenAI carrier forwarding;
  shared `ServiceLiveProjection` consumers inherit the main implementation.
  Do not ask Root to port uncompiled guesses.
- Publication beyond this emergency WIP preservation remains coordinated.
  No merge/release/tag/version change is authorized.

## Coordination and operational notes

- Coordinator: `f760343a-5dc5-4e49-a23a-7648416e1b11`.
- MobKit Root: `074d7dbf-8bf3-45e6-b888-997f540693b4`.
- Runtime/Core worker: `731e7e55-239e-4eff-932b-7091c6acdae4` (freeze requested).
- Evidence worker: `e1202c85-54d4-4b36-bdc8-4a1d9ac88280` (reported quiescent).
- Session artifacts and qualification logs:
  `/Users/luka/.copilot/session-state/ddf8be20-2d38-47d0-a808-140f4b03e666/files/`.
- Use Make or `./scripts/repo-cargo`; keep lanes isolated. `make SHELL=/bin/bash`
  avoids the local default-shell process-substitution issue.
- Use personal GitHub identity via `env -u GH_TOKEN -u GITHUB_TOKEN gh ...`.
  Never print or persist credentials. The approved future paid wrapper maps the
  inherited `OPENAI_API_KEY_OLD` into `OPENAI_API_KEY` for that command only and
  uses `GPT_LIVE_E2E_EXECUTOR_MODEL=gpt-5.5`; it is not retry authorization.
- Do not touch Root's demo, port 54040, other worktrees, or preserved binaries.
