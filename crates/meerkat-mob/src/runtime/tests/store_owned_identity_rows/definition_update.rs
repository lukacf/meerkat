//! A definition update against the created agents' rows: an update that
//! drops, or renames, a profile a created agent's row names is refused
//! before anything is written, an archived agent holds its profile as a
//! seated one does, a removed one holds nothing, and a start that carries
//! no update seats the agents as before. While a row of the mob cannot be
//! read by this build, an update that drops a profile is refused too, since
//! such a row may name it; an update that drops none judges no row.

use super::*;

/// The rows' definition: [`store_owned_definition`] with the profiles
/// `spare` and `courier` beside `worker`, each a copy of it.
fn definition_with_spare_profiles(label: &str) -> MobDefinition {
    let mut definition = store_owned_definition(label);
    let worker = definition.profiles[&ProfileName::from("worker")].clone();
    for name in ["spare", "courier"] {
        definition
            .profiles
            .insert(ProfileName::from(name), worker.clone());
    }
    definition
}

/// A host surface's create of `name` on `profile` as `operation_id`.
fn create_on(operation_id: &str, name: &str, profile: &str) -> StoreOwnedIdentityCreate {
    StoreOwnedIdentityCreate::new(
        operation(operation_id),
        IdentityChangeRequesterInput::host("console"),
        vec![CreatedIdentityRequest::new(spec(name, profile))],
    )
}

/// `definition` without the profile `name`.
fn without_profile(definition: &MobDefinition, name: &str) -> MobDefinition {
    let mut next = definition.clone();
    next.profiles.remove(&ProfileName::from(name));
    next
}

/// The diagnostics of a refused definition update, each as its code's wire
/// token, its location and its message.
fn refused_update(
    result: Result<crate::storage::MobDefinitionRevision, MobError>,
) -> Vec<(String, Option<String>, String)> {
    match result {
        Err(MobError::DefinitionError(diagnostics)) => diagnostics
            .into_iter()
            .map(|diagnostic| {
                (
                    diagnostic.code.to_string(),
                    diagnostic.location,
                    diagnostic.message,
                )
            })
            .collect(),
        other => panic!("expected a refused definition update, got {other:?}"),
    }
}

/// What a refused update must leave: the stored definition and the event
/// log's head, read before and compared after.
async fn stored(mob: &StoreOwnedMob) -> (Option<MobDefinition>, u64) {
    (
        mob.storage
            .created_definition()
            .await
            .expect("the stored definition reads"),
        mob.storage
            .events
            .latest_cursor()
            .await
            .expect("the event log's head reads"),
    )
}

/// Stop `mob`'s process, as a host does before it declares an update.
async fn stopped(mob: &StoreOwnedMob) {
    crash_stop_and_release_routes(mob.handle.clone()).await;
}

#[tokio::test]
async fn a_definition_update_that_drops_a_profile_an_agents_row_names_is_refused_and_writes_nothing()
 {
    let definition = definition_with_spare_profiles("definition-drop");
    let mob = StoreOwnedMob::build(definition.clone(), MobStorage::in_memory(), |b| b).await;
    committed(
        mob.create(create_on("create-1", "alpha", "spare"), ALLOW)
            .await,
    );
    stopped(&mob).await;
    let before = stored(&mob).await;

    let refused = refused_update(
        mob.storage
            .update_definition(1, without_profile(&definition, "spare"))
            .await,
    );
    assert_eq!(refused.len(), 1, "one diagnostic for the one held profile");
    let (code, location, message) = &refused[0];
    assert_eq!(code, "profile_held_by_identity");
    assert_eq!(location.as_deref(), Some("profiles.spare"));
    assert!(message.contains("alpha"), "the agent is named: {message}");
    assert!(
        message.contains("keep the profile") && message.contains("update or remove"),
        "the refusal says what the operator can do: {message}"
    );
    assert_eq!(stored(&mob).await, before, "nothing was written");
}

