# RCT Evidence Log

Evidence is appended after each phase gate. Each entry must include exact
commands, commit SHA, timestamp, and output excerpts or artifact paths.

## Phase 0 External Boundary Notes

- 2026-04-26: Official OpenAI docs confirm Responses hosted
  `image_generation` emits `image_generation_call.result` base64 images and
  optional `revised_prompt`, and GPT Image models are backend image models rather
  than valid Responses `model` values.
- 2026-04-26: Official Gemini docs confirm native image models return
  interleaved text and inline image parts through `generateContent`.

## Phase 0 Command Evidence

- Timestamp: `2026-04-26T21:54:24Z`
- Base commit: `4ea6b2d47e599dbe0e0a263b45c9933d11cde7d8`
- Commands:
  - `cargo test -p meerkat-core image_generation -- --nocapture`
  - `cargo test -p meerkat-core --test image_generation_contracts -- --nocapture`
  - `cargo check -p meerkat-openai -p meerkat-gemini -p meerkat-anthropic -p meerkat-contracts -p rkat -p meerkat-rpc`
  - `rg -n "todo!\(|unimplemented!\(|panic!\(\"not implemented|TODO|FIXME|HACK|XXX|future work|deferred|placeholder|not yet implemented" meerkat-core/src/image_generation.rs meerkat-core/src/types.rs meerkat-core/tests/image_generation_contracts.rs .rct docs/architecture/assistant-image-generation-substrate.md`
- Output excerpts:
  - `cargo test -p meerkat-core image_generation`: `7 passed; 0 failed`
  - `cargo test -p meerkat-core --test image_generation_contracts`: `2 passed; 0 failed`
  - `cargo check ...`: `Finished dev profile ... target(s) in 56.66s`
  - Stub scan found only the pre-existing `placeholders` serialization comment in
    `meerkat-core/src/types.rs`, not a new implementation stub.

## Phase 0 Blocker Fix Evidence

- Timestamp: `2026-04-26T22:02:22Z`
- Base commit: `4ea6b2d47e599dbe0e0a263b45c9933d11cde7d8`
- Review blockers addressed:
  - `CQ-001` / `CQ-002`: `GenerateImageRequest` now validates on serde
    deserialization and rejects empty edit `source_images` at the request
    boundary.
  - `SC-001` / `IC-001`: `WireAssistantBlock::Image` preserves generated image
    transcript blocks instead of degrading to `Unknown`.
  - `SC-002`: round-trip tests now cover execution plans, tool results,
    lifecycle phases, approvals, switch-turn phases, and scoped overrides.
  - `SC-003` / `IC-004` / `CQ-003`: the shipped schema emitter entrypoint now
    exposes the new image-generation/model-routing contracts.
- Commands:
  - `cargo test -p meerkat-core image_generation -- --nocapture`
  - `cargo test -p meerkat-core --test image_generation_contracts -- --nocapture`
  - `cargo test -p meerkat-contracts --test image_generation_wire --features schema -- --nocapture`
  - `cargo test -p meerkat-contracts --test image_generation_wire -- --nocapture`
  - `cargo run -p meerkat-contracts --features schema --bin emit-schemas`
  - `rg -n "WireGenerateImageRequest|WireAssistantImageRef|WireSwitchTurnPhase|WireSessionModelRoutingStatus|image_id|block_type" artifacts/schemas/wire-types.json`
  - `cargo check -p meerkat-openai -p meerkat-gemini -p meerkat-anthropic -p meerkat-contracts -p rkat -p meerkat-rpc`
- Output excerpts:
  - core image-generation unit tests: `9 passed; 0 failed`
  - core public contract tests: `2 passed; 0 failed`
  - contracts schema-feature tests: `4 passed; 0 failed`
  - contracts non-schema tests: `3 passed; 0 failed`
  - schema emitter: `Schemas written to artifacts/schemas`
  - schema artifact contains `WireGenerateImageRequest`, `WireAssistantImageRef`,
    `WireSwitchTurnPhase`, `WireSessionModelRoutingStatus`, and `image_id`.
  - cross-crate check: `Finished dev profile ... target(s) in 42.36s`

