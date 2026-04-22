# Lane EG Plan

## Owned Rows

- `DOGMA-14`
- `DOGMA-18`
- `DOGMA-20`
- `DOGMA-23`

## Owned Milestones

- `EG1a` = `DOGMA-20`
- `EG1b` = `DOGMA-14`, `DOGMA-23`
- `EG2` = `DOGMA-18`

## Owned Files

- `meerkat-session/src/persistent.rs`
- `meerkat-core/src/session_recovery.rs`
- `meerkat/src/surface/runtime_backed.rs`
- `meerkat-mob/src/runtime/builder.rs`
- `meerkat-mob/src/runtime/session_service.rs`
- `meerkat-rpc/src/realtime_ws.rs`
- `meerkat/src/surface/schedule_host.rs`
- `meerkat-skills/src/resolve.rs`

## Shared Files Touched Under Round Map

- `meerkat-rpc/src/realtime_ws.rs` as first owner for `EG1a`
- `meerkat-rest/src/lib.rs` as second owner only for `EG2`, after `B1`
- `meerkat-rpc/src/session_runtime.rs` as second owner only for `EG2`, after `B1`
- `meerkat-rpc/src/handlers/session.rs` as second owner only for `EG2`, after `B1`

## Blocked By

- `EG2` blocked by `B1`

## Unblocks

- `B2` after `EG1a`

## Public / Type / Schema Changes

- `DOGMA-20` changes websocket/product-session behavior, not public naming
- `DOGMA-23` is a full schedule-domain typed outcome fix and may require schema freshness work
- `DOGMA-18` changes canonical default skill-source identity resolution

## Defensive Scan Commitments

- add or update a scan banning helper-local UUID fabrication for default skill sources outside the canonical registry path

## Requested Orchestrator Probes

- `./scripts/repo-cargo check -p meerkat-rpc`
- `./scripts/repo-cargo check -p meerkat-mob`
- `./scripts/repo-cargo check -p meerkat-session`
- `./scripts/repo-cargo check -p meerkat-core`
- `./scripts/repo-cargo check -p meerkat-schedule`
- `./scripts/repo-cargo check -p meerkat`
- `./scripts/repo-cargo check -p meerkat-skills`
- `./scripts/repo-cargo check -p meerkat-rest`
- if schedule/public artifacts change:
  - `make regen-schemas`
  - `make verify-schema-freshness`

## Requested Build Gate Commands

- Quick probe `EG1a`:
  - `./scripts/repo-cargo check -p meerkat-rpc`
  - `./scripts/repo-cargo check -p meerkat-mob`
- Quick probe `EG1b`:
  - `./scripts/repo-cargo check -p meerkat-session`
  - `./scripts/repo-cargo check -p meerkat-core`
  - `./scripts/repo-cargo check -p meerkat-schedule`
  - `./scripts/repo-cargo check -p meerkat`
  - `make regen-schemas`
  - `make verify-schema-freshness`
- Quick probe `EG2`:
  - `./scripts/repo-cargo check -p meerkat-skills`
  - `./scripts/repo-cargo check -p meerkat-rpc`
  - `./scripts/repo-cargo check -p meerkat-rest`
- Full gate `EG1a`:
  - `./scripts/repo-cargo nextest run -p meerkat-rpc --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-rpc --test realtime_ws_protocol --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-mob --test member_realtime_bindings --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo e2e-fast`
- Full gate `EG1b`:
  - `./scripts/repo-cargo nextest run -p meerkat-session --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-session --test regression_lifecycle --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-schedule --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo e2e-fast`
- Full gate `EG2`:
  - `./scripts/repo-cargo nextest run -p meerkat-skills --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-core --test skill_identity_migrations --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-rpc --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo nextest run -p meerkat-rest --lib --show-progress none --status-level none --final-status-level fail`
  - `./scripts/repo-cargo e2e-fast`

## Risks

- `EG1a` is the unblocker for `B2`
- `DOGMA-23` is explicitly treated as a full domain change, not a one-line patch
- `EG2` shares REST/RPC session files with `B1` and must follow the ownership map strictly

## Open Decisions

- none

## Handoff Notes

- handoff commit:
- changed files:
  - `meerkat-core/src/session_recovery.rs`
  - `meerkat-core/src/skills_config.rs`
  - `meerkat-session/src/persistent.rs`
  - `meerkat-rest/src/lib.rs`
  - `meerkat-rpc/src/handlers/config.rs`
  - `meerkat-rpc/src/main.rs`
  - `meerkat-rpc/src/session_runtime.rs`
  - `meerkat-cli/src/main.rs`
  - `meerkat-skills/src/resolve.rs`
  - `meerkat-skills/src/source/filesystem.rs`
  - `scripts/skill_source_identity_scan.sh`
  - `meerkat/src/surface/schedule_host.rs`
  - `meerkat-rpc/src/realtime_ws.rs`
  - `.rct/round5/agents/EG-checklist.md`
  - `.rct/round5/agents/EG-plan.md`
- generated output request: none
- known risks:
  - `EG1a` was not compile- or test-verified in-lane because cargo commands were explicitly off-limits for this pass.
  - Rotation now rebuilds the live provider bridge and rebinds realtime observers/handles on `MemberRealtimeBindingEvent::Rotated`; this should get post-rotation status/input/commit traffic onto the new bridge session, but it still needs the requested RPC/mob probes.
  - `DOGMA-14` now rejects missing runtime bindings in stored-session runtime-backed recovery and updates the owned persistent recovery paths/tests to provide canonical bindings explicitly.
  - `DOGMA-23` static review found the owned schedule surface already carries runtime admission and completion meaning through typed `RuntimeAdmissionProjection` / `DeliveryTerminal` values into the canonical schedule lifecycle path; this pass adds owned unit coverage to pin that behavior instead of re-labeling it in a helper.
  - `DOGMA-18` now uses the shared default-source identity seam in config for REST/RPC canonicalization and default repository resolution, and the lane scan passes after removing remaining test-only literal UUID expectations.
