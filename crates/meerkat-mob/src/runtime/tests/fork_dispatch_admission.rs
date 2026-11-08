//! The tool dispatch admission of fork-derived members, and of every rebuild
//! of a member. A host's admission (its approval gate) is process-local: no
//! fork copies one. A fork of a member whose build runs under one asks the
//! host's spawn customizer for the child and runs under what it derives, or
//! is refused before it is seated; every later build of such a child
//! (a reload's warm revival, a restore after a restart, a wake, an explicit
//! resume, a respawn successor) asks again and is refused when the host
//! derives none. An ordinary member's rebuild runs under the admission the
//! customizer's resume answer carries, and so does a rebuild of a fork child
//! whose spawn row carries no lineage marker (one seated before spawn rows
//! recorded it), when the customizer installs one. These rows run on the
//! mock durable session service, which records the exact admission each
//! build carries, and drive a call through it to see which gates it passes.

use super::*;

const STEP: Duration = Duration::from_mins(1);

/// Every admission call and outcome of a row's gates, in order:
/// `<gate>:admit:<call id>` and `<gate>:<outcome>:<call id>`.
type AdmissionLog = Arc<std::sync::Mutex<Vec<String>>>;

/// An approval gate that admits every call and logs it.
struct LoggingGate {
    name: &'static str,
    log: AdmissionLog,
}

impl LoggingGate {
    fn new(name: &'static str, log: &AdmissionLog) -> Arc<dyn meerkat_core::ToolDispatchAdmission> {
        Arc::new(Self {
            name,
            log: Arc::clone(log),
        })
    }
}

#[async_trait]
impl meerkat_core::ToolDispatchAdmission for LoggingGate {
    async fn await_dispatch_admission(
        &self,
        call: ToolCallView<'_>,
        _context: Option<&meerkat_core::ToolDispatchContext>,
        _effect_kind: meerkat_core::LiveBridgeEffectKind,
    ) -> Result<(), ToolError> {
        self.log
            .lock()
            .expect("admission log")
            .push(format!("{}:admit:{}", self.name, call.id));
        Ok(())
    }

    async fn record_dispatch_outcome(
        &self,
        call: ToolCallView<'_>,
        _context: Option<&meerkat_core::ToolDispatchContext>,
        _effect_kind: meerkat_core::LiveBridgeEffectKind,
        outcome: meerkat_core::LiveBridgeEffectOutcome,
    ) -> Result<(), ToolError> {
        self.log
            .lock()
            .expect("admission log")
            .push(format!("{}:{outcome:?}:{}", self.name, call.id));
        Ok(())
    }
}

/// The host's spawn customizer: installs its gate (`host`) on every spawn
/// and rebuild it is asked about, except for the identities it leaves
/// ungated, and records each request it is asked.
struct GatingCustomizer {
    gate: Arc<dyn meerkat_core::ToolDispatchAdmission>,
    ungated: std::sync::Mutex<BTreeSet<String>>,
    asked: std::sync::Mutex<Vec<(SpawnSource, String)>>,
}

impl GatingCustomizer {
    fn new(log: &AdmissionLog) -> Arc<Self> {
        Arc::new(Self {
            gate: LoggingGate::new("host", log),
            ungated: std::sync::Mutex::new(BTreeSet::new()),
            asked: std::sync::Mutex::new(Vec::new()),
        })
    }

    /// From now on, install nothing for `identity`.
    fn leave_ungated(&self, identity: &AgentIdentity) {
        self.ungated
            .lock()
            .expect("ungated identities")
            .insert(identity.to_string());
    }

    /// The sources of the requests this customizer was asked for `identity`.
    fn asked_for(&self, identity: &AgentIdentity) -> Vec<SpawnSource> {
        self.asked
            .lock()
            .expect("asked requests")
            .iter()
            .filter(|(_, asked)| asked == identity.as_str())
            .map(|(source, _)| *source)
            .collect()
    }
}

impl SpawnMemberCustomizer for GatingCustomizer {
    fn customize_spawn(
        &self,
        ctx: &SpawnCustomizationContext,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError> {
        self.asked
            .lock()
            .expect("asked requests")
            .push((ctx.spawn_source, spec.identity.to_string()));
        if !self
            .ungated
            .lock()
            .expect("ungated identities")
            .contains(spec.identity.as_str())
        {
            spec.tool_dispatch_admission = Some(Arc::clone(&self.gate));
        }
        Ok(())
    }
}

/// A running mob over the mock durable session service whose host gates its
/// members through its spawn customizer.
struct GatedMob {
    handle: MobHandle,
    service: Arc<MockSessionService>,
    customizer: Arc<GatingCustomizer>,
    log: AdmissionLog,
    storage: MobStorage,
    /// The mob's stored event log, to rewrite a row as an older build wrote
    /// it.
    event_log: crate::store::InMemoryEventLogForTest,
}

impl GatedMob {
    async fn new(label: &str) -> Self {
        let service = Arc::new(MockSessionService::new());
        let _ = service.enable_runtime_adapter();
        let (storage, event_log) = MobStorage::in_memory_with_event_log_for_test();
        let log = AdmissionLog::default();
        let customizer = GatingCustomizer::new(&log);
        let handle = MobBuilder::new(
            with_unique_mob_id(sample_definition(), label),
            storage.clone(),
        )
        .with_session_service(service.clone())
        .with_spawn_member_customizer(customizer.clone())
        .create()
        .await
        .expect("create the mob");
        Self {
            handle,
            service,
            customizer,
            log,
            storage,
            event_log,
        }
    }

