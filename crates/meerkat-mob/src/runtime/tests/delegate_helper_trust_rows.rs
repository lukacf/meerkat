//! A delegate helper's reciprocal trust on the delegating side.
//!
//! A delegation that names its caller wires the helper to the caller as an
//! external peer and adds, on the caller's comms runtime, a trust row for
//! the helper. These rows follow that row over the helper's life: it is
//! there while the helper lives, and it is removed when the helper ends:
//! retired by the delegation service, retired because its delegation call
//! was dropped, or ended with its mob's destroy. They also pin what a
//! helper's end leaves alone: another live helper's row, the rows of
//! another owner of the caller's runtime, and a caller's runtime the
//! delegation never wired.
//!
//! The helper's session key pair follows the same line: it is there while
//! the helper is a member (and for a successor on the same session), and it
//! is deleted once the helper is retired, so a later build of that session
//! is another peer.

use super::*;
use crate::runtime::delegation::DelegationParentContext;

const MODEL: &str = "claude-sonnet-4-5";

/// A comms runtime of the delegating side: a real router and trust store.
fn caller_runtime(name: &str) -> Arc<meerkat_comms::CommsRuntime> {
    Arc::new(
        meerkat_comms::CommsRuntime::inproc_only(name)
            .expect("the delegating side's comms runtime"),
    )
}

fn caller_context(name: &str, runtime: &Arc<dyn CoreCommsRuntime>) -> DelegationParentContext {
    let peer_id = runtime
        .peer_id()
        .expect("the delegating side's runtime has a peer id");
    DelegationParentContext::new(name, peer_id, Arc::clone(runtime))
}

fn delegation_from(
    helper: &AgentIdentity,
    caller_name: &str,
    caller: &Arc<dyn CoreCommsRuntime>,
) -> DelegationExecutionRequest {
    let mut request = DelegationExecutionRequest::new(
        helper.clone(),
        "report back",
        BoundedResultSpec::new("delegate_result", 256).expect("valid delegate result spec"),
    );
    request.parent = Some(caller_context(caller_name, caller));
    request
}

/// A session's implicit delegation mob, as the `delegate` tool creates it:
/// the `delegate` profile, bound to its owner session, destroyed with it,
/// and marked a delegation mob in the machine.
async fn create_delegation_mob(label: &str, service: &Arc<MockSessionService>) -> MobHandle {
    MobBuilder::new(
        MobDefinition::implicit(label, MODEL),
        MobStorage::in_memory(),
    )
    .with_session_service(service.clone())
    .with_owner_bridge_session_create_authority(SessionId::new(), true, true)
    .create()
    .await
    .expect("create the delegation mob")
}

async fn delegation_mob(label: &str) -> (MobHandle, Arc<MockSessionService>) {
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    service.set_return_exact_run_result(true);
    let handle = create_delegation_mob(label, &service).await;
    (handle, service)
}

fn helper_comms_name(handle: &MobHandle, helper: &AgentIdentity) -> String {
    meerkat_core::MemberCommsName::new(handle.definition().id.as_str(), "delegate", helper.as_str())
        .expect("delegate helper comms name")
        .to_string()
}

async fn helper_peer_id(handle: &MobHandle, helper: &AgentIdentity) -> PeerId {
    handle
        .roster()
        .await
        .get_by_identity(helper)
        .and_then(|entry| entry.peer_id())
        .expect("a live delegate helper has a registered peer id")
}

async fn is_member(handle: &MobHandle, helper: &AgentIdentity) -> bool {
    handle
        .get_member(helper)
        .await
        .expect("roster lookup")
        .is_some()
}

/// The caller's router rows (its trust store), by peer id.
fn router_row_peer_ids(caller: &meerkat_comms::CommsRuntime) -> Vec<PeerId> {
    let mut peer_ids = caller
        .trusted_peers_shared()
        .entries()
        .into_iter()
        .map(|entry| entry.peer_id)
        .collect::<Vec<_>>();
    peer_ids.sort();
    peer_ids
}

fn router_row_names(caller: &meerkat_comms::CommsRuntime) -> Vec<String> {
    caller
        .trusted_peers_shared()
        .entries()
        .into_iter()
        .map(|entry| entry.name.as_str().to_string())
        .collect()
}

/// The caller's peers listing, by peer id.
async fn listed_peer_ids(caller: &dyn CoreCommsRuntime) -> Vec<PeerId> {
    CoreCommsRuntime::peers(caller)
        .await
        .into_iter()
        .map(|entry| entry.peer_id)
        .collect()
}

async fn listed_names(caller: &dyn CoreCommsRuntime) -> Vec<String> {
    CoreCommsRuntime::peers(caller)
        .await
        .into_iter()
        .map(|entry| entry.name.as_str().to_string())
        .collect()
}

