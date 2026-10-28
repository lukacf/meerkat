//! The build inputs a dormant seat carries to its member's first build. A
//! seat builds nothing, so the first wake applies the inputs of the seat's
//! spec exactly as a spawn of that spec would. The inputs the journal holds
//! survive a restart before the first wake; the process-local ones stay in
//! this process. A session a seat named and no build created (its member was
//! retired before its first wake) is seated unborn again. These rows run on
//! the combined candidate: the machine, the wake path and the boot path
//! together.

use super::*;
use crate::runtime::DormantSeatOutcome;

const STEP: Duration = Duration::from_secs(60);

const CONTEXT_DESK: &str = "north";
const INSTRUCTION: &str = "Answer in one line.";
const PROMPT: &str = "You keep the north desk.";

/// A running mob over the mock durable session service, with session-scoped
/// member comms identities: what a restart finds is its journal, its runtime
/// metadata and the service's durable state.
struct SeatMob {
    keys: tempfile::TempDir,
    handle: MobHandle,
    events: Arc<dyn MobEventStore>,
    runtime_metadata: Arc<dyn MobRuntimeMetadataStore>,
    service: Arc<MockSessionService>,
}

impl SeatMob {
    async fn new(label: &str) -> Self {
        let keys = tempfile::tempdir().expect("a comms identity root");
        let service = Arc::new(MockSessionService::new());
        let _adapter = service.enable_runtime_adapter();
        let storage = MobStorage::in_memory();
        let events = storage.events.clone();
        let runtime_metadata = storage.runtime_metadata.clone();
        let handle = MobBuilder::new(with_unique_mob_id(sample_definition(), label), storage)
            .with_session_service(service.clone())
            .with_member_comms_identity_root(keys.path())
            .create()
            .await
            .expect("create the mob");
        Self {
            keys,
            handle,
            events,
            runtime_metadata,
            service,
        }
    }

    /// Seat `spec` and answer the session it is seated on.
    async fn seat(&self, spec: SpawnMemberSpec) -> SessionId {
        let outcomes = self
            .handle
            .seat_dormant_members(vec![spec])
            .await
            .expect("the seat batch answers");
        match &outcomes[0] {
            DormantSeatOutcome::Seated { session_id, .. } => session_id.clone(),
            other => panic!("the spec is seated: {other:?}"),
        }
    }

    /// Crash the mob, restart its session service and resume it under the
    /// default policy.
    async fn restart(&mut self) {
        crash_stop_and_release_routes(self.handle.clone()).await;
        let restarted = Arc::new(self.service.cold_restart_preserving_durable_state().await);
        let _ = restarted.enable_runtime_adapter();
        self.service = restarted;
        self.handle = tokio::time::timeout(
            STEP,
            Box::pin(
                MobBuilder::for_resume(MobStorage::with_events_and_runtime_metadata(
                    self.events.clone(),
                    self.runtime_metadata.clone(),
                ))
                .with_session_service(self.service.clone())
                .with_member_comms_identity_root(self.keys.path())
                .notify_orchestrator_on_resume(false)
                .resume(),
            ),
        )
        .await
        .expect("the restart answers")
        .expect("the restart succeeds");
    }

    /// Wake `member` and answer the one build that created `session`.
    async fn first_build(
        &self,
        member: &AgentIdentity,
        session: &SessionId,
    ) -> CreateSessionRecord {
        assert!(
            self.service.recorded_create_requests().await.is_empty(),
            "nothing was built before the first wake"
        );
        tokio::time::timeout(STEP, self.handle.ensure_member_live(member))
            .await
            .expect("the first wake answers")
            .expect("the first wake materializes the member");
        let mut creates = self
            .service
            .recorded_create_requests()
            .await
            .into_iter()
            .filter(|record| &record.session_id == session)
            .collect::<Vec<_>>();
        assert_eq!(
            creates.len(),
            1,
            "the first wake creates the seat's session once"
        );
        assert!(
            !seat_session_unborn(&self.handle, member),
            "the created session is no longer unborn"
        );
        creates.remove(0)
    }
}

fn seat_session_unborn(handle: &MobHandle, member: &AgentIdentity) -> bool {
    crate::runtime::boot_plan::domain_member_seat_session_unborn(
        &handle.machine_state_watch_rx.borrow(),
        member,
    )
}

fn worker_spec(member: &AgentIdentity) -> SpawnMemberSpec {
    let mut spec = SpawnMemberSpec::new("worker", member.clone());
    spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    spec
}