    /// Spawn `member`, turn-driven and settled (a fork source), and answer
    /// its session.
    async fn spawn_member(&self, member: &AgentIdentity) -> SessionId {
        spawn_settled_fork_source(&self.handle, member)
            .await
            .bridge_session_id()
            .expect("the member's session")
            .clone()
    }

    /// Drop every member's live session and the mob's actor, as a process
    /// that dies leaves them (stopping the mob first when `stop_first`),
    /// then resume the mob over the same durable state with `customizer`,
    /// as the restarted host composes it.
    async fn crash_and_resume(
        self,
        stop_first: bool,
        customizer: Arc<GatingCustomizer>,
        policy: Option<MemberRestorePolicy>,
    ) -> (MobHandle, Arc<MockSessionService>) {
        self.crash_and_resume_with(stop_first, Some(customizer), policy)
            .await
    }

    /// [`Self::crash_and_resume`] for a restarted host that composes
    /// `customizer`, or no spawn customizer at all.
    async fn crash_and_resume_with(
        self,
        stop_first: bool,
        customizer: Option<Arc<GatingCustomizer>>,
        policy: Option<MemberRestorePolicy>,
    ) -> (MobHandle, Arc<MockSessionService>) {
        let sessions = self
            .handle
            .list_all_members()
            .await
            .into_iter()
            .filter_map(|entry| entry.member_ref.bridge_session_id().cloned())
            .collect::<Vec<_>>();
        if stop_first {
            self.handle.stop().await.expect("stop the mob");
        }
        for session in &sessions {
            MobSessionService::discard_live_session(self.service.as_ref(), session)
                .await
                .expect("discard a live session");
        }
        crash_stop_and_release_routes(self.handle).await;
        let mut builder = MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
            self.storage.events.clone(),
            self.storage.runtime_metadata.clone(),
        ))
        .with_session_service(self.service.clone());
        if let Some(customizer) = customizer {
            builder = builder.with_spawn_member_customizer(customizer);
        }
        if let Some(policy) = policy {
            builder = builder.with_member_restore_policy(policy);
        }
        let handle = tokio::time::timeout(STEP, Box::pin(builder.resume()))
            .await
            .expect("the restart answers")
            .expect("the restart succeeds");
        (handle, self.service)
    }
}

fn worker_spec(member: &AgentIdentity) -> SpawnMemberSpec {
    let mut spec = SpawnMemberSpec::new(ProfileName::from("worker"), member.clone());
    spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    spec
}

/// The admission the last build of `session` carried, if any.
async fn last_admission(
    service: &MockSessionService,
    session: &SessionId,
) -> Option<Arc<dyn meerkat_core::ToolDispatchAdmission>> {
    service
        .recorded_dispatch_admissions()
        .await
        .into_iter()
        .rev()
        .find(|(built, _)| built == session)
        .and_then(|(_, admission)| admission)
}

/// Drive one call `call_id` through `admission` and settle it committed.
async fn drive_call(admission: &Arc<dyn meerkat_core::ToolDispatchAdmission>, call_id: &str) {
    let args = serde_json::value::RawValue::from_string("{}".to_string()).expect("call args");
    let call = ToolCallView {
        id: call_id,
        name: "host_tool",
        args: &args,
    };
    admission
        .await_dispatch_admission(call, None, meerkat_core::LiveBridgeEffectKind::ToolDispatch)
        .await
        .expect("the admission admits the call");
    admission
        .record_dispatch_outcome(
            call,
            None,
            meerkat_core::LiveBridgeEffectKind::ToolDispatch,
            meerkat_core::LiveBridgeEffectOutcome::Committed,
        )
        .await
        .expect("the admission settles the call");
}

/// The last build of `session` runs under the host's gate: a call driven
/// through its admission passes the gate logged in `log`.
async fn assert_built_under_the_host_gate(
    service: &MockSessionService,
    session: &SessionId,
    log: &AdmissionLog,
    call_id: &str,
) {
    let admission = last_admission(service, session)
        .await
        .unwrap_or_else(|| panic!("the build of {session} carries a dispatch admission"));
    drive_call(&admission, call_id).await;
    let logged = log.lock().expect("admission log").clone();
    assert!(
        logged.contains(&format!("host:admit:{call_id}")),
        "the call passed the host's gate: {logged:?}"
    );
}

