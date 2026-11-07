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