#[tokio::test]
async fn a_definition_update_that_renames_a_profile_an_agents_row_names_is_refused() {
    let definition = definition_with_spare_profiles("definition-rename");
    let mob = StoreOwnedMob::build(definition.clone(), MobStorage::in_memory(), |b| b).await;
    committed(
        mob.create(create_on("create-1", "alpha", "spare"), ALLOW)
            .await,
    );
    stopped(&mob).await;
    let before = stored(&mob).await;

    let mut renamed = definition.clone();
    let spare = renamed
        .profiles
        .remove(&ProfileName::from("spare"))
        .expect("the spare profile");
    renamed.profiles.insert(ProfileName::from("reserve"), spare);
    let refused = refused_update(mob.storage.update_definition(1, renamed).await);
    assert_eq!(
        refused
            .iter()
            .map(|(code, location, _)| (code.as_str(), location.as_deref()))
            .collect::<Vec<_>>(),
        vec![("profile_held_by_identity", Some("profiles.spare"))]
    );
    assert_eq!(stored(&mob).await, before, "nothing was written");
}

#[tokio::test]
async fn a_definition_update_that_keeps_every_profile_an_agents_row_names_passes() {
    let definition = definition_with_spare_profiles("definition-keep");
    let mob = StoreOwnedMob::build(definition.clone(), MobStorage::in_memory(), |b| b).await;
    committed(
        mob.create(create_on("create-1", "alpha", "spare"), ALLOW)
            .await,
    );
    stopped(&mob).await;

    // The courier profile is on no agent's row: dropping it is an update
    // like any other.
    let next = without_profile(&definition, "courier");
    let revision = mob
        .storage
        .update_definition(1, next.clone())
        .await
        .expect("an update that keeps the held profile");
    assert_eq!(revision.epoch, 2);
    assert_eq!(stored(&mob).await.0, Some(next));
}

#[tokio::test]
async fn an_archived_agent_holds_its_profile_and_a_removed_one_does_not() {
    let definition = definition_with_spare_profiles("definition-archived");
    let mob = StoreOwnedMob::build(definition.clone(), MobStorage::in_memory(), |b| b).await;
    committed(
        mob.create(create_on("create-1", "shelved", "spare"), ALLOW)
            .await,
    );
    committed(
        mob.create(create_on("create-2", "gone", "courier"), ALLOW)
            .await,
    );
    committed(mob.archive("archive-1", "shelved").await);
    committed(mob.remove("remove-1", "gone").await);
    stopped(&mob).await;
    let before = stored(&mob).await;

    // The archived agent's unarchive needs its profile.
    let refused = refused_update(
        mob.storage
            .update_definition(1, without_profile(&definition, "spare"))
            .await,
    );
    assert_eq!(
        refused
            .iter()
            .map(|(code, location, _)| (code.as_str(), location.as_deref()))
            .collect::<Vec<_>>(),
        vec![("profile_held_by_identity", Some("profiles.spare"))]
    );
    assert!(refused[0].2.contains("shelved"), "{}", refused[0].2);
    assert_eq!(stored(&mob).await, before, "nothing was written");

    // The removed agent's row is its name's tombstone: it holds no profile.
    let revision = mob
        .storage
        .update_definition(1, without_profile(&definition, "courier"))
        .await
        .expect("an update that drops only the removed agent's profile");
    assert_eq!(revision.epoch, 2);
}

#[tokio::test]
async fn the_refusal_names_the_first_agents_of_a_profile_and_counts_the_rest() {
    let definition = definition_with_spare_profiles("definition-many");
    let mob = StoreOwnedMob::build(definition.clone(), MobStorage::in_memory(), |b| b).await;
    let names = ["a1", "a2", "a3", "a4", "a5", "a6", "a7"];
    committed(
        mob.create(
            StoreOwnedIdentityCreate::new(
                operation("create-1"),
                IdentityChangeRequesterInput::host("console"),
                names
                    .iter()
                    .map(|name| CreatedIdentityRequest::new(spec(name, "spare")))
                    .collect(),
            ),
            ALLOW,
        )
        .await,
    );
    stopped(&mob).await;

    let refused = refused_update(
        mob.storage
            .update_definition(1, without_profile(&definition, "spare"))
            .await,
    );
    assert_eq!(refused.len(), 1);
    let message = &refused[0].2;
    for named in &names[..5] {
        assert!(message.contains(named), "{named} is named: {message}");
    }
    for counted in &names[5..] {
        assert!(
            !message.contains(counted),
            "{counted} is counted: {message}"
        );
    }
    assert!(message.contains("2 more"), "the rest is counted: {message}");
}

