//! The exact-admission comparison of one recorded row against the expected
//! original, on the row a native admission records.

use super::*;
use crate::input_authority::tests::input;
use crate::input_state::StoredInputState;

/// The row a native admission of `admitted` records.
fn row(admitted: &Input) -> InputState {
    StoredInputState::accepted_for_test(admitted)
        .expect("admitted row")
        .state
}

fn association(input: &Input) -> &InputAuthorityAssociation {
    input
        .header()
        .authority_association
        .as_ref()
        .expect("governed")
}

fn observed(
    state: &InputState,
    association: &InputAuthorityAssociation,
    expected: &Input,
) -> Result<(), AdmittedOriginalRefusal> {
    own_admission(state, association, expected).map(|_| ())
}

/// The expected original's own admission is observed: exactly one own
/// contributor, with its association, and the same canonical replay
/// identity.
#[test]
fn the_own_admission_of_the_expected_original_is_observed() {
    let expected = input("caller");
    let state = row(&expected);
    let own = own_admission(&state, association(&expected), &expected).expect("its own admission");
    assert_eq!(own.input_id(), expected.id());
    assert_eq!(own.association(), association(&expected));
}

/// A row whose own contributor is missing, duplicated or another input's,
/// or names another association, is not the expected original's admission.
#[test]
fn a_missing_duplicated_or_other_own_contributor_conflicts() {
    let expected = input("caller");
    let conflict = Err(AdmittedOriginalRefusal::Conflict);

    let mut missing = row(&expected);
    missing.authority_contributors.clear();
    assert_eq!(
        observed(&missing, association(&expected), &expected),
        conflict,
        "no own contributor"
    );

    let mut others = row(&expected);
    others.authority_contributors = row(&input("caller")).authority_contributors;
    assert_eq!(
        observed(&others, association(&expected), &expected),
        conflict,
        "only another input's contributor"
    );

    let mut duplicated = row(&expected);
    let own = duplicated.authority_contributors[0].clone();
    duplicated.authority_contributors.push(own);
    assert_eq!(
        observed(&duplicated, association(&expected), &expected),
        conflict,
        "a duplicated own contributor"
    );

    let other = input("another-requester");
    assert_ne!(association(&other), association(&expected));
    assert_eq!(
        observed(&row(&expected), association(&other), &expected),
        conflict,
        "another association"
    );
}

/// Presence of a prompt witness is not equality: a row without one, or an
/// expected prompt with other content under the same association and key,
/// conflicts.
#[test]
fn the_recorded_prompt_witness_must_equal_the_expected_prompt() {
    let expected = input("caller");
    let conflict = Err(AdmittedOriginalRefusal::Conflict);

    let mut unwitnessed = row(&expected);
    unwitnessed.prompt_replay_identity = None;
    assert_eq!(
        observed(&unwitnessed, association(&expected), &expected),
        conflict,
        "a row without a prompt witness"
    );

    let Input::Prompt(mut changed) = expected.clone() else {
        panic!("prompt fixture")
    };
    changed.content = meerkat_core::types::ContentInput::Text("other content".into());
    let changed = Input::Prompt(changed);
    assert_eq!(
        observed(&row(&expected), association(&changed), &changed),
        conflict,
        "other content under the same association and key"
    );
}

fn peer() -> Input {
    use crate::input::{InputOrigin, PeerConvention, PeerInput};
    let mut header = input("caller").header().clone();
    header.ingress_context = None;
    header.source = InputOrigin::Peer {
        peer_id: meerkat_core::comms::PeerId::from_uuid(uuid::Uuid::from_u128(7)).to_string(),
        display_identity: None,
        runtime_id: None,
    };
    Input::Peer(PeerInput {
        header,
        directed_interaction_id: None,
        convention: Some(PeerConvention::Message),
        content: "one peer delivery".into(),
        payload: None,
        handling_mode: None,
        sender_taint: None,
        objective_id: None,
        system_prompts: vec![],
        injected_context: vec![],
        in_reply_to: None,
    })
}