async fn lineage(handle: &MobHandle, member: &AgentIdentity) -> bool {
    handle
        .get_member(member)
        .await
        .expect("read the member")
        .unwrap_or_else(|| panic!("{member} is seated"))
        .dispatch_admission_lineage
}

/// Seat a gated source and a fork child of it; answer the child's session.
async fn gated_source_and_child(
    mob: &GatedMob,
    source: &AgentIdentity,
    child: &AgentIdentity,
) -> SessionId {
    mob.spawn_member(source).await;
    mob.handle
        .fork_member(source, worker_spec(child), None)
        .await
        .expect("fork the child")
        .session_id
}

/// The contributor's case on the handle's API: a fork of a member the host
/// gates asks the host's customizer for the child at its seat, and the
/// child runs under the admission it derives. The child's spawn row records
/// its lineage.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fork_of_a_gated_member_runs_under_the_admission_the_host_derives_for_it() {
    let mob = GatedMob::new("fork-admission-seat").await;
    let source = AgentIdentity::from("gated-source");
    let child = AgentIdentity::from("gated-child");
    let source_session = mob.spawn_member(&source).await;
    assert!(
        last_admission(&mob.service, &source_session)
            .await
            .is_some(),
        "the host gates the source"
    );
    let child_session = gated_source_and_child_after_source(&mob, &source, &child).await;
    assert_eq!(
        mob.customizer.asked_for(&child),
        [SpawnSource::PersistedForkResume],
        "the host is asked for the child once, at its seat"
    );
    assert_built_under_the_host_gate(&mob.service, &child_session, &mob.log, "child-call").await;
    assert!(
        lineage(&mob.handle, &child).await,
        "the child's spawn row records that its lineage runs under an admission"
    );
}

/// [`gated_source_and_child`] for a source already spawned.
async fn gated_source_and_child_after_source(
    mob: &GatedMob,
    source: &AgentIdentity,
    child: &AgentIdentity,
) -> SessionId {
    mob.handle
        .fork_member(source, worker_spec(child), None)
        .await
        .expect("fork the child")
        .session_id
}

/// A fork of a gated member that the host's customizer derives no
/// admission for is refused with the typed refusal before anything is
/// forked: no child session, no seat, no build.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fork_the_host_derives_no_admission_for_is_refused_before_anything_is_forked() {
    let mob = GatedMob::new("fork-admission-refused").await;
    let source = AgentIdentity::from("gated-source");
    let child = AgentIdentity::from("ungated-child");
    mob.spawn_member(&source).await;
    mob.customizer.leave_ungated(&child);
    let sessions_before = mob.service.persisted_sessions.read().await.len();
    let error = mob
        .handle
        .fork_member(&source, worker_spec(&child), None)
        .await
        .expect_err("the fork is refused");
    assert!(
        matches!(&error, MobError::DispatchAdmissionRequired { member_id } if member_id == &child),
        "the refusal is typed: {error}"
    );
    assert_eq!(
        error.structured_data(),
        Some(serde_json::json!({
            "kind": "dispatch_admission_required",
            "member_id": child.as_str(),
        })),
        "a surface reads the refusal's kind"
    );
    assert_eq!(
        mob.customizer.asked_for(&child),
        [SpawnSource::PersistedForkResume],
        "the host was asked for the child"
    );
    assert!(
        mob.handle
            .get_member(&child)
            .await
            .expect("read the child")
            .is_none(),
        "nothing is seated"
    );
    assert_eq!(
        member_build_count(&mob.service, &child).await,
        0,
        "nothing is built"
    );
    assert_eq!(
        mob.service.persisted_sessions.read().await.len(),
        sessions_before,
        "nothing is forked"
    );
}

/// A fork of a member the host does not gate needs no admission: the host's
/// customizer is not asked for the child, and the child's build carries
/// none, as before.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fork_of_an_ungated_member_asks_the_host_nothing() {
    let mob = GatedMob::new("fork-admission-ungated").await;
    let source = AgentIdentity::from("ungated-source");
    let child = AgentIdentity::from("ungated-source-child");
    mob.customizer.leave_ungated(&source);
    let source_session = mob.spawn_member(&source).await;
    assert!(
        last_admission(&mob.service, &source_session)
            .await
            .is_none()
    );
    let child_session = gated_source_and_child_after_source(&mob, &source, &child).await;
    assert!(
        mob.customizer.asked_for(&child).is_empty(),
        "the host is not asked for the child of an ungated member"
    );
    assert!(
        last_admission(&mob.service, &child_session).await.is_none(),
        "the child's build carries no admission"
    );
    assert!(!lineage(&mob.handle, &child).await);
}

