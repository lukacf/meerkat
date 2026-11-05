# Requirement Traceability Matrix

| REQ-ID | Phase | Implemented In | Runtime Caller | Test Name | Status |
| --- | --- | --- | --- | --- | --- |
| TYPE-001 | 0 | `crates/meerkat-tui/src/cli.rs` | `rkat-tui` main | `cli::tests::*` | TYPED |
| TYPE-002 | 0 | `crates/meerkat-tui/src/error.rs` | `rkat-tui` main | compile-time exhaustive `exit_code()` | TYPED |
| TYPE-003 | 0 | — | — | — | MISSING |
| TYPE-004 | 0 | — | — | — | MISSING |
| TYPE-005 | 0 | — | — | — | MISSING |
| CONTRACT-001 | 0 | `crates/meerkat-tui/src/main.rs` | `meerkat_runtime::host_stack::run_host` | `rkat-tui --help` smoke | PROVISIONAL (not yet compiled) |
| CONTRACT-002 | 1 | — | — | — | MISSING |
| CONTRACT-003 | 2 | — | — | — | MISSING |
| CONTRACT-004 | 2 | — | — | — | MISSING |
| CONTRACT-005 | 1 | — | — | — | MISSING |
| CONTRACT-006 | 3 | `crates/meerkat-tui/src/cli.rs` (flags only) | — | `confinement_defaults_to_workspace_and_no_confinement_wins` | PROVISIONAL |
| REQ-001 | 0 | `crates/meerkat-tui/`, `scripts/release-rust-crates.sh`, `Cargo.toml`, `CLAUDE.md`, `CHANGELOG.md` | release tooling | `make check-rust-release-config` | PROVISIONAL (Bazel lock refresh pending) |
| REQ-002 | 1 | — | — | — | MISSING |
| REQ-003 | 1 | — | — | — | MISSING |
| REQ-004 | 4 | — | — | — | MISSING |
| REQ-005 | 5 | — | — | — | MISSING |
| REQ-006 | 6 | — | — | — | MISSING |
| E2E-001 | 0 | `crates/meerkat-tui/src/main.rs` | `rkat-tui --help` | `cli::tests::help_renders_without_terminal` | PROVISIONAL (not yet compiled) |
| E2E-002 | 1 | — | — | — | MISSING |
| E2E-003 | 2 | — | — | — | MISSING |
