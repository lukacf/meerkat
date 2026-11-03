//! A concurrent `ensure_member` of one identity must converge on the in-flight
//! spawn of that identity in every phase of the local spawn ladder, and a
//! concurrent spawn of the same identity must be refused as a duplicate while
//! the first spawn is still in flight.

use super::*;
use crate::runtime::reconcile::EnsureMemberOutcome;

fn ensure_worker_spec(identity: &AgentIdentity) -> SpawnMemberSpec {
    SpawnMemberSpec::new(ProfileName::from("worker"), identity.clone())
}

async fn wait_for_boundary_call(gate: &TestRuntimeControlBarrier, what: &str) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while gate.boundary_calls.load(Ordering::Acquire) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{what}"));
}

/// Window 1: the first spawn is still in its off-loop preparation, before
/// `StageSpawn` makes the identity pending in MobMachine. A second ensure is
/// refused by the actor's preparation slot, and must then wait on the first
/// spawn instead of answering `MemberNotFound`.
#[tokio::test]
async fn concurrent_ensure_waits_for_a_first_spawn_still_preparing_off_loop() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let identity = AgentIdentity::from("ensure-preparation-window");
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let (entered_tx, mut entered_rx) = tokio::sync::mpsc::unbounded_channel();
    arm_spawn_preparation_gate_for_test(
        &identity,
        crate::runtime::actor::spawn_preparation::SpawnPreparationTestGate {
            entered: Some(entered_tx),
            release: Some(Arc::clone(&release)),
            settled: None,
        },
    );
    let first = tokio::spawn({
        let handle = handle.clone();
        let spec = ensure_worker_spec(&identity);
        async move { handle.ensure_member(spec).await }
    });
    tokio::time::timeout(Duration::from_secs(10), entered_rx.recv())
        .await
        .expect("the first spawn reaches its off-loop preparation")
        .expect("preparation gate channel");
    let mut second = tokio::spawn({
        let handle = handle.clone();
        let spec = ensure_worker_spec(&identity);
        async move { handle.ensure_member(spec).await }
    });
    // The first spawn cannot leave its preparation while the gate holds, so a
    // second ensure that answers now has not converged on it.
    let early = tokio::time::timeout(Duration::from_millis(500), &mut second).await;
    assert!(
        early.is_err(),
        "the second ensure answered while the first spawn was still preparing: {early:?}"
    );
    release.add_permits(1);
    let first = tokio::time::timeout(Duration::from_secs(10), first)
        .await
        .expect("first ensure settles")
        .expect("first ensure join")
        .expect("first ensure");
    let second = tokio::time::timeout(Duration::from_secs(10), second)
        .await
        .expect("second ensure settles")
        .expect("second ensure join")
        .expect("second ensure converges");
    assert!(
        matches!(first, EnsureMemberOutcome::Spawned(_)),
        "{first:?}"
    );
    assert!(
        matches!(second, EnsureMemberOutcome::Existed(_)),
        "{second:?}"
    );
    assert_eq!(service.active_session_count().await, 1);
}

