# ESP32 Contract Probe

This harness is the internal Phase 0 probe entrypoint described in the embedded ESP32 planning dossier. It exists to close Meerkat-side and board-side assumptions before the embedded surface lands.

## Current scope

- `host-core-check` is live now. It runs targeted repo tests and emits explicit markers for:
  - canonical factory and session build path
  - provider request-shaping and incremental stream-model expectations
  - runtime-authority expectations around reset, stop, and queued input handling
- `single-node --host-sim` is live now. It exercises the marker protocol and parser without claiming hardware closure.
- `single-node --hardware` remains a board/config preflight lane.
- `single-node-rust-stack --hardware` is live now. It builds, flashes, monitors, and validates a real on-device provider turn on an ESP32-S3 Rust firmware lane.
- `negative` verifies that the marker parser rejects an invalid sequence.

## Commands

```bash
./scripts/live_smoke/esp32-contract-probe/run --mode host-core-check
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

## Artifacts

Runs write evidence under `artifacts/embedded-esp32/phase-0/<mode>/<timestamp>/`.

- `run.log`
- `summary.json`
- `marker_report.json` for marker-validated modes
- `commands/*.log` for wrapped repo tests
- `board_probe.log` for hardware preflight modes
