//! #1105 (wiring effect isolation): one member's blocked trust install or
//! peer-lifecycle notice must not park the actor loop.
//!
//! Every test here drives the ordinary public verbs (`wire`, `unwire`,
//! `wire_members_batch`, `retire`) against session-backed local members whose
//! `MockCommsRuntime` can be parked (`trust_mutation_gate`,
//! `peer_lifecycle_delay_ms`) or made to fail (`fail_add_trust`,
//! `fail_send_peer_added`).
//!
//! The pre-#1105 loop awaited those effects inline, so a parked member made
//! `QueryPhase` and every unrelated member's wiring wait for it. The bounds
//! below are tight enough to fail decisively on that shape while staying
//! green on a loaded CI box.

use super::*;
use std::sync::Arc;
use std::sync::atomic::Ordering;

/// Local session-backed members with no declarative wiring: each test drives
/// the edges it needs explicitly.
struct WiringMob {
    handle: MobHandle,
    service: Arc<MockSessionService>,
    members: Vec<(AgentIdentity, SessionId)>,
}

async fn create_wiring_mob(member_count: usize) -> WiringMob {
    let (handle, service) = create_test_mob(sample_definition()).await;
    let mut members = Vec::with_capacity(member_count);
    for index in 0..member_count {
        let identity = AgentIdentity::from(format!("w-{index}"));
        let receipt = handle
            .spawn(ProfileName::from("worker"), identity.clone(), None)
            .await
            .unwrap_or_else(|error| panic!("spawn {identity}: {error}"));
        let session_id = receipt
            .bridge_session_id()
            .expect("session-backed member")
            .clone();
        members.push((identity, session_id));
    }
    WiringMob {
        handle,
        service,
        members,
    }
}

impl WiringMob {
    fn member(&self, index: usize) -> &AgentIdentity {
        &self.members[index].0
    }

    fn session(&self, index: usize) -> &SessionId {
        &self.members[index].1
    }

    async fn comms(&self, index: usize) -> Arc<MockCommsRuntime> {
        self.service
            .sessions
            .read()
            .await
            .get(self.session(index))
            .cloned()
            .expect("member comms runtime")
    }

    /// One `QueryPhase` round trip; `Err(())` when it misses `budget`.
    async fn probe(&self, budget: Duration) -> Result<MobState, ()> {
        match tokio::time::timeout(budget, self.handle.status()).await {
            Ok(state) => Ok(state.expect("phase read")),
            Err(_) => Err(()),
        }
    }

    async fn trusted_peer_ids(&self, index: usize) -> Vec<String> {
        self.comms(index)
            .await
            .peers()
            .await
            .into_iter()
            .map(|peer| peer.peer_id.to_string())
            .collect()
    }

    async fn peer_id_of(&self, index: usize) -> String {
        self.comms(index)
            .await
            .peer_id()
            .expect("member peer id")
            .to_string()
    }

    async fn wired_to(&self, index: usize) -> BTreeSet<AgentIdentity> {
        self.handle
            .get_member(self.member(index))
            .await
            .expect("roster read")
            .expect("member entry")
            .wired_to
    }

    fn wire_task(
        &self,
        a: usize,
        b: usize,
    ) -> tokio::task::JoinHandle<Result<(), crate::error::MobError>> {
        let handle = self.handle.clone();
        let a = self.member(a).clone();
        let b = self.member(b).clone();
        tokio::spawn(async move { handle.wire(a, b).await })
    }

    fn unwire_task(
        &self,
        a: usize,
        b: usize,
    ) -> tokio::task::JoinHandle<Result<(), crate::error::MobError>> {
        let handle = self.handle.clone();
        let a = self.member(a).clone();
        let b = self.member(b).clone();
        tokio::spawn(async move { handle.unwire(a, b).await })
    }
}

/// Park one member's trust mutations and release them on drop, so a failing
/// assertion cannot wedge the test's own teardown.
struct ReleaseTrustGate(Arc<TestRuntimeControlBarrier>);

impl Drop for ReleaseTrustGate {
    fn drop(&mut self) {
        self.0.release_all();
    }
}

async fn park_trust_mutations(mob: &WiringMob, index: usize) -> Arc<TestRuntimeControlBarrier> {
    let gate = Arc::new(TestRuntimeControlBarrier::new());
    let comms = mob.comms(index).await;
    *comms.trust_mutation_gate.write().expect("trust gate") = Some(Arc::clone(&gate));
    gate
}

