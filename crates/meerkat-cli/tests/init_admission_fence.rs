//! `rkat init` while the user document's admission fence is held: with no
//! user document yet, the store creates nothing (it never waits for a
//! holder), and `init` says the document is held, never that a file is
//! missing, and writes no project config.
//!
//! Hermeticity: `HOME` and `XDG_DATA_HOME` point into a temporary directory.

#![cfg(unix)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::disallowed_methods
)]

use std::path::PathBuf;
use std::process::Command;

use meerkat_core::realm_admission::{AdmissionFence, AdmissionHold};

#[test]
fn init_while_the_user_document_is_held_says_so_and_creates_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let user_root = temp.path().join(".rkat");
    let fence = AdmissionFence::user_document(&user_root);
    fence
        .ensure_exists()
        .expect("the user document fence, which a holder needs to exist");
    let mut hold = AdmissionHold::try_acquire(vec![fence])
        .expect("an admission holds the user document fence");
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let binary = std::env::var_os("CARGO_BIN_EXE_rkat")
        .map(PathBuf::from)
        .expect("Cargo or Nextest must provide the runtime rkat binary path");
    let output = Command::new(binary)
        .current_dir(&project)
        .env("HOME", temp.path())
        .env("XDG_DATA_HOME", temp.path().join("data"))
        .env("RKAT_TEST_CLIENT", "1")
        .arg("init")
        .output()
        .unwrap();
    hold.release();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "init refuses: {stderr}");
    assert!(
        stderr.contains("an admission or another writer holds it"),
        "init says the document is held: {stderr}"
    );
    assert!(
        !stderr.contains("No such file or directory"),
        "not a missing file: {stderr}"
    );
    assert!(
        !user_root.join("config.toml").exists(),
        "no user document was created"
    );
    assert!(
        !project.join(".rkat").join("config.toml").exists(),
        "no project config was written"
    );
}
