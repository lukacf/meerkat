//! The downgrade cycle (0.9.11): the export of a mob's rows of schema
//! version 2 and later with their session references, the host operator's
//! purge of them before a downgrade, the reservation of a Created entry
//! whose row was purged, and the import that restores them after the
//! roll-forward, with the in-process legs and the fixtures of the cycle's
//! rollback rows.

use super::*;
use crate::identity::{
    CREATED_IDENTITY_SET_SCHEMA, CreatedIdentitySetExport, ExportedSessionState,
    IdentityChangeEntryOutcome, IdentityChangeKind,
};
use crate::runtime::{
    AdoptionSource, HistoryUnavailable, IdentityAdoption, IdentityImport, IdentityRowsPurge,
    PurgeRefusedReason,
};
use meerkat_core::approval::decision::{ConsentProvenance, DecisionReference};

/// The consent a host operator's import carries: decided outside Meerkat.
fn imported_consent() -> ConsentProvenance {
    ConsentProvenance::Imported {
        source: DecisionReference::new("downgrade-cycle").expect("the import's source"),
        reference: DecisionReference::new("export-1").expect("the import's reference"),
        approver_id: None,
        decided_at_ms: None,
    }
}

/// The outcome the receipt records for `name`.
fn outcome_of(receipt: &crate::IdentityChangeReceipt, name: &str) -> IdentityChangeEntryOutcome {
    receipt
        .identities
        .iter()
        .find(|entry| entry.identity == id(name))
        .unwrap_or_else(|| panic!("the receipt names {name}: {receipt:?}"))
        .outcome
}

impl StoreOwnedMob {
    /// Hold, or release, the mob's identity changes.
    async fn hold_changes(&self, held: bool) {
        self.handle
            .set_identity_changes_held(held)
            .await
            .expect("the hold answers");
    }

    /// The mob's created identity set.
    async fn export_set(&self) -> CreatedIdentitySetExport {
        tokio::time::timeout(STEP, self.handle.export_created_identities())
            .await
            .expect("the export answers")
            .expect("the export reads the rows")
    }

    /// `requester`'s purge under `export_digest`, as `operation_id`.
    async fn purge_as(
        &self,
        operation_id: &str,
        export_digest: &str,
        requester: IdentityChangeRequesterInput,
    ) -> Result<IdentityChangeOutcome, MobError> {
        tokio::time::timeout(
            STEP,
            self.handle.purge_created_identities(IdentityRowsPurge::new(
                operation(operation_id),
                requester,
                export_digest,
            )),
        )
        .await
        .expect("the purge answers")
    }

    /// The host operator's purge under `export_digest`.
    async fn purge(
        &self,
        operation_id: &str,
        export_digest: &str,
    ) -> Result<IdentityChangeOutcome, MobError> {
        self.purge_as(
            operation_id,
            export_digest,
            IdentityChangeRequesterInput::host_operator("console"),
        )
        .await
    }

    /// The downgrade cycle's steps before the downgrade: hold the changes,
    /// export the set, purge its rows. Answers the export.
    async fn export_and_purge(&self, operation_id: &str) -> CreatedIdentitySetExport {
        self.hold_changes(true).await;
        let export = self.export_set().await;
        committed(self.purge(operation_id, &export.export_digest).await);
        export
    }

    /// The host operator's import of `export` under imported consent.
    async fn import(
        &self,
        operation_id: &str,
        export: CreatedIdentitySetExport,
    ) -> Result<IdentityChangeOutcome, MobError> {
        let consent = imported_consent();
        tokio::time::timeout(
            STEP,
            self.handle.import_created_identities(
                IdentityImport::new(
                    operation(operation_id),
                    IdentityChangeRequesterInput::host_operator("console"),
                    export,
                ),
                IdentityChangeConsentInput::Imported(&consent),
            ),
        )
        .await
        .expect("the import answers")
    }

    /// A store-owned identity seated live: its kickoff ran on its session,
    /// which the session store keeps. Answers the session.
    async fn create_live(&self, name: &str) -> SessionId {
        committed(
            self.create(
                StoreOwnedIdentityCreate::new(
                    operation(&format!("create-{name}")),
                    IdentityChangeRequesterInput::host("console"),
                    vec![CreatedIdentityRequest::new(
                        spec(name, "worker")
                            .with_runtime_mode_override(crate::MobRuntimeMode::AutonomousHost)
                            .with_initial_message(meerkat_core::types::ContentInput::from(
                                "Begin.",
                            )),
                    )],
                ),
                ALLOW,
            )
            .await,
        );
        kicked_off(self, name, "Begin.").await
    }

    /// Whether `name` reads as a store-owned identity.
    fn reads_store_owned(&self, name: &str) -> bool {
        matches!(
            self.read(name),
            IdentityEntryRead::Entry(view) if view.origin == IdentityEntryOrigin::Created
        )
    }

    /// The host's retirement of `name`'s member.
    async fn retire_member(&self, name: &str) {
        tokio::time::timeout(STEP, self.handle.retire(id(name)))
            .await
            .expect("the retirement answers")
            .expect("retire the member");
    }
}

