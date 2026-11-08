//! End-to-end coverage for `rkat storage hosts-fences`: the admission fence
//! filesystem predicate (`meerkat_core::realm_admission::directory_hosts_fences`)
//! run in the CLI's own view of the filesystem, one JSON object per directory
//! and line, in argument order.
//!
//! Hermeticity: `HOME` and `XDG_DATA_HOME` point into a temporary directory,
//! and the storage verbs resolve no runtime scope.

#![cfg(unix)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::disallowed_methods
)]

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

fn hosts_fences(temp: &TempDir, directories: &[&Path]) -> Output {
    let binary = std::env::var_os("CARGO_BIN_EXE_rkat")
        .map(PathBuf::from)
        .expect("Cargo or Nextest must provide the runtime rkat binary path");
    Command::new(binary)
        .current_dir(temp.path())
        .env("HOME", temp.path())
        .env("XDG_DATA_HOME", temp.path().join("data"))
        .env("RKAT_TEST_CLIENT", "1")
        .args(["storage", "hosts-fences"])
        .args(directories)
        .output()
        .unwrap()
}

fn describe(output: &Output) -> String {
    format!(
        "status: {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// The JSON objects a successful run printed, one per line.
fn reported(output: &Output) -> Vec<serde_json::Value> {
    assert_eq!(output.status.code(), Some(0), "{}", describe(output));
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("not JSON ({error}): {line}\n{}", describe(output)))
        })
        .collect()
}

fn text(path: &Path) -> &str {
    path.to_str().expect("a UTF-8 temporary path")
}

/// A directory whose filesystem can host a fence reports `true`.
#[test]
fn a_hosted_directory_reports_true() {
    let temp = TempDir::new().unwrap();
    let directory = temp.path().join("state");
    std::fs::create_dir(&directory).unwrap();
    let output = hosts_fences(&temp, &[&directory]);
    assert_eq!(
        reported(&output),
        vec![serde_json::json!({ "dir": text(&directory), "hosts": true })]
    );
}

/// A directory on a filesystem that cannot host a fence (procfs on Linux,
/// devfs on macOS) reports `false`.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn an_excluded_filesystem_reports_false() {
    let temp = TempDir::new().unwrap();
    let excluded = if cfg!(target_os = "macos") {
        Path::new("/dev")
    } else {
        Path::new("/proc")
    };
    let output = hosts_fences(&temp, &[excluded]);
    assert_eq!(
        reported(&output),
        vec![serde_json::json!({ "dir": text(excluded), "hosts": false })]
    );
}

/// A directory the predicate cannot inspect (here, one that does not exist)
/// reports the typed reason, never an answer, and the run still exits 0.
#[test]
fn a_missing_directory_reports_the_typed_unavailable_result() {
    let temp = TempDir::new().unwrap();
    let missing = temp.path().join("missing");
    let output = hosts_fences(&temp, &[&missing]);
    let lines = reported(&output);
    assert_eq!(lines.len(), 1, "{}", describe(&output));
    let line = &lines[0];
    assert_eq!(line.as_object().unwrap().len(), 2, "{line}");
    assert_eq!(line["dir"], text(&missing));
    let unavailable = line["unavailable"].as_object().unwrap();
    assert_eq!(unavailable.len(), 2, "{line}");
    assert_eq!(unavailable["kind"], "filesystem");
    assert!(
        unavailable["cause"]
            .as_str()
            .is_some_and(|cause| !cause.is_empty()),
        "{line}"
    );
}

/// Every directory is reported, in argument order, whatever its answer, and
/// the run exits 0.
#[test]
fn every_directory_is_reported_in_argument_order() {
    let temp = TempDir::new().unwrap();
    let missing = temp.path().join("missing");
    let output = hosts_fences(&temp, &[&missing, temp.path(), &missing]);
    let lines = reported(&output);
    let directories: Vec<&str> = lines
        .iter()
        .map(|line| line["dir"].as_str().unwrap())
        .collect();
    assert_eq!(
        directories,
        [text(&missing), text(temp.path()), text(&missing)]
    );
    assert_eq!(lines[0]["unavailable"]["kind"], "filesystem");
    assert_eq!(lines[1]["hosts"], true);
    assert_eq!(lines[2]["unavailable"]["kind"], "filesystem");
}

/// A run without a directory is a usage error: exit 2, the missing argument
/// named, and nothing reported.
#[test]
fn no_directory_is_a_usage_error() {
    let temp = TempDir::new().unwrap();
    let output = hosts_fences(&temp, &[]);
    assert_eq!(output.status.code(), Some(2), "{}", describe(&output));
    assert!(output.stdout.is_empty(), "{}", describe(&output));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("<DIR>"),
        "{}",
        describe(&output)
    );
}