/// The whole delegation in one call, as the `delegate` tool runs it: the
/// helper is wired to its caller, answers, and is retired. Once the call
/// returns, the caller's router holds no row for the helper and the
/// caller's peers listing does not name it.
#[tokio::test]
async fn a_retired_delegate_helper_is_no_peer_of_the_delegating_side() {
    let (handle, _service) = delegation_mob("helper-trust-retired").await;
    let caller_name = "caller/lead/retired";
    let caller = caller_runtime(caller_name);
    let caller_dyn: Arc<dyn CoreCommsRuntime> = caller.clone();
    let helper = AgentIdentity::from("helper-retired");
    let helper_name = helper_comms_name(&handle, &helper);

    let outcome = DelegationExecutionService::new(handle.clone())
        .execute(delegation_from(&helper, caller_name, &caller_dyn))
        .await
        .expect("the delegation succeeds");

    assert!(
        outcome.wired(),
        "a delegation that names its caller is wired to it"
    );
    assert_eq!(outcome.retirement_error(), None);
    assert!(
        !is_member(&handle, &helper).await,
        "the delegation service retires its helper"
    );
    assert!(
        !router_row_names(&caller).contains(&helper_name),
        "the caller's router holds no row for a retired helper: {:?}",
        router_row_names(&caller)
    );
    assert!(
        !listed_names(caller_dyn.as_ref())
            .await
            .contains(&helper_name),
        "the caller's peers listing does not name a retired helper"
    );
}

/// The same over the split lifecycle (start, terminal, retire), with a stop
/// and a resume of the helper's mob between the helper's terminal turn and
/// its retirement. The row is there while the helper lives, by peer id, and
/// is removed by the retirement.
#[tokio::test]
async fn a_delegate_helper_retired_after_a_stop_and_resume_is_no_peer_of_the_delegating_side() {
    let (handle, _service) = delegation_mob("helper-trust-resumed").await;
    let caller_name = "caller/lead/resumed";
    let caller = caller_runtime(caller_name);
    let caller_dyn: Arc<dyn CoreCommsRuntime> = caller.clone();
    let helper = AgentIdentity::from("helper-resumed");
    let delegation_service = DelegationExecutionService::new(handle.clone());

    let execution = delegation_service
        .start(delegation_from(&helper, caller_name, &caller_dyn))
        .await
        .expect("the delegation starts");
    let helper_peer = helper_peer_id(&handle, &helper).await;
    assert!(
        router_row_peer_ids(&caller).contains(&helper_peer),
        "a live helper has its row on the caller's router"
    );
    assert!(
        listed_peer_ids(caller_dyn.as_ref())
            .await
            .contains(&helper_peer),
        "a live helper is in the caller's peers listing"
    );
    let terminal = execution.await_terminal().await;
    assert!(terminal.wired());

    handle.stop().await.expect("stop the helper's mob");
    handle.resume().await.expect("resume the helper's mob");
    assert!(
        router_row_peer_ids(&caller).contains(&helper_peer),
        "a stop and a resume of the helper's mob keep a live helper's row"
    );

    delegation_service
        .retire_terminalized(&terminal)
        .await
        .expect("retire the helper");

    assert!(!is_member(&handle, &helper).await);
    assert!(
        !router_row_peer_ids(&caller).contains(&helper_peer),
        "the caller's router holds no row for a retired helper"
    );
    assert!(
        !listed_peer_ids(caller_dyn.as_ref())
            .await
            .contains(&helper_peer),
        "the caller's peers listing does not name a retired helper"
    );
}

/// Control: a helper that still lives keeps its row and its place in the
/// caller's peers listing through another helper's retirement.
#[tokio::test]
async fn a_live_delegate_helper_stays_a_peer_through_another_helpers_retirement() {
    let (handle, _service) = delegation_mob("helper-trust-live").await;
    let caller_name = "caller/lead/live";
    let caller = caller_runtime(caller_name);
    let caller_dyn: Arc<dyn CoreCommsRuntime> = caller.clone();
    let retiring = AgentIdentity::from("helper-retiring");
    let living = AgentIdentity::from("helper-living");
    let delegation_service = DelegationExecutionService::new(handle.clone());

    let retiring_execution = delegation_service
        .start(delegation_from(&retiring, caller_name, &caller_dyn))
        .await
        .expect("the first delegation starts");
    let living_execution = delegation_service
        .start(delegation_from(&living, caller_name, &caller_dyn))
        .await
        .expect("the second delegation starts");
    let living_peer = helper_peer_id(&handle, &living).await;
    let retiring_peer = helper_peer_id(&handle, &retiring).await;
    assert_ne!(living_peer, retiring_peer);
    let rows = router_row_peer_ids(&caller);
    assert!(rows.contains(&living_peer) && rows.contains(&retiring_peer));

    let retiring_terminal = retiring_execution.await_terminal().await;
    assert!(retiring_terminal.wired());
    delegation_service
        .retire_terminalized(&retiring_terminal)
        .await
        .expect("retire the first helper");
    assert!(!is_member(&handle, &retiring).await);

    assert!(
        is_member(&handle, &living).await,
        "the other helper still lives"
    );
    assert!(
        router_row_peer_ids(&caller).contains(&living_peer),
        "a live helper keeps its row through another helper's retirement"
    );
    assert!(
        listed_peer_ids(caller_dyn.as_ref())
            .await
            .contains(&living_peer),
        "a live helper stays in the caller's peers listing"
    );

    let living_terminal = living_execution.await_terminal().await;
    assert!(living_terminal.wired());
    delegation_service
        .retire_terminalized(&living_terminal)
        .await
        .expect("retire the second helper");
    assert!(!is_member(&handle, &living).await);
}

