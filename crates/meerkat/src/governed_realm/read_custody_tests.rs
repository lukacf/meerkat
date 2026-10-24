//! Currentness decisions under the config document's shared read custody:
//! overlapping decisions both pass, a writer is excluded while one is held,
//! a held writer refuses them, and a changed document with an unchanged
//! generation is still refused. An administrative hold stays exclusive.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use meerkat_core::config_runtime::ConfigWriterLock;
use std::collections::HashMap;
use std::sync::mpsc;
use std::time::Duration;

/// One armed hold: the decision for this state path that next acquires its
/// custody acknowledges and waits for the release before it reads anything.
type CustodyHold = (PathBuf, mpsc::Sender<()>, mpsc::Receiver<()>);

static HOLDS: std::sync::Mutex<Vec<CustodyHold>> = std::sync::Mutex::new(Vec::new());

/// Called by [`RealmInstalledMcp::check`] right after it took its custody.
pub(super) fn hold_after_custody(state_path: &Path) {
    let hold = {
        let mut holds = HOLDS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        holds
            .iter()
            .position(|(path, _, _)| path == state_path)
            .map(|index| holds.remove(index))
    };
    if let Some((_, entered, release)) = hold {
        let _ = entered.send(());
        let _ = release.recv();
    }
}

/// Hold the next decision for `state_path` after it took its custody.
fn hold_next_decision(state_path: &Path) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    HOLDS
        .lock()
        .unwrap()
        .push((state_path.to_path_buf(), entered_tx, release_rx));
    (entered_rx, release_tx)
}

fn ceilinged_server() -> McpServerConfig {
    let mut server = McpServerConfig::stdio("reader", "true", Vec::new(), HashMap::new());
    server.access = Some(meerkat_core::McpServerAccess::allowing(["echo"]));
    server
}

struct Installed {
    _root: tempfile::TempDir,
    paths: RealmPaths,
    installed: Arc<RealmInstalledMcp>,
    server: McpServerConfig,
}

impl Installed {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let paths = meerkat_store::realm_paths_in(root.path(), "custody");
        std::fs::create_dir_all(paths.config_path.parent().unwrap()).unwrap();
        let server = ceilinged_server();
        let runtime = ConfigRuntime::new(
            Arc::new(FileConfigStore::new(
                paths.config_path.clone(),
                meerkat_models::canonical(),
            )),
            paths.config_path.with_file_name(CONFIG_STATE_FILE),
        );
        McpRealmPersistTarget::new(&runtime)
            .add(server.clone(), None)
            .await
            .expect("installed");
        let installed = RealmInstalledMcp::read(&paths).await.expect("read");
        Self {
            _root: root,
            paths,
            installed,
            server,
        }
    }

    fn state_path(&self) -> PathBuf {
        self.paths.config_path.with_file_name(CONFIG_STATE_FILE)
    }

    fn lock_path(&self) -> PathBuf {
        self.state_path().with_extension("lock")
    }
}

#[tokio::test]
async fn an_overlapping_decision_passes_while_one_holds_read_custody() {
    let fixture = Installed::new().await;
    let (entered, release) = hold_next_decision(&fixture.state_path());
    let first = {
        let installed = Arc::clone(&fixture.installed);
        let server = fixture.server.clone();
        std::thread::spawn(move || installed.check(&server))
    };
    entered
        .recv_timeout(Duration::from_secs(10))
        .expect("the first decision holds its read custody");

    // While the first decision holds its custody, before it read anything:
    // a second decision reads the actual document and passes, and a writer
    // is excluded.
    assert_eq!(fixture.installed.check(&fixture.server), Ok(()));
    assert!(
        ConfigWriterLock::try_hold(&fixture.lock_path())
            .unwrap()
            .is_none(),
        "a held read excludes writers"
    );

    release.send(()).unwrap();
    assert_eq!(first.join().unwrap(), Ok(()));
    assert!(
        ConfigWriterLock::try_hold(&fixture.lock_path())
            .unwrap()
            .is_some(),
        "a writer proceeds once the reads released"
    );
}

#[tokio::test]
async fn a_held_writer_refuses_the_decision_until_it_releases() {
    let fixture = Installed::new().await;
    let writer = ConfigWriterLock::try_hold(&fixture.lock_path())
        .unwrap()
        .expect("writer custody");
    assert_eq!(
        fixture.installed.check(&fixture.server),
        Err(McpAttachmentRefusal::NotCurrent)
    );
    drop(writer);
    assert_eq!(fixture.installed.check(&fixture.server), Ok(()));
}

