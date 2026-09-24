//! Fatal errors on the `rkat-rest` binary are rendered for the operator.
//!
//! `fn main() -> Result<(), Box<dyn Error>>` printed a returned error with
//! `Debug` (`Error: Store(UnledgeredDomainObjects { .. })`), hiding the
//! remedy sentence that only the `Display` form carries. The binary must
//! print the `Display` chain, prefixed with its name, and exit with status 1.
//! The cheapest deterministic fatal error is the `--realm`/`--isolated`
//! conflict, refused before any storage is touched; its `Display` text and
//! its `Debug` variant name (`ConflictingSelection`) differ visibly.

#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::path::PathBuf;
use std::process::{Command, Stdio};

const BINARY: &str = "rkat-rest";

/// `Display` of `RuntimeBootstrapError::ConflictingSelection`.
const CONFLICT_TEXT: &str = "`--realm` and `--isolated` cannot be used together";

/// Cargo and nextest advertise the crate's own binary to its integration
/// tests through the runtime environment. Resolving it there (never at compile
/// time) keeps the test valid inside a nextest archive.
fn binary() -> Option<PathBuf> {
    let advertised = std::env::var_os("CARGO_BIN_EXE_rkat-rest").map(PathBuf::from)?;
    advertised.exists().then_some(advertised)
}

#[test]
fn fatal_startup_error_renders_display_chain_and_exits_one() {
    let Some(binary) = binary() else {
        eprintln!(
            "skipping: {BINARY} binary not advertised (CARGO_BIN_EXE_rkat-rest unset or missing)"
        );
        return;
    };

    let output = Command::new(&binary)
        .args(["--realm", "conflicting-selection", "--isolated"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("rkat-rest should spawn");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        output.status.code(),
        Some(1),
        "a fatal startup error must exit with status 1; status={:?} stderr:\n{stderr}",
        output.status
    );
    let expected_line = format!("{BINARY}: {CONFLICT_TEXT}");
    assert!(
        stderr.lines().any(|line| line == expected_line),
        "stderr must carry the Display text prefixed with the binary name ({expected_line:?}); \
         stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("ConflictingSelection"),
        "the Debug variant name must not reach the operator; stderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("Error: "),
        "the `Error: ` prefix is Result-returning main's Debug rendering; stderr:\n{stderr}"
    );
}
