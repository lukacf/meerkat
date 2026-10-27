//! Producer bridge Part B: a held spawn stops at the provisioner's hold
//! point, exposes the finished surface, and is then activated (published
//! through the ordinary spawn path) or aborted. Abort, a dropped handle and
//! an expired setup deadline all await the held abort and fail the pending
//! spawn typed; the member is not published.
//!
//! The whole setup is bounded by its deadline and by caller loss. Each
//! control below parks one step (command admission, a preparation permit,
//! the turn-finalization boundary, a host hook, the actor creation, a
//! publication) and asserts what the caller observes before releasing it.

use super::*;
use crate::runtime::held_spawn::{
    HeldSpawn, HeldSpawnDeadline as Instant, HeldSpawnStart, abort_all_held,
};
use crate::{MemberDestinationSelection, MemberRuntimeBuildIdentity};
use meerkat_authorization_contracts::constraints::AudienceRef;
use meerkat_core::auth::{PrincipalKind, PrincipalRef};

/// Bound for test steps that must happen promptly; it only turns a hang into
/// a failure.
const STEP: Duration = Duration::from_secs(10);

/// A setup deadline far enough out for a held spawn to reach a parked step
/// before it.
const PARKED_DEADLINE: Duration = Duration::from_millis(1_500);

/// How long a parked step is shown to keep its settlement pending.
const STILL_PENDING: Duration = Duration::from_millis(200);

/// A host that selects each member's registration and finishes it under one
/// application tool policy: only such members can be held. With a
/// destination gate, each destination selection parks there until the gate
/// is released.
#[derive(Default)]
struct SelectingHost {
    destination_gate: Option<Arc<TestRuntimeControlBarrier>>,
}

impl SelectingHost {
    fn parking_destinations(gate: Arc<TestRuntimeControlBarrier>) -> Self {
        Self {
            destination_gate: Some(gate),
        }
    }

    fn selection(identity: &MemberRuntimeBuildIdentity) -> MemberDestinationSelection {
        let owner = PrincipalRef::new(PrincipalKind::SharedAgent, identity.member().member.clone())
            .unwrap();
        MemberDestinationSelection {
            logical_owner: owner.clone(),
            context: identity.session_document().clone(),
            context_generation: 1,
            audience: AudienceRef::Principal { principal: owner },
        }
    }
}

#[async_trait]
impl crate::MemberRuntimeRegistrationHook for SelectingHost {
    async fn select_destination(
        &self,
        identity: &MemberRuntimeBuildIdentity,
    ) -> Result<MemberDestinationSelection, MobError> {
        if let Some(gate) = self.destination_gate.as_ref() {
            gate.record_boundary_call();
            gate.wait_for_release().await;
        }
        Ok(Self::selection(identity))
    }

    async fn expected_destination(
        &self,
        identity: &MemberRuntimeBuildIdentity,
    ) -> Result<MemberDestinationSelection, MobError> {
        Ok(Self::selection(identity))
    }

    async fn verify_member_surface(
        &self,
        _identity: &MemberRuntimeBuildIdentity,
        _surface: &[meerkat_core::ToolDef],
    ) -> Result<(), MobError> {
        Ok(())
    }

    async fn prepare_member_surface(
        &self,
        _identity: &MemberRuntimeBuildIdentity,
        surface: &meerkat_core::PreparedToolSurface,
    ) -> Result<meerkat_core::ToolSurfaceSelection, MobError> {
        Ok(meerkat_core::ToolSurfaceSelection {
            binding: meerkat_core::ApplicationToolPolicyBinding::Provider {
                provider_id: meerkat_core::PolicyProviderId::new("host").unwrap(),
                policy_id: meerkat_core::PolicyId::new("member-tools").unwrap(),
            },
            compiled_against: surface.clone(),
        })
    }
}

/// The machine's governed work host: a scoped member destination needs one.
/// It admits no governed input, which these tests never submit.
struct AdmitsNothing;

impl meerkat_runtime::input_authority::NativeWorkAuthorizationHost for AdmitsNothing {
    fn authenticate_association(
        &self,
        _runtime_id: &meerkat_runtime::LogicalRuntimeId,
        _input: &meerkat_runtime::input::Input,
        _ingress: &meerkat_runtime::input_authority::NativeIngressContext,
        _association: &meerkat_authorization_contracts::work_association::InputAuthorityAssociation,
    ) -> Result<(), meerkat_runtime::input_authority::NativeAdmissionError> {
        Err(meerkat_core::OperationRefused::new(meerkat_core::OperationRefusalKind::Denied).into())
    }

    fn work_context(
        &self,
        _batch: &meerkat_runtime::input_authority::NativeWorkBatch,
    ) -> Result<
        meerkat_core::authorization::WorkAuthorizationContext,
        meerkat_runtime::input_authority::NativeWorkContextError,
    > {
        Err(meerkat_runtime::input_authority::NativeWorkContextError::MalformedAcceptedWork)
    }
}

/// A mob whose members are registered and finished by a selecting host, so
/// fresh members can be held.
async fn create_holding_mob() -> MobHandle {
    create_holding_mob_answering(None).await
}

/// [`create_holding_mob`] whose session service answers each held member's
/// controller route query with `answer` (unset: the trait default).
async fn create_holding_mob_answering(answer: Option<MockControllerRouteAnswer>) -> MobHandle {
    holding_mob(answer, SelectingHost::default()).await.0
}

/// A mob whose members are registered and finished by `host`, with its
/// session service, which answers each held member's controller route query
/// with `answer` (unset: the trait default).
async fn holding_mob(
    answer: Option<MockControllerRouteAnswer>,
    host: SelectingHost,
) -> (MobHandle, Arc<MockSessionService>) {
    let service = Arc::new(MockSessionService::new());
    if let Some(answer) = answer {
        service.answer_controller_route(answer);
    }
    let machine = Arc::new(
        meerkat_runtime::MeerkatMachine::ephemeral()
            .with_native_work_authorization_host(Arc::new(AdmitsNothing))
            .expect("install the governed work host"),
    );
    service.set_runtime_adapter(machine);
    service.decide_tool_surface_at_create();
    let handle = MobBuilder::new(sample_definition(), MobStorage::in_memory())
        .with_session_service(service.clone())
        .with_member_runtime_registration_hook(Arc::new(host))
        .create()
        .await
        .expect("create mob");
    (handle, service)
}

fn worker_spec(name: &str) -> SpawnMemberSpec {
    let mut spec = SpawnMemberSpec::new(ProfileName::from("worker"), AgentIdentity::from(name));
    spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    spec
}

fn deadline_in(duration: Duration) -> Instant {
    Instant::now() + duration
}

async fn held(handle: &MobHandle, name: &str, deadline: Instant) -> HeldSpawn {
    tokio::time::timeout(STEP, handle.spawn_held_within(worker_spec(name), deadline))
        .await
        .expect("the spawn reaches its hold point promptly")
        .expect("the member is held")
}

async fn in_roster(handle: &MobHandle, name: &str) -> bool {
    handle
        .get_member(&AgentIdentity::from(name))
        .await
        .expect("roster read")
        .is_some()
}

/// Start a held spawn of `name` in its own task, as a caller would.
fn start_held(
    handle: &MobHandle,
    name: &str,
    deadline: Instant,
) -> tokio::task::JoinHandle<Result<HeldSpawnStart, MobError>> {
    let handle = handle.clone();
    let spec = worker_spec(name);
    tokio::spawn(async move { handle.spawn_held_until(spec, deadline).await })
}