/// Low-level comparison controls use the same row constructor as the existing
/// prompt controls, with the qualified key a governed admission records.
/// The live/reopen controls exercise actual native admission separately.
fn peer_row(expected: &Input) -> InputState {
    let mut state = row(expected);
    state.idempotency_key =
        crate::input_authority::qualified_idempotency_key(expected).expect("qualified event key");
    state
}

#[test]
fn a_governed_peer_requires_exact_native_identity_after_payload_retirement() {
    let expected = peer();
    let state = peer_row(&expected);
    assert!(state.persisted_input.is_none());
    assert!(state.prompt_replay_identity.is_none());
    assert!(state.directed_run_started_attribution.is_none());
    assert!(observed(&state, association(&expected), &expected).is_ok());
    assert!(crate::input_state::verify_exact_replay(&state, &expected).is_ok());

    let mut later_observation = expected.clone();
    later_observation.header_mut().timestamp = chrono::DateTime::UNIX_EPOCH;
    assert!(
        crate::input_state::verify_exact_replay(&state, &later_observation).is_ok(),
        "native canonical replay retains its timestamp normalization"
    );

    for change in 0..9 {
        let mut changed = expected.clone();
        let Input::Peer(body) = &mut changed else {
            panic!("peer fixture")
        };
        match change {
            0 => body.header.id = InputId::new(),
            1 => body.header.idempotency_key = Some(IdempotencyKey::new("other-event")),
            2 => body.content = "other content".into(),
            3 => body.payload = Some(serde_json::json!({"different": true})),
            4 => {
                body.header.source = crate::input::InputOrigin::Peer {
                    peer_id: meerkat_core::comms::PeerId::from_uuid(uuid::Uuid::from_u128(8))
                        .to_string(),
                    display_identity: None,
                    runtime_id: None,
                }
            }
            5 => body.injected_context.push("other context".into()),
            6 => {
                let mut claims = body
                    .header
                    .authority_association
                    .as_ref()
                    .expect("claims")
                    .candidate()
                    .clone();
                claims.original_work.work =
                    meerkat_authorization_contracts::evidence::EvidenceId::new(
                        "other-delivery-work",
                    )
                    .expect("work");
                body.header.authority_association =
                    Some(InputAuthorityAssociation::new(claims).expect("well-formed changed work"));
            }
            7 => body.header.authority_association = None,
            _ => body.header.idempotency_key = None,
        }
        assert!(
            matches!(crate::input_state::verify_exact_replay(&state, &changed),
            Err(crate::RuntimeDriverError::InputIdempotencyConflict { existing_id })
                if existing_id == *expected.id()),
            "changed peer field {change}"
        );
    }

    for change in 0..5 {
        let mut changed = state.clone();
        match change {
            0 => changed.authority_contributors.clear(),
            1 => changed
                .authority_contributors
                .push(changed.authority_contributors[0].clone()),
            2 => changed.authority_contributors = peer_row(&peer()).authority_contributors,
            3 => changed.idempotency_key = expected.header().idempotency_key.clone(),
            _ => {
                let mut foreign = input("other-requester");
                foreign.header_mut().id = expected.id().clone();
                changed.authority_contributors[0] = row(&foreign).authority_contributors.remove(0);
            }
        }
        assert!(
            crate::input_state::verify_exact_replay(&changed, &expected).is_err(),
            "missing, duplicate, foreign or unqualified native witness {change}"
        );
    }

    let mut ungoverned = expected.clone();
    ungoverned.header_mut().authority_association = None;
    assert!(
        crate::input_state::verify_exact_replay(&row(&ungoverned), &ungoverned).is_err(),
        "an ungoverned row is not a governed original proof"
    );
}

