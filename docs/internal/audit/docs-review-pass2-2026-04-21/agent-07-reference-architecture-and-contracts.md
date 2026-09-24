# Agent 07 Findings

## Scope reviewed

- Reference docs for architecture, contracts, API surface, builtin tools, capability behavior, and skills governance.
- Design docs under `docs/architecture/` that still describe current machine/runtime ownership or are linked as canonical references.
- Mobpack design notes in `docs/design/mobpack.md`.
- Current crate layout and source-of-truth code for runtime bindings, tool registration, model capability gating, error-code mapping, and machine/runtime module layout.

Sub-areas with no material drift found in this pass:

- `docs/reference/architecture.mdx` core machine/composition counts and crate ownership summary matched the current workspace layout.
- `docs/reference/session-contracts.mdx` matched the current runtime-backed binding seam and session ownership model.
- `docs/reference/design-philosophy.mdx` broadly matched the current architectural direction.
- `docs/reference/skills-governance-runbook.mdx` matched the current skill-identity and lineage validation logic.
- `docs/reference/comms-redesign-v6-hard-cut.md` is clearly marked historical and did not present itself as current API reference.
- `docs/design/mobpack.md` is clearly labeled as a design proposal, and its implementation-status note matches the currently shipped CLI commands.

## Files reviewed

- `docs/reference/api-reference.mdx`
- `docs/reference/architecture.mdx`
- `docs/reference/builtin-tools.mdx`
- `docs/reference/capability-matrix.mdx`
- `docs/reference/comms-redesign-v6-hard-cut.md`
- `docs/reference/design-philosophy.mdx`
- `docs/reference/session-contracts.mdx`
- `docs/reference/skills-governance-runbook.mdx`
- `docs/architecture/finite-ownership-ledger.md`
- `docs/architecture/realtime-259-port-plan.md`
- `docs/architecture/rebuild-trigger-audit.md`
- `docs/architecture/machine-simplification-proposal.md`
- `docs/architecture/meerkat-runtime-dogma.md`
- `docs/architecture/meerkat-runtime-schema-parity-ledger.md`
- `docs/architecture/mob-runtime-schema-parity-ledger.md`
- `docs/design/mobpack.md`
- `meerkat/src/factory.rs`
- `meerkat/src/service_factory.rs`
- `meerkat-tools/src/lib.rs`
- `meerkat-tools/src/builtin/composite.rs`
- `meerkat-tools/src/builtin/skills/browse.rs`
- `meerkat-tools/src/builtin/skills/load.rs`
- `meerkat-tools/src/builtin/skills/resources.rs`
- `meerkat-tools/src/builtin/skills/functions.rs`
- `meerkat-tools/src/control_plane.rs`
- `meerkat-core/src/types.rs`
- `meerkat-core/src/session.rs`
- `meerkat-core/src/service/mod.rs`
- `meerkat-core/src/model_profile/capabilities.rs`
- `meerkat-contracts/src/error/mod.rs`
- `meerkat-models/src/lib.rs`
- `meerkat-runtime/src/meerkat_machine/runtime_control.rs`
- `meerkat-runtime/src/meerkat_machine/llm_reconfigure.rs`
- `meerkat-rpc/src/error.rs`
- `meerkat-rpc/src/session_runtime.rs`
- `Cargo.toml`
- `meerkat/Cargo.toml`

## Findings

### 1. `docs/reference/builtin-tools.mdx` is no longer comprehensive and omits shipped built-in tool surfaces

- The page title/description and overview table present the page as a comprehensive inventory, but the current docs stop at task/shell/memory/utility/comms/schedule tools and do not mention the skill builtins or deferred tool-catalog control plane. See `docs/reference/builtin-tools.mdx:2-4` and `docs/reference/builtin-tools.mdx:9-34`.
- The current runtime ships five skill builtins: `browse_skills`, `load_skill`, `skill_list_resources`, `skill_read_resource`, and `skill_invoke_function`. See `meerkat-tools/src/builtin/skills/mod.rs:22-49`, `meerkat-tools/src/builtin/skills/browse.rs:102-120`, `meerkat-tools/src/builtin/skills/load.rs:35-53`, `meerkat-tools/src/builtin/skills/resources.rs:58-76`, `meerkat-tools/src/builtin/skills/resources.rs:101-119`, and `meerkat-tools/src/builtin/skills/functions.rs:44-62`.
- The current runtime also exposes deferred catalog control-plane tools `tool_catalog_search` and `tool_catalog_load`, which are absent from the reference page. See `meerkat-tools/src/control_plane.rs:17-18` and `meerkat-tools/src/control_plane.rs:451-470`.
- This is more than a completeness nit: readers using the reference page today will miss current first-party tooling that is discoverable in code and reachable through the dispatcher surface.