#[tokio::test]
async fn after_a_refused_definition_update_a_start_without_it_seats_the_agents() {
    let definition = definition_with_spare_profiles("definition-start");
    let mut mob = StoreOwnedMob::build(definition.clone(), MobStorage::in_memory(), |b| b).await;
    committed(
        mob.create(create_on("create-1", "alpha", "spare"), ALLOW)
            .await,
    );
    committed(
        mob.create(create_on("create-2", "shelved", "spare"), ALLOW)
            .await,
    );
    committed(mob.archive("archive-1", "shelved").await);
    stopped(&mob).await;
    refused_update(
        mob.storage
            .update_definition(1, without_profile(&definition, "spare"))
            .await,
    );

    // A start that declares no update resumes the stored definition.
    mob.restart_as(MobCompositionAuthority::Authoritative).await;
    assert_eq!(stored(&mob).await.0, Some(definition));
    assert!(mob.seated_dormant("alpha"), "alpha is seated as before");
    // Shelved is archived as before, and its unarchive finds its profile.
    mob.unseated_for("shelved", UnseatedEntryCause::Archived)
        .await;
    committed(mob.unarchive("unarchive-1", "shelved").await);
    assert!(
        matches!(mob.standing("shelved"), Some(EntryStanding::Seated(_))),
        "shelved is seated again: {:?}",
        mob.read("shelved")
    );
}

/// A row a later build wrote, in a schema this build cannot read.
fn later_build_row() -> Vec<u8> {
    br#"{"schema_version":3,"a_later_field":true}"#.to_vec()
}

/// In-memory storage, paired as [`MobStorage::in_memory`] pairs its stores,
/// and its identity store, which a row writes rows into as another build
/// left them.
fn storage_with_identity_rows() -> (MobStorage, InMemoryMobIdentityStore) {
    let events = InMemoryMobEventStore::new();
    let identity = InMemoryMobIdentityStore::paired_with_event_store(
        &events,
        Arc::new(crate::store::SystemMobIdentityStoreClock),
    );
    let identity_member: Arc<dyn crate::store::MobIdentityMemberStore> = Arc::new(identity.clone());
    let storage = MobStorage {
        events: Arc::new(events),
        identity: Arc::new(identity.clone()),
        identity_member: Some(identity_member),
        ..MobStorage::in_memory()
    };
    (storage, identity)
}

/// A stopped mob of `definition` with the agent `alpha` created on
/// `alphas_profile`, and with the rows of `later-1` and `later-2` as a
/// later build wrote them: this build reads neither, so it cannot say
/// which profile they name.
async fn stopped_mob_with_unreadable_rows(
    definition: &MobDefinition,
    alphas_profile: &str,
) -> StoreOwnedMob {
    let (storage, identity) = storage_with_identity_rows();
    let mob = StoreOwnedMob::build(definition.clone(), storage, |b| b).await;
    committed(
        mob.create(create_on("create-1", "alpha", alphas_profile), ALLOW)
            .await,
    );
    stopped(&mob).await;
    for name in ["later-1", "later-2"] {
        identity
            .put_raw_identity_row(&definition.id, &id(name), later_build_row())
            .await;
    }
    mob
}