/// Row (RED): the export carries every row of the set (a seated identity's,
/// an archived one's, a removed name's tombstone), each with its session
/// reference: a seated member's session (current), an archived identity's
/// archived session (archived), none for a tombstone. It is sealed with the
/// digest of its rows and reads the same twice.
///
/// RED without the export: the handle has no export step.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn export_carries_each_identitys_session_reference() {
    let mob = StoreOwnedMob::new("cycle-export").await;
    mob.service.set_keep_alive_turns_complete_immediately(true);
    let seated = mob.create_live("seated").await;
    let parked = mob.create_live("parked").await;
    committed(
        mob.create(host_create("create-gone", &["gone"]), ALLOW)
            .await,
    );
    committed(mob.archive("archive-parked", "parked").await);
    mob.unseated_for("parked", UnseatedEntryCause::Archived)
        .await;
    committed(mob.remove("remove-gone", "gone").await);
    mob.removed("gone").await;

    let export = mob.export_set().await;
    assert_eq!(export.schema, CREATED_IDENTITY_SET_SCHEMA);
    assert_eq!(export.mob_id, mob.mob_id());
    export.validate().expect("the export is sealed");
    let session_of = |name: &str| {
        export
            .identities
            .iter()
            .find(|exported| exported.record.agent_identity == id(name))
            .unwrap_or_else(|| panic!("the export holds {name}"))
            .session
            .clone()
            .map(|session| (session.session_id, session.state))
    };
    assert_eq!(
        session_of("seated"),
        Some((seated, ExportedSessionState::Current))
    );
    assert_eq!(
        session_of("parked"),
        Some((parked, ExportedSessionState::Archived))
    );
    assert_eq!(session_of("gone"), None);
    assert_eq!(export.identities.len(), 3);
    assert_eq!(mob.export_set().await, export, "the set did not move");
}

/// Row (RED): the purge needs the host operator, held changes and an export
/// its rows still match; each refusal removes nothing. Then it removes the
/// rows, its receipt names each, and a retry replays it.
///
/// RED without the purge: the handle has no purge step.
#[tokio::test]
async fn purge_needs_held_changes_and_a_matching_export() {
    let mob = StoreOwnedMob::new("cycle-purge-guards").await;
    committed(
        mob.create(host_create("create-alpha", &["alpha"]), ALLOW)
            .await,
    );
    let export = mob.export_set().await;
    assert_eq!(
        refusal(mob.purge("purge-unheld", &export.export_digest).await),
        IdentityChangeRefusal::PurgeRefused {
            reason: PurgeRefusedReason::ChangesNotHeld
        }
    );
    mob.hold_changes(true).await;
    assert_eq!(
        refusal(
            mob.purge_as(
                "purge-host",
                &export.export_digest,
                IdentityChangeRequesterInput::host("console"),
            )
            .await
        ),
        IdentityChangeRefusal::PurgeRefused {
            reason: PurgeRefusedReason::NotHostOperator
        }
    );
    assert_eq!(
        refusal(mob.purge("purge-stale", "sha256:0").await),
        IdentityChangeRefusal::PurgeRefused {
            reason: PurgeRefusedReason::ExportMismatch {
                current_digest: export.export_digest.clone()
            }
        }
    );
    assert!(
        mob.rows().await.contains_key(&id("alpha")),
        "a refused purge removes nothing"
    );

    let receipt = committed(mob.purge("purge-alpha", &export.export_digest).await);
    assert_eq!(receipt.change, IdentityChangeKind::Purge);
    assert_eq!(
        outcome_of(&receipt, "alpha"),
        IdentityChangeEntryOutcome::Purged
    );
    assert!(mob.rows().await.is_empty());
    assert!(matches!(
        mob.purge("purge-alpha", &export.export_digest).await,
        Ok(IdentityChangeOutcome::Replayed { .. })
    ));
}

/// Row (RED): the purge removes the rows and nothing else: no journal row
/// is written or removed, the member stays seated on its session, and the
/// receipts of earlier changes stay.
///
/// RED without the purge: the handle has no purge step.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn purge_removes_v2_rows_only_never_sessions_or_journal_rows() {
    let mob = StoreOwnedMob::new("cycle-purge-scope").await;
    mob.service.set_keep_alive_turns_complete_immediately(true);
    let session = mob.create_live("alpha").await;
    committed(
        mob.create(host_create("create-gone", &["gone"]), ALLOW)
            .await,
    );
    committed(mob.remove("remove-gone", "gone").await);
    mob.removed("gone").await;
    let journal = mob.journal().await;

    mob.export_and_purge("purge-scope").await;
    assert!(mob.rows().await.is_empty(), "every row is removed");
    assert_eq!(
        mob.journal().await.len(),
        journal.len(),
        "no journal row is written or removed"
    );
    assert_eq!(mob.session("alpha").await, session, "the member stays");
    assert!(
        mob.handle
            .identity_operation("create-alpha")
            .await
            .expect("the receipt read answers")
            .is_some(),
        "an earlier change's receipt stays"
    );
}