#[test]
fn peer_proof_refuses_a_retained_resume_and_preserves_prompt_retry_semantics() {
    let expected = peer();
    let mut state = peer_row(&expected);
    let runtime = crate::identifiers::LogicalRuntimeId::for_session(&SessionId::new());
    state.retained_resume = Some(crate::retained_work::RetainedResumeRecord {
        identity: crate::retained_work::identity_from(
            &runtime,
            RunId::new(),
            &state.authority_contributors,
            &Default::default(),
            association(&expected).candidate().controller_model.clone(),
        )
        .expect("retained identity"),
        delivery: crate::retained_work::RetainedResumeDelivery {
            address: runtime,
            delivery_id: crate::delivery_inbox::RuntimeDeliveryId::new("continuation:other")
                .expect("delivery"),
            delivery_sequence: 1,
            submission_digest: "other-resume".into(),
        },
    });
    assert!(
        crate::input_state::verify_exact_replay(&state, &expected).is_err(),
        "a resume row cannot stand in for its contributor's own admission"
    );

    let prompt = input("caller");
    let state = row(&prompt);
    let mut retry = prompt.clone();
    retry.header_mut().id = InputId::new();
    retry.header_mut().timestamp = chrono::DateTime::UNIX_EPOCH;
    assert!(
        crate::input_state::verify_exact_replay(&state, &retry).is_ok(),
        "the existing prompt retry ID/time contract is unchanged"
    );
    let mut legacy = state;
    legacy.prompt_replay_identity = None;
    assert!(
        crate::input_state::verify_exact_replay(&legacy, &retry).is_err(),
        "old prompt rows do not acquire a new proof"
    );
}

/// The durable read on a real SQLite runtime store: a session the store
/// never held is unknown, a held runtime without the original's key is
/// absence, and once the runtime holds a row its key index cannot read, the
/// index proves neither presence nor absence and the read is unavailable.
#[cfg(feature = "sqlite-store")]
#[tokio::test]
async fn an_unprovable_sqlite_key_index_reads_unavailable() {
    use crate::store::{RuntimeStore, SqliteRuntimeStore};
    use DurableAdmittedOriginalRefusal::{NotAdmitted, Unavailable, UnknownRuntime};
    let dir = tempfile::TempDir::new().expect("temp dir");
    let path = dir.path().join("sessions.sqlite3");
    let store = Arc::new(SqliteRuntimeStore::new(path.clone()).expect("sqlite store"));
    let machine =
        MeerkatMachine::persistent_without_blobs(Arc::clone(&store) as Arc<dyn RuntimeStore>)
            .expect("persistent machine");
    let session = SessionId::new();
    let runtime = crate::identifiers::LogicalRuntimeId::for_session(&session);
    let expected = input("caller");

    let mut held = StoredInputState::new_accepted(InputId::new());
    held.state.idempotency_key = Some(crate::identifiers::IdempotencyKey::new("another-event"));
    store
        .persist_input_state(
            &runtime,
            &crate::input_state::InputStatePersistenceRecord::from_machine_snapshot(held)
                .expect("an authorized row"),
        )
        .await
        .expect("persist a decodable row of the session's runtime");
    assert_eq!(
        machine
            .durable_admitted_original(&SessionId::new(), &expected)
            .await,
        Err(UnknownRuntime),
        "a session the store never held"
    );
    assert_eq!(
        machine.durable_admitted_original(&session, &expected).await,
        Err(NotAdmitted),
        "a held runtime without the original's key"
    );

    let conn = rusqlite::Connection::open(&path).expect("open the store file");
    conn.execute(
        "INSERT INTO runtime_input_states (runtime_id, input_id, state_json) \
         VALUES (?1, ?2, ?3)",
        rusqlite::params![
            runtime.0.as_str(),
            "unreadable-input-row",
            b"{not-json".as_slice()
        ],
    )
    .expect("insert a row the key index cannot read");
    drop(conn);
    assert_eq!(
        machine.durable_admitted_original(&session, &expected).await,
        Err(Unavailable),
        "an index that cannot prove its rows is not absence"
    );
}