/// `spec` naming a context, instructions and a system prompt override.
fn with_prompt_inputs(mut spec: SpawnMemberSpec) -> SpawnMemberSpec {
    spec.context = Some(serde_json::json!({ "desk": CONTEXT_DESK }));
    spec.additional_instructions = Some(vec![INSTRUCTION.to_string()]);
    spec.system_prompt_override = Some(crate::runtime::SpawnSystemPromptOverride::Replace(
        PROMPT.to_string(),
    ));
    spec
}

fn assert_prompt_inputs(build: &CreateSessionRecord) {
    assert_eq!(
        build.app_context,
        Some(serde_json::json!({ "desk": CONTEXT_DESK })),
        "the first build applies the seat's context"
    );
    assert_eq!(
        build.additional_instructions,
        Some(vec![INSTRUCTION.to_string()]),
        "the first build applies the seat's instructions"
    );
    assert_eq!(
        build.system_prompt,
        meerkat_core::SystemPromptOverride::Set(PROMPT.to_string()),
        "the first build applies the seat's system prompt"
    );
}

/// Admits every dispatch: a process-local input the seat keeps.
struct AdmitEveryDispatch;

#[async_trait]
impl meerkat_core::ToolDispatchAdmission for AdmitEveryDispatch {
    async fn await_dispatch_admission(
        &self,
        _call: ToolCallView<'_>,
        _context: Option<&meerkat_core::ToolDispatchContext>,
        _effect_kind: meerkat_core::LiveBridgeEffectKind,
    ) -> Result<(), ToolError> {
        Ok(())
    }
}

fn seat_env() -> std::collections::HashMap<String, String> {
    std::collections::HashMap::from([("DESK".to_string(), CONTEXT_DESK.to_string())])
}

/// A fresh seat with instructions, a context and a system prompt override
/// builds nothing; its first wake applies all three, with the process-local
/// inputs this process kept (shell environment, compaction curator, dispatch
/// admission).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fresh_seats_first_wake_applies_its_instructions_context_and_system_prompt() {
    let mob = SeatMob::new("seat-inputs-first-wake").await;
    let member = AgentIdentity::from("desk-north");
    let curator: Arc<dyn meerkat_core::CompactionCurator> = Arc::new(TestCompactionCurator);
    let mut spec = with_prompt_inputs(worker_spec(&member));
    spec.shell_env = Some(seat_env());
    spec.compaction_curator_override = Some(Arc::clone(&curator));
    spec.tool_dispatch_admission = Some(Arc::new(AdmitEveryDispatch));
    let session = mob.seat(spec).await;
    assert!(seat_session_unborn(&mob.handle, &member));

    let build = mob.first_build(&member, &session).await;
    assert_prompt_inputs(&build);
    assert_eq!(
        build.shell_env,
        Some(seat_env()),
        "this process kept the seat's shell environment"
    );
    assert!(
        build.tool_dispatch_admission,
        "this process kept the seat's dispatch admission"
    );
    let curators = mob.service.recorded_compaction_curators().await;
    assert!(
        curators
            .last()
            .and_then(Option::as_ref)
            .is_some_and(|built| Arc::ptr_eq(built, &curator)),
        "this process kept the seat's compaction curator"
    );
}

/// The same across a restart before the first wake, for the inputs the
/// seat's journal row holds: the restarted mob's first wake of the member
/// applies its context, instructions and system prompt. The process-local
/// inputs stayed with the process that seated it; with no spawn customizer,
/// the restarted build has none.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fresh_seats_journaled_inputs_reach_its_first_wake_after_a_restart() {
    let mut mob = SeatMob::new("seat-inputs-restart").await;
    let member = AgentIdentity::from("desk-north");
    let mut spec = with_prompt_inputs(worker_spec(&member));
    spec.shell_env = Some(seat_env());
    let session = mob.seat(spec).await;

    mob.restart().await;
    assert!(
        seat_session_unborn(&mob.handle, &member),
        "the restart keeps the session unborn"
    );
    let build = mob.first_build(&member, &session).await;
    assert_prompt_inputs(&build);
    assert_eq!(
        build.shell_env, None,
        "the shell environment is never journaled"
    );
}

