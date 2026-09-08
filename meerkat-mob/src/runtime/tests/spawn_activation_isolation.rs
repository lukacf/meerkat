//! #1105: spawn-activation isolation.
//!
//! Spawn admission is machine authority and stays on the actor. Spawn
//! ACTIVATION — trusted-peer publication, peer-ingress drain, autonomous
//! runtime readiness, kickoff admission, initial-turn admission — is opaque
//! third-party work owned by a process worker holding the exact incarnation
//! custody, and it re-enters the loop as a typed continuation.
//!
//! These tests pin the three properties that made the pre-#1105 inline
//! activation a fleet-wide stall:
//!
//! 1. one member's parked activation must not delay `QueryPhase` or another
//!    member's real turn;
//! 2. a failed activation must compensate exactly (the identity is
//!    respawnable), never leave the spawn half-committed;
//! 3. a late stage completion whose incarnation is no longer current must be
//!    DECLINED — never allowed to roll back the successor that owns the
//!    identity now.

use super::*;
use std::sync::Arc;

use meerkat_runtime::{InMemoryRuntimeStore, MeerkatMachine};

use crate::runtime::actor::spawn_activation::{
    SpawnActivationCustody, SpawnActivationQuiescence, SpawnActivationStage,
    SpawnActivationStageOutcome, SpawnActivationTicket,
};
use crate::runtime::state::MobCommand;

struct ActivationMob {
    handle: MobHandle,
    service: Arc<MockSessionService>,
}

fn turn_driven_activation_definition() -> MobDefinition {
    let mut definition = sample_definition();
    let worker = definition
        .profiles
        .get_mut(&ProfileName::from("worker"))
        .expect("worker profile")
        .as_inline_mut()
        .expect("inline worker profile");
    worker.runtime_mode = crate::MobRuntimeMode::TurnDriven;
    worker.external_addressable = true;
    definition
}

/// Same definition with every worker wired to every other worker, so a spawn
/// plans real wiring targets and therefore needs its peer-ingress drain
/// before the fan-out.
fn wired_activation_definition() -> MobDefinition {
    let mut definition = turn_driven_activation_definition();
    definition.wiring.role_wiring = vec![RoleWiringRule {
        a: ProfileName::from("worker"),
        b: ProfileName::from("worker"),
    }];
    definition
}

async fn create_activation_mob(definition: MobDefinition) -> ActivationMob {
    let runtime_store: Arc<dyn meerkat_runtime::store::RuntimeStore> =
        Arc::new(InMemoryRuntimeStore::new());
    let blob_store: Arc<dyn meerkat_core::BlobStore> =
        Arc::new(meerkat_store::MemoryBlobStore::new());
    let adapter = Arc::new(MeerkatMachine::persistent(runtime_store, blob_store));
    let service = Arc::new(MockSessionService::new());
    service.set_runtime_adapter(Arc::clone(&adapter));
    let handle = MobBuilder::new(definition, MobStorage::in_memory())
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create activation mob");
    ActivationMob { handle, service }
}

impl ActivationMob {
    async fn spawn(&self, identity: &str) -> Result<MemberRef, MobError> {
        self.handle
            .spawn(
                ProfileName::from("worker"),
                AgentIdentity::from(identity),
                None,
            )
            .await
    }

    async fn send(&self, identity: &str, text: &str) -> Result<MemberDeliveryReceipt, MobError> {
        self.handle
            .member(&AgentIdentity::from(identity))
            .await?
            .send(text.to_string(), HandlingMode::Queue)
            .await
    }

    /// One `QueryPhase` round trip; `Err(())` when it misses `budget`.
    async fn probe(&self, budget: Duration) -> Result<Duration, ()> {
        let started = Instant::now();
        let reply_rx = self
            .handle
            .enqueue_actor_command_for_test(|reply_tx| MobCommand::QueryPhase { reply_tx })
            .await
            .expect("probe enqueue");
        match tokio::time::timeout(budget, reply_rx).await {
            Ok(reply) => {
                reply
                    .expect("probe reply channel")
                    .expect("probe phase read");
                Ok(started.elapsed())
            }
            Err(_) => Err(()),
        }
    }