/// The caller's start returns at the deadline with the setup's pending
/// settlement, not a held member.
async fn unsettled_at_the_deadline(
    start: tokio::task::JoinHandle<Result<HeldSpawnStart, MobError>>,
) -> crate::runtime::HeldSpawnSettlement {
    let start = tokio::time::timeout(STEP, start)
        .await
        .expect("the deadline bounds the caller's wait")
        .expect("start task")
        .expect("the spawn is admitted");
    match start {
        HeldSpawnStart::Unsettled(settlement) => settlement,
        HeldSpawnStart::Held(held) => panic!("{} was held past its parked step", held.identity()),
    }
}

/// While its setup is unsettled the identity stays owned: an ordinary spawn
/// of it is refused.
async fn identity_is_owned(handle: &MobHandle, name: &str) {
    let refused = tokio::time::timeout(STEP, handle.spawn_spec(worker_spec(name)))
        .await
        .expect("an ordinary spawn answers promptly");
    assert!(
        matches!(refused, Err(MobError::MemberAlreadyExists(_))),
        "the identity stays owned while its setup is unsettled: {:?}",
        refused.map(|spawned| spawned.agent_identity)
    );
}

/// While the returned guard lives, the first event on this thread whose
/// message is `message` blocks the thread until `until`, and sets `blocked`.
/// `#[tokio::test]` runs the mob's own tasks on the test's thread, so the step
/// that emits the event is held right there, between the awaits around it.
///
/// The guard also keeps a second, inert dispatcher registered. With a single
/// registered dispatcher, tracing-core computes the interest of a callsite
/// first hit on another (parallel test) thread from that thread's default
/// alone, which would disable the event for this thread's subscriber.
fn block_once_at_event(
    message: &'static str,
    until: Instant,
    blocked: Arc<AtomicBool>,
) -> (tracing::Dispatch, tracing::subscriber::DefaultGuard) {
    use tracing_subscriber::layer::SubscriberExt as _;

    struct MessageIs {
        expected: &'static str,
        matched: bool,
    }

    impl tracing::field::Visit for MessageIs {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            if field.name() == "message" && format!("{value:?}") == self.expected {
                self.matched = true;
            }
        }
    }

    struct BlockOnce {
        message: &'static str,
        until: Instant,
        blocked: Arc<AtomicBool>,
    }

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for BlockOnce {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            if self.blocked.load(Ordering::Acquire) {
                return;
            }
            let mut visitor = MessageIs {
                expected: self.message,
                matched: false,
            };
            event.record(&mut visitor);
            if visitor.matched && !self.blocked.swap(true, Ordering::AcqRel) {
                std::thread::sleep(self.until.saturating_duration_since(Instant::now()));
            }
        }
    }

    let companion = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
    let guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(BlockOnce {
        message,
        until,
        blocked,
    }));
    (companion, guard)
}

/// A held spawn started directly on the actor: the test keeps each part of
/// the caller's side, its presence (dropping it is the caller's loss), the
/// hold's delivery channel and the raw typed completion.
struct RawHeldStart {
    caller: Option<crate::runtime::held_spawn::HeldSetupCaller>,
    ready_rx: Option<tokio::sync::oneshot::Receiver<crate::runtime::held_spawn::HeldSpawnReady>>,
    completion:
        tokio::sync::oneshot::Receiver<Result<crate::runtime::MemberSpawnReceipt, MobError>>,
}

async fn start_held_raw(handle: &MobHandle, name: &str, deadline: Instant) -> RawHeldStart {
    let (caller, bound) = crate::runtime::held_spawn::HeldProvisionBound::new(name, deadline);
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let spec = worker_spec(name);
    let completion = handle
        .enqueue_actor_command_for_test(|reply_tx| MobCommand::Spawn {
            spec: Box::new(spec),
            spawn_source: crate::runtime::SpawnSource::Consumer,
            owner_bridge_session_id: None,
            ops_registry: None,
            held: Some(crate::runtime::held_spawn::HeldSpawnRequest { ready_tx, bound }),
            reply_tx,
        })
        .await
        .expect("held spawn enqueue");
    RawHeldStart {
        caller: Some(caller),
        ready_rx: Some(ready_rx),
        completion,
    }
}

impl RawHeldStart {
    /// The caller goes away: its presence and the hold's delivery channel
    /// are dropped together, as the public start drops them.
    fn lose_caller(&mut self) {
        drop(self.ready_rx.take());
        drop(self.caller.take());
    }

    /// The raw typed result the setup's owner settled with.
    async fn completion(&mut self) -> Result<crate::runtime::MemberSpawnReceipt, MobError> {
        tokio::time::timeout(STEP, &mut self.completion)
            .await
            .expect("the setup settles")
            .expect("the setup's owner answers")
    }
}

