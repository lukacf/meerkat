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
    assert!(own.association() == association(&expected));
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
    assert!(association(&other) != association(&expected));
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