## Phase 0 Spec Blocker Fix Evidence

- Timestamp: `2026-04-26T22:21:54Z`
- Base commit: `4ea6b2d47e599dbe0e0a263b45c9933d11cde7d8`
- Review blocker addressed:
  - `SC-001`: `phase0_declared_enum_variants_roundtrip` now exercises all
    Phase 0 declared enum families that shape image generation, continuity,
    provider metadata, lifecycle states, approvals, switch-turn routing, and
    scoped overrides.
- Commands:
  - `cargo test -p meerkat-core image_generation -- --nocapture`
  - `cargo test -p meerkat-contracts --test image_generation_wire --features schema -- --nocapture`
- Output excerpts:
  - core image-generation unit tests: `10 passed; 0 failed`
  - core public contract filtered run included
    `public_image_generation_request_contract_roundtrips`
  - contracts schema-feature tests: `4 passed; 0 failed`

## Phase 0 Exhaustive Variant Coverage Evidence

- Timestamp: `2026-04-26T22:25:09Z`
- Base commit: `4ea6b2d47e599dbe0e0a263b45c9933d11cde7d8`
- Review blocker addressed:
  - `SC-001`: core and wire tests now explicitly round-trip the previously
    omitted switch-turn policy reasons, switch-turn denial reasons,
    switch-turn terminal classes, approval reasons, model-routing approval
    terminal classes, post-activation image denial reasons, finite switch-turn
    scoped override kind, and the OpenAI Images API edits endpoint.
- Commands:
  - `cargo test -p meerkat-core image_generation -- --nocapture`
  - `cargo test -p meerkat-contracts --test image_generation_wire --features schema -- --nocapture`
- Output excerpts:
  - core image-generation unit tests: `10 passed; 0 failed`
  - contracts schema-feature tests: `5 passed; 0 failed`

## Phase 1 Machine Authority Evidence

- Timestamp: `2026-04-26T22:45:22Z`
- Base commit: `4ea6b2d47e599dbe0e0a263b45c9933d11cde7d8`
- Implemented:
  - MeerkatMachine-owned baseline/effective model-routing status projection.
  - Finite `switch_turn` pending boundary, activation, turn-counted restore,
    and typed denial paths.
  - `UntilChanged` handoff through the persistent reconfigure transition family
    rather than scoped override state.
  - Operation-scoped image lifecycle with operation > turn > baseline effective
    model precedence.
  - Scoped nesting guards and realtime conflict denials.
- Commands:
  - `cargo test -p meerkat-runtime meerkat_machine -- --nocapture`
  - `cargo xtask machine-codegen --all`
  - `cargo xtask machine-check-drift --all`
  - `cargo xtask machine-verify --all`
  - `cargo check -p meerkat-runtime -p meerkat-machine-schema`
- Output excerpts:
  - runtime machine tests: `162 passed; 0 failed`
  - `machine-check-drift`: `machine authority artifacts are up to date`
  - `machine-verify`: `MeerkatMachine` completed with `No error has been
    found`, `635698 distinct states found`, depth `7`.
  - `machine-verify`: all remaining machines/compositions completed with
    `No error has been found`.
  - post-codegen check: `Finished dev profile ... target(s) in 0.17s`

## Phase 2 Generate Image Runtime Path Evidence

- Timestamp: `2026-04-27T00:57:00Z`
- Base commit: `4ea6b2d47e599dbe0e0a263b45c9933d11cde7d8`
- Implemented:
  - `generate_image` as a Meerkat builtin wired through `CompositeDispatcher`.
  - MeerkatMachine command-path plan resolution and image operation lifecycle.
  - Provider image executors for OpenAI hosted Responses image tool, OpenAI
    Images API, and Gemini native image output.
  - Blob-store commit and assistant image block session effects.
  - Post-tool hook denial suppresses generated image session effects.
  - Factory/runtime-backed surfaces inject machine, blob store, and routed image
    executors through the shared builder path.
