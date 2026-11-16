//! Adoption into store custody (0.9.11): an existing member's move into
//! store custody as one change, an adopted identity's edges and its former
//! spawner, an unseated roster entry, and the shapes no adoption takes.

use super::*;
use crate::identity::{
    IdentityAdoptedFrom, IdentityChangeKind, IdentityEntered, OwnedEdgeAdmission, OwnedIdentityEdge,
};
use crate::runtime::{AdoptionSource, IdentityAdoption, NotAdoptableReason, OwnedEdgeEndOutcome};

impl StoreOwnedMob {
    /// A host surface's adoption of `name` from `source`, as `operation_id`.
    async fn adopt(
        &self,
        operation_id: &str,
        name: &str,
        source: AdoptionSource,
    ) -> Result<IdentityChangeOutcome, MobError> {
        tokio::time::timeout(
            STEP,
            self.handle.adopt_identity(
                IdentityAdoption::new(
                    operation(operation_id),
                    IdentityChangeRequesterInput::host("console"),
                    name,
                    source,
                ),
                ALLOW,
            ),
        )
        .await
        .expect("the adoption answers")
    }

    /// Spawn the worker `name` on the worker profile, spawned by `spawner`
    /// when one is named, with `labels`.
    async fn spawn_worker(
        &self,
        name: &str,
        spawner: Option<&str>,
        labels: &[(&str, &str)],
    ) -> SessionId {
        let mut spec = SpawnMemberSpec::new("worker", id(name)).with_labels(
            labels
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect(),
        );
        spec.spawned_by = spawner.map(id);
        tokio::time::timeout(STEP, self.handle.spawn_spec(spec))
            .await
            .expect("the spawn answers")
            .expect("spawn the worker");
        self.session(name).await
    }

    /// `name`'s member, as the roster holds it.
    async fn member(&self, name: &str) -> crate::roster::RosterEntry {
        self.handle
            .get_member(&id(name))
            .await
            .expect("roster read")
            .unwrap_or_else(|| panic!("{name} is a member"))
    }

    /// Whether `name` is a store-owned identity.
    fn store_owned(&self, name: &str) -> bool {
        matches!(
            self.read(name),
            IdentityEntryRead::Entry(view) if view.origin == IdentityEntryOrigin::Created
        )
    }

    /// The identity origin `name`'s last binding row records, if any.
    async fn binding_row_origin(&self, name: &str) -> Option<IdentityEntryOrigin> {
        self.journal()
            .await
            .into_iter()
            .rev()
            .find_map(|event| match event.kind {
                MobEventKind::MemberSessionBindingRecovered(recovered)
                    if recovered.agent_identity == id(name) =>
                {
                    Some(recovered.identity_entry)
                }
                _ => None,
            })
            .flatten()
    }
}

/// The receipt entry of an adoption's one identity.
fn adopted_entry(
    receipt: &crate::IdentityChangeReceipt,
) -> &crate::identity::IdentityChangeReceiptEntry {
    assert_eq!(receipt.change, IdentityChangeKind::Adopt);
    assert_eq!(receipt.identities.len(), 1, "{receipt:?}");
    &receipt.identities[0]
}

/// Row: no start, reconcile or declared apply adopts a worker into store
/// custody: only the adoption change does.
#[tokio::test]
async fn no_start_reconcile_or_apply_adopts_a_worker() {
    let mut mob = StoreOwnedMob::new("adopt-no-implicit").await;
    mob.spawn_worker("lead", None, &[]).await;
    mob.spawn_worker("helper", Some("lead"), &[]).await;
    mob.declare(&["listed"]).await;
    mob.restart_as(MobCompositionAuthority::Authoritative).await;
    mob.declare(&["listed"]).await;
    assert!(
        matches!(
            mob.read("helper"),
            IdentityEntryRead::NotAnEntry { worker: Some(_) }
        ),
        "{:?}",
        mob.read("helper")
    );
    assert!(
        !mob.rows().await.contains_key(&id("helper")),
        "no row holds the worker"
    );
    let generation = mob.member("helper").await.generation;
    committed(
        mob.adopt("adopt-helper", "helper", AdoptionSource::worker(generation))
            .await,
    );
    assert!(mob.store_owned("helper"));
}

/// Row (RED): a roster-owned identity's adoption moves it into store
/// custody in one change: its row (revision 1, presence present, the
/// roster's spec, provenance adopted from the roster), its binding row with
/// the Created origin and the receipt (kind adopt), and its member keeps its
/// runtime, generation and session. A retry replays; another adoption of it
/// is refused `not_adoptable` (store_owned).
///
/// RED without the adoption: the change has no step.
#[tokio::test]
async fn adopt_moves_a_roster_owned_identity_to_store_custody_keeping_runtime_generation_and_session()
 {
    let mob = StoreOwnedMob::new("adopt-roster").await;
    mob.declare(&["listed"]).await;
    let before = mob.member("listed").await;
    let session = mob.session("listed").await;
    let builds = mob.builds().await;

    let receipt = committed(
        mob.adopt(
            "adopt-listed",
            "listed",
            AdoptionSource::roster(spec("listed", "worker")),
        )
        .await,
    );
    assert_eq!(
        adopted_entry(&receipt).outcome,
        crate::identity::IdentityChangeEntryOutcome::Committed
    );
    assert!(mob.store_owned("listed"));
    let row = mob.row("listed").await;
    assert_eq!(row.revision, 1);
    assert_eq!(row.presence, CreatedIdentityPresence::Present);
    assert_eq!(row.spec, Some(spec("listed", "worker")));
    assert_eq!(
        row.provenance.entered,
        IdentityEntered::Adopted {
            from: IdentityAdoptedFrom::Roster,
            former_spawner: None,
        }
    );
    let after = mob.member("listed").await;
    assert_eq!(after.agent_runtime_id, before.agent_runtime_id);
    assert_eq!(after.generation, before.generation);
    assert_eq!(mob.session("listed").await, session);
    assert_eq!(mob.builds().await, builds, "nothing is built");
    assert_eq!(
        mob.binding_row_origin("listed").await,
        Some(IdentityEntryOrigin::Created)
    );

    assert!(matches!(
        mob.adopt(
            "adopt-listed",
            "listed",
            AdoptionSource::roster(spec("listed", "worker")),
        )
        .await,
        Ok(IdentityChangeOutcome::Replayed { .. })
    ));
    assert_eq!(
        refusal(
            mob.adopt(
                "adopt-listed-again",
                "listed",
                AdoptionSource::roster(spec("listed", "worker")),
            )
            .await
        ),
        IdentityChangeRefusal::NotAdoptable {
            identity: id("listed"),
            reason: NotAdoptableReason::StoreOwned,
        }
    );
}