/// Row: after a purge, the mob refuses every identity change, the hold
/// released or not, until it restarts.
#[tokio::test]
async fn after_a_purge_identity_changes_are_refused_until_restart() {
    let mut mob = StoreOwnedMob::new("cycle-purge-restart").await;
    committed(
        mob.create(host_create("create-alpha", &["alpha"]), ALLOW)
            .await,
    );
    mob.export_and_purge("purge-restart").await;
    mob.hold_changes(false).await;
    assert_eq!(
        refusal(
            mob.create(host_create("create-beta", &["beta"]), ALLOW)
                .await
        ),
        IdentityChangeRefusal::Purged
    );
    mob.restart_as(MobCompositionAuthority::Authoritative).await;
    committed(
        mob.create(host_create("create-beta", &["beta"]), ALLOW)
            .await,
    );
}

/// Row: the census a host's downgrade tooling refuses on counts every row
/// of the set before the purge, and none after it.
#[tokio::test]
async fn downgrade_tooling_refuses_while_any_v2_row_exists() {
    let mob = StoreOwnedMob::new("cycle-census").await;
    committed(
        mob.create(host_create("create-desks", &["kept", "gone"]), ALLOW)
            .await,
    );
    committed(mob.remove("remove-gone", "gone").await);
    mob.removed("gone").await;
    let census = mob
        .handle
        .identity_record_census()
        .await
        .expect("the census answers");
    assert_eq!(census.created_rows, 2, "{census:?}");
    assert!(
        !mob.storage
            .identity_record_rows()
            .await
            .expect("count the rows")
            .is_empty()
    );

    mob.export_and_purge("purge-census").await;
    let census = mob
        .handle
        .identity_record_census()
        .await
        .expect("the census answers");
    assert_eq!(census.created_rows, 0, "{census:?}");
    assert!(
        mob.storage
            .identity_record_rows()
            .await
            .expect("count the rows")
            .is_empty()
    );
}

/// Row (RED): a start after the purge reads a Created entry whose row is
/// gone as unreadable: its name is reserved (a create of it is refused) and
/// nothing acts on it, until the import restores its row.
///
/// RED without the reservation: the start reads it a store-owned identity.
#[tokio::test]
async fn a_created_origin_without_a_row_reads_unreadable_until_import() {
    let mut mob = StoreOwnedMob::new("cycle-rowless").await;
    committed(
        mob.create(host_create("create-alpha", &["alpha"]), ALLOW)
            .await,
    );
    let export = mob.export_and_purge("purge-rowless").await;
    mob.restart_as(MobCompositionAuthority::Authoritative).await;
    assert!(
        matches!(
            mob.read("alpha"),
            IdentityEntryRead::Unreadable { member: Some(_) }
        ),
        "{:?}",
        mob.read("alpha")
    );
    assert!(matches!(
        refusal(
            mob.create(host_create("create-alpha-again", &["alpha"]), ALLOW)
                .await
        ),
        IdentityChangeRefusal::NameTaken { .. }
    ));

    let receipt = committed(mob.import("import-rowless", export).await);
    assert_eq!(receipt.change, IdentityChangeKind::Import);
    assert_eq!(
        outcome_of(&receipt, "alpha"),
        IdentityChangeEntryOutcome::Committed
    );
    assert!(mob.reads_store_owned("alpha"));
    assert!(mob.rows().await.contains_key(&id("alpha")));
}

/// Row (RED): a store-owned identity whose member retired while its row was
/// purged is not seated again by the next start: no row, no seat.
///
/// RED without the reservation: the start seats it again from its last
/// spawn row.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_owed_created_seat_without_its_row_is_not_reseated() {
    let mut mob = StoreOwnedMob::new("cycle-no-reseat").await;
    mob.service.set_keep_alive_turns_complete_immediately(true);
    mob.create_live("alpha").await;
    mob.export_and_purge("purge-no-reseat").await;
    mob.retire_member("alpha").await;
    assert!(!mob.is_member("alpha").await);
    mob.restart_as(MobCompositionAuthority::Authoritative).await;
    assert!(!mob.is_member("alpha").await, "nothing seats it");
    assert!(
        matches!(
            mob.read("alpha"),
            IdentityEntryRead::Unreadable { member: None }
        ),
        "{:?}",
        mob.read("alpha")
    );
}

