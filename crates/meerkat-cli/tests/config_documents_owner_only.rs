//! The modes of the configuration documents `rkat` writes, on Unix. A
//! document can hold credential material, so `rkat` writes it owner-only
//! (0600) whatever its umask; in a directory whose setgid bit says it is
//! shared through its group, also readable by that group (0640). Every
//! `rkat` here is started by a shell that sets the permissive umask 022
//! first.
//!
//! Hermeticity: `HOME`, the XDG roots and the state root are in a temporary
//! directory.

#![cfg(unix)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::disallowed_methods
)]

use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

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
    for directory in [&home, &project, &state] {
        std::fs::create_dir_all(directory).unwrap();
    }
    Roots {
        _temp: temp,
        home,
        project,
        state,
    }
}

impl Roots {
    /// One `rkat` process with `args`, under umask 022, in the project
    /// directory.
    fn rkat(&self, args: &[&str]) -> Output {
        let binary = std::env::var_os("CARGO_BIN_EXE_rkat")
            .map(PathBuf::from)
            .expect("Cargo or Nextest must provide the runtime rkat binary path");
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "umask 022 && exec \"$@\"", "rkat-under-umask"])
            .arg(binary)
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

    /// [`Self::rkat`] on the realm `team` of this state root, which must
    /// succeed; its standard output.
    fn rkat_on_team_realm(&self, args: &[&str]) -> String {
        let state = self.state.to_string_lossy().into_owned();
        let project = self.project.to_string_lossy().into_owned();
        let home = self.home.to_string_lossy().into_owned();
        let mut all = vec![
            "--state-root",
            state.as_str(),
            "--context-root",
            project.as_str(),
            "--user-config-root",
            home.as_str(),
            "--realm",
            "team",
        ];
        all.extend_from_slice(args);
        let output = self.rkat(&all);
        assert!(
            output.status.success(),
            "rkat {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }
}

fn mode_bits(path: &Path) -> u32 {
    std::fs::symlink_metadata(path)
        .unwrap_or_else(|error| panic!("the metadata of {}: {error}", path.display()))
        .permissions()
        .mode()
        & 0o7777
}

/// `rkat init` under a permissive umask: the user document it creates from
/// the template, that document's generation sidecar and the project
/// document it copies are 0600; the `.rkat` directories it creates keep the
/// umask's mode. RED before: all three files were 0644, readable by every
/// local user.
#[test]
fn init_writes_owner_only_documents_under_a_permissive_umask() {
    let roots = roots();
    let output = roots.rkat(&["init"]);
    assert!(
        output.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let user = roots.home.join(".rkat");
    let project = roots.project.join(".rkat");
    for directory in [&user, &project] {
        assert_eq!(
            mode_bits(directory),
            0o755,
            "{} keeps the umask's mode",
            directory.display()
        );
    }
    for document in [
        user.join("config.toml"),
        user.join("config_state.json"),
        project.join("config.toml"),
    ] {
        assert_eq!(
            mode_bits(&document),
            0o600,
            "{} is owner-only",
            document.display()
        );
    }
}

/// `rkat init` in a project whose `.rkat` directory has the setgid bit,
/// shared through its group on purpose: the project document is 0640 and
/// belongs to the directory's group. The user document, in an ordinary
/// directory, stays 0600.
#[test]
fn init_shares_a_project_document_with_the_group_of_a_setgid_directory() {
    let roots = roots();
    let project = roots.project.join(".rkat");
    std::fs::create_dir(&project).unwrap();
    std::fs::set_permissions(&project, std::fs::Permissions::from_mode(0o2770)).unwrap();
    assert_eq!(mode_bits(&project), 0o2770, "the directory is setgid");
    let output = roots.rkat(&["init"]);
    assert!(
        output.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document = project.join("config.toml");
    assert_eq!(mode_bits(&document), 0o640, "group-readable, nothing more");
    assert_eq!(
        std::fs::metadata(&document).unwrap().gid(),
        std::fs::metadata(&project).unwrap().gid(),
        "the document belongs to the directory's group"
    );
    assert_eq!(
        mode_bits(&roots.home.join(".rkat").join("config.toml")),
        0o600,
        "the user document is owner-only"
    );
}

/// No self-lockout: a document one `rkat` process wrote owner-only is read
/// and rewritten by later processes of the same user, and stays 0600.
#[test]
fn a_second_process_of_the_same_user_reads_and_rewrites_an_owner_only_document() {
    let roots = roots();
    roots.rkat_on_team_realm(&["config", "patch", "--json", r#"{"max_tokens": 1234}"#]);
    let realm = roots.state.join("team");
    let document = realm.join("config.toml");
    let sidecar = realm.join("config_state.json");
    assert_eq!(mode_bits(&document), 0o600, "the first process's document");
    assert_eq!(mode_bits(&sidecar), 0o600, "and its generation sidecar");

    let read = roots.rkat_on_team_realm(&["config", "get", "--format", "json"]);
    assert!(read.contains("1234"), "a second process reads it: {read}");

    roots.rkat_on_team_realm(&["config", "patch", "--json", r#"{"max_tokens": 2345}"#]);
    let read = roots.rkat_on_team_realm(&["config", "get", "--format", "json"]);
    assert!(read.contains("2345"), "and rewrites it: {read}");
    assert_eq!(mode_bits(&document), 0o600, "the rewritten document");
    assert_eq!(mode_bits(&sidecar), 0o600, "and its generation sidecar");
}