/// Window 2: the first spawn has closed its pending admission (`CompleteSpawn`
/// in `handle_spawn_provisioned_batch`) and is parked in its off-actor
/// endpoint observation (the supervisor private-trust install), after
/// `CompleteSpawn` and before `CommitSpawnMembership` (with the fix its
/// spawn-exec ladder is already open here). A second ensure that is already
/// waiting on the staged spawn must keep waiting through this window.
#[tokio::test]
async fn concurrent_ensure_waits_while_the_first_spawn_observes_its_endpoint() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let identity = AgentIdentity::from("ensure-admission-window");
    let creation = service.park_next_session_creation();
    let trust = service.park_next_session_supervisor_publish();
    let first = tokio::spawn({
        let handle = handle.clone();
        let spec = ensure_worker_spec(&identity);
        async move { handle.ensure_member(spec).await }
    });
    wait_for_boundary_call(
        &creation,
        "the first spawn is staged and parks in create_session",
    )
    .await;
    let mut second = tokio::spawn({
        let handle = handle.clone();
        let spec = ensure_worker_spec(&identity);
        async move { handle.ensure_member(spec).await }
    });
    let early = tokio::time::timeout(Duration::from_millis(500), &mut second).await;
    assert!(
        early.is_err(),
        "the second ensure must wait while the first spawn is staged: {early:?}"
    );
    creation.release_all();
    wait_for_boundary_call(
        &trust,
        "the first spawn parks in its supervisor private-trust install",
    )
    .await;
    let early = tokio::time::timeout(Duration::from_millis(500), &mut second).await;
    assert!(
        early.is_err(),
        "the second ensure answered while the first spawn was between CompleteSpawn and \
         CommitSpawnMembership: {early:?}"
    );
    trust.release_all();
    let first = tokio::time::timeout(Duration::from_secs(10), first)
        .await
        .expect("first ensure settles")
        .expect("first ensure join")
        .expect("first ensure");
    let second = tokio::time::timeout(Duration::from_secs(10), second)
        .await
        .expect("second ensure settles")
        .expect("second ensure join")
        .expect("second ensure converges");
    assert!(
        matches!(first, EnsureMemberOutcome::Spawned(_)),
        "{first:?}"
    );
    assert!(
        matches!(second, EnsureMemberOutcome::Existed(_)),
        "{second:?}"
    );
    assert_eq!(service.active_session_count().await, 1);
}

/// The same window seen by a plain spawn: while the first spawn of an identity
/// is parked between `CompleteSpawn` and `CommitSpawnMembership`, a second
/// spawn of that identity must be refused as a duplicate, and the first spawn
/// must still seat its member with the actor serving.
#[tokio::test]
async fn second_spawn_is_refused_while_the_first_spawn_observes_its_endpoint() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let identity = AgentIdentity::from("spawn-admission-window");
    let trust = service.park_next_session_supervisor_publish();
    let first = tokio::spawn({
        let handle = handle.clone();
        let spec = ensure_worker_spec(&identity);
        async move { handle.spawn_spec(spec).await }
    });
    wait_for_boundary_call(
        &trust,
        "the first spawn parks in its supervisor private-trust install",
    )
    .await;
    let second = tokio::time::timeout(
        Duration::from_secs(10),
        handle.spawn_spec(ensure_worker_spec(&identity)),
    )
    .await;
    eprintln!("second spawn while the first is parked: {second:?}");
    trust.release_all();
    let first = tokio::time::timeout(Duration::from_secs(10), first).await;
    eprintln!("first spawn after release: {first:?}");
    let status = tokio::time::timeout(Duration::from_secs(5), handle.status()).await;
    eprintln!("mob status after both: {status:?}");
    eprintln!("active sessions: {}", service.active_session_count().await);
    assert!(
        matches!(&second, Ok(Err(MobError::MemberAlreadyExists(id))) if id == &identity),
        "a second spawn of an identity whose first spawn is still in flight must be refused \
         as a duplicate: {second:?}"
    );
    assert!(
        matches!(&first, Ok(Ok(Ok(_)))),
        "the first spawn still seats its member: {first:?}"
    );
    assert!(matches!(status, Ok(Ok(MobState::Running))), "{status:?}");
    assert_eq!(service.active_session_count().await, 1);
}

type EnsureJoin = Result<Result<EnsureMemberOutcome, MobError>, tokio::task::JoinError>;

/// Both ensures of one identity converged on the one member the first spawn
/// seated: the first spawned it, the second answered it as existing with the
/// same runtime binding and fence, one session exists, the roster holds the
/// identity once, and the mob is still running.
async fn assert_ensures_converged_on_one_member(
    handle: &MobHandle,
    service: &MockSessionService,
    identity: &AgentIdentity,
    first: EnsureJoin,
    second: EnsureJoin,
) {
    let status = tokio::time::timeout(Duration::from_secs(5), handle.status()).await;
    let sessions = service.active_session_count().await;
    let members = tokio::time::timeout(Duration::from_secs(5), handle.list_members())
        .await
        .map(|members| {
            members
                .into_iter()
                .filter(|entry| &entry.agent_identity == identity)
                .count()
        });
    let first_binding = match &first {
        Ok(Ok(EnsureMemberOutcome::Spawned(spawned))) => Some((
            Some(spawned.agent_runtime_id.clone()),
            Some(spawned.fence_token),
        )),
        _ => None,
    };
    let second_binding = match &second {
        Ok(Ok(EnsureMemberOutcome::Existed(entry))) => {
            Some((entry.agent_runtime_id.clone(), entry.fence_token))
        }
        _ => None,
    };
    assert!(
        first_binding.is_some()
            && second_binding == first_binding
            && matches!(status, Ok(Ok(MobState::Running)))
            && sessions == 1
            && matches!(members, Ok(1)),
        "both ensures must converge on the one member the first spawn seats, with one \
         session and the mob running: first={first:?} second={second:?} \
         status={status:?} sessions={sessions} members={members:?}"
    );
}

