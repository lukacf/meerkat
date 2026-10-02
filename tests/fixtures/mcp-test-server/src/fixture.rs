//! The one resolver every test uses to find the `mcp-test-server` binary.
//!
//! A test that needs the real stdio fixture fails when it cannot be found;
//! it never skips and passes.

use std::path::{Path, PathBuf};

/// Names the exact fixture binary. Bazel sets it from runfiles; Cargo lanes
/// export the output of `scripts/mcp-test-server-fixture`.
pub const FIXTURE_ENV: &str = "MEERKAT_MCP_TEST_SERVER";

const BINARY_NAME: &str = "mcp-test-server";

/// Why the fixture binary could not be resolved.
#[derive(Debug)]
pub enum FixtureError {
    /// `MEERKAT_MCP_TEST_SERVER` is set but does not name a file.
    EnvPathMissing(PathBuf),
    /// The variable is unset and the binary is not where
    /// `scripts/mcp-test-server-fixture` builds it for this test build.
    NotBuilt { expected: Option<PathBuf> },
}

impl std::fmt::Display for FixtureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EnvPathMissing(path) => write!(
                f,
                "{FIXTURE_ENV}={} does not name a file; point it at the binary printed by \
                 scripts/mcp-test-server-fixture",
                path.display()
            ),
            Self::NotBuilt { expected } => {
                write!(
                    f,
                    "mcp-test-server fixture not found: {FIXTURE_ENV} is unset"
                )?;
                if let Some(expected) = expected {
                    write!(f, " and {} does not exist", expected.display())?;
                }
                write!(
                    f,
                    ". Run `export {FIXTURE_ENV}=\"$(scripts/mcp-test-server-fixture)\"` \
                     before the tests (the script builds the fixture into this target \
                     directory and prints its path)"
                )
            }
        }
    }
}

impl std::error::Error for FixtureError {}

/// Resolve the fixture binary: `MEERKAT_MCP_TEST_SERVER` when set, otherwise
/// the path `scripts/mcp-test-server-fixture` builds it to, which is the
/// profile directory of the running test executable (`<target>/<profile>/`,
/// the parent of its `deps/`).
pub fn resolve_fixture_binary() -> Result<PathBuf, FixtureError> {
    if let Some(path) = std::env::var_os(FIXTURE_ENV).filter(|value| !value.is_empty()) {
        let path = PathBuf::from(path);
        return if path.is_file() {
            Ok(path)
        } else {
            Err(FixtureError::EnvPathMissing(path))
        };
    }
    let expected = std::env::current_exe()
        .ok()
        .as_deref()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .map(|profile_dir| {
            profile_dir.join(format!("{BINARY_NAME}{}", std::env::consts::EXE_SUFFIX))
        });
    match expected {
        Some(path) if path.is_file() => Ok(path),
        expected => Err(FixtureError::NotBuilt { expected }),
    }
}

/// [`resolve_fixture_binary`] for tests: panics with the resolution failure,
/// so a test that needs the fixture fails instead of skipping.
#[allow(clippy::panic)] // test fixture: a missing binary must fail the test
pub fn fixture_binary() -> PathBuf {
    match resolve_fixture_binary() {
        Ok(path) => path,
        Err(error) => panic!("{error}"),
    }
}