/// Pin: a caller's runtime takes one mob as the owner of its mob trust
/// rows. A helper of a second mob is therefore not wired back (`wired` is
/// false), and its retirement changes nothing on that runtime: the first
/// mob's row stays, by peer id and in the listing.
#[tokio::test]
async fn a_helper_that_was_not_wired_back_retires_without_touching_the_callers_rows() {
    let (first_mob, _first_service) = delegation_mob("helper-trust-first-owner").await;
    let (second_mob, _second_service) = delegation_mob("helper-trust-second-owner").await;
    let caller_name = "caller/lead/two-owners";
    let caller = caller_runtime(caller_name);
    let caller_dyn: Arc<dyn CoreCommsRuntime> = caller.clone();
    let first_helper = AgentIdentity::from("helper-first-owner");
    let second_helper = AgentIdentity::from("helper-second-owner");
    let first_service = DelegationExecutionService::new(first_mob.clone());
    let second_service = DelegationExecutionService::new(second_mob.clone());

    let first_execution = first_service
        .start(delegation_from(&first_helper, caller_name, &caller_dyn))
        .await
        .expect("the first mob's delegation starts");
    let first_peer = helper_peer_id(&first_mob, &first_helper).await;
    assert_eq!(router_row_peer_ids(&caller), vec![first_peer]);

    let second_execution = second_service
        .start(delegation_from(&second_helper, caller_name, &caller_dyn))
        .await
        .expect("the second mob's delegation starts");
    let second_peer = helper_peer_id(&second_mob, &second_helper).await;
    let second_terminal = second_execution.await_terminal().await;
    assert!(
        !second_terminal.wired(),
        "a runtime owned by another mob takes no reciprocal row of this one"
    );
    assert_eq!(
        router_row_peer_ids(&caller),
        vec![first_peer],
        "the refused add left the first mob's row alone"
    );

    second_service
        .retire_terminalized(&second_terminal)
        .await
        .expect("retire the second mob's helper");

    assert!(!is_member(&second_mob, &second_helper).await);
    assert_eq!(
        router_row_peer_ids(&caller),
        vec![first_peer],
        "the retirement of a helper that was not wired back changes no row"
    );
    let listed = listed_peer_ids(caller_dyn.as_ref()).await;
    assert!(listed.contains(&first_peer) && !listed.contains(&second_peer));
    assert!(
        is_member(&first_mob, &first_helper).await,
        "the first mob's helper still lives"
    );

    let first_terminal = first_execution.await_terminal().await;
    first_service
        .retire_terminalized(&first_terminal)
        .await
        .expect("retire the first mob's helper");
}

/// Pin: the caller's runtime stops answering (its session ended first)
/// before the helper is retired. The retirement still completes, and the
/// machine holds neither the helper's external edge nor its peer.
#[tokio::test]
async fn a_delegate_helper_retires_when_the_delegating_side_no_longer_answers() {
    let (handle, _service) = delegation_mob("helper-trust-caller-ended").await;
    let caller_name = "caller/lead/ended";
    let caller = Arc::new(MockCommsRuntime::new(
        caller_name,
        MockCommsBehavior::default(),
        SessionId::new(),
        None,
    ));
    let caller_dyn: Arc<dyn CoreCommsRuntime> = caller.clone();
    let helper = AgentIdentity::from("helper-caller-ended");
    let helper_name = helper_comms_name(&handle, &helper);
    let delegation_service = DelegationExecutionService::new(handle.clone());

    let execution = delegation_service
        .start(delegation_from(&helper, caller_name, &caller_dyn))
        .await
        .expect("the delegation starts");
    assert!(
        caller.trusted_peer_names().await.contains(&helper_name),
        "a live helper has its row on the caller's runtime"
    );
    let helper_peer = helper_peer_id(&handle, &helper).await;
    assert_eq!(
        caller
            .trusted_peer_sources
            .read()
            .await
            .get(&helper_peer.to_string())
            .copied(),
        Some(
            meerkat_core::comms::GeneratedCommsTrustAuthoritySourceKind::MobMachineExternalPeerTrustWiring
        ),
        "the helper's row is filed under the external peer wiring source"
    );
    let terminal = execution.await_terminal().await;
    assert!(terminal.wired());

    caller.set_behavior(MockCommsBehavior {
        missing_public_key: true,
        fail_remove_trust: true,
        ..MockCommsBehavior::default()
    });

    delegation_service
        .retire_terminalized(&terminal)
        .await
        .expect("the retirement completes without the caller's runtime");

    assert!(!is_member(&handle, &helper).await);
    let machine = handle
        .debug_dsl_t2_snapshot()
        .await
        .expect("machine state after the retirement");
    let helper_identity = crate::machines::mob_machine::AgentIdentity::from_domain(&helper);
    assert!(
        machine
            .external_peer_edges
            .iter()
            .all(|edge| edge.local != helper_identity),
        "the machine holds no external edge of the retired helper"
    );
    assert!(
        machine
            .external_peer_edges_by_key
            .keys()
            .all(|key| key.local != helper_identity),
        "the machine holds no external peer key of the retired helper"
    );
    assert!(
        !machine.member_peer_ids.contains_key(&helper_identity),
        "the machine holds no peer of the retired helper"
    );
}

