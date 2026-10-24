//! Governed ingress through the mob: an embedder submits an authenticated
//! scoped original, already bound by `bind_scoped_original`, to one mob
//! member through `MobHandle`. The member's session runs on a machine with
//! the real scoped grant composition (scoped original check, host invocation
//! policy and required scoped destination). The mob forwards the bound input
//! unchanged; admission stays native and its refusals stay typed.
#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod governed_support;

use governed_support::*;
use meerkat_runtime::input_admission_custody::{
    NativeInputAdmissionCompletion, NativeInputAdmissionCustody,
    NativeInputAdmissionSettlement as Settlement,
};

type Settlements = Arc<Mutex<Vec<Settlement>>>;

/// A host resource owner that records each settlement it receives.
struct RecordingCustody(Settlements);

impl NativeInputAdmissionCustody for RecordingCustody {
    fn settle(self: Box<Self>, settlement: Settlement) -> Option<NativeInputAdmissionCompletion> {
        self.0.lock().expect("settlements").push(settlement);
        None
    }
}

fn custody() -> (Box<dyn NativeInputAdmissionCustody>, Settlements) {
    let settled = Settlements::default();
    (Box::new(RecordingCustody(Arc::clone(&settled))), settled)
}

fn settled(log: &Settlements) -> Vec<Settlement> {
    log.lock().expect("settlements").clone()
}

/// A bound governed input submitted through `MobHandle` is admitted to the
/// bound member's session and runs a native turn there, with its claims
/// (contributors included) exactly as the embedder bound them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bound_governed_input_is_admitted_to_the_bound_member_and_runs() {
    let fixture = Fixture::new().await;
    let input = fixture.bound(|_| {});
    let input_id = input.id().clone();
    let association = input
        .header()
        .authority_association
        .clone()
        .expect("bound claims");
    let turn = fixture
        .handle
        .start_governed_input(&fixture.member, input)
        .await
        .expect("native admission accepts the bound original");
    assert_eq!(turn.session_id(), Some(&fixture.session_id));
    assert_eq!(turn.receipt().identity, fixture.member);
    tokio::time::timeout(WAIT, turn.wait())
        .await
        .expect("the governed turn completes")
        .expect("the governed turn succeeds");
    let requests = fixture.provider.requests();
    assert_eq!(requests.len(), 1, "one native controller request");
    assert!(mentions_prompt(&requests[0]));
    let stored = fixture
        .machine
        .input_state(&fixture.session_id, &input_id)
        .await
        .expect("native row query")
        .expect("the member session admitted exactly this input");
    assert_eq!(stored.state.authority_contributors.len(), 1);
    let retained = stored.state.authority_contributors[0].association();
    assert_eq!(stored.state.authority_contributors[0].input_id(), &input_id);
    assert!(
        retained == &association,
        "the mob neither minted, altered nor dropped any claim"
    );
    assert_eq!(
        retained.candidate().contributing_work,
        sources(),
        "every contributor arrives as a conjunct"
    );

    // A retry of the same original event through the mob is recognised by
    // the native replay lookup: it settles without a second turn.
    let retry = fixture
        .handle
        .start_governed_input(
            &fixture.member,
            fixture.bound_under(|_| {}, "retry-transport-authentication"),
        )
        .await
        .expect("the retry is answered by native admission");
    tokio::time::timeout(WAIT, retry.wait())
        .await
        .expect("the retry settles")
        .expect("the retry reports the original's completion");
    assert_eq!(
        fixture.provider.requests().len(),
        1,
        "a replayed original runs no second turn"
    );
}

/// The handle carries the member session's canonical admission: an accepted
/// governed input reports the id the session admitted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_accepted_governed_input_reports_its_canonical_admission() {
    let fixture = Fixture::new().await;
    let input = fixture.bound(|_| {});
    let input_id = input.id().clone();
    let turn = fixture
        .handle
        .start_governed_input(&fixture.member, input)
        .await
        .expect("native admission accepts the bound original");
    let admission = turn.admission().expect("governed admission is forwarded");
    assert_eq!(admission.session_id, fixture.session_id);
    assert_eq!(admission.input_id, input_id);
    assert!(!admission.deduplicated);
    fixture
        .machine
        .input_state(&fixture.session_id, &admission.input_id)
        .await
        .expect("native row query")
        .expect("the admitted id names the session's row");
    tokio::time::timeout(WAIT, turn.wait())
        .await
        .expect("the governed turn completes")
        .expect("the governed turn succeeds");
}

/// A replay of the same original reports the deduplication and the existing
/// admitted id, not the replay's own id.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_replayed_governed_input_reports_the_existing_admission() {
    let fixture = Fixture::new().await;
    let original = fixture.bound(|_| {});
    let original_id = original.id().clone();
    let turn = fixture
        .handle
        .start_governed_input(&fixture.member, original)
        .await
        .expect("native admission accepts the bound original");
    tokio::time::timeout(WAIT, turn.wait())
        .await
        .expect("the governed turn completes")
        .expect("the governed turn succeeds");

    let replay = fixture.bound_under(|_| {}, "retry-transport-authentication");
    let replay_id = replay.id().clone();
    assert_ne!(replay_id, original_id, "the replay carries its own id");
    let retry = fixture
        .handle
        .start_governed_input(&fixture.member, replay)
        .await
        .expect("the retry is answered by native admission");
    let admission = retry.admission().expect("governed admission is forwarded");
    assert_eq!(admission.session_id, fixture.session_id);
    assert!(admission.deduplicated);
    assert_eq!(admission.input_id, original_id);
    tokio::time::timeout(WAIT, retry.wait())
        .await
        .expect("the retry settles")
        .expect("the retry reports the original's completion");
}

/// The host's custody is carried to the member session's native admission:
/// an accepted governed input settles it `Admitted`, exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_admitted_governed_input_settles_the_host_custody_admitted() {
    let fixture = Fixture::new().await;
    let (custody, log) = custody();
    let input = fixture.bound(|_| {});
    let input_id = input.id().clone();
    let turn = fixture
        .handle
        .start_governed_input_with_custody(&fixture.member, input, custody)
        .await
        .expect("native admission accepts the bound original");
    assert_eq!(settled(&log), vec![Settlement::Admitted]);
    let admission = turn.admission().expect("governed admission is forwarded");
    assert_eq!(admission.input_id, input_id);
    assert!(!admission.deduplicated);
    tokio::time::timeout(WAIT, turn.wait())
        .await
        .expect("the governed turn completes")
        .expect("the governed turn succeeds");
    assert_eq!(settled(&log), vec![Settlement::Admitted], "settled once");
}

