//! Helper-free compile/bind execution and ownership of native children.
#![cfg(any(target_os = "linux", target_os = "macos"))]
#![allow(clippy::unwrap_used, clippy::expect_used, unsafe_code)]

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::time::Duration;

use meerkat_core::confinement::{
    ConfinementSpec, FilesystemAccess, IpNetworkAccess, PathAccess, PlatformBaseline,
};
use meerkat_sandbox::{
    CompiledConfinement, ConfinementBackend, ConfinementRefusal, ExecutionConfinement, NativeChild,
    ProcessLaunchSpec, SpawnIo,
};

struct Fixture {
    _root: tempfile::TempDir,
    root: PathBuf,
    requirement: ExecutionConfinement,
}

#[tokio::test]
#[cfg_attr(
    target_os = "linux",
    ignore = "Linux positive confinement acceptance lane; requires an eligible host"
)]
async fn confined_custody_gate_waits_for_release_and_preserves_pid() {
    let fixture = Fixture::new();
    let marker = fixture.root.join("gate-target-ran");
    let prepared = CompiledConfinement::compile(&fixture.requirement)
        .unwrap()
        .bind_launch(fixture.shell(
            "printf ran > \"$MARKER\"; if /bin/bash -c 'printf leaked >&3'; then exit 31; fi; printf '%s' \"$$\"",
            BTreeMap::from([("MARKER".into(), marker.as_os_str().to_owned())]),
        ))
        .unwrap();
    let (reader, mut writer) = std::io::pipe().unwrap();
    let mut child = prepared
        .spawn_behind_gate(
            reader.as_fd(),
            OsStr::new("host-release"),
            SpawnIo::default(),
        )
        .unwrap();
    drop(reader);
    let pid = child.id().unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), child.wait())
            .await
            .is_err()
    );
    assert!(!marker.exists(), "target ran before host release");
    writer.write_all(b"host-release\n").unwrap();
    drop(writer);
    let output = tokio::time::timeout(Duration::from_secs(5), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(output.stdout, pid.to_string().as_bytes());
    assert_eq!(std::fs::read(marker).unwrap(), b"ran");
}

#[tokio::test]
#[cfg_attr(
    target_os = "linux",
    ignore = "Linux positive confinement acceptance lane; requires an eligible host"
)]
async fn confined_custody_gate_eof_or_wrong_release_never_runs_target() {
    for wrong_release in [false, true] {
        let fixture = Fixture::new();
        let marker = fixture.root.join("gate-target-ran");
        let prepared = CompiledConfinement::compile(&fixture.requirement)
            .unwrap()
            .bind_launch(fixture.shell(
                "printf ran > \"$MARKER\"",
                BTreeMap::from([("MARKER".into(), marker.as_os_str().to_owned())]),
            ))
            .unwrap();
        let (reader, mut writer) = std::io::pipe().unwrap();
        let child = prepared
            .spawn_behind_gate(
                reader.as_fd(),
                OsStr::new("host-release"),
                SpawnIo::default(),
            )
            .unwrap();
        drop(reader);
        if wrong_release {
            writer.write_all(b"wrong-release\n").unwrap();
        }
        drop(writer);
        let output = tokio::time::timeout(Duration::from_secs(5), child.wait_with_output())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(output.status.code(), Some(125), "{output:?}");
        assert!(!marker.exists(), "unreleased target ran");
    }
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(temp.path()).unwrap();
        let requirement = ConfinementSpec {
            baseline: PlatformBaseline::CommandRuntimeV1,
            read: FilesystemAccess::Paths(vec![PathAccess::Subtree(root.clone())]),
            write: FilesystemAccess::Paths(vec![PathAccess::Subtree(root.clone())]),
            deny_read: vec![],
            deny_write: vec![],
            network: IpNetworkAccess::Denied,
            unix_connect: vec![],
            require_descendant_termination: false,
        }
        .try_into()
        .unwrap();
        Self {
            _root: temp,
            root,
            requirement,
        }
    }

    fn shell(&self, script: &str, environment: BTreeMap<OsString, OsString>) -> ProcessLaunchSpec {
        ProcessLaunchSpec::new(
            PathBuf::from("/bin/sh"),
            vec!["-c".into(), script.into()],
            self.root.clone(),
            environment,
        )
        .unwrap()
    }

    fn launch(&self, value: &str) -> ProcessLaunchSpec {
        self.shell(
            "test -z \"${HOME+x}\" && printf '%s' \"$EXACT_VALUE\"",
            BTreeMap::from([("EXACT_VALUE".into(), value.into())]),
        )
    }

    async fn running_child(&self) -> NativeChild {
        let ready = self.root.join("ready");
        let compiled = CompiledConfinement::compile(&self.requirement).unwrap();
        let mut child = compiled
            .bind_launch(self.shell(
                "printf ready > \"$READY\"; exec /bin/sleep 60",
                BTreeMap::from([("READY".into(), ready.as_os_str().to_owned())]),
            ))
            .unwrap()
            .spawn()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !ready.exists() {
                assert!(
                    child.try_wait().unwrap().is_none(),
                    "child exited before readiness"
                );
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the real confined child must report readiness");
        assert_eq!(std::fs::read(ready).unwrap(), b"ready");
        child
    }
}

