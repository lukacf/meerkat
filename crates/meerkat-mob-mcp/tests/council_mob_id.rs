//! The `council--` mob id form belongs to temporary councils.
//!
//! A temporary council runs in a real mob whose id is `council--` followed
//! by the council id. These rows drive real components (a real `MobMcpState`
//! over rooted durable custody, real mobs and members, a scripted model) and
//! pin three things:
//!
//! - no create door makes an ordinary mob under an id of that form, while a
//!   council's own create still does;
//! - a council's cleanup acts on the mob that council created and on no
//!   other mob that holds the same id, however late it runs;
//! - an ordinary mob already stored under such an id keeps loading and
//!   working, and its operator is told once.

#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use meerkat_core::SessionId;
use meerkat_core::service::MobIncarnationId;
use meerkat_mob::machines::temporary_council_lifecycle::{
    TemporaryCouncilLifecycleInput, TemporaryCouncilLifecycleMachineAuthority,
    TemporaryCouncilLifecycleMachineMutator, TemporaryCouncilLifecycleState,
};
use meerkat_mob::store::{MobStoreError, TemporaryCouncilRecord, TemporaryCouncilStore};
use meerkat_mob::temporary_council::{
    TEMPORARY_COUNCIL_MOB_ID_PREFIX, TemporaryCouncilCleanupDebt, TemporaryCouncilCleanupReceipt,
    TemporaryCouncilExitReason, TemporaryCouncilId, TemporaryCouncilStoreDurability,
};
use meerkat_mob::{
    DiagnosticCode, MobBackendKind, MobBuilder, MobDefinition, MobError, MobId, MobRuntimeMode,
    MobStorage, ProfileName,
};
use meerkat_mob_mcp::MobMcpState;
use meerkat_mob_mcp::temporary_council::{
    MergeBackPolicy, TemporaryCouncilBounds, TemporaryCouncilError,
    TemporaryCouncilParticipantSpec, TemporaryCouncilRequest,
};
use support::{CouncilFixture, ScriptedTurn, council_definition, identity, role_in_request};

const WAIT: Duration = Duration::from_mins(1);

fn role_script(request: &meerkat_client::LlmRequest) -> ScriptedTurn {
    match role_in_request(request) {
        Some(role) => ScriptedTurn::Text(format!("position from {role}")),
        None => ScriptedTurn::Text("ok".to_string()),
    }
}

/// A one-round council of the fixture's `researcher` and `reviewer`, seated
/// as `analyst` and `critic`.
fn council_request(fixture: &CouncilFixture, id: &TemporaryCouncilId) -> TemporaryCouncilRequest {
    let participant = |order, role: &str, source: &str, target: &str| {
        TemporaryCouncilParticipantSpec::new(
            order,
            role,
            fixture.source_mob_id(),
            identity(source),
            identity(target),
            ProfileName::from("participant"),
        )
    };
    TemporaryCouncilRequest::new(
        id.clone(),
        council_definition("template-is-replaced"),
        vec![
            participant(0, "analyst", "researcher", "analyst"),
            participant(1, "critic", "reviewer", "critic"),
        ],
        "Is the id form reserved?",
        TemporaryCouncilBounds::relative(Duration::from_mins(4), 1, 4096),
        MergeBackPolicy::NoMerge,
    )
}

/// Where the fixture's state keeps the database of `mob_id`.
fn mob_database(fixture: &CouncilFixture, mob_id: &MobId) -> std::path::PathBuf {
    MobMcpState::persistent_forked_participant_store_path(&fixture.root.join("state"))
        .parent()
        .expect("the per-mob database directory")
        .join(format!("mob--{mob_id}.db"))
}