/// Row (RED): an agent-created worker's adoption makes it a store-owned
/// identity: its row's spec is its member's spawn facts with the display
/// name the adoption gives it, its provenance names its former spawner, and
/// its member keeps its runtime, generation and session (nothing is built)
/// and is a host root.
///
/// RED without the adoption: the change has no step.
#[tokio::test]
async fn adopt_turns_an_agent_created_worker_into_a_store_owned_identity_keeping_its_session_and_history()
 {
    let mob = StoreOwnedMob::new("adopt-worker").await;
    mob.spawn_worker("lead", None, &[]).await;
    let session = mob.spawn_worker("helper", Some("lead"), &[]).await;
    let before = mob.member("helper").await;
    assert_eq!(before.spawned_by, Some(id("lead")));
    let builds = mob.builds().await;

    committed(
        mob.adopt(
            "adopt-helper",
            "helper",
            AdoptionSource::worker(before.generation).with_display_name("Helper"),
        )
        .await,
    );
    assert!(mob.store_owned("helper"));
    let row = mob.row("helper").await;
    assert_eq!(
        row.provenance.entered,
        IdentityEntered::Adopted {
            from: IdentityAdoptedFrom::Worker,
            former_spawner: Some(id("lead")),
        }
    );
    let row_spec = row.spec.expect("the row holds its spec");
    assert_eq!(row_spec.profile, ProfileName::from("worker"));
    assert_eq!(row_spec.display_name.as_deref(), Some("Helper"));
    let after = mob.member("helper").await;
    assert_eq!(after.agent_runtime_id, before.agent_runtime_id);
    assert_eq!(after.generation, before.generation);
    assert_eq!(mob.session("helper").await, session);
    assert_eq!(after.spawned_by, None, "the adopted member is a host root");
    assert_eq!(mob.builds().await, builds, "nothing is built");
}

/// Row (RED): an adopted worker outlives its former spawner's retirement,
/// which no longer cascades to it.
///
/// RED without the adoption: the retirement of its spawner retires it.
#[tokio::test]
async fn an_adopted_worker_is_not_retired_with_its_former_spawner() {
    let mob = StoreOwnedMob::new("adopt-spawner-retire").await;
    mob.spawn_worker("lead", None, &[]).await;
    mob.spawn_worker("helper", Some("lead"), &[]).await;
    let generation = mob.member("helper").await.generation;
    committed(
        mob.adopt("adopt-helper", "helper", AdoptionSource::worker(generation))
            .await,
    );
    tokio::time::timeout(STEP, mob.handle.retire(id("lead")))
        .await
        .expect("the retirement answers")
        .expect("retire the former spawner");
    assert!(!mob.is_member("lead").await);
    assert!(mob.is_member("helper").await, "the adopted identity stays");
    assert!(mob.store_owned("helper"));
}

/// Row (RED): the agent lane, its former spawner's, does not retire or
/// respawn an adopted identity: both answer `identity_member`.
///
/// RED without the adoption: the agent lane retires the worker.
#[tokio::test]
async fn the_former_spawner_cannot_retire_or_respawn_an_adopted_identity() {
    let mob = StoreOwnedMob::new("adopt-agent-lane").await;
    mob.spawn_worker("lead", None, &[]).await;
    mob.spawn_worker("helper", Some("lead"), &[]).await;
    let generation = mob.member("helper").await.generation;
    committed(
        mob.adopt("adopt-helper", "helper", AdoptionSource::worker(generation))
            .await,
    );
    let agent = mob
        .handle
        .clone()
        .with_command_authority(crate::control_policy::CommandAuthority::agent_lane());
    let error = agent
        .retire(id("helper"))
        .await
        .expect_err("the agent lane does not retire an adopted identity");
    assert!(
        matches!(&error, MobError::IdentityMember { member_id } if member_id == &id("helper")),
        "{error:?}"
    );
    let error = agent
        .respawn(id("helper"), None)
        .await
        .expect_err("the agent lane does not respawn an adopted identity");
    assert!(
        matches!(
            &error,
            crate::runtime::handle::MobRespawnError::Mob(MobError::IdentityMember { member_id })
                if member_id == &id("helper")
        ),
        "{error:?}"
    );
    assert!(mob.is_member("helper").await);
}

/// Row (RED): after a restart the adopted identity is a store-owned host
/// root on the same session: its binding row replays with the Created
/// origin, and the replay clears its spawner again.
///
/// RED without the adoption: it restarts a worker of its spawner.
#[tokio::test]
async fn a_restart_after_adoption_replays_a_store_owned_root_on_the_same_session() {
    let mut mob = StoreOwnedMob::new("adopt-restart").await;
    mob.spawn_worker("lead", None, &[]).await;
    let session = mob.spawn_worker("helper", Some("lead"), &[]).await;
    let generation = mob.member("helper").await.generation;
    committed(
        mob.adopt("adopt-helper", "helper", AdoptionSource::worker(generation))
            .await,
    );
    mob.restart_as(MobCompositionAuthority::Authoritative).await;
    assert!(mob.store_owned("helper"));
    assert_eq!(mob.session("helper").await, session);
    assert_eq!(mob.member("helper").await.spawned_by, None);
    tokio::time::timeout(STEP, mob.handle.retire(id("lead")))
        .await
        .expect("the retirement answers")
        .expect("retire the former spawner");
    assert!(mob.is_member("helper").await);
}