async fn wait_for_gate_entry(gate: &Arc<TestRuntimeControlBarrier>, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while gate.boundary_calls.load(Ordering::Acquire) == 0 {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wire_blocked_on_trust_install_keeps_query_phase_responsive() {
    let mob = create_wiring_mob(2).await;
    let gate = park_trust_mutations(&mob, 0).await;
    let release = ReleaseTrustGate(Arc::clone(&gate));

    let wire = mob.wire_task(0, 1);
    wait_for_gate_entry(&gate, "wire trust install").await;

    let phase = mob.probe(Duration::from_secs(1)).await;
    let wire_finished_early = wire.is_finished();

    drop(release);
    let wired = tokio::time::timeout(Duration::from_secs(10), wire)
        .await
        .expect("wire settles after the trust gate releases")
        .expect("wire task");

    assert_eq!(
        phase.expect("QueryPhase must not wait behind a parked trust install"),
        MobState::Running
    );
    assert!(
        !wire_finished_early,
        "the wire cannot settle while its own trust install is parked"
    );
    wired.expect("wire");
    assert!(mob.wired_to(0).await.contains(mob.member(1)));
    let peer_id_of_1 = mob.peer_id_of(1).await;
    assert!(
        mob.trusted_peer_ids(0).await.contains(&peer_id_of_1),
        "the released effect must still install the trust row"
    );
    mob.handle.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wire_blocked_on_one_edge_does_not_block_a_disjoint_edge() {
    let mob = create_wiring_mob(4).await;
    let gate = park_trust_mutations(&mob, 0).await;
    let release = ReleaseTrustGate(Arc::clone(&gate));

    let parked = mob.wire_task(0, 1);
    wait_for_gate_entry(&gate, "parked edge trust install").await;

    // A disjoint edge shares no member with the parked one, so it must not
    // be fenced behind it.
    let disjoint = tokio::time::timeout(Duration::from_secs(5), mob.wire_task(2, 3))
        .await
        .expect("a disjoint edge must not wait for the parked edge")
        .expect("disjoint wire task");
    let parked_finished_early = parked.is_finished();

    drop(release);
    let parked = tokio::time::timeout(Duration::from_secs(10), parked)
        .await
        .expect("parked edge settles after release")
        .expect("parked wire task");

    disjoint.expect("disjoint wire");
    parked.expect("parked wire");
    assert!(
        !parked_finished_early,
        "the parked edge cannot settle before its gate releases"
    );
    assert!(mob.wired_to(2).await.contains(mob.member(3)));
    mob.handle.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retire_of_a_wiring_member_waits_for_its_owned_effects() {
    let mob = create_wiring_mob(3).await;
    let gate = park_trust_mutations(&mob, 0).await;
    let release = ReleaseTrustGate(Arc::clone(&gate));

    let wire = mob.wire_task(0, 1);
    wait_for_gate_entry(&gate, "wire trust install").await;

    // Same-member race: retiring a member whose wiring effects this actor
    // still owns must defer until that exact ledger is reconciled.
    let mut retire = {
        let handle = mob.handle.clone();
        let identity = mob.member(1).clone();
        tokio::spawn(async move { handle.retire(identity).await })
    };
    let early = tokio::time::timeout(Duration::from_millis(250), &mut retire).await;
    let retired_early = early.is_ok();
    let phase = mob.probe(Duration::from_secs(1)).await;

    // An unrelated member's control work stays independent while the retire
    // is fenced.
    let unrelated =
        tokio::time::timeout(Duration::from_secs(5), mob.handle.get_member(mob.member(2)))
            .await
            .expect("unrelated roster read must stay responsive")
            .expect("roster read");

    drop(release);
    let wired = tokio::time::timeout(Duration::from_secs(10), wire)
        .await
        .expect("wire settles after release")
        .expect("wire task");
    let retired = match early {
        Ok(result) => result,
        Err(_) => tokio::time::timeout(Duration::from_secs(15), retire)
            .await
            .expect("retire settles after the wiring ledger is reconciled"),
    };

    assert!(
        !retired_early,
        "retire must wait for the exact in-flight wiring effects of its member"
    );
    assert_eq!(
        phase.expect("QueryPhase stays responsive while retire is fenced"),
        MobState::Running
    );
    assert!(unrelated.is_some());
    wired.expect("wire");
    retired.expect("retire task").expect("retire");
    mob.handle.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wire_trust_failure_rolls_back_the_installed_row_and_the_machine_edge() {
    let mob = create_wiring_mob(2).await;
    // The B-side install fails after the A-side row was created: exactly the
    // partial-failure shape whose compensation must remove the A-side row and
    // revert the machine edge.
    let comms_b = mob.comms(1).await;
    comms_b.set_behavior(MockCommsBehavior {
        fail_add_trust: true,
        ..MockCommsBehavior::default()
    });

    let error = mob
        .handle
        .wire(mob.member(0).clone(), mob.member(1).clone())
        .await
        .expect_err("a failed trust install must surface typed");
    assert!(
        matches!(error, MobError::CommsError(_)),
        "the exact comms fault must survive the detached lane: {error}"
    );

    let peer_id_of_1 = mob.peer_id_of(1).await;
    assert!(
        !mob.trusted_peer_ids(0).await.contains(&peer_id_of_1),
        "compensation must remove the row this run created"
    );
    assert!(
        !mob.wired_to(0).await.contains(mob.member(1)),
        "a failed wire must leave no machine edge"
    );
    mob.handle.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unwire_blocked_on_its_notice_keeps_query_phase_responsive() {
    let mob = create_wiring_mob(2).await;
    mob.handle
        .wire(mob.member(0).clone(), mob.member(1).clone())
        .await
        .expect("wire");

    // `peer_unwired` is delivered before trust removal; park that notice.
    mob.comms(0).await.set_behavior(MockCommsBehavior {
        peer_lifecycle_delay_ms: 1_500,
        ..MockCommsBehavior::default()
    });

    let unwire = mob.unwire_task(0, 1);
    let phase = mob.probe(Duration::from_millis(500)).await;
    let unwire_finished_early = unwire.is_finished();

    let unwired = tokio::time::timeout(Duration::from_secs(10), unwire)
        .await
        .expect("unwire settles after the delayed notice")
        .expect("unwire task");

    assert_eq!(
        phase.expect("QueryPhase must not wait behind a slow peer notice"),
        MobState::Running
    );
    assert!(
        !unwire_finished_early,
        "the unwire cannot settle before its own notice completes"
    );
    unwired.expect("unwire");
    let peer_id_of_1 = mob.peer_id_of(1).await;
    assert!(
        !mob.trusted_peer_ids(0).await.contains(&peer_id_of_1),
        "a settled unwire removes the trust row"
    );
    assert!(!mob.wired_to(0).await.contains(mob.member(1)));
    mob.handle.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wire_members_batch_blocked_on_trust_keeps_query_phase_responsive() {
    let mob = create_wiring_mob(3).await;
    let gate = park_trust_mutations(&mob, 0).await;
    let release = ReleaseTrustGate(Arc::clone(&gate));

    let batch = {
        let handle = mob.handle.clone();
        let edges = vec![
            (mob.member(0).clone(), mob.member(1).clone()),
            (mob.member(1).clone(), mob.member(2).clone()),
        ];
        tokio::spawn(async move { handle.wire_members_batch(edges).await })
    };
    wait_for_gate_entry(&gate, "batch trust install").await;

    let phase = mob.probe(Duration::from_secs(1)).await;
    let batch_finished_early = batch.is_finished();

    drop(release);
    let report = tokio::time::timeout(Duration::from_secs(10), batch)
        .await
        .expect("batch settles after the trust gate releases")
        .expect("batch task")
        .expect("batch report");

    assert_eq!(
        phase.expect("QueryPhase must not wait behind a parked batch trust install"),
        MobState::Running
    );
    assert!(
        !batch_finished_early,
        "the batch cannot report before its trust phase settles"
    );
    assert_eq!(report.requested, 2);
    assert_eq!(report.wired.len(), 2);
    assert!(mob.wired_to(1).await.contains(mob.member(2)));
    mob.handle.shutdown().await.expect("shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wire_members_batch_trust_failure_rolls_back_every_row_it_created() {
    let mob = create_wiring_mob(3).await;
    // w-2 refuses its trust row, so the rows already created for the first
    // edge must be unwound: the batch keeps its all-or-nothing trust
    // semantics through the detached lane.
    mob.comms(2).await.set_behavior(MockCommsBehavior {
        fail_add_trust: true,
        ..MockCommsBehavior::default()
    });

    let error = mob
        .handle
        .wire_members_batch(vec![
            (mob.member(0).clone(), mob.member(1).clone()),
            (mob.member(1).clone(), mob.member(2).clone()),
        ])
        .await
        .expect_err("a failed batch trust install must surface typed");
    assert!(
        matches!(error, MobError::CommsError(_)),
        "the exact comms fault must survive the detached lane: {error}"
    );

    let peer_id_of_1 = mob.peer_id_of(1).await;
    let peer_id_of_0 = mob.peer_id_of(0).await;
    assert!(
        !mob.trusted_peer_ids(0).await.contains(&peer_id_of_1),
        "the batch must unwind every trust row it created"
    );
    assert!(!mob.trusted_peer_ids(1).await.contains(&peer_id_of_0));
    mob.handle.shutdown().await.expect("shutdown");
}
