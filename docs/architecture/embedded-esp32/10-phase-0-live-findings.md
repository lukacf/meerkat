# Phase 0 Live Findings

This file records the real-hardware findings from the Phase 0 spike on the ESP32-S3 baseline board.

It is the authoritative Phase 0 results log for the embedded dossier. Planning assumptions remain in [02-external-contract-matrix.md](./02-external-contract-matrix.md), and the longer-lived technical consequences remain in [09-technical-dependency-analysis.md](./09-technical-dependency-analysis.md).

## Baseline board and environment

| Item | Value |
| --- | --- |
| Board class | ESP32-S3 |
| Flash | 16MB |
| PSRAM | 8MB embedded PSRAM |
| USB mode | Native USB-Serial/JTAG |
| Wi-Fi | `L&L` |
| Provider lane | OpenAI Responses API |
| Successful Meerkat model | `gpt-5.4-mini` |

## Canonical evidence artifacts

The strongest proof artifacts from the successful Phase 0 runs are:

- [artifacts/embedded-esp32/phase-0/manual-comms-20260405-082914/serial-live.log](../../../artifacts/embedded-esp32/phase-0/manual-comms-20260405-082914/serial-live.log)
- [artifacts/embedded-esp32/phase-0/manual-comms-20260405-082914/host-live.log](../../../artifacts/embedded-esp32/phase-0/manual-comms-20260405-082914/host-live.log)
- [artifacts/embedded-esp32/phase-0/manual-comms-20260405-082914/summary.json](../../../artifacts/embedded-esp32/phase-0/manual-comms-20260405-082914/summary.json)
- [artifacts/embedded-esp32/phase-0/manual-comms-verify-20260405-083253/serial-live.log](../../../artifacts/embedded-esp32/phase-0/manual-comms-verify-20260405-083253/serial-live.log)
- [artifacts/embedded-esp32/phase-0/manual-comms-verify-20260405-083253/host-live.log](../../../artifacts/embedded-esp32/phase-0/manual-comms-verify-20260405-083253/host-live.log)
- [artifacts/embedded-esp32/phase-0/manual-comms-verify-20260405-083253/summary.json](../../../artifacts/embedded-esp32/phase-0/manual-comms-verify-20260405-083253/summary.json)
- [artifacts/embedded-esp32/phase-0/manual-comms-postwake-20260405-143020/serial-live.log](../../../artifacts/embedded-esp32/phase-0/manual-comms-postwake-20260405-143020/serial-live.log)
- [artifacts/embedded-esp32/phase-0/manual-comms-postwake-20260405-143020/host-live.log](../../../artifacts/embedded-esp32/phase-0/manual-comms-postwake-20260405-143020/host-live.log)
- [artifacts/embedded-esp32/phase-0/manual-comms-loopmark-20260405-143732/serial-live.log](../../../artifacts/embedded-esp32/phase-0/manual-comms-loopmark-20260405-143732/serial-live.log)
- [artifacts/embedded-esp32/phase-0/manual-comms-loopmark-20260405-143732/host-live.log](../../../artifacts/embedded-esp32/phase-0/manual-comms-loopmark-20260405-143732/host-live.log)
- [artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-195045/serial.log](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-195045/serial.log)
- [artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-195045/host.log](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-195045/host.log)
- [artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-195045/summary.json](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-195045/summary.json)
- [artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-195302/serial.log](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-195302/serial.log)
- [artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-195302/host.log](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-195302/host.log)
- [artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-195302/summary.json](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-195302/summary.json)
- [artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-201008/serial.log](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-201008/serial.log)
- [artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-201008/host.log](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-201008/host.log)
- [artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-201008/summary.json](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-201008/summary.json)
- [artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-201303/serial.log](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-201303/serial.log)
- [artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-201303/host.log](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-201303/host.log)
- [artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-201303/summary.json](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-201303/summary.json)
- [artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-201643/serial.log](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-201643/serial.log)
- [artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-201643/host.log](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-201643/host.log)
- [artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-201643/summary.json](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-201643/summary.json)
- [artifacts/embedded-esp32/phase-0/single-node-rust-stack/20260404-195126/serial.log](../../../artifacts/embedded-esp32/phase-0/single-node-rust-stack/20260404-195126/serial.log)
- [artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-000319/serial.log](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-000319/serial.log)
- [artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-000319/host.log](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-000319/host.log)
- [artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-000319/summary.json](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-000319/summary.json)
- [artifacts/embedded-esp32/phase-0/comms-orchestrator/20260404-234826/serial.log](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260404-234826/serial.log)
- [artifacts/embedded-esp32/phase-0/comms-orchestrator/20260404-234826/host.log](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260404-234826/host.log)
- [artifacts/embedded-esp32/phase-0/comms-orchestrator/20260404-234826/summary.json](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260404-234826/summary.json)
- [artifacts/embedded-esp32/phase-0/comms-orchestrator/20260404-231421/serial.log](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260404-231421/serial.log)
- [artifacts/embedded-esp32/phase-0/comms-orchestrator/20260404-231421/host.log](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260404-231421/host.log)
- [artifacts/embedded-esp32/phase-0/comms-orchestrator/20260404-231421/summary.json](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260404-231421/summary.json)
- [artifacts/embedded-esp32/phase-0/comms-orchestrator/20260404-232303/serial.log](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260404-232303/serial.log)
- [artifacts/embedded-esp32/phase-0/comms-orchestrator/20260404-232303/host.log](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260404-232303/host.log)
- [artifacts/embedded-esp32/phase-0/comms-orchestrator/20260404-232303/summary.json](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260404-232303/summary.json)
- [artifacts/embedded-esp32/phase-0/comms-orchestrator/20260404-233011/serial.log](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260404-233011/serial.log)
- [artifacts/embedded-esp32/phase-0/comms-orchestrator/20260404-233011/host.log](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260404-233011/host.log)
- [artifacts/embedded-esp32/phase-0/comms-orchestrator/20260404-233011/summary.json](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260404-233011/summary.json)