#[tokio::test]
async fn a_replaced_or_removed_installation_at_an_unchanged_generation_is_refused() {
    let fixture = Installed::new().await;
    let generation = std::fs::read(fixture.state_path()).unwrap();

    // The installation replaced by another descriptor under the same name,
    // with no generation published.
    let mut replaced = fixture.server.clone();
    replaced.access = Some(meerkat_core::McpServerAccess::allowing(["echo", "add"]));
    let mut tools = toml::Table::new();
    tools.insert(
        "mcp_servers".to_string(),
        toml::Value::try_from(vec![replaced]).unwrap(),
    );
    let mut document = toml::Table::new();
    document.insert("tools".to_string(), toml::Value::Table(tools));
    std::fs::write(
        &fixture.paths.config_path,
        toml::to_string(&document).unwrap(),
    )
    .unwrap();
    assert_eq!(std::fs::read(fixture.state_path()).unwrap(), generation);
    assert_eq!(
        fixture.installed.check(&fixture.server),
        Err(McpAttachmentRefusal::NotCurrent)
    );

    // Removed altogether.
    std::fs::write(&fixture.paths.config_path, b"").unwrap();
    assert_eq!(std::fs::read(fixture.state_path()).unwrap(), generation);
    assert_eq!(
        fixture.installed.check(&fixture.server),
        Err(McpAttachmentRefusal::NotCurrent)
    );
}

#[tokio::test]
async fn an_administrative_hold_on_the_snapshot_stays_exclusive() {
    let fixture = Installed::new().await;
    let Ok((hold, installed)) = installation_under_writer_exclusion(&fixture.paths, "reader")
    else {
        panic!("the administrative hold");
    };
    assert_eq!(installed, Some(fixture.server.clone()));
    // While an administrative command acts on the snapshot, decisions and
    // other administrative holds are refused.
    assert_eq!(
        fixture.installed.check(&fixture.server),
        Err(McpAttachmentRefusal::NotCurrent)
    );
    assert!(installation_under_writer_exclusion(&fixture.paths, "reader").is_err());
    drop(hold);
    assert_eq!(fixture.installed.check(&fixture.server), Ok(()));
}

/// A process this test forks. It holds a copy of every descriptor the test
/// process has open, as any process spawned meanwhile does until it executes
/// its program, until it is dropped.
#[cfg(unix)]
struct ForkedCopies(nix::unistd::Pid);

#[cfg(unix)]
impl ForkedCopies {
    #[allow(unsafe_code, clippy::disallowed_methods)] // the test's own child, which only waits
    fn fork() -> Self {
        // SAFETY: the child calls only async-signal-safe functions (alarm and
        // pause) until a signal ends it.
        match unsafe { nix::unistd::fork() }.expect("fork") {
            nix::unistd::ForkResult::Child => {
                nix::unistd::alarm::set(600);
                loop {
                    nix::unistd::pause();
                }
            }
            nix::unistd::ForkResult::Parent { child } => Self(child),
        }
    }
}

#[cfg(unix)]
impl Drop for ForkedCopies {
    fn drop(&mut self) {
        // On request (MEERKAT_TEST_DESCRIPTOR_COPY_LINGER_MS), the copy stays open
        // that long first, so an fd sampler watching the run sees it.
        if let Some(linger) = std::env::var("MEERKAT_TEST_DESCRIPTOR_COPY_LINGER_MS")
            .ok()
            .and_then(|millis| millis.parse::<u64>().ok())
        {
            std::thread::sleep(std::time::Duration::from_millis(linger));
        }
        let _ = nix::sys::signal::kill(self.0, nix::sys::signal::Signal::SIGKILL);
        let _ = nix::sys::wait::waitpid(self.0, None);
    }
}

/// A writer's custody released while a process spawned meanwhile still holds
/// copies of the test process's descriptors refuses no decision: releasing
/// custody unlocks it, and no copy keeps it.
#[cfg(unix)]
#[tokio::test]
async fn a_released_writer_refuses_no_decision_while_a_spawned_process_holds_its_descriptor() {
    let fixture = Installed::new().await;
    let writer = ConfigWriterLock::try_hold(&fixture.lock_path())
        .unwrap()
        .expect("writer custody");
    let copies = ForkedCopies::fork();
    drop(writer);
    assert_eq!(
        fixture.installed.check(&fixture.server),
        Ok(()),
        "a decision right after the writer released, while a spawned process holds copies"
    );
    drop(copies);
}