/// Pin: the retirement asks only the runtime the delegation wired, and a
/// second retirement call for the same terminal asks nothing more. A
/// runtime built later for the same caller starts empty and is never
/// touched: it holds no row and no mob owner afterwards.
#[tokio::test]
async fn a_repeated_retirement_and_a_rebuilt_delegating_runtime_are_asked_nothing_more() {
    let (handle, _service) = delegation_mob("helper-trust-repeated").await;
    let caller_name = "caller/lead/repeated";
    let caller = Arc::new(MockCommsRuntime::new(
        caller_name,
        MockCommsBehavior::default(),
        SessionId::new(),
        None,
    ));
    let caller_dyn: Arc<dyn CoreCommsRuntime> = caller.clone();
    let helper = AgentIdentity::from("helper-repeated");
    let delegation_service = DelegationExecutionService::new(handle.clone());

    let execution = delegation_service
        .start(delegation_from(&helper, caller_name, &caller_dyn))
        .await
        .expect("the delegation starts");
    let terminal = execution.await_terminal().await;
    assert!(terminal.wired());

    // The caller's runtime as a later build of the same session would make
    // it: the same name and key, no rows, no owner.
    let rebuilt = MockCommsRuntime::new(
        caller_name,
        MockCommsBehavior::default(),
        SessionId::new(),
        None,
    );

    delegation_service
        .retire_terminalized(&terminal)
        .await
        .expect("retire the helper");
    assert!(!is_member(&handle, &helper).await);
    let rows_after_first = caller.trusted_peer_names().await;

    let repeated = delegation_service.retire_terminalized(&terminal).await;
    let plain = handle.retire(helper.clone()).await;
    assert_eq!(
        repeated.is_ok(),
        plain.is_ok(),
        "a repeated retirement call answers as a retirement of an absent member does: \
         repeated={repeated:?} plain={plain:?}"
    );
    assert_eq!(
        caller.trusted_peer_names().await,
        rows_after_first,
        "a repeated retirement call changes no row"
    );

    assert!(
        rebuilt.trusted_peer_names().await.is_empty(),
        "a rebuilt runtime holds no row"
    );
    assert!(
        rebuilt
            .mob_machine_trust_owner
            .read()
            .expect("mob trust owner lock")
            .is_none(),
        "a rebuilt runtime was never asked: it has no mob owner"
    );
}

const WAIT: Duration = Duration::from_secs(30);

/// A delegation mob whose members' comms key pairs live on disk, one
/// directory per session under the returned root, as an agent factory keeps
/// them: a member's peer id is the one its session's key pair names.
async fn delegation_mob_with_key_pairs(
    label: &str,
) -> (MobHandle, Arc<MockSessionService>, tempfile::TempDir) {
    let keys = tempfile::tempdir().expect("a comms identity root");
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    service.keep_member_comms_identities_under(keys.path());
    service.set_return_exact_run_result(true);
    let handle = create_delegation_mob(label, &service).await;
    (handle, service, keys)
}

async fn helper_session(handle: &MobHandle, helper: &AgentIdentity) -> SessionId {
    handle
        .resolve_bridge_session_id(helper)
        .await
        .expect("a session-backed helper")
}

fn key_pair_dir(root: &std::path::Path, session: &SessionId) -> std::path::PathBuf {
    root.join(session.to_string())
}

fn key_pair_files_exist(root: &std::path::Path, session: &SessionId) -> (bool, bool) {
    let dir = key_pair_dir(root, session);
    (
        dir.join("identity.key").exists(),
        dir.join("identity.pub").exists(),
    )
}

/// The peer id a build of `session` gets: the session's stored key pair's,
/// or a new key pair's when none is stored (the build's own load).
async fn peer_id_of_a_build(root: &std::path::Path, session: &SessionId) -> PeerId {
    meerkat_comms::Keypair::load_or_generate(&key_pair_dir(root, session))
        .await
        .expect("load or mint the session's key pair")
        .public_key()
        .to_peer_id()
}

