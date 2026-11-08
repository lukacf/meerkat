//! `rkat auth connector-client` writes a client's document entry and its
//! secret as one guarded operation: while an admission holds the realm
//! document's fence, `set`, `rotate-secret` and `remove` refuse before
//! anything is written (no secret is stored, replaced or removed and the
//! document is unchanged); once it releases, they land. Each of them reports
//! the realm document's config warnings on stderr.
//!
//! Hermeticity: `HOME`, `XDG_CONFIG_HOME` and `XDG_DATA_HOME` point into a
//! temporary directory, and `--state-root` names its realms.

#![cfg(unix)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::disallowed_methods
)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use meerkat_core::connection::RealmId;
use meerkat_core::realm_admission::{AdmissionFence, AdmissionHold};

struct Cli {
    home: tempfile::TempDir,
}

impl Cli {
    fn new() -> Self {
        Self {
            home: tempfile::tempdir().unwrap(),
        }
    }

    fn realms(&self) -> PathBuf {
        self.home.path().join("realms")
    }

    fn credentials(&self) -> PathBuf {
        #[cfg(target_os = "macos")]
        let config_root = self.home.path().join("Library").join("Application Support");
        #[cfg(not(target_os = "macos"))]
        let config_root = self.home.path().join("config");
        config_root.join("meerkat").join("credentials")
    }

    fn document(&self) -> PathBuf {
        self.realms().join("team").join("config.toml")
    }

    /// Run `rkat --state-root .. --realm team auth connector-client <args>`
    /// with `stdin` piped in.
    fn run(&self, args: &[&str], stdin: &str) -> Output {
        let binary = std::env::var_os("CARGO_BIN_EXE_rkat")
            .map(PathBuf::from)
            .expect("Cargo or Nextest must provide the runtime rkat binary path");
        let realms = self.realms();
        let mut child = Command::new(binary)
            .current_dir(self.home.path())
            .env("HOME", self.home.path())
            .env("XDG_CONFIG_HOME", self.home.path().join("config"))
            .env("XDG_DATA_HOME", self.home.path().join("data"))
            .env("RKAT_TEST_CLIENT", "1")
            .args(["--state-root", realms.to_str().unwrap(), "--realm", "team"])
            .args(["auth", "connector-client"])
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    /// Hold the realm document's admission fence as an admission does.
    fn hold_document(&self) -> AdmissionHold {
        let fence = AdmissionFence::realm(&self.realms(), &RealmId::parse("team").unwrap())
            .expect("the realm document's fence");
        fence
            .ensure_exists()
            .expect("the fence a holder needs to exist");
        AdmissionHold::try_acquire(vec![fence]).expect("an admission holds the document's fence")
    }

    /// Every credential file under the credential root, with its bytes:
    /// the stored secrets (fences and lock files are empty files and named
    /// apart).
    fn stored_secrets(&self) -> Vec<(PathBuf, Vec<u8>)> {
        fn walk(directory: &Path, found: &mut Vec<(PathBuf, Vec<u8>)>) {
            let Ok(entries) = std::fs::read_dir(directory) else {
                return;
            };
            for entry in entries {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, found);
                } else if path
                    .extension()
                    .is_some_and(|extension| extension == "json")
                {
                    found.push((path.clone(), std::fs::read(&path).unwrap()));
                }
            }
        }
        let mut found = Vec::new();
        walk(&self.credentials(), &mut found);
        found.sort();
        found
    }
}

fn refused(output: &Output, what: &str) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "{what} refuses under the held fence: {stderr}"
    );
}

fn landed(output: &Output, what: &str) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{what} lands: {stderr}");
}

const SET: &[&str] = &[
    "set",
    "desktop",
    "--issuer",
    "https://issuer.example",
    "--client",
    "client",
    "--client-secret",
    "--secret-stdin",
];