/// A governed submission dropped before it is first polled settles the
/// host custody `NotAdmitted` exactly once and dispatches nothing, from
/// both public entry points.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_never_polled_governed_submission_settles_the_host_custody_not_admitted() {
    let fixture = Fixture::new().await;
    let (mob_custody, mob_log) = custody();
    let unpolled = fixture.handle.start_governed_input_with_custody(
        &fixture.member,
        fixture.bound(|_| {}),
        mob_custody,
    );
    drop(unpolled);
    assert_eq!(settled(&mob_log), vec![Settlement::NotAdmitted]);

    let member = fixture
        .handle
        .member(&fixture.member)
        .await
        .expect("member handle");
    let (member_custody, member_log) = custody();
    let unpolled = member.start_governed_input_with_custody(
        fixture.bound_under(|_| {}, "unpolled-transport-authentication"),
        member_custody,
    );
    drop(unpolled);
    assert_eq!(settled(&member_log), vec![Settlement::NotAdmitted]);

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        settled(&mob_log),
        vec![Settlement::NotAdmitted],
        "settled once"
    );
    assert_eq!(
        settled(&member_log),
        vec![Settlement::NotAdmitted],
        "settled once"
    );
    assert_eq!(fixture.adapter.gate.entered(), 0, "no native admission");
    assert!(fixture.provider.requests().is_empty(), "nothing ran");
}

/// A native refusal settles the custody `NotAdmitted`, and so does a
/// governed input the mob refuses before any native admission.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_governed_input_settles_the_host_custody_not_admitted() {
    let fixture = Fixture::new().await;
    let mut foreign = fixture.target.clone();
    foreign.context = id("another-context");
    *fixture.adapter.target.lock().expect("fixture") = Some(foreign.clone());
    let (refused_custody, refused) = custody();
    let Err(error) = fixture
        .handle
        .start_governed_input_with_custody(
            &fixture.member,
            fixture.bound(|candidate| candidate.target = foreign.clone()),
            refused_custody,
        )
        .await
    else {
        panic!("native admission refuses the other destination")
    };
    assert!(
        matches!(error, MobError::MemberInputRefused { .. }),
        "{error:?}"
    );
    assert_eq!(settled(&refused), vec![Settlement::NotAdmitted]);

    let member = fixture
        .handle
        .member(&fixture.member)
        .await
        .expect("member handle");
    let (unbound_custody, unbound) = custody();
    let Err(error) = member
        .start_governed_input_with_custody(
            Input::Prompt(PromptInput::new(PROMPT, None)),
            unbound_custody,
        )
        .await
    else {
        panic!("an unbound input is not governed input")
    };
    assert!(
        matches!(error, MobError::GovernedInputUnbound { .. }),
        "{error:?}"
    );
    assert_eq!(settled(&unbound), vec![Settlement::NotAdmitted]);
    assert!(fixture.provider.requests().is_empty(), "nothing ran");
}

/// A failure inside the native admission attempt leaves its outcome
/// unknown: the custody is settled `Uncertain` and the mob never replaces
/// that with `NotAdmitted`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unknown_admission_outcome_settles_the_host_custody_uncertain() {
    let fixture = Fixture::new().await;
    // The third custody read is the one inside the admission attempt.
    fixture
        .adapter
        .panic_at_custody_read
        .store(3, std::sync::atomic::Ordering::SeqCst);
    let (custody, log) = custody();
    let outcome = tokio::time::timeout(
        WAIT,
        fixture.handle.start_governed_input_with_custody(
            &fixture.member,
            fixture.bound(|_| {}),
            custody,
        ),
    )
    .await
    .expect("the failed admission is answered");
    assert!(outcome.is_err(), "{:?}", outcome.map(drop));
    assert_eq!(
        fixture.adapter.gate.entered(),
        3,
        "the failure was inside the admission attempt"
    );
    assert_eq!(settled(&log), vec![Settlement::Uncertain]);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        settled(&log),
        vec![Settlement::Uncertain],
        "an uncertain admission is never withdrawn"
    );
}

/// A governed input the adapter authenticated for another destination is
/// refused by the member session's scoped destination binding. The mob
/// surfaces the native verdict typed and unchanged, and runs nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_governed_input_naming_another_destination_is_refused_typed() {
    let fixture = Fixture::new().await;
    let mut foreign = fixture.target.clone();
    foreign.context = id("another-context");
    // The adapter authenticated the original for the other destination, so
    // only the member session's own bound destination decides.
    *fixture.adapter.target.lock().expect("fixture") = Some(foreign.clone());
    let Err(direct) = fixture
        .machine
        .accept_input_with_completion(
            &fixture.session_id,
            fixture.bound(|candidate| candidate.target = foreign.clone()),
        )
        .await
    else {
        panic!("native admission refuses the other destination")
    };
    let Err(error) = fixture
        .handle
        .start_governed_input(
            &fixture.member,
            fixture.bound(|candidate| candidate.target = foreign.clone()),
        )
        .await
    else {
        panic!("the mob surfaces the refusal")
    };
    let MobError::MemberInputRefused { session_id, error } = error else {
        panic!("the native verdict stays typed: {error:?}")
    };
    assert_eq!(session_id, fixture.session_id);
    assert!(matches!(
        error,
        RuntimeDriverError::InputRefused { .. }
            | RuntimeDriverError::ControllerReadinessUnavailable { .. }
            | RuntimeDriverError::ValidationFailed { .. }
    ));
    assert_eq!(
        format!("{error:?}"),
        format!("{direct:?}"),
        "the mob returns the runtime's own verdict unchanged"
    );
    assert!(fixture.provider.requests().is_empty(), "nothing ran");
}

/// An ordinary ungoverned send to the bound member is still refused, as the
/// required scoped destination demands, and the governed lane never turns an
/// unbound input into one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_ordinary_send_to_a_bound_member_is_still_refused() {
    let fixture = Fixture::new().await;
    let member = fixture
        .handle
        .member(&fixture.member)
        .await
        .expect("member handle");
    let outcome = match member
        .start_turn(
            "an ordinary ungoverned request",
            HandlingMode::Queue,
            meerkat_mob::MemberTurnOptions::default(),
            None,
        )
        .await
    {
        Ok(turn) => tokio::time::timeout(WAIT, turn.wait())
            .await
            .expect("the ordinary turn settles")
            .map(drop),
        Err(error) => Err(error),
    };
    assert!(
        matches!(
            outcome,
            Err(MobError::MemberInputRefused { ref session_id, .. }) if session_id == &fixture.session_id
        ),
        "an ordinary send to a bound member is refused by its native admission: {outcome:?}"
    );

    let unbound = Input::Prompt(PromptInput::new(PROMPT, None));
    let Err(error) = member.start_governed_input(unbound).await else {
        panic!("an unbound input is not governed input")
    };
    assert!(
        matches!(error, MobError::GovernedInputUnbound { ref member_id } if member_id == &fixture.member),
        "{error:?}"
    );
    assert!(fixture.provider.requests().is_empty(), "nothing ran");
}