/// The entries of `directory`, sorted.
fn entries(directory: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    entries.sort();
    entries
}

/// The probe runs before any realm, state, config, auth or migration step:
/// with `HOME`, the XDG directories and the working directory all in an
/// empty directory, nothing is created there (no realm directory, no config
/// file, no migration marker), and nothing in the probed directory either.
#[test]
fn the_probe_creates_nothing_and_opens_no_state() {
    let home = TempDir::new().unwrap();
    let probed = TempDir::new().unwrap();
    let directory = probed.path().join("state");
    std::fs::create_dir(&directory).unwrap();
    let binary = std::env::var_os("CARGO_BIN_EXE_rkat")
        .map(PathBuf::from)
        .expect("Cargo or Nextest must provide the runtime rkat binary path");
    let output = Command::new(binary)
        .current_dir(home.path())
        .env("HOME", home.path())
        .env("XDG_DATA_HOME", home.path().join("data"))
        .env("XDG_CONFIG_HOME", home.path().join("config"))
        .env("XDG_STATE_HOME", home.path().join("state"))
        .env("XDG_CACHE_HOME", home.path().join("cache"))
        .env("RKAT_TEST_CLIENT", "1")
        .args(["storage", "hosts-fences"])
        .arg(&directory)
        .output()
        .unwrap();
    assert_eq!(
        reported(&output),
        vec![serde_json::json!({ "dir": text(&directory), "hosts": true })]
    );
    assert_eq!(entries(home.path()), Vec::<PathBuf>::new());
    assert_eq!(entries(probed.path()), vec![directory.clone()]);
    assert_eq!(entries(&directory), Vec::<PathBuf>::new());
}

/// A global option names state the probe never reads, so giving one, before
/// or after the subcommand, is a usage error: exit 2, nothing reported, and
/// nothing created at the state root it names.
#[test]
fn a_global_option_is_a_usage_error() {
    let temp = TempDir::new().unwrap();
    let state_root = temp.path().join("realms");
    let directory = temp.path().join("state");
    std::fs::create_dir(&directory).unwrap();
    let binary = std::env::var_os("CARGO_BIN_EXE_rkat")
        .map(PathBuf::from)
        .expect("Cargo or Nextest must provide the runtime rkat binary path");
    let cases: [Vec<&OsStr>; 3] = [
        vec![OsStr::new("--state-root"), state_root.as_os_str()],
        vec![OsStr::new("--realm"), OsStr::new("team")],
        vec![OsStr::new("--isolated")],
    ];
    for args in cases {
        for before in [true, false] {
            let mut command = Command::new(&binary);
            command
                .current_dir(temp.path())
                .env("HOME", temp.path())
                .env("XDG_DATA_HOME", temp.path().join("data"))
                .env("RKAT_TEST_CLIENT", "1");
            if before {
                command.args(&args).args(["storage", "hosts-fences"]);
            } else {
                command.args(["storage", "hosts-fences"]).args(&args);
            }
            let output = command.arg(&directory).output().unwrap();
            assert_eq!(output.status.code(), Some(2), "{}", describe(&output));
            assert!(output.stdout.is_empty(), "{}", describe(&output));
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("takes only its directories"),
                "{}",
                describe(&output)
            );
        }
    }
    assert_eq!(entries(temp.path()), vec![directory]);
}

/// When the report cannot be written (stdout on a full device), the run
/// exits 1 with the reason on stderr, and creates nothing.
#[cfg(target_os = "linux")]
#[test]
fn an_unwritable_report_exits_1() {
    let temp = TempDir::new().unwrap();
    let directory = temp.path().join("state");
    std::fs::create_dir(&directory).unwrap();
    let full = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .unwrap();
    let binary = std::env::var_os("CARGO_BIN_EXE_rkat")
        .map(PathBuf::from)
        .expect("Cargo or Nextest must provide the runtime rkat binary path");
    let output = Command::new(binary)
        .current_dir(temp.path())
        .env("HOME", temp.path())
        .env("XDG_DATA_HOME", temp.path().join("data"))
        .env("RKAT_TEST_CLIENT", "1")
        .args(["storage", "hosts-fences"])
        .arg(&directory)
        .stdout(full)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", describe(&output));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("the hosts-fences report could not be written"),
        "{}",
        describe(&output)
    );
    assert_eq!(entries(temp.path()), vec![directory.clone()]);
    assert_eq!(entries(&directory), Vec::<PathBuf>::new());
}