The single-node harness wrapper did not persist `summary.json` for the first passing run before the monitor process was terminated, so that serial transcript remains the source of truth for the original closure. The later comms-extension run did persist a full summary bundle.

## What Phase 0 proved on real hardware

The successful on-device run emitted all of the following markers in order:

- `MKT:BOOT:OK`
- `MKT:DISPLAY:OK`
- `MKT:WIFI:OK`
- `MKT:TIME:OK`
- `MKT:TLS:OK`
- `MKT:PROVIDER:STREAM_START`
- `MKT:PROVIDER:STREAM_DONE`
- `MKT:MEERKAT:BOOTSTRAP_OK`
- `MKT:MEERKAT:RUN_START`
- `MKT:MEERKAT:TOOLS_OK`
- `MKT:MEERKAT:HOOKS_OK`
- `MKT:MEERKAT:RUN_DONE`
- `MKT:RUST_STACK:OK`
- `MKT:SINGLE_NODE:PASS`

That means the real ESP32-S3 baseline board can:

1. Boot the Rust `std` firmware lane and drive the 1.9-inch LCD.
2. Join Wi-Fi and complete SNTP time sync.
3. Perform real HTTPS with certificate validation.
4. Perform a real streamed OpenAI Responses call outside the Meerkat mini-surface.
5. Boot a real Meerkat mini-surface using `meerkat-core`, `meerkat-client`, `meerkat-hooks`, and `meerkat-tools`.
6. Complete a full provider-backed Meerkat turn on-device, including one real builtin tool call and hook execution.

## What the Phase 0 comms extension proved on real hardware

The later comms-orchestrator runs extended Phase 0 beyond single-node viability. The real ESP32-S3 board emitted:

- `MKT:COMMS:BOOTSTRAP_OK`
- `MKT:COMMS:RUNTIME_MODE`
- `MKT:COMMS:LISTENING`
- `MKT:COMMS:WAITING`
- `MKT:COMMS:READY`
- repeated `MKT:COMMS:INCOMING`
- repeated `MKT:COMMS:SEND_REQUESTED`
- repeated `MKT:COMMS:SEND_COMPLETED`
- `MKT:COMMS:PASS`

The local host Meerkat emitted:

- `MKT:HOST_MEERKAT:SESSION_CREATED`
- `MKT:HOST_MEERKAT:EXECUTOR_READY`
- `MKT:HOST_MEERKAT:KICKOFF_ACCEPTED`
- repeated `MKT:HOST_MEERKAT:SEND_REQUESTED`
- repeated `MKT:HOST_MEERKAT:TRANSCRIPT`
- `MKT:HOST_MEERKAT:STOP_PROMPT`
- `MKT:HOST_MEERKAT:PASS`

That means the real ESP32-S3 baseline board can also:

1. Hold a runtime-backed keep-alive session open on-device.
2. Accept peer traffic through the standard comms drain path instead of a fake surface loop.
3. Use the built-in `send` tool on both sides to drive the conversation.
4. Sustain a multi-turn Mac-to-ESP and ESP-to-Mac conversation with real OpenAI calls on both sides.
5. Render the peer transcript live on the ESP32 1.9-inch display during the conversation.