/// Whether `error` is the refusal of an ordinary create under the reserved
/// form: the definition error every invalid definition answers with, whose
/// diagnostic names the reserved identifier at `id`.
fn is_reserved_form_refusal(error: &MobError) -> bool {
    matches!(
        error,
        MobError::DefinitionError(diagnostics) if diagnostics.iter().any(|diagnostic| {
            diagnostic.code == DiagnosticCode::ReservedSystemIdentifier
                && diagnostic.location.as_deref() == Some("id")
        })
    )
}

/// The members `mob_id` lists.
async fn members(state: &Arc<MobMcpState>, mob_id: &MobId) -> BTreeSet<String> {
    state
        .handle_for(mob_id)
        .await
        .expect("the mob is managed")
        .list_members()
        .await
        .into_iter()
        .map(|member| member.agent_identity.to_string())
        .collect()
}

async fn spawn_member(state: &Arc<MobMcpState>, mob_id: &MobId, member: &str) {
    state
        .mob_spawn(
            mob_id,
            ProfileName::from("participant"),
            identity(member),
            Some(MobRuntimeMode::TurnDriven),
            Some(MobBackendKind::Session),
            None,
        )
        .await
        .unwrap_or_else(|error| panic!("spawn {member} in {mob_id}: {error}"));
}