#[tokio::test]
#[cfg_attr(
    target_os = "linux",
    ignore = "Linux positive confinement acceptance lane; requires an eligible host"
)]
async fn one_compiled_profile_binds_distinct_launches_with_exact_capability_report() {
    let fixture = Fixture::new();
    let compiled = CompiledConfinement::compile(&fixture.requirement).unwrap();
    #[cfg(target_os = "macos")]
    assert_eq!(
        compiled.capabilities().backend(),
        ConfinementBackend::MacOsSeatbeltV1
    );
    #[cfg(target_os = "linux")]
    assert_eq!(
        compiled.capabilities().backend(),
        ConfinementBackend::LinuxNamespaceSeccompV1
    );
    assert_eq!(compiled.capabilities().requirement(), &fixture.requirement);
    assert!(!format!("{compiled:?}").contains(fixture.root.to_str().unwrap()));
    assert!(!format!("{:?}", compiled.capabilities()).contains(fixture.root.to_str().unwrap()));
    for value in ["first exact launch", "second exact launch"] {
        let output = compiled
            .bind_launch(fixture.launch(value))
            .unwrap()
            .output()
            .await
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(output.stdout, value.as_bytes());
    }
}

#[test]
fn setup_refuses_exact_ip_instead_of_broadening_the_requirement() {
    let fixture = Fixture::new();
    for endpoint in ["127.0.0.1:443", "[::1]:443", "192.0.2.1:443"] {
        let mut spec = fixture.requirement.specification().clone();
        spec.network = IpNetworkAccess::Connect(vec![endpoint.parse().unwrap()]);
        let required = ExecutionConfinement::try_from(spec).unwrap();
        assert!(matches!(
            CompiledConfinement::compile(&required),
            Err(ConfinementRefusal::UnsupportedRequirement)
        ));
    }
}

#[tokio::test]
#[cfg_attr(
    target_os = "linux",
    ignore = "Linux positive confinement acceptance lane; requires an eligible host"
)]
async fn cancelled_native_wait_retains_child_for_kill_and_cached_reaping() {
    let fixture = Fixture::new();
    let mut child = fixture.running_child().await;
    let pid = child.id().expect("live child PID");
    assert!(
        tokio::time::timeout(Duration::from_millis(20), child.wait())
            .await
            .is_err()
    );
    assert_eq!(child.id(), Some(pid));
    assert!(child.try_wait().unwrap().is_none());
    tokio::time::timeout(Duration::from_secs(5), child.kill())
        .await
        .unwrap()
        .unwrap();
    let status = child.wait().await.unwrap();
    assert_eq!(status.signal(), Some(nix::libc::SIGKILL));
    assert_eq!(child.try_wait().unwrap(), Some(status));
    assert_eq!(child.wait().await.unwrap(), status);
    assert_eq!(child.id(), None);
    let mut raw_status = 0;
    // SAFETY: This nonblocking query only checks that our exact child was reaped.
    assert_eq!(
        unsafe {
            nix::libc::waitpid(
                pid as i32,
                std::ptr::addr_of_mut!(raw_status),
                nix::libc::WNOHANG,
            )
        },
        -1
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(nix::libc::ECHILD)
    );
}

#[tokio::test]
#[cfg_attr(
    target_os = "linux",
    ignore = "Linux positive confinement acceptance lane; requires an eligible host"
)]
async fn dropping_live_native_child_kills_and_reaps_the_exact_pid() {
    let fixture = Fixture::new();
    let child = fixture.running_child().await;
    let pid = child.id().expect("live child PID");
    drop(child);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let mut information = std::mem::MaybeUninit::<nix::libc::siginfo_t>::uninit();
            // SAFETY: waitid writes siginfo_t. WNOWAIT observes without stealing
            // the exit status from the child's sole cleanup owner.
            let result = unsafe {
                nix::libc::waitid(
                    nix::libc::P_PID,
                    pid as nix::libc::id_t,
                    information.as_mut_ptr(),
                    nix::libc::WEXITED | nix::libc::WNOHANG | nix::libc::WNOWAIT,
                )
            };
            if result == -1 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() == Some(nix::libc::ECHILD) {
                    break;
                }
                assert_eq!(error.kind(), std::io::ErrorKind::Interrupted, "{error}");
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("drop must kill and reap its child without another wait call");
}