The strongest stable long-run comms artifact is [manual-comms-20260405-082914](../../../artifacts/embedded-esp32/phase-0/manual-comms-20260405-082914/summary.json). That rerun reused the repaired 16MB-flash image, kept the standard runtime-backed path, used the owning `meerkat-runtime` comms drain implementation instead of the earlier probe shim, rendered the transcript on the ESP display, and closed with:

- host listen: `192.168.0.197:4220`
- ESP address: `tcp://192.168.0.17:4210`
- host transcript pass at `17` transcript lines
- ESP comms pass with `incoming=17`, `outgoing=17`
- final heap after pass: `8248084`
- final internal heap after pass: `110739`
- final PSRAM-capable heap after pass: `8169376`
- graceful teardown ending in `main_task: Returned from app_main()`

The earlier owning-runtime long-run comms artifact at [20260404-234826](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260404-234826/summary.json) remains important because it proved the owning runtime path before the final stability work. It closed with:

- host listen: `192.168.0.197:4220`
- ESP address: `tcp://192.168.0.17:4210`
- host transcript pass at `17` transcript lines
- ESP comms pass with `incoming=19`, `outgoing=17`
- final heap after pass: `8243956`
- final internal heap after pass: `110739`
- final PSRAM-capable heap after pass: `8165248`

Phase 0 also reran the same repaired image from a fresh board reset and got a second clean pass at [manual-comms-verify-20260405-083253](../../../artifacts/embedded-esp32/phase-0/manual-comms-verify-20260405-083253/summary.json). That rerun is important because it is a reproducibility check, not just a single golden trace:

- host transcript pass at `17` transcript lines
- ESP comms pass with `incoming=30`, `outgoing=17`
- final heap after pass: `8164716`
- final internal heap after pass: `110739`
- final PSRAM-capable heap after pass: `8086008`

The asymmetry in `incoming` versus `outgoing` is real. In that rerun, both agents sometimes emitted more than one `send` call in a single completion, and the host shutdown happened earlier than the ESP’s final drain. That produced some trailing `peer_unreachable` send errors on the ESP side before the device still closed with `MKT:COMMS:PASS`. This is a shutdown-semantics nuance, not a stability or reboot failure.

After the later first-turn-freeze investigation, Phase 0 also produced two additional post-fix validation passes on the repaired runtime wake path:

- [manual-comms-postwake-20260405-143020](../../../artifacts/embedded-esp32/phase-0/manual-comms-postwake-20260405-143020/host-live.log) and [serial-live.log](../../../artifacts/embedded-esp32/phase-0/manual-comms-postwake-20260405-143020/serial-live.log)
- [manual-comms-loopmark-20260405-143732](../../../artifacts/embedded-esp32/phase-0/manual-comms-loopmark-20260405-143732/host-live.log) and [serial-live.log](../../../artifacts/embedded-esp32/phase-0/manual-comms-loopmark-20260405-143732/serial-live.log)

These two runs are important because they validate the same real board image after the runtime wake fix, not the earlier pre-fix baseline. Both runs captured:

- `MKT:HOST_MEERKAT:PASS exchanges=17 transcript_lines=17`
- `MKT:COMMS:PASS ...`
- live LCD transcript rendering on the board
- clean device shutdown ending in `main_task: Returned from app_main()`

The later instrumented run also emitted `MKT:RUNTIME_LOOP:*` markers that proved the owning runtime loop received the wake signal, batched inputs, started the run, and entered `executor.apply(...)` on-device instead of stalling after the first inbound peer message.

Phase 0 then reran the same repaired image from clean resets using a hardened serial-reset harness and stricter peer-chat prompts that required exactly one `send` call per turn. The run at [20260405-195045](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-195045/summary.json) is the cleanest current-image proof:

- host transcript pass at `17` transcript lines
- ESP comms pass with `incoming=37`, `outgoing=16`
- final heap after pass: `8160532`
- final internal heap after pass: `110739`
- final PSRAM-capable heap after pass: `8081824`

The immediately following rerun at [20260405-195302](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-195302/summary.json) reproduced the same repaired runtime behavior after another clean reset while making the shutdown-tail asymmetry easier to see:

- host transcript pass at `17` transcript lines
- ESP comms pass with `incoming=44`, `outgoing=17`
- final heap after pass: `8139724`
- final internal heap after pass: `110739`
- final PSRAM-capable heap after pass: `8061016`
- clean device shutdown ending in `main_task: Returned from app_main()`

That rerun still contains a late host-side `peer_unreachable` error after the device has already emitted `MKT:COMMS:PASS` and begun orderly teardown. This is the same shutdown-tail nuance seen in earlier passes, not a regression in the repaired runtime wake path.

