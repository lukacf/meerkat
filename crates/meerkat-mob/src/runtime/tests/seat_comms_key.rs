//! The comms key pair a resume seat binds. A seat records its member's
//! endpoint from the session's stored key pair. A built session whose key
//! pair is missing and whose endpoint no row of this mob recorded (one
//! imported without its key pair) gets a fresh key pair, as its first build
//! would; one whose endpoint this mob recorded is refused, typed, because
//! its peers trust the key that is gone. A resume seat of an archived
//! session that is no identity entry's archived record reads no key pair:
//! MobMachine refuses it, and it is answered `SessionArchived`. These rows
//! run on the combined candidate: the machine, the wake path and the boot
//! path together.
//!
//! Where the key pair lives: under the root the mob's session service
//! reports for its agent builds, unless the host declares one, which wins.
//! A service that reports no durable root seats no member without a
//! declared one.

use super::*;
use crate::machines::mob_machine as dsl;
use crate::runtime::DormantSeatOutcome;

const STEP: Duration = Duration::from_mins(1);

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

/// A resume seat of a session whose endpoint this mob recorded, archived
/// since by its member's retirement, and whose key pair is missing (a store
/// an earlier release wrote keeps its comms identities outside its state
/// directory, and its host revives a member it retires onto the same
/// session) is answered `SessionArchived`: the session is no identity
/// entry's archived record, so MobMachine refuses the seat, and the host
/// revives the identity through the spawn path, whose build gives the
/// session a key pair. The seat reads and mints no key pair and writes
/// nothing.
///
/// RED: read the key pair of every archived session a seat names, as the
/// preparation did once the declared apply's revival handed archived
/// sessions to MobMachine: the missing key pair refuses the seat first,
/// `WiringError` ("no comms identity"), and the host, never told the
/// session is archived, leaves the identity unseated.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_seat_on_an_archived_session_whose_recorded_key_pair_is_missing_is_answered_archived() {
    let keys = tempfile::tempdir().expect("a comms identity root");
    let service = Arc::new(MockSessionService::new());
    let _adapter = service.enable_runtime_adapter();
    let storage = MobStorage::in_memory();
    let handle = MobBuilder::new(
        with_unique_mob_id(sample_definition(), "seat-archived-lost-key"),
        storage.clone(),
    )
    .with_session_service(service.clone())
    .with_member_comms_identity_root(keys.path())
    .create()
    .await
    .expect("create the mob");
    // A seat records the member's endpoint, its first wake builds the
    // session, and its retirement archives it.
    let member = AgentIdentity::from("archived-owner");
    let outcomes = handle
        .seat_dormant_members(vec![turn_driven(&member)])
        .await
        .expect("the seat batch answers");
    let session = match &outcomes[0] {
        DormantSeatOutcome::Seated { session_id, .. } => session_id.clone(),
        other => panic!("the fresh spec is seated: {other:?}"),
    };
    tokio::time::timeout(STEP, handle.ensure_member_live(&member))
        .await
        .expect("the first wake answers")
        .expect("the first wake builds the session");
    tokio::time::timeout(STEP, handle.retire(member.clone()))
        .await
        .expect("the retire answers")
        .expect("the retire succeeds");
    assert!(
        service.test_is_session_archived(&session).await,
        "the retirement archived the session"
    );
    std::fs::remove_file(key_pair_path(keys.path(), &session)).expect("lose the key pair");
    let seat_rows = |journal: Vec<MobEvent>| {
        journal
            .iter()
            .filter(|event| {
                matches!(
                    &event.kind,
                    MobEventKind::MemberSpawned(spawned) if spawned.agent_identity == member
                )
            })
            .count()
    };
    let rows = seat_rows(
        storage
            .events
            .poll(0, usize::MAX)
            .await
            .expect("read the journal"),
    );

    let outcomes = handle
        .seat_dormant_members(vec![resume_of(&member, &session)])
        .await
        .expect("the seat batch answers");
    assert!(
        matches!(
            &outcomes[0],
            DormantSeatOutcome::SessionArchived { session_id, .. } if session_id == &session
        ),
        "the seat is answered archived, not refused for the missing key pair: {outcomes:?}"
    );
    assert!(
        !key_pair_path(keys.path(), &session).exists(),
        "no key pair was minted for the archived session"
    );
    assert!(
        handle
            .get_member(&member)
            .await
            .expect("roster read")
            .is_none(),
        "the identity is not seated"
    );
    assert_eq!(
        seat_rows(
            storage
                .events
                .poll(0, usize::MAX)
                .await
                .expect("read the journal"),
        ),
        rows,
        "the seat wrote no row"
    );
}