/// `fork_member_then_run_bounded` and `fork_member_then_run_detached` seat
/// their children the same way: each runs under the host's admission.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bounded_and_detached_forks_of_a_gated_member_run_under_the_hosts_admission() {
    let mob = GatedMob::new("fork-admission-bounded").await;
    mob.service.set_return_exact_run_result(true);
    let source = AgentIdentity::from("gated-source");
    mob.spawn_member(&source).await;

    let bounded = AgentIdentity::from("bounded-child");
    let outcome = mob
        .handle
        .fork_member_then_run_bounded(
            &source,
            bounded_fork_child_spec(&bounded),
            None,
            "fork_child_result",
            256,
            meerkat_core::DurableForkSourceAdmission::Quiescent,
        )
        .await
        .expect("the bounded fork runs");
    assert_built_under_the_host_gate(
        &mob.service,
        &outcome.fork.session_id,
        &mob.log,
        "bounded-call",
    )
    .await;
    assert!(lineage(&mob.handle, &bounded).await);

    let detached = AgentIdentity::from("detached-child");
    let (fork, _run) = mob
        .handle
        .fork_member_then_run_detached(
            &source,
            bounded_fork_child_spec(&detached),
            None,
            "fork_child_result",
            256,
            meerkat_core::DurableForkSourceAdmission::Quiescent,
            None,
            None,
        )
        .await
        .expect("the detached fork starts");
    assert_built_under_the_host_gate(&mob.service, &fork.session_id, &mob.log, "detached-call")
        .await;
    assert!(lineage(&mob.handle, &detached).await);
}

/// A live delegation's fork carries the delegation's own gate on its
/// request: the child runs under both, the delegation's first and then the
/// host's, and both hear each call's outcome.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_live_delegation_fork_runs_under_its_own_gate_and_the_hosts() {
    let mob = GatedMob::new("fork-admission-live").await;
    let source = AgentIdentity::from("gated-source");
    mob.spawn_member(&source).await;
    let child = AgentIdentity::from("live-worker");
    let mut spec = worker_spec(&child);
    spec.tool_dispatch_admission = Some(LoggingGate::new("live", &mob.log));
    let outcome = mob
        .handle
        .fork_member_at_turn_boundary(&source, spec, None, Duration::from_secs(5))
        .await
        .expect("the live fork answers");
    let ForkMemberAtTurnBoundary::Forked(fork) = outcome else {
        panic!("an idle source forks at once")
    };
    let admission = last_admission(&mob.service, &fork.session_id)
        .await
        .expect("the worker's build carries a dispatch admission");
    drive_call(&admission, "live-call").await;
    assert_eq!(
        *mob.log.lock().expect("admission log"),
        [
            "live:admit:live-call",
            "host:admit:live-call",
            "live:Committed:live-call",
            "host:Committed:live-call",
        ],
        "the delegation's gate and the host's both admit and both settle the call"
    );
    assert!(lineage(&mob.handle, &child).await);
}

/// A reload's warm revival of a gated fork child rebuilds it under the
/// admission the host derives for its own identity now; once the host
/// derives none for it, the revival is refused and builds nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_revived_fork_child_runs_under_the_hosts_admission_or_is_not_revived() {
    let mob = GatedMob::new("fork-admission-revival").await;
    let source = AgentIdentity::from("gated-source");
    let child = AgentIdentity::from("gated-child");
    let child_session = gated_source_and_child(&mob, &source, &child).await;
    let builds = member_build_count(&mob.service, &child).await;

    MobSessionService::discard_live_session(mob.service.as_ref(), &child_session)
        .await
        .expect("discard the child's live session");
    mob.handle
        .member(&child)
        .await
        .expect("the child's handle")
        .internal_turn(ContentInput::from("come back online".to_string()))
        .await
        .expect("the warm revival rebuilds the child");
    assert_eq!(member_build_count(&mob.service, &child).await, builds + 1);
    assert!(
        mob.customizer
            .asked_for(&child)
            .contains(&SpawnSource::Resume),
        "the host is asked for the child's own identity: {:?}",
        mob.customizer.asked_for(&child)
    );
    assert_built_under_the_host_gate(&mob.service, &child_session, &mob.log, "revived-call").await;

    mob.customizer.leave_ungated(&child);
    MobSessionService::discard_live_session(mob.service.as_ref(), &child_session)
        .await
        .expect("discard the child's live session again");
    let refused = mob
        .handle
        .member(&child)
        .await
        .expect("the child's handle")
        .internal_turn(ContentInput::from("come back once more".to_string()))
        .await;
    assert!(
        refused.is_err(),
        "a child the host derives no admission for is not revived"
    );
    assert_eq!(
        member_build_count(&mob.service, &child).await,
        builds + 1,
        "nothing is built without an admission"
    );
}