## DRY-limit reruns that also passed on real hardware

Phase 0 then pushed back toward standard Meerkat ownership instead of stopping at the first passing shape.

### Signed peer-auth rerun

The later rerun at [20260404-232303](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260404-232303/summary.json) restored the real peer-identity and envelope-verification path instead of leaving comms in an unsigned probe-only mode.

What changed:

- peer auth was turned back on for both the ESP-side agent and the host-side Meerkat
- the first signed attempt failed with `task pthread` stack overflow
- increasing `CONFIG_PTHREAD_TASK_STACK_SIZE_DEFAULT` to `32768` let the signed path pass

What this proves:

1. The standard signed `meerkat-comms` identity path is viable on the ESP32-S3 baseline board.
2. The blocker was not “signed comms is incompatible with ESP32” but “the default ESP pthread stack was too small for the real ingress/auth path.”

The signed-auth pass closed with:

- host transcript pass at `17` transcript lines
- ESP comms pass with `incoming=16`, `outgoing=10`
- final heap after pass: `8300904`
- final internal heap after pass: `111335`
- final PSRAM-capable heap after pass: `8221600`

### Persistent-session rerun

The later rerun at [20260404-233011](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260404-233011/summary.json) restored the real `PersistentSessionService` path on the ESP side while still keeping storage memory-backed for the spike.

What changed:

- the ESP probe stopped using the ephemeral session service for the long-lived comms lane
- the ESP probe used `PersistentSessionService`
- runtime state flowed through `InMemoryRuntimeStore`
- session storage flowed through `MemoryStore`
- blob storage flowed through `MemoryBlobStore`
- signed peer auth remained enabled

What this proves:

1. The real persistent-session orchestration path can boot and survive a real multi-turn Mac-to-ESP conversation on the board.
2. The current architectural blocker is not `PersistentSessionService` semantics.
3. The real portability issue is the accidental backend and `Send` coupling that had to be loosened to let the standard service compile and run on `espidf`.

The later owning-runtime rerun at [20260404-234826](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260404-234826/summary.json) then proved that the timerless comms-wait and completion-bridge behavior could move back into the owning `meerkat-runtime` crate without reopening the earlier probe-only shim.

Phase 0 then pushed the same idea one step further on the host side. The host probe was moved from the ephemeral session path onto the same `PersistentSessionService` shape with memory-backed `SessionStore`, `RuntimeStore`, and `BlobStore`. That rerun uncovered one real crate bug and one behavioral boundary that was later closed:

- a real owning-crate feature-graph bug in [meerkat-session/Cargo.toml](../../../meerkat-session/Cargo.toml), where optional `meerkat-store`, `redb`, and `meerkat-runtime` dependencies had drifted under the `espidf` target table instead of the global dependency table
- an initially behavioral rather than transport-level limit in the host-persistent conversation rerun

The first attempted host-persistent run at [20260405-000830](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-000830/host.log) is not valid evidence of a Meerkat limit because the run also had an empty serial log and a stale host listener already occupying the bind address. After that was cleaned up, the later rerun at [20260405-001233](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-001233/host.log) and [serial.log](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-001233/serial.log) proved a stronger but still incomplete result:

- the host-persistent path compiles and boots
- the host session creates successfully
- the host accepts the kickoff input and sends a real peer message
- the ESP accepts that peer message and completes real provider-backed turns

The stronger rerun at [20260405-001828](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-001828/summary.json) then closed the host-persistent DRY path on real hardware:

- host transcript pass at `17` transcript lines
- ESP comms pass with `incoming=18`, `outgoing=17`
- signed comms still enabled
- ESP display transcript still enabled

That means the host-persistent path is now part of the proven baseline for the Phase 0 comms harness rather than an unresolved branch.

## Final repeated-validation closure for the repaired wake and teardown paths

The final runtime-side fix set has two parts:

1. the `espidf` runtime wake path in the owning [meerkat-runtime/src/session_adapter.rs](../../../meerkat-runtime/src/session_adapter.rs) now uses awaited `send()` instead of opportunistic `try_send()`;
2. the host harness teardown now uses the owning comms `DISMISS` message semantics and keeps the listener alive long enough for the ESP peer to drain late queued work.

Those repairs were validated repeatedly from clean resets on the same flashed image. The strongest repeated-validation artifacts are:

- [20260405-201008](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-201008/summary.json)
- [20260405-201303](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-201303/summary.json)
- [20260405-201643](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-201643/summary.json)

Across those clean-reset reruns, the board repeatedly produced:

