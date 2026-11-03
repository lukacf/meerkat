# Console voice: remote-agent handover

Emergency WIP checkpoint, 2026-09-17. The user requested immediate commit and
`--no-verify` push because this computer is losing internet access.
**This is not a completed feature or a release candidate. Keep PRs draft.**

## Start here

- Meta: [PR #408](https://github.com/lukacf/meerkat-meta/pull/408),
  branch `luka-crnkovicfriis-abk-console-voice-mode`.
- All 35 Meerkat dependencies are pinned together to the now-public
  `73a0b869afb872d16b39d9de2ef5bd74e1c4f8a1`, not local filesystem paths.
  Backup ref: `lukacf/meerkat`, branch
  `wip/meta-voice-73a0b869-checkpoint`.
  Another equivalent ref is `luka-crnkovicfriis-abk-voice-qualified-73a0b869`.
- That pin includes recovery/close repair, canonical human input, context-first
  assistant attribution, and concurrent summary bootstrap. **It has a confirmed
  post-bootstrap self-echo bug and failed real S99 acceptance.**
- Newer ordinal/provenance repair and durable test evidence work is **pushed**
  separately at `ddf52762e279f9e58cb4bf21e665dee46aa187ed` on upstream branch
  `luka-crnkovicfriis-abk-async-voice-context` (557adda4 plus two late carrier
  edits; lineage rooted at 73).
  It is incomplete/unqualified; **do not bind Meta to it yet**.
  Read [ASYNC_VOICE_CONTEXT_HANDOFF.md](https://github.com/lukacf/meerkat/blob/ddf52762e279f9e58cb4bf21e665dee46aa187ed/ASYNC_VOICE_CONTEXT_HANDOFF.md).
  The redacted failed-run log is committed there at
  `artifacts/evidence/s99-paid-73a0b869.log`.
- Older original-workspace changes were also preserved, separately, at
  `058ee571793a0169c1366cb71f9312e300c20749` on upstream branch
  `wip/voice-legacy-original-workspace`. Its
  `VOICE_LEGACY_WORKSPACE_HANDOFF.md` identifies it as a historical archive,
  **not** the latest integration or a consumer pin. Do not merge it blindly.
- Latest public release baseline checked: Meerkat **0.8.39**, Meta **0.8.36**.
  These releases do not contain the complete new voice work. Recheck before
  selecting the next pair; do not republish either version.

The user's eventual objective is convincing realistic Live proof, independent
PR review, green current CI, then merge and release the next pairing. The
emergency checkpoint does not waive those gates.

## Implemented in Meta

Voice button and dual real-audio waveforms; independent mic/speaker mute and
close; selected existing agent as background executor; persistent voice target
across navigation; close-before-switch; concurrent text; 15-minute audio-only
silence termination; bounded teardown/recovery; exact authenticated ownership.

New work in this checkpoint:

- Console-only human admission through Meerkat's bounded host-human APIs.
  Actual conversational `Message::User` preserves interaction identity.
  Generic work remains ExternalEvent. Injected memory stays canonical but is
  excluded from the human-input UI projection.
- Identity-first native SSE resolves the authorized core member and captures
  its actual runtime alias under the lifecycle guard. Stale/reset fencing stays
  strict.
- Console opts into `LiveContextBootstrapMode::Concurrent`. Audio can activate
  while a tool-free factual summary is generated using the background model.
- `meta/console/voice/context_status` accepts exactly
  `{identity, request_id, channel_id}` and echoes that scope plus
  `context_preparation`. Stages are capturing/generating/delivering; terminal
  states are not_requested/provider_acknowledged/failed with 13 typed reasons.
  See the shared `crates/meerkat-meta/tests/fixtures/console_voice_v1.json`.
- Browser status observation is independent of media activation, bounded and
  request/channel-fenced. Failures are visible without muting connected audio.
  Replacement/close/dispose cancel observers; late replies cannot affect a new
  call. Context updates never refresh the silence clock.
- Shared live owner (later addition): `console_voice` and `live: true` compose
  one `GatewayLiveContext`; `live_wiring::LiveOwnerArbiter` arbitrates the two
  doors "latest engaged wins" with typed reasons
  (`superseded_by_console_voice`, `superseded_by_external_live`), the console
  error kind `voice_superseded`, readiness `reason: external_live_active` with
  a `holder`, `close_reason` on external status/close, and the stdio
  notification `meta/live/superseded`. See docs/design/live-sessions.md,
  "One shared live owner".

Important files: `console/src/lib/voice-{session,context}.ts`,
`console/src/panels/VoiceBar.tsx`, `crates/meerkat-meta/src/console_voice{.rs,/}`,
`live_wiring.rs`, `http_console.rs`, `identity_first/{runtime,bridge}.rs`,
`console_aggregator/mod.rs`, `mob_handle_runtime.rs`, `unified_runtime/http.rs`,
`console_human_input_tests.rs`, and `tests/agent_events_identity_resolution.rs`.
Generated console/embedded bundles and Bazel metadata are included.

## What passed, and what did not

On the exact immutable 73 source, before converting the equivalent sources
from archive paths to public Git bindings:

- Production `meta_gateway` build with `--features openai-live`.
- 220 targeted Rust integration/unit tests; strict all-target
  `openai-live-test` Clippy; 112 UI tests including Rust/TS golden parity.
- Real local WebRTC without provider calls: actual audio before a held context
  gate is released, ACK/failure states, mute/navigation/text/recovery/cleanup.
- Independent browser and backend reviews: no significant issues.
- These results are **not** current public-PR CI or proof of paid acceptance.
  Last green published Meta checkpoint was `f4028d886bd762ed2333612448d01166c0fa3a8f`,
  CI run `35161886190`. This emergency push skips push hooks by user request.
  Git-source fetch/rebuild was not completed locally before the emergency
  handoff. Lockfile changes only replace the 35 archive package sources with
  the identical public Git SHA; run locked metadata/build on the remote host.

The earlier real console suite passed genuine peer work, canonical typed input
and voiced recall, delayed peer delivery, held real tool work, close, and reopen.
It then failed before a late keeper result reached the reopened call: typed
context triggered speech without a new native user turn and the attribution
guard killed the reader. The context-first repair in 73 fixes that proven
owner bug. **The complete console paid suite has NOT been rerun on 73.**

Exactly one upstream S99 paid attempt on 73 failed after 143.36 seconds:
native non-silent voice was active before the summary gate, linked `pwd`
executed, a real summary completed in 3178 ms (580 bytes), and thinking delivery
was acknowledged. Fresh historical-vault recall then timed out for 90 seconds.
Only metrics/assertions survived: actual summary, expected phrase, command
fragments, and failed ASR/answer text were lost with the temporary/browser state.
**The paid failure's cause is unknown.** Post-summary Cobalt/Marigold retention
and second-job-close/third-job-isolation assertions were not reached.

## Current upstream blocker

An offline test independently proved:
`bootstrap_ack_does_not_reassert_fresh_already_heard_live_output`.
On 73, a fresh AlreadyPresent row after bootstrap ACK incorrectly becomes
`ReassertCausalTail`, because eligibility uses preparation-map presence even
after ProviderAcknowledged. This is a real lifecycle defect, but is not proven
to be the cause of the paid recall failure.

The in-progress repair requires a **generated per-channel observation ordinal**
admitted before async projection, plus an exact summary-ACK cut. Carry opaque
provenance through `LiveTranscriptIdentity`, deferred Core/SessionDocument rows,
and sealed origins. Eligibility remains machine-owned. A simple phase guard
loses pre-ACK speech whose projection/materialization arrives late.
Legacy history must remain readable and honestly unsequenced; do not infer
ordinal zero or derive authority from canonical row position.

The active console uses shared `ServiceLiveProjection`, so upstream handles
its carrier propagation. Root's **default/no-openai-live** compatibility sink
in `live_wiring.rs::ordinary_compat::GatewayLiveProjectionSink` manually
reconstructs `RealtimeTranscriptEvent` and may require forwarding changes.
Wait for exact compiled carrier signatures. The session-service decorators
forward whole events and the exact `&MeerkatMachine`; preserve that.

Before another paid attempt, finish and offline-test bounded durable diagnostic
capture surviving failure/cancellation. Retain synthetic facts, summary text,
owned append/ACK correlations, ASR/answer deltas, aligned audio metrics, and
reached phases. Exclude keys, auth headers, SDP, capability receipts, raw HTTP,
and runtime token stores. Never replace failed assertions with weaker ones.

## Resume sequence

1. Fetch upstream emergency WIP and read its handover. Finish/review the ordinal
   repair and evidence journal; qualify one immutable combined head.
2. Rebind **every Meerkat crate to that one head**, adapt any compiled carrier
   changes, regenerate lock/build metadata, and rerun default plus Live checks.
3. Coordinate one upstream S99 attempt with durable evidence. No automatic paid
   retries. Run the console paid suite only after explicit S99 PASS and matching
   consumer build/tests. Preserve and investigate any failure offline first.
4. Publish portable changes, obtain fresh required PR CI and final review.
   Only then merge/release. Follow the repository release-candidate/promote
   protocol; tags validate rather than building new release bytes.

```bash
export CARGO_INCREMENTAL=0
./scripts/repo-cargo build -p meerkat-meta --bin meta_gateway --features openai-live
./scripts/repo-cargo nextest run -p meerkat-meta --features openai-live-test \
  --lib --test agent_events_identity_resolution \
  -E 'test(console_human) or test(console_voice) or test(identity_first::runtime) or test(console_aggregator) or test(live_contracts) or test(live_wiring) or binary(agent_events_identity_resolution)' \
  --no-fail-fast
./scripts/repo-cargo clippy -p meerkat-meta --features openai-live-test --all-targets -- -D warnings
npm --prefix console run voice:test
npm --prefix console run build
node console/voice-e2e.cjs
node console/voice-e2e-live.cjs --self-test
node console/voice-e2e-live.cjs --self-test-audio
node console/voice-e2e-live.cjs --help
```

The paid command is `npm --prefix console run e2e:voice:live`; do not run it
accidentally. `META_VOICE_GATEWAY_BIN=/absolute/path` selects a prebuilt
matching gateway. Supply approved remote credentials through the environment,
never through committed files. Synthetic WAVs replace microphone capture.

## Operational caveats

- No merge, tag, release, or new paid retry was performed for this checkpoint.
- The old local demo at `127.0.0.1:54040/console` is still the earlier e4 binary,
  not current code. Do not treat it as a valid retest or transfer its credentials.
  Its runtime data was preserved; the old launcher cannot resume a mob it has
  permanently marked Stopped.
- Local 73 binary SHA-256:
  `a51e2eb6907c189fe680325ac056235cf0f220b2fd2a78c8a1f870fa7727e01f`.
  Rebuild on the remote host; local binaries/evidence directories are not in Git.
- Disk exhaustion interrupted one build, then the identical retry passed after
  deleting only audited obsolete local cache artifacts. Source/evidence stayed
  unchanged. Use the repository Cargo wrapper and monitor disk capacity.
- Existing unrelated strict-TS baseline issue: duplicate `session_id` in
  `console/src/types.ts` (lines 95/100 at checkpoint). The new protocol parser
  passes strict TS; the full graph is not claimed clean. No suppression added.
- Injected GitHub credentials on this machine use an Enterprise Managed account.
  Personal operations used `env -u GH_TOKEN -u GITHUB_TOKEN gh ...` and Git's
  `!gh auth git-credential` helper. Do not globally change account settings.
  Native upstream PR creation previously hit 403; publication used personal Git.