- Commands:
  - `cargo check -p meerkat -p meerkat-runtime -p meerkat-tools -p meerkat-client -p meerkat-openai -p meerkat-gemini -p meerkat-llm-core`
  - `cargo test -p meerkat-tools generate_image -- --nocapture`
  - `cargo test -p meerkat-core hook_denied_tool_result_suppresses_session_effects -- --nocapture`
  - `cargo test -p meerkat-openai openai_hosted_image_executor_normalizes_fake_response -- --nocapture`
  - `cargo test -p meerkat-gemini gemini_native_image_executor_requests_text_and_image_modalities -- --nocapture`
  - `cargo test -p meerkat factory_builder_wires_generate_image_on_runtime_backed_path -- --nocapture`
  - `cargo test -p meerkat-runtime image_plan -- --nocapture`
  - `cargo test -p meerkat-runtime openai_images_api_rejects -- --nocapture`
  - `cargo test -p meerkat-runtime model_routing -- --nocapture`
  - `cargo test -p meerkat-core --test image_generation_contracts -- --nocapture`
  - `cargo test -p meerkat-contracts --test image_generation_wire -- --nocapture`
  - `cargo test -p meerkat-core -p meerkat-contracts -p meerkat-runtime -p meerkat-tools -p meerkat-openai -p meerkat-gemini -p meerkat-client -p meerkat --lib --tests`
- Output excerpts:
  - cross-crate check: `Finished dev profile ... target(s) in 19.48s`
  - generate-image tool tests: `2 passed; 0 failed`
  - hook denial regression: `1 passed; 0 failed`
  - OpenAI adapter regression: `1 passed; 0 failed`
  - Gemini adapter regression: `1 passed; 0 failed`
  - factory runtime-backed regression: `1 passed; 0 failed`
  - image plan separation regression: `1 passed; 0 failed`
  - unprojected OpenAI source denial regression: `1 passed; 0 failed`
  - runtime model-routing tests: `2 passed; 0 failed`
  - core image-generation contract tests: `2 passed; 0 failed`
  - wire image-generation tests: `4 passed; 0 failed`
  - broad deterministic crate lane: all selected lib/tests completed green;
    notable crate totals include `meerkat` lib `72 passed`, `meerkat-runtime`
    lib `487 passed`, `meerkat-tools` lib `342 passed`, and
    `meerkat-tools` RCT contracts `14 passed`.

## Phase 3 Live Lane And Drift Evidence

- Timestamp: `2026-04-27T00:57:00Z`
- Base commit: `4ea6b2d47e599dbe0e0a263b45c9933d11cde7d8`
- Commands:
  - `cargo test -p meerkat --features integration-real-tests --test live_meerkat_regression e2e_smoke_openai_generate_image_substrate -- --ignored --nocapture`
  - `cargo test -p meerkat --features integration-real-tests --test live_meerkat_regression e2e_smoke_gemini_generate_image_substrate -- --ignored --nocapture`
  - `cargo test -p meerkat-client --features integration-real-tests e2e_smoke_ -- --ignored --nocapture`
  - `cargo run -p meerkat-contracts --features schema --bin emit-schemas`
  - `cargo xtask machine-codegen --all`
  - `cargo xtask machine-check-drift --all`
- Output excerpts:
  - OpenAI substrate smoke: committed `1664364` base64 bytes as `image/png`;
    `1 passed; 0 failed`.
  - Gemini substrate smoke: committed `596864` base64 bytes as `image/jpeg`;
    `1 passed; 0 failed`.
  - Provider live image smokes: OpenAI and Gemini both `terminal=Generated
    images=1 warnings=[]`; `2 passed; 0 failed`.
  - schema emitter: `Schemas written to artifacts/schemas`
  - machine codegen: regenerated machine/composition artifacts.
  - machine drift: `machine authority artifacts are up to date`

## Phase 3 Reviewer Blocker Resolution Evidence