- `MKT:COMMS:READY`
- repeated `MKT:COMMS:INCOMING`
- repeated `MKT:OPENAI:REQ_START`
- repeated `MKT:COMMS:SEND_COMPLETED`
- final `MKT:COMMS:PASS`

And the host repeatedly produced:

- `MKT:HOST_MEERKAT:READY`
- repeated `MKT:HOST_MEERKAT:SEND_REQUESTED`
- repeated `MKT:HOST_MEERKAT:TRANSCRIPT`
- `MKT:HOST_MEERKAT:PASS`
- `MKT:HOST_MEERKAT:DISMISS_SENT`

This matters because it closes two earlier ambiguities:

1. the first inbound peer message no longer freezes the ESP conversation before the first provider request;
2. the harness no longer relies on a probe-only shutdown sentinel that the owning comms/runtime path does not understand.

The repeated-validation runs still show that the host can declare `PASS` before the ESP has completely drained all queued work. That asymmetry is now bounded and expected: the host sends the owning `DISMISS`, remains alive for a short grace window, and the ESP closes with `MKT:COMMS:PASS` instead of dying in a tail-send storm. That remaining nuance is a shutdown-policy consideration for later phases, not a Phase 0 correctness blocker.
- the loop can still deadlock if the device agent ends a turn without actually calling `send`

That meant the remaining limit in that DRY pass was not “host-persistent sessions break the comms runtime.” The remaining limit was that the long-lived chat harness was still prompt-sensitive enough that a missed `send` tool call could stall the peer loop even when the runtime, session service, and transport remained healthy.

Phase 0 then tightened the host and device peer-chat prompts so the turn contract explicitly forbade plain assistant-text endings and required one `send` call per turn. With that change, the later rerun at [20260405-001828](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-001828/summary.json) passed on the host-persistent path as well.

What changed:

- the host remained on `PersistentSessionService` with memory-backed `SessionStore`, `RuntimeStore`, and `BlobStore`
- the host remained on the standard `subscribe_session_events()` path
- the device remained on the standard runtime-backed keep-alive and `send` tool path
- the peer-chat prompts were tightened to require exactly one `send` call per turn and forbid plain assistant-text endings

What this proves:

1. The host-persistent DRY rerun is now closed on real hardware, not just compile-fixed.
2. The earlier “dead turn” issue was a prompt-discipline problem in the probe harness, not a transport or session-runtime incompatibility.
3. The stronger Phase 0 evidence now includes a full multi-exchange loop with persistent-session orchestration, signed comms, real API calls on both sides, LCD transcript rendering, and the standard runtime-backed peer path.

The host-persistent pass closed with:

- host transcript pass at `17` transcript lines
- ESP comms pass with `incoming=18`, `outgoing=17`
- final heap after pass: `8246588`
- final internal heap after pass: `110739`
- final PSRAM-capable heap after pass: `8167880`

The persistent-session pass closed with:

- host transcript pass at `17` transcript lines
- ESP comms pass with `incoming=22`, `outgoing=14`
- final heap after pass: `8231468`
- final internal heap after pass: `110955`
- final PSRAM-capable heap after pass: `8152544`

## The successful Meerkat Phase 0 shape

The passing run did not use the unmodified desktop runtime shape. The shape that succeeded on hardware was:

- timer-free `tokio::runtime::Builder::new_current_thread().build()`
- a dedicated outer worker thread with `131072` bytes of stack
- a reduced one-tool probe surface using `datetime`
- `gpt-5.4-mini`
- a real provider-backed OpenAI Responses call with `stream: false` for the Meerkat lane
- a separate direct streaming proof still retained in the firmware before the Meerkat mini-surface begins

This is a valid Phase 0 result because the purpose of the spike was to find the real stability boundary and record the workarounds needed to cross it on hardware.

The later long-run comms extension used a slightly different but related passing shape:

- timer-free `tokio::runtime::Builder::new_current_thread().build()`
- explicit PSRAM enablement in the ESP-IDF config, including SPIRAM-backed heap and Wi-Fi or lwIP preference for SPIRAM
- corrected `16MB` flash geometry with a large-enough factory app partition for the actual firmware image
- a dedicated outer worker thread with `65536` bytes of stack
- no extra nested ESP-side runtime thread for the keep-alive comms lane
- the standard runtime-backed keep-alive path plus the standard comms `send` tool
- `gpt-5.4-mini` on both the host Meerkat and the ESP Meerkat
- live transcript rendering on the LCD during the peer conversation
- later reruns restored signed peer auth and persistent-session orchestration while preserving the same standard comms path
- later reruns repaired an intermittent first-turn stall by changing the owning runtime wake path on `espidf` from opportunistic `try_send()` to awaited `send()`

