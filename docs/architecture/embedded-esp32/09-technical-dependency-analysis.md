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
| TD-LOCAL-005 | `CODE_GROUNDED` | The current comms path depends on Ed25519 signing and verification through `ed25519-dalek` and on target-gated Tokio I/O in non-wasm builds. | [meerkat-comms/Cargo.toml#L14-L52](../../../meerkat-comms/Cargo.toml#L14-L52), [meerkat-comms/src/identity.rs#L1-L117](../../../meerkat-comms/src/identity.rs#L1-L117) | Swarm closure cannot treat comms as transport-only. Phase 3 must prove that the target can compile, boot, and execute the identity path used by real peer messaging. |
| TD-LOCAL-006 | `CODE_GROUNDED` | The facade currently links `meerkat-tools` and `meerkat-hooks` unconditionally, while `meerkat-tools` links `meerkat-skills` unconditionally even though skills are presented as optional at the facade feature layer. | [meerkat/Cargo.toml#L15-L35](../../../meerkat/Cargo.toml#L15-L35), [meerkat/Cargo.toml#L86-L89](../../../meerkat/Cargo.toml#L86-L89), [meerkat-tools/Cargo.toml#L19-L34](../../../meerkat-tools/Cargo.toml#L19-L34) | Phase 0 should measure this pre-fix baseline explicitly, and Phase 1 must make hooks, tools, and skills genuinely optional before embedded sizing claims are trusted. |

## Externally verified platform and toolchain constraints

These items come from primary or near-primary ecosystem sources and should be treated as planning constraints immediately.

| ID | Constraint | Source | Planning consequence |
| --- | --- | --- | --- |
| TD-EXT-001 | `ring` issue `#2088` for `xtensa-esp32s3-espidf` was closed as not planned, and the reported failure is `Unknown target CPU`. | [ring issue #2088](https://github.com/briansmith/ring/issues/2088) | The current `reqwest` + `rustls-tls` path cannot be assumed to compile for the ESP32-S3 Xtensa target. Phase 1 must not phrase the work as a transport-only swap. |
| TD-EXT-002 | ESP-IDF's HTTP client supports a blocking `esp_http_client_perform()` path, a non-blocking mode via `is_async`, and a stream-reader flow. | [ESP-IDF HTTP client docs](https://docs.espressif.com/projects/esp-idf/en/v5.0.1/esp32s3/api-reference/protocols/esp_http_client.html) | Streaming on ESP-IDF is viable, but the Rust-side bridge into Meerkat's async provider contract must be designed explicitly. |
| TD-EXT-003 | Espressif now marks the `std` crates (`esp-idf-sys`, `esp-idf-hal`, `esp-idf-svc`) as community supported and points users seeking a more stable official environment toward `esp-hal`. | [Espressif esp-hal beta announcement](https://developer.espressif.com/blog/2025/02/rust-esp-hal-beta/) | The plan can still use the `std` path, but it must treat maintenance risk and fallback options as first-class concerns rather than hidden assumptions. |
| TD-EXT-004 | The Rust on ESP `std` approach relies on the Espressif stack and installation flow for Xtensa targets rather than a plain upstream Rust target setup. | [Rust on ESP training intro](https://docs.esp-rs.org/std-training/03_0_intro_workshop.html) | The toolchain bootstrap belongs in Phase 0 evidence, not as an implicit prerequisite. |
| TD-EXT-005 | Mio's `mio_unsupported_force_poll_poll` and related flags are explicitly described as unsupported and may disappear in the future. | [mio README](https://github.com/tokio-rs/mio) | Any Tokio-on-ESP strategy that depends on unsupported Mio flags needs an explicit feasibility gate and a fallback plan. |
| TD-EXT-006 | Rust `std` on ESP32-S3 is a hosted environment on top of ESP-IDF and FreeRTOS rather than a full Unix-class host. | User-supplied ESP32/Tokio constraint summary aligned with ESP-IDF and esp-rs docs | Phase 0 and Phase 1 must treat “desktop-like `std` assumptions” as suspect by default. Passing `cargo check` is weaker evidence than a real on-device run. |
| TD-EXT-007 | ESP-IDF pthread support is partial, thread attributes are limited, and timing behavior is RTOS-tick-shaped rather than desktop-like. | User-supplied ESP32/Tokio constraint summary aligned with ESP-IDF pthread documentation | Thread creation cost, stack sizing, cancellation, and timer behavior are first-class planning concerns. Crates that quietly assume rich POSIX thread semantics should be treated as portability risks. |
| TD-EXT-008 | Tokio networking on ESP-IDF rides a simpler polling backend than mainstream epoll/kqueue Unix targets, and that backend is less battle-hardened for “desktop boringness” assumptions. | User-supplied ESP32/Tokio constraint summary aligned with Mio platform notes | The plan should target a proven hosted-async subset on ESP32 rather than assuming that all Tokio ecosystem crates transfer unchanged. |

## Phase 0 live evidence added after dossier freeze

These findings come from the real Phase 0 spike on the 8MB-PSRAM ESP32-S3 baseline board and should now be treated as stronger than the pre-spike planning assumptions.

| ID | Verdict | Evidence | Planning consequence |
| --- | --- | --- | --- |
| TD-LIVE-001 | `LIVE_VALIDATED` | The Rust `std` lane can boot on the real board, use the LCD, join Wi-Fi, sync SNTP time, perform HTTPS, and complete a real streamed OpenAI call. | Phase 0 has already closed the “basic board + network + provider” viability question for the planned Rust lane. |
| TD-LIVE-002 | `LIVE_VALIDATED` | A temporary firmware can link a real Meerkat mini-surface shape using `meerkat-core`, `meerkat-client`, `meerkat-hooks`, `meerkat-tools`, and `meerkat-skills` on `xtensa-esp32s3-espidf`. | The blocker is no longer “can the crates link at all?” but “which execution model can actually run them on target?” |
| TD-LIVE-003 | `LIVE_REJECTED` for the current hypothesis | A real on-device probe shows `tokio::runtime::Builder::new_current_thread().enable_time().build()` panicking with `LoadProhibited` inside `tokio::runtime::scheduler::current_thread::CurrentThread::new` before the Meerkat agent turn begins. | The current “Tokio current-thread runtime on ESP32-S3 std lane” hypothesis is not safe to carry into Phase 1 as an assumed execution model. Phase 0 must keep probing executor/runtime alternatives or workarounds first. |
| TD-LIVE-004 | `LIVE_VALIDATED` | The outer Meerkat worker thread can be spawned on the real board with a `131072` byte stack, while `163840`, `196608`, and `262144` byte stacks fail immediately with `Not enough space (os error 12)`. | The target is capable of running Meerkat bootstrap work in a dedicated thread, but the thread-stack budget is already tight enough that runtime choices must be planned against a small-stack envelope rather than a desktop-like default. |
| TD-LIVE-005 | `LIVE_REJECTED` for the tested fallback | A real on-device probe with `tokio::runtime::Builder::new_multi_thread().worker_threads(1)` and a bounded Tokio worker stack does not hit the current-thread crash path, but it aborts because Tokio cannot spawn its own internal worker thread on the board under the tested envelope. | “Just switch to Tokio multi-thread” is not a safe fallback hypothesis for Phase 1. Any executor fallback must account for internal worker-thread allocation pressure explicitly. |
| TD-LIVE-006 | `LIVE_VALIDATED` with a bounded caveat | A real on-device probe with `tokio::runtime::Builder::new_current_thread().build()` succeeds without timers and reaches Meerkat dispatcher, skill runtime, hook engine, OpenAI client, event channel, and `AgentBuilder::build()`. The first subsequent failure is an expected panic when hook execution reaches `tokio::time::timeout` with timers disabled. | The current-thread scheduler itself is not universally broken on target. The live blocker is narrower: timer-enabled current-thread runtime creation is broken, while timer-free current-thread execution can carry a substantial fraction of the Meerkat mini-surface bootstrap. |
| TD-LIVE-007 | `LIVE_VALIDATED` | The measured memory envelope is already narrow during the Meerkat mini-surface spike: roughly `256k` free heap before runtime bring-up, falling to about `65k` free heap after agent build, with low-water marks around `61k`. | Phase 1 decoupling and profile shaping are not optional cleanup. They are required if the embedded surface is expected to retain enough headroom for real provider turns and runtime-owned behavior. |
| TD-LIVE-008 | `LIVE_VALIDATED` | The strongest working interpretation of the current results is “Tokio on ESP32-S3 is viable as a hosted ESP-IDF subset, but not as a drop-in desktop runtime model.” Real hardware has already separated three classes of failure: timer-enabled current-thread runtime creation, internal worker-thread allocation, and later timer-dependent hook behavior. | Phase 0 should keep classifying failures into hosted-runtime shape limits, thread-stack limits, timer limits, or Meerkat-specific assumptions instead of treating “Tokio” as a single yes-or-no verdict. |
| TD-LIVE-009 | `LIVE_VALIDATED` | A real provider-backed Meerkat turn now completes end to end on the 8MB-PSRAM ESP32-S3 board. The successful run reached `MKT:MEERKAT:RUN_DONE`, `MKT:RUST_STACK:OK`, and `MKT:SINGLE_NODE:PASS` in [serial.log](../../../artifacts/embedded-esp32/phase-0/single-node-rust-stack/20260404-195126/serial.log). | Phase 0 no longer needs to answer “can Meerkat run at all on the target?” The answer is yes for the documented passing runtime shape. |
| TD-LIVE-010 | `LIVE_VALIDATED` with bounded scope | The passing on-device Meerkat shape uses a timer-free current-thread Tokio runtime, a `131072`-byte outer worker stack, a reduced one-tool surface, `gpt-5.4-mini`, and a real non-streaming Responses call for the Meerkat lane while keeping the separate direct-streaming provider proof intact. | Phase 1 must treat this as the current known-good baseline rather than assuming that the desktop runtime shape transfers unchanged. |
| TD-LIVE-011 | `LIVE_VALIDATED` | The final Phase 0 stall was not inside the agent loop itself. `run_with_events()` returned successfully on-device, and the last fix was dropping the agent-owned event sender before joining the collector so the probe could finish cleanly. | The remaining embedded work is architectural shaping and generalization, not rescuing a fundamentally broken agent loop. |
| TD-LIVE-015 | `LIVE_VALIDATED` | The owning runtime wake path had to be fixed on `espidf` by changing wake delivery in `RuntimeSessionAdapter` from opportunistic `try_send()` to awaited `send()`. Real hardware proved this removes the intermittent first-turn freeze where the ESP showed the first peer message but never progressed into the provider call. | Phase 1 should explicitly own ESP-aware runtime wake semantics in `meerkat-runtime` instead of leaving them as probe-only behavior. |
| TD-LIVE-016 | `LIVE_VALIDATED` | The strongest post-rebase end-to-end proof on `main` closed with `status: "pass"`, `host_pass: true`, `device_comms_pass: true`, and `micropy_calls: 18` in [summary.json](../../../artifacts/embedded-esp32/phase-0/rebase-proof-fullsync-20260406-150254/summary.json). | The dossier should now treat the rebased tree, not only the pre-rebase spike branch, as the authoritative baseline for later phases. |
| TD-LIVE-017 | `LIVE_VALIDATED` | Embedded MicroPython now initializes on-device and is invoked repeatedly through the existing host-tool callback contract from a normal host-to-ESP Meerkat conversation, with repeated `MKT:MICROPY:COMPLETED is_error=false` in [summary.json](../../../artifacts/embedded-esp32/phase-0/micropython-serial-validate/20260406-132019/summary.json). | Phase 2 should explicitly treat on-device scripting tools as a supported embedded pattern and keep them on the standard host-tool callback seam rather than inventing a separate scripting surface. |
| TD-LIVE-018 | `LIVE_VALIDATED` with bounded caveat | `PersistentSessionService` is proven on both sides only with memory-backed stores. The Phase 0 long-lived chat and MicroPython validations do not yet prove crash/reboot durability across device restarts. | Phase 2 should keep restart durability as an open closure target while treating memory-backed persistence orchestration as the current supported baseline. |

## ESP32-P4 target board consideration

The primary production target is moving from ESP32-S3 (Xtensa) to ESP32-P4 (RISC-V) with an M5Stamp ESP32-P4 module paired with the Stamp-AddOn C6 companion for Wi-Fi and BLE. The S3 remains the Phase 0 baseline and early development board. The P4+C6 becomes the primary target once boards arrive and Phase 0 S3 evidence has stabilized the architecture.

### Why the P4 changes the constraint landscape

| Constraint | ESP32-S3 (current Phase 0 board) | ESP32-P4 + C6 companion |
| --- | --- | --- |
| CPU ISA | Xtensa LX7, dual-core 240 MHz | RISC-V, dual HP-core 400 MHz + LP core 40 MHz |
| Rust toolchain | Espressif LLVM fork via `espup` | **Mainline `rustup` nightly** with RISC-V target |
| `ring` / `rustls` | Broken (`ring` #2088, Xtensa unsupported) | **Viable** (RISC-V is a supported `ring` target) |
| Internal SRAM | 512 KB (~180-220 KB free after Wi-Fi) | 768 KB (~500-600 KB free, no on-chip radio overhead) |
| PSRAM bandwidth | ~30 MB/s (OPI, 120 MHz DDR) | ~50 MB/s (OPI, 200 MHz DDR) |
| L2 cache | None | **512 KB unified** |
| Wi-Fi / BLE | Integrated | **None on-chip**; provided by C6 companion over SPI/SDIO |
| Board availability | Mature, widely available | Early availability (M5Stamp P4 module ordered) |

### What this resolves

- **TD-EXT-001** (`ring` blocked on Xtensa) does not apply on P4. The `reqwest` + `rustls-tls` workspace default should compile as-is for the RISC-V target. The transport seam (SEAM-004) is still architecturally correct but the forcing function shifts from "must replace entire TLS stack" to "should abstract for composition."
- **TD-EXT-004** (Xtensa fork toolchain) does not apply. Standard `rustup` nightly with `riscv32imafc-esp-espidf` target.
- **TD-LIVE-003/005/006** (Tokio timer panic, multi-thread abort, timer-free workaround) may or may not reproduce on P4. The 400 MHz RISC-V cores, 768 KB SRAM, and 512 KB L2 cache give substantially more headroom. These must be re-probed on the P4 board when it arrives.
- **TD-LIVE-004** (131 KB stack limit) may relax on P4 due to 50% more internal SRAM and no Wi-Fi stack competing for it.
- **TD-LIVE-007** (~65 KB free heap after agent build) should improve materially on P4 given the larger SRAM and L2 cache reducing PSRAM access pressure.

### What this does NOT resolve

- **TD-EXT-002** (ESP-IDF HTTP client streaming model) applies identically on P4. The Rust-side async bridge design is CPU-ISA-independent.
- **TD-EXT-003** (community-maintained `std` crates) applies on P4 as well. `esp-idf-svc` supports P4 via ESP-IDF v5.3+, but the community-maintained status is the same.
- **TD-EXT-005** (unsupported Mio flags) still applies for Tokio on any ESP-IDF target.
- **Wi-Fi** is now a companion-chip dependency. ASSUMP-001 and ASSUMP-002 must be re-validated on P4+C6 because the network path goes through the C6's SPI/SDIO bridge rather than the main SoC's integrated radio. Latency, throughput, and initialization sequence may differ.
- **Swarm comms** (ASSUMP-010) are still over Wi-Fi, but the Wi-Fi radio is on the C6, not the P4. Peer discovery and messaging may behave differently when the network stack is bridged.

### Dual-target strategy

Phase 0 S3 evidence has already closed the fundamental viability questions (ASSUMP-001 through ASSUMP-004, ASSUMP-008, ASSUMP-012). The Phase 1 seam extraction work is target-independent (it operates on the desktop/host codebase). The P4 board enters the picture at Phase 2 when the real embedded surface is assembled:

| Phase | Primary board | Role |
| --- | --- | --- |
| Phase 0 | ESP32-S3 (8MB PSRAM) | Baseline viability, already closed |
| Phase 1 | Desktop/host only | Seam extraction, no target hardware needed |
| Phase 2 | ESP32-P4 + C6 | Primary embedded surface and ESP backend target |
| Phase 3 | ESP32-P4 + C6 (multiple units for swarm) | Examples 036 and 037 on production hardware |

S3 becomes a secondary/regression target. Phase 2 should verify that the embedded surface can still build for S3 (Xtensa) as a CI check, but P4+C6 is the primary smoke and example target.

### P4-specific re-validation requirements

When the P4+C6 boards arrive, the following Phase 0-equivalent checks must be run before Phase 2 begins in earnest:

1. Boot, Wi-Fi via C6 companion, time sync, provider HTTPS — analogous to ASSUMP-001/002 but through the companion radio path.
2. Tokio runtime bring-up — re-probe TD-LIVE-003/005/006 on P4 to see if the timer and worker-thread issues reproduce on the RISC-V target.
3. Memory envelope baseline — re-measure TD-LIVE-004/007 equivalents on P4 to establish the actual headroom.
4. `ring`/`rustls` compilation — confirm that `reqwest` with `rustls-tls` compiles and connects on the RISC-V target as expected.

These checks are lightweight re-validations, not full Phase 0 re-runs. The architecture and probe harness from S3 Phase 0 carry over.

## Predeclared executor fallback candidates

If Phase 0 rejects the current Rust `std` plus Tokio baseline, the autonomous run may only choose among these predeclared fallback families before Phase 1 starts:

1. A smaller `std`-retaining execution envelope with a single-threaded or `block_on`-style control loop plus a bounded blocking I/O worker for provider traffic.
2. A lighter embedded executor path, such as an Embassy-class or edge-executor-class runtime, provided it still satisfies the Phase 1 and Phase 2 contracts.
3. A hybrid bridge where the embedded surface remains canonical, but provider and network work is isolated behind a bounded worker and explicit async-to-sync bridge.

These are candidate families, not validated recommendations. The point is to keep the autonomous run from inventing a brand-new runtime category midstream if the baseline Tokio plan is rejected.

## Planning consequences that are now fixed

1. Phase 0 must answer four technical questions before architecture-prep work starts:
   - Can the chosen real board and toolchain boot, flash, monitor, and reconnect deterministically?
   - Can the chosen ESP networking and TLS path reach at least one provider endpoint and stream incrementally?
   - Can the planned Rust stack, including Tokio expectations, fit the board envelope well enough to remain the production path?
   - What PSRAM class is the minimum supported baseline for the required single-node scope?
2. Phase 0 must also capture a before-fix footprint baseline for the currently smallest-runnable Meerkat agent shapes so the project can prove whether Phase 1 decoupling work meaningfully reduces the embedded envelope.
3. `CONTRACT-003` cannot mean "the same transport abstraction everywhere." It must mean "shared request construction, incremental response parsing, retry semantics, and normalized event mapping," while allowing different TLS backends, HTTP clients, and sync/async bridges per target.
4. Passing a sacrificial Phase 0 probe on Arduino, PlatformIO, or ESP-IDF C does not automatically close Rust-stack-specific risk. Toolchain, executor, and memory claims must be rerun on the planned Rust stack before later phases close.
5. Embedded sizing claims based on the current facade build graph are provisional until hooks, tools, and skills are made genuinely optional. Pre-fix measurements are evidence, not the target steady state.
6. Swarm and RSSI viability do not belong in Phase 0. They belong with Example 037 on the real Phase 2 stack, where the transport, comms, and runtime choices are no longer hypothetical.
7. Swarm viability is not just a radio question. The real comms identity and signing stack must compile and execute on target before Example 037 can be considered credible.
8. The timer-enabled current-thread Tokio runtime path is now an evidenced blocker rather than a generic concern. Any later plan that continues to assume that exact runtime shape must first explain why the Phase 0 panic does not apply.
9. The timer-free current-thread runtime result proves that the embedded path is not blocked at the first Meerkat object boundary. The platform can already carry real Meerkat bootstrap work once runtime assumptions are shaped correctly.
10. Multi-thread Tokio is now an evidenced dead end for the tested board envelope unless the plan can materially change worker-thread allocation behavior.
11. The right mental model is “hosted async on a constrained ESP-IDF/FreeRTOS substrate,” not “desktop async on a smaller CPU.” Phase 0 should keep proving the supported subset explicitly instead of assuming Unix-like behavior.
12. The probe program should distinguish platform-shape blockers from Meerkat blockers. A portability issue in POSIX emulation, timer support, or worker-thread creation is different from a Meerkat semantic or ownership bug.
13. Direct provider streaming and provider-backed Meerkat execution should now be treated as two distinct closure questions. Phase 0 proved both, but not with the same runtime shape.
14. The current known-good Meerkat baseline on the board is strong enough to justify Phase 1 architecture work immediately, even though some of the passing choices are still spike-specific workarounds rather than final product decisions.
15. The standard runtime-backed keep-alive plus comms path is now also proven on real hardware, but it is only acceptable as a Phase 0 baseline if later phases absorb the watchdog and scheduler-shaping work instead of ignoring it.
16. The current probe should not keep compile-time host peer addresses as a hidden coupling. The Phase 2 embedded surface should move peer-address configuration to a proper runtime or surface input.
17. The awaited runtime wake fix on `espidf` is not a temporary harness adjustment. It is a proven owning-crate compatibility change that belongs in Phase 1.
18. The current stable device baseline is explicitly non-streaming for Meerkat provider turns, even though direct provider streaming and host-side Meerkat streaming were both proven separately. Phase 2 should preserve that distinction.
19. On-device scripting through the standard host-tool callback contract is now a proven embedded pattern. Phase 2 examples should treat repeated tool execution and clean tool teardown as first-class smoke criteria.
20. Embedded simplification should happen by shrinking the reachable capability graph, not by introducing a second embedded semantic runtime. If hardware limits force out mobs, background task execution, or selected long-running tools, those should become explicit profile exclusions on the canonical runtime path.
21. This same rule suggests moving the wasm surface toward the canonical runtime direction over time rather than using the current browser-shaped substrate authority as the model for embedded. The long-term DRY win is one semantic runtime with multiple surfaces and profiles.

## What this file does not close

- It does not prove that Meerkat already runs on ESP32-S3.
- It does not prove that Tokio is acceptable on the target; it only makes the gate explicit.
- It does not prove that RSSI-based relative positioning is good enough for a recommended user pattern.

Those closures still require the later phase proofs defined elsewhere in this dossier.