/// Governed input shares the member's existing admission lane with ordinary
/// work: it never overtakes an admission held in flight, it parks in FIFO
/// order across both kinds and counts against the lane bound, an abandoned
/// parked request is never admitted, an admitted run left unfinished
/// survives its dropped waiter, and another member's lane stays usable
/// throughout.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn governed_input_shares_the_member_admission_lane() {
    let fixture = Fixture::new().await;
    let other = AgentIdentity::from("ordinary-worker");
    fixture
        .handle
        .spawn_spec(SpawnMemberSpec::new("worker", other.clone()))
        .await
        .expect("spawn a second member");
    let parked = |handle: &MobHandle, member: &AgentIdentity| {
        handle
            .member_admission_backlog()
            .parked
            .get(member)
            .copied()
            .unwrap_or(0)
    };
    let governed = |input: Input| {
        let handle = fixture.handle.clone();
        let member = fixture.member.clone();
        tokio::spawn(async move { handle.start_governed_input(&member, input).await })
    };
    let ordinary = |label: String| {
        let handle = fixture.handle.clone();
        let member = fixture.member.clone();
        tokio::spawn(async move {
            handle
                .member(&member)
                .await?
                .start_turn(
                    label,
                    HandlingMode::Queue,
                    meerkat_mob::MemberTurnOptions::default(),
                    None,
                )
                .await
                .map(drop)
        })
    };

    // A: governed admission held in flight inside native admission.
    fixture.adapter.gate.hold();
    let first = governed(fixture.bound(|_| {}));
    wait_until("the first admission is held", || {
        fixture.adapter.gate.entered() == 1
    })
    .await;

    // C (ordinary) then B (governed retry) park behind A in FIFO order.
    let held_ordinary = ordinary("held ordinary request".to_string());
    wait_until("the ordinary request parks", || {
        parked(&fixture.handle, &fixture.member) == 1
    })
    .await;
    let retry = governed(fixture.bound_under(|_| {}, "retry-transport-authentication"));
    wait_until("the governed request parks behind it", || {
        parked(&fixture.handle, &fixture.member) == 2
    })
    .await;
    // D: a governed request whose caller leaves while it is parked.
    let abandoned = governed(fixture.bound_under(|_| {}, "abandoned-transport-authentication"));
    wait_until("the abandoned request parks", || {
        parked(&fixture.handle, &fixture.member) == 3
    })
    .await;
    abandoned.abort();

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        fixture.adapter.gate.entered(),
        1,
        "parked governed work never overtakes the held admission"
    );

    // Another member's lane is independent: its delivery is answered by its
    // own native admission while this member is held.
    let other_outcome = tokio::time::timeout(
        WAIT,
        fixture
            .handle
            .member(&other)
            .await
            .expect("member")
            .start_turn(
                "another member's request",
                HandlingMode::Queue,
                meerkat_mob::MemberTurnOptions::default(),
                None,
            ),
    )
    .await
    .expect("another member's lane answers while this one is held");
    assert!(
        matches!(other_outcome, Err(MobError::MemberInputRefused { .. })),
        "{:?}",
        other_outcome.map(drop)
    );

    // Fill the bounded lane with ordinary work: governed input is refused
    // typed by the same bound before any machine transition.
    let capacity = meerkat_mob::MEMBER_ADMISSION_LANE_CAPACITY;
    let fill = (3..capacity)
        .map(|n| ordinary(format!("fill {n}")))
        .collect::<Vec<_>>();
    wait_until("the lane is full", || {
        parked(&fixture.handle, &fixture.member) == capacity
    })
    .await;
    let Err(full) = fixture
        .handle
        .start_governed_input(
            &fixture.member,
            fixture.bound_under(|_| {}, "overflow-transport-authentication"),
        )
        .await
    else {
        panic!("a full lane refuses governed input")
    };
    assert!(
        matches!(full, MobError::MemberAdmissionBacklogFull { ref member_id, depth } if member_id == &fixture.member && depth == capacity),
        "{full:?}"
    );

    // Let only A through, with its run held at the provider. A is admitted
    // and its run stays unfinished. The lane then admits C, which native
    // admission refuses, and next starts B, which the gate holds inside
    // native admission. C has settled while B is still held, so C's admission
    // finished before B's began.
    fixture.provider.hold();
    fixture.adapter.gate.let_through("original-authentication");
    let first_turn = tokio::time::timeout(WAIT, first)
        .await
        .expect("the held admission settles")
        .expect("task")
        .expect("the held governed input is admitted");
    wait_until("the governed retry reaches native admission", || {
        fixture
            .adapter
            .gate
            .authentications()
            .iter()
            .any(|authentication| authentication == "retry-transport-authentication")
    })
    .await;
    // The lane is single-flight and B is held inside admission now, so C
    // can only settle here if its admission completed before B's began.
    let held_outcome = tokio::time::timeout(WAIT, held_ordinary)
        .await
        .expect("the ordinary request ahead of the held retry has settled")
        .expect("task");
    assert!(
        matches!(held_outcome, Err(MobError::MemberInputRefused { .. })),
        "{held_outcome:?}"
    );
    assert!(!retry.is_finished(), "the retry is still held in admission");

    // B is held inside native admission, which holds the member session's
    // mutation gate and driver lock. A's run dequeues under that same driver
    // lock, so it can reach the provider only once B's admission completes:
    // release the gate first. B is admitted after the work ahead of it, and
    // the provider still holds A's run unfinished.
    fixture.adapter.gate.release();
    let retry_turn = tokio::time::timeout(WAIT, retry)
        .await
        .expect("the parked governed retry settles")
        .expect("task")
        .expect("the governed retry is admitted after the work ahead of it");

    // A's run is unfinished at the provider when its waiter is dropped; the
    // admitted run still completes, which the retry's replay reports.
    wait_until("A's run reaches the provider", || {
        !fixture.provider.requests().is_empty()
    })
    .await;
    assert_eq!(
        fixture.provider.requests().len(),
        1,
        "A's held run is the only provider request"
    );
    drop(first_turn);
    fixture.provider.release();
    tokio::time::timeout(WAIT, retry_turn.wait())
        .await
        .expect("the replay settles")
        .expect("the replay reports the original run's completion");
    for delivery in fill {
        let outcome = tokio::time::timeout(WAIT, delivery)
            .await
            .expect("the fill drains")
            .expect("task");
        assert!(matches!(outcome, Err(MobError::MemberInputRefused { .. })));
    }
    assert_eq!(
        fixture.provider.requests().len(),
        1,
        "the original ran once; the replay ran no second turn"
    );
    let mut admitted = Vec::new();
    for authentication in fixture.adapter.gate.authentications() {
        if !admitted.contains(&authentication) {
            admitted.push(authentication);
        }
    }
    assert_eq!(
        admitted,
        ["original-authentication", "retry-transport-authentication"],
        "the held original, then its retry, reached native admission; the abandoned and the refused requests never did"
    );
    assert_eq!(parked(&fixture.handle, &fixture.member), 0);
}

/// A deferred session of `model` on `service`: its actor exists and no turn
/// runs.
fn deferred_session(model: &str) -> meerkat_core::service::CreateSessionRequest {
    meerkat_core::service::CreateSessionRequest {
        model: model.to_string(),
        prompt: "".into(),
        injected_context: Vec::new(),
        system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
        max_tokens: None,
        event_tx: None,
        initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
        deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::Discard,
        build: None,
        labels: None,
    }
}

