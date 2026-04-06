# ESP32 Contract Probe

This harness is the internal Phase 0 probe entrypoint described in the embedded ESP32 planning dossier. It exists to close Meerkat-side and board-side assumptions before the embedded surface lands.

## Current scope

- `host-core-check` is live now. It runs targeted repo tests and emits explicit markers for:
  - canonical factory and session build path
  - provider request-shaping and incremental stream-model expectations
  - runtime-authority expectations around reset, stop, and queued input handling
- `meerkat-baseline` is live now. It captures the current pre-Phase-1 facade-linked embedded baseline, including:
  - the current ESP-toolchain MSRV mismatch against the Meerkat workspace
  - the first target-side compile blocker for the OpenAI provider path on `xtensa-esp32s3-espidf`
  - dependency evidence for the current `reqwest -> rustls -> ring` and `tokio -> signal-hook-registry` stack
- `single-node --host-sim` is live now. It exercises the marker protocol and parser without claiming hardware closure.
- `single-node --hardware` remains a board/config preflight lane.
- `single-node-rust-stack --hardware` is live now. It builds, flashes, monitors, and validates a real on-device provider turn on an ESP32-S3 Rust firmware lane.
  - Current confirmed success path: board boot, LCD status, Wi-Fi, SNTP time sync, HTTPS, and a real streamed OpenAI turn.
  - Current confirmed Meerkat success path: a real provider-backed Meerkat mini-surface turn now completes on-device on the 8MB-PSRAM ESP32-S3 board.
  - Current confirmed runtime boundary: the passing Meerkat lane uses a timer-free current-thread Tokio runtime and a real non-streaming Responses call, while the direct provider streaming proof remains separate and still passes on-device.
- The firmware also has a temporary `PARK_ONLY=1` control path for Phase 0. It exits before Wi-Fi and provider setup so the board can be parked between experiments without burning provider tokens.
- `negative` verifies that the marker parser rejects an invalid sequence.

## Current Phase 0 findings

- The 8MB-PSRAM ESP32-S3 baseline board can boot the Rust `std` lane, drive the LCD, join Wi-Fi, sync time, and complete a real streamed provider turn.
- The Meerkat mini-surface spike can cross-compile and link real `meerkat-core`, `meerkat-client`, `meerkat-hooks`, `meerkat-tools`, and `meerkat-skills` into the firmware.
- The Meerkat mini-surface now completes a full provider-backed turn on hardware with `MKT:MEERKAT:RUN_DONE`, `MKT:RUST_STACK:OK`, and `MKT:SINGLE_NODE:PASS` in the serial transcript.
- Outer worker-thread stack allocation is constrained on the real board: `262144`, `196608`, and `163840` byte stacks fail to spawn with `Not enough space`, while `131072` bytes spawns successfully in the current firmware lane.
- `tokio::runtime::Builder::new_current_thread().enable_time().build()` is rejected by live evidence on-device: it panics with `LoadProhibited` inside `tokio::runtime::scheduler::current_thread::CurrentThread::new`.
- `tokio::runtime::Builder::new_multi_thread().worker_threads(1)` avoids that specific crash, but it immediately aborts because Tokio cannot spawn its own worker thread on the board under the tested envelope.
- `tokio::runtime::Builder::new_current_thread().build()` does succeed on-device without timers. The firmware reaches Meerkat bootstrap, dispatcher construction, skills, hooks, OpenAI client wiring, event channel setup, and `AgentBuilder::build()`.
- The first runtime-dependent failure after that is explicit: hooks invoke `tokio::time::timeout`, and the run aborts because timers are disabled. That means the problem is no longer “Tokio current-thread cannot exist at all”; it is “the timer-enabled current-thread runtime path is broken on this target, and the multi-thread fallback cannot spawn under the measured envelope.”
- The passing workaround for the Meerkat lane is now explicit: keep the direct streaming proof for the board/provider contract, but use a real non-streaming Responses call for the embedded Meerkat turn itself on the current known-good runtime shape.
- The measured headroom is tight but informative. Just before runtime bring-up the firmware typically has about `256k` free heap. After building the mini-surface agent, free heap falls to roughly `65k`, with low-water marks around `61k`.
- In the passing end-to-end Meerkat run, the probe recorded `meerkat_heap_before=256776`, `meerkat_heap_after=121512`, and `meerkat_heap_min_after=47616`.
- The best current interpretation is not “Tokio does not work on ESP32-S3.” It is “Tokio works as a hosted ESP-IDF and FreeRTOS subset with real thread, timer, and memory constraints.” The probe should keep labeling failures by class instead of treating Tokio as a single binary verdict.
- USB CDC and monitor behavior can be unstable across resets. Re-discover the active serial port after disruptive resets or failed monitor sessions instead of assuming the old port path is still valid.

## Commands

```bash
./scripts/live_smoke/esp32-contract-probe/run --mode host-core-check
./scripts/live_smoke/esp32-contract-probe/run --mode meerkat-baseline
./scripts/live_smoke/esp32-contract-probe/run --mode single-node --host-sim
./scripts/live_smoke/esp32-contract-probe/run --mode single-node --hardware
./scripts/live_smoke/esp32-contract-probe/run --mode single-node-rust-stack --hardware
./scripts/live_smoke/esp32-contract-probe/run --mode negative
python3 scripts/live_smoke/esp32-contract-probe/test_esp32_contract_probe.py
```

## Local inputs

Use environment variables or command-line flags for local runtime inputs. Do not commit secrets.

- `ESP32_PROBE_PORT`
- `ESP32_PROBE_WIFI_SSID`
- `ESP32_PROBE_WIFI_PASSWORD`
- `ESP32_PROBE_PROVIDER`
- `ESP32_PROBE_PROVIDER_API_KEY`

Provider-specific environment variables are also accepted for `openai` and `anthropic`.

## Operational notes

- When a partially successful probe would otherwise keep calling the provider, rebuild or flash with `PARK_ONLY=1` before continuing work.
- If `espflash` monitor fails after a reset, do not assume the firmware is broken first. Re-probe the active USB CDC port, then retry flash or serial capture.
- Treat successful compile and link results as weaker evidence than an on-device run. On this target, the hosted runtime shape matters as much as the crate graph.

## Artifacts

Runs write evidence under `artifacts/embedded-esp32/phase-0/<mode>/<timestamp>/`.

- `run.log`
- `summary.json`
- `marker_report.json` for marker-validated modes
- `commands/*.log` for wrapped repo tests
- `board_probe.log` for hardware preflight modes