/// The restart row: a fork child of a gated member, the process restarted,
/// and the child's restore (an eager start) runs under the admission the
/// restarted host derives for it, since its spawn row records its lineage.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fork_child_restored_after_a_restart_is_still_gated() {
    let mob = GatedMob::new("fork-admission-restart").await;
    let source = AgentIdentity::from("zz-gated-source");
    let child = AgentIdentity::from("aa-gated-child");
    let child_session = gated_source_and_child(&mob, &source, &child).await;
    let builds = member_build_count(&mob.service, &child).await;
    let log = AdmissionLog::default();
    let restarted_host = GatingCustomizer::new(&log);
    let (resumed, service) = mob
        .crash_and_resume(
            false,
            restarted_host.clone(),
            Some(MemberRestorePolicy::Eager),
        )
        .await;
    assert_eq!(
        member_build_count(&service, &child).await,
        builds + 1,
        "the child is restored"
    );
    assert!(lineage(&resumed, &child).await, "replay keeps the lineage");
    assert!(
        restarted_host
            .asked_for(&child)
            .contains(&SpawnSource::Resume),
        "the restarted host is asked for the child"
    );
    assert_built_under_the_host_gate(&service, &child_session, &log, "restored-call").await;
}

/// The same child, woken after a lazy restart: the wake rebuilds it under
/// the restarted host's admission.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fork_child_woken_after_a_restart_is_still_gated() {
    let mob = GatedMob::new("fork-admission-wake").await;
    let source = AgentIdentity::from("gated-source");
    let child = AgentIdentity::from("gated-child");
    let child_session = gated_source_and_child(&mob, &source, &child).await;
    let builds = member_build_count(&mob.service, &child).await;
    let log = AdmissionLog::default();
    let restarted_host = GatingCustomizer::new(&log);
    let (resumed, service) = mob
        .crash_and_resume(false, restarted_host.clone(), None)
        .await;
    tokio::time::timeout(STEP, resumed.ensure_member_live(&child))
        .await
        .expect("the wake answers")
        .expect("the wake rebuilds the child");
    assert_eq!(member_build_count(&service, &child).await, builds + 1);
    assert_built_under_the_host_gate(&service, &child_session, &log, "woken-call").await;
}

/// A restarted host that derives no admission for a gated fork child
/// leaves it unbuilt: the restore fails with the typed refusal as its
/// diagnostic, and the wake fails. The child never runs ungated.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restarted_host_that_derives_no_admission_leaves_the_fork_child_unbuilt() {
    let mob = GatedMob::new("fork-admission-restart-refused").await;
    let source = AgentIdentity::from("zz-gated-source");
    let child = AgentIdentity::from("aa-gated-child");
    gated_source_and_child(&mob, &source, &child).await;
    let builds = member_build_count(&mob.service, &child).await;
    let restarted_host = GatingCustomizer::new(&AdmissionLog::default());
    restarted_host.leave_ungated(&child);
    let (resumed, service) = mob
        .crash_and_resume(false, restarted_host, Some(MemberRestorePolicy::Eager))
        .await;
    let diagnostic = resumed
        .restore_diagnostics
        .read()
        .await
        .get(&child)
        .cloned()
        .expect("the child's restore failed");
    assert!(
        diagnostic.reason.contains("tool dispatch admission"),
        "the diagnostic names the refusal: {}",
        diagnostic.reason
    );
    let woken = tokio::time::timeout(STEP, resumed.ensure_member_live(&child))
        .await
        .expect("the wake answers");
    assert!(woken.is_err(), "the child is not woken ungated");
    assert_eq!(
        member_build_count(&service, &child).await,
        builds,
        "nothing is built without an admission"
    );
}

/// An explicit resume of a stopped mob after a restart rebuilds a gated
/// fork child under the restarted host's admission.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fork_child_rebuilt_by_an_explicit_resume_is_still_gated() {
    let mob = GatedMob::new("fork-admission-explicit").await;
    let source = AgentIdentity::from("zz-gated-source");
    let child = AgentIdentity::from("aa-gated-child");
    let child_session = gated_source_and_child(&mob, &source, &child).await;
    let builds = member_build_count(&mob.service, &child).await;
    let log = AdmissionLog::default();
    let (resumed, service) = mob
        .crash_and_resume(
            true,
            GatingCustomizer::new(&log),
            Some(MemberRestorePolicy::Eager),
        )
        .await;
    tokio::time::timeout(STEP, resumed.resume())
        .await
        .expect("the explicit resume answers")
        .expect("the explicit resume rebuilds the stopped members");
    assert_eq!(member_build_count(&service, &child).await, builds + 1);
    assert_built_under_the_host_gate(&service, &child_session, &log, "explicit-call").await;
}