/// A held member's controller route is observed inside its hold from the
/// exact client its actor pinned: the report names the member's actual
/// lowered route. After activation the same published actor's own pin
/// reports the same route, no model was called and nothing else was
/// constructed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_held_member_reports_the_route_its_published_actor_pins() {
    let mob = HoldingMob::new().await;
    let member = AgentIdentity::from(MEMBER);
    let held = tokio::time::timeout(
        WAIT,
        mob.handle.spawn_held_within(
            SpawnMemberSpec::new("worker", member.clone()),
            tokio::time::Instant::now() + WAIT,
        ),
    )
    .await
    .expect("the member reaches its hold point")
    .expect("the member is held");
    let report = held
        .controller_route_report()
        .report()
        .cloned()
        .expect("the actual pinned client's route report");
    assert_eq!(
        Some(report.key.clone()),
        model_facts().controller_route_key(),
        "the member's actual lowered route"
    );
    assert_eq!(report.model, MODEL);
    assert_eq!(report.endpoint, ENDPOINT);

    tokio::time::timeout(WAIT, held.activate())
        .await
        .expect("activation answers")
        .expect("the held member is published");
    let session_id = mob
        .handle
        .get_member(&member)
        .await
        .expect("roster read")
        .expect("the published member")
        .bridge_session_id()
        .cloned()
        .expect("session-backed member");
    let actor = mob
        .service
        .live_session_actor_witness(&session_id)
        .await
        .expect("the published actor");
    let pinned = mob
        .service
        .pin_controller_client_for_actor(&actor)
        .await
        .expect("the published actor's own pin");
    assert_eq!(
        meerkat::controller_route::controller_route_report(&pinned),
        Some(report)
    );
    assert!(mob.provider.requests().is_empty(), "no model was called");
    let sessions = meerkat_core::service::SessionService::list(&*mob.service, Default::default())
        .await
        .expect("session listing");
    assert_eq!(sessions.len(), 1, "nothing else was constructed");
}

/// The persistent session service observes an exact actor's route while the
/// caller already holds that session's runtime turn boundary, as a hold
/// does: it never reacquires the boundary. Once that actor is discarded its
/// witness is an error, never a report.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_persistent_actor_reports_its_route_under_the_held_boundary() {
    use meerkat_mob::MobSessionService;
    let root = tempfile::tempdir().expect("state root");
    let provider = Arc::new(ScriptedProvider::default());
    let mut builder =
        FactoryAgentBuilder::new(AgentFactory::minimal().comms(true), Config::default());
    builder.default_llm_client = Some(provider.clone());
    let store = Arc::new(meerkat_store::JsonlStore::new(root.path().join("sessions")));
    builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(store.clone())));
    let service = Arc::new(meerkat_session::PersistentSessionService::new(
        builder,
        4,
        store as Arc<dyn meerkat::SessionStore>,
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
        Arc::new(meerkat_store::MemoryBlobStore::default()),
    ));
    let session_id =
        meerkat_core::service::SessionService::create_session(&*service, deferred_session(MODEL))
            .await
            .expect("a deferred session")
            .session_id;
    let actor = service
        .live_session_actor_witness(&session_id)
        .await
        .expect("its live actor");
    let boundary =
        MobSessionService::acquire_runtime_turn_finalization_guard(&*service, &session_id)
            .await
            .expect("the runtime turn boundary, as a hold owns it");
    let observed = tokio::time::timeout(
        WAIT,
        service.controller_route_under_runtime_turn_boundary(&actor),
    )
    .await
    .expect("observed without reacquiring the held boundary")
    .expect("the exact actor's route");
    drop(boundary);
    assert_eq!(
        observed.report().map(|report| report.key.clone()),
        model_facts().controller_route_key()
    );

    meerkat_mob::MobSessionService::discard_live_session(&*service, &session_id)
        .await
        .expect("the actor is discarded");
    let stale = service
        .controller_route_under_runtime_turn_boundary(&actor)
        .await;
    assert!(
        matches!(
            stale,
            Err(meerkat_core::service::SessionError::NotFound { .. })
        ),
        "{stale:?}"
    );
    assert!(provider.requests().is_empty(), "no model was called");
}

/// An actor whose client reports no controller selection pins none: the
/// real service observes the typed `Unavailable`, never a substituted report
/// and never an error for that live actor.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_actor_without_controller_facts_reports_unavailable() {
    use meerkat_mob::MobSessionService;
    let mut builder =
        FactoryAgentBuilder::new(AgentFactory::minimal().comms(true), Config::default());
    builder.default_llm_client = Some(Arc::new(meerkat_llm_core::TestClient::for_provider(
        Provider::Anthropic,
    )));
    builder.default_session_store = Some(Arc::new(RecordingStore::default()));
    let service = Arc::new(EphemeralSessionService::new(builder, 4));
    let session_id =
        meerkat_core::service::SessionService::create_session(&*service, deferred_session(MODEL))
            .await
            .expect("a deferred session")
            .session_id;
    let actor = service
        .live_session_actor_witness(&session_id)
        .await
        .expect("its live actor");
    let observed = tokio::time::timeout(
        WAIT,
        service.controller_route_under_runtime_turn_boundary(&actor),
    )
    .await
    .expect("observed promptly")
    .expect("a live actor's answer");
    assert_eq!(
        observed,
        meerkat_mob::ControllerRouteObservation::Unavailable
    );
}

/// A persistent actor's controller is pinned while the caller already holds
/// that session's turn boundary, as an attachment owner pins it for a cold
/// consumer: the pin never reacquires the boundary and is the actor's actual
/// selection. Once that actor is discarded its witness is an error, never
/// another actor's pin.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_persistent_actor_pins_its_controller_under_the_held_boundary() {
    use meerkat_mob::MobSessionService;
    let root = tempfile::tempdir().expect("state root");
    let provider = Arc::new(ScriptedProvider::default());
    let mut builder =
        FactoryAgentBuilder::new(AgentFactory::minimal().comms(true), Config::default());
    builder.default_llm_client = Some(provider.clone());
    let store = Arc::new(meerkat_store::JsonlStore::new(root.path().join("sessions")));
    builder.default_session_store = Some(Arc::new(meerkat_store::StoreAdapter::new(store.clone())));
    let service = Arc::new(meerkat_session::PersistentSessionService::new(
        builder,
        4,
        store as Arc<dyn meerkat::SessionStore>,
        Arc::new(meerkat_runtime::InMemoryRuntimeStore::new()),
        Arc::new(meerkat_store::MemoryBlobStore::default()),
    ));
    let session_id =
        meerkat_core::service::SessionService::create_session(&*service, deferred_session(MODEL))
            .await
            .expect("a deferred session")
            .session_id;
    let actor = service
        .live_session_actor_witness(&session_id)
        .await
        .expect("its live actor");
    let boundary =
        MobSessionService::acquire_runtime_turn_finalization_guard(&*service, &session_id)
            .await
            .expect("the session's turn boundary, held by the caller");
    let pinned = tokio::time::timeout(
        WAIT,
        service.pin_controller_client_under_runtime_turn_boundary(&actor),
    )
    .await
    .expect("pinned without reacquiring the held boundary")
    .expect("the exact actor's controller");
    drop(boundary);
    assert!(pinned.selection() == &selection());

    MobSessionService::discard_live_session(&*service, &session_id)
        .await
        .expect("the actor is discarded");
    let stale = service
        .pin_controller_client_under_runtime_turn_boundary(&actor)
        .await
        .err();
    assert!(
        matches!(
            stale,
            Some(meerkat_core::service::SessionError::NotFound { .. })
        ),
        "{stale:?}"
    );
    assert!(provider.requests().is_empty(), "no model was called");
}