/// A host that declares no comms identity root seats an inproc member under
/// the root its session service reports for its agent builds: the seat
/// mints the member's key pair there, and the member's first wake builds it
/// with that key. RED before the report existed: with no declared root,
/// every seat was refused typed ("needs the members' comms identity
/// source"), and a host that declared the platform's default root instead
/// had the member's first wake refused (its endpoint disagreed with the
/// seat's).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_seat_with_no_declared_root_reads_the_key_under_the_root_the_service_reports() {
    let keys = tempfile::tempdir().expect("the service's comms identity root");
    let service = Arc::new(MockSessionService::new());
    service.keep_member_comms_identities_under(keys.path());
    let _adapter = service.enable_runtime_adapter();
    let handle = MobBuilder::new(
        with_unique_mob_id(sample_definition(), "seat-reported-root"),
        MobStorage::in_memory(),
    )
    .with_session_service(service.clone())
    .create()
    .await
    .expect("create the mob");
    let member = AgentIdentity::from("reported-root");
    let outcomes = handle
        .seat_dormant_members(vec![turn_driven(&member)])
        .await
        .expect("the seat batch answers");
    let session = match &outcomes[0] {
        DormantSeatOutcome::Seated { session_id, .. } => session_id.clone(),
        other => panic!("the member is seated under the reported root: {other:?}"),
    };
    assert!(
        key_pair_path(keys.path(), &session).exists(),
        "the seat minted the member's key pair under the root the service reports"
    );
    assert_eq!(
        phase_of(&handle, &member),
        Some(dsl::MemberMaterializationPhase::Dormant)
    );
    tokio::time::timeout(STEP, handle.ensure_member_live(&member))
        .await
        .expect("the wake answers")
        .expect("the member wakes with the key pair the seat read");
    assert_eq!(
        phase_of(&handle, &member),
        Some(dsl::MemberMaterializationPhase::Live),
        "the wake made the member Live"
    );
}

/// A declared root wins over the one the session service reports. Here the
/// service reports a root its builds do not use, and the host declares the
/// one they do: the seat reads the member's key under the declared root,
/// nothing is minted under the reported one, and the member wakes with the
/// declared root's key. A seat under the reported root would have recorded
/// a key the member's build never loads, which its first wake refuses.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_declared_root_wins_over_the_root_the_service_reports() {
    let keys = tempfile::tempdir().expect("the root the service's builds use");
    let reported = tempfile::tempdir().expect("the root the service reports");
    let service = Arc::new(MockSessionService::new());
    service.report_comms_identity_root(meerkat_core::SessionCommsIdentityRoot::Durable(
        reported.path().to_path_buf(),
    ));
    let _adapter = service.enable_runtime_adapter();
    let handle = MobBuilder::new(
        with_unique_mob_id(sample_definition(), "seat-declared-root"),
        MobStorage::in_memory(),
    )
    .with_session_service(service.clone())
    .with_member_comms_identity_root(keys.path())
    .create()
    .await
    .expect("create the mob");
    let member = AgentIdentity::from("declared-root");
    let outcomes = handle
        .seat_dormant_members(vec![turn_driven(&member)])
        .await
        .expect("the seat batch answers");
    let session = match &outcomes[0] {
        DormantSeatOutcome::Seated { session_id, .. } => session_id.clone(),
        other => panic!("the member is seated under the declared root: {other:?}"),
    };
    assert!(
        key_pair_path(keys.path(), &session).exists(),
        "the seat minted the member's key pair under the declared root"
    );
    assert!(
        !key_pair_path(reported.path(), &session).exists(),
        "nothing was minted under the root the service reports"
    );
    tokio::time::timeout(STEP, handle.ensure_member_live(&member))
        .await
        .expect("the wake answers")
        .expect("the member wakes with the declared root's key pair");
    assert_eq!(
        phase_of(&handle, &member),
        Some(dsl::MemberMaterializationPhase::Live),
        "the wake made the member Live"
    );
}

/// A session service that reports no durable comms identity root, or one
/// it cannot resolve, seats no inproc member unless the host declares a
/// root: every seat is refused typed, the refusal names the report, and
/// nothing of the seat is written. No other root (the platform's per-user
/// default) stands in for the one the service's builds use.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_service_without_a_durable_root_seats_nothing_without_a_declared_root() {
    for (label, report, named) in [
        (
            "seat-root-not-kept",
            None,
            "keeps no durable session comms identity",
        ),
        (
            "seat-root-unresolved",
            Some(meerkat_core::SessionCommsIdentityRoot::Unresolved(
                "HOME is not set".to_string(),
            )),
            "HOME is not set",
        ),
    ] {
        let service = Arc::new(MockSessionService::new());
        if let Some(report) = report {
            service.report_comms_identity_root(report);
        }
        let _adapter = service.enable_runtime_adapter();
        let handle = MobBuilder::new(
            with_unique_mob_id(sample_definition(), label),
            MobStorage::in_memory(),
        )
        .with_session_service(service.clone())
        .create()
        .await
        .expect("create the mob");
        let member = AgentIdentity::from("no-root");
        let outcomes = handle
            .seat_dormant_members(vec![turn_driven(&member)])
            .await
            .expect("the seat batch answers");
        assert!(
            matches!(
                &outcomes[0],
                DormantSeatOutcome::Refused {
                    error: MobError::WiringError(reason),
                    ..
                } if reason.contains(named)
                    && reason.contains("with_member_comms_identity_root")
            ),
            "{label}: {outcomes:?}"
        );
        assert_eq!(
            phase_of(&handle, &member),
            None,
            "{label}: nothing of the refused seat was written"
        );
    }
}
