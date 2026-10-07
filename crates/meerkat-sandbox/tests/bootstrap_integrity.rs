//! Mutable target executables enter Seatbelt before any target bytes execute.
//! The retired helper-integrity probes remain in the original source snapshot.
//! These probes require real target mutation, not preservation of an unused helper.
#![cfg(target_os = "macos")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Output;

use meerkat_core::confinement::{
    ConfinementSpec, ExecutionConfinement, FilesystemAccess, IpNetworkAccess, PathAccess,
    PlatformBaseline,
};
use meerkat_sandbox::{CompiledConfinement, PreparedConfinement, ProcessLaunchSpec, prepare};

const PASSED: &str = "mutable target native probe passed";
const ENTERED: &str = "mutated target entered";
const SECRET: &[u8] = b"isolated target escape canary\n";
const PAYLOAD: &[u8] = b"#!/bin/sh\nprintf 'mutated target entered\\n'\n/bin/cat \"$TARGET_CANARY\"\nexec \"$TARGET_ORIGINAL_PROBE\" --ignored --exact mutable_target_native_probe --nocapture\n";

struct Fixture {
    _root: tempfile::TempDir,
    root: PathBuf,
    install: PathBuf,
    target: PathBuf,
    work: PathBuf,
    outside: PathBuf,
    original_probe: PathBuf,
    original: Vec<u8>,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        let install = root.join("installation");
        let work = root.join("work");
        std::fs::create_dir(&install).unwrap();
        std::fs::create_dir(&work).unwrap();
        let target = install.join("mutable-target");
        let original_probe = std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
        std::fs::copy(&original_probe, &target).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        let original = std::fs::read(&target).unwrap();
        std::fs::write(&target, &original).expect("the target must be owner-writable");
        let metadata = std::fs::metadata(&target).unwrap();
        assert_eq!(metadata.nlink(), 1);
        assert_eq!(metadata.uid(), std::fs::metadata(&root).unwrap().uid());
        let outside = root.join("private-canary");
        std::fs::write(&outside, SECRET).unwrap();
        Self {
            _root: temp,
            root,
            install,
            target,
            work,
            outside,
            original_probe,
            original,
        }
    }

    fn requirement(&self, writable: &Path) -> ExecutionConfinement {
        ConfinementSpec {
            baseline: PlatformBaseline::CommandRuntimeV1,
            read: FilesystemAccess::Paths(vec![
                PathAccess::Subtree(self.install.clone()),
                PathAccess::Subtree(self.work.clone()),
                PathAccess::Literal(self.original_probe.clone()),
            ]),
            write: FilesystemAccess::Paths(vec![PathAccess::Subtree(writable.to_owned())]),
            deny_read: vec![],
            deny_write: vec![],
            unix_connect: vec![],
            network: IpNetworkAccess::Denied,
            require_descendant_termination: false,
        }
        .try_into()
        .unwrap()
    }

    fn launch(&self, program: &Path, mode: &str, first: &Path) -> ProcessLaunchSpec {
        ProcessLaunchSpec::new(
            program.to_owned(),
            [
                "--ignored",
                "--exact",
                "mutable_target_native_probe",
                "--nocapture",
            ]
            .into_iter()
            .map(OsString::from)
            .collect(),
            self.work.clone(),
            BTreeMap::from([
                ("TARGET_MODE".into(), mode.into()),
                ("TARGET_FIRST".into(), first.as_os_str().to_owned()),
                ("TARGET_WORK".into(), self.work.as_os_str().to_owned()),
                ("TARGET_CANARY".into(), self.outside.as_os_str().to_owned()),
                (
                    "TARGET_ORIGINAL_PROBE".into(),
                    self.original_probe.as_os_str().to_owned(),
                ),
            ]),
        )
        .unwrap()
    }

    fn prepare_target(&self) -> PreparedConfinement {
        prepare(
            &self.requirement(&self.work),
            self.launch(&self.target, "read-denied", &self.outside),
        )
        .expect("the target must prepare without an installed helper")
    }

    async fn positive_controls(&self) {
        let target = prepare(
            &self.requirement(&self.work),
            self.launch(&self.target, "write-allowed", &self.work.join("allowed")),
        )
        .unwrap()
        .output()
        .await
        .unwrap();
        assert_probe(target);
        assert_eq!(
            std::fs::read(self.work.join("allowed")).unwrap(),
            b"ordinary write"
        );
        let ordinary = self.install.join("other-profile-ordinary-write");
        let writer = prepare(
            &self.requirement(&self.root),
            self.launch(&self.original_probe, "write-allowed", &ordinary),
        )
        .unwrap()
        .output()
        .await
        .unwrap();
        assert_probe(writer);
        assert_eq!(std::fs::read(ordinary).unwrap(), b"ordinary write");
        assert_eq!(std::fs::read(&self.target).unwrap(), self.original);
    }

    async fn mutate_from_other_profile(&self, mode: &str) {
        let output = prepare(
            &self.requirement(&self.root),
            self.launch(&self.original_probe, mode, &self.target),
        )
        .unwrap()
        .output()
        .await
        .unwrap();
        assert_probe(output);
        assert_eq!(
            std::fs::read(&self.target).unwrap(),
            PAYLOAD,
            "the other confined profile must actually replace target bytes"
        );
    }

    fn assert_mutated_execution(&self, output: Output) {
        assert!(
            !output
                .stdout
                .windows(SECRET.len())
                .any(|bytes| bytes == SECRET),
            "mutated target executed outside Seatbelt and disclosed the private canary"
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains(ENTERED),
            "the mutated executable must actually run: {output:?}"
        );
        assert_probe(output);
        assert_eq!(std::fs::read(&self.target).unwrap(), PAYLOAD);
        assert_eq!(std::fs::read(&self.outside).unwrap(), SECRET);
    }
}