- Timestamp: `2026-04-27T00:44:16Z`
- Base commit: `4ea6b2d47e599dbe0e0a263b45c9933d11cde7d8`
- Review blockers addressed:
  - Auto image planning is now provider-aware: OpenAI hosted image tool auto
    projection is only selected for OpenAI session models, Gemini native output
    only for Gemini session models, and unknown/non-image-capable session
    providers get a typed `UnsupportedTarget` denial.
  - Explicit OpenAI provider defaults no longer borrow a non-OpenAI session
    model as the image model; they use the known OpenAI image default.
  - Live substrate coverage now includes a shipped factory/session turn that
    lets a normal agent LLM emit the model-facing `generate_image` tool call,
    routes it through `FactoryAgentBuilder`, `MeerkatMachine`, the builtin
    dispatcher, the real OpenAI image executor, blob commit, and assistant
    image block session storage.
- Commands:
  - `cargo test -p meerkat --features integration-real-tests --test live_meerkat_regression e2e_smoke_ -- --ignored --nocapture`
  - `cargo test -p meerkat-runtime image_plan -- --nocapture`
  - `cargo test -p meerkat-runtime auto_image_plan_rejects -- --nocapture`
  - `cargo test -p meerkat-runtime provider_default_openai_uses_known_image_default -- --nocapture`
  - `cargo check -p meerkat -p meerkat-runtime -p meerkat-tools`
- Output excerpts:
  - live substrate lane: OpenAI direct substrate committed `1578044` base64
    bytes as `image/png`; Gemini direct substrate committed `624052` base64
    bytes as `image/jpeg`; OpenAI factory/session turn committed `1606532`
    base64 bytes as `image/png`; `3 passed; 0 failed`.
  - provider-aware image plan filtered run: `2 passed; 0 failed`.
  - unsupported auto-plan regression: `1 passed; 0 failed`.
  - OpenAI provider-default regression: `1 passed; 0 failed`.
  - post-fix check: `Finished dev profile ... target(s) in 0.82s`.

## Phase 3 Final Deterministic Gate Evidence

- Timestamp: `2026-04-27T00:45:56Z`
- Base commit: `4ea6b2d47e599dbe0e0a263b45c9933d11cde7d8`
- Final fix before rerun:
  - The `meerkat-tools` image-generation unit harness now uses the catalogued
    OpenAI model `gpt-5.4`, so Auto planning exercises the supported OpenAI
    auto route instead of tripping the intended unknown-provider denial.
- Commands:
  - `cargo test -p meerkat-tools generate_image -- --nocapture`
  - `cargo test -p meerkat-core -p meerkat-contracts -p meerkat-runtime -p meerkat-tools -p meerkat-openai -p meerkat-gemini -p meerkat-client -p meerkat --lib --tests`
  - `cargo run -p meerkat-contracts --features schema --bin emit-schemas`
  - `cargo xtask machine-check-drift --all`
- Output excerpts:
  - generate-image filtered tool tests: `2 passed; 0 failed`.
  - broad deterministic crate lane: all selected lib/tests completed green;
    notable crate totals include `meerkat` lib `72 passed`, `meerkat-runtime`
    lib `487 passed`, `meerkat-tools` lib `342 passed`, and
    `meerkat-tools` RCT contracts `14 passed`.
  - schema emitter: `Schemas written to artifacts/schemas`.
  - machine drift: `machine authority artifacts are up to date`.

## Phase 3 Explicit Model Consistency Evidence

- Timestamp: `2026-04-27T00:49:06Z`
- Base commit: `4ea6b2d47e599dbe0e0a263b45c9933d11cde7d8`
- Review blocker addressed:
  - Explicit model targets now validate provider/model consistency during
    machine-owned image plan resolution. Catalogued mismatches such as
    `provider=openai, model=claude-sonnet-4-5` and
    `provider=gemini, model=gpt-image-1` return typed `UnsupportedTarget`
    denials before provider dispatch.
