//! Real OS process/lease and Controller census controls. These do not construct
//! a native receipt, restore a grant, or prove final resource adoption.
use super::*;
use futures_util::FutureExt;
use std::{os::unix::fs::PermissionsExt, panic::AssertUnwindSafe};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
const BOUND: std::time::Duration = std::time::Duration::from_secs(20);

struct Case {
    _temp: tempfile::TempDir,
    root: PathBuf,
    path: PathBuf,
    sha: String,
    parent: Option<Arc<PhysicalLease>>,
    selection: PhysicalSelection,
    child: tokio::process::Child,
    input: Option<ChildStdin>,
    output: Option<BufReader<ChildStdout>>,
    bridge: Option<tokio::process::Child>,
}
impl Case {
    fn new(mode: &str) -> Self {
        Self::with_peers(mode, 1)
    }
    fn with_peers(mode: &str, peers: usize) -> Self {
        let temp = tempfile::tempdir().unwrap();
        // The fence admits only a private (0700) artifact directory, and the
        // installed resource manifest may be public but never group or other
        // writable. Set both modes here instead of inheriting them from the
        // process umask, which concurrent tests share and must not change.
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let root = temp.path().canonicalize().unwrap();
        let resources = root.join("resources");
        fs::create_dir(&resources).unwrap();
        fs::write(
            resources.join("resources.json"),
            b"selected fixture resources",
        )
        .unwrap();
        fs::set_permissions(
            resources.join("resources.json"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        lifetime::write_new(&root.join("prepare.stdout"), b"selected fixture descriptor").unwrap();
        let realm = meerkat_core::RealmLocator {
            state_root: root.clone(),
            realm: meerkat_core::mob_realm_id("physical-fence-control").unwrap(),
        };
        let (path, sha) = lifetime::PhysicalLifetime::retain(
            &root,
            &resources,
            &realm,
            &"a".repeat(64),
            "/usr/bin/env",
            &["selected-fixture".into()],
            peers,
            None,
        )
        .unwrap();
        let parent = PhysicalLease::join(&path, &sha).unwrap();
        let selection = parent.selection();
        let python = PathBuf::from(
            std::env::var_os("TOOLKIT_SHARED_TEST_PYTHON")
                .expect("select the same exact Python used by existing shared native controls"),
        );
        assert!(python.is_absolute() && python.is_file());
        let helper = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../sdk/consumer-setup/tests/physical_fence_process.py")
            .canonicalize()
            .unwrap();
        let mut child = tokio::process::Command::new(python)
            .args(["-I", "-B"])
            .arg(helper)
            .arg(&path)
            .arg(&sha)
            .arg(mode)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let output = BufReader::new(child.stdout.take().unwrap());
        Self {
            _temp: temp,
            root,
            path,
            sha,
            parent: Some(parent),
            selection,
            child,
            input,
            output: Some(output),
            bridge: None,
        }
    }
    async fn read(&mut self) -> Value {
        let mut raw = String::new();
        tokio::time::timeout(BOUND, self.output.as_mut().unwrap().read_line(&mut raw))
            .await
            .unwrap()
            .unwrap();
        assert!(raw.len() < 1024 * 1024);
        serde_json::from_str(&raw).unwrap()
    }
    async fn send(&mut self, value: Value) {
        let raw = serde_json::to_vec(&value).unwrap();
        tokio::time::timeout(BOUND, async {
            let input = self.input.as_mut().unwrap();
            input.write_all(&raw).await.unwrap();
            input.write_all(b"\n").await.unwrap();
            input.flush().await.unwrap();
        })
        .await
        .unwrap();
    }
    async fn command(&mut self, value: Value) -> Value {
        self.send(value).await;
        self.read().await
    }
    async fn crash(&mut self) {
        self.send(json!({"op":"crash"})).await;
        assert_eq!(
            tokio::time::timeout(BOUND, self.child.wait())
                .await
                .unwrap()
                .unwrap()
                .code(),
            Some(77)
        );
        self.parent.take();
    }
    async fn operation(&mut self, entered: bool) -> String {
        let parent = self.parent.as_ref().unwrap();
        let member = meerkat_core::MobMemberBinding {
            mob_id: "physical-fence-control".into(),
            role: "helper".into(),
            member: "helper:test".into(),
        };
        let session = uuid::Uuid::new_v4().to_string();
        parent.member(0, &member, &session).unwrap();
        let mut request = json!({"operation":"prepare_entry","peer":0,"connection":"1",
            "tool":"setup_state","arguments":{},"preparation":null,
            "native_request":{"session_id":session,"run_id":uuid::Uuid::new_v4().to_string(),
                "call_id":"physical-control","retained_work":null}});
        request["physical_intent"] = parent.intent(0, &request).unwrap();
        let token = self.command(request).await["entry"]
            .as_str()
            .unwrap()
            .to_owned();
        if entered {
            assert_eq!(
                self.command(json!({"op":"enter","entry":token})).await,
                json!({"entered":true})
            );
        }
        let closed = self
            .command(json!({"operation":"close_entry","peer":0,"entry":token}))
            .await;
        self.parent
            .as_ref()
            .unwrap()
            .observed(&token, closed["decision"].as_str().unwrap())
            .unwrap();
        self.command(json!({"operation":"finish_entry","peer":0,"entry":token}))
            .await;
        token
    }
    async fn cleanup(&mut self) {
        // Complete both real waits before asserting. Forced or unavailable
        // cleanup is a test failure, never a successful physical observation.
        let mut failures = Vec::new();
        if fs::write(self.root.join("release-descendant"), b"release").is_err() {
            failures.push("descendant release write failed");
        }
        self.input.take();
        if let Some(mut bridge) = self.bridge.take() {
            bridge.stdin.take();
            match tokio::time::timeout(BOUND, bridge.wait()).await {
                Ok(Ok(status)) if status.success() => {}
                Ok(Ok(_)) => failures.push("bridge exited unsuccessfully"),
                _ => {
                    failures.push("bridge wait failed or timed out");
                    let _ = bridge.start_kill();
                    if bridge.wait().await.is_err() {
                        failures.push("bridge reap failed");
                    }
                }
            }
        }
        match tokio::time::timeout(BOUND, self.child.wait()).await {
            Ok(Ok(status)) if status.success() || status.code() == Some(77) => {}
            Ok(Ok(_)) => failures.push("owner fixture exited unsuccessfully"),
            _ => {
                failures.push("owner fixture wait failed or timed out");
                let _ = self.child.start_kill();
                if self.child.wait().await.is_err() {
                    failures.push("owner fixture reap failed");
                }
            }
        }
        self.parent.take();
        if self.root.join("descendant-live").exists() {
            let joined = fs::read(self.root.join("descendant-joined.json"))
                .ok()
                .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok());
            if joined != Some(json!({"returncode":0,"scope":"actual_test_supervisor_waitpid"})) {
                failures.push("descendant actual waitpid evidence unavailable");
            }
        }
        assert!(
            failures.is_empty(),
            "fixture cleanup failures: {failures:?}"
        );
    }
}

#[tokio::test]
async fn cold_nonentry_requires_real_controller_census_and_observed_decision() {
    let mut case = Case::new("owner");
    let body = AssertUnwindSafe(async {
        assert_eq!(case.read().await["ready"], true);
        assert!(
            case.selection.seal_no_entry().is_err(),
            "live Controller/parent custody is not exclusive"
        );
        case.operation(false).await;
        case.crash().await;
        let proof = case.selection.seal_no_entry().unwrap();
        assert_eq!(proof.census()["operations"].as_array().unwrap().len(), 1);
        assert!(
            case.selection.seal_no_entry().is_err(),
            "proof retains exclusive custody"
        );
        proof.recheck().unwrap();
        let owner = case
            .root
            .join(format!("shared-physical-{}", case.selection.owner));
        let startup = owner.join("startup.json");
        let original = fs::read(&startup).unwrap();
        fs::write(&startup, b"changed after exclusive capture").unwrap();
        assert!(
            proof.recheck().is_err(),
            "held lease cannot bless changed census bytes"
        );
        fs::write(&startup, &original).unwrap();
        proof.recheck().unwrap();
        assert!(
            case.selection.seal_no_entry().is_err(),
            "recheck did not release or reacquire custody"
        );
        drop(proof);
        let again = case.selection.seal_no_entry().unwrap();
        assert_eq!(again.selection(), &case.selection);
        drop(again);
        assert!(
            PhysicalLease::join(&case.path, &case.sha).is_err(),
            "late old owner cannot reopen"
        );
    })
    .catch_unwind()
    .await;
    case.cleanup().await;
    if let Err(error) = body {
        std::panic::resume_unwind(error);
    }
}

#[tokio::test]
async fn cold_seal_refuses_late_child_and_actual_native_bridge_holds_lease() {
    let mut case = Case::new("bridge");
    let body = AssertUnwindSafe(async {
        let ready = case.read().await;
        let python = PathBuf::from(std::env::var_os("TOOLKIT_SHARED_TEST_PYTHON").unwrap());
        let helper = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../sdk/consumer-setup/local_session.py");
        let bridge = tokio::process::Command::new(&python)
            .args(["-I", "-B"])
            .arg(&helper)
            .arg("--shared-peer")
            .arg(ready["paths"][0].as_str().unwrap())
            .arg("--shared-lifetime")
            .arg(&case.path)
            .arg("--shared-lifetime-sha256")
            .arg(&case.sha)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        case.bridge = Some(bridge);
        assert_eq!(case.read().await, json!({"connected":true}));
        case.parent.take();
        let (_, _, lease) = case.selection.open().unwrap();
        assert!(
            lease.try_lock().is_err(),
            "actual bridge independently holds shared custody"
        );
        case.bridge.as_mut().unwrap().stdin.take();
        assert_eq!(
            case.command(json!({"op":"disconnect"})).await,
            json!({"disconnected":true})
        );
        assert!(
            tokio::time::timeout(BOUND, case.bridge.as_mut().unwrap().wait())
                .await
                .unwrap()
                .unwrap()
                .success()
        );
        // The exact selected socket remains alive, but no lease remains. A
        // missing-startup refusal still seals; late refusal cannot be explained
        // by a nonexistent endpoint or mismatched socket identity.
        assert!(case.selection.seal_no_entry().is_err());
        let late = tokio::process::Command::new(python)
            .args(["-I", "-B"])
            .arg(helper)
            .arg("--shared-peer")
            .arg(ready["paths"][0].as_str().unwrap())
            .arg("--shared-lifetime")
            .arg(&case.path)
            .arg("--shared-lifetime-sha256")
            .arg(&case.sha)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        case.bridge = Some(late);
        let status = tokio::time::timeout(BOUND, case.bridge.as_mut().unwrap().wait())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(status.code(), Some(2));
        // The expected refused child has been actually waited; remove it so
        // cleanup does not classify this intentional refusal as a bridge crash.
        case.bridge.take();
        assert_eq!(
            case.command(json!({"op":"probe-late"})).await,
            json!({"connection_pending":false})
        );
        assert!(
            tokio::time::timeout(BOUND, case.child.wait())
                .await
                .unwrap()
                .unwrap()
                .success()
        );
        assert!(PhysicalLease::join(&case.path, &case.sha).is_err());
    })
    .catch_unwind()
    .await;
    case.cleanup().await;
    if let Err(error) = body {
        std::panic::resume_unwind(error);
    }
}

#[tokio::test]
async fn cold_census_refuses_missing_orphan_changed_and_initialization_gaps() {
    for mode in [
        "missing",
        "orphan",
        "changed",
        "initialization-gap",
        "unsupported",
        "changed-initial-roster",
    ] {
        let mut case = Case::with_peers(
            if matches!(mode, "initialization-gap" | "unsupported") {
                mode
            } else {
                "owner"
            },
            if mode == "changed-initial-roster" {
                2
            } else {
                1
            },
        );
        let body = AssertUnwindSafe(async {
            assert_eq!(case.read().await["ready"], true);
            if mode == "initialization-gap" {
                assert_eq!(
                    tokio::time::timeout(BOUND, case.child.wait())
                        .await
                        .unwrap()
                        .unwrap()
                        .code(),
                    Some(77)
                );
                case.parent.take();
            } else {
                let token = case.operation(false).await;
                case.crash().await;
                let (_, dir, _) = case.selection.open().unwrap();
                match mode {
                    "missing" => {
                        fs::remove_file(dir.path.join(format!("observed-{token}.json"))).unwrap();
                    }
                    "orphan" => {
                        dir.write("operation-orphan.json", b"{}").unwrap();
                    }
                    "changed" => {
                        fs::write(
                            dir.path.join(format!("census-{token}.decision.json")),
                            b"{}",
                        )
                        .unwrap();
                    }
                    "changed-initial-roster" => {
                        let mut ready: Value =
                            serde_json::from_slice(&dir.read("startup.json").unwrap()).unwrap();
                        assert_eq!(ready["peers"], 2);
                        assert!(dir.exists("peer-1.json").unwrap());
                        // The real two-peer Controller census is valid before
                        // corruption; retain only peer zero's actual operation.
                        drop(case.selection.seal_no_entry().unwrap());
                        fs::remove_file(dir.path.join("peer-1.json")).unwrap();
                        ready["peers"] = json!(1);
                        fs::write(
                            dir.path.join("startup.json"),
                            serde_json::to_vec(&ready).unwrap(),
                        )
                        .unwrap();
                    }
                    _ => {}
                }
            }
            assert!(
                case.selection.seal_no_entry().is_err(),
                "{mode} cannot become nonentry proof"
            );
        })
        .catch_unwind()
        .await;
        case.cleanup().await;
        if let Err(error) = body {
            std::panic::resume_unwind(error);
        }
    }
}

#[tokio::test]
async fn cold_fence_refuses_directory_and_lease_replacement() {
    for replace_directory in [false, true] {
        let mut case = Case::new("owner");
        let body = AssertUnwindSafe(async {
            assert_eq!(case.read().await["ready"], true);
            case.crash().await;
            let directory = case
                .root
                .join(format!("shared-physical-{}", "a".repeat(64)));
            if replace_directory {
                fs::rename(&directory, case.root.join("retained-old-directory")).unwrap();
                fs::create_dir(&directory).unwrap();
                fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
            } else {
                fs::rename(
                    directory.join("lease"),
                    directory.join("retained-old-lease"),
                )
                .unwrap();
            }
            lifetime::write_new(&directory.join("lease"), b"").unwrap();
            assert!(case.selection.seal_no_entry().is_err());
            assert!(PhysicalLease::join(&case.path, &case.sha).is_err());
        })
        .catch_unwind()
        .await;
        case.cleanup().await;
        if let Err(error) = body {
            std::panic::resume_unwind(error);
        }
    }
}

#[tokio::test]
async fn cold_entered_refuses_even_with_free_lease_and_live_detached_descendant() {
    let mut case = Case::new("owner");
    let body = AssertUnwindSafe(async {
        assert_eq!(case.read().await["ready"], true);
        case.operation(true).await;
        assert_eq!(
            case.command(json!({"op":"detached-after-entry"})).await,
            json!({"detached":true,"tracked_joined":true})
        );
        // This supervisor deliberately remains alive solely to waitpid the
        // descendant after inspection. Separate controls prove abrupt loss.
        assert!(case.child.try_wait().unwrap().is_none());
        case.parent.take();
        let (_, _, lease) = case.selection.open().unwrap();
        lease.try_lock().unwrap();
        drop(lease);
        assert!(case.root.join("descendant-live").exists());
        assert!(!case.root.join("descendant-done").exists());
        assert!(
            case.selection.seal_no_entry().is_err(),
            "exclusive custody is not effect evidence"
        );
    })
    .catch_unwind()
    .await;
    case.cleanup().await;
    if let Err(error) = body {
        std::panic::resume_unwind(error);
    }
}

#[tokio::test]
async fn cold_census_rechecks_same_snapshot_after_interleaved_mutation() {
    for original in [false, true] {
        let mut case = Case::new("owner");
        let body = AssertUnwindSafe(async {
            assert_eq!(case.read().await["ready"], true);
            let token = case.operation(false).await;
            case.crash().await;
            let (_, dir, lease) = case.selection.open().unwrap();
            lease.try_lock().unwrap();
            dir.exact(
                "sealed.json",
                &json!({"schema_version":1,"scope":"closed_physical_admission",
                "lifetime_sha256":case.sha}),
            )
            .unwrap();
            let changed = if original {
                case.root
                    .join(format!("shared-entries-{}", "a".repeat(64)))
                    .join(format!("{token}.decision.json"))
            } else {
                dir.path.join(format!("census-{token}.decision.json"))
            };
            assert!(
                case.selection
                    .no_entry(&dir, || {
                        fs::write(&changed, b"{}").unwrap();
                    })
                    .is_err(),
                "pins cannot come from bytes different from the validated snapshot"
            );
        })
        .catch_unwind()
        .await;
        case.cleanup().await;
        if let Err(error) = body {
            std::panic::resume_unwind(error);
        }
    }
}

#[tokio::test]
async fn ordinary_shared_entry_without_console_member_retains_unresolved_provenance() {
    let mut case = Case::new("framed");
    let body = AssertUnwindSafe(async {
        let ready = case.read().await;
        assert_eq!(ready["shared_setup"], 1);
        let python = PathBuf::from(std::env::var_os("TOOLKIT_SHARED_TEST_PYTHON").unwrap());
        let helper = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../sdk/consumer-setup/local_session.py");
        let configuration = meerkat_core::McpServerConfig::stdio(
            "toolkit-setup",
            python.to_string_lossy().into_owned(),
            vec![
                "-I".into(),
                "-B".into(),
                helper.to_string_lossy().into_owned(),
                "--shared-peer".into(),
                ready["peers"][0].as_str().unwrap().into(),
                "--shared-lifetime".into(),
                case.path.to_string_lossy().into_owned(),
                "--shared-lifetime-sha256".into(),
                case.sha.clone(),
            ],
            Default::default(),
        );
        let channel = Arc::new(tokio::sync::Mutex::new(Channel::new(
            case.input.take().unwrap(),
            case.output.take().unwrap(),
        )));
        let entries = Arc::new(entry::EntryTasks::default());
        let peer = SharedSetupPeer::new(channel.clone(), 0, configuration, entries.clone())
            .with_physical(case.parent.as_ref().unwrap().clone());
        let session = uuid::Uuid::new_v4().to_string();
        // This is the actual private call-context send/close/finish door used
        // by SharedSetupCallContext::prepare, with the actual Python owner loop
        // and Controller. No Console registration or member is fabricated.
        let native = json!({"session_id":session,"run_id":uuid::Uuid::new_v4().to_string(),
            "call_id":"ordinary-shared","retained_work":null});
        let token = peer
            .prepare_entry("1", "setup_state", &json!({}), None, &native)
            .await
            .unwrap();
        peer.close_entry(&token).await.unwrap();
        peer.finish_entry(&token).await.unwrap();
        entries.close().await.unwrap();
        let directory = &case.parent.as_ref().unwrap().directory;
        assert_eq!(
            directory.value("unresolved-member-0.json").unwrap()["scope"],
            "member_provenance_unavailable"
        );
        // A subsequent actual registration writer still cannot upgrade the
        // preceding unbound operation into complete historical provenance.
        case.parent
            .as_ref()
            .unwrap()
            .member(
                0,
                &meerkat_core::MobMemberBinding {
                    mob_id: "physical-fence-control".into(),
                    role: "helper".into(),
                    member: "helper:test".into(),
                },
                &session,
            )
            .unwrap();
        channel.lock().await.input.take();
        assert!(
            tokio::time::timeout(BOUND, case.child.wait())
                .await
                .unwrap()
                .unwrap()
                .success()
        );
        drop(peer);
        drop(channel);
        drop(entries);
        case.parent.take();
        assert!(
            case.selection.seal_no_entry().is_err(),
            "ordinary dispatch is not a cold member receipt"
        );
    })
    .catch_unwind()
    .await;
    case.cleanup().await;
    if let Err(error) = body {
        std::panic::resume_unwind(error);
    }
}