fn assert_probe(output: Output) {
    assert!(
        output.status.success(),
        "actual native probe failed: {output:?}"
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains(PASSED),
        "actual native probe did not complete: {output:?}"
    );
}

#[test]
#[ignore = "subprocess entrypoint for mutable target probes"]
fn mutable_target_native_probe() {
    use std::io::Write;
    fn denied<T>(result: std::io::Result<T>) {
        match result {
            Err(error)
                if matches!(
                    error.raw_os_error(),
                    Some(nix::libc::EPERM | nix::libc::EACCES)
                ) => {}
            Err(error) => panic!("unrelated failure instead of enforcement: {error}"),
            Ok(_) => panic!("target confinement was bypassed"),
        }
    }
    let first = PathBuf::from(std::env::var_os("TARGET_FIRST").unwrap());
    let work = PathBuf::from(std::env::var_os("TARGET_WORK").unwrap());
    let canary = PathBuf::from(std::env::var_os("TARGET_CANARY").unwrap());
    match std::env::var("TARGET_MODE").unwrap().as_str() {
        "write-allowed" => std::fs::write(first, b"ordinary write").unwrap(),
        "read-denied" => denied(std::fs::read(first)),
        "overwrite-target" => {
            denied(std::fs::read(canary));
            std::fs::write(first, PAYLOAD).expect("confined writer must mutate the allowed target");
        }
        "replace-target" => {
            denied(std::fs::read(canary));
            let ordinary = first.parent().unwrap().join("ordinary-replacement-control");
            let source = work.join("ordinary-replacement-source");
            std::fs::write(&ordinary, b"old ordinary file").unwrap();
            std::fs::write(&source, b"new ordinary file").unwrap();
            std::fs::rename(&source, &ordinary).expect("ordinary rename-over must work");
            assert_eq!(std::fs::read(&ordinary).unwrap(), b"new ordinary file");
            let replacement = work.join("replacement-target");
            std::fs::write(&replacement, PAYLOAD).unwrap();
            std::fs::set_permissions(&replacement, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert_ne!(
                std::fs::metadata(&replacement).unwrap().ino(),
                std::fs::metadata(&first).unwrap().ino()
            );
            std::fs::rename(replacement, first)
                .expect("confined writer must atomically replace the allowed target");
        }
        _ => panic!("unknown mutable target probe mode"),
    }
    println!("{PASSED}");
    std::io::stdout().flush().unwrap();
    std::process::exit(0);
}

#[tokio::test]
async fn mutable_target_owned_by_user_remains_usable_without_a_helper() {
    let fixture = Fixture::new();
    fixture.positive_controls().await;
    assert_probe(fixture.prepare_target().output().await.unwrap());
}

#[tokio::test]
async fn mutable_target_overwritten_after_binding_still_enters_seatbelt() {
    let fixture = Fixture::new();
    fixture.positive_controls().await;
    let prepared = fixture.prepare_target();
    let inode = std::fs::metadata(&fixture.target).unwrap().ino();
    fixture.mutate_from_other_profile("overwrite-target").await;
    assert_eq!(std::fs::metadata(&fixture.target).unwrap().ino(), inode);
    fixture.assert_mutated_execution(prepared.output().await.unwrap());
}

#[tokio::test]
async fn mutable_target_corrupted_before_fresh_compilation_still_enters_seatbelt() {
    let fixture = Fixture::new();
    fixture.positive_controls().await;
    fixture.mutate_from_other_profile("overwrite-target").await;
    let compiled = CompiledConfinement::compile(&fixture.requirement(&fixture.work)).unwrap();
    let prepared = compiled
        .bind_launch(fixture.launch(&fixture.target, "read-denied", &fixture.outside))
        .unwrap();
    fixture.assert_mutated_execution(prepared.output().await.unwrap());
}

#[tokio::test]
async fn mutable_target_atomically_replaced_after_preparation_still_enters_seatbelt() {
    let fixture = Fixture::new();
    fixture.positive_controls().await;
    let prepared = fixture.prepare_target();
    let inode = std::fs::metadata(&fixture.target).unwrap().ino();
    fixture.mutate_from_other_profile("replace-target").await;
    assert_ne!(std::fs::metadata(&fixture.target).unwrap().ino(), inode);
    fixture.assert_mutated_execution(prepared.output().await.unwrap());
}
