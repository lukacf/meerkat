//! Future-Core controls through real public router staging and native records.
//! Private request/key/cancellation constructors are never reproduced. Native
//! registration selection uses the existing mechanical fixture hook. The
//! inherited command peer below is synthetic in the original controls. The
//! explicit physical mode instead uses the existing actual Controller child,
//! peer sockets, census and joined snapshot with the same native registrations.
//! This does not qualify native operation authority or whole G recovery.
//!
//! Every case runs its children under TOOLKIT_SHARED_TEST_PYTHON. The
//! synthetic command peer needs only that interpreter's standard library. The
//! physical mode runs `sdk/consumer-setup/tests/shared_native_owner.py` and
//! `local_session.py` peers, which need the whole pinned set of
//! `scripts/connect-test-python-requirements.txt`: mcp 2.2.0 and its closure,
//! among them httpx2, anyio, jsonschema and referencing.
//! `scripts/connect-test-python` prepares that interpreter and CI's Connect
//! unit rows select it; the owner refuses any other before it starts.
use super::*;
use crate::application_setup::compilation::tests::{
    AttemptRegistrationFixture, attempt_registration_fixture,
};
use futures_util::FutureExt;
use meerkat_core::{ExternalToolDeltaPhase, McpServerConfig};
use meerkat_mcp::{McpConstructionOwner, McpRouter, McpStdioLaunchProfile};
use std::{panic::AssertUnwindSafe, sync::Mutex, time::Duration};
use tokio::io::BufReader;

const BOUND: Duration = Duration::from_secs(20);
const CHANNEL: &str = r#"
import json, pathlib, sys
root = pathlib.Path(sys.argv[1]); closed = set()
try:
    for line in sys.stdin:
        envelope = json.loads(line)
        assert set(envelope) == {'id','command'}
        assert type(envelope['id']) is int and 0 < envelope['id'] <= 2**64-1
        row = envelope['command']
        assert isinstance(row, dict) and set(row) == {'peer','operation'}
        peer = row['peer']; op = row['operation']
        assert type(peer) is int and peer in (0,1)
        assert op in ('close_peer','observe_peer')
        with (root/'commands.jsonl').open('a') as f: f.write(json.dumps(envelope)+'\n')
        if op == 'close_peer': closed.add(peer)
        state = 'not_requested' if peer not in closed else ('pending' if (root/f'hold-{peer}').exists() else 'joined')
        answer = {'scope':'host_shared_peer_retirement','peer':peer,'state':state}
        malformed = (root/f'malformed-{peer}').exists()
        if malformed: answer['peer'] = 1-peer
        reply = {'id':envelope['id'],'result':answer}
        if malformed:
            with (root/'malformed-replies.jsonl').open('a') as f:
                f.write(json.dumps({'request':envelope,'response':reply})+'\n')
        print(json.dumps(reply),flush=True)
finally:
    (root/'channel-joined').write_text('inherited command process drained\n')
"#;
const SERVER: &str = r#"
import json, pathlib, sys
root = pathlib.Path(sys.argv[1]); peer = sys.argv[2]
(root/f'spawned-{peer}').write_text('actual native stdio entry\n')
for line in sys.stdin:
    row=json.loads(line); method=row.get('method')
    if 'id' not in row: continue
    if method == 'initialize':
        result={'protocolVersion':'2025-03-26','capabilities':{'tools':{}},'serverInfo':{'name':'attempt-fixture','version':'0'}}
    elif method == 'tools/list':
        result={'tools':[{'name':'setup_selected_check','description':'inert transport control','inputSchema':{'type':'object'}}]}
    elif method == 'tools/call':
        assert row['params']['name']=='setup_selected_check'
        (root/f'called-{peer}').write_text('actual inert native call\n')
        result={'content':[{'type':'text','text':'inert transport call'}]}
    else: raise AssertionError(method)
    print(json.dumps({'jsonrpc':'2.0','id':row['id'],'result':result}),flush=True)
"#;

struct Probe {
    inner: Arc<Factory>,
    requests: Mutex<Vec<McpAttemptGuardRequest>>,
    hold_after_claim: tokio::sync::watch::Sender<bool>,
    held_peer: McpServerConfig,
}
#[async_trait]
impl McpAttemptGuardFactory for Probe {
    async fn begin(
        &self,
        request: McpAttemptGuardRequest,
        cancel: McpAttemptCancellation,
    ) -> Result<Option<Box<dyn McpAttemptGuard>>, McpAttachmentRefusal> {
        self.requests.lock().unwrap().push(request.clone());
        let hold = request.server == self.held_peer;
        let guard = self.inner.begin(request, cancel).await?;
        if hold && guard.is_some() {
            // Hold the actual returned custody inside Core's owned begin.
            // Losing the router must cancel this late guard, not spawn it.
            let mut held = self.hold_after_claim.subscribe();
            let _ = held.wait_for(|value| !*value).await;
        }
        Ok(guard)
    }
}

