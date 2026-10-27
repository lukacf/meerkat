//! Process/protocol custody controls. The selected Python executes only this
//! synthetic fixed-protocol child, not the production preparer or native MCP.
use super::*;
#[cfg(feature = "console")]
use serde_json::{Value, json};
use std::time::Duration;
#[cfg(feature = "console")]
use std::{os::unix::fs::PermissionsExt, pin::Pin};
#[cfg(feature = "console")]
use tokio::{sync::oneshot, task::JoinHandle};

#[cfg(feature = "console")]
const BOUND: Duration = Duration::from_secs(10);
#[cfg(feature = "console")]
const SCRIPT: &str = r#"import json, os, pathlib, select, sys, time
config = json.loads((pathlib.Path(__file__).parent / 'fixture.json').read_text())
output = pathlib.Path(config['output'])
pid = os.getpid()
def record(suffix, value):
    target = output / (str(pid) + '.' + suffix)
    temporary = target.with_suffix(target.suffix + '.tmp')
    temporary.write_text(json.dumps(value))
    os.replace(temporary, target)
record('started', {'pid': pid, 'argv': sys.argv})
try:
    if config['mode'] == 'hold':
        while not (output / 'release').exists():
            readable, _, _ = select.select([sys.stdin], [], [], 0.01)
            if readable:
                assert sys.stdin.buffer.read(1) == b''
                sys.exit(0)
    ready = b'{"ready":false}\n' if config['mode'] == 'bad_ready' else b'{"ready":true}\n'
    sys.stdout.buffer.write(ready); sys.stdout.buffer.flush()
    frame = sys.stdin.buffer.readline()
    if frame:
        assert frame == b'{"settle":true}\n'
        response = b'{"consumed":true}\n{"settled":true}\n'
        if config['mode'] == 'bad_ack':
            response = b'{"consumed":false}\n{"settled":true}\n'
        if config['mode'] == 'oversized_ack':
            response = b'x' * 4097
        sys.stdout.buffer.write(response); sys.stdout.buffer.flush()
finally:
    record('ended', {'pid': pid})
"#;

#[cfg(feature = "console")]
type Begin = Result<OwnedLocalRoundGuard, (LocalRoundOwner, ConnectError)>;
#[cfg(feature = "console")]
type Finish = (LocalRoundOwner, Result<(), ConnectError>);

#[cfg(feature = "console")]
struct Case {
    _directory: tempfile::TempDir,
    output: PathBuf,
    selected_root: PathBuf,
    owner: Option<LocalRoundOwner>,
    guard: Option<OwnedLocalRoundGuard>,
    begin: Option<JoinHandle<Begin>>,
    finish: Option<JoinHandle<Finish>>,
    cancel: Option<oneshot::Sender<()>>,
    last_error: bool,
}