/// The same rows for an ordinary member: a reload's warm revival rebuilds
/// it under the admission the host's customizer gives its rebuild, not
/// without one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_ordinary_member_revived_by_a_reload_keeps_the_hosts_admission() {
    let mob = GatedMob::new("ordinary-admission-revival").await;
    let member = AgentIdentity::from("ordinary-member");
    let session = mob.spawn_member(&member).await;
    let builds = member_build_count(&mob.service, &member).await;
    MobSessionService::discard_live_session(mob.service.as_ref(), &session)
        .await
        .expect("discard the member's live session");
    mob.handle
        .member(&member)
        .await
        .expect("the member's handle")
        .internal_turn(ContentInput::from("come back online".to_string()))
        .await
        .expect("the warm revival rebuilds the member");
    assert_eq!(member_build_count(&mob.service, &member).await, builds + 1);
    assert_built_under_the_host_gate(&mob.service, &session, &mob.log, "revived-call").await;
}

/// An explicit resume of a stopped mob after a restart rebuilds an ordinary
/// member under the admission the restarted host's customizer gives it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_ordinary_member_rebuilt_by_an_explicit_resume_keeps_the_hosts_admission() {
    let mob = GatedMob::new("ordinary-admission-explicit").await;
    let member = AgentIdentity::from("ordinary-member");
    let session = mob.spawn_member(&member).await;
    let builds = member_build_count(&mob.service, &member).await;
    let log = AdmissionLog::default();
    let (resumed, service) = mob
        .crash_and_resume(
            true,
            GatingCustomizer::new(&log),
            Some(MemberRestorePolicy::Eager),
        )
        .await;
    tokio::time::timeout(STEP, resumed.resume())
        .await
        .expect("the explicit resume answers")
        .expect("the explicit resume rebuilds the stopped members");
    assert_eq!(member_build_count(&service, &member).await, builds + 1);
    assert_built_under_the_host_gate(&service, &session, &log, "explicit-call").await;
}

/// A respawn successor of a gated fork child keeps its lineage: the host's
/// customizer is asked for it and it runs under what it derives; a respawn
/// the host derives none for is refused before the child is retired.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_respawned_fork_child_keeps_its_lineage_or_is_not_respawned() {
    let mob = GatedMob::new("fork-admission-respawn").await;
    let source = AgentIdentity::from("gated-source");
    let child = AgentIdentity::from("gated-child");
    gated_source_and_child(&mob, &source, &child).await;

    mob.handle
        .respawn(child.clone(), None)
        .await
        .expect("respawn the child");
    assert!(
        mob.customizer
            .asked_for(&child)
            .contains(&SpawnSource::Respawn),
        "the host is asked for the successor"
    );
    let successor = last_member_build(&mob.service, &child).await;
    assert_built_under_the_host_gate(
        &mob.service,
        &successor.session_id,
        &mob.log,
        "successor-call",
    )
    .await;
    assert!(
        lineage(&mob.handle, &child).await,
        "the successor's spawn row keeps the lineage"
    );

    let generation = mob
        .handle
        .get_member(&child)
        .await
        .expect("read the successor")
        .expect("the successor is seated")
        .generation;
    mob.customizer.leave_ungated(&child);
    let refused = mob.handle.respawn(child.clone(), None).await;
    assert!(
        matches!(
            &refused,
            Err(MobRespawnError::Mob(MobError::DispatchAdmissionRequired { member_id }))
                if member_id == &child
        ),
        "the respawn is refused, typed: {refused:?}"
    );
    assert_eq!(
        mob.handle
            .get_member(&child)
            .await
            .expect("read the child")
            .expect("the child is still seated")
            .generation,
        generation,
        "nothing was retired"
    );
}

/// A prompt-context fork of a gated member (a spawn whose launch mode forks
/// the member's history, as `fork_helper` spawns) is asked of the host as
/// any spawn is: it runs under the admission the customizer installs, and
/// its spawn row records its lineage; when the customizer installs none for
/// it, it is refused before anything is seated.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_prompt_context_fork_of_a_gated_member_runs_under_the_hosts_admission_or_is_refused() {
    let mob = GatedMob::new("fork-admission-prompt-fork").await;
    let source = AgentIdentity::from("gated-source");
    mob.spawn_member(&source).await;
    let fork_of_source = |member: &AgentIdentity| {
        worker_spec(member).with_launch_mode(crate::MemberLaunchMode::Fork {
            source_member_id: source.clone(),
            fork_context: crate::ForkContext::FullHistory,
        })
    };

    let child = AgentIdentity::from("prompt-fork-child");
    mob.handle
        .spawn_spec(fork_of_source(&child))
        .await
        .expect("spawn the prompt-context fork");
    assert_eq!(mob.customizer.asked_for(&child), [SpawnSource::Fork]);
    let built = last_member_build(&mob.service, &child).await;
    assert_built_under_the_host_gate(&mob.service, &built.session_id, &mob.log, "prompt-call")
        .await;
    assert!(lineage(&mob.handle, &child).await);

    let refused_child = AgentIdentity::from("ungated-prompt-fork-child");
    mob.customizer.leave_ungated(&refused_child);
    let error = mob
        .handle
        .spawn_spec(fork_of_source(&refused_child))
        .await
        .expect_err("the prompt-context fork is refused");
    assert!(
        matches!(&error, MobError::DispatchAdmissionRequired { member_id } if member_id == &refused_child),
        "the refusal is typed: {error}"
    );
    assert!(
        mob.handle
            .get_member(&refused_child)
            .await
            .expect("read the child")
            .is_none(),
        "nothing is seated"
    );
    assert_eq!(member_build_count(&mob.service, &refused_child).await, 0);
}

