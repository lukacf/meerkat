//! The comms key pair a resume seat binds. A seat records its member's
//! endpoint from the session's stored key pair. A built session whose key
//! pair is missing and whose endpoint no row of this mob recorded (one
//! imported without its key pair) gets a fresh key pair, as its first build
//! would; one whose endpoint this mob recorded is refused, typed, because
//! its peers trust the key that is gone. These rows run on the combined
//! candidate: the machine, the wake path and the boot path together.

use super::*;
use crate::machines::mob_machine as dsl;
use crate::runtime::DormantSeatOutcome;

const STEP: Duration = Duration::from_secs(60);

fn turn_driven(identity: &AgentIdentity) -> SpawnMemberSpec {
    let mut spec = SpawnMemberSpec::new("worker", identity.clone());
    spec.runtime_mode = Some(crate::MobRuntimeMode::TurnDriven);
    spec
}

fn resume_of(identity: &AgentIdentity, session: &SessionId) -> SpawnMemberSpec {
    let mut spec = turn_driven(identity);
    spec.launch_mode = crate::launch::MemberLaunchMode::Resume {
        bridge_session_id: session.clone(),
        resume_from_role: None,
    };
    spec
}

fn phase_of(
    handle: &MobHandle,
    identity: &AgentIdentity,
) -> Option<dsl::MemberMaterializationPhase> {
    handle.member_materialization_for_test(identity)
}

fn key_pair_path(root: &std::path::Path, session: &SessionId) -> std::path::PathBuf {
    root.join(session.to_string()).join("identity.key")
}

/// An imported session (its transcript and metadata exist; its key pair and
/// any record of its endpoint do not) is seated by a resume seat Dormant,
/// with a fresh key pair where its builds load it. The member's wake then
/// builds it on that session with that key (the wake refuses a live endpoint
/// other than the one the seat recorded), and a wired peer's message to the
/// woken member is handed to it. A peer message's own wake of a dormant
/// member needs real comms (its wake route), and is covered on that stack
/// (`member_wake_rows`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_imported_session_without_a_key_pair_is_seated_with_a_fresh_one_and_wakes_on_it() {
    let definition = with_unique_mob_id(sample_definition(), "seat-imported-key");
    let member = AgentIdentity::from("imported");

    // The export: a member built its session in another deployment, whose
    // comms identities are not carried over.
    let exported_keys = tempfile::tempdir().expect("the exporting comms identity root");
    let exporting = Arc::new(MockSessionService::new());
    let _exporting_adapter = exporting.enable_runtime_adapter();
    let exporter = MobBuilder::new(definition.clone(), MobStorage::in_memory())
        .with_session_service(exporting.clone())
        .with_member_comms_identity_root(exported_keys.path())
        .create()
        .await
        .expect("create the exporting mob");
    exporter
        .spawn_spec(turn_driven(&member))
        .await
        .expect("spawn the member");
    let session = exporter
        .resolve_bridge_session_id(&member)
        .await
        .expect("a session-backed member");
    crash_stop_and_release_routes(exporter).await;

    // The import: the sessions, a new journal and an empty comms identity
    // root.
    let service = Arc::new(exporting.cold_restart_preserving_durable_state().await);
    let _adapter = service.enable_runtime_adapter();
    let keys = tempfile::tempdir().expect("an empty comms identity root");
    let handle = MobBuilder::new(definition, MobStorage::in_memory())
        .with_session_service(service.clone())
        .with_member_comms_identity_root(keys.path())
        .create()
        .await
        .expect("create the importing mob");
    let outcomes = handle
        .seat_dormant_members(vec![resume_of(&member, &session)])
        .await
        .expect("the seat batch answers");
    assert!(
        matches!(
            &outcomes[0],
            DormantSeatOutcome::Seated { session_id, .. } if session_id == &session
        ),
        "{outcomes:?}"
    );
    assert!(
        key_pair_path(keys.path(), &session).exists(),
        "the seat minted the session's key pair where its builds load it"
    );
    assert_eq!(
        phase_of(&handle, &member),
        Some(dsl::MemberMaterializationPhase::Dormant)
    );

    let sender = AgentIdentity::from("sender");
    handle
        .spawn_spec(turn_driven(&sender))
        .await
        .expect("spawn the sender");
    handle
        .wire(sender.clone(), member.clone())
        .await
        .expect("wire the sender to the imported member");

    tokio::time::timeout(STEP, handle.ensure_member_live(&member))
        .await
        .expect("the wake answers")
        .expect("the imported member wakes with the key pair the seat minted");
    assert_eq!(
        phase_of(&handle, &member),
        Some(dsl::MemberMaterializationPhase::Live),
        "the wake made the member Live"
    );
    assert_eq!(
        handle.resolve_bridge_session_id(&member).await,
        Some(session.clone()),
        "the member is woken on its imported session"
    );
    let receipt = tokio::time::timeout(
        STEP,
        handle.send_peer_message(
            sender,
            member.clone(),
            "a message for the imported member",
            meerkat_core::types::HandlingMode::Queue,
        ),
    )
    .await
    .expect("the send answers")
    .expect("the message is handed to the woken member");
    assert_eq!(receipt.to, member);
}

/// A resume seat of a built session whose endpoint this mob recorded (its
/// peers trust that key) and whose key pair is now missing is refused,
/// typed, and no key pair is minted for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resume_seat_of_a_session_whose_recorded_key_pair_is_missing_is_refused() {
    let keys = tempfile::tempdir().expect("a comms identity root");
    let service = Arc::new(MockSessionService::new());
    let _adapter = service.enable_runtime_adapter();
    let handle = MobBuilder::new(
        with_unique_mob_id(sample_definition(), "seat-lost-key"),
        MobStorage::in_memory(),
    )
    .with_session_service(service.clone())
    .with_member_comms_identity_root(keys.path())
    .create()
    .await
    .expect("create the mob");
    // A seat records the member's endpoint; its first wake builds the
    // session.
    let owner = AgentIdentity::from("key-owner");
    let outcomes = handle
        .seat_dormant_members(vec![turn_driven(&owner)])
        .await
        .expect("the seat batch answers");
    let session = match &outcomes[0] {
        DormantSeatOutcome::Seated { session_id, .. } => session_id.clone(),
        other => panic!("the fresh spec is seated: {other:?}"),
    };
    tokio::time::timeout(STEP, handle.ensure_member_live(&owner))
        .await
        .expect("the first wake answers")
        .expect("the first wake builds the session");
    std::fs::remove_file(key_pair_path(keys.path(), &session)).expect("lose the key pair");

    let outcomes = handle
        .seat_dormant_members(vec![resume_of(&AgentIdentity::from("second"), &session)])
        .await
        .expect("the seat batch answers");
    assert!(
        matches!(
            &outcomes[0],
            DormantSeatOutcome::Refused {
                error: MobError::WiringError(reason),
                ..
            } if reason.contains("no comms identity")
        ),
        "{outcomes:?}"
    );
    assert!(
        !key_pair_path(keys.path(), &session).exists(),
        "no key pair was minted for a session whose peers trust another"
    );
}