/// The governance inputs of a seat's spec are in force at its first build:
/// its tool access policy, application tool policy, budget limits, auth
/// binding and web search override.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fresh_seats_governance_inputs_are_in_force_at_its_first_build() {
    let mob = SeatMob::new("seat-inputs-governance").await;
    let member = AgentIdentity::from("desk-north");
    let limits = meerkat_core::BudgetLimits {
        max_tokens: Some(4096),
        ..Default::default()
    };
    let policy = meerkat_core::ApplicationToolPolicyBinding::Provider {
        provider_id: meerkat_core::PolicyProviderId::new("host").expect("provider id"),
        policy_id: meerkat_core::PolicyId::new("seat-tools").expect("policy id"),
    };
    let mut spec = worker_spec(&member);
    spec.tool_access_policy = Some(meerkat_core::ops::ToolAccessPolicy::ReadOnly);
    spec.application_tool_policy = Some(policy.clone());
    spec.budget_limits = Some(limits.clone());
    spec.auth_binding = Some(scoped_auth_binding());
    spec.tool_category_overrides.web_search = ToolCategoryOverride::Disable;
    let session = mob.seat(spec).await;

    let build = mob.first_build(&member, &session).await;
    assert_eq!(
        build.tool_access_policy,
        Some(meerkat_core::ops::ToolAccessPolicy::ReadOnly),
        "the seat's tool access policy"
    );
    assert_eq!(
        build.application_tool_policy, policy,
        "the seat's application tool policy"
    );
    assert_eq!(
        build.budget_limits,
        Some(limits),
        "the seat's budget limits"
    );
    assert_eq!(
        build.auth_binding,
        Some(scoped_auth_binding()),
        "the seat's auth binding"
    );
    assert_eq!(
        build.override_web_search,
        ToolCategoryOverride::Disable,
        "the seat's web search override"
    );
}

/// An identity retired before its first wake is seated again on the session
/// its first seat named, which no build created: the re-seat, a resume of
/// that session, seats it unborn and creates nothing; its first wake then
/// creates the session once, under its id. The first build applies the
/// first seat's inputs with what the re-seat's spec names over them: the
/// re-seat names instructions and no system prompt.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_identity_retired_before_its_first_wake_is_reseated_unborn_and_created_once() {
    let mob = SeatMob::new("seat-never-born").await;
    let member = AgentIdentity::from("never-woken");
    let session = mob.seat(with_prompt_inputs(worker_spec(&member))).await;
    tokio::time::timeout(STEP, mob.handle.retire(member.clone()))
        .await
        .expect("the retirement answers")
        .expect("the never-woken member is retired");
    assert!(
        mob.handle
            .get_member(&member)
            .await
            .expect("read the roster")
            .is_none(),
        "the retired member left the roster"
    );

    let mut reseat = worker_spec(&member);
    reseat.launch_mode = crate::launch::MemberLaunchMode::Resume {
        bridge_session_id: session.clone(),
        resume_from_role: None,
    };
    reseat.additional_instructions = Some(vec!["Answer in two lines.".to_string()]);
    assert_eq!(
        mob.seat(reseat).await,
        session,
        "the re-seat binds the session no build created"
    );
    assert!(
        seat_session_unborn(&mob.handle, &member),
        "the re-seated session is unborn"
    );

    let build = mob.first_build(&member, &session).await;
    assert_eq!(
        build.additional_instructions,
        Some(vec!["Answer in two lines.".to_string()]),
        "the re-seat's instructions"
    );
    assert_eq!(
        build.system_prompt,
        meerkat_core::SystemPromptOverride::Set(PROMPT.to_string()),
        "the first seat's system prompt: the re-seat names none"
    );
    assert_eq!(
        build.app_context,
        Some(serde_json::json!({ "desk": CONTEXT_DESK })),
        "the first seat's context"
    );
}

/// A resume seat of an absent session that no seat named unborn is still
/// refused: that session was lost (or never existed), not never born.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resume_seat_of_an_absent_session_no_seat_named_is_refused_absent() {
    let mob = SeatMob::new("seat-lost-session").await;
    let mut spec = worker_spec(&AgentIdentity::from("lost-session"));
    spec.launch_mode = crate::launch::MemberLaunchMode::Resume {
        bridge_session_id: SessionId::new(),
        resume_from_role: None,
    };
    let outcomes = mob
        .handle
        .seat_dormant_members(vec![spec])
        .await
        .expect("the seat batch answers");
    assert!(
        matches!(
            &outcomes[0],
            DormantSeatOutcome::Refused {
                error: MobError::SessionUnavailableForResume {
                    reason: crate::error::SessionResumeUnavailableReason::Absent,
                    ..
                },
                ..
            }
        ),
        "{outcomes:?}"
    );
}