/// A fork of a gated member this process has not built (a lazy restart
/// left it Dormant) follows the host's answer for the member's next build,
/// without waking it: the child runs under the host's admission.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fork_of_a_dormant_gated_member_runs_under_the_hosts_admission() {
    let mob = GatedMob::new("fork-admission-dormant-source").await;
    let source = AgentIdentity::from("gated-source");
    mob.spawn_member(&source).await;
    let log = AdmissionLog::default();
    let restarted_host = GatingCustomizer::new(&log);
    let (resumed, service) = mob
        .crash_and_resume(false, restarted_host.clone(), None)
        .await;
    let source_builds = member_build_count(&service, &source).await;
    let child = AgentIdentity::from("dormant-source-child");
    let fork = resumed
        .fork_member(&source, worker_spec(&child), None)
        .await
        .expect("fork the dormant source");
    assert_eq!(
        member_build_count(&service, &source).await,
        source_builds,
        "the source is not woken"
    );
    assert_eq!(
        restarted_host.asked_for(&child),
        [SpawnSource::PersistedForkResume]
    );
    assert_built_under_the_host_gate(&service, &fork.session_id, &log, "dormant-child-call").await;
    assert!(lineage(&resumed, &child).await);
}

/// Seat a gated source and a fork child of it, then drop the dispatch
/// admission lineage marker from the child's spawn row, as a fork child
/// seated before spawn rows recorded it reads back. Answers the child's
/// session.
async fn unmarked_fork_child(
    mob: &GatedMob,
    source: &AgentIdentity,
    child: &AgentIdentity,
) -> SessionId {
    let child_session = gated_source_and_child(mob, source, child).await;
    assert_eq!(
        mob.event_log.strip_dispatch_admission_lineage(child).await,
        1,
        "the child's spawn row carried the marker"
    );
    child_session
}

/// The upgrade row: a fork child whose spawn row carries no lineage marker
/// (seated before spawn rows recorded one) is not taken to run under none.
/// Each rebuild of it asks the host's customizer for its own identity and
/// runs under what it installs, as a fresh spawn would: here, the restore
/// after a restart and a reload's warm revival.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fork_child_seated_before_lineage_records_is_gated_when_it_is_rebuilt() {
    let mob = GatedMob::new("fork-admission-unmarked-rebuild").await;
    let source = AgentIdentity::from("zz-gated-source");
    let child = AgentIdentity::from("aa-gated-child");
    let child_session = unmarked_fork_child(&mob, &source, &child).await;
    let builds = member_build_count(&mob.service, &child).await;
    let log = AdmissionLog::default();
    let restarted_host = GatingCustomizer::new(&log);
    let (resumed, service) = mob
        .crash_and_resume(
            false,
            restarted_host.clone(),
            Some(MemberRestorePolicy::Eager),
        )
        .await;
    assert!(
        !lineage(&resumed, &child).await,
        "the replayed spawn row records no lineage"
    );
    assert_eq!(
        member_build_count(&service, &child).await,
        builds + 1,
        "the child is restored"
    );
    assert!(
        restarted_host
            .asked_for(&child)
            .contains(&SpawnSource::Resume),
        "the restarted host is asked for the child: {:?}",
        restarted_host.asked_for(&child)
    );
    assert_built_under_the_host_gate(&service, &child_session, &log, "restored-call").await;

    MobSessionService::discard_live_session(service.as_ref(), &child_session)
        .await
        .expect("discard the child's live session");
    resumed
        .member(&child)
        .await
        .expect("the child's handle")
        .internal_turn(ContentInput::from("come back online".to_string()))
        .await
        .expect("the warm revival rebuilds the child");
    assert_eq!(member_build_count(&service, &child).await, builds + 2);
    assert_built_under_the_host_gate(&service, &child_session, &log, "revived-call").await;
}