/// Row (RED): an adoption keeps the member's labels, and its member's
/// current edges to other identities become its owned edges (recorded
/// `adopted`), refused a plain unwire; its edges to members that are no
/// identity stay plain wiring. The receipt names both.
///
/// RED without the adoption: the change has no step.
#[tokio::test]
async fn adoption_keeps_labels_and_turns_current_edges_into_owned_edges() {
    let mob = StoreOwnedMob::new("adopt-edges").await;
    committed(
        mob.create(host_create("create-beta", &["beta"]), ALLOW)
            .await,
    );
    mob.spawn_worker("plain", None, &[]).await;
    mob.spawn_worker("helper", None, &[("team", "blue")]).await;
    for peer in ["beta", "plain"] {
        tokio::time::timeout(STEP, mob.handle.wire(id("helper"), id(peer)))
            .await
            .expect("the wire answers")
            .expect("wire the helper");
    }
    let generation = mob.member("helper").await.generation;

    let receipt = committed(
        mob.adopt("adopt-helper", "helper", AdoptionSource::worker(generation))
            .await,
    );
    let entry = adopted_entry(&receipt);
    assert_eq!(entry.owned_edges, vec![id("beta")]);
    assert_eq!(entry.plain_edges, vec![id("plain")]);
    let row = mob.row("helper").await;
    assert_eq!(
        row.owned_edges,
        vec![OwnedIdentityEdge::new("beta", OwnedEdgeAdmission::Adopted)]
    );
    assert_eq!(
        row.spec.expect("the row holds its spec").labels,
        BTreeMap::from([("team".to_string(), "blue".to_string())]),
        "the labels are kept"
    );
    assert!(matches!(
        tokio::time::timeout(STEP, mob.handle.unwire(id("beta"), id("helper")))
            .await
            .expect("the unwire answers"),
        Err(MobError::OwnedIdentityEdge { owner, .. }) if owner == id("helper")
    ));
    tokio::time::timeout(STEP, mob.handle.unwire(id("helper"), id("plain")))
        .await
        .expect("the unwire answers")
        .expect("a plain edge takes a plain unwire");
    assert!(mob.wired_to("helper").await.contains(&id("beta")));
}

/// Row: an adoption decided on a generation that is not the worker's
/// current one is refused `predecessor_mismatch`, with nothing written; at
/// the current one it commits.
#[tokio::test]
async fn adopt_refuses_a_stale_observation_predecessor_mismatch() {
    let mob = StoreOwnedMob::new("adopt-stale").await;
    mob.spawn_worker("helper", None, &[]).await;
    let generation = mob.member("helper").await.generation;
    let stale = crate::ids::Generation::new(generation.get() + 1);
    assert_eq!(
        refusal(
            mob.adopt("adopt-stale", "helper", AdoptionSource::worker(stale))
                .await
        ),
        IdentityChangeRefusal::PredecessorMismatch {
            identity: id("helper")
        }
    );
    assert!(mob.rows().await.is_empty(), "nothing is written");
    committed(
        mob.adopt(
            "adopt-current",
            "helper",
            AdoptionSource::worker(generation),
        )
        .await,
    );
}

/// Row: after a roster-owned identity's adoption, a roster that still lists
/// it gets the quiet note for that identity alone, and nothing is retired.
#[tokio::test]
async fn a_roster_still_listing_an_adopted_identity_retires_nothing_and_raises_nothing() {
    let mob = StoreOwnedMob::new("adopt-roster-listing").await;
    mob.declare(&["listed", "other"]).await;
    committed(
        mob.adopt(
            "adopt-listed",
            "listed",
            AdoptionSource::roster(spec("listed", "worker")),
        )
        .await,
    );
    let outcome = mob.declare(&["listed", "other"]).await;
    assert!(
        matches!(
            outcome.entries.get(&id("listed")),
            Some(crate::runtime::DeclaredEntryOutcome::Noted {
                note: crate::runtime::DeclaredEntryNote::OwnedByStore
            })
        ),
        "{outcome:?}"
    );
    assert!(mob.is_member("listed").await);
    assert!(mob.is_member("other").await);
    assert!(mob.store_owned("listed"));
}

/// Row: the host's policy reload that denies an adopted edge ends it, as
/// any owned edge.
#[tokio::test]
async fn a_reload_that_denies_an_adopted_edge_ends_it() {
    let mob = StoreOwnedMob::new("adopt-reload").await;
    committed(
        mob.create(host_create("create-beta", &["beta"]), ALLOW)
            .await,
    );
    mob.spawn_worker("helper", None, &[]).await;
    tokio::time::timeout(STEP, mob.handle.wire(id("helper"), id("beta")))
        .await
        .expect("the wire answers")
        .expect("wire the helper");
    let generation = mob.member("helper").await.generation;
    committed(
        mob.adopt("adopt-helper", "helper", AdoptionSource::worker(generation))
            .await,
    );
    let ended = tokio::time::timeout(
        STEP,
        mob.handle
            .end_denied_identity_edge(id("helper"), id("beta")),
    )
    .await
    .expect("the end answers")
    .expect("the end runs");
    assert_eq!(ended, OwnedEdgeEndOutcome::Ended);
    assert!(mob.row("helper").await.owned_edges.is_empty());
    assert!(!mob.wired_to("helper").await.contains(&id("beta")));
}

/// Row: an adopted member's edge to a worker stays plain wiring, and a
/// later, unrelated worker of the same name gets no owned edge.
#[tokio::test]
async fn an_edge_to_a_worker_stays_plain_and_a_later_worker_of_its_name_gets_no_owned_edge() {
    let mob = StoreOwnedMob::new("adopt-plain-worker").await;
    mob.spawn_worker("plain", None, &[]).await;
    mob.spawn_worker("helper", None, &[]).await;
    tokio::time::timeout(STEP, mob.handle.wire(id("helper"), id("plain")))
        .await
        .expect("the wire answers")
        .expect("wire the helper");
    let generation = mob.member("helper").await.generation;
    let receipt = committed(
        mob.adopt("adopt-helper", "helper", AdoptionSource::worker(generation))
            .await,
    );
    assert_eq!(adopted_entry(&receipt).plain_edges, vec![id("plain")]);
    assert!(adopted_entry(&receipt).owned_edges.is_empty());
    tokio::time::timeout(STEP, mob.handle.retire(id("plain")))
        .await
        .expect("the retirement answers")
        .expect("retire the worker");
    mob.spawn_worker("plain", None, &[]).await;
    assert!(mob.row("helper").await.owned_edges.is_empty());
    assert!(!mob.wired_to("helper").await.contains(&id("plain")));
}

/// Row: a roster-owned identity owed its seat (its member retired outside
/// the declared apply) is adopted, and seated as a store-owned identity.
#[tokio::test]
async fn an_unseated_roster_entry_owed_its_seat_is_adopted_and_seated() {
    let mob = StoreOwnedMob::new("adopt-owed-seat").await;
    mob.declare(&["listed"]).await;
    tokio::time::timeout(STEP, mob.handle.retire(id("listed")))
        .await
        .expect("the retirement answers")
        .expect("retire the member");
    mob.unseated_for("listed", UnseatedEntryCause::SeatOwed)
        .await;
    committed(
        mob.adopt(
            "adopt-listed",
            "listed",
            AdoptionSource::roster(spec("listed", "worker")),
        )
        .await,
    );
    assert!(mob.store_owned("listed"));
    assert!(mob.is_member("listed").await, "the adoption seats it");
}

