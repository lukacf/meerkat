//! Fatal errors on the `rkat-rpc` binary are rendered for the operator.
//!
//! `fn main() -> Result<(), Box<dyn Error>>` printed a returned error with
//! `Debug` (`Error: Store(UnledgeredDomainObjects { .. })`), hiding the
//! remedy sentence that only the `Display` form carries. The binary must
//! print the `Display` chain, prefixed with its name, and exit with status 1.
//! The cheapest deterministic fatal error is the `--realm`/`--isolated`
//! conflict, refused before any storage is touched; its `Display` text and
//! its `Debug` variant name (`ConflictingSelection`) differ visibly.

#![allow(
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_used,
    clippy::disallowed_methods
)]

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const BINARY: &str = "rkat-rpc";

/// `Display` of `RuntimeBootstrapError::ConflictingSelection`.
const CONFLICT_TEXT: &str = "`--realm` and `--isolated` cannot be used together";

/// Cargo and nextest advertise the crate's own binary to its integration
/// tests through the runtime environment. Resolving it there (never at compile
/// time) keeps the test valid inside a nextest archive. Both always advertise
/// it, so a missing binary fails the test instead of skipping it.
fn binary() -> PathBuf {
    let advertised = std::env::var_os("CARGO_BIN_EXE_rkat-rpc")
        .map(PathBuf::from)
        .expect("cargo advertises CARGO_BIN_EXE_rkat-rpc to the crate's integration tests");
    assert!(
        advertised.exists(),
        "CARGO_BIN_EXE_rkat-rpc names a missing binary: {}",
        advertised.display()
    );
    advertised
}

#[test]
fn fatal_startup_error_renders_display_chain_and_exits_one() {
    let binary = binary();

    let output = Command::new(&binary)
        .args(["--realm", "conflicting-selection", "--isolated"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("rkat-rpc should spawn");
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

/// Write a realm manifest for `realm_id` under the realms root `root`.
fn materialize_realm(root: &Path, realm_id: &str) {
    let dir = root.join(realm_id);
    std::fs::create_dir_all(&dir).expect("realm dir");
    let manifest = serde_json::json!({
        "realm_id": realm_id,
        "backend": "sqlite",
        "created_at": "0",
    });
    std::fs::write(dir.join("realm_manifest.json"), manifest.to_string()).expect("manifest");
}

/// A server started with `--context-root` probes the state root of every
/// ancestor of that directory that holds a `.rkat` entry. A realm present
/// under two of them is refused, and the refusal names both roots.
#[test]
fn realm_under_two_ancestor_state_roots_refuses_startup_naming_both() {
    const REALM: &str = "two-roots-row";
    let binary = binary();
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("proj");
    let nested = project.join("sub");
    let project_realms = project.join(".rkat").join("realms");
    let nested_realms = nested.join(".rkat").join("realms");
    materialize_realm(&project_realms, REALM);
    materialize_realm(&nested_realms, REALM);
    let user_root = temp.path().join("user");
    std::fs::create_dir_all(&user_root).expect("user root");

    let mut child = Command::new(&binary)
        .current_dir(temp.path())
        .args(["--realm", REALM, "--context-root"])
        .arg(&nested)
        .arg("--user-config-root")
        .arg(&user_root)
        .env_remove("RUST_LOG")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("rkat-rpc should spawn");
    let mut stderr_pipe = child.stderr.take().expect("stderr pipe");
    let stderr_reader = std::thread::spawn(move || {
        let mut buffer = String::new();
        let _ = stderr_pipe.read_to_string(&mut buffer);
        buffer
    });
    // A refusal exits at once; a server that started instead is ended here.
    let deadline = Instant::now() + Duration::from_mins(1);
    let status = loop {
        if let Some(status) = child.try_wait().expect("wait") {
            break Some(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let stderr = stderr_reader.join().expect("stderr reader");

    assert_eq!(
        status.and_then(|status| status.code()),
        Some(1),
        "a realm under two candidate state roots must refuse the start; stderr:\n{stderr}"
    );
    let head = format!("{BINARY}: realm '{REALM}' exists under more than one candidate state root");
    assert!(
        stderr.lines().any(|line| line.starts_with(&head)),
        "stderr must carry the refusal ({head:?}); stderr:\n{stderr}"
    );
    for root in [&nested_realms, &project_realms] {
        assert!(
            stderr.contains(&format!("'{}'", root.display())),
            "the refusal must name {}; stderr:\n{stderr}",
            root.display()
        );
    }
}