async fn wait_until_no_member(handle: &MobHandle, helper: &AgentIdentity) -> bool {
    tokio::time::timeout(WAIT, async {
        while is_member(handle, helper).await {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .is_ok()
}

/// A delegation call that is dropped while it waits on its helper (its
/// caller's turn was cancelled): the helper is retired on its own, its row
/// leaves the caller's router and the caller's peers listing, and its
/// session's key pair is deleted.
#[tokio::test]
async fn a_dropped_delegation_retires_its_helper_and_leaves_no_row() {
    let (handle, service, keys) = delegation_mob_with_key_pairs("helper-trust-dropped").await;
    let caller_name = "caller/lead/dropped";
    let caller = caller_runtime(caller_name);
    let caller_dyn: Arc<dyn CoreCommsRuntime> = caller.clone();
    let helper = AgentIdentity::from("helper-dropped");
    let delegation_service = DelegationExecutionService::new(handle.clone());

    service.hold_start_turns();
    let mut held = service.held_start_turn_entries();
    let entered_before = *held.borrow_and_update();
    let (helper_peer, session) = {
        let call = delegation_service.execute(delegation_from(&helper, caller_name, &caller_dyn));
        tokio::pin!(call);
        tokio::time::timeout(WAIT, async {
            tokio::select! {
                outcome = &mut call => panic!(
                    "the helper's turn is held, so the call cannot end: ok={}",
                    outcome.is_ok()
                ),
                entered = held.wait_for(|entries| *entries > entered_before) => {
                    entered.expect("the helper's turn enters the hold");
                }
            }
        })
        .await
        .expect("the delegation reaches its helper's turn");
        let helper_peer = helper_peer_id(&handle, &helper).await;
        let session = helper_session(&handle, &helper).await;
        assert!(
            router_row_peer_ids(&caller).contains(&helper_peer),
            "a live helper has its row on the caller's router"
        );
        assert_eq!(
            key_pair_files_exist(keys.path(), &session),
            (true, true),
            "a live helper's session has its key pair"
        );
        (helper_peer, session)
        // The call is dropped here, while it waits on its helper.
    };
    service.release_held_start_turns();

    assert!(
        wait_until_no_member(&handle, &helper).await,
        "the helper of a dropped delegation is retired"
    );
    assert!(
        !router_row_peer_ids(&caller).contains(&helper_peer),
        "the caller's router holds no row for the helper of a dropped delegation"
    );
    assert!(
        !listed_peer_ids(caller_dyn.as_ref())
            .await
            .contains(&helper_peer),
        "the caller's peers listing does not name the helper of a dropped delegation"
    );
    // Nobody awaits this retirement (it runs on a detached task), and the
    // key pair is deleted once the seat has ended: after the member has left
    // the roster. Wait for the retirement's last step, bounded.
    let key_pair_deleted = tokio::time::timeout(WAIT, async {
        while key_pair_files_exist(keys.path(), &session) != (false, false) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .is_ok();
    assert!(
        key_pair_deleted,
        "the retired helper's session key pair is deleted: {:?}",
        key_pair_files_exist(keys.path(), &session)
    );
}

/// The destroy of the helper's mob ends a live helper too: afterwards the
/// caller's router holds no row for it, the caller's peers listing does not
/// name it, and its session's key pair is deleted.
#[tokio::test]
async fn a_destroyed_delegation_mob_leaves_no_helper_row_on_the_delegating_side() {
    let (handle, _service, keys) = delegation_mob_with_key_pairs("helper-trust-destroyed").await;
    let caller_name = "caller/lead/destroyed";
    let caller = caller_runtime(caller_name);
    let caller_dyn: Arc<dyn CoreCommsRuntime> = caller.clone();
    let helper = AgentIdentity::from("helper-destroyed");
    let delegation_service = DelegationExecutionService::new(handle.clone());

    let execution = delegation_service
        .start(delegation_from(&helper, caller_name, &caller_dyn))
        .await
        .expect("the delegation starts");
    let helper_peer = helper_peer_id(&handle, &helper).await;
    let session = helper_session(&handle, &helper).await;
    let terminal = execution.await_terminal().await;
    assert!(terminal.wired());
    assert!(
        router_row_peer_ids(&caller).contains(&helper_peer),
        "a live helper has its row on the caller's router"
    );
    assert_eq!(key_pair_files_exist(keys.path(), &session), (true, true));

    tokio::time::timeout(WAIT, handle.destroy())
        .await
        .expect("the destroy answers")
        .expect("destroy the helper's mob");

    assert!(
        !router_row_peer_ids(&caller).contains(&helper_peer),
        "the caller's router holds no row for a helper whose mob is destroyed"
    );
    assert!(
        !listed_peer_ids(caller_dyn.as_ref())
            .await
            .contains(&helper_peer),
        "the caller's peers listing does not name a helper whose mob is destroyed"
    );
    assert_eq!(
        key_pair_files_exist(keys.path(), &session),
        (false, false),
        "the destroyed mob's helper's session key pair is deleted"
    );
}

/// A retired delegate helper's session key pair is deleted once the helper
/// is retired, and not before: while the helper is a member the key pair is
/// there and names the peer the caller's row holds. A later build of that
/// session mints a new key pair, so it is another peer, one the caller's
/// router holds no row for.
#[tokio::test]
async fn a_retired_delegate_helpers_key_pair_is_deleted_and_its_session_rebuilds_as_another_peer() {
    let (handle, _service, keys) = delegation_mob_with_key_pairs("helper-trust-key").await;
    let caller_name = "caller/lead/key";
    let caller = caller_runtime(caller_name);
    let caller_dyn: Arc<dyn CoreCommsRuntime> = caller.clone();
    let helper = AgentIdentity::from("helper-key");
    let delegation_service = DelegationExecutionService::new(handle.clone());

    let execution = delegation_service
        .start(delegation_from(&helper, caller_name, &caller_dyn))
        .await
        .expect("the delegation starts");
    let helper_peer = helper_peer_id(&handle, &helper).await;
    let session = helper_session(&handle, &helper).await;
    let terminal = execution.await_terminal().await;
    assert!(terminal.wired());
    assert_eq!(
        key_pair_files_exist(keys.path(), &session),
        (true, true),
        "a helper that is still a member has its key pair"
    );
    assert_eq!(
        peer_id_of_a_build(keys.path(), &session).await,
        helper_peer,
        "the stored key pair names the peer the caller's row holds"
    );
    assert!(router_row_peer_ids(&caller).contains(&helper_peer));

    delegation_service
        .retire_terminalized(&terminal)
        .await
        .expect("retire the helper");

    assert!(!is_member(&handle, &helper).await);
    assert_eq!(
        key_pair_files_exist(keys.path(), &session),
        (false, false),
        "the retired helper's session key pair is deleted"
    );
    let rebuilt_peer = peer_id_of_a_build(keys.path(), &session).await;
    assert_ne!(
        rebuilt_peer, helper_peer,
        "a later build of the retired helper's session is another peer"
    );
    let rows = router_row_peer_ids(&caller);
    assert!(
        !rows.contains(&rebuilt_peer) && !rows.contains(&helper_peer),
        "the caller's router holds no row for either: {rows:?}"
    );
}

/// The same where the caller's runtime did not take the removal (it stopped
/// answering): its row for the retired helper stays, and names nobody. The
/// helper's key pair is deleted all the same, so a later build of the
/// helper's session is a peer that row does not name.
#[tokio::test]
async fn a_row_the_caller_kept_names_no_later_build_of_the_retired_helpers_session() {
    let (handle, _service, keys) = delegation_mob_with_key_pairs("helper-trust-kept-row").await;
    let caller_name = "caller/lead/kept-row";
    let caller = Arc::new(MockCommsRuntime::new(
        caller_name,
        MockCommsBehavior::default(),
        SessionId::new(),
        None,
    ));
    let caller_dyn: Arc<dyn CoreCommsRuntime> = caller.clone();
    let helper = AgentIdentity::from("helper-kept-row");
    let delegation_service = DelegationExecutionService::new(handle.clone());

    let execution = delegation_service
        .start(delegation_from(&helper, caller_name, &caller_dyn))
        .await
        .expect("the delegation starts");
    let helper_peer = helper_peer_id(&handle, &helper).await;
    let session = helper_session(&handle, &helper).await;
    let terminal = execution.await_terminal().await;
    assert!(terminal.wired());
    caller.set_behavior(MockCommsBehavior {
        fail_remove_trust: true,
        ..MockCommsBehavior::default()
    });

    delegation_service
        .retire_terminalized(&terminal)
        .await
        .expect("the retirement completes though the caller's runtime kept its row");

    assert!(!is_member(&handle, &helper).await);
    let kept_rows = caller
        .trusted_peers
        .read()
        .await
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        kept_rows,
        vec![helper_peer.to_string()],
        "the caller's runtime kept its row for the retired helper"
    );
    assert_eq!(
        key_pair_files_exist(keys.path(), &session),
        (false, false),
        "the retired helper's session key pair is deleted"
    );
    let rebuilt_peer = peer_id_of_a_build(keys.path(), &session).await;
    assert!(
        !kept_rows.contains(&rebuilt_peer.to_string()),
        "the kept row does not name a later build of the helper's session"
    );
}

/// Control: a helper respawned on its own session is the same peer. Its
/// key pair stays, byte for byte, and so does its row on the caller's
/// router.
#[tokio::test]
async fn a_helper_respawned_on_its_session_keeps_its_key_pair_and_its_row() {
    let (handle, _service, keys) = delegation_mob_with_key_pairs("helper-trust-respawned").await;
    let caller_name = "caller/lead/respawned";
    let caller = caller_runtime(caller_name);
    let caller_dyn: Arc<dyn CoreCommsRuntime> = caller.clone();
    let helper = AgentIdentity::from("helper-respawned");
    let delegation_service = DelegationExecutionService::new(handle.clone());

    let execution = delegation_service
        .start(delegation_from(&helper, caller_name, &caller_dyn))
        .await
        .expect("the delegation starts");
    let helper_peer = helper_peer_id(&handle, &helper).await;
    let session = helper_session(&handle, &helper).await;
    let terminal = execution.await_terminal().await;
    assert!(terminal.wired());
    let key_file = key_pair_dir(keys.path(), &session).join("identity.key");
    let key_before = std::fs::read(&key_file).expect("the helper's key file");

    tokio::time::timeout(
        WAIT,
        handle.respawn_member(helper.clone(), crate::runtime::MemberRespawn::new()),
    )
    .await
    .expect("the respawn answers")
    .expect("the helper is respawned on its own session");

    assert_eq!(
        helper_session(&handle, &helper).await,
        session,
        "the successor keeps the helper's session"
    );
    assert_eq!(
        std::fs::read(&key_file).expect("the successor's key file"),
        key_before,
        "a same-session successor keeps the key pair"
    );
    assert!(
        router_row_peer_ids(&caller).contains(&helper_peer),
        "a same-session successor keeps its row on the caller's router"
    );
}

/// Control: the key pair deletion belongs to a session's delegation mob. A
/// member under the same profile name in a mob that is no delegation mob
/// keeps its session's key pair at its retirement.
#[tokio::test]
async fn a_retired_member_of_a_mob_that_is_no_delegation_mob_keeps_its_key_pair() {
    let keys = tempfile::tempdir().expect("a comms identity root");
    let service = Arc::new(MockSessionService::new());
    let _ = service.enable_runtime_adapter();
    service.keep_member_comms_identities_under(keys.path());
    service.set_return_exact_run_result(true);
    let handle = MobBuilder::new(
        MobDefinition::implicit("helper-trust-plain-mob", MODEL),
        MobStorage::in_memory(),
    )
    .with_session_service(service.clone())
    .create()
    .await
    .expect("create a mob that is no delegation mob");
    let member = AgentIdentity::from("member-plain-mob");
    let delegation_service = DelegationExecutionService::new(handle.clone());

    let execution = delegation_service
        .start(DelegationExecutionRequest::new(
            member.clone(),
            "report back",
            BoundedResultSpec::new("delegate_result", 256).expect("valid delegate result spec"),
        ))
        .await
        .expect("the member's work starts");
    let session = helper_session(&handle, &member).await;
    let terminal = execution.await_terminal().await;
    assert_eq!(key_pair_files_exist(keys.path(), &session), (true, true));

    delegation_service
        .retire_terminalized(&terminal)
        .await
        .expect("retire the member");

    assert!(!is_member(&handle, &member).await);
    assert_eq!(
        key_pair_files_exist(keys.path(), &session),
        (true, true),
        "a member of a mob that is no delegation mob keeps its key pair"
    );
}

/// How long a row watches for something that must not happen.
const QUIET: Duration = Duration::from_secs(1);

/// Whether the key pair of `session` is gone from `root` within the wait.
/// Nobody awaits a retirement that a run's own task or a detached task
/// issues, and the key pair is deleted once the seat has ended.
async fn wait_until_no_key_pair(root: &std::path::Path, session: &SessionId) -> bool {
    tokio::time::timeout(WAIT, async {
        while key_pair_files_exist(root, session) != (false, false) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .is_ok()
}

/// A bounded delegation (the run owns its helper: it watches its own limit
/// and its own caller) that ends at its limit: the run cancels and retires
/// its helper itself. Afterwards the caller's router holds no row for the
/// helper, the caller's peers listing does not name it, and its session's
/// key pair is deleted.
#[tokio::test]
async fn a_bounded_delegation_ended_at_its_limit_leaves_no_row_and_no_key_pair() {
    let (handle, service, keys) = delegation_mob_with_key_pairs("helper-trust-limit").await;
    let caller_name = "caller/lead/limit";
    let caller = caller_runtime(caller_name);
    let caller_dyn: Arc<dyn CoreCommsRuntime> = caller.clone();
    let helper = AgentIdentity::from("helper-limit");
    let delegation_service = DelegationExecutionService::new(handle.clone());

    // The helper's turn does not end by itself, and the run pauses once its
    // limit has elapsed, before it cancels the helper.
    service.hold_start_turns();
    let (mut limit_elapsed, go_on) =
        crate::runtime::delegation::hold_delegation_limit_for_test(helper.clone());
    let call = delegation_service.execute_within(
        delegation_from(&helper, caller_name, &caller_dyn),
        DelegationRunBounds::new(Duration::from_millis(500), WAIT),
    );
    tokio::pin!(call);
    // A limit that elapses while the start is still in flight answers the
    // caller before the run reaches its pause: either order is this row's.
    let mut answer = None;
    tokio::time::timeout(WAIT, async {
        loop {
            tokio::select! {
                settled = &mut call, if answer.is_none() => answer = Some(settled),
                paused = &mut limit_elapsed => {
                    paused.expect("the run reaches its limit with its helper seated");
                    break;
                }
            }
        }
    })
    .await
    .expect("the run's limit elapses");

    let helper_peer = helper_peer_id(&handle, &helper).await;
    let session = helper_session(&handle, &helper).await;
    assert!(
        router_row_peer_ids(&caller).contains(&helper_peer),
        "a live helper has its row on the caller's router"
    );
    assert_eq!(
        key_pair_files_exist(keys.path(), &session),
        (true, true),
        "a live helper's session has its key pair"
    );

    drop(go_on);
    let answer = match answer {
        Some(answer) => answer,
        None => tokio::time::timeout(WAIT, &mut call)
            .await
            .expect("the delegation answers once its helper was ended"),
    };
    let failure = match answer {
        Ok(_) => panic!("a run that reached its limit has no result"),
        Err(failure) => failure,
    };
    assert!(
        matches!(
            failure.error,
            DelegationExecutionError::MaxRunElapsed { .. }
        ),
        "the delegation ended at its limit: {}",
        failure.error
    );

    assert!(
        wait_until_no_member(&handle, &helper).await,
        "the helper of a delegation that ended at its limit is retired"
    );
    assert!(
        !router_row_peer_ids(&caller).contains(&helper_peer),
        "the caller's router holds no row for the helper of a delegation that ended at its limit"
    );
    assert!(
        !listed_peer_ids(caller_dyn.as_ref())
            .await
            .contains(&helper_peer),
        "the caller's peers listing does not name the helper of a run that ended at its limit"
    );
    assert!(
        wait_until_no_key_pair(keys.path(), &session).await,
        "the retired helper's session key pair is deleted: {:?}",
        key_pair_files_exist(keys.path(), &session)
    );
    service.release_held_start_turns();
}

/// A bounded delegation whose caller stops listening (the call is dropped
/// while the helper's turn runs): the run's own task goes on, cancels and
/// retires its helper. Afterwards the caller's router holds no row for the
/// helper, the caller's peers listing does not name it, and its session's
/// key pair is deleted.
#[tokio::test]
async fn a_bounded_delegation_whose_caller_is_gone_leaves_no_row_and_no_key_pair() {
    let (handle, service, keys) = delegation_mob_with_key_pairs("helper-trust-caller-gone").await;
    let caller_name = "caller/lead/caller-gone";
    let caller = caller_runtime(caller_name);
    let caller_dyn: Arc<dyn CoreCommsRuntime> = caller.clone();
    let helper = AgentIdentity::from("helper-caller-gone");
    let delegation_service = DelegationExecutionService::new(handle.clone());

    service.hold_start_turns();
    let mut held = service.held_start_turn_entries();
    let entered_before = *held.borrow_and_update();
    let (helper_peer, session) = {
        let call = delegation_service.execute_within(
            delegation_from(&helper, caller_name, &caller_dyn),
            DelegationRunBounds::new(Duration::from_mins(10), WAIT),
        );
        tokio::pin!(call);
        tokio::time::timeout(WAIT, async {
            tokio::select! {
                outcome = &mut call => panic!(
                    "the helper's turn is held and the limit is far, so the call cannot end: ok={}",
                    outcome.is_ok()
                ),
                entered = held.wait_for(|entries| *entries > entered_before) => {
                    entered.expect("the helper's turn enters the hold");
                }
            }
        })
        .await
        .expect("the delegation reaches its helper's turn");
        let helper_peer = helper_peer_id(&handle, &helper).await;
        let session = helper_session(&handle, &helper).await;
        assert!(
            router_row_peer_ids(&caller).contains(&helper_peer),
            "a live helper has its row on the caller's router"
        );
        assert_eq!(
            key_pair_files_exist(keys.path(), &session),
            (true, true),
            "a live helper's session has its key pair"
        );
        (helper_peer, session)
        // The call is dropped here: nobody listens to the run any more.
    };

    assert!(
        wait_until_no_member(&handle, &helper).await,
        "the helper of a delegation nobody listens to is retired"
    );
    assert!(
        !router_row_peer_ids(&caller).contains(&helper_peer),
        "the caller's router holds no row for the helper of a delegation nobody listens to"
    );
    assert!(
        !listed_peer_ids(caller_dyn.as_ref())
            .await
            .contains(&helper_peer),
        "the caller's peers listing does not name the helper of a delegation nobody listens to"
    );
    assert!(
        wait_until_no_key_pair(keys.path(), &session).await,
        "the retired helper's session key pair is deleted: {:?}",
        key_pair_files_exist(keys.path(), &session)
    );
    service.release_held_start_turns();
}

/// The one-call form retires its helper and is done with it: once the call
/// has returned, a helper spawned again under the same name belongs to the
/// delegation that spawned it, and nothing of the returned call retires it.
#[tokio::test]
async fn a_helper_spawned_again_under_a_returned_delegations_name_stays_a_member() {
    let (handle, service) = delegation_mob("helper-trust-name-again").await;
    let caller_name = "caller/lead/name-again";
    let caller = caller_runtime(caller_name);
    let caller_dyn: Arc<dyn CoreCommsRuntime> = caller.clone();
    let helper = AgentIdentity::from("helper-name-again");
    let delegation_service = DelegationExecutionService::new(handle.clone());

    let outcome = delegation_service
        .execute(delegation_from(&helper, caller_name, &caller_dyn))
        .await
        .expect("the first delegation succeeds");
    assert_eq!(outcome.retirement_error(), None);
    assert!(
        !is_member(&handle, &helper).await,
        "the returned delegation retired its helper"
    );

    // The same name again, over the split lifecycle, its turn held: this
    // helper lives until the row retires it.
    service.hold_start_turns();
    let mut held = service.held_start_turn_entries();
    let entered_before = *held.borrow_and_update();
    let second = delegation_service
        .start(delegation_from(&helper, caller_name, &caller_dyn))
        .await
        .expect("a second helper starts under the returned delegation's name");
    tokio::time::timeout(WAIT, held.wait_for(|entries| *entries > entered_before))
        .await
        .expect("the second helper's turn is admitted")
        .expect("the second helper's turn enters the hold");
    let second_peer = helper_peer_id(&handle, &helper).await;

    tokio::time::sleep(QUIET).await;
    assert!(
        is_member(&handle, &helper).await,
        "nothing of the returned delegation retires a later helper of the same name"
    );
    assert!(
        router_row_peer_ids(&caller).contains(&second_peer),
        "the later helper is still its caller's peer"
    );

    service.release_held_start_turns();
    let terminal = second.await_terminal().await;
    delegation_service
        .retire_terminalized(&terminal)
        .await
        .expect("the second helper is retired by its own delegation");
    assert!(!is_member(&handle, &helper).await);
    assert!(
        !router_row_peer_ids(&caller).contains(&second_peer),
        "the caller's router holds no row for the second helper once it is retired"
    );
}

/// The one-call form asks its helper's retirement once and answers with
/// what that came to. A retirement that did not complete is the answer's
/// debt: once the call has returned, nothing of it asks for that retirement
/// again behind the caller.
#[tokio::test]
async fn a_returned_delegation_asks_its_helpers_retirement_no_second_time() {
    let (handle, service) = delegation_mob("helper-trust-debt").await;
    let caller_name = "caller/lead/debt";
    let caller = caller_runtime(caller_name);
    let caller_dyn: Arc<dyn CoreCommsRuntime> = caller.clone();
    let helper = AgentIdentity::from("helper-debt");
    let helper_session_name =
        test_comms_name_for(&handle.definition().id, "delegate", helper.as_str());
    service
        .set_archive_failure_for_comms_name(&helper_session_name)
        .await;

    let outcome = DelegationExecutionService::new(handle.clone())
        .execute(delegation_from(&helper, caller_name, &caller_dyn))
        .await
        .expect("the helper's result survives a retirement that did not complete");
    assert!(
        outcome.retirement_error().is_some(),
        "a retirement that did not complete is the answer's debt"
    );
    let session = service
        .session_comms_names
        .read()
        .await
        .iter()
        .find(|(_, name)| **name == helper_session_name)
        .map(|(session, _)| session.clone())
        .expect("the helper's session");
    let asked = service.archive_call_count(&session).await;
    assert!(
        asked > 0,
        "the delegation asked its helper's retirement before it returned"
    );

    tokio::time::sleep(QUIET).await;
    assert_eq!(
        service.archive_call_count(&session).await,
        asked,
        "nothing asks the helper's retirement again once the call has returned"
    );
}