#[tokio::test]
async fn a_definition_update_that_drops_a_profile_is_refused_while_an_agents_row_cannot_be_read() {
    let definition = definition_with_spare_profiles("definition-unreadable");
    let mut mob = stopped_mob_with_unreadable_rows(&definition, "worker").await;
    let before = stored(&mob).await;

    // The courier profile is on no row this build reads: the rows it
    // cannot read may name it.
    let refused = refused_update(
        mob.storage
            .update_definition(1, without_profile(&definition, "courier"))
            .await,
    );
    assert_eq!(
        refused.len(),
        1,
        "one diagnostic for the rows that cannot be read: {refused:?}"
    );
    let (code, location, message) = &refused[0];
    assert_eq!(code, "profile_held_by_identity");
    assert_eq!(location.as_deref(), Some("profiles"));
    for named in ["later-1", "later-2", "courier"] {
        assert!(message.contains(named), "{named} is named: {message}");
    }
    assert!(
        !message.contains("alpha"),
        "an agent whose row reads is not among them: {message}"
    );
    assert!(
        message.contains("keep") && message.contains("build that reads"),
        "the refusal says what the operator can do: {message}"
    );
    assert_eq!(stored(&mob).await, before, "nothing was written");

    // A start that declares no update is as it was: the stored definition,
    // alpha seated, and the rows still unreadable, each alone.
    mob.restart_as(MobCompositionAuthority::Authoritative).await;
    assert_eq!(stored(&mob).await.0, Some(definition));
    assert!(mob.seated_dormant("alpha"), "alpha is seated as before");
    for name in ["later-1", "later-2"] {
        assert!(
            matches!(
                mob.read(name),
                IdentityEntryRead::Unreadable { member: None }
            ),
            "{name} reads unreadable as before: {:?}",
            mob.read(name)
        );
    }
}

#[tokio::test]
async fn a_refused_update_names_the_agents_on_a_dropped_profile_and_then_the_rows_it_cannot_read() {
    let definition = definition_with_spare_profiles("definition-unreadable-both");
    let mob = stopped_mob_with_unreadable_rows(&definition, "spare").await;
    let before = stored(&mob).await;

    let refused = refused_update(
        mob.storage
            .update_definition(1, without_profile(&definition, "spare"))
            .await,
    );
    assert_eq!(
        refused
            .iter()
            .map(|(code, location, _)| (code.as_str(), location.as_deref()))
            .collect::<Vec<_>>(),
        vec![
            ("profile_held_by_identity", Some("profiles.spare")),
            ("profile_held_by_identity", Some("profiles")),
        ]
    );
    assert!(refused[0].2.contains("alpha"), "{}", refused[0].2);
    assert!(!refused[0].2.contains("later-"), "{}", refused[0].2);
    assert!(
        refused[1].2.contains("later-1") && refused[1].2.contains("later-2"),
        "{}",
        refused[1].2
    );
    assert!(!refused[1].2.contains("alpha"), "{}", refused[1].2);
    assert_eq!(stored(&mob).await, before, "nothing was written");
}

#[tokio::test]
async fn a_definition_update_that_drops_no_profile_passes_while_an_agents_row_cannot_be_read() {
    let definition = definition_with_spare_profiles("definition-unreadable-keep");
    let mob = stopped_mob_with_unreadable_rows(&definition, "spare").await;

    // An update that adds a profile and drops none judges no row.
    let mut next = definition.clone();
    let worker = next.profiles[&ProfileName::from("worker")].clone();
    next.profiles.insert(ProfileName::from("extra"), worker);
    let revision = mob
        .storage
        .update_definition(1, next.clone())
        .await
        .expect("an update that drops no profile");
    assert_eq!(revision.epoch, 2);
    assert_eq!(stored(&mob).await.0, Some(next));
}

#[tokio::test]
async fn a_created_row_scan_whose_cursor_does_not_advance_fails_the_update_and_writes_nothing() {
    let definition = definition_with_spare_profiles("definition-cursor");
    let (storage, identity) = storage_with_identity_rows();
    let mob = StoreOwnedMob::build(definition.clone(), storage, |b| b).await;
    committed(
        mob.create(create_on("create-1", "alpha", "worker"), ALLOW)
            .await,
    );
    stopped(&mob).await;
    identity.repeat_created_scan_cursor_for_test().await;
    let before = stored(&mob).await;

    let result = tokio::time::timeout(
        Duration::from_secs(20),
        mob.storage
            .update_definition(1, without_profile(&definition, "courier")),
    )
    .await
    .expect("the update answers: its scan does not follow a cursor that stands still");
    assert!(
        matches!(
            &result,
            Err(MobError::Internal(detail)) if detail.contains("non-advancing cursor")
        ),
        "the update fails on the cursor: {result:?}"
    );
    assert_eq!(stored(&mob).await, before, "nothing was written");
}