/// Row (RED): a worker spawned with a model override is refused
/// `not_adoptable` (`profile_override`), decided by MobMachine from the
/// member's spawn facts, since a store-owned identity's spec carries no
/// override: nothing is written, and the worker stays a worker on its
/// session.
///
/// RED with the verdict reading no override: the worker is adopted.
#[tokio::test]
async fn a_worker_spawned_with_an_override_is_refused_not_adoptable() {
    let mob = StoreOwnedMob::new("adopt-override").await;
    let mut spec = SpawnMemberSpec::new("worker", id("overridden"));
    spec.model_override = Some("override-model".to_string());
    tokio::time::timeout(STEP, mob.handle.spawn_spec(spec))
        .await
        .expect("the spawn answers")
        .expect("spawn the worker");
    let session = mob.session("overridden").await;
    let generation = mob.member("overridden").await.generation;
    assert_eq!(
        refusal(
            mob.adopt(
                "adopt-overridden",
                "overridden",
                AdoptionSource::worker(generation)
            )
            .await
        ),
        IdentityChangeRefusal::NotAdoptable {
            identity: id("overridden"),
            reason: NotAdoptableReason::ProfileOverride,
        }
    );
    assert!(!mob.store_owned("overridden"));
    assert!(mob.rows().await.is_empty(), "nothing is written");
    assert_eq!(mob.session("overridden").await, session);
}

/// Row (RED): `not_adoptable` for a roster-owned identity whose host still
/// settles it (hosted on its first use, awaiting its held creation or its
/// host's preparation) is retryable, in its typed refusal and on the wire:
/// the host settles the identity and retries the same adoption. Every other
/// reason is final.
///
/// RED with every `not_adoptable` final.
#[test]
fn not_adoptable_while_its_host_settles_the_identity_is_retryable() {
    for (reason, retryable) in [
        (NotAdoptableReason::HostedOnFirstUse, true),
        (NotAdoptableReason::AwaitingHeldCreation, true),
        (NotAdoptableReason::AwaitingHostPreparation, true),
        (NotAdoptableReason::StoreOwned, false),
        (NotAdoptableReason::NotAnIdentity, false),
        (NotAdoptableReason::RosterOwned, false),
        (NotAdoptableReason::NotAMember, false),
        (NotAdoptableReason::Placed, false),
        (NotAdoptableReason::NoSession, false),
        (NotAdoptableReason::ProfileOverride, false),
    ] {
        let refusal = IdentityChangeRefusal::NotAdoptable {
            identity: id("listed"),
            reason,
        };
        assert_eq!(refusal.is_retryable(), retryable, "{reason:?}");
        let data = MobError::IdentityChangeRefused { refusal }
            .structured_data()
            .expect("a refused change carries its data");
        assert_eq!(data["retryable"], retryable, "{reason:?}: {data}");
    }
}

/// A store-owned identity whose seat was recorded ephemeral becomes a
/// durable identity at its member's same-session successor, with its
/// conversation kept: an adopted worker here (the adoption keeps the
/// worker's seat as it was recorded), and likewise an identity an earlier
/// release seated on a profile that is not durable. The successor's seat is
/// recorded durable, keyed by the identity, its member is built with the
/// host's resume gate bound on its registration, and Core lists it among
/// its durable identities. Beside it a plain worker's successor stays
/// ephemeral with no gate, the successor of a member a host spawned as a
/// durable identity keeps the key its host gave it, and the successor of a
/// roster-owned identity its host declared with no durable mark stays
/// ephemeral with no gate: its host says what it is, and only an identity
/// whose entry is the store's takes the rule.
///
/// RED before a successor took the rule: the successor copied the recorded
/// intent, so the adopted identity stayed ephemeral through every respawn,
/// with no gate and absent from the list.
#[tokio::test]
async fn a_respawn_seats_a_store_owned_identity_recorded_ephemeral_as_a_durable_identity() {
    let gate: Arc<dyn crate::DurableResumeGate> = Arc::new(ContinueResumeGate);
    let mob = StoreOwnedMob::build(
        store_owned_definition("respawn-durable"),
        MobStorage::in_memory(),
        |builder| builder.with_durable_resume_gate(Arc::clone(&gate)),
    )
    .await;
    let machine = mob.service.enable_runtime_adapter();
    let session = mob.spawn_worker("helper", None, &[]).await;
    mob.spawn_worker("plain", None, &[]).await;
    let mut marked = SpawnMemberSpec::new("worker", id("marked"));
    marked.continuity_intent = crate::runtime::SpawnContinuityIntent::DurableIdentity {
        continuity_key: "desk-7".to_string(),
    };
    tokio::time::timeout(STEP, mob.handle.spawn_spec(marked))
        .await
        .expect("the spawn answers")
        .expect("spawn the host's durable identity");
    let generation = mob.member("helper").await.generation;
    committed(
        mob.adopt("adopt-helper", "helper", AdoptionSource::worker(generation))
            .await,
    );
    assert!(mob.store_owned("helper"));
    assert_eq!(
        mob.spawn_row("helper").await.continuity_intent,
        crate::runtime::SpawnContinuityIntent::Ephemeral,
        "the adoption keeps the worker's seat as it was recorded"
    );
    // A roster-owned identity on the same profile, declared with no durable
    // mark: an identity entry too, recorded ephemeral, but not the store's.
    mob.declare(&["listed"]).await;
    assert_eq!(
        mob.spawn_row("listed").await.continuity_intent,
        crate::runtime::SpawnContinuityIntent::Ephemeral,
        "the roster-owned identity's seat is recorded as its host declared it"
    );

    for name in ["helper", "plain", "marked", "listed"] {
        tokio::time::timeout(
            STEP,
            mob.handle
                .respawn_member(id(name), crate::runtime::MemberRespawn::new()),
        )
        .await
        .expect("the respawn answers")
        .expect("the respawn commits");
        tokio::time::timeout(STEP, mob.handle.ensure_member_live(&id(name)))
            .await
            .expect("the wake answers")
            .expect("the successor wakes");
    }

    assert_eq!(mob.session("helper").await, session, "the same session");
    assert_eq!(
        mob.spawn_row("helper").await.continuity_intent,
        crate::runtime::SpawnContinuityIntent::DurableIdentity {
            continuity_key: "helper".to_string(),
        },
        "the store-owned identity's successor is recorded durable"
    );
    assert_eq!(
        mob.spawn_row("plain").await.continuity_intent,
        crate::runtime::SpawnContinuityIntent::Ephemeral,
        "a plain worker's successor stays ephemeral"
    );
    assert_eq!(
        mob.spawn_row("marked").await.continuity_intent,
        crate::runtime::SpawnContinuityIntent::DurableIdentity {
            continuity_key: "desk-7".to_string(),
        },
        "a host's durable identity keeps the key its host gave it"
    );
    assert_eq!(
        mob.spawn_row("listed").await.continuity_intent,
        crate::runtime::SpawnContinuityIntent::Ephemeral,
        "a roster-owned identity's successor keeps the continuity its host declared"
    );
    assert!(
        member_registration_holds_resume_admission(&mob.handle, &machine, &id("helper")).await,
        "the store-owned identity's successor holds the gate"
    );
    assert!(
        !member_registration_holds_resume_admission(&mob.handle, &machine, &id("plain")).await,
        "a plain worker's successor holds none"
    );
    assert!(
        member_registration_holds_resume_admission(&mob.handle, &machine, &id("marked")).await,
        "the host's durable identity holds the gate as before"
    );
    assert!(
        !member_registration_holds_resume_admission(&mob.handle, &machine, &id("listed")).await,
        "a roster-owned identity declared with no durable mark holds none"
    );
    let durable: Vec<AgentIdentity> = mob
        .handle
        .durable_identity_members()
        .await
        .expect("Core's durable identity members")
        .into_iter()
        .map(|binding| binding.identity().clone())
        .collect();
    assert!(
        durable.contains(&id("helper"))
            && durable.contains(&id("marked"))
            && !durable.contains(&id("plain"))
            && !durable.contains(&id("listed")),
        "Core lists the store-owned identity among its durable identities: {durable:?}"
    );
}