/// Row (RED): the import re-binds each identity's session: a member seated
/// under its name is bound in place, on the same session, and one retired
/// while the row was purged is seated again, Dormant, on its archived
/// session.
///
/// RED without the import: the handle has no import step.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_rebinds_a_seated_member_in_place_and_a_retired_one_on_its_archived_session() {
    let mut mob = StoreOwnedMob::new("cycle-rebind").await;
    mob.service.set_keep_alive_turns_complete_immediately(true);
    let seated = mob.create_live("seated").await;
    let retired = mob.create_live("retired").await;
    let export = mob.export_and_purge("purge-rebind").await;
    mob.retire_member("retired").await;
    mob.restart_as(MobCompositionAuthority::Authoritative).await;

    let receipt = committed(mob.import("import-rebind", export).await);
    for name in ["seated", "retired"] {
        assert_eq!(
            outcome_of(&receipt, name),
            IdentityChangeEntryOutcome::Committed
        );
        assert!(mob.reads_store_owned(name), "{name}: {:?}", mob.read(name));
    }
    assert_eq!(mob.session("seated").await, seated, "bound in place");
    assert_eq!(
        mob.session("retired").await,
        retired,
        "seated again on its archived session"
    );
    assert!(mob.seated_dormant("retired"));
}

/// Row (RED): only when an identity's archived session is gone from the
/// store does the import's seat start a fresh session, with the notice
/// that its history is unavailable; one whose session the store keeps is
/// seated on it with no notice.
///
/// RED without the import: the handle has no import step.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_seats_fresh_with_history_unavailable_only_when_the_session_is_gone() {
    let mut mob = StoreOwnedMob::new("cycle-history").await;
    mob.service.set_keep_alive_turns_complete_immediately(true);
    let lost = mob.create_live("lost").await;
    let kept = mob.create_live("kept").await;
    let export = mob.export_and_purge("purge-history").await;
    mob.retire_member("lost").await;
    mob.retire_member("kept").await;
    // The session store lost one archived session (a restore of a state
    // snapshot older than it).
    mob.service.delete_persisted_session(&lost).await;
    mob.restart_as(MobCompositionAuthority::Authoritative).await;

    let receipt = committed(mob.import("import-history", export).await);
    for name in ["lost", "kept"] {
        assert_eq!(
            outcome_of(&receipt, name),
            IdentityChangeEntryOutcome::Committed,
            "{name} is restored"
        );
    }
    assert_ne!(mob.session("lost").await, lost, "a fresh session");
    assert_eq!(
        mob.spawn_row("lost").await.history_unavailable().cloned(),
        Some(HistoryUnavailable::new(lost))
    );
    assert_eq!(mob.session("kept").await, kept);
    assert_eq!(mob.spawn_row("kept").await.history_unavailable(), None);
}

/// Row (RED): the import restores archived states and tombstones: the
/// archived identity reads archived again, with its archived session, and
/// the removed name is tombstoned again, refused to a create.
///
/// RED without the import: the handle has no import step.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_restores_tombstones_and_archived_states() {
    let mut mob = StoreOwnedMob::new("cycle-restore-states").await;
    mob.service.set_keep_alive_turns_complete_immediately(true);
    let parked = mob.create_live("parked").await;
    committed(
        mob.create(host_create("create-gone", &["gone"]), ALLOW)
            .await,
    );
    committed(mob.archive("archive-parked", "parked").await);
    mob.unseated_for("parked", UnseatedEntryCause::Archived)
        .await;
    committed(mob.remove("remove-gone", "gone").await);
    mob.removed("gone").await;
    let export = mob.export_and_purge("purge-states").await;
    mob.restart_as(MobCompositionAuthority::Authoritative).await;

    let receipt = committed(mob.import("import-states", export).await);
    assert_eq!(
        outcome_of(&receipt, "parked"),
        IdentityChangeEntryOutcome::Archived
    );
    assert_eq!(
        outcome_of(&receipt, "gone"),
        IdentityChangeEntryOutcome::Removed
    );
    mob.unseated_for("parked", UnseatedEntryCause::Archived)
        .await;
    assert!(matches!(mob.read("gone"), IdentityEntryRead::Removed));
    assert!(matches!(
        refusal(
            mob.create(host_create("create-gone-again", &["gone"]), ALLOW)
                .await
        ),
        IdentityChangeRefusal::NameTaken {
            by: crate::runtime::IdentityNameHolder::Removed,
            ..
        }
    ));
    committed(mob.unarchive("unarchive-parked", "parked").await);
    assert_eq!(mob.session("parked").await, parked, "its archived session");
}

/// Row: on a host store, the export holds each identity's whole record (its
/// provenance with its consent evidence) from the mob's storage alone: no
/// decision store is read, and the export validates.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_export_on_a_host_store_realm_is_complete_without_the_decision_file() {
    let host = Arc::new(meerkat_store_conformance::InMemoryHostStore::new());
    let realm = meerkat_runtime::store::spi::SpiRealm::open(
        Arc::clone(&host) as Arc<dyn meerkat_core::host_store::HostStore>,
        meerkat_runtime::store::spi::SpiRealmConfig::default(),
    )
    .await
    .expect("open the host store realm");
    let mobs = crate::store::SpiMobStoreHost::for_realm(&realm)
        .await
        .expect("claim the mob families");
    let storage = mobs
        .open("root")
        .await
        .expect("open the mob's storage")
        .storage();
    let mob = StoreOwnedMob::build(
        store_owned_definition("cycle-host-store"),
        storage,
        |builder| builder,
    )
    .await;
    committed(
        mob.create(host_create("create-alpha", &["alpha"]), ALLOW)
            .await,
    );
    let export = mob.export_set().await;
    export.validate().expect("the export is sealed");
    let record = &export.identities[0].record;
    assert_eq!(record, &mob.row("alpha").await, "the whole record");
    assert_eq!(
        record.provenance.consent,
        crate::identity::IdentityChangeConsent::Policy {
            outcome: crate::identity::IdentityPolicyOutcome::Allow
        }
    );
}

