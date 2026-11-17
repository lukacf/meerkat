//! `rkat realm create` and then, at once, `rkat realm govern`, on the real
//! binary: every round of the pair succeeds, and no `realm govern` meets a
//! manifest lock file that a `realm create` left behind.
//!
//! A realm's manifest lock is one file in the realm's directory. `realm
//! create` takes it to write the manifest and `realm govern` takes it to
//! record the posture. A `realm create` that ends while its lock file is
//! still there makes the `realm govern` that follows wait five seconds and
//! fail with "timed out acquiring realm manifest lock", and the realm stays
//! ungoverned.
//!
//! This row is a guard of the command pair, not the proof that the lock is
//! released. Whether the file is left is decided by a race at the end of
//! the first process, which a test of the real binary cannot force: on a
//! build whose release could be lost that way, measured on a development
//! host, none of sixty rounds lost the race and this row passed. The proof
//! is deterministic and lives with the lock: the store crate's
//! `manifest_lock` rows. What this row holds is that, over [`ROUNDS`]
//! rounds, each on a realm of its own, as a setup script that makes and
//! governs realms one after the other does, every `realm create` and every
//! `realm govern` succeeds and the last realm reads governed; a round that
//! does meet a leftover lock is counted and fails the row.
//!
//! Hermeticity: `HOME`, the XDG roots, the context root and the state root
//! are in a temporary directory. The three providers' key variables, with
//! and without the `RKAT_` prefix, are not passed on; the commands call no
//! provider.

#![cfg(feature = "local-authorization")]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::disallowed_methods
)]

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

/// Rounds of create-then-govern. A choice, not a number derived from a
/// measurement: see the module text.
const ROUNDS: usize = 60;

const LOCK_TIMEOUT: &str = "timed out acquiring realm manifest lock";

struct Roots {
    _temp: tempfile::TempDir,
    home: PathBuf,
    project: PathBuf,
    state: PathBuf,
}

fn roots() -> Roots {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let project = temp.path().join("project");
    let state = temp.path().join("state");
    for dir in [&home, &project, &state] {
        std::fs::create_dir_all(dir).unwrap();
    }
    Roots {
        _temp: temp,
        home,
        project,
        state,
    }
}

impl Roots {
    /// One `rkat` process on `realm` of this state root, run to its end.
    fn rkat(&self, realm: &str, args: &[&str]) -> Output {
        let binary = std::env::var_os("CARGO_BIN_EXE_rkat")
            .map(PathBuf::from)
            .expect("Cargo or Nextest must provide the runtime rkat binary path");
        let mut command = Command::new(binary);
        command
            .arg("--state-root")
            .arg(&self.state)
            .arg("--context-root")
            .arg(&self.project)
            .arg("--user-config-root")
            .arg(&self.home)
            .args(["--realm", realm])
            .args(args)
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .env("XDG_DATA_HOME", self.home.join("data"))
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("RKAT_TEST_CLIENT", "1")
            .stdin(Stdio::null());
        for name in [
            "RKAT_ANTHROPIC_API_KEY",
            "ANTHROPIC_API_KEY",
            "RKAT_OPENAI_API_KEY",
            "OPENAI_API_KEY",
            "RKAT_GEMINI_API_KEY",
            "GEMINI_API_KEY",
        ] {
            command.env_remove(name);
        }
        command.output().unwrap()
    }
}

fn report(step: &str, output: &Output) -> String {
    format!(
        "{step}: exit {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn realm_govern_right_after_realm_create_never_meets_a_leftover_manifest_lock() {
    let roots = roots();
    let mut met_the_leftover = Vec::new();
    let mut first_report = None;

    for round in 0..ROUNDS {
        let realm = format!("lab-{round}");
        let created = roots.rkat(&realm, &["realm", "create", &realm]);
        assert!(
            created.status.success(),
            "round {round}: {}",
            report("rkat realm create", &created)
        );

        // At once: nothing between the two commands.
        let governed = roots.rkat(&realm, &["realm", "govern", &realm]);
        if governed.status.success() {
            continue;
        }
        // The only failure this row counts is the lock timeout; anything
        // else is not this row's subject and fails it as it is.
        assert!(
            String::from_utf8_lossy(&governed.stderr).contains(LOCK_TIMEOUT),
            "round {round}: {}",
            report("rkat realm govern", &governed)
        );
        met_the_leftover.push(round);
        first_report.get_or_insert_with(|| report("rkat realm govern", &governed));
    }

    assert!(
        met_the_leftover.is_empty(),
        "{} of {ROUNDS} rounds met a manifest lock file left by `realm create` \
         (rounds {met_the_leftover:?}); the first:\n{}",
        met_the_leftover.len(),
        first_report.unwrap_or_default()
    );

    // The commands did their work: the last realm made is governed.
    let last = format!("lab-{}", ROUNDS - 1);
    let shown = roots.rkat(&last, &["realm", "show", &last]);
    assert!(
        shown.status.success(),
        "{}",
        report("rkat realm show", &shown)
    );
    assert!(
        String::from_utf8_lossy(&shown.stdout).contains("governance: governed"),
        "{}",
        report("rkat realm show", &shown)
    );
}
