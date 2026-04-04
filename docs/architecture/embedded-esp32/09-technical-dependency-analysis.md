# Technical Dependency Analysis

This file captures platform and toolchain facts that already constrain the embedded plan before live target probing begins.

These items are not repo-current-state claims, so they do not belong in the `CODE_GROUNDED` baseline table. They are also not live-validated target closures yet, so they do not replace the contract matrix. Their job is to prevent the autonomous implementation run from discovering known toolchain and runtime realities too late.

## Repo-grounded dependency pressure

| ID | Basis | Constraint | Evidence | Planning consequence |
| --- | --- | --- | --- | --- |
| TD-LOCAL-001 | `CODE_GROUNDED` | The workspace baseline is `tokio` plus `reqwest` with `rustls-tls`. | [Cargo.toml#L46-L63](../../../Cargo.toml#L46-L63) | The embedded plan must explicitly validate both executor fit and HTTP/TLS backend replacement instead of assuming the desktop stack transfers. |
| TD-LOCAL-002 | `CODE_GROUNDED` | Provider clients own concrete `reqwest::Client` instances directly. | [meerkat-client/src/openai.rs#L20-L61](../../../meerkat-client/src/openai.rs#L20-L61), [meerkat-client/src/anthropic.rs#L24-L104](../../../meerkat-client/src/anthropic.rs#L24-L104) | The transport seam is not optional. It is the first architecture-prep seam. |
| TD-LOCAL-003 | `CODE_GROUNDED` | Provider streaming currently assumes an async byte-stream interface by calling `response.bytes_stream()` and parsing SSE or chunked events above that stream. | [meerkat-client/src/openai.rs#L329-L370](../../../meerkat-client/src/openai.rs#L329-L370), [meerkat-client/src/anthropic.rs#L568-L640](../../../meerkat-client/src/anthropic.rs#L568-L640) | Phase 1 must define a shared provider contract at the level of request shaping, incremental bytes, and normalized event mapping. It cannot assume identical underlying I/O primitives across platforms. |
| TD-LOCAL-004 | `CODE_GROUNDED` | The non-wasm runtime path already uses `tokio` primitives directly in key factory wiring such as watch channels and spawned tasks. | [Cargo.toml#L46-L49](../../../Cargo.toml#L46-L49), [meerkat/src/factory.rs#L1310-L1395](../../../meerkat/src/factory.rs#L1310-L1395) | Phase 0 must explicitly gate Tokio or executor viability on the target. This is not a background implementation detail. |

## Externally verified platform and toolchain constraints

These items come from primary or near-primary ecosystem sources and should be treated as planning constraints immediately.

| ID | Constraint | Source | Planning consequence |
| --- | --- | --- | --- |
| TD-EXT-001 | `ring` issue `#2088` for `xtensa-esp32s3-espidf` was closed as not planned, and the reported failure is `Unknown target CPU`. | [ring issue #2088](https://github.com/briansmith/ring/issues/2088) | The current `reqwest` + `rustls-tls` path cannot be assumed to compile for the ESP32-S3 Xtensa target. Phase 1 must not phrase the work as a transport-only swap. |
| TD-EXT-002 | ESP-IDF's HTTP client supports a blocking `esp_http_client_perform()` path, a non-blocking mode via `is_async`, and a stream-reader flow. | [ESP-IDF HTTP client docs](https://docs.espressif.com/projects/esp-idf/en/v5.0.1/esp32s3/api-reference/protocols/esp_http_client.html) | Streaming on ESP-IDF is viable, but the Rust-side bridge into Meerkat's async provider contract must be designed explicitly. |
| TD-EXT-003 | Espressif now marks the `std` crates (`esp-idf-sys`, `esp-idf-hal`, `esp-idf-svc`) as community supported and points users seeking a more stable official environment toward `esp-hal`. | [Espressif esp-hal beta announcement](https://developer.espressif.com/blog/2025/02/rust-esp-hal-beta/) | The plan can still use the `std` path, but it must treat maintenance risk and fallback options as first-class concerns rather than hidden assumptions. |
| TD-EXT-004 | The Rust on ESP `std` approach relies on the Espressif stack and installation flow for Xtensa targets rather than a plain upstream Rust target setup. | [Rust on ESP training intro](https://docs.esp-rs.org/std-training/03_0_intro_workshop.html) | The toolchain bootstrap belongs in Phase 0 evidence, not as an implicit prerequisite. |
| TD-EXT-005 | Mio's `mio_unsupported_force_poll_poll` and related flags are explicitly described as unsupported and may disappear in the future. | [mio README](https://github.com/tokio-rs/mio) | Any Tokio-on-ESP strategy that depends on unsupported Mio flags needs an explicit feasibility gate and a fallback plan. |

## Planning consequences that are now fixed

1. Phase 0 must answer four technical questions before architecture-prep work starts:
   - Can the chosen real board and toolchain boot, flash, monitor, and reconnect deterministically?
   - Can the chosen ESP networking and TLS path reach at least one provider endpoint and stream incrementally?
   - Can the planned Rust stack, including Tokio expectations, fit the board envelope well enough to remain the production path?
   - What PSRAM class is the minimum supported baseline for the required single-node scope?
2. `CONTRACT-003` cannot mean "the same transport abstraction everywhere." It must mean "shared request construction, incremental response parsing, retry semantics, and normalized event mapping," while allowing different TLS backends, HTTP clients, and sync/async bridges per target.
3. Passing a sacrificial Phase 0 probe on Arduino, PlatformIO, or ESP-IDF C does not automatically close Rust-stack-specific risk. Toolchain, executor, and memory claims must be rerun on the planned Rust stack before later phases close.
4. Swarm and RSSI viability do not belong in Phase 0. They belong with Example 037 on the real Phase 2 stack, where the transport, comms, and runtime choices are no longer hypothetical.

## What this file does not close

- It does not prove that Meerkat already runs on ESP32-S3.
- It does not prove that Tokio is acceptable on the target; it only makes the gate explicit.
- It does not prove that RSSI-based relative positioning is good enough for a recommended user pattern.

Those closures still require the later phase proofs defined elsewhere in this dossier.