    /// The exact custody census a graph-scoped lifecycle gate waits on.
    async fn custody(&self) -> SpawnActivationQuiescence {
        let reply_rx = self
            .handle
            .enqueue_actor_command_for_test(|reply_tx| MobCommand::SpawnActivationCustodyProbe {
                reply_tx,
            })
            .await
            .expect("custody probe enqueue");
        tokio::time::timeout(Duration::from_secs(2), reply_rx)
            .await
            .expect("custody probe must answer while the loop is healthy")
            .expect("custody probe reply channel")
    }

    async fn member_exists(&self, identity: &str) -> bool {
        self.handle
            .member(&AgentIdentity::from(identity))
            .await
            .is_ok()
    }
}

/// A parked activation stage holds only its own worker task. The loop keeps
/// answering `QueryPhase` and keeps executing an unrelated member's turn.
///
/// Before #1105 the peer-ingress drain ran inline on the actor, so this
/// 2.5s park was 2.5s of fleet-wide silence.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parked_spawn_activation_does_not_stall_the_loop_or_other_members() {
    let mob = create_activation_mob(turn_driven_activation_definition()).await;
    mob.spawn("w-0").await.expect("first member spawns");
    // Every comms-runtime read in the next member's activation parks. The
    // turn-driven peer-ingress drain always makes one, so the activation is
    // guaranteed to be in flight for at least one park.
    mob.service.set_comms_runtime_delay_for_next_calls(3, 1_200);

    let spawn_started = Instant::now();
    let spawning = {
        let handle = mob.handle.clone();
        tokio::spawn(async move {
            handle
                .spawn(
                    ProfileName::from("worker"),
                    AgentIdentity::from("w-1"),
                    None,
                )
                .await
        })
    };

    // The loop must stay responsive while the activation is parked.
    for _ in 0..5 {
        let elapsed = mob
            .probe(Duration::from_millis(400))
            .await
            .expect("QueryPhase must not wait behind a parked spawn activation");
        assert!(
            elapsed < Duration::from_millis(400),
            "probe took {elapsed:?} while a spawn activation was parked"
        );
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    // And an unrelated member's real turn must complete, not queue behind it.
    let turn_started = Instant::now();
    mob.send("w-0", "unrelated work")
        .await
        .expect("unrelated member turn is admitted while an activation is parked");
    assert!(
        turn_started.elapsed() < Duration::from_secs(2),
        "unrelated turn waited {:?} behind a parked spawn activation",
        turn_started.elapsed()
    );

    let receipt = tokio::time::timeout(Duration::from_secs(20), spawning)
        .await
        .expect("parked activation settles")
        .expect("spawn task joins");
    receipt.expect("the parked activation eventually completes the spawn");
    assert!(
        spawn_started.elapsed() >= Duration::from_millis(1_000),
        "the activation was never actually parked ({:?}); the test proves nothing",
        spawn_started.elapsed()
    );
    assert!(mob.member_exists("w-1").await);
}

/// A failed activation stage compensates EXACTLY: the spawn error reaches the
/// caller and the identity is respawnable, so nothing is left half-committed
/// (no dropped provision, no wedged spawn-exec phase).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_spawn_activation_compensates_and_leaves_the_identity_respawnable() {
    let mob = create_activation_mob(wired_activation_definition()).await;
    mob.spawn("w-0").await.expect("first member spawns");

    // The next comms-runtime observation reports the runtime as unavailable,
    // so the new member's required peer-ingress drain fails.
    mob.service.set_comms_runtime_missing_after_successes(0);
    let error = mob
        .spawn("w-1")
        .await
        .expect_err("a member whose peer ingress cannot start must not spawn");
    let rendered = error.to_string();
    assert!(
        rendered.contains("peer ingress") || rendered.contains("comms"),
        "unexpected activation failure: {rendered}"
    );
    assert!(
        !mob.member_exists("w-1").await,
        "a failed activation must not leave the member live"
    );

    // The loop is still healthy and the identity is genuinely reusable.
    mob.probe(Duration::from_millis(500))
        .await
        .expect("loop stays responsive after a failed activation");
    // Restore availability without touching the shared mock's gate surface:
    // the switch counts down successful observations before it fails.
    mob.service
        .set_comms_runtime_missing_after_successes(usize::MAX);
    mob.spawn("w-1")
        .await
        .expect("the compensated identity is respawnable");
    assert!(mob.member_exists("w-1").await);
}