/// A store-owned identity's seat that was recorded ephemeral stays as it
/// was recorded until the identity's next seat or its member's respawn:
/// neither the adoption that made a plain worker a store-owned identity nor
/// a start rewrites its spawn row. Until then the identity is not among
/// Core's durable identities and its member is built with no resume gate,
/// after a restart as before it. A respawn then makes it a durable
/// identity (the respawn row above).
///
/// Pin of what an upgrade leaves as it is: a start that rewrote such a
/// seat, or an adoption that marked it, would fail here.
#[tokio::test]
async fn an_adopted_workers_seat_stays_as_recorded_across_a_start_until_its_next_seat() {
    let gate: Arc<dyn crate::DurableResumeGate> = Arc::new(ContinueResumeGate);
    let mut mob = StoreOwnedMob::build(
        store_owned_definition("adopted-seat-as-recorded"),
        MobStorage::in_memory(),
        |builder| builder.with_durable_resume_gate(Arc::clone(&gate)),
    )
    .await;
    let session = mob.spawn_worker("helper", None, &[]).await;
    let generation = mob.member("helper").await.generation;
    committed(
        mob.adopt("adopt-helper", "helper", AdoptionSource::worker(generation))
            .await,
    );
    let recorded = mob.spawn_row("helper").await;
    assert_eq!(
        recorded.continuity_intent,
        crate::runtime::SpawnContinuityIntent::Ephemeral,
        "the adoption keeps the worker's seat as it was recorded"
    );

    mob.restart_configured(MobCompositionAuthority::Authoritative, |builder| {
        builder.with_durable_resume_gate(Arc::clone(&gate))
    })
    .await;
    let machine = mob.service.enable_runtime_adapter();
    assert!(mob.store_owned("helper"));
    assert_eq!(mob.session("helper").await, session, "the same session");
    assert_eq!(
        mob.spawn_row("helper").await,
        recorded,
        "the start writes no new seat row for the identity"
    );
    tokio::time::timeout(STEP, mob.handle.ensure_member_live(&id("helper")))
        .await
        .expect("the wake answers")
        .expect("the member wakes after the restart");
    assert!(
        !member_registration_holds_resume_admission(&mob.handle, &machine, &id("helper")).await,
        "a seat recorded ephemeral is built with no resume gate until its next seat"
    );
    let durable: Vec<AgentIdentity> = mob
        .handle
        .durable_identity_members()
        .await
        .expect("Core's durable identity members")
        .into_iter()
        .map(|binding| binding.identity().clone())
        .collect();
    assert!(
        !durable.contains(&id("helper")),
        "the identity is not among Core's durable identities until its next seat: {durable:?}"
    );
}

