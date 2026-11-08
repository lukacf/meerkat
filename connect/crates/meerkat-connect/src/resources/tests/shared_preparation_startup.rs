//! Executable-layout verification and public shared startup custody.
//!
//! These controls retain the public future and signal its cancellation input.
//! They do not qualify dropping that future or any native permission/P behavior.
use super::*;
use futures_util::FutureExt;
use std::{any::Any, panic::AssertUnwindSafe, time::Duration};
use tokio::{sync::oneshot, task::JoinHandle};

const BOUND: Duration = Duration::from_secs(30);
const SCRIPT: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../sdk/consumer-setup/tests/shared_preparation_startup.py"
));

struct Case {
    fixture: Fixture,
    output: PathBuf,
    cancel: Option<oneshot::Sender<()>>,
    preparation: Option<JoinHandle<Result<PreparedSharedSetup, ConnectError>>>,
    prepared: Option<PreparedSharedSetup>,
    preparation_error: Option<ConnectError>,
    close: Option<JoinHandle<Result<(), ConnectError>>>,
    close_result: Option<Result<(), ConnectError>>,
    preparer_only: bool,
}

impl Case {
    fn new(mode: &str) -> Self {
        let python = PathBuf::from(
            std::env::var_os("TOOLKIT_SHARED_TEST_PYTHON")
                .expect("explicit selected Python with matching MCP dependency closure"),
        );
        assert!(python.is_absolute() && python.is_file());
        let fixture = Fixture::new();
        let output = fixture.directory.path().join("startup-observations");
        fs::create_dir(&output).unwrap();
        fs::set_permissions(&output, fs::Permissions::from_mode(0o700)).unwrap();
        let output = fs::canonicalize(output).unwrap();
        let source = fs::canonicalize(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sdk/consumer-setup"),
        )
        .unwrap();
        for name in ["prepare.py", "local_session.py"] {
            fs::write(fixture.root.join(name), SCRIPT).unwrap();
            fs::set_permissions(fixture.root.join(name), fs::Permissions::from_mode(0o755))
                .unwrap();
        }
        // The exact fixture wrapper is pinned in the public resource manifest.
        // Its interpreter is an explicit qualification input, never discovered
        // from PATH or inherited credentials. Preserve venv path semantics.
        let quoted = python.to_str().unwrap().replace('\'', "'\\''");
        fs::write(
            fixture.root.join("runtime/python/bin/python3.11"),
            format!("#!/bin/sh\nexec '{quoted}' \"$@\"\n"),
        )
        .unwrap();
        fs::write(
            fixture.root.join("fixture.json"),
            serde_json::to_vec(&json!({"python": python, "source": source,
                "output": output, "mode": mode}))
            .unwrap(),
        )
        .unwrap();
        fs::set_permissions(
            fixture.root.join("fixture.json"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        let pin = fixture.pin(fixture.manifest());
        let executable = fixture.directory.path().join("bin/meerkat-connect");
        fs::create_dir(executable.parent().unwrap()).unwrap();
        fs::write(&executable, "public startup fixture executable layout").unwrap();
        let resources = InstalledResources::from_executable(&executable, &pin).unwrap();
        let storage = meerkat_core::StorageLayout::resolve_with_global_candidate(
            meerkat_core::StorageLayoutInputs {
                invocation_context: fixture.directory.path().to_owned(),
                explicit_state_root: Some(fixture.directory.path().join("selected-state")),
                ..Default::default()
            },
            &meerkat_core::RealmConfig {
                selection: meerkat_core::RealmSelection::Explicit {
                    realm_id: "startup-test".into(),
                },
                ..Default::default()
            },
            &fixture.directory.path().join("unused-global"),
        )
        .unwrap();
        let (send, receive) = oneshot::channel();
        let preparation = tokio::spawn(async move {
            resources
                .prepare_with_shared_setup(&storage, 2, async move {
                    receive.await.map_err(std::io::Error::other)
                })
                .await
        });
        Self {
            fixture,
            output,
            cancel: Some(send),
            preparation: Some(preparation),
            prepared: None,
            preparation_error: None,
            close: None,
            close_result: None,
            preparer_only: mode == "preparer-cancel",
        }
    }

    fn cancel(&mut self) {
        if let Some(send) = self.cancel.take() {
            // Once Ready returned, the cancellation receiver may be gone.
            // This does not revoke the returned owner's cleanup obligation.
            let _ = send.send(());
        }
    }

    fn release(&self) -> std::io::Result<()> {
        fs::write(
            self.output.join("RELEASE-READY"),
            "release retained Ready callback\n",
        )
    }

    fn read(&self, name: &str) -> Value {
        let path = self.output.join(name);
        let raw = fs::read(&path)
            .unwrap_or_else(|error| panic!("read startup observation {}: {error}", path.display()));
        serde_json::from_slice(&raw).unwrap_or_else(|error| {
            panic!("decode startup observation {}: {error}", path.display())
        })
    }

    async fn wait(&self, name: &str) {
        tokio::time::timeout(BOUND, async {
            while !self.output.join(name).exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("actual startup milestone absent: {name}"));
    }

    async fn prepared(&mut self) {
        let result = tokio::time::timeout(BOUND, self.preparation.as_mut().unwrap())
            .await
            .expect("retained public preparation completes");
        self.preparation.take();
        // Retain any returned resource before evaluating its result.
        match result.expect("public preparation task joined") {
            Ok(value) => self.prepared = Some(value),
            Err(error) => self.preparation_error = Some(error),
        }
    }

    fn start_close(&mut self) {
        if let Some(prepared) = self.prepared.take() {
            let closing = prepared.owner.close();
            self.close = Some(tokio::spawn(closing));
        }
    }

    async fn closed(&mut self) {
        let result = tokio::time::timeout(BOUND, self.close.as_mut().unwrap())
            .await
            .expect("same retained caller close completes");
        self.close.take();
        self.close_result = Some(result.expect("caller close task joined"));
    }

    fn assert_joined(&self) {
        assert!(self.preparation.is_none() && self.close.is_none() && self.prepared.is_none());
        let preparer = self.read("PREPARER-STARTED.json");
        assert_eq!(self.read("PREPARER-RETURNING.json")["pid"], preparer["pid"]);
        let controller = self.read("CONTROLLER-STARTED.json");
        assert_ne!(controller["pid"], preparer["pid"]);
        let launches: Vec<Value> = fs::read_to_string(self.output.join("LAUNCHES.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            launches,
            vec![
                json!({"role": "preparer", "pid": preparer["pid"]}),
                json!({"role": "controller", "pid": controller["pid"]})
            ]
        );
        let joined = self.read("OWNER-JOINED.json");
        assert_eq!(joined["pid"], controller["pid"]);
        assert_eq!(joined["workers_alive"], 0);
        assert_eq!(joined["closing"], true);
        assert_eq!(joined["sockets_removed"], true);
        assert_eq!(joined["paths"], self.read("READY-BOUNDARY.json")["paths"]);
        let paths = joined["paths"].as_array().unwrap();
        assert_eq!(paths.len(), 2);
        assert_ne!(paths[0], paths[1]);
        for path in paths {
            assert!(!Path::new(path.as_str().unwrap()).exists());
        }
        for value in [preparer, controller] {
            let pid = i32::try_from(value["pid"].as_i64().unwrap()).unwrap();
            assert_eq!(
                nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
                Err(nix::errno::Errno::ESRCH),
                "the actual owned startup child must have exited and been reaped"
            );
        }
        // Marker writes are exclusive, so a second preparer/Controller launch
        // fails instead of replacing evidence. The public API has no retry.
        assert!(self.fixture.directory.path().exists());
    }

    fn assert_preparer_only_joined(&self) {
        assert!(self.preparation.is_none() && self.close.is_none() && self.prepared.is_none());
        assert!(self.close_result.is_none());
        let started = self.read("PREPARER-STARTED.json");
        let cancelled = self.read("PREPARER-CANCELLED.json");
        assert_eq!(cancelled["pid"], started["pid"]);
        assert_eq!(
            cancelled["signal"],
            nix::sys::signal::Signal::SIGTERM as i32
        );
        assert_eq!(self.read("PREPARER-RETURNING.json")["pid"], started["pid"]);
        let launches: Vec<Value> = fs::read_to_string(self.output.join("LAUNCHES.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(
            launches,
            vec![json!({"role": "preparer", "pid": started["pid"]})]
        );
        let pid = i32::try_from(started["pid"].as_i64().unwrap()).unwrap();
        assert_eq!(
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
            Err(nix::errno::Errno::ESRCH)
        );
        for name in [
            "CONTROLLER-STARTED.json",
            "READY-BOUNDARY.json",
            "READY-EMITTED.json",
            "OWNER-INPUT-CLOSED.json",
            "OWNER-JOINED.json",
        ] {
            assert!(
                !self.output.join(name).exists(),
                "unexpected later startup: {name}"
            );
        }
    }

    async fn finish(mut self, outcome: Result<(), Box<dyn Any + Send>>) {
        self.cancel();
        let mut failures = Vec::new();
        if let Err(error) = self.release() {
            failures.push(format!("release startup hold: {error}"));
        }
        // Borrowed waits never discard the public future or a close receiver.
        // After releasing fixture holds, join these same tasks even if the
        // earlier bounded assertion failed, before dropping the resource tree.
        if let Some(task) = self.preparation.take() {
            match task.await {
                Ok(Ok(prepared)) => self.prepared = Some(prepared),
                Ok(Err(error)) => self.preparation_error = Some(error),
                Err(error) => failures.push(format!("preparation join: {error}")),
            }
        }
        self.start_close();
        if let Some(task) = self.close.take() {
            match task.await {
                Ok(result) => self.close_result = Some(result),
                Err(error) => failures.push(format!("physical close join: {error}")),
            }
        }
        if let Some(Err(error)) = &self.close_result {
            failures.push(format!("physical close result: {error}"));
        }
        if let Some(ConnectError::Cleanup { .. }) = &self.preparation_error {
            failures.push("internal preparation cleanup refused".into());
        }
        let cleanup_panic = AssertUnwindSafe(async {
            if self.preparer_only {
                self.assert_preparer_only_joined();
            } else {
                self.assert_joined();
            }
        })
        .catch_unwind()
        .await
        .err();
        if cleanup_panic.is_some() {
            failures.push("actual child/socket join assertions failed during cleanup".into());
        }
        if outcome.is_err() || !failures.is_empty() {
            // Preserve child stderr and the real observation files after the
            // same preparation/close task waits. A missing marker is
            // failed evidence, never permission to erase the underlying error.
            let retained = self.fixture.directory.keep();
            use std::io::Write;
            let _ = writeln!(
                std::io::stderr(),
                "shared_startup_failure_evidence={}",
                retained.display()
            );
            let _ = writeln!(std::io::stderr(), "startup failure cleanup: {failures:?}");
        }
        match outcome {
            Err(panic) => {
                std::panic::resume_unwind(panic);
            }
            Ok(()) => {
                if let Some(panic) = cleanup_panic {
                    std::panic::resume_unwind(panic);
                }
                assert!(failures.is_empty(), "startup cleanup: {failures:?}");
            }
        }
    }
}

#[tokio::test]
async fn cancellation_while_preparer_runs_joins_before_any_controller_launch() {
    let mut case = Case::new("preparer-cancel");
    let outcome = AssertUnwindSafe(async {
        case.wait("PREPARER-STARTED.json").await;
        assert!(!case.output.join("PREPARER-RETURNING.json").exists());
        case.cancel();
        case.wait("PREPARER-CANCELLED.json").await;
        assert!(!case.preparation.as_ref().unwrap().is_finished());
        assert!(!case.output.join("PREPARER-RETURNING.json").exists());
        assert!(!case.output.join("CONTROLLER-STARTED.json").exists());
        case.release().unwrap();
        case.prepared().await;
        assert!(case.prepared.is_none());
        assert!(matches!(
            case.preparation_error,
            Some(ConnectError::InvalidInput(_))
        ));
        case.assert_preparer_only_joined();
    })
    .catch_unwind()
    .await;
    case.finish(outcome).await;
}

#[tokio::test]
async fn cancellation_before_ready_joins_the_actual_public_startup_owner() {
    let mut case = Case::new("cancel");
    let outcome = AssertUnwindSafe(async {
        case.wait("READY-BOUNDARY.json").await;
        assert!(!case.output.join("READY-EMITTED.json").exists());
        case.cancel();
        case.wait("OWNER-INPUT-CLOSED.json").await;
        assert!(!case.preparation.as_ref().unwrap().is_finished());
        assert!(!case.output.join("OWNER-JOINED.json").exists());
        case.release().unwrap();
        case.prepared().await;
        assert!(case.prepared.is_none());
        assert!(matches!(
            case.preparation_error,
            Some(ConnectError::InvalidInput(_))
        ));
        assert!(!case.output.join("READY-EMITTED.json").exists());
        case.assert_joined();
    })
    .catch_unwind()
    .await;
    case.finish(outcome).await;
}

#[tokio::test]
async fn malformed_ready_refuses_and_joins_without_returning_a_caller_owner() {
    let mut case = Case::new("malformed");
    let outcome = AssertUnwindSafe(async {
        case.wait("READY-EMITTED.json").await;
        assert_eq!(case.read("READY-EMITTED.json")["valid"], false);
        case.wait("OWNER-INPUT-CLOSED.json").await;
        assert!(!case.preparation.as_ref().unwrap().is_finished());
        assert!(!case.output.join("OWNER-JOINED.json").exists());
        case.release().unwrap();
        case.prepared().await;
        assert!(case.prepared.is_none());
        assert!(matches!(
            case.preparation_error,
            Some(ConnectError::InvalidInput(_))
        ));
        case.assert_joined();
    })
    .catch_unwind()
    .await;
    case.finish(outcome).await;
}

#[tokio::test]
async fn ready_return_transfers_cleanup_to_the_caller_despite_later_cancellation() {
    let mut case = Case::new("ready");
    let outcome = AssertUnwindSafe(async {
        case.wait("READY-EMITTED.json").await;
        case.prepared().await;
        let prepared = case
            .prepared
            .as_ref()
            .expect("actual Ready returns an owner");
        assert_eq!(prepared.connections.len(), 2);
        assert!(prepared.artifact_directory.join("prepare.stdout").is_file());
        assert!(case.preparation_error.is_none());
        assert!(!case.output.join("OWNER-INPUT-CLOSED.json").exists());
        case.cancel();
        case.start_close();
        case.wait("OWNER-INPUT-CLOSED.json").await;
        assert!(!case.close.as_ref().unwrap().is_finished());
        assert!(!case.output.join("OWNER-JOINED.json").exists());
        case.release().unwrap();
        case.closed().await;
        assert!(matches!(case.close_result, Some(Ok(()))));
        case.assert_joined();
    })
    .catch_unwind()
    .await;
    case.finish(outcome).await;
}