/// A stage completion whose incarnation is no longer current is declined: it
/// must not roll back, retire, or otherwise touch the member that owns the
/// identity now.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stale_activation_stage_completion_never_touches_the_current_incarnation() {
    let mob = create_activation_mob(turn_driven_activation_definition()).await;
    let member_ref = mob.spawn("w-0").await.expect("member spawns");
    let identity = AgentIdentity::from("w-0");

    // A late completion from a superseded incarnation: same identity, a
    // generation/fence tuple the roster never held, and a ticket that no
    // live activation owns.
    let custody = SpawnActivationCustody {
        agent_identity: identity.clone(),
        generation: crate::ids::Generation::new(99),
        fence_token: crate::ids::FenceToken::new(99),
        agent_runtime_id: crate::ids::AgentRuntimeId::new(
            identity.clone(),
            crate::ids::Generation::new(99),
        ),
        operation_id: meerkat_core::ops::OperationId::new(),
        // Exact resource fence: a member ref this mob never issued.
        member_ref: crate::event::MemberRef::Session {
            session_id: SessionId::new(),
        },
    };
    let _rx = mob
        .handle
        .enqueue_actor_command_for_test(|_reply_tx: tokio::sync::oneshot::Sender<()>| {
            MobCommand::SpawnActivationStageSettled {
                ticket: SpawnActivationTicket::default(),
                outcome: Box::new(SpawnActivationStageOutcome {
                    stage: SpawnActivationStage::AutonomousReadiness,
                    custody,
                    result: Ok(()),
                }),
            }
        })
        .await
        .expect("stale settlement enqueues");

    mob.probe(Duration::from_millis(500))
        .await
        .expect("a declined settlement must not wedge the loop");
    assert!(
        mob.member_exists("w-0").await,
        "a declined stale settlement must not remove the current incarnation"
    );
    mob.send("w-0", "still alive")
        .await
        .expect("the current incarnation keeps accepting work after a declined settlement");
    assert!(member_ref.bridge_session_id().is_some());
}

/// A LATE FAILING stage outcome must never destroy a live member.
///
/// The failure path inside the pipeline unwinds through
/// `rollback_failed_spawn` (retire + unwire) — a destructive graph mutation.
/// When the outcome arrives for an activation that no longer holds custody
/// (settled, superseded, or released by a lifecycle control), it must be
/// declined and observed, never applied to whoever owns the identity now.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn late_failing_stage_outcome_never_rolls_back_a_live_member() {
    let mob = create_activation_mob(turn_driven_activation_definition()).await;
    mob.spawn("w-0").await.expect("member spawns");
    mob.spawn("w-1").await.expect("second member spawns");
    let identity = AgentIdentity::from("w-0");

    // Exactly the shape of a wiring/kickoff stage that failed after its
    // activation lost custody: current identity, superseded incarnation.
    let custody = SpawnActivationCustody {
        agent_identity: identity.clone(),
        generation: crate::ids::Generation::new(41),
        fence_token: crate::ids::FenceToken::new(41),
        agent_runtime_id: crate::ids::AgentRuntimeId::new(
            identity.clone(),
            crate::ids::Generation::new(41),
        ),
        operation_id: meerkat_core::ops::OperationId::new(),
        member_ref: crate::event::MemberRef::Session {
            session_id: SessionId::new(),
        },
    };
    let _rx = mob
        .handle
        .enqueue_actor_command_for_test(|_reply_tx: tokio::sync::oneshot::Sender<()>| {
            MobCommand::SpawnActivationStageSettled {
                ticket: SpawnActivationTicket::default(),
                outcome: Box::new(SpawnActivationStageOutcome {
                    stage: SpawnActivationStage::WiringPeerIngress,
                    custody,
                    result: Err(MobError::WiringError(
                        "late peer ingress failure from a superseded activation".to_string(),
                    )),
                }),
            }
        })
        .await
        .expect("late failure enqueues");

    mob.probe(Duration::from_millis(500))
        .await
        .expect("a declined late failure must not wedge the loop");
    assert!(
        mob.member_exists("w-0").await,
        "a late stage failure must not retire the live member"
    );
    assert!(
        mob.member_exists("w-1").await,
        "a late stage failure must not disturb unrelated members"
    );
    mob.send("w-0", "still wired")
        .await
        .expect("the live member keeps accepting work after a declined late failure");
}