/// Row (RED): an adoption writes no record that takes an approval branch's
/// reservation. A roster-owned identity whose roster spec carries the
/// branch's label, owed its seat or seated, and one whose name takes the
/// branch's identity prefix, is refused with the reservation's own
/// refusal and nothing is written: no row, the identity is no store-owned
/// one, and no receipt (the same operation then commits the adoption of a
/// spec that takes neither).
///
/// RED before the adoption refused the reservation: the adoption commits a
/// record that carries the label, or holds the name.
#[tokio::test]
async fn an_adoption_whose_spec_takes_the_approval_branch_reservation_is_refused_and_writes_nothing()
 {
    // The label, for an identity owed its seat.
    let mob = StoreOwnedMob::new("adopt-branch-label-owed").await;
    mob.declare(&["listed"]).await;
    tokio::time::timeout(STEP, mob.handle.retire(id("listed")))
        .await
        .expect("the retirement answers")
        .expect("retire the member");
    mob.unseated_for("listed", UnseatedEntryCause::SeatOwed)
        .await;
    assert_branch_reserved(
        mob.adopt(
            "adopt-listed",
            "listed",
            AdoptionSource::roster(with_branch_label(&spec("listed", "worker"))),
        )
        .await,
        "listed",
        "the label, owed its seat",
    );
    assert!(mob.rows().await.is_empty(), "owed: nothing is written");
    assert!(!mob.store_owned("listed"), "owed: {:?}", mob.read("listed"));
    assert!(!mob.is_member("listed").await, "owed: nothing is seated");
    committed(
        mob.adopt(
            "adopt-listed",
            "listed",
            AdoptionSource::roster(spec("listed", "worker")),
        )
        .await,
    );
    assert!(mob.store_owned("listed"));

    // The label, for a seated identity.
    let mob = StoreOwnedMob::new("adopt-branch-label-seated").await;
    mob.declare(&["seated"]).await;
    let member = mob.member("seated").await;
    assert_branch_reserved(
        mob.adopt(
            "adopt-seated",
            "seated",
            AdoptionSource::roster(with_branch_label(&spec("seated", "worker"))),
        )
        .await,
        "seated",
        "the label, seated",
    );
    assert!(mob.rows().await.is_empty(), "seated: nothing is written");
    assert!(
        !mob.store_owned("seated"),
        "seated: {:?}",
        mob.read("seated")
    );
    let now = mob.member("seated").await;
    assert_eq!(
        (now.generation, now.fence_token, &now.labels),
        (member.generation, member.fence_token, &member.labels),
        "seated: the member is as it was"
    );
    committed(
        mob.adopt(
            "adopt-seated",
            "seated",
            AdoptionSource::roster(spec("seated", "worker")),
        )
        .await,
    );
    assert!(mob.store_owned("seated"));

    // The identity prefix: a roster may declare such a name (no seat
    // follows it), and its adoption writes no record of it.
    let prefixed = "approval-branch-made-up";
    let mob = StoreOwnedMob::new("adopt-branch-prefix").await;
    mob.declare(&[prefixed]).await;
    assert!(!mob.is_member(prefixed).await, "no seat takes the prefix");
    assert_branch_reserved(
        mob.adopt(
            "adopt-prefixed",
            prefixed,
            AdoptionSource::roster(spec(prefixed, "worker")),
        )
        .await,
        prefixed,
        "the identity prefix",
    );
    assert!(mob.rows().await.is_empty(), "prefix: nothing is written");
    assert!(
        !mob.store_owned(prefixed),
        "prefix: {:?}",
        mob.read(prefixed)
    );
}

/// Row (RED): an open approval branch is not adopted as a worker. Its
/// adoption is refused with the reservation's own refusal, by the name
/// every branch is seated under, and the branch stays the branch it was:
/// its roster entry records its decision on the same incarnation, it is no
/// store-owned identity, and no row is written.
///
/// RED before the adoption refused the reservation: the branch is adopted
/// into a store-owned record that holds its name and its label.
#[tokio::test]
async fn an_open_approval_branch_is_not_adopted_as_a_worker() {
    let mob = StoreOwnedMob::new("adopt-open-branch").await;
    mob.spawn_worker("grantee", None, &[]).await;
    let decision = meerkat_core::approval::decision::DecisionId::derive(
        &meerkat_core::approval::decision::DecisionKind::new("grant.child").expect("kind"),
        &meerkat_core::approval::decision::DecisionSubjectKey::new("adopt-target")
            .expect("subject key"),
    );
    let (branch, _) = tokio::time::timeout(
        STEP,
        mob.handle.open_approval_branch(&id("grantee"), &decision),
    )
    .await
    .expect("the opening answers")
    .expect("the approval branch opens");
    let name = branch.as_str().to_string();
    let opened = mob.member(&name).await;
    assert_eq!(opened.approval_branch(), Some(&decision));
    assert_branch_reserved(
        mob.adopt(
            "adopt-branch",
            &name,
            AdoptionSource::worker(opened.generation),
        )
        .await,
        &name,
        "an open approval branch",
    );
    assert!(mob.rows().await.is_empty(), "nothing is written");
    assert!(!mob.store_owned(&name), "{:?}", mob.read(&name));
    let now = mob.member(&name).await;
    assert_eq!(
        (now.approval_branch(), now.generation, now.fence_token),
        (Some(&decision), opened.generation, opened.fence_token),
        "the branch is the branch it was"
    );
}

/// The labels a host that reads `name`'s row answers its seat with.
fn host_answers_the_row(name: &str, labels: &BTreeMap<String, String>) -> SeatScript {
    SeatScript::RowLabels(BTreeMap::from([(id(name), labels.clone())]))
}

/// Restart `mob` under the host build preparer `preparer`.
async fn restart_with_preparer(mob: &mut StoreOwnedMob, preparer: &Arc<ScriptedPreparer>) {
    mob.restart_configured(MobCompositionAuthority::Authoritative, {
        let preparer = preparer.clone();
        move |builder| builder.with_member_build_preparer(preparer)
    })
    .await;
    mob.service.set_keep_alive_turns_complete_immediately(true);
}

/// Row (RED): the seat of a member adopted from a roster spec follows the
/// adopted row. `listed` is a roster member seated with no label; its
/// adoption writes its row with the labels its host's roster declares. Its
/// roster entry reads the row's labels from the adoption on. Its host
/// answers its seat from the row, as a host that composes the seats of its
/// store-owned members does: after a restart the member is built, on its
/// own session, with the row's labels, and again after a second restart.
///
/// RED on the old tree: the seat keeps the labels it was seated with, the
/// host's answer differs from them, the build is refused for good naming
/// the keys the row added, and the member is Broken.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_adopted_roster_members_seat_follows_its_row_at_its_next_build() {
    let preparer = ScriptedPreparer::new();
    let mut mob = seat_composing_mob("adopted-seat-follows-row", &preparer).await;
    mob.declare(&["listed"]).await;
    let session = mob.session("listed").await;
    let adopted = spec("listed", "worker");
    committed(
        mob.adopt(
            "adopt-listed",
            "listed",
            AdoptionSource::roster(adopted.clone()),
        )
        .await,
    );
    preparer.compose_seats(host_answers_the_row("listed", &adopted.labels));
    for _ in 0..2 {
        restart_with_preparer(&mut mob, &preparer).await;
        builds_with_labels(&mob, "listed", &session, &adopted.labels).await;
    }
    assert_eq!(mob.session("listed").await, session, "on its own session");
}