/// Row: imported consent comes only with the host operator's import: an
/// import on another requester's path, or under another consent, is
/// refused, and writes nothing.
#[tokio::test]
async fn imported_consent_is_refused_on_an_agent_path() {
    let mob = StoreOwnedMob::new("cycle-import-consent").await;
    committed(
        mob.create(host_create("create-alpha", &["alpha"]), ALLOW)
            .await,
    );
    let export = mob.export_set().await;
    let consent = imported_consent();
    let journal = mob.journal().await.len();
    for (requester, consent) in [
        (
            IdentityChangeRequesterInput::host("console"),
            IdentityChangeConsentInput::Imported(&consent),
        ),
        (
            IdentityChangeRequesterInput::host_operator("console"),
            ALLOW,
        ),
    ] {
        assert_eq!(
            refusal(
                mob.handle
                    .import_created_identities(
                        IdentityImport::new(operation("import-refused"), requester, export.clone()),
                        consent,
                    )
                    .await
            ),
            IdentityChangeRefusal::ImportRefused
        );
    }
    assert_eq!(mob.journal().await.len(), journal, "nothing is written");
    assert_eq!(
        mob.handle
            .identity_operation("import-refused")
            .await
            .expect("the receipt read answers"),
        None
    );
}

/// Row (RED): an adopted identity whose roster declared its name again (the
/// mob's store restored to a state from before the adoption, its roster
/// listing the name) returns to store custody by the import, through the
/// adoption's step: its member keeps its session, and the roster's further
/// listings get the quiet note.
///
/// RED without the import: the handle has no import step.
#[tokio::test]
async fn import_returns_a_redeclared_adopted_identity_to_store_custody() {
    let definition = store_owned_definition("cycle-redeclared");
    let adopted = StoreOwnedMob::build(definition.clone(), MobStorage::in_memory(), |builder| {
        builder
    })
    .await;
    adopted.declare(&["listed"]).await;
    committed(
        tokio::time::timeout(
            STEP,
            adopted.handle.adopt_identity(
                IdentityAdoption::new(
                    operation("adopt-listed"),
                    IdentityChangeRequesterInput::host("console"),
                    "listed",
                    AdoptionSource::roster(spec("listed", "worker")),
                ),
                ALLOW,
            ),
        )
        .await
        .expect("the adoption answers"),
    );
    let export = adopted.export_set().await;

    // The same mob over a store restored to a state from before the
    // adoption, whose roster lists the name.
    crash_stop_and_release_routes(adopted.handle.clone()).await;
    let restored =
        StoreOwnedMob::build(definition, MobStorage::in_memory(), |builder| builder).await;
    restored.declare(&["listed"]).await;
    let session = restored.session("listed").await;
    let receipt = committed(restored.import("import-redeclared", export).await);
    assert_eq!(
        outcome_of(&receipt, "listed"),
        IdentityChangeEntryOutcome::Committed
    );
    assert!(restored.reads_store_owned("listed"));
    assert_eq!(restored.session("listed").await, session);
    let outcome = restored.declare(&["listed"]).await;
    assert!(
        matches!(
            outcome.entries.get(&id("listed")),
            Some(crate::runtime::DeclaredEntryOutcome::Noted {
                note: crate::runtime::DeclaredEntryNote::OwnedByStore
            })
        ),
        "{outcome:?}"
    );
}

/// Row: a removed name a rolled-back roster seated meanwhile is reported by
/// the import (`name_taken_meanwhile`) and left as the roster has it: its
/// tombstone is not restored, and the roster's identity keeps its session.
#[tokio::test]
async fn import_reports_a_tombstoned_name_a_roster_seated_meanwhile() {
    let definition = store_owned_definition("cycle-taken");
    let removed = StoreOwnedMob::build(definition.clone(), MobStorage::in_memory(), |builder| {
        builder
    })
    .await;
    committed(
        removed
            .create(host_create("create-gone", &["gone", "kept"]), ALLOW)
            .await,
    );
    committed(removed.remove("remove-gone", "gone").await);
    removed.removed("gone").await;
    let export = removed.export_set().await;

    // The same mob over a store a rolled-back host left, whose roster
    // seated the removed name.
    crash_stop_and_release_routes(removed.handle.clone()).await;
    let rolled_back =
        StoreOwnedMob::build(definition, MobStorage::in_memory(), |builder| builder).await;
    rolled_back.declare(&["gone"]).await;
    let session = rolled_back.session("gone").await;
    let receipt = committed(rolled_back.import("import-taken", export).await);
    assert_eq!(
        outcome_of(&receipt, "gone"),
        IdentityChangeEntryOutcome::NameTakenMeanwhile
    );
    assert_eq!(
        outcome_of(&receipt, "kept"),
        IdentityChangeEntryOutcome::Committed
    );
    assert!(
        matches!(
            rolled_back.read("gone"),
            IdentityEntryRead::Entry(view) if view.origin == IdentityEntryOrigin::Declared
        ),
        "{:?}",
        rolled_back.read("gone")
    );
    assert_eq!(rolled_back.session("gone").await, session);
    assert!(!rolled_back.rows().await.contains_key(&id("gone")));
}