/// Start a raw held spawn of `name` and park its owned create right before it
/// awaits its registration witness. The member's turn-finalization boundary
/// is held until the park is armed, so its session is known before its owned
/// create starts.
async fn raw_start_parked_at_the_registration_witness(
    handle: &MobHandle,
    service: &MockSessionService,
    name: &str,
    deadline: Instant,
) -> (RawHeldStart, tokio::sync::oneshot::Sender<()>) {
    let boundary = service.install_non_reentrant_turn_finalization_gate();
    let blocked = Arc::clone(&boundary).lock_owned().await;
    let acquisitions = service.turn_finalization_sessions().len();
    let start = start_held_raw(handle, name, deadline).await;
    let session_id = tokio::time::timeout(STEP, async {
        loop {
            if let Some(session_id) = service.turn_finalization_sessions().get(acquisitions) {
                break session_id.clone();
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the setup waits for its turn-finalization boundary");
    let (entered, release) =
        crate::runtime::provisioner::arm_owned_create_before_registration_witness_for_test(
            session_id,
        );
    drop(blocked);
    tokio::time::timeout(STEP, entered)
        .await
        .expect("the owned create reaches its registration witness")
        .expect("entered");
    (start, release)
}

/// The pending spawn settled: an ordinary spawn of the same identity is
/// admitted again.
async fn identity_is_free(handle: &MobHandle, name: &str) {
    let respawned = tokio::time::timeout(STEP, async {
        loop {
            match handle.spawn_spec(worker_spec(name)).await {
                Ok(result) => break result,
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        }
    })
    .await
    .expect("the held spawn's pending custody settles");
    assert_eq!(respawned.agent_identity, AgentIdentity::from(name));
}

#[tokio::test]
async fn an_activated_held_spawn_is_published_through_the_ordinary_path() {
    let handle = create_holding_mob().await;
    let spawn = held(&handle, "held-worker", deadline_in(STEP)).await;
    assert_eq!(spawn.identity(), "held-worker");
    let _surface = spawn.finished_surface().clone();
    // A session service that cannot pin its actors' controllers (this mock
    // keeps the trait default) still holds and publishes its members: the
    // typed observation carries no report, which only a host that requires
    // the route refuses.
    assert_eq!(
        spawn.controller_route_report(),
        &crate::ControllerRouteObservation::Unsupported
    );
    assert!(spawn.controller_route_report().report().is_none());
    assert!(
        !in_roster(&handle, "held-worker").await,
        "a held member is not published"
    );
    let result = tokio::time::timeout(STEP, spawn.activate())
        .await
        .expect("activation completes promptly")
        .expect("activation publishes the member");
    assert_eq!(result.agent_identity, AgentIdentity::from("held-worker"));
    assert!(in_roster(&handle, "held-worker").await);
}

#[tokio::test]
async fn an_aborted_held_spawn_publishes_nothing_and_frees_the_identity() {
    let handle = create_holding_mob().await;
    let spawn = held(&handle, "held-worker", deadline_in(STEP)).await;
    tokio::time::timeout(STEP, spawn.abort())
        .await
        .expect("abort completes promptly")
        .expect("the held member is aborted");
    assert!(!in_roster(&handle, "held-worker").await);
    identity_is_free(&handle, "held-worker").await;
}

#[tokio::test]
async fn a_dropped_held_spawn_is_aborted() {
    let handle = create_holding_mob().await;
    drop(held(&handle, "held-worker", deadline_in(STEP)).await);
    identity_is_free(&handle, "held-worker").await;
}

#[tokio::test]
async fn a_held_spawn_past_its_setup_deadline_expires_and_cannot_activate() {
    let handle = create_holding_mob().await;
    // The deadline bounds the construction too, so it leaves room to reach
    // the hold point on a loaded test host.
    let deadline = deadline_in(PARKED_DEADLINE);
    let spawn = held(&handle, "held-worker", deadline).await;
    tokio::time::sleep_until(deadline + STILL_PENDING).await;
    let error = tokio::time::timeout(STEP, spawn.activate())
        .await
        .expect("the expired hold answers promptly")
        .expect_err("an expired hold cannot be activated");
    assert!(
        matches!(
            error,
            MobError::HeldSpawnExpired { ref member, cleanup: None } if member == "held-worker"
        ),
        "expected HeldSpawnExpired, got {error}"
    );
    assert!(!in_roster(&handle, "held-worker").await);
    identity_is_free(&handle, "held-worker").await;
}

/// An Activate sent after the deadline is refused even when the holding task
/// first observes it together with the elapsed timer: the current-thread
/// runtime is blocked past the deadline, so the decision and the expired
/// timer are both ready before the holding task runs again.
#[tokio::test]
async fn an_activation_sent_after_the_deadline_expires_even_when_ready_with_the_timer() {
    let handle = create_holding_mob().await;
    let deadline = deadline_in(PARKED_DEADLINE);
    let spawn = held(&handle, "held-worker", deadline).await;
    std::thread::sleep(deadline.saturating_duration_since(Instant::now()) + STILL_PENDING);
    let error = tokio::time::timeout(STEP, spawn.activate())
        .await
        .expect("the late activation answers promptly")
        .expect_err("an activation after the deadline cannot publish");
    assert!(
        matches!(
            error,
            MobError::HeldSpawnExpired { ref member, cleanup: None } if member == "held-worker"
        ),
        "expected HeldSpawnExpired, got {error}"
    );
    assert!(!in_roster(&handle, "held-worker").await);
    identity_is_free(&handle, "held-worker").await;
}

#[tokio::test]
async fn a_failed_bundle_aborts_every_unpublished_member_and_keeps_each_outcome() {
    let handle = create_holding_mob().await;
    // One setup deadline for the whole pass: the first member stays held
    // while the others are constructed.
    let deadline = deadline_in(STEP);
    let first = held(&handle, "held-a", deadline).await;
    let second = held(&handle, "held-b", deadline).await;
    let outcomes = abort_all_held(vec![first, second]).await;
    assert_eq!(outcomes.len(), 2);
    for (member, outcome) in &outcomes {
        assert!(outcome.is_ok(), "{member}: {outcome:?}");
        assert!(!in_roster(&handle, member).await);
    }
}

#[tokio::test]
async fn a_bundle_past_its_shared_deadline_reports_each_expired_member() {
    let handle = create_holding_mob().await;
    // Both members are constructed within the one deadline, which leaves
    // room for that on a loaded test host.
    let deadline = deadline_in(PARKED_DEADLINE);
    let first = held(&handle, "held-a", deadline).await;
    let second = held(&handle, "held-b", deadline).await;
    tokio::time::sleep_until(deadline + STILL_PENDING).await;
    let outcomes = tokio::time::timeout(STEP, abort_all_held(vec![first, second]))
        .await
        .expect("the expired bundle answers promptly");
    let mut members: Vec<&str> = outcomes.iter().map(|(member, _)| member.as_str()).collect();
    members.sort_unstable();
    assert_eq!(members, ["held-a", "held-b"]);
    for (member, outcome) in &outcomes {
        // An expired hold whose abort confirmed its cleanup aborts cleanly.
        assert!(outcome.is_ok(), "{member}: {outcome:?}");
        assert!(!in_roster(&handle, member).await);
        identity_is_free(&handle, member).await;
    }
}

#[tokio::test]
async fn a_resumed_member_cannot_be_held() {
    let handle = create_holding_mob().await;
    let mut spec = worker_spec("held-resume");
    spec.launch_mode = crate::launch::MemberLaunchMode::Resume {
        bridge_session_id: SessionId::new(),
        resume_from_role: None,
    };
    let error = tokio::time::timeout(STEP, handle.spawn_held_within(spec, deadline_in(STEP)))
        .await
        .expect("refused promptly")
        .expect_err("a resumed member is not held");
    assert!(
        matches!(error, MobError::HeldSpawnUnsupported { .. }),
        "expected HeldSpawnUnsupported, got {error}"
    );
}

/// A member whose actor pinned no controller client with route facts is held
/// with the typed `Unavailable` observation (never a substituted report) and
/// still publishes.
#[tokio::test]
async fn a_held_member_without_route_facts_reports_unavailable_and_still_activates() {
    let handle = create_holding_mob_answering(Some(MockControllerRouteAnswer::Observed(
        crate::ControllerRouteObservation::Unavailable,
    )))
    .await;
    let spawn = held(&handle, "held-worker", deadline_in(STEP)).await;
    assert_eq!(
        spawn.controller_route_report(),
        &crate::ControllerRouteObservation::Unavailable
    );
    assert!(spawn.controller_route_report().report().is_none());
    tokio::time::timeout(STEP, spawn.activate())
        .await
        .expect("activation completes promptly")
        .expect("activation publishes the member");
    assert!(in_roster(&handle, "held-worker").await);
}

/// A held member whose exact actor is lost before its route is observed is
/// never offered: the spawn fails once the hold's cleanup was awaited.
#[tokio::test]
async fn a_held_member_whose_actor_is_lost_fails_after_its_awaited_cleanup() {
    let handle = create_holding_mob_answering(Some(MockControllerRouteAnswer::ActorLost)).await;
    let error = tokio::time::timeout(
        STEP,
        handle.spawn_held_within(worker_spec("held-worker"), deadline_in(STEP)),
    )
    .await
    .expect("the spawn answers promptly")
    .expect_err("a lost actor is not held");
    assert!(
        error
            .to_string()
            .contains("lost its actor before its controller route was observed"),
        "{error}"
    );
    assert!(!in_roster(&handle, "held-worker").await);
    identity_is_free(&handle, "held-worker").await;
}

/// A route query still running at the setup deadline is given up: the hold's
/// own abort is awaited and the spawn expires with that abort's cleanup
/// outcome, before the member was ever offered.
#[tokio::test]
async fn a_route_query_crossing_the_setup_deadline_expires_with_its_cleanup_outcome() {
    let handle = create_holding_mob_answering(Some(MockControllerRouteAnswer::Parked)).await;
    let error = tokio::time::timeout(
        STEP,
        handle.spawn_held_within(worker_spec("held-worker"), deadline_in(PARKED_DEADLINE)),
    )
    .await
    .expect("the deadline bounds the route query")
    .expect_err("an expired hold is not offered");
    assert!(
        matches!(
            error,
            MobError::HeldSpawnExpired { ref member, cleanup: None } if member == "held-worker"
        ),
        "expected HeldSpawnExpired with confirmed cleanup, got {error}"
    );
    assert!(!in_roster(&handle, "held-worker").await);
    identity_is_free(&handle, "held-worker").await;
}

/// A caller that stops waiting while the route query runs ends the hold long
/// before its setup deadline: the query is given up, the hold's abort is
/// awaited, and the pending spawn settles without publishing anything.
#[tokio::test]
async fn a_caller_lost_during_the_route_query_settles_the_awaited_abort() {
    let handle = create_holding_mob_answering(Some(MockControllerRouteAnswer::Parked)).await;
    let gone = tokio::time::timeout(
        Duration::from_millis(200),
        handle.spawn_held_within(
            worker_spec("held-worker"),
            deadline_in(Duration::from_mins(10)),
        ),
    )
    .await;
    assert!(gone.is_err(), "the caller stopped waiting during the query");
    identity_is_free(&handle, "held-worker").await;
    assert!(
        in_roster(&handle, "held-worker").await,
        "the ordinary respawn"
    );
}

/// A spawn whose command still waits for admission at the deadline expires
/// there: the command was never delivered, so nothing was admitted and
/// nothing exists to settle.
#[tokio::test]
async fn a_spawn_still_waiting_for_admission_at_the_deadline_expires_unadmitted() {
    let handle = create_holding_mob().await;
    // A saturated actor command channel: its one slot is reserved and never
    // used.
    let (saturated_tx, mut saturated_rx) = tokio::sync::mpsc::channel(1);
    let reserved = saturated_tx.reserve().await.expect("reserve the only slot");
    let mut saturated = handle.clone();
    saturated.command_tx = saturated_tx.clone();
    let error = tokio::time::timeout(
        STEP,
        saturated.spawn_held_until(
            worker_spec("held-worker"),
            deadline_in(Duration::from_millis(200)),
        ),
    )
    .await
    .expect("the deadline bounds command admission")
    .expect_err("a spawn that was never admitted is not held");
    assert!(
        matches!(
            error,
            MobError::HeldSpawnExpired { ref member, cleanup: None } if member == "held-worker"
        ),
        "expected HeldSpawnExpired with nothing to clean up, got {error}"
    );
    drop(reserved);
    assert!(
        saturated_rx.try_recv().is_err(),
        "the held spawn's command was never delivered"
    );
    // The mob never saw it: the identity is free at once.
    let spawned = tokio::time::timeout(STEP, handle.spawn_spec(worker_spec("held-worker")))
        .await
        .expect("an ordinary spawn answers promptly")
        .expect("the identity was never claimed");
    assert_eq!(spawned.agent_identity, AgentIdentity::from("held-worker"));
}

/// A held spawn whose preparation still waits for a preparation permit at
/// the deadline settles without waiting for one: the wait owned nothing and
/// the preparation never started, while every permit stays held.
#[tokio::test]
async fn a_preparation_permit_wait_past_the_deadline_settles_while_every_permit_is_held() {
    let handle = create_holding_mob().await;
    let permits = crate::runtime::actor::spawn_preparation::SPAWN_PREPARATION_CONCURRENCY;
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let (entered_tx, mut entered_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut fillers = Vec::new();
    for index in 0..permits {
        let name = format!("held-permit-filler-{index}");
        arm_spawn_preparation_gate_for_test(
            &AgentIdentity::from(name.as_str()),
            crate::runtime::actor::spawn_preparation::SpawnPreparationTestGate {
                entered: Some(entered_tx.clone()),
                release: Some(Arc::clone(&release)),
                settled: None,
            },
        );
        let spec = worker_spec(&name);
        fillers.push(
            handle
                .enqueue_actor_command_for_test(|reply_tx| MobCommand::Spawn {
                    spec: Box::new(spec),
                    spawn_source: crate::runtime::SpawnSource::Consumer,
                    owner_bridge_session_id: None,
                    ops_registry: None,
                    held: None,
                    reply_tx,
                })
                .await
                .expect("filler spawn enqueue"),
        );
    }
    for _ in 0..permits {
        tokio::time::timeout(STEP, entered_rx.recv())
            .await
            .expect("a filler preparation holds its permit")
            .expect("entered channel");
    }
    // The held member's preparation would report here too, had it started.
    let held_identity = AgentIdentity::from("held-permit-worker");
    arm_spawn_preparation_gate_for_test(
        &held_identity,
        crate::runtime::actor::spawn_preparation::SpawnPreparationTestGate {
            entered: Some(entered_tx.clone()),
            ..Default::default()
        },
    );
    let start = start_held(
        &handle,
        "held-permit-worker",
        deadline_in(Duration::from_millis(300)),
    );
    let settlement = unsettled_at_the_deadline(start).await;
    tokio::time::timeout(STEP, settlement.settled())
        .await
        .expect("the setup settles without waiting for a permit")
        .expect("it expired with nothing to clean up");
    let census = spawn_preparation_census_for_test(&handle, &held_identity).await;
    assert_eq!(
        census.preparation_permits_in_use, permits,
        "every permit is still held by a filler"
    );
    assert!(!census.identity_materialization_in_flight);
    assert!(
        entered_rx.try_recv().is_err(),
        "the held member's preparation never started"
    );
    release.add_permits(permits);
    for filler in fillers {
        tokio::time::timeout(STEP, filler)
            .await
            .expect("every filler spawn completes")
            .expect("filler reply")
            .expect("filler spawn");
    }
    identity_is_free(&handle, "held-permit-worker").await;
}

/// A held spawn still waiting for its turn-finalization boundary at the
/// deadline: the caller gets the pending settlement while the boundary stays
/// blocked, and once the acquisition returns the bound is checked before
/// anything is prepared under it (preparing would fail visibly here).
#[tokio::test]
async fn a_boundary_wait_past_the_deadline_stops_before_anything_is_prepared() {
    let (handle, service) = holding_mob(None, SelectingHost::default()).await;
    let boundary = service.install_non_reentrant_turn_finalization_gate();
    let blocked = Arc::clone(&boundary).lock_owned().await;
    let acquisitions = service.turn_finalization_sessions().len();
    let start = start_held(&handle, "held-worker", deadline_in(PARKED_DEADLINE));
    let session_id = tokio::time::timeout(STEP, async {
        loop {
            if let Some(session_id) = service.turn_finalization_sessions().get(acquisitions) {
                break session_id.clone();
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the setup waits for its turn-finalization boundary");
    crate::runtime::provisioner::arm_provision_prepare_fault_for_test(
        session_id,
        crate::runtime::provisioner::ProvisionPrepareTestFault::UnrecordedFailure,
    );
    let settlement = unsettled_at_the_deadline(start).await;
    let settled = settlement.settled();
    tokio::pin!(settled);
    assert!(
        tokio::time::timeout(STILL_PENDING, &mut settled)
            .await
            .is_err(),
        "the setup settles only once its boundary acquisition returns"
    );
    identity_is_owned(&handle, "held-worker").await;
    drop(blocked);
    tokio::time::timeout(STEP, settled)
        .await
        .expect("the setup settles once its boundary returns")
        .expect("it expired before anything was prepared");
    assert_eq!(
        service.max_concurrent_create_session_calls(),
        0,
        "no actor was created"
    );
    assert!(!in_roster(&handle, "held-worker").await);
    identity_is_free(&handle, "held-worker").await;
}

/// A held spawn whose host destination selection still runs at the
/// deadline: the caller gets the pending settlement while the hook stays
/// parked, and once the hook returns the owned create task stops before the
/// actor is created and settles its abort.
#[tokio::test]
async fn a_host_hook_parked_past_the_deadline_stops_before_the_actor_is_created() {
    let destination = Arc::new(TestRuntimeControlBarrier::new());
    let (handle, service) = holding_mob(
        None,
        SelectingHost::parking_destinations(Arc::clone(&destination)),
    )
    .await;
    let start = start_held(&handle, "held-worker", deadline_in(PARKED_DEADLINE));
    tokio::time::timeout(STEP, destination.boundary_entered.notified())
        .await
        .expect("the host's destination selection parks");
    let settlement = unsettled_at_the_deadline(start).await;
    assert_eq!(settlement.member(), "held-worker");
    let settled = settlement.settled();
    tokio::pin!(settled);
    assert!(
        tokio::time::timeout(STILL_PENDING, &mut settled)
            .await
            .is_err(),
        "the setup settles only once the hook returns"
    );
    identity_is_owned(&handle, "held-worker").await;
    destination.release_all();
    tokio::time::timeout(STEP, settled)
        .await
        .expect("the setup settles once the hook returns")
        .expect("it expired with its cleanup confirmed");
    assert_eq!(
        service.max_concurrent_create_session_calls(),
        0,
        "the actor was never created"
    );
    assert!(!in_roster(&handle, "held-worker").await);
    identity_is_free(&handle, "held-worker").await;
}

/// A held spawn whose actor creation still runs at the deadline: the caller
/// gets the pending settlement while the creation stays parked, and the
/// owner settles the setup as an expiry once the creation returns.
#[tokio::test]
async fn an_actor_creation_parked_past_the_deadline_returns_its_pending_settlement() {
    let (handle, service) = holding_mob(None, SelectingHost::default()).await;
    let creation = service.park_next_session_creation();
    let start = start_held(&handle, "held-worker", deadline_in(PARKED_DEADLINE));
    tokio::time::timeout(STEP, creation.boundary_entered.notified())
        .await
        .expect("the member's actor creation parks");
    let settlement = unsettled_at_the_deadline(start).await;
    assert_eq!(
        service.create_session_in_flight.load(Ordering::Relaxed),
        1,
        "the actor creation is still in flight"
    );
    let settled = settlement.settled();
    tokio::pin!(settled);
    assert!(
        tokio::time::timeout(STILL_PENDING, &mut settled)
            .await
            .is_err(),
        "the setup settles only once the creation returns"
    );
    identity_is_owned(&handle, "held-worker").await;
    creation.release_all();
    tokio::time::timeout(STEP, settled)
        .await
        .expect("the setup settles once the creation returns")
        .expect("it expired with its cleanup confirmed");
    assert!(!in_roster(&handle, "held-worker").await);
    identity_is_free(&handle, "held-worker").await;
}

/// A caller lost while its member's host hook runs: the identity stays owned
/// until the hook returns, then the owned create task observes the loss and
/// stops before the actor is created.
#[tokio::test]
async fn a_caller_lost_while_a_host_hook_runs_stops_before_the_actor_is_created() {
    let destination = Arc::new(TestRuntimeControlBarrier::new());
    let (handle, service) = holding_mob(
        None,
        SelectingHost::parking_destinations(Arc::clone(&destination)),
    )
    .await;
    let start = start_held(&handle, "held-worker", deadline_in(Duration::from_mins(10)));
    tokio::time::timeout(STEP, destination.boundary_entered.notified())
        .await
        .expect("the host's destination selection parks");
    start.abort();
    assert!(
        start
            .await
            .expect_err("the caller's wait was dropped")
            .is_cancelled()
    );
    identity_is_owned(&handle, "held-worker").await;
    destination.release_all();
    let identity = AgentIdentity::from("held-worker");
    tokio::time::timeout(STEP, async {
        while spawn_preparation_census_for_test(&handle, &identity)
            .await
            .identity_materialization_in_flight
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the setup settles once the hook returns");
    assert_eq!(
        service.max_concurrent_create_session_calls(),
        0,
        "the actor was never created"
    );
    assert!(!in_roster(&handle, "held-worker").await);
    identity_is_free(&handle, "held-worker").await;
}

/// Activation acceptance is the linearization point: an activation accepted
/// before the deadline whose publication still runs past it keeps its actual
/// outcome, the publication; it is never relabeled an expiry.
#[tokio::test]
async fn an_activation_accepted_before_the_deadline_keeps_its_publication_past_it() {
    let (handle, service) = holding_mob(None, SelectingHost::default()).await;
    let publication = service.park_next_session_supervisor_publish();
    let deadline = deadline_in(PARKED_DEADLINE);
    let start = tokio::time::timeout(
        STEP,
        handle.spawn_held_until(worker_spec("held-worker"), deadline),
    )
    .await
    .expect("the spawn reaches its hold point promptly")
    .expect("the spawn is admitted");
    let HeldSpawnStart::Held(spawn) = start else {
        panic!("the member is held before its deadline");
    };
    // Accepted now, before the deadline; its publication then parks.
    let activation = tokio::spawn(spawn.activate());
    tokio::time::timeout(STEP, async {
        while publication.boundary_calls.load(Ordering::Acquire) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the accepted activation's publication parks");
    tokio::time::sleep_until(deadline + STILL_PENDING).await;
    assert!(
        !activation.is_finished(),
        "the publication is still running past the deadline"
    );
    publication.release_all();
    let published = tokio::time::timeout(STEP, activation)
        .await
        .expect("the publication completes once released")
        .expect("activation task")
        .expect("the accepted activation keeps its publication");
    assert_eq!(published.agent_identity, AgentIdentity::from("held-worker"));
    assert!(in_roster(&handle, "held-worker").await);
}

/// Deadline first includes the simultaneous case: a member that already
/// reached its hold, but was not delivered before the deadline was ready, is
/// not delivered. The caller gets the pending settlement, and the hold that
/// was never delivered expires.
#[tokio::test]
async fn a_deadline_ready_together_with_an_undelivered_hold_returns_the_pending_settlement() {
    let (handle, service) = holding_mob(None, SelectingHost::default()).await;
    let deadline = deadline_in(PARKED_DEADLINE);
    let mut start = Box::pin(handle.spawn_held_until(worker_spec("held-worker"), deadline));
    // One poll admits the command; the caller is then not polled again until
    // both its hold and its deadline are ready.
    assert!(
        futures::poll!(&mut start).is_pending(),
        "the caller waits for its member"
    );
    tokio::time::timeout(STEP, async {
        while service.controller_route_queries.load(Ordering::Acquire) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the member reaches its hold and is sent to the caller");
    tokio::time::sleep_until(deadline + STILL_PENDING).await;
    let start = tokio::time::timeout(STEP, start)
        .await
        .expect("the caller answers at once")
        .expect("the spawn is admitted");
    let HeldSpawnStart::Unsettled(settlement) = start else {
        panic!("a hold not delivered before the deadline is never delivered after it");
    };
    tokio::time::timeout(STEP, settlement.settled())
        .await
        .expect("the undelivered hold settles")
        .expect("it expired with its cleanup confirmed");
    assert!(!in_roster(&handle, "held-worker").await);
    identity_is_free(&handle, "held-worker").await;
}

/// The caller's loss stops governing cancellation at the accepted
/// activation: a caller dropped after its activation was accepted does not
/// abort the member, whose publication completes on its own.
#[tokio::test]
async fn a_caller_dropped_after_its_activation_was_accepted_does_not_abort_the_member() {
    let (handle, service) = holding_mob(None, SelectingHost::default()).await;
    let publication = service.park_next_session_supervisor_publish();
    let start = tokio::time::timeout(
        STEP,
        handle.spawn_held_until(
            worker_spec("held-worker"),
            deadline_in(Duration::from_mins(10)),
        ),
    )
    .await
    .expect("the spawn reaches its hold point promptly")
    .expect("the spawn is admitted");
    let HeldSpawnStart::Held(spawn) = start else {
        panic!("the member is held before its deadline");
    };
    let activation = tokio::spawn(spawn.activate());
    tokio::time::timeout(STEP, async {
        while publication.boundary_calls.load(Ordering::Acquire) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the accepted activation's publication parks");
    // The caller goes away after its activation was accepted.
    activation.abort();
    assert!(
        activation
            .await
            .expect_err("the caller's activation wait was dropped")
            .is_cancelled()
    );
    publication.release_all();
    tokio::time::timeout(STEP, async {
        while !in_roster(&handle, "held-worker").await {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the accepted member is published without its caller");
}

/// The legacy `spawn_held_within` is not bounded by the deadline: it waits
/// for the setup's settlement and returns a clean one as `HeldSpawnExpired`
/// without cleanup.
#[tokio::test]
async fn the_legacy_held_spawn_waits_for_the_settlement_and_maps_a_clean_one_to_expiry() {
    let (handle, service) = holding_mob(None, SelectingHost::default()).await;
    let creation = service.park_next_session_creation();
    let deadline = deadline_in(PARKED_DEADLINE);
    let within = tokio::spawn({
        let handle = handle.clone();
        async move {
            handle
                .spawn_held_within(worker_spec("held-worker"), deadline)
                .await
        }
    });
    tokio::time::timeout(STEP, creation.boundary_entered.notified())
        .await
        .expect("the member's actor creation parks");
    tokio::time::sleep_until(deadline + STILL_PENDING).await;
    assert!(
        !within.is_finished(),
        "the legacy call waits for the settlement past the deadline"
    );
    creation.release_all();
    let error = tokio::time::timeout(STEP, within)
        .await
        .expect("the legacy call returns once the setup settles")
        .expect("legacy task")
        .expect_err("an expired setup is not held");
    assert!(
        matches!(
            error,
            MobError::HeldSpawnExpired { ref member, cleanup: None } if member == "held-worker"
        ),
        "expected HeldSpawnExpired with its cleanup confirmed, got {error}"
    );
    assert!(!in_roster(&handle, "held-worker").await);
    identity_is_free(&handle, "held-worker").await;
}

/// The legacy `spawn_held_within` keeps an actual settlement error as it
/// is: here the parked actor creation fails after the deadline.
#[tokio::test]
async fn the_legacy_held_spawn_keeps_an_actual_settlement_error() {
    let (handle, service) = holding_mob(None, SelectingHost::default()).await;
    let creation = service.park_next_session_creation();
    let acquisitions = service.turn_finalization_sessions().len();
    let deadline = deadline_in(PARKED_DEADLINE);
    let within = tokio::spawn({
        let handle = handle.clone();
        async move {
            handle
                .spawn_held_within(worker_spec("held-worker"), deadline)
                .await
        }
    });
    tokio::time::timeout(STEP, creation.boundary_entered.notified())
        .await
        .expect("the member's actor creation parks");
    let session_id = service
        .turn_finalization_sessions()
        .get(acquisitions)
        .cloned()
        .expect("the member's session took its turn-finalization boundary");
    service.set_create_session_failure(&session_id).await;
    tokio::time::sleep_until(deadline + STILL_PENDING).await;
    creation.release_all();
    let error = tokio::time::timeout(STEP, within)
        .await
        .expect("the legacy call returns once the setup settles")
        .expect("legacy task")
        .expect_err("a failed creation is not held");
    assert!(
        error.to_string().contains("mock create_session failure"),
        "expected the creation's own failure, got {error}"
    );
    assert!(!in_roster(&handle, "held-worker").await);
    identity_is_free(&handle, "held-worker").await;
}

/// A held spawn whose bound ended while its command waited for the actor
/// stops at the actor's first check: no preparation task is started for it.
#[tokio::test]
async fn a_bound_ended_while_the_command_waited_for_the_actor_starts_no_preparation() {
    let handle = create_holding_mob().await;
    let preparation_settled = Arc::new(tokio::sync::Notify::new());
    arm_spawn_preparation_gate_for_test(
        &AgentIdentity::from("held-queued-worker"),
        crate::runtime::actor::spawn_preparation::SpawnPreparationTestGate {
            settled: Some(Arc::clone(&preparation_settled)),
            ..Default::default()
        },
    );
    let deadline = deadline_in(Duration::from_millis(300));
    let mut start = Box::pin(handle.spawn_held_until(worker_spec("held-queued-worker"), deadline));
    // One poll admits the command. The actor runs on this thread, so blocking
    // the thread keeps the command waiting for the actor past the deadline.
    assert!(
        futures::poll!(&mut start).is_pending(),
        "the caller waits for its member"
    );
    std::thread::sleep(deadline.saturating_duration_since(Instant::now()) + STILL_PENDING);
    // The actor now handles the command, after the deadline, and the timer
    // driver catches up with the elapsed deadline.
    tokio::time::sleep(STILL_PENDING).await;
    let start = tokio::time::timeout(STEP, start)
        .await
        .expect("the caller answers at once")
        .expect("the spawn is admitted");
    let HeldSpawnStart::Unsettled(settlement) = start else {
        panic!("a member whose command waited past the deadline is not held");
    };
    tokio::time::timeout(STEP, settlement.settled())
        .await
        .expect("the setup settles")
        .expect("it expired before any preparation started");
    assert!(
        tokio::time::timeout(STILL_PENDING, preparation_settled.notified())
            .await
            .is_err(),
        "no preparation task ran for it"
    );
    identity_is_free(&handle, "held-queued-worker").await;
}

/// A held spawn whose bound ended while its preparation ran off the loop
/// stops once the preparation settles, before the actor stages anything for
/// it in the machine.
#[tokio::test]
async fn a_bound_ended_during_the_preparation_stops_before_anything_is_staged() {
    let handle = create_holding_mob().await;
    let (entered_tx, mut entered_rx) = tokio::sync::mpsc::unbounded_channel();
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    arm_spawn_preparation_gate_for_test(
        &AgentIdentity::from("held-preparing-worker"),
        crate::runtime::actor::spawn_preparation::SpawnPreparationTestGate {
            entered: Some(entered_tx),
            release: Some(Arc::clone(&release)),
            settled: None,
        },
    );
    let start = start_held(
        &handle,
        "held-preparing-worker",
        deadline_in(PARKED_DEADLINE),
    );
    tokio::time::timeout(STEP, entered_rx.recv())
        .await
        .expect("the preparation runs off the loop")
        .expect("entered channel");
    // Every machine fact the actor's inline steps published is seen; a staged
    // spawn would publish another.
    let mut machine = handle.machine_state_watch_rx.clone();
    drop(machine.borrow_and_update());
    let settlement = unsettled_at_the_deadline(start).await;
    let settled = settlement.settled();
    tokio::pin!(settled);
    assert!(
        tokio::time::timeout(STILL_PENDING, &mut settled)
            .await
            .is_err(),
        "the setup settles only once its preparation returns"
    );
    release.add_permits(1);
    tokio::time::timeout(STEP, settled)
        .await
        .expect("the setup settles once its preparation returns")
        .expect("it expired before anything was staged");
    assert!(
        !machine.has_changed().expect("the mob actor is alive"),
        "no spawn was staged in the machine"
    );
    identity_is_free(&handle, "held-preparing-worker").await;
}

/// The provisioning task's preparation-permit wait is bounded too: a held
/// spawn that passed its preparation and was staged, but whose provisioning
/// task still waits for a permit at the deadline, settles while every permit
/// stays held, and its member is never provisioned.
#[tokio::test]
async fn a_provisioning_permit_wait_past_the_deadline_settles_while_every_permit_is_held() {
    let (handle, service) = holding_mob(None, SelectingHost::default()).await;
    let permits = crate::runtime::actor::spawn_preparation::SPAWN_PREPARATION_CONCURRENCY;
    let acquisitions = service.turn_finalization_sessions().len();
    let held_identity = AgentIdentity::from("held-provision-worker");
    let (held_entered_tx, mut held_entered_rx) = tokio::sync::mpsc::unbounded_channel();
    let held_release = Arc::new(tokio::sync::Semaphore::new(0));
    arm_spawn_preparation_gate_for_test(
        &held_identity,
        crate::runtime::actor::spawn_preparation::SpawnPreparationTestGate {
            entered: Some(held_entered_tx),
            release: Some(Arc::clone(&held_release)),
            settled: None,
        },
    );
    let start = start_held(
        &handle,
        "held-provision-worker",
        deadline_in(PARKED_DEADLINE),
    );
    tokio::time::timeout(STEP, held_entered_rx.recv())
        .await
        .expect("the held member's preparation holds a permit")
        .expect("entered channel");
    // Fillers take every other permit, and one more waits for a permit.
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let (entered_tx, mut entered_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut fillers = Vec::new();
    for index in 0..permits {
        let name = format!("held-provision-filler-{index}");
        arm_spawn_preparation_gate_for_test(
            &AgentIdentity::from(name.as_str()),
            crate::runtime::actor::spawn_preparation::SpawnPreparationTestGate {
                entered: Some(entered_tx.clone()),
                release: Some(Arc::clone(&release)),
                settled: None,
            },
        );
        let spec = worker_spec(&name);
        fillers.push(
            handle
                .enqueue_actor_command_for_test(|reply_tx| MobCommand::Spawn {
                    spec: Box::new(spec),
                    spawn_source: crate::runtime::SpawnSource::Consumer,
                    owner_bridge_session_id: None,
                    ops_registry: None,
                    held: None,
                    reply_tx,
                })
                .await
                .expect("filler spawn enqueue"),
        );
    }
    for _ in 1..permits {
        tokio::time::timeout(STEP, entered_rx.recv())
            .await
            .expect("a filler preparation holds a permit")
            .expect("entered channel");
    }
    // FIFO: the census answers after every filler's preparation was started,
    // and one yield lets the last one queue for a permit.
    let census = spawn_preparation_census_for_test(&handle, &held_identity).await;
    assert_eq!(census.preparation_permits_in_use, permits);
    tokio::task::yield_now().await;
    // The held preparation finishes, and the waiting filler takes its permit
    // (the semaphore is fair).
    held_release.add_permits(1);
    tokio::time::timeout(STEP, entered_rx.recv())
        .await
        .expect("the waiting filler takes the released permit")
        .expect("entered channel");
    // The held spawn is staged, and its provisioning task waits for a permit.
    tokio::time::timeout(STEP, async {
        while handle
            .debug_orchestrator_snapshot()
            .await
            .expect("orchestrator snapshot")
            .pending_spawn_count
            != 1
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the held spawn is staged");
    let settlement = unsettled_at_the_deadline(start).await;
    tokio::time::timeout(STEP, settlement.settled())
        .await
        .expect("the setup settles without waiting for a permit")
        .expect("it expired with its staging unwound");
    let census = spawn_preparation_census_for_test(&handle, &held_identity).await;
    assert_eq!(
        census.preparation_permits_in_use, permits,
        "every permit is still held by a filler"
    );
    assert!(!census.identity_materialization_in_flight);
    assert_eq!(
        service.turn_finalization_sessions().len(),
        acquisitions,
        "the member was never provisioned"
    );
    release.add_permits(permits);
    for filler in fillers {
        tokio::time::timeout(STEP, filler)
            .await
            .expect("every filler spawn completes")
            .expect("filler reply")
            .expect("filler spawn");
    }
    identity_is_free(&handle, "held-provision-worker").await;
}

/// The pre-create check fires on its own: a held setup that passed its
/// after-boundary check before the deadline, and is held past the deadline
/// after preparing under B, stops before its owned create. No owned create
/// starts (the registration-witness park point, armed as a probe, is never
/// reached), the host is never asked for the member's destination, and no
/// actor is created.
#[tokio::test]
async fn a_bound_ended_after_the_boundary_check_stops_before_the_owned_create() {
    // The host answers at once; the gate only counts its destination
    // selections.
    let destinations = Arc::new(TestRuntimeControlBarrier::new());
    destinations.release_all();
    let (handle, service) = holding_mob(
        None,
        SelectingHost::parking_destinations(Arc::clone(&destinations)),
    )
    .await;
    let deadline = deadline_in(PARKED_DEADLINE);
    let blocked = Arc::new(AtomicBool::new(false));
    // Emitted after the after-boundary check and before the pre-create check.
    let guard = block_once_at_event(
        "SessionBackend::provision_member stamped eager turn metadata",
        deadline + STILL_PENDING,
        Arc::clone(&blocked),
    );
    // B is held until the probe is armed for the member's session.
    let boundary = service.install_non_reentrant_turn_finalization_gate();
    let boundary_held = Arc::clone(&boundary).lock_owned().await;
    let acquisitions = service.turn_finalization_sessions().len();
    let start = start_held(&handle, "held-worker", deadline);
    let session_id = tokio::time::timeout(STEP, async {
        loop {
            if let Some(session_id) = service.turn_finalization_sessions().get(acquisitions) {
                break session_id.clone();
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the setup waits for its turn-finalization boundary");
    // A probe: it reports an owned create that reaches its registration
    // witness, and never parks it (its release is already gone).
    let (mut owned_create_started, release) =
        crate::runtime::provisioner::arm_owned_create_before_registration_witness_for_test(
            session_id,
        );
    drop(release);
    drop(boundary_held);
    let settlement = unsettled_at_the_deadline(start).await;
    tokio::time::timeout(STEP, settlement.settled())
        .await
        .expect("the setup settles")
        .expect("it expired before its owned create");
    drop(guard);
    assert!(
        blocked.load(Ordering::Acquire),
        "the setup was held past its deadline after preparing under B"
    );
    assert!(
        matches!(
            owned_create_started.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ),
        "no owned create started"
    );
    assert_eq!(
        destinations.boundary_calls.load(Ordering::Acquire),
        0,
        "the host was never asked for the member's destination"
    );
    assert_eq!(
        service.max_concurrent_create_session_calls(),
        0,
        "no actor was created"
    );
    assert!(!in_roster(&handle, "held-worker").await);
    identity_is_free(&handle, "held-worker").await;
}

/// Deadline first by the clock: a member already held when the runtime
/// thread resumes past the deadline, before the deadline's timer has fired,
/// is not delivered. The caller gets the pending settlement, never a held
/// member after its deadline.
#[tokio::test]
async fn a_hold_ready_when_the_runtime_resumes_past_the_deadline_is_not_delivered() {
    let (handle, service) = holding_mob(None, SelectingHost::default()).await;
    let deadline = deadline_in(PARKED_DEADLINE);
    let mut start = Box::pin(handle.spawn_held_until(worker_spec("held-worker"), deadline));
    assert!(
        futures::poll!(&mut start).is_pending(),
        "the caller waits for its member"
    );
    tokio::time::timeout(STEP, async {
        while service.controller_route_queries.load(Ordering::Acquire) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the member reaches its hold and is sent to the caller");
    // The runtime thread is blocked past the deadline, and the caller is
    // polled again without yielding first, so the deadline's timer has not
    // fired yet while the hold is ready.
    std::thread::sleep(deadline.saturating_duration_since(Instant::now()) + STILL_PENDING);
    match futures::poll!(&mut start) {
        std::task::Poll::Ready(Ok(HeldSpawnStart::Unsettled(settlement))) => {
            tokio::time::timeout(STEP, settlement.settled())
                .await
                .expect("the undelivered hold settles")
                .expect("it expired with its cleanup confirmed");
        }
        std::task::Poll::Ready(Ok(HeldSpawnStart::Held(held))) => {
            panic!("{} was delivered after its deadline", held.identity())
        }
        other => panic!("expected the pending settlement at once, got {other:?}"),
    }
    assert!(!in_roster(&handle, "held-worker").await);
    identity_is_free(&handle, "held-worker").await;
}

/// Deadline first by the clock in the fallback too: a spawn that ended before
/// its hold, whose caller is polled again only past the deadline and before
/// the deadline's timer has fired, returns the pending settlement, never the
/// completion's error. The settlement keeps that actual error.
#[tokio::test]
async fn an_ended_spawn_seen_only_past_the_deadline_returns_the_pending_settlement() {
    let handle = create_holding_mob_answering(Some(MockControllerRouteAnswer::ActorLost)).await;
    let identity = AgentIdentity::from("held-worker");
    let deadline = deadline_in(PARKED_DEADLINE);
    let mut start = Box::pin(handle.spawn_held_until(worker_spec("held-worker"), deadline));
    assert!(
        futures::poll!(&mut start).is_pending(),
        "the caller waits for its member"
    );
    // FIFO: each census is answered after the spawn command; the spawn fails
    // before its hold and has settled once its identity is no longer in
    // flight.
    tokio::time::timeout(STEP, async {
        while spawn_preparation_census_for_test(&handle, &identity)
            .await
            .identity_materialization_in_flight
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the spawn ends before its hold");
    std::thread::sleep(deadline.saturating_duration_since(Instant::now()) + STILL_PENDING);
    match futures::poll!(&mut start) {
        std::task::Poll::Ready(Ok(HeldSpawnStart::Unsettled(settlement))) => {
            let error = tokio::time::timeout(STEP, settlement.settled())
                .await
                .expect("the settlement is already there")
                .expect_err("the setup settled with its own failure");
            assert!(
                error
                    .to_string()
                    .contains("lost its actor before its controller route was observed"),
                "{error}"
            );
        }
        other => panic!("expected the pending settlement at once, got {other:?}"),
    }
    identity_is_free(&handle, "held-worker").await;
}

/// ADR P2-A: a deadline that passes while the owned create waits for its
/// registration witness stops the setup before the host's destination
/// callback; its awaited abort's outcome is kept.
#[tokio::test]
async fn a_deadline_passed_during_the_registration_witness_wait_starts_no_host_callback() {
    let destinations = Arc::new(TestRuntimeControlBarrier::new());
    destinations.release_all();
    let (handle, service) = holding_mob(
        None,
        SelectingHost::parking_destinations(Arc::clone(&destinations)),
    )
    .await;
    let deadline = deadline_in(PARKED_DEADLINE);
    let (mut start, release) = raw_start_parked_at_the_registration_witness(
        &handle,
        &service,
        "held-witness-expiry",
        deadline,
    )
    .await;
    tokio::time::sleep_until(deadline + STILL_PENDING).await;
    let _ = release.send(());
    let completion = start.completion().await;
    assert!(
        matches!(
            completion,
            Err(MobError::HeldSpawnExpired { ref member, cleanup: None })
                if member == "held-witness-expiry"
        ),
        "expected HeldSpawnExpired with its cleanup confirmed, got {completion:?}"
    );
    assert_eq!(
        destinations.boundary_calls.load(Ordering::Acquire),
        0,
        "the host's destination callback never started"
    );
    assert_eq!(service.max_concurrent_create_session_calls(), 0);
    drop(start);
    identity_is_free(&handle, "held-witness-expiry").await;
}

/// ADR P2-A: a caller lost while the owned create waits for its registration
/// witness stops the setup before the host's destination callback; its
/// awaited abort's outcome is kept.
#[tokio::test]
async fn a_caller_lost_during_the_registration_witness_wait_starts_no_host_callback() {
    let destinations = Arc::new(TestRuntimeControlBarrier::new());
    destinations.release_all();
    let (handle, service) = holding_mob(
        None,
        SelectingHost::parking_destinations(Arc::clone(&destinations)),
    )
    .await;
    let (mut start, release) = raw_start_parked_at_the_registration_witness(
        &handle,
        &service,
        "held-witness-caller",
        deadline_in(Duration::from_mins(10)),
    )
    .await;
    start.lose_caller();
    let _ = release.send(());
    let completion = start.completion().await;
    assert!(
        matches!(
            completion,
            Err(MobError::HeldSpawnAborted { ref member, cleanup: None })
                if member == "held-witness-caller"
        ),
        "expected HeldSpawnAborted with its cleanup confirmed, got {completion:?}"
    );
    assert_eq!(
        destinations.boundary_calls.load(Ordering::Acquire),
        0,
        "the host's destination callback never started"
    );
    assert_eq!(service.max_concurrent_create_session_calls(), 0);
    drop(start);
    identity_is_free(&handle, "held-witness-caller").await;
}

/// ADR P2-B: a hold whose delivery is refused after its route-query check,
/// once the deadline has passed, is classified at the refusal, deadline
/// first: the exact typed result is HeldSpawnExpired, not HeldSpawnAborted.
#[tokio::test]
async fn a_delivery_refused_after_the_deadline_is_classified_as_an_expiry() {
    let handle = create_holding_mob().await;
    let deadline = deadline_in(PARKED_DEADLINE);
    let (entered, release) =
        crate::runtime::held_spawn::arm_held_ready_delivery_for_test("held-refused-expiry");
    let mut start = start_held_raw(&handle, "held-refused-expiry", deadline).await;
    tokio::time::timeout(STEP, entered)
        .await
        .expect("the hold passed its route-query check before the deadline")
        .expect("entered");
    tokio::time::sleep_until(deadline + STILL_PENDING).await;
    // At its deadline the caller's side goes away, as the public start's does.
    start.lose_caller();
    let _ = release.send(());
    let completion = start.completion().await;
    assert!(
        matches!(
            completion,
            Err(MobError::HeldSpawnExpired { ref member, cleanup: None })
                if member == "held-refused-expiry"
        ),
        "expected HeldSpawnExpired, got {completion:?}"
    );
    drop(start);
    identity_is_free(&handle, "held-refused-expiry").await;
}

/// A hold whose delivery is refused before the deadline because its caller
/// went away stays an abort: the exact typed result is HeldSpawnAborted.
#[tokio::test]
async fn a_delivery_refused_by_a_lost_caller_before_the_deadline_stays_an_abort() {
    let handle = create_holding_mob().await;
    let (entered, release) =
        crate::runtime::held_spawn::arm_held_ready_delivery_for_test("held-refused-caller");
    let mut start = start_held_raw(
        &handle,
        "held-refused-caller",
        deadline_in(Duration::from_mins(10)),
    )
    .await;
    tokio::time::timeout(STEP, entered)
        .await
        .expect("the hold passed its route-query check")
        .expect("entered");
    start.lose_caller();
    let _ = release.send(());
    let completion = start.completion().await;
    assert!(
        matches!(
            completion,
            Err(MobError::HeldSpawnAborted { ref member, cleanup: None })
                if member == "held-refused-caller"
        ),
        "expected HeldSpawnAborted, got {completion:?}"
    );
    drop(start);
    identity_is_free(&handle, "held-refused-caller").await;
}

/// An owned create stopped at its bound keeps a failed abort's error: with
/// the abort's failure forced after the host hook returned past the deadline,
/// the exact typed result is HeldSpawnExpired with `cleanup: Some(..)`.
#[tokio::test]
async fn an_owned_create_stop_whose_abort_fails_keeps_the_cleanup_error() {
    let destination = Arc::new(TestRuntimeControlBarrier::new());
    let (handle, service) = holding_mob(
        None,
        SelectingHost::parking_destinations(Arc::clone(&destination)),
    )
    .await;
    let acquisitions = service.turn_finalization_sessions().len();
    let deadline = deadline_in(PARKED_DEADLINE);
    let mut start = start_held_raw(&handle, "held-abort-worker", deadline).await;
    tokio::time::timeout(STEP, destination.boundary_entered.notified())
        .await
        .expect("the host's destination selection parks");
    let session_id = service
        .turn_finalization_sessions()
        .get(acquisitions)
        .cloned()
        .expect("the member's session took its turn-finalization boundary");
    crate::runtime::provisioner::arm_owned_create_abort_failure_for_test(session_id);
    tokio::time::sleep_until(deadline + STILL_PENDING).await;
    destination.release_all();
    match start.completion().await {
        Err(MobError::HeldSpawnExpired {
            member,
            cleanup: Some(cleanup),
        }) => {
            assert_eq!(member, "held-abort-worker");
            assert!(
                cleanup
                    .to_string()
                    .contains("test-forced owned create abort failure"),
                "{cleanup}"
            );
        }
        other => panic!("expected an expiry keeping its cleanup error, got {other:?}"),
    }
    drop(start);
    identity_is_free(&handle, "held-abort-worker").await;
}