#[test]
fn connector_client_commands_use_the_guarded_funnel() {
    let cli = Cli::new();

    // set: no secret is stored and no document written while held.
    let mut hold = cli.hold_document();
    let output = cli.run(SET, "secret-one\n");
    hold.release();
    refused(&output, "set");
    assert!(
        cli.stored_secrets().is_empty(),
        "no secret was stored: {:?}",
        cli.stored_secrets()
    );
    assert!(!cli.document().exists(), "no document was written");

    landed(&cli.run(SET, "secret-one\n"), "set");
    let secrets = cli.stored_secrets();
    assert_eq!(secrets.len(), 1, "the client's secret is stored");
    let document = std::fs::read(cli.document()).unwrap();
    assert!(
        String::from_utf8_lossy(&document).contains("desktop"),
        "the document names the client"
    );

    // rotate-secret and remove: the secret and the document are unchanged
    // while held.
    let mut hold = cli.hold_document();
    let rotation = cli.run(&["rotate-secret", "desktop"], "secret-two\n");
    let removal = cli.run(&["remove", "desktop"], "");
    hold.release();
    refused(&rotation, "rotate-secret");
    refused(&removal, "remove");
    assert_eq!(
        cli.stored_secrets(),
        secrets,
        "the stored secret is unchanged"
    );
    assert_eq!(std::fs::read(cli.document()).unwrap(), document);

    landed(&cli.run(&["remove", "desktop"], ""), "remove");
    assert!(cli.stored_secrets().is_empty(), "the secret is removed");
    assert!(
        !String::from_utf8_lossy(&std::fs::read(cli.document()).unwrap()).contains("desktop"),
        "the client is removed from the document"
    );
}

/// `what` printed the realm document's config warning on stderr once: one
/// line that names the document, the realm and the legacy table.
fn reports_the_documents_warning(output: &Output, what: &str) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let lines: Vec<&str> = stderr
        .lines()
        .filter(|line| {
            line.starts_with("warning: config ")
                && line.contains("team/config.toml (realm 'team'): ")
        })
        .collect();
    assert_eq!(
        lines.len(),
        1,
        "{what} reports the document's config warning once: {stderr}"
    );
    assert!(
        lines[0].contains("model_fallback.enabled = true"),
        "{what} names the legacy table: {stderr}"
    );
}

/// A legacy `[model_fallback]` table: the document loads with a config
/// warning (the table loads as no fallback policy), and a write of the
/// document persists it without its legacy key.
const LEGACY_FALLBACK: &str = "[model_fallback]\nenabled = true\n";

/// Each command reads the realm's own document, and `set` and `remove`
/// rewrite it, dropping the legacy key: each reports the document's config
/// warning on stderr, as every CLI command that loads or rewrites a persisted
/// config document reports it, so the command that drops the key says so.
#[test]
fn connector_client_commands_report_the_documents_config_warnings() {
    // `set` on a document that holds only the legacy table.
    let cli = Cli::new();
    std::fs::create_dir_all(cli.document().parent().unwrap()).unwrap();
    std::fs::write(cli.document(), LEGACY_FALLBACK).unwrap();
    let set = cli.run(SET, "secret-one\n");
    landed(&set, "set");
    reports_the_documents_warning(&set, "set");

    // `rotate-secret`, then `remove`, on a document that names the client
    // and holds the legacy table.
    let cli = Cli::new();
    landed(&cli.run(SET, "secret-one\n"), "set");
    let mut document = std::fs::read_to_string(cli.document()).unwrap();
    document.push('\n');
    document.push_str(LEGACY_FALLBACK);
    std::fs::write(cli.document(), document).unwrap();
    let rotation = cli.run(&["rotate-secret", "desktop"], "secret-two\n");
    landed(&rotation, "rotate-secret");
    reports_the_documents_warning(&rotation, "rotate-secret");
    let removal = cli.run(&["remove", "desktop"], "");
    landed(&removal, "remove");
    reports_the_documents_warning(&removal, "remove");
}