/// Row (RED): on a host that composes no seat, a member adopted from a
/// roster spec carries its row's labels all the same. Its roster entry
/// reads them from the adoption on, and its next build after a restart
/// carries them.
///
/// RED on the old tree: the member is built, with the labels it was seated
/// with; the row's labels never reach its seat.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_adopted_roster_member_carries_its_rows_labels_on_a_host_that_composes_no_seat() {
    let mut mob = StoreOwnedMob::new("adopted-seat-no-composer").await;
    mob.service.set_keep_alive_turns_complete_immediately(true);
    mob.declare(&["listed"]).await;
    let session = mob.session("listed").await;
    let adopted = spec("listed", "worker");
    committed(
        mob.adopt(
            "adopt-listed",
            "listed",
            AdoptionSource::roster(adopted.clone()),
        )
        .await,
    );
    assert_eq!(
        seat(&mob, "listed").await.labels,
        adopted.labels,
        "the roster entry reads the adopted row's labels from the adoption on"
    );
    mob.restart_as(MobCompositionAuthority::Authoritative).await;
    mob.service.set_keep_alive_turns_complete_immediately(true);
    assert_eq!(
        seat(&mob, "listed").await.labels,
        adopted.labels,
        "the start seats the row's labels on the roster entry"
    );
    builds_with_labels(&mob, "listed", &session, &adopted.labels).await;
}

/// Leave `name`'s row as a release before this one wrote it at an adoption:
/// with no record that its seat is owed a label restamp.
async fn leave_the_row_as_an_earlier_release_wrote_it(mob: &StoreOwnedMob, name: &str) {
    let row = mob.row(name).await;
    if row.label_restamp_owed {
        let store = mob
            .handle
            .identity_member
            .clone()
            .expect("the built-in identity store");
        let expected = crate::store::CreatedIdentityRowExpectation::Current {
            authority_digest: row.authority_digest.clone(),
        };
        let record = row
            .with_label_restamp_owed(false)
            .expect("the row without the record");
        crate::store::MobIdentityMemberStore::settle_created_identity_row(
            store.as_ref(),
            &crate::store::CreatedIdentityRowWrite { record, expected },
        )
        .await
        .expect("the row is written");
    }
    assert!(!label_restamp_owed(mob, name).await, "{name}");
}

/// Row (RED): a member a release before this one adopted is built again
/// with no step by its host. `plain` and `moved` were adopted from roster
/// specs by an earlier release: their rows hold the labels their host
/// declared and no record of an owed restamp, and their seat rows are as
/// their members were seated. An earlier release also updated `moved`'s
/// labels, which moved its row and left its seat. After a restart into
/// this release each roster entry reads its row's labels, and a host that
/// answers each seat from its row builds each member on its own session
/// with the row's labels. Nothing is written for it: neither row records
/// an owed restamp afterwards.
///
/// RED on the old tree: each build is refused for good naming the keys the
/// row added, and each member is Broken.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_member_an_earlier_release_adopted_is_built_with_its_rows_labels_after_a_restart() {
    let preparer = ScriptedPreparer::new();
    let mut mob = seat_composing_mob("adopted-by-an-earlier-release", &preparer).await;
    let names = ["plain", "moved"];
    mob.declare(&names).await;
    let mut rows = BTreeMap::new();
    for name in names {
        let adopted = spec(name, "worker");
        committed(
            mob.adopt(
                &format!("adopt-{name}"),
                name,
                AdoptionSource::roster(adopted.clone()),
            )
            .await,
        );
        leave_the_row_as_an_earlier_release_wrote_it(&mob, name).await;
        rows.insert(name, adopted.labels);
    }
    commit_update_transaction_of_an_earlier_release(
        &mob.handle,
        "relabel-moved",
        spec("moved", "worker").with_labels(moved_team("moved")),
    )
    .await;
    assert!(!label_restamp_owed(&mob, "moved").await);
    rows.insert("moved", moved_team("moved"));
    preparer.compose_seats(SeatScript::RowLabels(
        rows.iter()
            .map(|(name, labels)| (id(name), labels.clone()))
            .collect(),
    ));

    // The identity store's rows as the earlier release left them, in their
    // stored form.
    let mut stored = BTreeMap::new();
    for name in names {
        stored.insert(
            name,
            serde_json::to_string(&mob.row(name).await).expect("the row's stored form"),
        );
    }

    restart_with_preparer(&mut mob, &preparer).await;
    for name in names {
        let labels = &rows[name];
        let session = mob.session(name).await;
        builds_with_labels(&mob, name, &session, labels).await;
    }
    for name in names {
        assert_eq!(
            serde_json::to_string(&mob.row(name).await).expect("the row's stored form"),
            stored[name],
            "{name}: neither the start nor a build rewrote the row"
        );
        assert!(
            !label_restamp_owed(&mob, name).await,
            "{name}: a build writes nothing to the row"
        );
    }
}

/// Row: the remedy an earlier release documents for a member whose build
/// was refused for its labels serves an adopted member too. `listed` was
/// adopted from a roster spec and its host then answered a label its row
/// does not carry, so its build was refused for good and it is Broken. Its
/// host answers the row's labels from then on, and any update of the
/// identity (here its display name alone) seats the member again: the
/// update answers `restamped`, the next incarnation holds the session the
/// Broken one held, and it is built with the row's labels.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_update_seats_a_broken_adopted_member_again_on_its_session_with_its_rows_labels() {
    let preparer = ScriptedPreparer::new();
    let mut mob = seat_composing_mob("adopted-broken-remedy", &preparer).await;
    mob.declare(&["listed"]).await;
    let session = mob.session("listed").await;
    let adopted = spec("listed", "worker");
    committed(
        mob.adopt(
            "adopt-listed",
            "listed",
            AdoptionSource::roster(adopted.clone()),
        )
        .await,
    );
    let mut with_a_label_the_row_lacks = adopted.labels.clone();
    with_a_label_the_row_lacks.insert(
        LabelChange::ADDED.0.to_string(),
        LabelChange::ADDED.1.to_string(),
    );
    preparer.compose_seats(host_answers_the_row("listed", &with_a_label_the_row_lacks));
    restart_with_preparer(&mut mob, &preparer).await;
    assert!(wake(&mob, "listed").await.is_err(), "the build is refused");
    assert!(!serves(&mob, "listed"), "the member is Broken");

    preparer.compose_seats(host_answers_the_row("listed", &adopted.labels));
    let generation = mob.generation("listed").await.expect("a member");
    let updated = mob
        .update(
            "rename-listed",
            adopted.clone().with_display_name("The renamed identity"),
            &spec_digest(&adopted),
        )
        .await
        .expect("the update commits");
    assert_eq!(
        answer_kind(updated.applied.as_ref().expect("the update's answer")),
        "restamped",
        "{updated:?}"
    );
    assert_eq!(mob.generation("listed").await, generation.next());
    assert!(serves(&mob, "listed"), "the next incarnation is not Broken");
    assert_eq!(mob.session("listed").await, session, "on its own session");
    builds_with_labels(&mob, "listed", &session, &adopted.labels).await;
}