#[cfg(feature = "console")]
impl Case {
    fn new(mode_name: &str) -> Self {
        let python = PathBuf::from(
            std::env::var_os("TOOLKIT_SHARED_TEST_PYTHON")
                .expect("explicit selected Python; these controls use its standard library only"),
        );
        assert!(python.is_absolute() && python.is_file());
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("libexec/meerkat-connect");
        let output = directory.path().join("observations");
        let artifacts = directory.path().join("artifacts");
        for path in [&root, &output, &artifacts] {
            fs::create_dir_all(path).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let quoted = python.to_str().unwrap().replace('\'', "'\\''");
        for (name, bytes) in [
            ("prepare.py", SCRIPT.to_owned()),
            (
                "runtime/python/bin/python3.11",
                format!("#!/bin/sh\nexec '{quoted}' \"$@\"\n"),
            ),
            ("runtime/node/bin/node", "inert node".into()),
            (
                "runtime/node/lib/node_modules/npm/bin/npm-cli.js",
                "inert npm".into(),
            ),
            ("starter/start.py", "inert starter".into()),
            ("archives/library.tar.gz", "inert archive".into()),
            (
                "fixture.json",
                serde_json::to_string(&json!({"mode":mode_name,"output":output})).unwrap(),
            ),
        ] {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, bytes).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        // Normalize intermediate directories before binding the exact inventory.
        // create_dir_all inherits the process umask; the resource verifier refuses
        // group- or world-writable entries.
        for path in inventory(&root).unwrap() {
            let path = root.join(path);
            if path.is_dir() {
                fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
            }
        }
        let entries: Vec<Value> = inventory(&root)
            .unwrap()
            .into_iter()
            .map(|path| {
                let full = root.join(&path);
                let metadata = fs::symlink_metadata(&full).unwrap();
                if metadata.is_dir() {
                    json!({"path":path,"kind":"directory","mode":mode(&metadata)})
                } else {
                    json!({"path":path,"kind":"file","mode":mode(&metadata),
                    "sha256":hash_file(&full,metadata.len()).unwrap(),"size":metadata.len()})
                }
            })
            .collect();
        let raw = serde_json::to_vec(&json!({"schema_version":1,"kind":"meerkat_connect_resources",
            "target":platform_target().unwrap(),"prepare":"prepare.py",
            "python":"runtime/python/bin/python3.11","node":"runtime/node/bin/node",
            "npm_cli":"runtime/node/lib/node_modules/npm/bin/npm-cli.js","starter":"starter/start.py",
            "archive":"archives/library.tar.gz","entries":entries,"sources":[]})).unwrap();
        fs::write(root.join("resources.json"), &raw).unwrap();
        fs::set_permissions(
            root.join("resources.json"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        let executable = directory.path().join("bin/meerkat-connect");
        fs::create_dir(executable.parent().unwrap()).unwrap();
        fs::write(&executable, "fixture executable layout").unwrap();
        let pin = digest(&raw);
        let resources = InstalledResources::from_executable(&executable, &pin).unwrap();
        let selected_root = resources.root.clone();
        let owner = LocalRoundOwner {
            resources,
            artifacts,
            request: json!({"synthetic_round_protocol":true}),
            handoff_sha256: digest(b"synthetic-handoff"),
            poisoned: false,
            process: None,
            test_location: Some((executable, pin)),
        };
        Self {
            _directory: directory,
            output,
            selected_root,
            owner: Some(owner),
            guard: None,
            begin: None,
            finish: None,
            cancel: None,
            last_error: false,
        }
    }

    fn start(&mut self, panic_on_cancel: bool) {
        let (send, receive) = oneshot::channel();
        self.cancel = Some(send);
        let owner = self.owner.take().unwrap();
        self.begin = Some(tokio::spawn(async move {
            owner
                .begin_owned(async move {
                    receive.await.map_err(io::Error::other)?;
                    assert!(!panic_on_cancel, "synthetic caller cancellation panic");
                    Ok(())
                })
                .await
        }));
    }

    async fn receive_begin(&mut self) {
        let result = tokio::time::timeout(BOUND, self.begin.as_mut().unwrap())
            .await
            .expect("retained begin returns after cancellation/readiness");
        self.begin.take();
        self.keep_begin(result.expect("begin task joins"));
    }

    fn keep_begin(&mut self, result: Begin) {
        match result {
            Ok(guard) => {
                self.guard = Some(guard);
                self.last_error = false;
            }
            Err((owner, _error)) => {
                self.owner = Some(owner);
                self.last_error = true;
            }
        }
    }

    fn start_finish(&mut self, settle: bool) {
        let guard = self.guard.take().unwrap();
        self.finish = Some(tokio::spawn(async move {
            if settle {
                guard.settle().await
            } else {
                guard.cancel().await
            }
        }));
    }

    async fn receive_finish(&mut self) {
        let result = tokio::time::timeout(BOUND, self.finish.as_mut().unwrap())
            .await
            .expect("retained finish returns after its actual child join");
        self.finish.take();
        let (owner, result) = result.expect("finish task joins");
        self.owner = Some(owner);
        self.last_error = result.is_err();
    }

    fn cancel(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
    }

    fn pids(&self) -> Vec<i32> {
        fs::read_dir(&self.output)
            .unwrap()
            .filter_map(|entry| {
                let path = entry.unwrap().path();
                if path.extension().and_then(|s| s.to_str()) != Some("started") {
                    return None;
                }
                let value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
                Some(i32::try_from(value["pid"].as_i64().unwrap()).unwrap())
            })
            .collect()
    }

    async fn started(&self) {
        tokio::time::timeout(BOUND, async {
            while self.pids().is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("actual protocol child starts");
    }

    fn assert_reaped(&self) {
        assert!(
            !self.pids().is_empty(),
            "this control must spawn a real child"
        );
        for pid in self.pids() {
            assert_eq!(
                nix::sys::wait::waitpid(
                    nix::unistd::Pid::from_raw(pid),
                    Some(nix::sys::wait::WaitPidFlag::WNOHANG)
                ),
                Err(nix::errno::Errno::ECHILD),
                "the exact child must already have been reaped by its retained Child"
            );
        }
        assert!(!self.owner.as_ref().unwrap().has_pending_round());
    }

    async fn cleanup(&mut self) {
        self.cancel();
        fs::write(self.output.join("release"), "release Ready").unwrap();
        // Do not drop a timed-out task. Cleanup awaits those same retained
        // tasks without inventing a second bound or a false joined flag.
        if let Some(task) = self.begin.take() {
            self.keep_begin(task.await.expect("cleanup joins original begin"));
        }
        if self.guard.is_some() {
            self.start_finish(false);
        }
        if let Some(task) = self.finish.take() {
            let (owner, _) = task.await.expect("cleanup joins original finish");
            self.owner = Some(owner);
        }
        if let Some(owner) = self.owner.as_mut() {
            owner
                .cancel_pending_round()
                .await
                .expect("cleanup joins actual retained child");
        }
    }
}

#[cfg(feature = "console")]
async fn run_case(
    mode: &str,
    body: impl for<'a> FnOnce(&'a mut Case) -> Pin<Box<dyn Future<Output = ()> + 'a>>,
) {
    let mut case = Case::new(mode);
    let result = AssertUnwindSafe(body(&mut case)).catch_unwind().await;
    case.cleanup().await;
    case.assert_reaped();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[cfg(feature = "console")]
#[tokio::test]
async fn owned_success_joins_and_returns_the_same_preparation_for_next_round() {
    run_case("ready", |case| {
        Box::pin(async move {
            let original = case.owner.as_ref().unwrap().artifacts.clone();
            for expected_children in 1..=2 {
                case.start(false);
                case.receive_begin().await;
                assert!(!case.last_error);
                case.started().await;
                assert!(case.guard.as_ref().unwrap().owner.has_pending_round());
                case.start_finish(true);
                case.receive_finish().await;
                assert!(!case.last_error);
                let owner = case.owner.as_ref().unwrap();
                assert_eq!(owner.artifacts, original);
                assert_eq!(owner.resources.root, case.selected_root);
                assert!(!owner.poisoned);
                assert_eq!(case.pids().len(), expected_children);
                case.assert_reaped();
            }
        })
    })
    .await;
}

#[cfg(feature = "console")]
#[tokio::test]
async fn cancelled_begin_and_late_ready_guard_keep_custody_and_poison_reuse() {
    run_case("hold", |case| {
        Box::pin(async move {
            case.start(false);
            case.started().await;
            case.cancel();
            case.receive_begin().await;
            assert!(case.last_error);
            case.assert_reaped();
            let owner = case.owner.as_ref().unwrap();
            assert!(owner.poisoned);
            let count = case.pids().len();
            case.start(false);
            case.receive_begin().await;
            assert!(case.last_error);
            assert_eq!(
                case.pids().len(),
                count,
                "poisoned owner must not spawn again"
            );
        })
    })
    .await;
    // A host can stop waiting just after Ready. It must retain and cancel the
    // returned guard; this is a local custody control, not a native begin claim.
    run_case("ready", |case| {
        Box::pin(async move {
            case.start(false);
            case.receive_begin().await;
            case.started().await;
            case.cancel();
            case.start_finish(false);
            case.receive_finish().await;
            assert!(!case.last_error);
            assert!(case.owner.as_ref().unwrap().poisoned);
            case.assert_reaped();
        })
    })
    .await;
}

#[cfg(feature = "console")]
#[tokio::test]
async fn malformed_ready_and_settlement_return_the_poisoned_owner_after_join() {
    for mode in ["bad_ready", "bad_ack", "oversized_ack"] {
        run_case(mode, |case| {
            Box::pin(async move {
                case.start(false);
                case.receive_begin().await;
                case.started().await;
                if case.guard.is_some() {
                    case.start_finish(true);
                    case.receive_finish().await;
                }
                assert!(case.last_error);
                assert!(case.owner.as_ref().unwrap().poisoned);
                case.assert_reaped();
            })
        })
        .await;
    }
}

#[cfg(feature = "console")]
#[tokio::test]
async fn changed_resources_after_ready_refuse_reuse_after_actual_child_join() {
    run_case("ready", |case| {
        Box::pin(async move {
            case.start(false);
            case.receive_begin().await;
            case.started().await;
            fs::write(
                case.selected_root.join("prepare.py"),
                "changed pinned resource",
            )
            .unwrap();
            case.start_finish(true);
            case.receive_finish().await;
            assert!(case.last_error);
            assert!(case.owner.as_ref().unwrap().poisoned);
            case.assert_reaped();
            let count = case.pids().len();
            case.start(false);
            case.receive_begin().await;
            assert!(case.last_error);
            assert_eq!(case.pids().len(), count);
        })
    })
    .await;
}

#[cfg(feature = "console")]
#[tokio::test]
async fn panicking_cancellation_is_caught_with_child_outside_the_future_and_joined() {
    run_case("hold", |case| {
        Box::pin(async move {
            case.start(true);
            case.started().await;
            case.cancel();
            case.receive_begin().await;
            assert!(case.last_error);
            assert!(case.owner.as_ref().unwrap().poisoned);
            case.assert_reaped();
            assert!(
                case.output
                    .join(format!("{}.ended", case.pids()[0]))
                    .exists(),
                "EOF cleanup ran in the actual child before the poisoned owner returned"
            );
        })
    })
    .await;
}

#[cfg(feature = "console")]
#[tokio::test]
async fn borrowed_guard_uses_the_same_slot_and_drop_keeps_cleanup_accessible() {
    run_case("ready", |case| {
        Box::pin(async move {
            let owner = case.owner.as_mut().unwrap();
            let guard = owner
                .begin(std::future::pending())
                .await
                .expect("borrowed Ready");
            guard.settle().await.expect("borrowed settle joins");
            assert!(!owner.poisoned && !owner.has_pending_round());
            let guard = owner
                .begin(std::future::pending())
                .await
                .expect("second borrowed Ready");
            drop(guard);
            assert!(
                owner.has_pending_round(),
                "dropping the borrow does not drop its child"
            );
            // Observe the requested physical stop through the same retained
            // Child, before invoking cancellation cleanup. The old no-op Drop
            // leaves this waiter pending; all-exit cleanup still owns the child.
            tokio::time::timeout(BOUND, owner.process.as_mut().unwrap().child.wait())
                .await.expect("Drop stops the real child before explicit cleanup")
                .expect("same retained Child observes its exit");
            assert!(owner.process.as_ref().unwrap().input.is_none(), "Drop closes its actual pipe");
            assert!(
                owner.begin(std::future::pending()).await.is_err(),
                "occupied owner refuses overlap"
            );
            owner
                .cancel_pending_round()
                .await
                .expect("same owner joins dropped borrow's child");
            assert!(owner.poisoned && !owner.has_pending_round());
            case.assert_reaped();
        })
    })
    .await;
}

#[tokio::test]
async fn readiness_is_exact_and_refuses_oversized_unterminated_output_without_waiting_for_eof() {
    assert!(read_ready(&mut &b"{\"ready\":true}\n"[..]).await);
    for bytes in [
        &b"{\"ready\":false}"[..],
        &b"{\"ready\":true} "[..],
        &b"{\"ready\":tru"[..],
    ] {
        assert!(!read_ready(&mut &bytes[..]).await);
    }
    let (mut writer, mut reader) = tokio::io::duplex(4096);
    writer.write_all(&vec![b'x'; 4096]).await.unwrap();
    // Keep the writer live: no newline or EOF can release an unbounded
    // readline. The production fixed-size read must refuse immediately.
    assert!(
        !tokio::time::timeout(Duration::from_secs(1), read_ready(&mut reader))
            .await
            .expect("readiness must be bounded")
    );
    drop(writer);
}

#[cfg(feature = "console")]
mod native_attempts;