- Commands:
  - `cargo fmt --all`
  - `cargo test -p meerkat-runtime image_plan -- --nocapture`
  - `cargo test -p meerkat-tools generate_image -- --nocapture`
  - `cargo test -p meerkat --features integration-real-tests --test live_meerkat_regression e2e_smoke_openai_generate_image_factory_session_turn -- --ignored --nocapture`
  - `cargo test -p meerkat-core -p meerkat-contracts -p meerkat-runtime -p meerkat-tools -p meerkat-openai -p meerkat-gemini -p meerkat-client -p meerkat --lib --tests`
  - `cargo test -p meerkat --features integration-real-tests --test live_meerkat_regression e2e_smoke_ -- --ignored --nocapture`
- Output excerpts:
  - runtime image-plan filtered run: `3 passed; 0 failed`.
  - generate-image filtered tool tests: `2 passed; 0 failed`.
  - factory/session OpenAI live turn: committed `1660336` base64 bytes as
    `image/png`; `1 passed; 0 failed`.
  - broad deterministic crate lane: all selected lib/tests completed green;
    notable crate totals include `meerkat` lib `72 passed`, `meerkat-runtime`
    lib `487 passed`, `meerkat-tools` lib `342 passed`, and
    `meerkat-tools` RCT contracts `14 passed`.
  - live substrate lane: Gemini direct substrate committed `396748` base64
    bytes as `image/jpeg`; OpenAI factory/session turn committed `1515176`
    base64 bytes as `image/png`; OpenAI direct substrate committed `1539868`
    base64 bytes as `image/png`; `3 passed; 0 failed`.

## Final Local CI Evidence

- Timestamp: `2026-04-27T01:27:00Z`
- Base commit: `4ea6b2d47e599dbe0e0a263b45c9933d11cde7d8`
- Final fixes before publication:
  - Clippy-cleaned new image-generation tests and provider adapters.
  - Boxed finalized streaming assistant blocks in the LLM block assembler so
    the new assistant image block variant does not inflate the enum slot size.
  - Declared model-routing DSL wrapper inputs as runtime-internal and exempted
    corresponding facade command variants from the runtime/schema manifest
    parity gate.
- Commands:
  - `make fmt-check`
  - `make lint`
  - `cargo unit`
  - `cargo int`
  - `cargo e2e-fast`
  - `cargo e2e-system`
  - `cargo test -p meerkat-machine-codegen --test runtime_alphabet_parity -- --nocapture`
  - `cargo xtask machine-codegen --all`
  - `cargo xtask machine-check-drift --all`
  - `cargo run -p meerkat-contracts --features schema --bin emit-schemas`
  - `cargo test -p meerkat --features integration-real-tests --test live_meerkat_regression e2e_smoke_ -- --ignored --nocapture`
  - `cargo test -p meerkat-client --features integration-real-tests --test live_client_provider_matrix e2e_smoke_ -- --ignored --nocapture`
  - `git diff --check`
- Output excerpts:
  - `make fmt-check`: completed with no formatting diffs.
  - `make lint`: finished `clippy --workspace --all-targets --all-features -- -D warnings` green.
  - `cargo unit`: `3950 passed`, `12 skipped`.
  - `cargo int`: `5188 passed`, `80 skipped`.
  - `cargo e2e-fast`: `5 passed`, `0 skipped`.
  - `cargo e2e-system`: `16 passed`, `0 skipped`.
  - runtime alphabet parity: `4 passed; 0 failed`.
  - machine drift: `machine authority artifacts are up to date`.
  - schema emitter: `Schemas written to artifacts/schemas`.
  - live substrate lane: Gemini direct substrate committed `645756` base64
    bytes as `image/jpeg`; OpenAI factory/session turn committed `1674904`
    base64 bytes as `image/png`; OpenAI direct substrate committed `1645676`
    base64 bytes as `image/png`; `3 passed; 0 failed`.
  - provider live image smokes: OpenAI and Gemini both
    `terminal=Generated images=1 warnings=[]`; `2 passed; 0 failed`.
  - `git diff --check`: clean.