/// An activation admitted BEFORE a graph-scoped control must hold that
/// control off until its custody drains — and the custody census the control
/// gates on must report it honestly while it is outstanding.
///
/// This is the observable half of the reciprocal-custody contract: the
/// lifecycle gate refuses `BeginExplicitResumeTopology` while
/// `pending_spawn_activations` is non-empty, so an in-flight activation is
/// exactly what it must wait for. The census is read directly instead of
/// being inferred from timing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn activation_admitted_before_topology_holds_custody_until_it_drains() {
    let mob = create_activation_mob(turn_driven_activation_definition()).await;
    mob.spawn("w-0").await.expect("first member spawns");
    assert_eq!(
        mob.custody().await.total,
        0,
        "a settled spawn must leave no activation custody behind"
    );

    // Park the new member's activation inside its peer-ingress stage.
    mob.service.set_comms_runtime_delay_for_next_calls(3, 1_500);
    let spawning = {
        let handle = mob.handle.clone();
        tokio::spawn(async move {
            handle
                .spawn(
                    ProfileName::from("worker"),
                    AgentIdentity::from("w-1"),
                    None,
                )
                .await
        })
    };

    // While the stage is outstanding the census must report custody, and the
    // loop must stay responsive so the gate can keep polling it.
    let mut observed_outstanding = false;
    // Endpoint observations precede activation and are now detached too.
    // Wait through those two delayed reads to observe the activation itself.
    for _ in 0..200 {
        let census = mob.custody().await;
        if census.total > 0 {
            observed_outstanding = true;
            assert!(
                census.members.contains(&AgentIdentity::from("w-1")),
                "the census must name the member whose activation holds custody"
            );
            assert!(
                census.inflight_stages > 0 || census.parked > 0,
                "outstanding custody must be attributable to a stage or a park"
            );
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(
        observed_outstanding,
        "an in-flight activation must be visible to the lifecycle gate's census"
    );
    mob.probe(Duration::from_millis(400))
        .await
        .expect("the gate must be able to keep polling while custody is outstanding");

    let receipt = tokio::time::timeout(Duration::from_secs(20), spawning)
        .await
        .expect("activation settles")
        .expect("spawn task joins");
    receipt.expect("the activation completes the spawn");

    // Custody must drain to exactly zero, which is the only reading that
    // authorizes the control to begin.
    wait_until_custody_drained(&mob).await;
}

async fn wait_until_custody_drained(mob: &ActivationMob) {
    for _ in 0..80 {
        let census = mob.custody().await;
        if census.total == 0 && census.parked == 0 && census.deferred_outcomes == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let census = mob.custody().await;
    panic!(
        "spawn activation custody never drained: total={} parked={} inflight={} deferred={} members={:?}",
        census.total,
        census.parked,
        census.inflight_stages,
        census.deferred_outcomes,
        census.members
    );
}