## First-turn freeze root cause and fix

The later on-demand validation uncovered a real intermittent failure mode:

- the host would successfully call the standard `send` tool
- the ESP would log `MKT:COMMS:INCOMING` and render the first peer message on the LCD
- but the runtime loop would sometimes fail to begin the first turn, so no `MKT:OPENAI:REQ_START` appeared and the chat looked frozen on the display

The owning-crate fix lives in [meerkat-runtime/src/session_adapter.rs](../../../meerkat-runtime/src/session_adapter.rs). On `espidf`, the runtime wake path now uses awaited `mpsc::Sender::send(())` instead of opportunistic `try_send(())` for the runtime wake channel. That change applies to:

- executor attachment wake-up after loop publication
- `accept_input`
- `accept_input_with_completion`
- runtime control-plane ingress and recovery wake sites

The later investigation also added temporary ESP-only trace markers in [meerkat-runtime/src/runtime_loop.rs](../../../meerkat-runtime/src/runtime_loop.rs) so the next reruns could prove whether the loop observed the wake, selected a batch, started the run, and entered the executor. The successful instrumented pass at [manual-comms-loopmark-20260405-143732](../../../artifacts/embedded-esp32/phase-0/manual-comms-loopmark-20260405-143732/serial-live.log) shows:

- `MKT:RUNTIME_LOOP:WAKE_RX`
- `MKT:RUNTIME_LOOP:BATCH`
- `MKT:RUNTIME_LOOP:START_RUN_OK`
- `MKT:RUNTIME_LOOP:STAGE_BATCH_OK`
- `MKT:RUNTIME_LOOP:EXECUTOR_APPLY_START`

That evidence closes the first-turn stall as a real runtime wake bug rather than a display-only issue or a peer-tool prompt problem.

## Validation-harness caveats discovered during the fix

Phase 0 also found two harness-level issues that should not be confused with embedded runtime failures:

1. A stale host listener on `192.168.0.197:4220` can make a rerun fail before the host side actually starts the conversation.
2. Some strict validator variants missed valid runs because USB CDC re-enumeration and monitor timing are noisy on macOS after board reset.

Those harness issues do not invalidate the real passing artifacts above. They do mean that future automated validation should prefer the repaired orchestration path or explicitly clear stale listeners and tolerate USB reconnect churn.

## Memory and runtime observations

The successful run recorded the following envelope:

- Meerkat heap before runtime: `256776`
- Meerkat heap after run: `121512`
- Meerkat low-water after run: `47616`
- Outer worker stacks that failed to spawn: `262144`, `196608`, `163840`
- Outer worker stack that succeeded: `131072`
- Direct streaming probe elapsed time: `2533 ms`
- Meerkat mini-surface elapsed time: `5658 ms`
- Meerkat turns: `2`
- Meerkat tool calls: `1`

The important reading is not that the board has abundant headroom. It does not. The important reading is that a real provider-backed Meerkat turn can fit and complete on the 8MB-PSRAM board when the runtime shape is narrowed appropriately.

The long-run comms pass adds two more important readings:

- PSRAM is a real requirement for the successful long-lived lane, not just a safety margin.
- Internal-memory thread allocation is still the first hard ceiling, even after PSRAM is enabled.
- Signed peer auth does fit, but only after the standard ESP pthread stack was raised to `32768`.
- The persistent-session rerun still left only about `110955` bytes of internal heap free at the end of the passing run.

## Long-run stability caveat from the successful comms pass

The successful comms run also emitted repeated `task_wdt` warnings while still eventually closing with `MKT:COMMS:PASS`.

The right interpretation is:

- The long-lived runtime-backed conversation path is now proven.
- The current probe runtime shape is not yet production-stable with respect to watchdog behavior.
- The watchdog warnings specifically implicate the ESP pthread lane on CPU0 starving `IDLE0` during longer stretches of work.

Phase 1 and Phase 2 should therefore treat “task watchdog compatibility under long keep-alive peer traffic” as an explicit architecture item, not as a polish issue.

## Stability follow-up that closed the watchdog regression

Phase 0 kept pushing after the first display-backed long chat because later clean-flash reruns exposed two more real issues:

1. the build or flash path was still using a `2MB` flash assumption and a too-small factory partition even though the actual board is `16MB`
2. the owning hot loops still needed a small amount of ESP-specific cooperative yielding to stay friendly to the task watchdog under sustained peer traffic