/// The ephemeral service pins an exact actor's controller under its own
/// held per-session boundary the same way, never reacquiring it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_ephemeral_actor_pins_its_controller_under_the_held_boundary() {
    use meerkat_mob::MobSessionService;
    let provider = Arc::new(ScriptedProvider::default());
    let mut builder =
        FactoryAgentBuilder::new(AgentFactory::minimal().comms(true), Config::default());
    builder.default_llm_client = Some(provider.clone());
    builder.default_session_store = Some(Arc::new(RecordingStore::default()));
    let service = Arc::new(EphemeralSessionService::new(builder, 4));
    let session_id =
        meerkat_core::service::SessionService::create_session(&*service, deferred_session(MODEL))
            .await
            .expect("a deferred session")
            .session_id;
    let actor = service
        .live_session_actor_witness(&session_id)
        .await
        .expect("its live actor");
    let boundary =
        MobSessionService::acquire_runtime_turn_finalization_guard(&*service, &session_id)
            .await
            .expect("the session's turn boundary, held by the caller");
    let pinned = tokio::time::timeout(
        WAIT,
        service.pin_controller_client_under_runtime_turn_boundary(&actor),
    )
    .await
    .expect("pinned without reacquiring the held boundary")
    .expect("the exact actor's controller");
    drop(boundary);
    assert!(pinned.selection() == &selection());
    assert!(provider.requests().is_empty(), "no model was called");
}

/// The member's executor attachment owner, which the machine retains, names
/// the member's exact live actor and lends the member service's own turn
/// boundary, the one its runtime loop holds across each turn: while the
/// owner's boundary is held, the service's boundary is held. Under it the
/// owner pins that actor's actual controller without reacquiring it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_member_attachment_owner_pins_its_actor_under_the_member_boundary() {
    use meerkat_mob::MobSessionService;
    let fixture = Fixture::new().await;
    let attachment = fixture
        .machine
        .current_executor_attachment_witness(&fixture.session_id)
        .await
        .expect("the member's attachment");
    let owner = fixture
        .machine
        .publication_handle_for_executor_attachment(&attachment)
        .await
        .expect("the attachment's owner");
    let actor = fixture
        .service
        .live_session_actor_witness(&fixture.session_id)
        .await
        .expect("the member's actor");
    assert!(
        owner
            .actor_identity()
            .is_some_and(|named| named.is_same_actor(&actor.executor_actor_identity())),
        "the owner names the member's actor"
    );
    let held = owner
        .turn_finalization_boundary()
        .expect("the owner's boundary")
        .acquire()
        .await
        .expect("the member's boundary");
    let waiting = MobSessionService::acquire_runtime_turn_finalization_guard(
        &*fixture.service,
        &fixture.session_id,
    );
    tokio::pin!(waiting);
    assert!(
        tokio::time::timeout(Duration::from_millis(200), &mut waiting)
            .await
            .is_err(),
        "the owner lent the member service's own boundary"
    );
    let pinned = tokio::time::timeout(WAIT, owner.pin_actor_controller_under_turn_boundary())
        .await
        .expect("pinned without reacquiring the boundary")
        .expect("the member actor's controller");
    assert!(pinned.selection() == &selection());
    drop(held);
    drop(
        tokio::time::timeout(WAIT, waiting)
            .await
            .expect("released with the owner's boundary")
            .expect("the boundary"),
    );
    assert!(
        fixture.provider.requests().is_empty(),
        "no model was called"
    );
}

/// A member whose profile does not admit external work.
async fn non_addressable_fixture() -> Fixture {
    Fixture::with_options(FixtureOptions {
        non_addressable_member: true,
        ..Default::default()
    })
    .await
}

/// External governed input to a member whose profile is not externally
/// addressable is refused by the MobMachine before native admission, from
/// both public external entries: nothing is admitted and the custody
/// settles `NotAdmitted` exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_governed_input_to_a_non_addressable_member_is_refused_before_admission() {
    let fixture = non_addressable_fixture().await;
    let (mob_custody, mob_log) = custody();
    let Err(error) = fixture
        .handle
        .start_governed_input_with_custody(&fixture.member, fixture.bound(|_| {}), mob_custody)
        .await
    else {
        panic!("external governed work to a non-addressable member is refused")
    };
    assert!(
        matches!(error, MobError::NotExternallyAddressable(_)),
        "{error:?}"
    );
    assert_eq!(settled(&mob_log), vec![Settlement::NotAdmitted]);

    let member = fixture
        .handle
        .member(&fixture.member)
        .await
        .expect("member handle");
    let (member_custody, member_log) = custody();
    let Err(error) = member
        .start_governed_input_with_custody(
            fixture.bound_under(|_| {}, "member-transport-authentication"),
            member_custody,
        )
        .await
    else {
        panic!("the member's external entry is refused the same way")
    };
    assert!(
        matches!(error, MobError::NotExternallyAddressable(_)),
        "{error:?}"
    );
    assert_eq!(settled(&member_log), vec![Settlement::NotAdmitted]);
    assert_eq!(fixture.adapter.gate.entered(), 0, "no native admission");
    assert!(fixture.provider.requests().is_empty(), "nothing ran");
}

/// The trusted host's internal governed input reaches the same
/// non-addressable member: it is admitted by the member session's native
/// admission, the custody settles `Admitted` exactly once, and the admitted
/// row keeps the input's bound claims exactly as the embedder bound them.
/// The external entry stays refused afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn internal_governed_input_reaches_a_non_addressable_member_and_settles_once() {
    let fixture = non_addressable_fixture().await;
    let input = fixture.bound(|_| {});
    let input_id = input.id().clone();
    let association = input
        .header()
        .authority_association
        .clone()
        .expect("bound claims");
    assert!(input.header().ingress_context.is_some(), "bound ingress");
    let (internal_custody, log) = custody();
    let turn = fixture
        .handle
        .start_internal_governed_input_with_custody(&fixture.member, input, internal_custody)
        .await
        .expect("native admission accepts the internal governed input");
    assert_eq!(settled(&log), vec![Settlement::Admitted]);
    assert_eq!(turn.session_id(), Some(&fixture.session_id));
    let admission = turn.admission().expect("governed admission is forwarded");
    assert_eq!(admission.session_id, fixture.session_id);
    assert_eq!(admission.input_id, input_id);
    assert!(!admission.deduplicated);
    tokio::time::timeout(WAIT, turn.wait())
        .await
        .expect("the governed turn completes")
        .expect("the governed turn succeeds");
    assert_eq!(settled(&log), vec![Settlement::Admitted], "settled once");
    let requests = fixture.provider.requests();
    assert_eq!(requests.len(), 1, "one native controller request");
    assert!(mentions_prompt(&requests[0]));
    let stored = fixture
        .machine
        .input_state(&fixture.session_id, &input_id)
        .await
        .expect("native row query")
        .expect("the member session admitted exactly this input");
    assert_eq!(stored.state.authority_contributors.len(), 1);
    assert!(
        stored.state.authority_contributors[0].association() == &association,
        "the internal route neither minted, altered nor dropped any claim"
    );

    let (external_custody, external_log) = custody();
    let Err(error) = fixture
        .handle
        .start_governed_input_with_custody(
            &fixture.member,
            fixture.bound_under(|_| {}, "external-transport-authentication"),
            external_custody,
        )
        .await
    else {
        panic!("an external caller is still refused")
    };
    assert!(
        matches!(error, MobError::NotExternallyAddressable(_)),
        "{error:?}"
    );
    assert_eq!(settled(&external_log), vec![Settlement::NotAdmitted]);
    assert_eq!(fixture.provider.requests().len(), 1, "nothing more ran");
}