### 2. `docs/reference/capability-matrix.mdx` overstates `view_image` gating and no longer matches the actual capability filter

- The doc says `view_image` is hidden when either `vision` or `image_tool_results` is false. See `docs/reference/capability-matrix.mdx:88-93`.
- The current machine-owned capability filter only gates `view_image` on `image_tool_results`; there is no corresponding `vision` check in the tool-visibility filter path. See `meerkat-core/src/session.rs:192-199`.
- The live reconfigure path also applies only `capability_base_filter_for_image_tool_results(target_capability_surface.image_tool_results)`, again without a `vision`-based tool denial. See `meerkat-rpc/src/session_runtime.rs:1294-1319` and `meerkat-runtime/src/meerkat_machine/llm_reconfigure.rs:51-79`.
- The page should either narrow the claim to `image_tool_results` or explicitly document any separate `vision` enforcement path if one is intended later.

### 3. `docs/reference/api-reference.mdx` has drift in core type descriptions for `Message`

- The API reference still describes `Message` as `System`, `User`, `Assistant`, `BlockAssistant`, `ToolResults`. See `docs/reference/api-reference.mdx:13-18`.
- The current enum also includes `SystemNotice`, which matters for runtime/system notices and tool-visibility updates. See `meerkat-core/src/types.rs:767-782`.
- Because this page is supposed to be the quick-lookup public type index, omitting `SystemNotice` now understates the actual persisted/runtime-visible message model.

### 4. `docs/reference/api-reference.mdx` error-code reference is incomplete and partially out of sync with current source of truth

- The “canonical `ErrorCode`” table omits `REQUEST_CANCELLED` and `DUPLICATE_INPUT`, even though both are part of the current public enum. Compare `docs/reference/api-reference.mdx:287-300` with `meerkat-contracts/src/error/mod.rs:25-40`.
- The current canonical projections are defined in `meerkat-contracts/src/error/mod.rs:43-100`; that file assigns `RequestCancelled -> -32005 / 499 / 14` and `DuplicateInput -> -32004 / 409 / 13`, neither of which is documented in the table.
- There is also a broader mismatch across docs and code for “session not running”: the API reference says the canonical wire code is `-32003` (`docs/reference/api-reference.mdx:289-292`), the capability-matrix page documents a lower-level session transport mapping of `-32005` (`docs/reference/capability-matrix.mdx:40-49`), and the current RPC adapter maps `SessionError::NotRunning` to `INTERNAL_ERROR` in `meerkat-rpc/src/session_runtime.rs:3876-3894`.
- Even if the intent is to distinguish canonical-wire vs transport-specific mappings, the current reference set does not explain the divergence cleanly enough, and the API page is definitely missing current error codes.

### 5. Several `docs/architecture/*` pages still point at pre-split runtime module paths, creating stale internal references

- `docs/architecture/finite-ownership-ledger.md` repeatedly points at `meerkat-runtime/src/meerkat_machine.rs` as the ownership boundary location. See `docs/architecture/finite-ownership-ledger.md:21-27` and `docs/architecture/finite-ownership-ledger.md:39-71`.
- That monolithic file no longer exists; the runtime machine is now split across `meerkat-runtime/src/meerkat_machine/mod.rs` plus submodules such as `runtime_control.rs`, `dispatch_session.rs`, `session_management.rs`, and others. The current `prepare_bindings` entry point, for example, lives in `meerkat-runtime/src/meerkat_machine/runtime_control.rs:322-342`.
- `docs/architecture/realtime-259-port-plan.md` still calls out a shell module path `meerkat-runtime/src/meerkat_machine/realtime_attachment.rs` as “my current implementation.” See `docs/architecture/realtime-259-port-plan.md:39-43`. That file no longer exists.
- These stale paths matter because the reference architecture pages are supposed to help readers locate current owners and current seams. Right now they send readers to files that are gone after the runtime module split.

## Suggested follow-ups

- Update `docs/reference/builtin-tools.mdx` so it either becomes truly comprehensive or explicitly scopes itself to a subset. At minimum, add the skill builtins and `tool_catalog_search` / `tool_catalog_load`.
- Correct `docs/reference/capability-matrix.mdx` to document the current `view_image` rule as implemented, or add a note distinguishing model-input `vision` capability from tool-visibility gating.
- Refresh `docs/reference/api-reference.mdx` against current enums and contracts: add `SystemNotice`, fill out the full canonical `ErrorCode` table, and clarify canonical-wire vs transport-specific mappings.
- Regenerate or manually refresh the affected `docs/architecture/*` pages so file-path references point to the split runtime module layout instead of the removed monolithic files.
- Sanity-check the RPC `SessionError::NotRunning` mapping in `meerkat-rpc/src/session_runtime.rs:3876-3894`; if the implementation is intentional, document that exception explicitly, and if not, fix code and then align the docs.