/// A second ensure of an identity arrives while the first ensure's spawn is
/// still in its off-loop preparation, before `StageSpawn` (a host that
/// retries an ensure after its own timeout does this). Both ensures must
/// converge on the one member the first spawn seats.
#[tokio::test]
async fn concurrent_ensures_converge_when_the_second_arrives_during_preparation() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let identity = AgentIdentity::from("ensure-pair-preparation-window");
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let (entered_tx, mut entered_rx) = tokio::sync::mpsc::unbounded_channel();
    arm_spawn_preparation_gate_for_test(
        &identity,
        crate::runtime::actor::spawn_preparation::SpawnPreparationTestGate {
            entered: Some(entered_tx),
            release: Some(Arc::clone(&release)),
            settled: None,
        },
    );
    let first = tokio::spawn({
        let handle = handle.clone();
        let spec = ensure_worker_spec(&identity);
        async move { handle.ensure_member(spec).await }
    });
    tokio::time::timeout(Duration::from_secs(10), entered_rx.recv())
        .await
        .expect("the first spawn reaches its off-loop preparation")
        .expect("preparation gate channel");
    let mut second = tokio::spawn({
        let handle = handle.clone();
        let spec = ensure_worker_spec(&identity);
        async move { handle.ensure_member(spec).await }
    });
    // Let the second ensure decide while the first spawn is held.
    let early = tokio::time::timeout(Duration::from_secs(2), &mut second).await;
    release.add_permits(1);
    let second = match early {
        Ok(joined) => joined,
        Err(_) => tokio::time::timeout(Duration::from_secs(10), second)
            .await
            .expect("second ensure settles"),
    };
    let first = tokio::time::timeout(Duration::from_secs(10), first)
        .await
        .expect("first ensure settles");
    assert_ensures_converged_on_one_member(&handle, &service, &identity, first, second).await;
}

/// A second ensure of an identity arrives while the first ensure's spawn is
/// parked between `CompleteSpawn` and its membership commit (in its
/// supervisor private-trust install). Both ensures must converge on the one
/// member the first spawn seats, and the mob must keep running.
#[tokio::test]
async fn concurrent_ensures_converge_when_the_second_arrives_during_the_endpoint_observation() {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let identity = AgentIdentity::from("ensure-pair-admission-window");
    let trust = service.park_next_session_supervisor_publish();
    let first = tokio::spawn({
        let handle = handle.clone();
        let spec = ensure_worker_spec(&identity);
        async move { handle.ensure_member(spec).await }
    });
    wait_for_boundary_call(
        &trust,
        "the first spawn parks in its supervisor private-trust install",
    )
    .await;
    let mut second = tokio::spawn({
        let handle = handle.clone();
        let spec = ensure_worker_spec(&identity);
        async move { handle.ensure_member(spec).await }
    });
    // Let the second ensure decide while the first spawn is parked.
    let early = tokio::time::timeout(Duration::from_secs(2), &mut second).await;
    trust.release_all();
    let second = match early {
        Ok(joined) => joined,
        Err(_) => tokio::time::timeout(Duration::from_secs(10), second)
            .await
            .expect("second ensure settles"),
    };
    let first = tokio::time::timeout(Duration::from_secs(10), first)
        .await
        .expect("first ensure settles");
    assert_ensures_converged_on_one_member(&handle, &service, &identity, first, second).await;
}