/// MobMachine's custody map of the owned edges `mob` published, by pair.
fn edge_custody(mob: &StoreOwnedMob) -> BTreeMap<(AgentIdentity, AgentIdentity), AgentIdentity> {
    mob.handle
        .machine_state_watch_rx
        .borrow()
        .identity_edge_owners
        .iter()
        .map(|(edge, owner)| {
            (
                (
                    AgentIdentity::from(edge.a.0.as_str()),
                    AgentIdentity::from(edge.b.0.as_str()),
                ),
                AgentIdentity::from(owner.0.as_str()),
            )
        })
        .collect()
}

/// Row (RED): the import restores its rows' owned edges as a restart
/// recovers them from the rows: an edge to a roster-owned identity the
/// roster no longer lists is held, out of force, so the custody map the
/// import leaves is the one the next restart reads, and the identity's
/// re-listing realizes the edge.
///
/// RED with the restoration needing the edge's other end to be an identity,
/// as a declaration does: the import's map lacks the edge the restart's
/// holds.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_imports_owned_edges_are_the_ones_a_restart_recovers() {
    let mut mob = StoreOwnedMob::new("cycle-import-edges").await;
    mob.declare(&["listed"]).await;
    committed(
        mob.create(
            StoreOwnedIdentityCreate::new(
                operation("create-alpha"),
                IdentityChangeRequesterInput::host("console"),
                vec![
                    CreatedIdentityRequest::new(spec("alpha", "worker")).with_peers(vec![
                        crate::identity::OwnedIdentityEdge::new(
                            "listed",
                            crate::identity::OwnedEdgeAdmission::rule("team-peers")
                                .expect("a peer rule id"),
                        ),
                    ]),
                ],
            ),
            ALLOW,
        )
        .await,
    );
    let export = mob.export_and_purge("purge-import-edges").await;
    mob.restart_as(MobCompositionAuthority::Authoritative).await;
    tokio::time::timeout(STEP, mob.handle.unlist_declared_identity(&id("listed")))
        .await
        .expect("the unlist answers")
        .expect("the unlist runs");
    let deadline = tokio::time::Instant::now() + STEP;
    while mob.is_member("listed").await {
        assert!(tokio::time::Instant::now() < deadline, "listed never left");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    committed(mob.import("import-edges", export).await);
    let imported = edge_custody(&mob);
    mob.restart_as(MobCompositionAuthority::Authoritative).await;
    let restarted = edge_custody(&mob);
    assert_eq!(
        imported, restarted,
        "the import's custody of the owned edges is the one a restart recovers"
    );
    assert_eq!(
        restarted.get(&(id("alpha"), id("listed"))),
        Some(&id("alpha")),
        "alpha's restored row owns the edge to the unlisted identity"
    );

    mob.declare(&["listed"]).await;
    let deadline = tokio::time::Instant::now() + STEP;
    while !mob.wired_to("alpha").await.contains(&id("listed")) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the re-listed identity's owned edge was never realized"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Row (RED): the import binds a plain member in place only on the session
/// its record names: a worker an older build seated under an exported
/// identity's name, on another session, is reported
/// (`name_taken_meanwhile`) and left as it is, with nothing written for the
/// name.
///
/// RED with the verdict reading no session: the worker is bound in place
/// as the store-owned identity.
#[tokio::test]
async fn import_reports_a_worker_on_another_session_and_binds_nothing() {
    let definition = store_owned_definition("cycle-other-session");
    let exported = StoreOwnedMob::build(definition.clone(), MobStorage::in_memory(), |builder| {
        builder
    })
    .await;
    committed(
        exported
            .create(host_create("create-alpha", &["alpha"]), ALLOW)
            .await,
    );
    let export = exported.export_set().await;

    // The same mob over a store an older build left, which seated a worker
    // of the name on a session of its own.
    crash_stop_and_release_routes(exported.handle.clone()).await;
    let older = StoreOwnedMob::build(definition, MobStorage::in_memory(), |builder| builder).await;
    tokio::time::timeout(
        STEP,
        older
            .handle
            .spawn_spec(SpawnMemberSpec::new("worker", id("alpha"))),
    )
    .await
    .expect("the spawn answers")
    .expect("spawn the worker");
    let session = older.session("alpha").await;
    let receipt = committed(older.import("import-other-session", export).await);
    assert_eq!(
        outcome_of(&receipt, "alpha"),
        IdentityChangeEntryOutcome::NameTakenMeanwhile
    );
    assert!(
        matches!(
            older.read("alpha"),
            IdentityEntryRead::NotAnEntry { worker: Some(_) }
        ),
        "{:?}",
        older.read("alpha")
    );
    assert_eq!(older.session("alpha").await, session);
    assert!(!older.rows().await.contains_key(&id("alpha")));
}

/// Row (RED): a purge of a mob whose scan reads no row of schema version 2
/// is refused `purge_refused` (`nothing_to_purge`), decided by MobMachine,
/// and writes nothing: no receipt, and identity changes go on.
///
/// RED with the verdict reading no row count: the empty purge goes on.
#[tokio::test]
async fn a_purge_with_no_row_to_purge_is_refused_nothing_to_purge() {
    let mob = StoreOwnedMob::new("cycle-purge-nothing").await;
    mob.hold_changes(true).await;
    let export = mob.export_set().await;
    assert!(export.identities.is_empty());
    assert_eq!(
        refusal(mob.purge("purge-nothing", &export.export_digest).await),
        IdentityChangeRefusal::PurgeRefused {
            reason: PurgeRefusedReason::NothingToPurge
        }
    );
    assert!(
        mob.handle
            .identity_operation("purge-nothing")
            .await
            .expect("the receipt read answers")
            .is_none(),
        "no receipt is written"
    );
    mob.hold_changes(false).await;
    committed(
        mob.create(host_create("create-alpha", &["alpha"]), ALLOW)
            .await,
    );
}

/// Row: a purge and an import each commit at most one change's identities
/// (`IDENTITY_CHANGE_BATCH_MAX`, tombstones included): a mob with more
/// rows is refused `batch_too_large` at its purge, with nothing removed,
/// and an export of more records at its import, with nothing written.
#[tokio::test]
async fn a_purge_and_an_import_above_the_batch_bound_are_refused_batch_too_large() {
    let mob = StoreOwnedMob::new("cycle-batch-bound").await;
    let names = (0..=IDENTITY_CHANGE_BATCH_MAX)
        .map(|index| format!("member-{index:03}"))
        .collect::<Vec<_>>();
    let (first, second) = names.split_at(IDENTITY_CHANGE_BATCH_MAX);
    for (operation_id, batch) in [("create-first", first), ("create-second", second)] {
        committed(
            mob.create(
                StoreOwnedIdentityCreate::new(
                    operation(operation_id),
                    IdentityChangeRequesterInput::host("console"),
                    batch
                        .iter()
                        .map(|name| CreatedIdentityRequest::new(spec(name, "worker")))
                        .collect(),
                ),
                ALLOW,
            )
            .await,
        );
    }
    mob.hold_changes(true).await;
    let export = mob.export_set().await;
    assert_eq!(export.identities.len(), IDENTITY_CHANGE_BATCH_MAX + 1);
    let bound = IdentityChangeRefusal::BatchTooLarge {
        max: IDENTITY_CHANGE_BATCH_MAX,
    };
    assert_eq!(
        refusal(mob.purge("purge-too-many", &export.export_digest).await),
        bound
    );
    assert_eq!(
        mob.rows().await.len(),
        IDENTITY_CHANGE_BATCH_MAX + 1,
        "nothing is removed"
    );
    assert_eq!(refusal(mob.import("import-too-many", export).await), bound);
    assert!(
        mob.handle
            .identity_operation("import-too-many")
            .await
            .expect("the receipt read answers")
            .is_none(),
        "nothing is written"
    );
}

/// Rollback row RB-CL4-3, in-process leg: a store-owned member that the
/// older build retired (here, retired after the purge, which no re-seat
/// follows) is seated again on its archived session by the import after
/// the roll-forward, its history kept.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rb_cl4_3_a_store_owned_member_retired_meanwhile_is_rebound_to_its_archived_session_by_the_import()
 {
    let mut mob = StoreOwnedMob::new("rb-cl4-3").await;
    mob.service.set_keep_alive_turns_complete_immediately(true);
    let session = mob.create_live("member").await;
    let export = mob.export_and_purge("purge-rb-cl4-3").await;
    mob.retire_member("member").await;
    mob.restart_as(MobCompositionAuthority::Authoritative).await;
    assert!(!mob.is_member("member").await);
    let receipt = committed(mob.import("import-rb-cl4-3", export).await);
    assert_eq!(
        outcome_of(&receipt, "member"),
        IdentityChangeEntryOutcome::Committed
    );
    assert!(mob.reads_store_owned("member"));
    assert_eq!(mob.session("member").await, session);
}

/// The fixture of a cycle's rollback rows: a persistent mob store (SQLite)
/// at `$MEERKAT_ROLLBACK_FIXTURE_DIR/<name>/mob.sqlite` that `setup` built,
/// then exported (the export written beside it, `export.json`) and purged,
/// as a host leaves a mob before its downgrade.
async fn write_purged_fixture<F, Fut>(name: &str, setup: F)
where
    F: FnOnce(MobHandle) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let dir = super::super::created_identity_rows::rollback_fixture_dir(name);
    let export_path = dir.join("export.json");
    super::super::identity_entry_rows::write_rollback_fixture(
        &dir,
        &format!("rollback-{name}"),
        |handle| async move {
            setup(handle.clone()).await;
            handle
                .set_identity_changes_held(true)
                .await
                .expect("hold the changes");
            let export = handle
                .export_created_identities()
                .await
                .expect("export the set");
            std::fs::write(
                &export_path,
                serde_json::to_vec_pretty(&export).expect("encode the export"),
            )
            .expect("write the export");
            committed(
                handle
                    .purge_created_identities(IdentityRowsPurge::new(
                        operation("purge-fixture"),
                        IdentityChangeRequesterInput::host_operator("console"),
                        export.export_digest.clone(),
                    ))
                    .await,
            );
        },
    )
    .await;
}