struct Case {
    directory: tempfile::TempDir,
    owner: Option<SharedSetupOwner>,
    contexts: Vec<Arc<SharedSetupCallContext>>,
    registrations: Vec<AttemptRegistrationFixture>,
    routers: Vec<Option<McpRouter>>,
    notices: Vec<(usize, meerkat_core::ExternalToolDelta)>,
    probe: Arc<Probe>,
    installation: Arc<Installation>,
    expected_failed_cleanup: bool,
    retired: BTreeSet<usize>,
    close_joined: bool,
    // A deliberately foreign consuming join still has a cleanup owner.
    consumed_records: Option<Vec<McpGuardCompletionRecord>>,
    physical: bool,
}
impl Case {
    async fn new(hold: bool) -> Self {
        Self::with_installation(hold, true).await
    }
    async fn with_installation(hold: bool, install: bool) -> Self {
        Self::with_members(hold, install, [None, None]).await
    }
    async fn independent_members(hold: bool) -> Self {
        Self::with_members(
            hold,
            true,
            [Some("helper:attempt-a"), Some("helper:attempt-b")],
        )
        .await
    }
    async fn with_members(hold: bool, install: bool, identities: [Option<&str>; 2]) -> Self {
        Self::with_channel(hold, install, identities, None).await
    }
    async fn with_physical(mode: &str) -> Self {
        Self::with_channel(
            false,
            true,
            [Some("helper:attempt-a"), Some("helper:attempt-b")],
            Some(mode),
        )
        .await
    }
    async fn with_channel(
        hold: bool,
        install: bool,
        identities: [Option<&str>; 2],
        physical: Option<&str>,
    ) -> Self {
        use std::os::unix::fs::PermissionsExt;
        // The physical fence admits only a private (0700) artifact directory.
        // Set this owned root's mode instead of inheriting it from the process
        // umask, which concurrent tests share and must not change.
        let directory = tempfile::Builder::new()
            .permissions(fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let root = directory.path().canonicalize().unwrap();
        let python = PathBuf::from(
            std::env::var_os("TOOLKIT_SHARED_TEST_PYTHON")
                .expect("select the exact test Python explicitly"),
        );
        assert!(python.is_absolute() && python.is_file());
        let mut registrations = Vec::new();
        let mut owner: Option<SharedSetupOwner> = None;
        let built = AssertUnwindSafe(async {
            let mut helpers = Vec::new();
            // These fixture owners are built before any stdio attempt. The same
            // selected native participant is retained in each actual peer slot.
            for (index, identity) in identities.into_iter().enumerate() {
                let member_root = root.join(format!("member-{index}"));
                fs::create_dir(&member_root).unwrap();
                let registration = attempt_registration_fixture(&member_root, identity).await;
                // Retain the registered native session before any fallible
                // projection so constructor cleanup can unregister it.
                registrations.push(registration);
                let registration = registrations.last().unwrap();
                let bytes = serde_json::to_vec(&serde_json::json!({"mcpServers":{"toolkit-setup":{
                "command":python,"args":["-I","-B","-c",SERVER,root,index.to_string()]}}}))
                .unwrap();
                let raw = crate::setup::PreparedSetupConnection::from_preparer_output(&bytes)
                    .unwrap()
                    .into_server();
                let helper = crate::setup::PreparedSetupConnection::from_preparer_output(&bytes)
                    .unwrap()
                    .for_mobkit_helper(
                        registration.source_member.clone(),
                        crate::setup::SetupHelperTools::CheckedInstallation,
                    )
                    .unwrap();
                assert_eq!(helper.member(), &registration.member);
                helpers.push((raw, helper));
            }
            let realm = meerkat_core::RealmLocator {
                state_root: root.clone(),
                realm: meerkat_core::mob_realm_id(&registrations[0].member.mob_id).unwrap(),
            };
            let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../sdk/consumer-setup");
            let source = if physical.is_some() {
                source.canonicalize().unwrap()
            } else {
                source
            };
            let mut command = tokio::process::Command::new(&python);
            let mut lifetime_selection = None;
            if let Some(mode) = physical {
                use std::os::unix::fs::PermissionsExt;
                let helper = source.join("tests/shared_native_owner.py");
                let arguments: Vec<String> = vec![
                    "-i".into(),
                    "PATH=/usr/bin:/bin".into(),
                    "PYTHONDONTWRITEBYTECODE=1".into(),
                    python.to_string_lossy().into_owned(),
                    "-I".into(),
                    "-B".into(),
                    helper.to_string_lossy().into_owned(),
                    root.to_string_lossy().into_owned(),
                ];
                let descriptor = serde_json::to_vec(&serde_json::json!({"mcpServers": {
                    "toolkit-setup": {"command": "/usr/bin/env", "args": arguments}}}))
                .unwrap();
                // Controlled preparation input, not an authored cleanup record.
                lifetime::write_new(&root.join("prepare.stdout"), &descriptor).unwrap();
                let resources = root.join("fixture-resources");
                fs::create_dir(&resources).unwrap();
                fs::write(
                    resources.join("resources.json"),
                    serde_json::to_vec(&serde_json::json!({
                        "fixture_source": helper, "sha256": digest(&fs::read(&helper).unwrap())
                    }))
                    .unwrap(),
                )
                .unwrap();
                fs::set_permissions(
                    resources.join("resources.json"),
                    fs::Permissions::from_mode(0o600),
                )
                .unwrap();
                let identity = "a".repeat(64);
                let (path, sha) = lifetime::PhysicalLifetime::retain(
                    &root,
                    &resources,
                    &realm,
                    &identity,
                    "/usr/bin/env",
                    &arguments,
                    2,
                    None,
                )
                .unwrap();
                command = tokio::process::Command::new("/usr/bin/env");
                command
                    .args(&arguments)
                    .arg("--shared-close-id")
                    .arg(&identity)
                    .arg("--shared-lifetime")
                    .arg(path)
                    .arg("--shared-lifetime-sha256")
                    .arg(&sha)
                    .arg("--census-mode")
                    .arg(mode)
                    .env_clear()
                    .env("PATH", "/usr/bin:/bin");
                lifetime_selection = Some((identity, sha));
            } else {
                command.args(["-I", "-B", "-c", CHANNEL]).arg(&root);
            }
            let mut child = command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .process_group(0)
                .kill_on_drop(true)
                .spawn()
                .unwrap();
            let channel = Arc::new(tokio::sync::Mutex::new(Channel::new(
                child.stdin.take().unwrap(),
                BufReader::new(child.stdout.take().unwrap()),
            )));
            let entries = Arc::new(entry::EntryTasks::default());
            owner = Some(SharedSetupOwner {
                child,
                channel: channel.clone(),
                entries: entries.clone(),
                attempts: OnceLock::new(),
                artifact_directory: root.clone(),
                realm,
                close_identity: lifetime_selection.as_ref().map(|row| row.0.clone()),
                lifetime_sha256: lifetime_selection.as_ref().map(|row| row.1.clone()),
                physical: None,
            });

            if physical.is_some() {
                // The owner was retained before Ready; constructor failure joins
                // this actual child through the existing cleanup path below.
                let paths = {
                    let mut selected = channel.lock().await;
                    tokio::time::timeout(BOUND, read_ready(&mut selected.output, 2))
                        .await
                        .unwrap()
                        .unwrap()
                };
                helpers.clear();
                for (index, path) in paths.into_iter().enumerate() {
                    let bytes = serde_json::to_vec(&serde_json::json!({"mcpServers": {
                        "toolkit-setup": {"command": "/usr/bin/env", "args": ["-i",
                            "PATH=/usr/bin:/bin", "PYTHONDONTWRITEBYTECODE=1", python,
                            "-I", "-B", source.join("local_session.py"), "--shared-peer", path]}
                    }}))
                    .unwrap();
                    let raw = crate::setup::PreparedSetupConnection::from_preparer_output(&bytes)
                        .unwrap()
                        .into_server();
                    let helper =
                        crate::setup::PreparedSetupConnection::from_preparer_output(&bytes)
                            .unwrap()
                            .for_mobkit_helper(
                                registrations[index].source_member.clone(),
                                crate::setup::SetupHelperTools::CheckedInstallation,
                            )
                            .unwrap();
                    helpers.push((raw, helper));
                }
            }

            let mut contexts = Vec::new();
            for (index, (raw, helper)) in helpers.into_iter().enumerate() {
                let registration = &registrations[index];
                let context = SharedSetupCallContext::new(
                    SharedSetupPeer::new(channel.clone(), index, raw, entries.clone()),
                    &helper,
                    Arc::new(NoContext),
                )
                .unwrap();
                context
                    .bind_attempt_registration(
                        registration.member.clone(),
                        registration.session.to_string(),
                        registration.participant.clone(),
                    )
                    .unwrap();
                contexts.push(context);
            }
            let peers: Vec<_> = contexts
                .iter()
                .map(|context| context.attempt_peer())
                .collect();
            let factory = Arc::new(Factory::new(peers));
            let probe = Arc::new(Probe {
                held_peer: factory.peers[0].configuration.clone(),
                inner: factory.clone(),
                requests: Mutex::new(Vec::new()),
                hold_after_claim: tokio::sync::watch::channel(hold).0,
            });
            let installation = Arc::new(Installation {
                factory,
                guards: McpAttemptGuards::new(
                    probe.clone(),
                    McpAttemptGuardBounds::new(
                        Duration::from_millis(80),
                        Duration::from_millis(80),
                        Duration::from_millis(80),
                    )
                    .unwrap(),
                ),
                directory: root.join("attempt-guards"),
                state: tokio::sync::Mutex::new(JoinState::default()),
            });
            if install {
                fs::create_dir(&installation.directory).unwrap();
                assert!(
                    owner
                        .as_ref()
                        .unwrap()
                        .attempts
                        .set(installation.clone())
                        .is_ok()
                );
            }
            (contexts, probe, installation)
        })
        .catch_unwind()
        .await;
        let (contexts, probe, installation) = match built {
            Ok(value) => value,
            Err(panic) => {
                // Retain every acquired owner before another fallible step.
                // Constructor failure joins any started child before unwinding.
                let mut failures = Vec::new();
                for registration in &registrations {
                    if let Err(error) = registration
                        .machine
                        .unregister_session(&registration.session)
                        .await
                    {
                        failures.push(error.to_string());
                    }
                }
                if let Some(owner) = owner.take()
                    && let Err(error) = owner.close().await
                {
                    failures.push(error.to_string());
                }
                assert!(failures.is_empty(), "construction cleanup: {failures:?}");
                std::panic::resume_unwind(panic);
            }
        };
        Self {
            directory,
            owner,
            contexts,
            registrations,
            routers: Vec::new(),
            notices: Vec::new(),
            probe,
            installation,
            expected_failed_cleanup: false,
            retired: BTreeSet::new(),
            close_joined: false,
            consumed_records: None,
            physical: physical.is_some(),
        }
    }
    fn peer(&self, index: usize) -> Arc<SharedSetupPeer> {
        self.contexts[index].attempt_peer()
    }
    fn route(&mut self, index: usize, owner: Option<McpConstructionOwner>) -> usize {
        let registration = &self.registrations[index];
        let owner = owner.unwrap_or_else(|| {
            McpConstructionOwner::new(
                registration.session.clone(),
                Some(registration.member.clone()),
            )
        });
        let router = McpRouter::new_with_surface_handle_and_stdio_profile(
            Arc::new(meerkat_runtime::RuntimeExternalToolSurfaceHandle::ephemeral()),
            McpStdioLaunchProfile::trusted_host(),
        )
        .with_attempt_guards(self.installation.guards(), owner);
        self.routers.push(Some(router));
        self.routers.len() - 1
    }
    async fn add(&mut self, route: usize, config: McpServerConfig, phase: ExternalToolDeltaPhase) {
        let name = config.name.clone();
        let router = self.routers[route].as_mut().unwrap();
        let notices = &mut self.notices;
        router.stage_add(config).unwrap();
        router.apply_staged().await.unwrap();
        let waited = tokio::time::timeout(BOUND, async {
            loop {
                let update = router.take_external_updates();
                // Retain and emit the complete actual typed notices before any
                // assertion. A failed setup must not disappear behind a later
                // timeout waiting only for Applied. These inert-fixture records
                // contain no live credentials or provider responses.
                if !update.notices.is_empty() {
                    eprintln!("attempt_fixture_notices {}", serde_json::json!({
                        "route": route, "expected_target": name, "expected_phase": phase,
                        "notices": update.notices, "pending": update.pending,
                    }));
                }
                let matched = update.notices.iter().any(|notice| {
                    notice.target == name && notice.phase == phase
                        && notice.operation == meerkat_core::ToolConfigChangeOperation::Add
                });
                let wrong_terminal = update.notices.iter().any(|notice| {
                    notice.target == name
                        && matches!(notice.phase,
                            ExternalToolDeltaPhase::Applied | ExternalToolDeltaPhase::Failed
                                | ExternalToolDeltaPhase::Forced)
                        && (notice.phase != phase
                            || notice.operation != meerkat_core::ToolConfigChangeOperation::Add)
                });
                notices.extend(update.notices.into_iter().map(|notice| (route, notice)));
                assert!(!wrong_terminal,
                    "unexpected terminal native notice for route {route}, expected {phase:?}: {notices:?}");
                if matched { break; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await;
        assert!(
            waited.is_ok(),
            "actual native attempt notice for route {route}: {waited:?}; retained: {notices:?}"
        );
    }
    async fn close_router(&mut self, index: usize) -> meerkat_mcp::McpRouterShutdownReport {
        self.routers[index]
            .take()
            .unwrap()
            .shutdown_with_report()
            .await
    }
    async fn cleanup(&mut self) -> Vec<String> {
        let mut failures = Vec::new();
        self.probe.hold_after_claim.send_replace(false);
        for index in 0..2 {
            for prefix in ["hold", "malformed"] {
                let _ = fs::remove_file(self.directory.path().join(format!("{prefix}-{index}")));
            }
        }
        let hidden = self.directory.path().join("hidden-records");
        if hidden.exists() {
            let _ = fs::remove_file(&self.installation.directory);
            if let Err(error) = fs::rename(hidden, &self.installation.directory) {
                failures.push(error.to_string());
            }
        }
        if self.consumed_records.is_some() {
            // Restore actual native evidence before any later join, including
            // when the missing-coverage assertion failed. Do not strand close
            // by dropping the only vector after Core has forgotten it.
            let mut state = self.installation.state.lock().await;
            if let Some(records) = self.consumed_records.take() {
                state.unsaved.get_or_insert_with(Vec::new).extend(records);
            }
        }
        for router in &mut self.routers {
            if let Some(router) = router.take() {
                let report = router.shutdown_with_report().await;
                if !report.is_settled() {
                    failures.push(format!("native process close: {report:?}"));
                }
            }
        }
        for (index, registration) in self.registrations.iter().enumerate() {
            if self.retired.contains(&index) {
                continue;
            }
            if let Err(error) = registration
                .machine
                .unregister_session(&registration.session)
                .await
            {
                failures.push(error.to_string());
            }
        }
        if let Some(owner) = self.owner.take() {
            if owner.attempts.get().is_some() {
                match owner
                    .join_attempt_guards(tokio::time::Instant::now() + BOUND)
                    .await
                {
                    Ok(report)
                        if report.native_drained
                            && report.missing_terminal_receipts == 0
                            && report.failed == self.expected_failed_cleanup => {}
                    value => failures.push(format!("bounded attempt join: {value:?}")),
                }
            }
            match tokio::time::timeout(BOUND, owner.close()).await {
                Ok(result) => {
                    self.close_joined = true;
                    match result {
                        Ok(()) if !self.expected_failed_cleanup => (),
                        Err(_) if self.expected_failed_cleanup => (),
                        value => failures.push(format!("physical close disposition: {value:?}")),
                    }
                }
                Err(error) => failures.push(format!("physical close wait timed out: {error}")),
            }
        }
        if self.owner.is_none() && !self.close_joined {
            if let Some(retained) = self.peer(0).entries.retained_resource_close() {
                // Holds and storage have already been restored. A timed-out
                // receiver did not join this task, so preserve that failure
                // and join the same owner before dropping its directory.
                match (&mut *retained.lock().await).await {
                    Ok(Ok(())) if !self.expected_failed_cleanup => (),
                    Ok(Err(_)) if self.expected_failed_cleanup => (),
                    value => failures.push(format!("retained resource close: {value:?}")),
                }
                self.close_joined = true;
            } else {
                failures.push("physical close task was not retained".into());
            }
        }
        if self.physical {
            match fs::read(self.directory.path().join("OWNER-JOINED.json"))
                .ok()
                .and_then(|raw| serde_json::from_slice::<serde_json::Value>(&raw).ok())
            {
                Some(value) if value["workers_alive"] == 0 && value["closing"] == true => (),
                _ => failures.push("actual Controller child lacks joined observation".into()),
            }
        } else if !self.directory.path().join("channel-joined").is_file() {
            failures.push("inherited process lacks drained receipt".into());
        }
        failures
    }
}
struct NoContext;
#[async_trait]
impl meerkat_mcp::McpCallContextProvider for NoContext {
    async fn prepare(
        &self,
        _: meerkat_mcp::McpCallTarget<'_>,
        _: meerkat_core::ToolCallView<'_>,
        _: &meerkat_core::ToolDispatchContext,
    ) -> Result<Option<meerkat_mcp::McpCallContext>, meerkat_mcp::McpCallContextError> {
        Ok(None)
    }
}

async fn complete(case: &mut Case, checked: Result<(), Box<dyn std::any::Any + Send>>) {
    let failures = case.cleanup().await;
    assert!(
        failures.is_empty(),
        "all actual owners must join: {failures:?}"
    );
    if let Err(panic) = checked {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
async fn native_attempt_claim_checks_exact_registration_descriptor_and_one_use() {
    let mut case = Case::independent_members(false).await;
    let checked = AssertUnwindSafe(async {
        let owner = case.owner.as_ref().unwrap();
        let realm = owner.realm.clone();
        assert!(owner.selects_peer(&case.peer(0), &realm));
        let mut foreign_realm = realm.clone();
        foreign_realm.realm = meerkat_core::mob_realm_id("other-application").unwrap();
        assert!(!owner.selects_peer(&case.peer(0), &foreign_realm));
        let other_root = tempfile::tempdir().unwrap();
        foreign_realm = realm.clone();
        foreign_realm.state_root = other_root.path().to_path_buf();
        assert!(!owner.selects_peer(&case.peer(0), &foreign_realm));
        let alias = case.directory.path().join("state-alias");
        std::os::unix::fs::symlink(&realm.state_root, &alias).unwrap();
        let mut aliased = realm.clone();
        aliased.state_root = alias;
        assert!(owner.selects_peer(&case.peer(0), &aliased));
        // A descriptor and channel cannot substitute for this physical
        // owner's retained entry-task custody.
        let foreign_peer = SharedSetupPeer::new(
            owner.channel.clone(),
            0,
            case.peer(0).configuration.clone(),
            Arc::new(entry::EntryTasks::default()),
        );
        assert!(!owner.selects_peer(&foreign_peer, &realm));
        assert_ne!(
            case.registrations[0].source_member,
            case.registrations[1].source_member
        );
        assert_ne!(case.registrations[0].member, case.registrations[1].member);
        assert_eq!(
            case.peer(0).configuration.name,
            case.peer(1).configuration.name
        );
        let config = case.peer(0).configuration.clone();
        let mut wrong_member = case.registrations[0].member.clone();
        wrong_member.member.push_str("-wrong");
        let bad_owners = [
            McpConstructionOwner::new(case.registrations[0].session.clone(), Some(wrong_member)),
            McpConstructionOwner::new(
                meerkat_core::SessionId::new(),
                Some(case.registrations[0].member.clone()),
            ),
        ];
        for owner in bad_owners {
            let route = case.route(0, Some(owner));
            case.add(route, config.clone(), ExternalToolDeltaPhase::Failed)
                .await;
            assert!(case.peer(0).attempt.claim.get().is_none());
        }
        let route = case.route(0, None);
        let mut wrong = config.clone();
        wrong.access = None;
        case.add(route, wrong, ExternalToolDeltaPhase::Failed).await;
        assert!(case.peer(0).attempt.claim.get().is_none());
        let route = case.route(0, None);
        case.add(route, config.clone(), ExternalToolDeltaPhase::Applied)
            .await;
        let claimed = case.peer(0).attempt.claim.get().unwrap().key.clone();
        let route = case.route(0, None);
        case.add(route, config, ExternalToolDeltaPhase::Failed)
            .await;
        assert_eq!(case.peer(0).attempt.claim.get().unwrap().key, claimed);
        assert!(
            case.contexts[0]
                .bind_attempt_registration(
                    case.registrations[0].member.clone(),
                    case.registrations[0].session.to_string(),
                    case.registrations[0].participant.clone()
                )
                .is_err()
        );
        case.registrations[1]
            .machine
            .unregister_session(&case.registrations[1].session)
            .await
            .unwrap();
        case.retired.insert(1);
        let route = case.route(1, None);
        case.add(
            route,
            case.peer(1).configuration.clone(),
            ExternalToolDeltaPhase::Failed,
        )
        .await;
        assert!(case.peer(1).attempt.claim.get().is_none());
        assert!(!case.directory.path().join("spawned-1").exists());
        let route = case.route(1, None);
        let mut unrelated = case.peer(1).configuration.clone();
        unrelated.name = "unrelated-stdio".into();
        case.add(route, unrelated, ExternalToolDeltaPhase::Applied)
            .await;
        assert!(
            case.peer(1).attempt.claim.get().is_none(),
            "unrelated routing carries no setup claim"
        );
    })
    .catch_unwind()
    .await;
    complete(&mut case, checked).await;
}

#[tokio::test]
async fn dropped_begin_and_pending_finish_retain_a_while_b_remains_usable() {
    let mut case = Case::independent_members(true).await;
    let checked = AssertUnwindSafe(async {
        assert_eq!(
            case.registrations[0].member.mob_id,
            case.registrations[1].member.mob_id
        );
        assert_ne!(
            case.registrations[0].member, case.registrations[1].member,
            "independent peers require distinct validated native members"
        );
        assert_ne!(case.registrations[0].session, case.registrations[1].session);
        fs::write(case.directory.path().join("hold-0"), b"held drain").unwrap();
        let a = case.route(0, None);
        let b = case.route(1, None);
        let config = case.peer(0).configuration.clone();
        let router = case.routers[a].as_mut().unwrap();
        router.stage_add(config).unwrap();
        router.apply_staged().await.unwrap();
        tokio::time::timeout(BOUND, async {
            while case.peer(0).attempt.claim.get().is_none() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        drop(case.routers[a].take());
        case.probe.hold_after_claim.send_replace(false);
        case.add(
            b,
            case.peer(1).configuration.clone(),
            ExternalToolDeltaPhase::Applied,
        )
        .await;
        tokio::time::timeout(BOUND, async {
            while !case.directory.path().join("commands.jsonl").exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert!(
            !case.directory.path().join("spawned-0").exists(),
            "late begin may not spawn A"
        );
        assert_eq!(
            case.peer(1).observe_peer().await.unwrap(),
            control::PeerRetirement::NotRequested
        );
        let b_router = case.routers[b].as_ref().unwrap();
        assert!(
            b_router
                .list_tools()
                .iter()
                .any(|tool| tool.name.as_ref() == "setup_selected_check"),
            "the actual raw tool is offered without a configured alias"
        );
        b_router
            .call_tool("setup_selected_check", &serde_json::json!({}))
            .await
            .unwrap();
        assert!(case.directory.path().join("called-1").is_file());
        let waiting = case
            .owner
            .as_ref()
            .unwrap()
            .join_attempt_guards(tokio::time::Instant::now() + Duration::from_millis(40));
        drop(waiting); // The owner already retained join and evidence save.
        tokio::time::timeout(BOUND, async {
            while case.installation.state.lock().await.batches == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert!(!case.installation.state.lock().await.pending.is_empty());
        assert!(
            !case.contexts[0].attempt_renewable(
                &case.registrations[0].member,
                &case.registrations[0].session.to_string()
            ),
            "pending native drain cannot authorize replacement"
        );
        // Once the same native pending state has settled into the snapshot,
        // repeated observations must not append identical evidence forever.
        let first = case
            .owner
            .as_ref()
            .unwrap()
            .join_attempt_guards(tokio::time::Instant::now() + Duration::from_millis(40))
            .await
            .unwrap();
        let second = case
            .owner
            .as_ref()
            .unwrap()
            .join_attempt_guards(tokio::time::Instant::now() + Duration::from_millis(40))
            .await
            .unwrap();
        assert_eq!(first.recorded_batches, second.recorded_batches);
        assert_eq!(
            case.peer(1).observe_peer().await.unwrap(),
            control::PeerRetirement::NotRequested
        );
        fs::remove_file(case.directory.path().join("hold-0")).unwrap();
        let report = case.close_router(b).await;
        assert!(report.is_settled());
        let report = case
            .owner
            .as_ref()
            .unwrap()
            .join_attempt_guards(tokio::time::Instant::now() + BOUND)
            .await
            .unwrap();
        assert!(
            report.is_complete(),
            "same finish gains final evidence: {report:?}"
        );
        assert_eq!(
            case.peer(0).observe_peer().await.unwrap(),
            control::PeerRetirement::Joined
        );
        assert!(case.contexts[0].attempt_renewable(
            &case.registrations[0].member,
            &case.registrations[0].session.to_string()
        ));
        assert!(!case.contexts[0].attempt_unbound());
        assert!(!case.contexts[0].attempt_renewable(
            &case.registrations[1].member,
            &case.registrations[0].session.to_string()
        ));
        assert!(!case.contexts[0].attempt_renewable(
            &case.registrations[0].member,
            &case.registrations[1].session.to_string()
        ));
        let old = case.peer(0);
        let replacement = SharedSetupPeer::new(
            old.channel.clone(),
            2,
            old.configuration.clone(),
            old.entries.clone(),
        );
        assert!(
            replacement.attempt.unbound(),
            "only a new peer owns fresh OnceLocks"
        );
        assert!(old.attempt.claim.get().is_some() && old.attempt.registration.get().is_some());
        assert!(!replacement.attempt.renewable(
            &case.registrations[0].member,
            &case.registrations[0].session.to_string()
        ));
        let commands = fs::read_to_string(case.directory.path().join("commands.jsonl")).unwrap();
        assert!(
            commands.lines().all(|line| {
                let value: serde_json::Value = serde_json::from_str(line).unwrap();
                matches!(
                    value["command"]["operation"].as_str(),
                    Some("close_peer" | "observe_peer")
                )
            }),
            "lease never sends P leave or Controller stop"
        );
    })
    .catch_unwind()
    .await;
    complete(&mut case, checked).await;
}

#[tokio::test]
async fn failed_finish_and_unsaved_native_batch_survive_later_empty_join() {
    let mut case = Case::new(false).await;
    case.expected_failed_cleanup = true;
    let checked = AssertUnwindSafe(async {
        let a = case.route(0, None);
        case.add(
            a,
            case.peer(0).configuration.clone(),
            ExternalToolDeltaPhase::Applied,
        )
        .await;
        fs::write(
            case.directory.path().join("malformed-0"),
            b"wrong peer in exact reply",
        )
        .unwrap();
        let native = case.close_router(a).await;
        assert!(
            native
                .guards()
                .iter()
                .any(|row| row.completion == McpGuardCompletion::Failed)
        );
        let malformed =
            fs::read_to_string(case.directory.path().join("malformed-replies.jsonl")).unwrap();
        let replies: Vec<serde_json::Value> = malformed
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(replies.len(), 1, "one intentional wrong-peer response");
        let id = replies[0]["request"]["id"].as_u64().unwrap();
        assert_eq!(
            replies[0],
            serde_json::json!({
                "request": {"id": id, "command": {"operation": "close_peer", "peer": 0}},
                "response": {"id": id, "result": {
                    "scope": "host_shared_peer_retirement", "peer": 1, "state": "joined"
                }}
            })
        );
        fs::remove_file(case.directory.path().join("malformed-0")).unwrap();
        assert_eq!(
            case.peer(0).observe_peer().await.unwrap(),
            control::PeerRetirement::Joined,
            "the actual command owner remains usable after the typed refusal"
        );
        // Model a different host caller consuming the real native history.
        // A later empty join must not count the selected peer as settled merely
        // because Core no longer retains its ended records. Keep the actual
        // returned vector in Case before any fallible check, so failed
        // assertions still restore it through the same cleanup owner.
        case.consumed_records = Some(
            case.installation
                .guards
                .close_and_join(tokio::time::Instant::now() + BOUND)
                .await,
        );
        assert!(
            case.consumed_records
                .as_ref()
                .unwrap()
                .iter()
                .any(|row| row.completion == McpGuardCompletion::Failed)
        );
        let missing = case
            .owner
            .as_ref()
            .unwrap()
            .join_attempt_guards(tokio::time::Instant::now() + BOUND)
            .await
            .unwrap();
        assert_eq!(missing.pending_calls, 0);
        assert!(missing.native_drained && missing.missing_terminal_receipts > 0);
        assert!(missing.failed && !missing.is_complete());
        assert_eq!(missing.recorded_calls, 0);
        assert!(case.installation.state.lock().await.terminal.is_empty());
        // Test-only handoff of the retained actual vector to the same save
        // slot. This is not a production recovery API or reconstructed record.
        {
            let mut state = case.installation.state.lock().await;
            state.unsaved = case.consumed_records.take();
        }
        fs::rename(
            &case.installation.directory,
            case.directory.path().join("hidden-records"),
        )
        .unwrap();
        fs::write(&case.installation.directory, b"not a directory").unwrap();
        assert!(
            case.owner
                .as_ref()
                .unwrap()
                .join_attempt_guards(tokio::time::Instant::now() + BOUND)
                .await
                .is_err()
        );
        assert!(
            case.installation
                .state
                .lock()
                .await
                .unsaved
                .as_ref()
                .unwrap()
                .iter()
                .any(|row| row.completion == McpGuardCompletion::Failed)
        );
        fs::remove_file(&case.installation.directory).unwrap();
        fs::rename(
            case.directory.path().join("hidden-records"),
            &case.installation.directory,
        )
        .unwrap();
        let saved = case
            .owner
            .as_ref()
            .unwrap()
            .join_attempt_guards(tokio::time::Instant::now() + BOUND)
            .await
            .unwrap();
        let empty = case
            .owner
            .as_ref()
            .unwrap()
            .join_attempt_guards(tokio::time::Instant::now() + BOUND)
            .await
            .unwrap();
        assert!(saved.failed && empty.failed && empty.pending_calls == 0);
        assert_eq!(saved.recorded_calls, empty.recorded_calls);
        assert_eq!(saved.recorded_batches, empty.recorded_batches);
        let immediate = case
            .owner
            .as_ref()
            .unwrap()
            .join_attempt_guards(tokio::time::Instant::now())
            .await
            .unwrap();
        assert_eq!(immediate.recorded_batches, empty.recorded_batches);
        assert_eq!(
            case.installation.state.lock().await.batches,
            empty.recorded_batches
        );
        let bytes = fs::read_to_string(
            case.installation
                .directory
                .join("native-attempt-join-1.json"),
        )
        .unwrap();
        assert!(
            bytes.contains("Failed"),
            "complete native evidence remains host-private and unparsed"
        );
    })
    .catch_unwind()
    .await;
    complete(&mut case, checked).await;
}

#[tokio::test]
async fn factory_installation_refuses_overwrite_and_resource_close_survives_caller_loss() {
    let mut case = Case::with_installation(false, false).await;
    let checked = AssertUnwindSafe(async {
        let owner = case.owner.as_ref().unwrap();
        let opaque =
            meerkat::AgentFactory::minimal().with_mcp_attempt_guards(case.installation.guards());
        assert!(
            owner
                .install_attempt_guards(opaque, &case.contexts)
                .is_err()
        );
        assert!(owner.attempts.get().is_none());
        assert!(!case.installation.directory.exists());
        assert!(
            owner
                .install_attempt_guards(
                    meerkat::AgentFactory::minimal(),
                    &[case.contexts[0].clone(), case.contexts[0].clone()]
                )
                .is_err()
        );
        let mut foreign = case.peer(1).configuration.clone();
        foreign.access = None;
        let bytes = serde_json::to_vec(&serde_json::json!({"mcpServers":{"toolkit-setup":{
            "command":match &foreign.transport {
                meerkat_core::mcp_config::McpTransportConfig::Stdio(value) => &value.command,
                _ => panic!("test selects stdio"),
            },
            "args":match &foreign.transport {
                meerkat_core::mcp_config::McpTransportConfig::Stdio(value) => &value.args,
                _ => panic!("test selects stdio"),
            },
        }}}))
        .unwrap();
        let helper = crate::setup::PreparedSetupConnection::from_preparer_output(&bytes)
            .unwrap()
            .for_mobkit_helper(
                case.registrations[1].source_member.clone(),
                crate::setup::SetupHelperTools::CheckedInstallation,
            )
            .unwrap();
        assert_eq!(helper.member(), &case.registrations[1].member);
        let foreign = SharedSetupCallContext::new(
            SharedSetupPeer::new(
                owner.channel.clone(),
                1,
                foreign,
                Arc::new(entry::EntryTasks::default()),
            ),
            &helper,
            Arc::new(NoContext),
        )
        .unwrap();
        assert!(
            owner
                .install_attempt_guards(
                    meerkat::AgentFactory::minimal(),
                    &[case.contexts[0].clone(), foreign]
                )
                .is_err()
        );
        assert!(owner.attempts.get().is_none());
        let factory = owner
            .install_attempt_guards(meerkat::AgentFactory::minimal(), &case.contexts)
            .unwrap();
        assert!(factory.mcp_attempt_guards().is_some());
        case.installation = owner.attempts.get().unwrap().clone();
        assert!(
            owner
                .install_attempt_guards(factory, &case.contexts)
                .is_err()
        );
        assert!(Arc::ptr_eq(
            owner.attempts.get().unwrap(),
            &case.installation
        ));
        assert!(bounded_debug(&"x".repeat(MAX_NATIVE_DEBUG_BYTES)).is_err());
        assert_eq!(bounded_debug(&42).unwrap(), "42");

        let a = case.route(0, None);
        let b = case.route(1, None);
        case.add(
            a,
            case.peer(0).configuration.clone(),
            ExternalToolDeltaPhase::Applied,
        )
        .await;
        case.add(
            b,
            case.peer(1).configuration.clone(),
            ExternalToolDeltaPhase::Applied,
        )
        .await;
        fs::write(case.directory.path().join("hold-0"), b"retained peer drain").unwrap();
        let a_closed = case.close_router(a).await;
        assert!(a_closed.is_settled());
        assert!(
            a_closed
                .guards()
                .iter()
                .any(|row| row.completion == McpGuardCompletion::TimedOut)
        );
        assert!(case.close_router(b).await.is_settled());
        let pending = case
            .owner
            .as_ref()
            .unwrap()
            .join_attempt_guards(tokio::time::Instant::now() + Duration::from_millis(40))
            .await
            .unwrap();
        assert!(pending.pending_calls > 0 && !pending.is_complete());
        let entries = case.peer(0).entries.clone();
        let closing = case.owner.take().unwrap().close();
        drop(closing); // Unpolled receiver loss leaves the actual child owner.
        let retained = entries.retained_resource_close().unwrap();
        assert!(!retained.lock().await.is_finished());
        fs::remove_file(case.directory.path().join("hold-0")).unwrap();
        tokio::time::timeout(BOUND, async { (&mut *retained.lock().await).await })
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        case.close_joined = true;
        assert!(
            case.installation
                .report(&*case.installation.state.lock().await)
                .is_complete()
        );
        assert!(case.directory.path().join("channel-joined").is_file());
    })
    .catch_unwind()
    .await;
    complete(&mut case, checked).await;
}

#[tokio::test]
async fn missing_terminal_receipt_allows_failed_physical_close_after_current_native_drain() {
    let mut case = Case::new(false).await;
    case.expected_failed_cleanup = true;
    let checked = AssertUnwindSafe(async {
        let a = case.route(0, None);
        case.add(
            a,
            case.peer(0).configuration.clone(),
            ExternalToolDeltaPhase::Applied,
        )
        .await;
        fs::write(
            case.directory.path().join("hold-0"),
            b"hold actual peer drain",
        )
        .unwrap();
        let report = case.close_router(a).await;
        assert!(
            report.is_settled(),
            "native process joined before peer drain"
        );
        assert!(
            report
                .guards()
                .iter()
                .any(|row| row.completion == McpGuardCompletion::TimedOut)
        );
        let pending = case
            .owner
            .as_ref()
            .unwrap()
            .join_attempt_guards(tokio::time::Instant::now() + Duration::from_millis(100))
            .await
            .unwrap();
        assert!(pending.pending_calls > 0 && !pending.native_drained);
        assert!(!case.installation.state.lock().await.pending.is_empty());
        fs::remove_file(case.directory.path().join("hold-0")).unwrap();
        // A foreign caller consumes the actual ended records after our saved
        // Held/TimedOut observation. Case retains them solely for failure-path
        // cleanup; the successful path never restores or ingests this vector.
        case.consumed_records = Some(
            case.installation
                .guards
                .close_and_join(tokio::time::Instant::now() + BOUND)
                .await,
        );
        assert!(
            case.consumed_records
                .as_ref()
                .unwrap()
                .iter()
                .any(|row| row.completion == McpGuardCompletion::Completed
                    && row.disposition.is_some())
        );
        let immediate = case
            .owner
            .as_ref()
            .unwrap()
            .join_attempt_guards(tokio::time::Instant::now())
            .await
            .unwrap();
        assert_eq!(
            immediate.pending_calls, 0,
            "old pending is not current work"
        );
        assert!(!immediate.native_drained && !immediate.is_complete());
        assert_eq!(immediate.missing_terminal_receipts, 1);
        let missing = case
            .owner
            .as_ref()
            .unwrap()
            .join_attempt_guards(tokio::time::Instant::now() + BOUND)
            .await
            .unwrap();
        assert!(missing.native_drained && missing.failed && !missing.is_complete());
        assert_eq!(missing.pending_calls, 0);
        assert_eq!(missing.missing_terminal_receipts, 1);
        assert!(case.installation.state.lock().await.terminal.is_empty());
        assert!(case.installation.state.lock().await.unsaved.is_none());
        let result = tokio::time::timeout(BOUND, case.owner.take().unwrap().close())
            .await
            .expect("missing history cannot keep the drained Controller alive");
        case.close_joined = true;
        assert!(
            result.is_err(),
            "missing native evidence cannot report success"
        );
        assert!(case.directory.path().join("channel-joined").is_file());
        assert!(
            case.installation.state.lock().await.terminal.is_empty(),
            "physical close did not manufacture or restore terminal evidence"
        );
        let after = case
            .installation
            .report(&*case.installation.state.lock().await);
        assert!(after.failed && after.native_drained && !after.is_complete());
        assert_eq!(after.missing_terminal_receipts, 1);
        // Deliberately model an irreversible foreign loss after physical close
        // has returned Err. No receipt was restored to make that close finish.
        drop(case.consumed_records.take());
    })
    .catch_unwind()
    .await;
    complete(&mut case, checked).await;
}

// This additional control observes Linux /proc membership in the fixture
// child's private process group. Keep its census and test Linux-only; the
// other owner controls use Unix APIs and do not depend on /proc.
#[cfg(target_os = "linux")]
fn fixture_group_members(group: u32) -> Vec<u32> {
    let mut members = Vec::new();
    for entry in fs::read_dir("/proc").unwrap() {
        let entry = entry.unwrap();
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(stat) = fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        let Some((_, fields)) = stat.rsplit_once(')') else {
            panic!("invalid process stat for fixture group observation");
        };
        if fields
            .split_whitespace()
            .nth(2)
            .and_then(|value| value.parse::<u32>().ok())
            == Some(group)
        {
            members.push(pid);
        }
    }
    members
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn drained_physical_owner_closes_while_failed_persistence_remains_owned() {
    let mut case = Case::new(false).await;
    case.expected_failed_cleanup = true;
    let checked = AssertUnwindSafe(async {
        let child = case.owner.as_ref().unwrap().child.id().unwrap();
        assert!(fixture_group_members(child).contains(&child));
        let a = case.route(0, None);
        case.add(
            a,
            case.peer(0).configuration.clone(),
            ExternalToolDeltaPhase::Applied,
        )
        .await;
        fs::write(case.directory.path().join("hold-0"), b"held native finish").unwrap();
        let process = case.close_router(a).await;
        assert!(process.is_settled());
        assert!(
            process
                .guards()
                .iter()
                .any(|row| row.completion == McpGuardCompletion::TimedOut)
        );
        let pending = case
            .owner
            .as_ref()
            .unwrap()
            .join_attempt_guards(tokio::time::Instant::now() + Duration::from_millis(100))
            .await
            .unwrap();
        assert!(pending.pending_calls > 0 && !pending.native_drained);
        assert!(case.installation.state.lock().await.unsaved.is_none());
        assert!(fixture_group_members(child).contains(&child));
        assert!(!case.directory.path().join("channel-joined").exists());

        fs::remove_file(case.directory.path().join("hold-0")).unwrap();
        fs::rename(
            &case.installation.directory,
            case.directory.path().join("hidden-records"),
        )
        .unwrap();
        fs::write(&case.installation.directory, b"persistent save refusal").unwrap();
        assert!(
            case.owner
                .as_ref()
                .unwrap()
                .join_attempt_guards(tokio::time::Instant::now() + BOUND)
                .await
                .is_err()
        );
        let (number, actual_records) = {
            let state = case.installation.state.lock().await;
            let records = state.unsaved.as_ref().unwrap();
            assert!(records.iter().any(|row| row.disposition.is_some()));
            (state.batches + 1, bounded_debug(records).unwrap())
        };
        let entries = case.peer(0).entries.clone();
        drop(case.owner.take().unwrap().close());
        let retained = entries.retained_resource_close().unwrap();
        // The old production ordering must fail here with the actual physical
        // child still alive, before any returned boolean is checked. Case
        // cleanup restores storage and joins the same retained task on failure.
        tokio::time::timeout(BOUND, async {
            loop {
                if case.directory.path().join("channel-joined").is_file()
                    && !Path::new("/proc").join(child.to_string()).exists()
                    && fixture_group_members(child).is_empty()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("drained child and its group must stop while persistence is still refused");
        assert!(case.installation.directory.is_file());
        assert!(!retained.lock().await.is_finished());
        {
            let state = case.installation.state.lock().await;
            assert!(state.native_drained && state.failed && state.pending.is_empty());
            assert_eq!(
                bounded_debug(state.unsaved.as_ref().unwrap()).unwrap(),
                actual_records
            );
            assert!(!case.installation.report(&state).is_complete());
        }

        fs::remove_file(&case.installation.directory).unwrap();
        fs::rename(
            case.directory.path().join("hidden-records"),
            &case.installation.directory,
        )
        .unwrap();
        let completed = tokio::time::timeout(BOUND, async { (&mut *retained.lock().await).await })
            .await
            .expect("restored storage settles the same retained close task");
        case.close_joined = true;
        assert!(
            completed.unwrap().is_err(),
            "save failure remains a failed close"
        );
        let saved: serde_json::Value = serde_json::from_slice(
            &fs::read(
                case.installation
                    .directory
                    .join(format!("native-attempt-join-{number}.json")),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            saved["native_records_debug"].as_str(),
            Some(actual_records.as_str())
        );
        let state = case.installation.state.lock().await;
        assert!(state.unsaved.is_none() && state.failed);
        assert!(!case.installation.report(&state).is_complete());
        assert!(fixture_group_members(child).is_empty());
    })
    .catch_unwind()
    .await;
    complete(&mut case, checked).await;
}

#[tokio::test]
async fn abandoned_physical_disposition_requires_real_native_and_controller_joins() {
    // The preparer/Controller fixture is controlled; native claim and router
    // retirement, shared listeners, census, child exit and Rust artifact
    // production are real. No native tool result or G restoration is claimed.
    for mode in ["not_entered", "entered", "missing_identity", "unclaimed"] {
        let mut case = Case::with_physical(if mode == "entered" {
            "entered"
        } else {
            "not_entered"
        })
        .await;
        let checked = AssertUnwindSafe(async {
            let selected = case.peer(0);
            assert!(selected.attempt.closed_registration().is_none());
            if mode != "unclaimed" {
                let route = case.route(0, None);
                case.add(
                    route,
                    selected.configuration.clone(),
                    ExternalToolDeltaPhase::Applied,
                )
                .await;
                assert!(selected.attempt.claim.get().is_some());
                assert!(selected.attempt.closed_registration().is_none());
                let report = case.close_router(route).await;
                assert!(
                    report.is_settled(),
                    "actual native stdio retirement: {report:?}"
                );
                assert_eq!(
                    selected.attempt.closed_registration(),
                    Some((
                        case.registrations[0].member.clone(),
                        case.registrations[0].session.to_string()
                    ))
                );
            } else {
                assert!(selected.attempt.claim.get().is_none());
            }
            let root = case.directory.path().canonicalize().unwrap();
            let identity = case.owner.as_ref().unwrap().close_identity.clone().unwrap();
            let lifetime_path = root.join(format!("shared-lifetime-{identity}.json"));
            let lifetime_before = fs::read(&lifetime_path).unwrap();
            if mode == "missing_identity" {
                // Actual identity loss is not replaced by a reconstructed row.
                fs::remove_file(&lifetime_path).unwrap();
            }
            let owner = case.owner.take().unwrap();
            let disposition = tokio::time::timeout(
                BOUND,
                owner.close_abandoned_for_recovery(case.contexts[0].clone()),
            )
            .await
            .expect("retained real resource close finishes")
            .unwrap();
            case.close_joined = true;
            let joined: serde_json::Value =
                serde_json::from_slice(&fs::read(root.join("OWNER-JOINED.json")).unwrap()).unwrap();
            assert_eq!(joined["workers_alive"], 0);
            assert_eq!(joined["physical_entries"], 0);
            assert_eq!(joined["closing"], true);
            let request: serde_json::Value =
                serde_json::from_slice(&fs::read(root.join("CENSUS-REQUEST.json")).unwrap())
                    .unwrap();
            let entry = request["entry"].as_str().unwrap();
            let records = root.join(format!("shared-entries-{identity}"));
            let actual_request: serde_json::Value = serde_json::from_slice(
                &fs::read(records.join(format!("{entry}.request.json"))).unwrap(),
            )
            .unwrap();
            assert_eq!(actual_request, request);
            let decision: serde_json::Value = serde_json::from_slice(
                &fs::read(records.join(format!("{entry}.decision.json"))).unwrap(),
            )
            .unwrap();
            assert_eq!(
                decision["decision"],
                if mode == "entered" {
                    "entered"
                } else {
                    "not_entered"
                }
            );
            if mode == "not_entered" {
                let artifact =
                    disposition.expect("exact real joins produce the physical constituent");
                let bytes = fs::read(&artifact.artifact().path).unwrap();
                assert_eq!(digest(&bytes), artifact.artifact().sha256);
                let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(value["scope"], "joined_shared_resource_not_entered_only");
                assert_eq!(
                    value["member"],
                    serde_json::to_value(&case.registrations[0].member).unwrap()
                );
                assert_eq!(value["session"], case.registrations[0].session.to_string());
                assert_eq!(value["peer"], 0);
                assert_eq!(value["lifetime_sha256"], digest(&lifetime_before));
                assert_eq!(fs::read(lifetime_path).unwrap(), lifetime_before);
            } else {
                assert!(
                    disposition.is_none(),
                    "entered, missing identity or unclaimed native registration stays unresolved"
                );
                assert!(
                    !fs::read_dir(&root)
                        .unwrap()
                        .filter_map(Result::ok)
                        .any(|row| row
                            .file_name()
                            .to_string_lossy()
                            .starts_with("shared-abandoned-"))
                );
            }
        })
        .catch_unwind()
        .await;
        complete(&mut case, checked).await;
    }
}