/// Row (RED): a label its row does not carry is still refused an adopted
/// member, and the refusal names that key alone. `listed` was adopted from
/// a roster spec; its host answers its seat with the row's labels and one
/// more. The build is compared with the row's labels: it is refused for
/// good naming the added key, no key of the row is named, and the member
/// is Broken.
///
/// RED on the old tree: the build is compared with the labels the member
/// was seated with, so the refusal names every key of the row as added.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_adopted_roster_member_is_refused_a_label_its_row_does_not_carry_naming_that_key_alone()
{
    let preparer = ScriptedPreparer::new();
    let mut mob = seat_composing_mob("adopted-seat-added-label", &preparer).await;
    mob.declare(&["listed"]).await;
    let adopted = spec("listed", "worker");
    committed(
        mob.adopt(
            "adopt-listed",
            "listed",
            AdoptionSource::roster(adopted.clone()),
        )
        .await,
    );
    let mut with_a_label_the_row_lacks = adopted.labels.clone();
    with_a_label_the_row_lacks.insert(
        LabelChange::ADDED.0.to_string(),
        LabelChange::ADDED.1.to_string(),
    );
    preparer.compose_seats(host_answers_the_row("listed", &with_a_label_the_row_lacks));
    restart_with_preparer(&mut mob, &preparer).await;
    let refused = wake(&mob, "listed").await;
    let reason = refused
        .as_ref()
        .expect_err("a label the row lacks is refused")
        .to_string();
    assert!(
        reason.contains(&format!("label keys added [{}])", LabelChange::ADDED.0)),
        "the refusal names the added key alone: {reason}"
    );
    assert!(
        !reason.contains(LabelChange::ADDED.1),
        "the refusal names no value: {reason}"
    );
    assert!(!serves(&mob, "listed"), "the member is Broken");
}

/// Row: an update that sets a value the identity already has lets a member
/// an earlier release adopted build again. `listed` was adopted from a
/// roster spec by an earlier release (its row records no owed restamp) and
/// has no live build after a restart. An update of the identity with the
/// spec it has commits all the same: the row is at its next revision and
/// records the restamp its seat is owed, the member is kept (no successor
/// is seated: its generation and session are its own), and its next build
/// carries the row's labels, again after a cold restart.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_update_that_sets_an_unchanged_value_lets_an_adopted_member_with_no_live_build_build_again()
 {
    let preparer = ScriptedPreparer::new();
    let mut mob = seat_composing_mob("adopted-unchanged-update", &preparer).await;
    mob.declare(&["listed"]).await;
    let session = mob.session("listed").await;
    let adopted = spec("listed", "worker");
    committed(
        mob.adopt(
            "adopt-listed",
            "listed",
            AdoptionSource::roster(adopted.clone()),
        )
        .await,
    );
    leave_the_row_as_an_earlier_release_wrote_it(&mob, "listed").await;
    preparer.compose_seats(host_answers_the_row("listed", &adopted.labels));
    restart_with_preparer(&mut mob, &preparer).await;
    let generation = mob.generation("listed").await.expect("a member");

    let updated = mob
        .update("same-listed", adopted.clone(), &spec_digest(&adopted))
        .await
        .expect("an update that sets the spec the identity has commits");
    assert_eq!(
        answer_kind(updated.applied.as_ref().expect("the update's answer")),
        "kept",
        "{updated:?}"
    );
    let row = mob.row("listed").await;
    assert_eq!(
        row.revision, 2,
        "an unchanged value commits the row at its next revision"
    );
    assert!(
        row.label_restamp_owed,
        "the update records the restamp the member's seat is owed"
    );
    assert_eq!(
        mob.generation("listed").await,
        Some(generation),
        "no successor is seated"
    );
    for _ in 0..2 {
        builds_with_labels(&mob, "listed", &session, &adopted.labels).await;
        restart_with_preparer(&mut mob, &preparer).await;
    }
    assert_eq!(mob.session("listed").await, session, "on its own session");
}

/// Row: an update that sets a value the identity already has seats a
/// Broken adopted member again. `listed` was adopted from a roster spec;
/// its host answered a label its row does not carry, so its build was
/// refused for good and it is Broken. Its host answers the row's labels
/// from then on, and an update of the identity with the spec it has
/// commits and answers `restamped`: the next incarnation holds the session
/// the Broken one held and is built with the row's labels.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_update_that_sets_an_unchanged_value_seats_a_broken_adopted_member_again() {
    let preparer = ScriptedPreparer::new();
    let mut mob = seat_composing_mob("adopted-broken-unchanged-update", &preparer).await;
    mob.declare(&["listed"]).await;
    let session = mob.session("listed").await;
    let adopted = spec("listed", "worker");
    committed(
        mob.adopt(
            "adopt-listed",
            "listed",
            AdoptionSource::roster(adopted.clone()),
        )
        .await,
    );
    let mut with_a_label_the_row_lacks = adopted.labels.clone();
    with_a_label_the_row_lacks.insert(
        LabelChange::ADDED.0.to_string(),
        LabelChange::ADDED.1.to_string(),
    );
    preparer.compose_seats(host_answers_the_row("listed", &with_a_label_the_row_lacks));
    restart_with_preparer(&mut mob, &preparer).await;
    assert!(wake(&mob, "listed").await.is_err(), "the build is refused");
    assert!(!serves(&mob, "listed"), "the member is Broken");

    preparer.compose_seats(host_answers_the_row("listed", &adopted.labels));
    let generation = mob.generation("listed").await.expect("a member");
    let updated = mob
        .update("same-listed", adopted.clone(), &spec_digest(&adopted))
        .await
        .expect("an update that sets the spec the identity has commits");
    assert_eq!(
        answer_kind(updated.applied.as_ref().expect("the update's answer")),
        "restamped",
        "{updated:?}"
    );
    assert_eq!(mob.row("listed").await.revision, 2);
    assert_eq!(mob.generation("listed").await, generation.next());
    assert!(serves(&mob, "listed"), "the next incarnation is not Broken");
    assert_eq!(mob.session("listed").await, session, "on its own session");
    builds_with_labels(&mob, "listed", &session, &adopted.labels).await;
}