/// Run one turn of `member` to the model: the mob serves its member.
async fn serves_a_turn(fixture: &CouncilFixture, mob_id: &MobId, member: &str) {
    let before = fixture.provider_calls();
    let handle = fixture
        .state
        .handle_for(mob_id)
        .await
        .expect("the mob is managed");
    handle
        .member(&identity(member))
        .await
        .expect("the member is reachable")
        .internal_turn(meerkat_core::types::ContentInput::Text("ping".to_string()))
        .await
        .expect("the turn is admitted");
    tokio::time::timeout(WAIT, async {
        while fixture.provider_calls() == before {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the member's turn reaches the model");
}

/// An ordinary mob a host built itself and inserted under `mob_id`, with
/// `seated` as its members. Answers its incarnation.
async fn host_inserts_a_mob(
    fixture: &CouncilFixture,
    mob_id: &MobId,
    seated: &[&str],
) -> MobIncarnationId {
    let handle = MobBuilder::new(council_definition(mob_id.as_str()), MobStorage::in_memory())
        .with_session_service(fixture.state.session_service())
        .allow_ephemeral_sessions(true)
        .create()
        .await
        .expect("the host builds its own mob");
    let incarnation = handle.incarnation();
    fixture
        .state
        .try_mob_insert_handle(mob_id.clone(), handle)
        .await
        .expect("the host inserts its mob");
    for member in seated {
        spawn_member(&fixture.state, mob_id, member).await;
    }
    incarnation
}

/// The mob under `mob_id` is the one that ran at `incarnation`, lists exactly
/// `seated`, seats one more member and runs a turn.
async fn assert_untouched_and_served(
    fixture: &CouncilFixture,
    mob_id: &MobId,
    incarnation: MobIncarnationId,
    seated: &[&str],
) {
    let handle = fixture
        .state
        .handle_for(mob_id)
        .await
        .expect("the mob under the council's id is still managed");
    assert_eq!(
        handle.incarnation(),
        incarnation,
        "the mob under the council's id is the one that was there"
    );
    drop(handle);
    assert_eq!(
        members(&fixture.state, mob_id).await,
        seated.iter().map(ToString::to_string).collect(),
        "its members are untouched"
    );
    spawn_member(&fixture.state, mob_id, "seated-afterwards").await;
    serves_a_turn(fixture, mob_id, "seated-afterwards").await;
}

/// A council store whose first commit that `fails` matches is refused, once:
/// the write a process that died at that point never made.
struct FailFirstCommit {
    inner: Arc<dyn TemporaryCouncilStore>,
    fails: fn(&TemporaryCouncilRecord) -> bool,
    fired: AtomicBool,
}

#[async_trait::async_trait]
impl TemporaryCouncilStore for FailFirstCommit {
    fn durability(&self) -> TemporaryCouncilStoreDurability {
        self.inner.durability()
    }

    async fn insert_new(
        &self,
        record: &TemporaryCouncilRecord,
    ) -> Result<TemporaryCouncilRecord, MobStoreError> {
        self.inner.insert_new(record).await
    }

    async fn load(
        &self,
        council_id: &TemporaryCouncilId,
    ) -> Result<Option<TemporaryCouncilRecord>, MobStoreError> {
        self.inner.load(council_id).await
    }

    async fn commit(
        &self,
        record: &TemporaryCouncilRecord,
    ) -> Result<TemporaryCouncilRecord, MobStoreError> {
        if (self.fails)(record) && !self.fired.swap(true, Ordering::SeqCst) {
            return Err(MobStoreError::WriteFailed(
                "injected council custody write fault".to_string(),
            ));
        }
        self.inner.commit(record).await
    }

    async fn list_all(&self) -> Result<Vec<TemporaryCouncilRecord>, MobStoreError> {
        self.inner.list_all().await
    }
}

/// A fixture whose durable council store refuses, once, the first commit
/// `fails` matches.
fn fixture_failing(fails: fn(&TemporaryCouncilRecord) -> bool) -> CouncilFixture {
    CouncilFixture::new_with(role_script, move |state, root| {
        let path = MobMcpState::persistent_forked_participant_store_path(root);
        let inner: Arc<dyn TemporaryCouncilStore> = Arc::new(
            meerkat_mob::store::SqliteTemporaryCouncilStore::open(&path)
                .expect("open durable council custody"),
        );
        state.with_temporary_council_store(Arc::new(FailFirstCommit {
            inner,
            fails,
            fired: AtomicBool::new(false),
        }))
    })
}

/// The commit that records a cleanup attempt.
fn the_cleanup_receipt(record: &TemporaryCouncilRecord) -> bool {
    record.cleanup.is_some()
}

/// The commit that seals the council's result.
fn the_sealed_result(record: &TemporaryCouncilRecord) -> bool {
    record.result.is_some()
}

async fn load(fixture: &CouncilFixture, id: &TemporaryCouncilId) -> TemporaryCouncilRecord {
    fixture
        .state
        .temporary_council_store_for_tests()
        .load(id)
        .await
        .expect("read council custody")
        .expect("the council record")
}

/// Wait until the record of `id` satisfies `ready`.
async fn record_when(
    fixture: &CouncilFixture,
    id: &TemporaryCouncilId,
    ready: impl Fn(&TemporaryCouncilRecord) -> bool,
) -> TemporaryCouncilRecord {
    tokio::time::timeout(WAIT, async {
        loop {
            let record = load(fixture, id).await;
            if ready(&record) {
                return record;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the council record reaches the awaited state")
}

/// Rewrite the record of `id` as a release that recorded nothing about the
/// temporary mob wrote it: the same record without that key. `change` then
/// edits the rest of it.
async fn as_an_earlier_release_wrote_it(
    fixture: &CouncilFixture,
    id: &TemporaryCouncilId,
    change: impl FnOnce(&mut TemporaryCouncilRecord),
) {
    let record = load(fixture, id).await;
    let mut json = serde_json::to_value(&record).expect("encode the record");
    json.as_object_mut()
        .expect("the record is an object")
        .remove("temporary_mob_custody");
    let mut earlier: TemporaryCouncilRecord =
        serde_json::from_value(json).expect("a record without the key still decodes");
    change(&mut earlier);
    fixture
        .state
        .temporary_council_store_for_tests()
        .commit(&earlier)
        .await
        .expect("store the earlier release's record");
}

/// Run every cleanup the state still owes, and wait out the sweep the state
/// may have started on its own.
async fn run_owed_cleanups(fixture: &CouncilFixture) {
    fixture
        .state
        .temporary_council()
        .recover_unfinished()
        .await
        .expect("the recovery sweep runs");
}

// ===========================================================================
// (a) No create door makes an ordinary mob under the reserved form
// ===========================================================================

/// Every Rust create entry of the mob state refuses an ordinary mob whose id
/// has the council form, before anything of the mob exists, and creates the
/// same definition under an id without the form.
#[tokio::test(flavor = "multi_thread")]
async fn every_rust_create_door_refuses_an_ordinary_mob_under_the_council_form() {
    let fixture = CouncilFixture::new(role_script);
    let state = &fixture.state;

    type Door = (
        &'static str,
        Box<
            dyn Fn(
                Arc<MobMcpState>,
                MobDefinition,
            ) -> std::pin::Pin<
                Box<dyn std::future::Future<Output = Result<MobId, MobError>> + Send>,
            >,
        >,
    );
    let doors: Vec<Door> = vec![
        (
            "mob_create_definition",
            Box::new(|state, definition| {
                Box::pin(async move { state.mob_create_definition(definition).await })
            }),
        ),
        (
            "mob_create_definition_with_member_external_tools",
            Box::new(|state, definition| {
                Box::pin(async move {
                    let no_tools: meerkat_mob::ExternalToolsProvider = Arc::new(|| None);
                    state
                        .mob_create_definition_with_member_external_tools(definition, no_tools)
                        .await
                })
            }),
        ),
        (
            "mob_create_definition_with_owner_bridge_session",
            Box::new(|state, definition| {
                Box::pin(async move {
                    state
                        .mob_create_definition_with_owner_bridge_session(
                            definition,
                            SessionId::new(),
                            false,
                            false,
                        )
                        .await
                })
            }),
        ),
        (
            "mob_create_definition_with_claim_verifier",
            Box::new(|state, definition| {
                Box::pin(async move {
                    let token = meerkat_mob::MobClaimToken::mint().expect("claim token");
                    state
                        .mob_create_definition_with_claim_verifier(
                            definition,
                            None,
                            token.verifier(),
                        )
                        .await
                })
            }),
        ),
        (
            "mob_create_from_mobpack",
            Box::new(|state, definition| {
                Box::pin(async move {
                    state
                        .mob_create_from_mobpack(
                            definition,
                            BTreeMap::new(),
                            meerkat_mob::MobDefinitionSourceIdentity::mobpack(
                                "a".repeat(64),
                                Vec::new(),
                            ),
                        )
                        .await
                })
            }),
        ),
    ];

    let mut wrong = Vec::new();
    for (index, (door, create)) in doors.iter().enumerate() {
        let reserved = MobId::from(format!(
            "{TEMPORARY_COUNCIL_MOB_ID_PREFIX}door{index}-{}",
            fixture.scope
        ));
        match create(Arc::clone(state), council_definition(reserved.as_str())).await {
            Err(error) if is_reserved_form_refusal(&error) => {}
            Err(error) => wrong.push(format!("{door}: refused, but with: {error}")),
            Ok(_) => wrong.push(format!("{door}: created the ordinary mob {reserved}")),
        }
        if state.handle_for(&reserved).await.is_ok() {
            wrong.push(format!("{door}: a mob is managed under {reserved}"));
        }
        if mob_database(&fixture, &reserved).exists() {
            wrong.push(format!("{door}: storage was allocated for {reserved}"));
        }

        // Control: the form is `council--`; one hyphen is an ordinary id.
        let ordinary = MobId::from(format!("council-door{index}-{}", fixture.scope));
        if let Err(error) = create(Arc::clone(state), council_definition(ordinary.as_str())).await {
            wrong.push(format!(
                "{door}: refused the ordinary id {ordinary}: {error}"
            ));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");

    fixture.teardown().await;
}

// ===========================================================================
// (d) A council is still created, settles and is cleaned up
// ===========================================================================

/// A council's own create uses the form: the council runs, its record keeps
/// the incarnation of the mob it created, its cleanup destroys that mob and
/// settles, and the id stays reserved afterwards.
#[tokio::test(flavor = "multi_thread")]
async fn a_council_creates_its_mob_under_the_form_settles_and_cleans_up() {
    let fixture = CouncilFixture::new(role_script);
    fixture.seed_source_mob(&["researcher", "reviewer"]).await;
    let id = fixture.council_id("own-door");
    let mob_id = id.temporary_mob_id();
    assert!(mob_id.as_str().starts_with(TEMPORARY_COUNCIL_MOB_ID_PREFIX));

    let outcome = fixture
        .state
        .temporary_council()
        .run(council_request(&fixture, &id))
        .await
        .expect("the council runs");
    assert_eq!(
        outcome.result.exit_reason,
        TemporaryCouncilExitReason::Completed
    );
    assert_eq!(outcome.result.temporary_mob_id, mob_id);
    assert!(outcome.cleanup.settled(), "{:?}", outcome.cleanup);
    assert_eq!(outcome.cleanup.released_participants.len(), 2);
    assert!(
        fixture.state.handle_for(&mob_id).await.is_err(),
        "the council's mob is destroyed"
    );
    assert!(!mob_database(&fixture, &mob_id).exists());

    let record = load(&fixture, &id).await;
    assert_eq!(
        record.machine_state.lifecycle_phase,
        TemporaryCouncilLifecycleState::Settled
    );
    let custody =
        serde_json::to_value(&record).expect("encode the record")["temporary_mob_custody"].clone();
    assert_eq!(
        custody["state"], "created",
        "the record keeps what the council created: {custody}"
    );
    assert!(
        custody["incarnation"]
            .as_str()
            .is_some_and(|incarnation| incarnation.parse::<MobIncarnationId>().is_ok()),
        "the record keeps the incarnation of the mob the council created: {custody}"
    );

    // The id of a settled council is still no ordinary mob's to take.
    let reused = fixture
        .state
        .mob_create_definition(council_definition(mob_id.as_str()))
        .await;
    assert!(
        matches!(&reused, Err(error) if is_reserved_form_refusal(error)),
        "an ordinary create under a settled council's mob id: {reused:?}"
    );

    fixture.teardown().await;
}

// ===========================================================================
// (b) A late cleanup acts only on the mob the council created
// ===========================================================================

/// The council destroyed its mob and died before its cleanup receipt was
/// stored. A host then inserts a mob of its own under the same id, with
/// members named like the council's seats. The cleanup that runs again
/// retires none of them and does not destroy the mob: the record names the
/// incarnation the council created, and that mob is gone.
#[tokio::test(flavor = "multi_thread")]
async fn a_late_cleanup_leaves_a_later_mob_under_the_councils_id_untouched() {
    let fixture = fixture_failing(the_cleanup_receipt);
    fixture.seed_source_mob(&["researcher", "reviewer"]).await;
    let id = fixture.council_id("late-cleanup");
    let mob_id = id.temporary_mob_id();

    let error = fixture
        .state
        .temporary_council()
        .run(council_request(&fixture, &id))
        .await
        .expect_err("the cleanup receipt was never stored");
    assert!(
        matches!(error, TemporaryCouncilError::Store { .. }),
        "{error:?}"
    );
    assert!(
        fixture.state.handle_for(&mob_id).await.is_err(),
        "the council's own cleanup destroyed its mob"
    );
    let record = load(&fixture, &id).await;
    assert!(record.result.is_some() && record.cleanup.is_none());
    assert!(record.is_unfinished(), "the council still owes its cleanup");

    let seated = ["analyst", "critic", "later-worker"];
    let later = host_inserts_a_mob(&fixture, &mob_id, &seated).await;

    run_owed_cleanups(&fixture).await;
    let record = record_when(&fixture, &id, |record| !record.is_unfinished()).await;
    let cleanup = record.cleanup.expect("the late cleanup's receipt");
    assert!(cleanup.settled(), "{cleanup:?}");
    assert!(
        cleanup.released_participants.is_empty(),
        "the late cleanup retired no member: {cleanup:?}"
    );
    assert_untouched_and_served(&fixture, &mob_id, later, &seated).await;

    // And once more: a settled council owes nothing.
    run_owed_cleanups(&fixture).await;
    assert_eq!(
        fixture
            .state
            .handle_for(&mob_id)
            .await
            .expect("still managed")
            .incarnation(),
        later
    );

    fixture.teardown().await;
}

/// The same crash, in a record an earlier release wrote: it names no
/// incarnation. The mob found under the id has members the record does not
/// know, so the cleanup leaves it alone and keeps its debt; once that mob is
/// gone the council settles.
#[tokio::test(flavor = "multi_thread")]
async fn an_earlier_releases_record_leaves_a_mob_whose_members_it_does_not_know() {
    let fixture = fixture_failing(the_cleanup_receipt);
    fixture.seed_source_mob(&["researcher", "reviewer"]).await;
    let id = fixture.council_id("earlier-unknown");
    let mob_id = id.temporary_mob_id();

    fixture
        .state
        .temporary_council()
        .run(council_request(&fixture, &id))
        .await
        .expect_err("the cleanup receipt was never stored");
    as_an_earlier_release_wrote_it(&fixture, &id, |_| {}).await;

    let seated = ["analyst", "critic", "later-worker"];
    let later = host_inserts_a_mob(&fixture, &mob_id, &seated).await;

    run_owed_cleanups(&fixture).await;
    let record = record_when(&fixture, &id, |record| record.cleanup.is_some()).await;
    let cleanup = record.cleanup.clone().expect("the cleanup's receipt");
    assert!(
        !cleanup.settled() && !cleanup.temporary_mob_destroyed,
        "a mob the record cannot call its own keeps the debt: {cleanup:?}"
    );
    assert!(
        cleanup.released_participants.is_empty(),
        "the cleanup retired no member: {cleanup:?}"
    );
    assert!(record.is_unfinished());
    assert_untouched_and_served(&fixture, &mob_id, later, &seated).await;

    // The operator removes that mob; the council then owes nothing.
    fixture
        .state
        .mob_destroy(&mob_id)
        .await
        .expect("the operator destroys the mob");
    run_owed_cleanups(&fixture).await;
    let record = record_when(&fixture, &id, |record| !record.is_unfinished()).await;
    assert!(record.cleanup.expect("receipt").settled());

    fixture.teardown().await;
}

/// An earlier release's record whose last cleanup receipt already says the
/// temporary mob is destroyed, with a debt left: the retry never looks for a
/// mob to destroy, so a mob a host inserted under the id since is untouched.
#[tokio::test(flavor = "multi_thread")]
async fn an_earlier_releases_receipt_that_says_destroyed_spares_the_later_mob() {
    let fixture = fixture_failing(the_cleanup_receipt);
    fixture.seed_source_mob(&["researcher", "reviewer"]).await;
    let id = fixture.council_id("earlier-destroyed");
    let mob_id = id.temporary_mob_id();

    fixture
        .state
        .temporary_council()
        .run(council_request(&fixture, &id))
        .await
        .expect_err("the cleanup receipt was never stored");

    // The record as the earlier release left it after its first cleanup:
    // the mob destroyed, both seats released, one obligation still owed,
    // under a coordinator that is gone.
    as_an_earlier_release_wrote_it(&fixture, &id, |record| {
        let coordinator = "an-earlier-coordinator".to_string();
        let mut authority = TemporaryCouncilLifecycleMachineAuthority::new();
        for input in [
            TemporaryCouncilLifecycleInput::Open {
                request_fingerprint: record.request_fingerprint.clone(),
            },
            TemporaryCouncilLifecycleInput::Claim {
                claim_id: coordinator.clone(),
                lease_expired: false,
            },
            TemporaryCouncilLifecycleInput::StartDiscussion {
                claim_id: coordinator.clone(),
                claim_epoch: 1,
            },
            TemporaryCouncilLifecycleInput::StartMerge {
                claim_id: coordinator.clone(),
                claim_epoch: 1,
            },
            TemporaryCouncilLifecycleInput::SealResult {
                claim_id: coordinator.clone(),
                claim_epoch: 1,
            },
            TemporaryCouncilLifecycleInput::RecordCleanupDebt {
                claim_id: coordinator,
                claim_epoch: 1,
            },
        ] {
            TemporaryCouncilLifecycleMachineMutator::apply(&mut authority, input)
                .expect("the council lifecycle admits the step");
        }
        record.machine_state = authority.state().clone();
        record.claim_lease_expires_at = chrono::Utc::now() - chrono::Duration::seconds(1);
        record.cleanup = Some(TemporaryCouncilCleanupReceipt {
            attempted_at: chrono::Utc::now(),
            attempts: 1,
            temporary_mob_destroyed: true,
            released_participants: vec![0, 1],
            revoked_participants: Vec::new(),
            debts: vec![TemporaryCouncilCleanupDebt {
                subject: "capability:owed".to_string(),
                detail: "revocation failed".to_string(),
            }],
            budget_exhausted: false,
        });
    })
    .await;
    assert!(load(&fixture, &id).await.is_unfinished());

    let seated = ["later-worker"];
    let later = host_inserts_a_mob(&fixture, &mob_id, &seated).await;

    run_owed_cleanups(&fixture).await;
    let record = record_when(&fixture, &id, |record| !record.is_unfinished()).await;
    let cleanup = record.cleanup.expect("the retry's receipt");
    assert!(cleanup.settled(), "{cleanup:?}");
    assert_untouched_and_served(&fixture, &mob_id, later, &seated).await;

    fixture.teardown().await;
}

/// Control for the two rows above: an earlier release's record whose council
/// died with its mob alive and its seats in it is still cleaned up. The mob
/// found under the id holds exactly the sessions the record seated.
#[tokio::test(flavor = "multi_thread")]
async fn an_earlier_releases_record_still_cleans_up_its_own_mob() {
    let fixture = fixture_failing(the_sealed_result);
    fixture.seed_source_mob(&["researcher", "reviewer"]).await;
    let id = fixture.council_id("earlier-own");
    let mob_id = id.temporary_mob_id();

    fixture
        .state
        .temporary_council()
        .run(council_request(&fixture, &id))
        .await
        .expect_err("the result was never sealed");
    assert_eq!(
        members(&fixture.state, &mob_id).await,
        BTreeSet::from(["analyst".to_string(), "critic".to_string()]),
        "the council died with its seats in its mob"
    );
    as_an_earlier_release_wrote_it(&fixture, &id, |_| {}).await;

    run_owed_cleanups(&fixture).await;
    let record = record_when(&fixture, &id, |record| !record.is_unfinished()).await;
    let cleanup = record.cleanup.expect("the recovery's receipt");
    assert!(cleanup.settled(), "{cleanup:?}");
    assert_eq!(cleanup.released_participants.len(), 2, "{cleanup:?}");
    assert!(
        fixture.state.handle_for(&mob_id).await.is_err(),
        "the council's own mob is destroyed"
    );

    fixture.teardown().await;
}

// ===========================================================================
// (c) An ordinary mob already stored under the form
// ===========================================================================

/// The WARN lines this process logged.
#[derive(Clone, Default)]
struct Warnings(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Warnings {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Warnings {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

impl Warnings {
    /// Capture this process's WARN lines from now on.
    fn capture() -> &'static Self {
        static WARNINGS: OnceLock<Warnings> = OnceLock::new();
        WARNINGS.get_or_init(|| {
            let warnings = Self::default();
            tracing_subscriber::fmt()
                .with_writer(warnings.clone())
                .with_ansi(false)
                .with_max_level(tracing::Level::WARN)
                .try_init()
                .expect("this test binary installs the only subscriber");
            warnings
        })
    }

    /// The captured lines that name `mob_id` as holding a reserved id.
    fn about_the_reserved_form(&self, mob_id: &MobId) -> Vec<String> {
        String::from_utf8_lossy(
            &self
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
        .lines()
        .filter(|line| {
            line.contains(mob_id.as_str()) && line.contains("reserved for temporary councils")
        })
        .map(str::to_string)
        .collect()
    }
}

/// An ordinary mob stored under a `council--` id, as a release that did not
/// reserve the form could create it. The state restores and serves it, and
/// tells the operator once. A council convened under the matching council id
/// ends at seating without touching it. Once the operator destroys it, the
/// id is refused like any other id of the form.
#[tokio::test(flavor = "multi_thread")]
async fn an_ordinary_mob_stored_under_the_council_form_keeps_loading_and_working() {
    let warnings = Warnings::capture();
    let fixture = CouncilFixture::new(role_script);
    let id = fixture.council_id("stored");
    let mob_id = id.temporary_mob_id();

    // The stored mob, before the state has restored anything.
    let database = mob_database(&fixture, &mob_id);
    std::fs::create_dir_all(database.parent().expect("the mob root")).expect("create the mob root");
    let stored = MobBuilder::new(
        council_definition(mob_id.as_str()),
        MobStorage::persistent(&database).expect("the stored mob's database"),
    )
    .with_session_service(fixture.state.session_service())
    .create()
    .await
    .expect("the earlier release created the mob");
    let incarnation = stored.incarnation();
    stored.shutdown().await.expect("the earlier process ended");
    drop(stored);

    // The state's first verb restores it: no create door is involved.
    fixture.seed_source_mob(&["researcher", "reviewer"]).await;
    let restored = fixture
        .state
        .handle_for(&mob_id)
        .await
        .expect("the stored mob is restored and managed");
    assert_eq!(restored.incarnation(), incarnation);
    drop(restored);
    spawn_member(&fixture.state, &mob_id, "worker").await;
    serves_a_turn(&fixture, &mob_id, "worker").await;

    // A council convened under the matching council id is not seated in it.
    let outcome = fixture
        .state
        .temporary_council()
        .run(council_request(&fixture, &id))
        .await
        .expect("the council reaches a terminal outcome");
    assert!(
        matches!(
            outcome.result.exit_reason,
            TemporaryCouncilExitReason::ParticipantSeatingFailed {
                participant_order: 0,
                ..
            }
        ),
        "the council ends at seating: {:?}",
        outcome.result.exit_reason
    );
    assert!(outcome.result.exchanges.is_empty(), "no council turn ran");
    assert!(
        load(&fixture, &id).await.participants.iter().all(|seat| {
            seat.acquisition
                == meerkat_mob::temporary_council::TemporaryCouncilAcquisition::NotAttempted
        }),
        "the council ended before it forked anybody"
    );
    assert!(outcome.cleanup.settled(), "{:?}", outcome.cleanup);
    assert_untouched_and_served(&fixture, &mob_id, incarnation, &["worker"]).await;

    // The operator was told once, by name.
    let told = warnings.about_the_reserved_form(&mob_id);
    assert_eq!(told.len(), 1, "one line for the stored mob: {told:#?}");

    // The operator can still destroy it; the id is then reserved.
    fixture
        .state
        .mob_destroy(&mob_id)
        .await
        .expect("the operator destroys the stored mob");
    let again = fixture
        .state
        .mob_create_definition(council_definition(mob_id.as_str()))
        .await;
    assert!(
        matches!(&again, Err(error) if is_reserved_form_refusal(error)),
        "an ordinary create under the freed id: {again:?}"
    );

    fixture.teardown().await;
}