/// The same unmarked child as a fork source, before this process builds it
/// (a fork the `fork_off` tool takes goes through the same seat): its next
/// build would ask the host for its own identity, so the fork's child is
/// seated from a lineage that runs under an admission and runs under the
/// host's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fork_of_a_fork_child_seated_before_lineage_records_runs_under_the_hosts_admission() {
    let mob = GatedMob::new("fork-admission-unmarked-source").await;
    let source = AgentIdentity::from("gated-source");
    let child = AgentIdentity::from("gated-child");
    unmarked_fork_child(&mob, &source, &child).await;
    let log = AdmissionLog::default();
    let restarted_host = GatingCustomizer::new(&log);
    let (resumed, service) = mob
        .crash_and_resume(false, restarted_host.clone(), None)
        .await;
    let child_builds = member_build_count(&service, &child).await;
    let grandchild = AgentIdentity::from("unmarked-child-fork");
    let fork = resumed
        .fork_member(&child, worker_spec(&grandchild), None)
        .await
        .expect("fork the unmarked child");
    assert_eq!(
        member_build_count(&service, &child).await,
        child_builds,
        "the unmarked child is not woken"
    );
    assert_eq!(
        restarted_host.asked_for(&grandchild),
        [SpawnSource::PersistedForkResume],
        "the host is asked for the fork's child at its seat"
    );
    assert_built_under_the_host_gate(&service, &fork.session_id, &log, "grandchild-call").await;
    assert!(lineage(&resumed, &grandchild).await);
}

/// A host whose customizer installs no admission for the same unmarked
/// child, or a host without a spawn customizer, rebuilds it as before:
/// nothing refuses it, and it runs under no admission.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_that_derives_no_admission_rebuilds_a_fork_child_seated_before_lineage_records_as_before()
 {
    for with_customizer in [true, false] {
        let mob = GatedMob::new("fork-admission-unmarked-ungated").await;
        let source = AgentIdentity::from("zz-gated-source");
        let child = AgentIdentity::from("aa-gated-child");
        let child_session = unmarked_fork_child(&mob, &source, &child).await;
        let builds = member_build_count(&mob.service, &child).await;
        let restarted_host = with_customizer.then(|| {
            let host = GatingCustomizer::new(&AdmissionLog::default());
            host.leave_ungated(&child);
            host
        });
        let (resumed, service) = mob
            .crash_and_resume_with(false, restarted_host, Some(MemberRestorePolicy::Eager))
            .await;
        assert_eq!(
            member_build_count(&service, &child).await,
            builds + 1,
            "the child is restored (customizer: {with_customizer})"
        );
        assert!(
            resumed
                .restore_diagnostics
                .read()
                .await
                .get(&child)
                .is_none(),
            "nothing refuses the child (customizer: {with_customizer})"
        );
        assert!(
            last_admission(&service, &child_session).await.is_none(),
            "the child runs under no admission, as before (customizer: {with_customizer})"
        );
    }
}

/// A policy auto-spawn of a host that gates its members through its spawn
/// customizer: a `SubmitWork` to an identity no member holds seats it from
/// the spawn policy, and the customizer is asked for its tool dispatch
/// admission (`SpawnSource::PolicySpawn`). The member runs under what it
/// installs, and so does a fork of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_policy_auto_spawn_runs_under_the_hosts_admission_and_so_does_its_fork() {
    let mob = GatedMob::new("policy-spawn-admission").await;
    mob.handle
        .set_spawn_policy(Some(Arc::new(StaticLeadSpawnPolicy)))
        .await
        .expect("set the spawn policy");
    let target = AgentIdentity::from("policy-spawned");
    mob.handle
        .submit_work(
            AgentRuntimeId::initial(target.clone()),
            FenceToken::new(0),
            WorkRef::new(),
            WorkSpec::new(
                "queued for a policy spawn".to_string(),
                WorkOrigin::External,
            ),
        )
        .await
        .expect("the policy auto-spawn seats the member");
    let session = mob
        .handle
        .resolve_bridge_session_id(&target)
        .await
        .expect("the member's session");
    assert_eq!(
        mob.customizer.asked_for(&target),
        [SpawnSource::PolicySpawn],
        "the host is asked for the member once, at its policy spawn"
    );
    assert_built_under_the_host_gate(&mob.service, &session, &mob.log, "policy-call").await;

    wait_for_fork_source_settled(&mob.handle, &session).await;
    let child = AgentIdentity::from("policy-spawned-fork");
    let fork = mob
        .handle
        .fork_member(&target, worker_spec(&child), None)
        .await
        .expect("fork the policy-spawned member");
    assert_eq!(
        mob.customizer.asked_for(&child),
        [SpawnSource::PersistedForkResume],
        "the host is asked for the fork's child at its seat"
    );
    assert_built_under_the_host_gate(&mob.service, &fork.session_id, &mob.log, "policy-fork-call")
        .await;
    assert!(lineage(&mob.handle, &child).await);
}