/// The internal route keeps the governed contract: an unbound input is not
/// governed input, from both internal entries, and settles `NotAdmitted`
/// with nothing admitted.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn internal_governed_route_refuses_an_unbound_input() {
    let fixture = non_addressable_fixture().await;
    let (mob_custody, mob_log) = custody();
    let Err(error) = fixture
        .handle
        .start_internal_governed_input_with_custody(
            &fixture.member,
            Input::Prompt(PromptInput::new(PROMPT, None)),
            mob_custody,
        )
        .await
    else {
        panic!("an unbound input is not governed input")
    };
    assert!(
        matches!(error, MobError::GovernedInputUnbound { .. }),
        "{error:?}"
    );
    assert_eq!(settled(&mob_log), vec![Settlement::NotAdmitted]);

    let member = fixture
        .handle
        .member(&fixture.member)
        .await
        .expect("member handle");
    let (member_custody, member_log) = custody();
    let Err(error) = member
        .start_internal_governed_input_with_custody(
            Input::Prompt(PromptInput::new(PROMPT, None)),
            member_custody,
        )
        .await
    else {
        panic!("an unbound input is not governed input")
    };
    assert!(
        matches!(error, MobError::GovernedInputUnbound { .. }),
        "{error:?}"
    );
    assert_eq!(settled(&member_log), vec![Settlement::NotAdmitted]);
    assert_eq!(fixture.adapter.gate.entered(), 0, "no native admission");
    assert!(fixture.provider.requests().is_empty(), "nothing ran");
}

/// An internal governed submission dropped before it is first polled
/// settles the host custody `NotAdmitted` exactly once and dispatches
/// nothing, from both internal entries.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_never_polled_internal_governed_submission_settles_not_admitted_once() {
    let fixture = non_addressable_fixture().await;
    let (mob_custody, mob_log) = custody();
    let unpolled = fixture.handle.start_internal_governed_input_with_custody(
        &fixture.member,
        fixture.bound(|_| {}),
        mob_custody,
    );
    drop(unpolled);
    assert_eq!(settled(&mob_log), vec![Settlement::NotAdmitted]);

    let member = fixture
        .handle
        .member(&fixture.member)
        .await
        .expect("member handle");
    let (member_custody, member_log) = custody();
    let unpolled = member.start_internal_governed_input_with_custody(
        fixture.bound_under(|_| {}, "unpolled-transport-authentication"),
        member_custody,
    );
    drop(unpolled);
    assert_eq!(settled(&member_log), vec![Settlement::NotAdmitted]);

    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        settled(&mob_log),
        vec![Settlement::NotAdmitted],
        "settled once"
    );
    assert_eq!(
        settled(&member_log),
        vec![Settlement::NotAdmitted],
        "settled once"
    );
    assert_eq!(fixture.adapter.gate.entered(), 0, "no native admission");
    assert!(fixture.provider.requests().is_empty(), "nothing ran");
}

// ---------------------------------------------------------------------------
// Host originals through Core's delivery owner, end to end: the observation a
// namespace owner holds for the governed member's session is bound by the
// owner's binder with `bind_scoped_original` (Core's admission key as the
// event key) and admitted through `MachineContinuationSink` into the member's
// session on the real scoped grant composition. Native admission decides.
// ---------------------------------------------------------------------------