The repaired stable rerun at [manual-comms-20260405-082914](../../../artifacts/embedded-esp32/phase-0/manual-comms-20260405-082914/summary.json) closed both issues together:

- the repaired image booted with `SPI Flash Size : 16MB`
- the board reported the large factory partition and 8MB PSRAM pool correctly
- the host completed `MKT:HOST_MEERKAT:PASS exchanges=17 transcript_lines=17`
- the ESP completed `MKT:COMMS:PASS incoming=17 outgoing=17`
- the board exited cleanly with `main_task: Returned from app_main()`
- the serial transcript did **not** show the earlier watchdog reboot pattern on this passing rerun

The owning-crate yield shaping that made the stable rerun pass now lives in:

- [meerkat-runtime/src/comms_drain.rs](../../../meerkat-runtime/src/comms_drain.rs)
- [meerkat-session/src/ephemeral.rs](../../../meerkat-session/src/ephemeral.rs)

The ESP firmware-specific rendering pause remains local to the probe firmware in:

- [scripts/live_smoke/esp32-contract-probe/firmware-rust/src/main.rs](../../../scripts/live_smoke/esp32-contract-probe/firmware-rust/src/main.rs)

This changes the final Phase 0 reading:

- watchdog pressure was a real Phase 0 blocker
- it is no longer an open blocker for the current known-good baseline
- longer soak and production policy still belong in Phase 2, but the baseline long chat is now stable enough to count as a solved Phase 0 run rather than a caveated partial success
- the long-chat baseline is now also reproducible across a fresh board reset, not just a one-off pass on a warmed-up image

## DRY reuse versus remaining special-case shaping

Phase 0 did not stop at a probe-only comms loop. The later reruns deliberately put standard Meerkat machinery back in.

### Reused standard Meerkat paths already proven on hardware

- standard runtime-backed keep-alive semantics
- standard `meerkat-runtime` comms-drain ownership, including target-aware wait and completion handling
- standard `send` tool driven conversation
- standard `meerkat-comms` signed peer identity path
- standard `PersistentSessionService` orchestration
- standard `RuntimeStore`, `SessionStore`, and `BlobStore` seams using in-memory backends
- standard transcript or event-driven peer conversation rendered on the ESP display

### Still spike-specific or platform-shaped

- timer-free current-thread Tokio runtime
- raised `CONFIG_PTHREAD_TASK_STACK_SIZE_DEFAULT=32768`
- PSRAM-backed heap as part of the baseline firmware config
- non-streaming Responses call for the ESP-side Meerkat lane even though direct provider streaming was separately proven
- `espidf`-specific `?Send` accommodations in some async trait implementations
- memory-backed persistent stores only; restart durability is not closed yet
- watchdog warnings under longer keep-alive peer traffic

The key architectural lesson is that the standard Meerkat semantic path transfers much further than expected. The remaining deviations are mostly execution-envelope and backend-shaping issues, not evidence that Meerkat needs a second runtime model.

### Host-persistent DRY-limit rerun

The host-side DRY rerun also restored the more-standard event subscription path by switching from the ephemeral-only raw receiver path to `subscribe_session_events()`. That is now a meaningful reuse win with real-hardware closure:

- host `PersistentSessionService` plus memory-backed stores now compile, boot, and pass
- the host can inject a kickoff input, emit real `send` traffic, and complete the stop sequence
- the ESP can receive that input, complete real provider turns, and close the conversation cleanly
- the remaining lesson is not a broken host-persistent runtime path, but that peer-chat examples need prompt discipline and tests that fail fast when a turn ends without the expected `send` tool call

So the current Phase 0 map is:

- host-persistent orchestration is no longer blocked at compile time
- host-persistent orchestration is now also proven on real hardware in the same bounded peer-chat shape
- the earlier dead-turn boundary was a prompt-discipline issue in the probe harness rather than a transport or session bootstrap failure

### Streaming rerun boundary

After the owning-runtime comms path was passing, Phase 0 reran the same long-lived Mac-to-ESP conversation with `OPENAI_STREAM=1` restored for the Meerkat lane. That rerun is captured in:

- [artifacts/embedded-esp32/phase-0/comms-orchestrator/20260404-235525/serial.log](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260404-235525/serial.log)
- [artifacts/embedded-esp32/phase-0/comms-orchestrator/20260404-235525/host.log](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260404-235525/host.log)

What happened:

- the board booted, joined Wi-Fi, and re-entered the standard runtime-backed comms path
- the host keep-alive Meerkat booted and sent the first real peer message
- the ESP accepted that inbound peer message
- both logs then stopped advancing before the first streamed Meerkat response completed