/// Rollback rows RB-CL1-1 and RB-CL1-3, lane-run leg:
/// `a_purged_mob_opens_and_adopts_under_<pair>` for each published pair.
///
/// This build writes the fixture: a store-owned identity `owner` and a
/// worker `peer`, exported and purged. The lane serves it with the pair and
/// checks: the mob opens (a host store) and its identity-first adoption of
/// `peer` is admitted (no row of schema version 2 remains), `owner` runs as
/// a plain member; rolled forward to this build, `owner` reads unreadable
/// until the import of `export.json` restores it in place.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "lane:rollback - writes the fixture the published pairs run"]
async fn rb_cl1_1_a_purged_mob_opens_and_adopts_fixture() {
    write_purged_fixture("purged-created-owner", |handle| async move {
        committed(
            handle
                .create_store_owned_identities(host_create("create-owner", &["owner"]), ALLOW)
                .await
                .map(|created| created.change),
        );
        handle
            .spawn_spec(SpawnMemberSpec::new("worker", id("peer")))
            .await
            .expect("spawn the peer");
    })
    .await;
}

/// Rollback row RB-CL4-2, lane-run leg:
/// `an_adopted_identity_the_roster_lists_reverts_to_roster_custody_under_<pair>_keeping_its_session`
/// for each published pair.
///
/// This build writes the fixture: `listed`, a roster-owned identity adopted
/// into store custody, exported and purged. The lane serves it with the
/// pair, whose roster lists `listed`, and checks: it is a roster identity
/// on the same session; rolled forward, the import of `export.json`
/// returns it to store custody on that session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "lane:rollback - writes the fixture the published pairs run"]
async fn rb_cl4_2_an_adopted_identity_the_roster_lists_fixture() {
    write_purged_fixture("purged-adopted-identity", |handle| async move {
        handle
            .apply_declared_roster(DeclaredRoster::partial([DeclaredEntry::new(roster_spec(
                "listed",
            ))]))
            .await
            .expect("declare the roster");
        committed(
            handle
                .adopt_identity(
                    IdentityAdoption::new(
                        operation("adopt-listed"),
                        IdentityChangeRequesterInput::host("console"),
                        "listed",
                        AdoptionSource::roster(spec("listed", "worker")),
                    ),
                    ALLOW,
                )
                .await,
        );
    })
    .await;
}