mod host_original {
    use super::*;
    use meerkat::{
        ContinuationDelivery, ContinuationDeliverySink, ContinuationHandling, ContinuationKey,
        ContinuationOwner, ContinuationOwnerService, ContinuationProducer, ContinuationResultRef,
        ContinuationStatus, RetainedJobSource, RuntimeDeliveryHost,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    const NAMESPACE: &str = "app.ingest";
    const OBSERVATION: &str = "work-7";
    const KEY: &str = "ingest:work-7";
    const BODY: &str = "a new item arrived for the governed member";

    fn observation() -> ContinuationDelivery {
        ContinuationDelivery {
            key: ContinuationKey::new(KEY).expect("key"),
            result: ContinuationResultRef {
                producer: ContinuationProducer::Host {
                    namespace: NAMESPACE.into(),
                },
                producer_id: OBSERVATION.into(),
                result_digest: "sha256:work-7".into(),
                summary: None,
            },
            body: BODY.into(),
            handling: ContinuationHandling::Queue,
        }
    }

    /// The original the adapter authenticates: `work-7` of the source owner.
    fn original_work() -> OriginalWorkRef {
        OriginalWorkRef {
            authority: principal("source-owner"),
            work: id("work-7"),
        }
    }

    #[derive(Default)]
    struct Custody {
        accepted: Mutex<Vec<meerkat::HostOriginalAdmission>>,
        dropped: AtomicUsize,
    }

    struct Settlement {
        custody: Arc<Custody>,
        settled: bool,
    }

    impl meerkat::HostOriginalSettlement for Settlement {
        fn accepted(
            mut self: Box<Self>,
            admission: &meerkat::HostOriginalAdmission,
        ) -> Result<(), String> {
            self.settled = true;
            self.custody
                .accepted
                .lock()
                .expect("custody")
                .push(admission.clone());
            Ok(())
        }
    }

    impl Drop for Settlement {
        fn drop(&mut self) {
            if !self.settled {
                self.custody.dropped.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    /// The trusted host's binder over the fixture's composition: it pins the
    /// member's live controller without waiting and binds Core's input to the
    /// confirmed original with `bind_scoped_original`.
    struct Binder {
        service: Arc<EphemeralSessionService<FactoryAgentBuilder>>,
        runtime: LogicalRuntimeId,
        target: NativeWorkTarget,
        controller: GrantLineageRef,
        operation: GrantLineageRef,
        custody: Arc<Custody>,
        bound: Mutex<Vec<InputAuthorityAssociation>>,
    }

    #[async_trait]
    impl meerkat::HostOriginalBinder for Binder {
        async fn bind(
            &self,
            original: &meerkat::HostOriginalRef,
            session: &SessionId,
            input: Input,
        ) -> Result<meerkat::BoundHostOriginal, meerkat::HostOriginalRefusal> {
            let Some(witness) = self.service.live_session_actor_witness(session).await else {
                return Err(meerkat::HostOriginalRefusal::NotReady(
                    meerkat_core::authorization::ControllerReadinessFailure::ExecutorUnavailable,
                ));
            };
            let pin = match self
                .service
                .try_pin_controller_client_for_actor(&witness)
                .await
            {
                Ok(pin) => pin,
                Err(meerkat_session::TryPinControllerError::TurnBoundaryHeld) => {
                    return Err(meerkat::HostOriginalRefusal::NotReady(
                        meerkat_core::authorization::ControllerReadinessFailure::TurnBoundaryHeld,
                    ));
                }
                Err(error) => {
                    return Err(meerkat::HostOriginalRefusal::Unavailable(error.to_string()));
                }
            };
            let mut candidate = association(
                &self.runtime,
                self.controller.clone(),
                self.operation.clone(),
                pin.selection().clone(),
            );
            candidate.target = self.target.clone();
            candidate.original_work = original.original_work.clone();
            let association = InputAuthorityAssociation::new(candidate).expect("claims");
            self.bound.lock().expect("bound").push(association.clone());
            let key = input
                .header()
                .idempotency_key
                .clone()
                .expect("Core's admission key");
            let bound = meerkat_runtime::meerkat_machine::bind_scoped_original(
                input,
                association,
                key,
                principal("requester"),
                principal("ingress"),
                RealmId::parse("governed-mob").expect("realm"),
                evidence("original-authentication"),
                pin,
            )
            .map_err(|error| meerkat::HostOriginalRefusal::Unavailable(error.to_string()))?;
            Ok(meerkat::BoundHostOriginal::new(
                bound,
                Box::new(Settlement {
                    custody: Arc::clone(&self.custody),
                    settled: false,
                }),
            ))
        }

        async fn readiness(
            &self,
            _namespace: &str,
            _owner: &ContinuationOwner,
        ) -> meerkat::HostOriginalBinderReadiness {
            meerkat::HostOriginalBinderReadiness::Ready
        }

        fn retain_unresolved(
            &self,
            _settlement: Box<dyn meerkat::HostOriginalSettlement>,
            _detail: &str,
        ) {
        }
    }

    /// The ingest namespace's committed owner: it holds the observation owed
    /// to the member's session and offers the binder.
    struct NamespaceOwner {
        facts: meerkat::HostOriginalFacts,
        binder: Arc<Binder>,
    }

    #[async_trait]
    impl RetainedJobSource for NamespaceOwner {
        async fn retained_job(
            &self,
            _producer: &ContinuationProducer,
            _job_id: &str,
        ) -> meerkat::RetainedJobLookup {
            meerkat::RetainedJobLookup::Absent
        }

        async fn host_original(
            &self,
            namespace: &str,
            producer_id: &str,
        ) -> meerkat::HostOriginalLookup {
            if namespace == NAMESPACE && producer_id == OBSERVATION {
                meerkat::HostOriginalLookup::Found(self.facts.clone())
            } else {
                meerkat::HostOriginalLookup::Absent
            }
        }

        fn host_original_binder(
            &self,
            namespace: &str,
        ) -> Option<Arc<dyn meerkat::HostOriginalBinder>> {
            (namespace == NAMESPACE)
                .then(|| Arc::clone(&self.binder) as Arc<dyn meerkat::HostOriginalBinder>)
        }
    }

    struct NoJobs;

    #[async_trait]
    impl meerkat::JobDeliverySink for NoJobs {
        async fn apply(
            &self,
            _application: meerkat::JobDeliveryApplication,
        ) -> Result<(), meerkat::JobDeliveryApplyError> {
            Ok(())
        }
    }

    /// One process hosting the governed member's session, admitting
    /// continuations through the machine sink of the fixture's machine.
    struct Host {
        machine: Arc<MeerkatMachine>,
        source: Arc<NamespaceOwner>,
    }

    #[async_trait]
    impl RuntimeDeliveryHost for Host {
        async fn delivery_route(&self, _session_id: &SessionId) -> Option<meerkat::DeliveryRoute> {
            Some(meerkat::DeliveryRoute::ServedHere(Arc::new(NoJobs)))
        }

        async fn claim_cold_delivery(
            &self,
            session_id: &SessionId,
        ) -> Option<Result<meerkat_runtime::HostingClaim, meerkat_runtime::HostingRefused>>
        {
            Some(meerkat_runtime::grant_session_hosting(
                &meerkat_runtime::HostingCapability::ProcessLocal,
                &meerkat_runtime::HostingOwner::mint(),
                session_id,
            ))
        }

        async fn resolve_address(&self, address: &LogicalRuntimeId) -> meerkat::AddressResolution {
            meerkat::default_address_resolution(address)
        }

        async fn continuation_sink(
            &self,
            _session_id: &SessionId,
        ) -> Option<Arc<dyn ContinuationDeliverySink>> {
            Some(Arc::new(meerkat::MachineContinuationSink::new(Arc::clone(
                &self.machine,
            ))))
        }

        fn retained_job_source(&self) -> Option<Arc<dyn RetainedJobSource>> {
            Some(Arc::clone(&self.source) as Arc<dyn RetainedJobSource>)
        }
    }

    /// The observation committed for the member's session from its namespace
    /// owner, with Core's delivery owner armed over its inbox.
    struct Delivered {
        continuations: ContinuationOwnerService,
        owner: ContinuationOwner,
        binder: Arc<Binder>,
        custody: Arc<Custody>,
        passes: tokio::sync::watch::Receiver<meerkat::RuntimeDeliveryPass>,
        _handle: meerkat::RuntimeDeliveryOwnerHandle,
    }

    impl Delivered {
        async fn status(&self) -> ContinuationStatus {
            self.continuations
                .continuation_status(&self.owner, &ContinuationKey::new(KEY).expect("key"))
                .await
                .expect("status")
        }

        async fn settled(&mut self) -> ContinuationStatus {
            let settled = tokio::time::timeout(WAIT, async {
                loop {
                    let status = self.status().await;
                    if matches!(
                        status,
                        ContinuationStatus::Applied { .. } | ContinuationStatus::Refused { .. }
                    ) {
                        return status;
                    }
                    self.passes.changed().await.expect("owner pass channel");
                }
            })
            .await;
            settled.unwrap_or_else(|_| {
                panic!(
                    "the host original was neither applied nor settled; last pass {:?}",
                    self.passes.borrow().clone()
                )
            })
        }
    }

    async fn deliver(fixture: &Fixture) -> Delivered {
        let inbox = meerkat_runtime::RuntimeDeliveryInbox::new(Arc::new(
            meerkat_runtime::InMemoryRuntimeStore::new(),
        ));
        let continuations = ContinuationOwnerService::new(
            inbox.clone(),
            Arc::new(meerkat::SessionAddressResolver),
            Arc::clone(&fixture.machine),
        );
        let owner = ContinuationOwner::Session {
            session_id: fixture.session_id.clone(),
        };
        let custody = Arc::new(Custody::default());
        let binder = Arc::new(Binder {
            service: Arc::clone(&fixture.service),
            runtime: fixture.runtime.clone(),
            target: fixture.target.clone(),
            controller: fixture.controller.clone(),
            operation: fixture.operation.clone(),
            custody: Arc::clone(&custody),
            bound: Mutex::new(Vec::new()),
        });
        let source = Arc::new(NamespaceOwner {
            facts: meerkat::HostOriginalFacts::new(owner.clone(), observation(), original_work()),
            binder: Arc::clone(&binder),
        });
        let record =
            meerkat::HostOriginalRecord::from_owner(source.as_ref(), NAMESPACE, OBSERVATION)
                .await
                .expect("read the namespace owner")
                .expect("the namespace owner holds the observation");
        continuations
            .submit_host_original(&owner, observation(), &record, 1)
            .await
            .expect("a governed runtime commits the host original");
        let handle = meerkat::RuntimeDeliveryOwner::new(
            Arc::new(meerkat::MemoryDetachedJobStore::new()),
            inbox,
        )
        .arm(Arc::new(Host {
            machine: Arc::clone(&fixture.machine),
            source,
        }))
        .expect("arm Core's delivery owner");
        Delivered {
            continuations,
            owner,
            binder,
            custody,
            passes: handle.subscribe_passes(),
            _handle: handle,
        }
    }

    #[derive(serde::Deserialize)]
    struct AuditProjection {
        #[serde(default)]
        authorization_audit:
            Vec<meerkat_authorization_contracts::audit::StoredAuthorizationAuditObservation>,
    }

    /// Allowed: the host original lands in the governed member's session
    /// through its binder; the admitted row retains exactly the bound
    /// association; the host's custody is promoted once; and the member's run
    /// is authorized and audited under that association.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_host_original_lands_in_the_governed_member_through_its_binder_and_runs() {
        let fixture = Fixture::new().await;
        let mut delivered = deliver(&fixture).await;
        let status = delivered.settled().await;
        let ContinuationStatus::Applied { session, input, .. } = status else {
            panic!("the host original lands, got {status:?}");
        };
        assert_eq!(session, fixture.session_id);
        let stored = fixture
            .machine
            .input_state(&fixture.session_id, &input)
            .await
            .expect("native row query")
            .expect("the member session admitted the host original");
        assert_eq!(stored.state.authority_contributors.len(), 1);
        assert_eq!(stored.state.authority_contributors[0].input_id(), &input);
        let bound = delivered.binder.bound.lock().expect("bound").clone();
        assert_eq!(bound.len(), 1, "bound once");
        assert!(
            &bound[0] == stored.state.authority_contributors[0].association(),
            "the admitted row retains exactly the association its binder bound"
        );
        let accepted = delivered.custody.accepted.lock().expect("custody").clone();
        assert_eq!(accepted.len(), 1, "custody promoted once");
        assert_eq!(accepted[0].input, input);
        assert_eq!(delivered.custody.dropped.load(Ordering::SeqCst), 0);

        wait_until("the member ran the host original", || {
            fixture.provider.requests().iter().any(|messages| {
                serde_json::to_string(messages)
                    .expect("render")
                    .contains(BODY)
            })
        })
        .await;
        let audit = tokio::time::timeout(WAIT, async {
            loop {
                let row = fixture
                    .machine
                    .input_state(&fixture.session_id, &input)
                    .await
                    .expect("native row query")
                    .expect("row");
                let audit = serde_json::from_value::<AuditProjection>(
                    serde_json::to_value(&row).expect("row renders"),
                )
                .expect("audit projection")
                .authorization_audit;
                if !audit.is_empty() {
                    return audit;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the run's operation is audited on the admitted row");
        assert!(
            audit.iter().all(|record| {
                record.contributors.iter().any(|contributor| {
                    contributor.input_id == input
                        && contributor.requester == principal("requester")
                        && contributor.logical_executor == principal("executor")
                })
            }),
            "every audit record names the admitted input under the bound association"
        );
    }

    /// The observation is refused `AuthorityDenied` by native admission: no
    /// input row exists, the custody is dropped, and nothing ran.
    async fn refused_by_native_admission(fixture: &Fixture, delivered: &mut Delivered) {
        let status = delivered.settled().await;
        assert!(
            matches!(
                status,
                ContinuationStatus::Refused {
                    reason: meerkat_runtime::RuntimeDeliveryRefusalReason::AuthorityDenied,
                    ..
                }
            ),
            "{status:?}"
        );
        assert!(
            delivered
                .custody
                .accepted
                .lock()
                .expect("custody")
                .is_empty()
        );
        assert_eq!(
            delivered.custody.dropped.load(Ordering::SeqCst),
            1,
            "the custody is dropped"
        );
        assert!(
            fixture
                .machine
                .list_active_inputs(&fixture.session_id)
                .await
                .expect("active inputs")
                .is_empty(),
            "no input was admitted"
        );
        assert!(fixture.provider.requests().is_empty(), "nothing ran");
    }

    struct AnyCustody;

    impl meerkat_authorization_contracts::grant_mutation::ControllerGrantMutationCustody
        for AnyCustody
    {
        fn with_unreferenced_controller_grant<T, E>(
            &mut self,
            _: &GrantLineageRef,
            mutate: impl FnOnce() -> Result<T, E>,
        ) -> Result<
            Result<T, E>,
            meerkat_authorization_contracts::grant_mutation::ControllerCustodyRefusal,
        > {
            Ok(mutate())
        }
    }

    /// The invocation policy denies: the requester's invocation grant is
    /// revoked, so the host invocation policy denies the requester.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_host_original_the_invocation_policy_denies_is_refused_authority_denied() {
        let fixture = Fixture::new().await;
        fixture
            .grants
            .revoke(
                &principal("grant-owner"),
                &fixture.invocation,
                &mut AnyCustody,
            )
            .expect("revoke the requester's invocation grant");
        let mut delivered = deliver(&fixture).await;
        refused_by_native_admission(&fixture, &mut delivered).await;
    }

    /// The original is cancelled: the source owner cancelled the original's
    /// operations before the drain.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_host_original_whose_original_is_cancelled_is_refused_authority_denied() {
        let fixture = Fixture::new().await;
        fixture.adapter.cancelled.store(true, Ordering::SeqCst);
        let mut delivered = deliver(&fixture).await;
        refused_by_native_admission(&fixture, &mut delivered).await;
    }

    /// The grant lineage is revoked: the controller grant the binding names is
    /// revoked before the drain.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_host_original_whose_controller_grant_is_revoked_is_refused_authority_denied() {
        let fixture = Fixture::new().await;
        fixture
            .grants
            .revoke(
                &principal("grant-owner"),
                &fixture.controller,
                &mut AnyCustody,
            )
            .expect("revoke the controller grant");
        let mut delivered = deliver(&fixture).await;
        refused_by_native_admission(&fixture, &mut delivered).await;
    }
}