This is a useful negative finding, not just an operational failure. Direct transport streaming remains proven on-device, but the long-lived Meerkat comms lane still does not inherit that proof automatically. For the current ESP32-S3 baseline, `stream: false` remains part of the known-good Meerkat conversation shape.

Phase 0 then split the streaming control between host and device and reran the same long-lived conversation with host streaming enabled and device streaming disabled. That rerun is captured in:

- [artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-000319/serial.log](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-000319/serial.log)
- [artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-000319/host.log](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-000319/host.log)
- [artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-000319/summary.json](../../../artifacts/embedded-esp32/phase-0/comms-orchestrator/20260405-000319/summary.json)

That split rerun passed with:

- `MKT:HOST_MEERKAT:PASS exchanges=17 transcript_lines=17`
- `MKT:COMMS:PASS incoming=23 outgoing=18`

This narrows the streaming limit precisely:

- host-side Meerkat streaming is compatible with the long-lived peer conversation
- device-side Meerkat streaming is still the unstable boundary on the ESP32-S3 baseline
- the current known-good profile is therefore “host may stream, ESP Meerkat lane stays non-streaming”

## Blockers that were proven along the way

Phase 0 also proved several negative results that must inform Phase 1:

- `tokio::runtime::Builder::new_current_thread().enable_time().build()` is not a safe assumed runtime shape on this target.
- `tokio::runtime::Builder::new_multi_thread().worker_threads(1)` is not a safe assumed fallback under the tested board envelope.
- The current facade-linked build graph still pulls too much optional machinery for trustworthy embedded sizing.
- The raw streaming Meerkat lane was less stable than the non-streaming Meerkat lane, even though direct provider streaming itself worked on-device.
- Restoring `OPENAI_STREAM=1` on the later owning-runtime comms lane caused the conversation to stall after the first inbound peer message. The later split rerun then showed that host-only streaming still passes. So the explicit open boundary is the ESP-side Meerkat streaming lane, not the general peer conversation path.
- The ESP peer could not sustain the long-run keep-alive lane while an extra nested ESP-side runtime thread was still part of the host shape. The successful comms extension required removing that redundant OS-thread layer while keeping the standard runtime and comms semantics intact.
- Host peer address mismatches and stale host listener processes are an easy failure mode in the current probe workflow because the temporary probe still compiles the host peer address into the ESP image.
- Signed peer auth is not the wrong design for ESP32. It is viable once the default ESP pthread stack is treated as part of the platform envelope.
- `PersistentSessionService` is not the wrong design for ESP32. The real issue is accidental desktop-oriented backend and async-trait coupling in `meerkat-session` and `meerkat-store`.
- The host-persistent DRY rerun exposed a real feature-graph ownership bug in `meerkat-session/Cargo.toml`; Phase 1 must treat that style of accidental target-scoped dependency leakage as a first-class cleanup category.

## Phase 1 consequences

Phase 1 should proceed as if these findings are now fixed inputs:

1. Hooks, tools, and skills must become genuinely optional in the facade and transitive feature graph.
2. The provider transport seam is required, not optional.
3. The embedded surface should treat the working executor/runtime subset as a first-class platform constraint.
4. The difference between “direct provider streaming works on-device” and “the Meerkat lane should always stream on-device” is now an evidenced distinction. The later split rerun narrowed that further: host-side Meerkat streaming passes, while device-side Meerkat streaming remains the unstable edge on the ESP32-S3 baseline.
5. The embedded surface should preserve semantics while still allowing target-shaped execution decisions.
6. The standard runtime-backed comms path is viable on the board and now carries the timerless comms-drain logic in the owning runtime crate, but it still needs watchdog-aware shaping and less thread layering on the ESP host side.
7. PSRAM enablement belongs in the embedded baseline itself, not as an optional build tweak.
8. Persistent-session orchestration must be separable from desktop durability backends such as `redb`.
9. The target-facing async trait surface in session or store crates cannot assume desktop-style `Send` behavior by default.

## Operational note

The flash-and-monitor path is good enough for autonomous work, but it is still somewhat fragile around USB reset and monitor lifecycle. The probe should continue to treat serial-port rediscovery and log retention as part of the artifact contract rather than assuming a desktop-stable device path.

Later repeated validation narrowed that operational note further:

- on this board, raw USB reset alone was not deterministic enough for repeated validation
- the hardened harness now prefers a board-specific serial control-line reset and only falls back to broader USB reset behavior when needed
- repeated peer-chat validation also became more reliable once both agents were told that a turn without exactly one `send` call is invalid

Those harness changes do not change runtime ownership or semantic conclusions. They reduce false negatives during repeated hardware validation.