/// Rollback rows RB-CL4-3 and RB-CL4-4, lane-run leg:
/// `a_store_owned_member_retired_under_<pair>_is_rebound_to_its_archived_session_by_the_import`
/// and `tombstones_and_archived_states_return_with_the_import` for each
/// published pair.
///
/// This build writes the fixture: `member`, a store-owned identity; `parked`,
/// an archived one; `gone`, a removed name; exported and purged. The lane
/// serves it with the pair (whose worker reconcile with `retire_stale`
/// retires `member` and archives its session) and checks that the names
/// are free there; rolled forward, the import of `export.json` seats
/// `member` Dormant on its archived session, reads `parked` archived and
/// `gone` removed again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "lane:rollback - writes the fixture the published pairs run"]
async fn rb_cl4_3_and_4_store_owned_states_fixture() {
    write_purged_fixture("purged-store-owned-states", |handle| async move {
        committed(
            handle
                .create_store_owned_identities(
                    host_create("create-states", &["member", "parked", "gone"]),
                    ALLOW,
                )
                .await
                .map(|created| created.change),
        );
        committed(
            handle
                .archive_identity(
                    crate::runtime::IdentityArchiveChange::new(
                        operation("archive-parked"),
                        IdentityChangeRequesterInput::host("console"),
                        "parked",
                    ),
                    ALLOW,
                )
                .await,
        );
        committed(
            handle
                .remove_identity(
                    crate::runtime::IdentityRemoval::new(
                        operation("remove-gone"),
                        IdentityChangeRequesterInput::host("console"),
                        "gone",
                    ),
                    ALLOW,
                )
                .await,
        );
    })
    .await;
}
