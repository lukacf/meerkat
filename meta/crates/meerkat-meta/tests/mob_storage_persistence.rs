//! Persistent mob storage: durable mob state must survive a literal drop and
//! rebootstrap, and a resume must never silently boot a stale composition.
//!
//! Before this lane existed, the persistent launch paths composed
//! `MobStorage::in_memory()`, so every restart presented as a healthy boot
//! with the right member count and no durable mob state at all.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use meerkat::{AgentFactory, Config, build_ephemeral_service};
use meerkat_client::TestClient;
use meerkat_core::image_generation::{
    SwitchTurnDuration, SwitchTurnIntent, SwitchTurnOrigin, SwitchTurnReasonTextDisposition,
    SwitchTurnRequestId,
};
use meerkat_core::lifecycle::RunId;
use meerkat_core::lifecycle::run_primitive::ModelId;
use meerkat_core::session::model_routing_control::SessionModelRoutingControlRecord;
use meerkat_core::{SessionLlmIdentity, SessionStore};
use meerkat_meta::identity_first::orchestrator::RestoreOutcome;
use meerkat_meta::identity_first::{
    AgentAddressability, AgentIdentity as MetaAgentIdentity, AgentRuntimeServices, ContinuityStore,
    DurabilityPolicy, DurableAgentSpec, EdgeDiscoveryTopologyAdapter, IdentityFirstRuntimeContext,
    IdentityRuntime, IdentityRuntimeConfig, LocalContinuityStore, LocalLeaseProvider,
    MobSessionBridge, MutableRosterProvider, SessionBridge,
};
use meerkat_meta::mob_composition_manifest::{
    CandidateDefinition, MobCompositionManifest, MobCompositionProvenanceError,
    MobStorageProvenance, manifest_path, persistent_mob_storage,
};
use meerkat_meta::mob_handle_runtime::{MobRuntimeError, auto_mark_declared_resume_overrides};
use meerkat_meta::spec_update_ceremony::{SpecUpdateError, declare_spec_update};
use meerkat_meta::storage_health::{BlobDurability, ResolvedStorageSummary, StorageSlotSummary};
use meerkat_meta::unified_runtime::edge_reconcile::DefinitionWiringEdgeDiscovery;
use meerkat_meta::{
    DesiredPeerEdge, DiscoverySpec, IdentityBootstrapMode, MetaConfig, MobBootstrapOptions,
    MobBootstrapSpec, MobRuntime, UnifiedRuntime,
};
use meerkat_mob::event::MobEventKind;
use meerkat_mob::ids::ProfileName;
use meerkat_mob::store::{InMemoryMobSpecStore, MobEventStore, MobSpecStore, SqliteMobStores};
use meerkat_mob::{
    MobBuilder, MobDefinition, MobDefinitionProjectionHealth, MobDefinitionProjectionMismatchKind,
    MobStorage, SpawnMemberSpec,
};

fn definition_with(profiles: &str) -> MobDefinition {
    MobDefinition::from_toml(profiles).expect("parse mob definition")
}

/// The comms participant registry is process-global and the supervisor name is
/// derived from the mob id, so every test needs its own id or parallel tests
/// collide with `ParticipantNameOccupied` on a name that has nothing to do
/// with what they are testing.
fn base_definition_for(mob_id: &str) -> MobDefinition {
    definition_with(&format!(
        r#"
[mob]
id = "{mob_id}"

[profiles.lead]
model = "gpt-5.5"
external_addressable = true

[profiles.lead.tools]
comms = true
"#
    ))
}

/// Same mob, one extra profile: a real composition change an operator could
/// make by editing their config between restarts.
fn diverged_definition_for(mob_id: &str) -> MobDefinition {
    definition_with(&format!(
        r#"
[mob]
id = "{mob_id}"

[profiles.lead]
model = "gpt-5.5"
external_addressable = true

[profiles.lead.tools]
comms = true

[profiles.worker]
model = "gpt-5.5"
external_addressable = true

[profiles.worker.tools]
comms = true
"#
    ))
}

fn overwrite_definition_projection(mob_path: &Path, definition: &MobDefinition, revision: u64) {
    let connection = rusqlite::Connection::open(mob_path).expect("open mob projection");
    let projected = serde_json::to_vec(&serde_json::json!({
        "definition": definition,
        "revision": revision,
    }))
    .expect("serialize projected definition");
    connection
        .execute(
            "UPDATE mob_specs SET spec_json = ?1 WHERE mob_id = ?2",
            rusqlite::params![projected, definition.id.as_str()],
        )
        .expect("overwrite projected definition");
}

fn routing_definition_for(
    mob_id: &str,
    model: &str,
    explicit_identity_override: bool,
    add_worker: bool,
) -> MobDefinition {
    let resume_overrides = if explicit_identity_override {
        "resume_overrides = [\"model\", \"provider\"]\n"
    } else {
        ""
    };
    let worker = if add_worker {
        format!(
            r#"
[profiles.worker]
model = "{model}"
external_addressable = true

[profiles.worker.tools]
comms = true
"#
        )
    } else {
        String::new()
    };
    definition_with(&format!(
        r#"
[mob]
id = "{mob_id}"

[profiles.lead]
model = "{model}"
{resume_overrides}external_addressable = true
runtime_mode = "turn_driven"

[profiles.lead.tools]
comms = true
{worker}
"#
    ))
}

fn options() -> MobBootstrapOptions {
    MobBootstrapOptions {
        allow_ephemeral_sessions: true,
        notify_orchestrator_on_resume: true,
        default_llm_client: Some(Arc::new(TestClient::default())),
    }
}

async fn boot_unified(
    mob_path: &Path,
    state_root: &Path,
    definition: MobDefinition,
) -> (UnifiedRuntime, Arc<IdentityRuntime>) {
    std::fs::create_dir_all(state_root).expect("state root");
    let (storage, provenance) =
        persistent_mob_storage(mob_path.to_path_buf()).expect("open persistent mob storage");
    let spec = MobBootstrapSpec::persistent(
        definition,
        storage,
        provenance,
        state_root.to_path_buf(),
        16,
    )
    .await
    .expect("compose persistent Meta stores")
    // Creates no agent child mob. It restarts in this process, which the
    // durable child-mob default refuses as a duplicate owner; explicit
    // ProcessBound keeps this test's cold-recovery oracle.
    .with_child_mob_storage(meerkat_meta::ChildMobStorage::ProcessBound)
    .with_options(options());
    let mut runtime = UnifiedRuntime::bootstrap(
        spec,
        MetaConfig {
            modules: Vec::new(),
            discovery: DiscoverySpec {
                namespace: "definition-epoch-routing".to_string(),
                modules: Vec::new(),
            },
            pre_spawn: Vec::new(),
        },
        Duration::from_secs(2),
    )
    .await
    .expect("bootstrap unified runtime");
    let roster = vec![DurableAgentSpec {
        identity: MetaAgentIdentity::parse("lead-1").expect("valid Meta agent identity"),
        profile: ProfileName::from("lead"),
        addressability: AgentAddressability::Addressable,
        display_name: None,
        labels: BTreeMap::new(),
        context: None,
        additional_instructions: Vec::new(),
        initial_message: None,
        runtime_mode_override: None,
        backend: None,
        binding: None,
        placement: None,
    }];
    let continuity_store = Arc::new(
        LocalContinuityStore::open(state_root.join("identity-continuity.sqlite3"))
            .expect("open identity continuity store"),
    );
    let identity_runtime = Arc::new(
        IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: continuity_store as Arc<dyn ContinuityStore>,
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "definition-epoch-routing-test".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(Arc::new(MobSessionBridge::with_session_service(
                runtime.mob_handle(),
                runtime
                    .mob_runtime()
                    .session_service()
                    .cloned()
                    .expect("persistent runtime has a session service"),
            ))),
            default_timeout: None,
        })
        .with_runtime_services(AgentRuntimeServices::new(runtime.mob_handle())),
    );
    let context = Arc::new(IdentityFirstRuntimeContext::new(
        identity_runtime.clone(),
        Arc::new(MutableRosterProvider::new(roster.clone())),
        None,
        None,
        Some(runtime.mob_handle().definition().clone()),
    ));
    let restored = runtime
        .install_and_bootstrap_identity_first_context(context, &roster)
        .await
        .expect("activate and restore identity-first runtime");
    let outcome = restored
        .outcomes
        .get(&roster[0].identity)
        .expect("lead-1 restore outcome");
    assert!(
        matches!(
            outcome,
            RestoreOutcome::Created { .. } | RestoreOutcome::Resumed { .. }
        ),
        "lead-1 must be materialized, got {outcome:?}"
    );
    (runtime, identity_runtime)
}

async fn assert_member_model(
    runtime: &UnifiedRuntime,
    identity_runtime: &IdentityRuntime,
    expected_model: &str,
    expected_provider: meerkat_core::Provider,
) {
    let status = identity_runtime
        .status(&MetaAgentIdentity::parse("lead-1").expect("valid Meta agent identity"))
        .await
        .expect("read lead-1 identity status");
    let session_id = status
        .session_id
        .expect("lead-1 must retain its bridge session");
    let session_service = runtime
        .mob_runtime()
        .session_service()
        .expect("persistent runtime has a session service");
    let persisted = session_service
        .load_persisted_session_metadata(&session_id)
        .await
        .expect("read persisted session metadata")
        .expect("persisted session exists");
    let metadata = persisted
        .session_metadata
        .expect("persisted session carries LLM identity metadata");
    assert_eq!(metadata.model, expected_model);
    assert_eq!(metadata.provider, expected_provider);
}

async fn member_session_id(identity_runtime: &IdentityRuntime) -> meerkat_core::SessionId {
    identity_runtime
        .status(&MetaAgentIdentity::parse("lead-1").expect("valid Meta agent identity"))
        .await
        .expect("read lead-1 identity status")
        .session_id
        .expect("lead-1 must retain its bridge session")
}

struct RealizedRoutingFixture {
    request_id: SwitchTurnRequestId,
    originating_run_id: RunId,
}

fn routing_intent(target_model: &str) -> SwitchTurnIntent {
    SwitchTurnIntent {
        target_model: ModelId::new(target_model),
        duration: SwitchTurnDuration::UntilChanged,
        origin: SwitchTurnOrigin::Model {
            reason: SwitchTurnReasonTextDisposition::NotProvided,
        },
    }
}

async fn install_realized_routing_fixture(
    state_root: &Path,
    session_id: &meerkat_core::SessionId,
    target_model: &str,
) -> RealizedRoutingFixture {
    use meerkat_runtime::RuntimeStore as _;

    // Sessions live in Core's pair on runtime.sqlite, where the runtime
    // store's head-canonical authority is the session's only authority: a
    // session-store save would move the physical head past it, and the next
    // boot could not resume the member. Commit the fixture as one boundary on
    // the runtime store instead, extending the head it last issued.
    let runtime_db = state_root.join("runtime.sqlite");
    let runtime_store = meerkat_runtime::store::SqliteRuntimeStore::new_head_canonical(&runtime_db)
        .expect("open the runtime store for the routing fixture");
    let session_store = meerkat_store::SqliteSessionStore::open(runtime_db)
        .expect("open the session store for the routing fixture");
    let mut session = session_store
        .load(session_id)
        .await
        .expect("load the session for the routing fixture")
        .expect("routing fixture session exists");
    let mut metadata = session
        .try_session_metadata()
        .expect("decode session metadata")
        .expect("session carries LLM metadata");
    assert_eq!(metadata.provider, meerkat_core::Provider::Anthropic);

    let request_id = SwitchTurnRequestId::new(uuid::Uuid::new_v4());
    let originating_run_id = RunId::new();
    let intent = routing_intent(target_model);
    session
        .append_model_routing_control_record(
            SessionModelRoutingControlRecord::request(
                request_id,
                originating_run_id.clone(),
                intent.clone(),
            )
            .expect("construct durable brain-swap request"),
        )
        .expect("append durable brain-swap request");
    let applied_identity = SessionLlmIdentity {
        model: target_model.to_string(),
        provider: meerkat_core::Provider::OpenAI,
        self_hosted_server_id: None,
        provider_params: None,
        auth_binding: None,
    };
    session
        .append_model_routing_control_record(
            SessionModelRoutingControlRecord::ModelRoutingIntentRealized {
                request_id,
                originating_run_id: originating_run_id.clone(),
                intent,
                applied_identity: Box::new(applied_identity.clone()),
            },
        )
        .expect("append matching realized brain-swap terminal");
    metadata.apply_llm_identity(&applied_identity);
    session
        .set_session_metadata(metadata)
        .expect("persist applied routing identity");

    let runtime_id = meerkat_runtime::LogicalRuntimeId::for_session(session_id);
    let observed = runtime_store
        .load_session_boundary_authority(&runtime_id)
        .await
        .expect("read the session's runtime authority")
        .expect("the session has a runtime authority")
        .head_canonical()
        .expect("the native pair's authority is head-canonical")
        .boundary_head()
        .clone();
    let mutation = meerkat_core::session_store::PreparedHeadCanonicalMutation::prepare(
        &session,
        Some(observed),
    )
    .expect("prepare the realized routing fixture");
    runtime_store
        .commit_prepared_session_boundary(
            &runtime_id,
            meerkat_runtime::PreparedRuntimeSessionCommit::snapshot_only(
                meerkat_core::lifecycle::core_executor::BoundSessionCommit::head_canonical_from_session(
                    &session, mutation,
                )
                .expect("bind the realized routing fixture"),
            ),
        )
        .await
        .expect("commit the realized routing fixture");
    // The next boot takes the runtime store's execution custody.
    drop(runtime_store);

    RealizedRoutingFixture {
        request_id,
        originating_run_id,
    }
}

async fn assert_realized_routing_history(
    state_root: &Path,
    session_id: &meerkat_core::SessionId,
    fixture: &RealizedRoutingFixture,
    target_model: &str,
) {
    let store = meerkat_store::SqliteSessionStore::open(state_root.join("runtime.sqlite"))
        .expect("open session store for routing assertion");
    let session = store
        .load(session_id)
        .await
        .expect("load session for routing assertion")
        .expect("routing assertion session exists");
    let records = session.model_routing_control().records();
    assert_eq!(
        records.len(),
        2,
        "routing history must remain one Requested plus one Realized"
    );
    assert!(matches!(
        &records[0],
        SessionModelRoutingControlRecord::ModelRoutingIntentRequested {
            request_id,
            originating_run_id,
            intent,
        } if request_id == &fixture.request_id
            && originating_run_id == &fixture.originating_run_id
            && intent.target_model == ModelId::new(target_model)
    ));
    assert!(matches!(
        &records[1],
        SessionModelRoutingControlRecord::ModelRoutingIntentRealized {
            request_id,
            originating_run_id,
            intent,
            applied_identity,
        } if request_id == &fixture.request_id
            && originating_run_id == &fixture.originating_run_id
            && intent.target_model == ModelId::new(target_model)
            && applied_identity.model == target_model
            && applied_identity.provider == meerkat_core::Provider::OpenAI
    ));
}

async fn commit_member_turn(identity_runtime: &IdentityRuntime, prompt: &str) {
    identity_runtime
        .send_awaiting_commit(
            &MetaAgentIdentity::parse("lead-1").expect("valid Meta agent identity"),
            &meerkat_core::ContentInput::Text(prompt.to_string()),
        )
        .await
        .expect("complete member turn");
}

async fn session_service(session_root: &Path) -> Arc<dyn meerkat_mob::MobSessionService> {
    std::fs::create_dir_all(session_root).expect("session root");
    let factory = AgentFactory::new(session_root).comms(true);
    Arc::new(build_ephemeral_service(factory, Config::default(), 16))
}

/// Resume precedence is explicit, not declaration-driven:
///
/// 1. create on Anthropic A, then install the same typed Requested+Realized
///    routing history and applied OpenAI B identity that a committed brain swap
///    owns, through the public Session and SessionStore authorities,
/// 2. prove two ordinary unmasked A cold resumes keep B and its exact history,
/// 3. advance an epoch that only adds a profile and prove B still survives, and
/// 4. explicitly mask A and prove operator authority intentionally wins without
///    rewriting the settled routing history.
#[tokio::test]
async fn durable_routing_survives_ordinary_profiles_and_profile_only_definition_epochs() {
    const MOB_ID: &str = "definition-epoch-routing";
    const MODEL_A: &str = "claude-opus-4-8";
    const MODEL_B: &str = "gpt-5.5";
    let temp = tempfile::tempdir().expect("temp dir");
    let mob_path = temp.path().join("mob.sqlite3");
    let state_root = temp.path().join("state");

    let (first, first_identity) = boot_unified(
        &mob_path,
        &state_root,
        routing_definition_for(MOB_ID, MODEL_A, false, false),
    )
    .await;
    commit_member_turn(&first_identity, "commit model A").await;
    let session_id = member_session_id(&first_identity).await;
    assert_member_model(
        &first,
        &first_identity,
        MODEL_A,
        meerkat_core::Provider::Anthropic,
    )
    .await;
    first.shutdown().await;

    let fixture = install_realized_routing_fixture(&state_root, &session_id, MODEL_B).await;
    let (second, second_identity) = boot_unified(
        &mob_path,
        &state_root,
        routing_definition_for(MOB_ID, MODEL_A, false, false),
    )
    .await;
    commit_member_turn(
        &second_identity,
        "ordinary model A profile must retain durable B",
    )
    .await;
    assert_member_model(
        &second,
        &second_identity,
        MODEL_B,
        meerkat_core::Provider::OpenAI,
    )
    .await;
    assert_realized_routing_history(&state_root, &session_id, &fixture, MODEL_B).await;
    second.shutdown().await;

    let (third, third_identity) = boot_unified(
        &mob_path,
        &state_root,
        routing_definition_for(MOB_ID, MODEL_A, false, false),
    )
    .await;
    commit_member_turn(
        &third_identity,
        "ordinary model A profile must retain durable B",
    )
    .await;
    assert_member_model(
        &third,
        &third_identity,
        MODEL_B,
        meerkat_core::Provider::OpenAI,
    )
    .await;
    assert_realized_routing_history(&state_root, &session_id, &fixture, MODEL_B).await;
    third.shutdown().await;

    let storage = MobStorage::persistent(&mob_path).expect("reopen mob storage");
    declare_spec_update(
        &storage,
        MOB_ID,
        &routing_definition_for(MOB_ID, MODEL_A, false, true),
        1,
    )
    .await
    .expect("add worker profile");
    drop(storage);
    let (fourth, fourth_identity) = boot_unified(
        &mob_path,
        &state_root,
        routing_definition_for(MOB_ID, MODEL_A, false, true),
    )
    .await;
    assert_realized_routing_history(&state_root, &session_id, &fixture, MODEL_B).await;
    commit_member_turn(&fourth_identity, "profile-only epoch must retain durable B").await;
    assert_member_model(
        &fourth,
        &fourth_identity,
        MODEL_B,
        meerkat_core::Provider::OpenAI,
    )
    .await;
    assert!(
        fourth
            .mob_handle()
            .definition()
            .profiles
            .contains_key(&ProfileName::from("worker")),
        "the profile-only epoch must be the one that resumed"
    );
    fourth.shutdown().await;

    let storage = MobStorage::persistent(&mob_path).expect("reopen mob storage");
    declare_spec_update(
        &storage,
        MOB_ID,
        &routing_definition_for(MOB_ID, MODEL_A, true, true),
        2,
    )
    .await
    .expect("declare explicit return to model A");
    drop(storage);
    let (fifth, fifth_identity) = boot_unified(
        &mob_path,
        &state_root,
        routing_definition_for(MOB_ID, MODEL_A, true, true),
    )
    .await;
    commit_member_turn(
        &fifth_identity,
        "explicit operator mask must restore model A",
    )
    .await;
    assert_member_model(
        &fifth,
        &fifth_identity,
        MODEL_A,
        meerkat_core::Provider::Anthropic,
    )
    .await;
    assert_realized_routing_history(&state_root, &session_id, &fixture, MODEL_B).await;
    fifth.shutdown().await;
}

/// Boot against a persistent mob path, declaring provenance the way the stock
/// launch paths do.
async fn boot(
    mob_path: &Path,
    session_root: &Path,
    definition: MobDefinition,
) -> Result<MobRuntime, MobRuntimeError> {
    let (storage, provenance) =
        persistent_mob_storage(mob_path.to_path_buf()).expect("open persistent mob storage");
    MobRuntime::bootstrap(
        MobBootstrapSpec::new(definition, storage, session_service(session_root).await)
            .with_mob_storage_provenance(provenance)
            .with_options(options()),
    )
    .await
}

/// The event log written by a live mob must still be on disk after the
/// runtime is shut down and dropped, and a fresh bootstrap against the same
/// path must resume it rather than create a second mob.
///
/// The observables are positive on purpose. Asserting that the second
/// bootstrap merely returns `Ok` would pass just as well against the defect
/// being fixed here, because creating a brand new empty mob also succeeds.
/// What cannot happen under the defect is a non-empty event log at the same
/// path after the process-side state is gone.
#[tokio::test]
async fn durable_mob_state_survives_drop_and_rebootstrap() {
    const MOB_ID: &str = "persistence-survives";
    let temp = tempfile::tempdir().expect("temp dir");
    let mob_path = temp.path().join("mob.sqlite3");
    let session_root = temp.path().join("sessions");

    let first = boot(&mob_path, &session_root, base_definition_for(MOB_ID))
        .await
        .expect("first bootstrap");
    let handle = first.handle();
    Box::pin(handle.spawn_spec(SpawnMemberSpec::from_wire(
        "lead".to_string(),
        "lead-1".to_string(),
        Some("You are lead-1.".into()),
        None,
        None,
    )))
    .await
    .expect("spawn lead-1");
    let before: Vec<String> = handle
        .list_members_including_retiring()
        .await
        .into_iter()
        .map(|member| member.agent_identity.as_str().to_string())
        .collect();
    assert!(
        before.contains(&"lead-1".to_string()),
        "the member must exist before the drop, else the test proves nothing: {before:?}"
    );

    // Retire before shutdown to release the member's comms endpoint. The comms
    // participant registry is process-global and `shutdown` alone does not
    // release member endpoints, so an in-process rebootstrap of the same mob
    // otherwise fails with a durable-generation-binding disagreement. A real
    // restart is a new process and never sees this.
    let identity = handle
        .list_members_including_retiring()
        .await
        .into_iter()
        .find(|member| member.agent_identity.as_str() == "lead-1")
        .map(|member| member.agent_identity)
        .expect("lead-1 identity");
    handle.retire(identity).await.expect("retire lead-1");
    handle.shutdown().await.expect("shutdown first runtime");
    drop(handle);
    drop(first);

    // The mob's own events are on disk, written by a runtime that no longer
    // exists. Under the defect this path held an in-memory storage and there
    // would be nothing here at all.
    let reopened = MobStorage::persistent(&mob_path).expect("reopen persistent storage");
    assert!(
        !reopened
            .is_event_log_empty()
            .await
            .expect("read event log emptiness"),
        "the mob event log did not survive the drop: persistent storage is not persisting"
    );
    drop(reopened);

    // And a fresh bootstrap against that same non-empty path resumes it. The
    // companion test proves this took the resume branch rather than creating a
    // second mob: a changed definition at this point is refused, which is only
    // reachable through the resume path.
    let second = boot(&mob_path, &session_root, base_definition_for(MOB_ID))
        .await
        .expect("second bootstrap must resume the existing storage");
    drop(second);
}

/// A changed definition must refuse loudly, naming the diverged field, rather
/// than boot the composition recorded at first create.
async fn boot_non_authoritative(
    mob_path: &Path,
    session_root: &Path,
    definition: MobDefinition,
) -> Result<MobRuntime, MobRuntimeError> {
    let (storage, provenance) =
        persistent_mob_storage(mob_path.to_path_buf()).expect("open persistent mob storage");
    MobRuntime::bootstrap(
        MobBootstrapSpec::new(definition, storage, session_service(session_root).await)
            .with_mob_storage_provenance(provenance)
            .with_composition_authority(
                meerkat_meta::mob_composition_manifest::CompositionAuthority::NonAuthoritative,
            )
            .with_options(options()),
    )
    .await
}

/// A downstream app's production rollback, reproduced - and the answer it actually
/// deserves.
///
/// Their deploy boots a deliberately RESTRICTED candidate first to certify it,
/// then promotes. On a fresh store the candidate boot was the first persistent
/// launch, so it CREATED the composition pin from the candidate-phase
/// composition; the promoted boot supplied the real composition and was refused
/// on every field the two modes differ in. launchd respawned the refused boot
/// 929 times.
///
/// The first fix exempted the candidate from BOTH halves of pin semantics, and
/// that was wrong in a way the refusal had been concealing. A resume cannot
/// apply a new definition - `MobBuilder::for_resume` takes only the storage - so
/// the promoted composition can NEVER take effect on a store the candidate
/// created. Letting the promoted boot succeed did not fix the pipeline; it made
/// the mob run the rehearsal composition silently while the manifest recorded
/// the promoted one.
///
/// So the store records WHICH authority created it, and the promoted boot is
/// refused with that as the reason. The pipeline is still refused, because it is
/// genuinely unapplicable, but for a cause that points at the fix: create the
/// durable store from an authoritative launch, rehearse on a separate path.
#[tokio::test]
async fn a_rehearsal_created_store_is_refused_by_name_not_silently_adopted() {
    const MOB_ID: &str = "candidate-promote";
    let temp = tempfile::tempdir().expect("temp dir");
    let mob_path = temp.path().join("mob.sqlite");
    let session_root = temp.path().join("sessions");

    // 1. The CANDIDATE boots first, with the restricted composition, on a fresh
    //    store. It must boot: it is not refused by pin semantics it does not
    //    speak for.
    let candidate =
        boot_non_authoritative(&mob_path, &session_root, diverged_definition_for(MOB_ID))
            .await
            .expect("a candidate launch must boot on a fresh store");
    // shutdown, not just drop: the comms participant registry is process-global
    // and the supervisor name derives from the mob id, so an in-process
    // rebootstrap of the SAME mob otherwise collides on the supervisor endpoint.
    //
    // Awaiting shutdown IS what frees the name, and meerkat guarantees that
    // deliberately: the actor teardown calls `MobSupervisorBridge::shutdown`,
    // which calls `retire_inproc_route` generation-exactly. Its own comment says
    // release happens there "not at `Arc` drop", because binding route lifetime
    // to the last surviving bridge reference - typically an actor task's - is
    // itself the race. So the explicit drop below is tidiness, not the mechanism.
    //
    // A real candidate/promote deploy is separate processes and never sees this.
    candidate
        .handle()
        .shutdown()
        .await
        .expect("shutdown the candidate runtime");
    drop(candidate);

    // 2. The PROMOTED boot supplies the real composition, and must be refused
    //    for the REAL reason. Ok here is the certified lie: the manifest would
    //    record the promoted composition while the event log boots the rehearsal
    //    one, and every later restart would verify clean.
    match boot(&mob_path, &session_root, base_definition_for(MOB_ID)).await {
        Err(MobRuntimeError::CompositionProvenance(
            MobCompositionProvenanceError::CreatedByRehearsal { .. },
        )) => {}
        Err(MobRuntimeError::CompositionProvenance(MobCompositionProvenanceError::Divergent {
            fields,
            ..
        })) => panic!(
            "refused for diverged fields {fields:?}, which names a cause an operator can \
             try to fix by editing the definition - but no edit makes a rehearsal-created \
             store host the promoted composition, so that message sends them to change \
             the one thing that cannot help"
        ),
        Err(other) => panic!("expected a rehearsal-origin refusal, got: {other}"),
        Ok(_) => panic!(
            "the promoted boot succeeded on a store the CANDIDATE created: the manifest \
             now certifies the promoted composition while the event log runs the \
             rehearsal one - a silent wrong composition in place of a loud refusal"
        ),
    }
}

/// `base_definition_for` with the operator's newly added tool deny on `lead`:
/// the shape of a config edit that must not silently stay out of effect.
fn deny_added_definition_for(mob_id: &str) -> MobDefinition {
    definition_with(&format!(
        r#"
[mob]
id = "{mob_id}"

[profiles.lead]
model = "gpt-5.5"
external_addressable = true

[profiles.lead.tools]
comms = true
deny = ["spawn_member", "wire_members"]
"#
    ))
}

/// A candidate boot with an explicit candidate-definition policy and a
/// storage census to report health into.
async fn boot_candidate_with(
    mob_path: &Path,
    session_root: &Path,
    definition: MobDefinition,
    policy: CandidateDefinition,
) -> Result<MobRuntime, MobRuntimeError> {
    let (storage, provenance) =
        persistent_mob_storage(mob_path.to_path_buf()).expect("open persistent mob storage");
    MobRuntime::bootstrap(
        MobBootstrapSpec::new(definition, storage, session_service(session_root).await)
            .with_mob_storage_provenance(provenance)
            .with_composition_authority(
                meerkat_meta::mob_composition_manifest::CompositionAuthority::NonAuthoritative,
            )
            .with_candidate_definition(policy)
            .with_resolved_storage(ResolvedStorageSummary::new(
                BlobDurability::DeclaredEphemeral,
                None,
            ))
            .with_options(options()),
    )
    .await
}

/// An authoritative launch creates the store and pin for `base_definition_for`.
async fn create_authoritative_store(mob_path: &Path, session_root: &Path, mob_id: &str) {
    let promoted = boot(mob_path, session_root, base_definition_for(mob_id))
        .await
        .expect("an authoritative launch creates the store and its pin");
    promoted
        .handle()
        .shutdown()
        .await
        .expect("shutdown the promoted runtime");
    drop(promoted);
}

/// The candidate's `mob_composition` health slot, when it reported one.
fn composition_slot(runtime: &MobRuntime) -> Option<StorageSlotSummary> {
    runtime
        .resolved_storage()
        .expect("the boot declared a storage census")
        .slots
        .into_iter()
        .find(|slot| slot.durability.domain() == "mob_composition")
}

/// A candidate whose config matches the stored definition boots as before,
/// with no degraded composition slot.
#[tokio::test]
async fn a_candidate_matching_the_stored_definition_boots() {
    const MOB_ID: &str = "candidate-matches";
    let temp = tempfile::tempdir().expect("temp dir");
    let mob_path = temp.path().join("mob.sqlite");
    let session_root = temp.path().join("sessions");
    create_authoritative_store(&mob_path, &session_root, MOB_ID).await;

    let candidate = boot_candidate_with(
        &mob_path,
        &session_root,
        base_definition_for(MOB_ID),
        CandidateDefinition::RequireMatch,
    )
    .await
    .expect("a candidate whose config matches the stored definition boots");
    assert!(composition_slot(&candidate).is_none());
    let _ = candidate.handle().shutdown().await;
}

/// The defect: a candidate resume boots the STORED definition, so a config
/// whose `profiles.lead.tools.deny` adds a deny the store lacks used to boot
/// silently without it. It now refuses, naming the diverged field.
#[tokio::test]
async fn a_candidate_with_a_divergent_tool_deny_is_refused_naming_the_field() {
    const MOB_ID: &str = "candidate-divergent-deny";
    let temp = tempfile::tempdir().expect("temp dir");
    let mob_path = temp.path().join("mob.sqlite");
    let session_root = temp.path().join("sessions");
    create_authoritative_store(&mob_path, &session_root, MOB_ID).await;

    match boot_candidate_with(
        &mob_path,
        &session_root,
        deny_added_definition_for(MOB_ID),
        CandidateDefinition::RequireMatch,
    )
    .await
    {
        Err(MobRuntimeError::CompositionProvenance(
            MobCompositionProvenanceError::CandidateDivergent { fields },
        )) => {
            assert!(
                fields
                    .iter()
                    .any(|field| field == "profiles.lead.tools.deny"),
                "the refusal names the diverged deny: {fields:?}"
            );
        }
        Err(other) => panic!("expected a candidate divergence refusal, got: {other}"),
        Ok(runtime) => {
            let _ = runtime.handle().shutdown().await;
            panic!(
                "the candidate booted the stored definition without the deny its config \
                 declares: the config is presented but not in effect"
            );
        }
    }
}

/// The explicit opt-in: a certification candidate that knowingly accepts the
/// stored definition boots it, and reports the diverged fields as degraded
/// health instead of hiding them.
#[tokio::test]
async fn a_candidate_acknowledging_the_stored_definition_boots_it_with_degraded_health() {
    const MOB_ID: &str = "candidate-stored-opt-in";
    let temp = tempfile::tempdir().expect("temp dir");
    let mob_path = temp.path().join("mob.sqlite");
    let session_root = temp.path().join("sessions");
    create_authoritative_store(&mob_path, &session_root, MOB_ID).await;

    let candidate = boot_candidate_with(
        &mob_path,
        &session_root,
        deny_added_definition_for(MOB_ID),
        CandidateDefinition::Stored,
    )
    .await
    .expect("an acknowledged stored definition boots");
    let slot = composition_slot(&candidate).expect("the divergence is health-visible");
    assert!(slot.degraded, "{slot:?}");
    assert!(
        slot.detail
            .as_deref()
            .is_some_and(|detail| detail.contains("profiles.lead.tools.deny")),
        "{slot:?}"
    );
    let _ = candidate.handle().shutdown().await;
}

/// The candidate arm makes the authoritative arm's released-representation
/// allowance: on a store a 0.8.28 writer created, an unchanged operator
/// config boots as a candidate, while the same config with an added
/// `tools.deny` still refuses, naming the field.
#[tokio::test]
async fn a_candidate_on_a_released_synthesized_store_is_judged_in_the_released_form() {
    const MOB_ID: &str = "candidate-legacy-synthesized";
    let temp = tempfile::tempdir().expect("temp dir");
    let mob_path = temp.path().join("mob.sqlite3");
    let session_root = temp.path().join("sessions");
    let mut supplied = base_definition_for(MOB_ID);
    supplied
        .profiles
        .get_mut(&ProfileName::from("lead"))
        .and_then(meerkat_mob::ProfileBinding::as_inline_mut)
        .expect("inline lead profile")
        .provider_params = Some(
        meerkat_core::lifecycle::run_primitive::ProviderParamsOverride {
            temperature: Some(0.2),
            ..Default::default()
        },
    );
    let mut released = supplied.clone();
    let profile = released
        .profiles
        .get_mut(&ProfileName::from("lead"))
        .and_then(meerkat_mob::ProfileBinding::as_inline_mut)
        .expect("inline lead profile");
    profile.provider = Some(meerkat_core::Provider::OpenAI);
    profile.resume_overrides = vec![
        meerkat_mob::ResumeOverrideField::Model,
        meerkat_mob::ResumeOverrideField::Provider,
        meerkat_mob::ResumeOverrideField::ProviderParams,
    ];
    let first = boot(&mob_path, &session_root, released)
        .await
        .expect("create using the released synthesized representation");
    first
        .handle()
        .shutdown()
        .await
        .expect("shutdown released runtime");
    drop(first);
    let manifest_path = manifest_path(&mob_path);
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).expect("read manifest"))
            .expect("decode manifest");
    let manifest = manifest.as_object_mut().expect("manifest object");
    manifest.insert(
        "created_by_meta".to_string(),
        serde_json::Value::String("0.8.28".to_string()),
    );
    manifest.remove("legacy_synthesized_profile_normalization");
    std::fs::write(
        manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("encode released manifest"),
    )
    .expect("write released manifest");

    let mut with_deny = supplied.clone();
    with_deny
        .profiles
        .get_mut(&ProfileName::from("lead"))
        .and_then(meerkat_mob::ProfileBinding::as_inline_mut)
        .expect("inline lead profile")
        .tools
        .deny = vec!["spawn_member".to_string()];

    let candidate = boot_candidate_with(
        &mob_path,
        &session_root,
        supplied,
        CandidateDefinition::RequireMatch,
    )
    .await
    .expect("an unchanged operator config boots as a candidate on a released store");
    assert!(composition_slot(&candidate).is_none());
    candidate
        .handle()
        .shutdown()
        .await
        .expect("shutdown the candidate runtime");
    drop(candidate);

    match boot_candidate_with(
        &mob_path,
        &session_root,
        with_deny,
        CandidateDefinition::RequireMatch,
    )
    .await
    {
        Err(MobRuntimeError::CompositionProvenance(
            MobCompositionProvenanceError::CandidateDivergent { fields },
        )) => assert!(
            fields
                .iter()
                .any(|field| field == "profiles.lead.tools.deny"),
            "{fields:?}"
        ),
        Err(other) => panic!("expected a candidate divergence refusal, got: {other}"),
        Ok(runtime) => {
            let _ = runtime.handle().shutdown().await;
            panic!("the added deny booted silently on a released store");
        }
    }
}

/// A store with an event log and NO manifest must be adopted by an
/// authoritative resume, not refused.
///
/// Two reachable ways in, both the downstream app's: a store created before the manifest
/// existed, and recovery surgery that removed the manifest deliberately.
/// Refusing would report a conflict with a claim nobody made, and would turn
/// their recovery into a second refusal.
///
/// This is the path the rehearsal tag exists to keep narrow. Without the tag,
/// a rehearsal-created store would arrive here too and be adopted.
#[tokio::test]
async fn an_authoritative_resume_adopts_a_store_with_no_manifest() {
    const MOB_ID: &str = "manifest-adoption";
    let temp = tempfile::tempdir().expect("temp dir");
    let mob_path = temp.path().join("mob.sqlite");
    let session_root = temp.path().join("sessions");

    let first = boot(&mob_path, &session_root, base_definition_for(MOB_ID))
        .await
        .expect("first bootstrap");
    first
        .handle()
        .shutdown()
        .await
        .expect("shutdown the first runtime");
    drop(first);

    // Recovery surgery: the manifest is gone, the event log is not.
    let manifest = meerkat_meta::mob_composition_manifest::manifest_path(&mob_path);
    std::fs::remove_file(&manifest).expect("remove the manifest");

    let resumed = boot(&mob_path, &session_root, base_definition_for(MOB_ID)).await;
    assert!(
        resumed.is_ok(),
        "an authoritative resume must adopt a store with no manifest rather than refuse \
         a claim nobody made: {:?}",
        resumed.err()
    );
    if let Ok(runtime) = resumed {
        let _ = runtime.handle().shutdown().await;
    }
    assert!(
        manifest.exists(),
        "adoption must leave a manifest behind, or the next restart faces the same \
         unjudgeable store"
    );
}

#[tokio::test]
async fn changed_definition_refuses_instead_of_booting_stale() {
    const MOB_ID: &str = "persistence-diverged";
    let temp = tempfile::tempdir().expect("temp dir");
    let mob_path = temp.path().join("mob.sqlite3");
    let session_root = temp.path().join("sessions");

    let first = boot(&mob_path, &session_root, base_definition_for(MOB_ID))
        .await
        .expect("first bootstrap");
    drop(first);

    let result = boot(&mob_path, &session_root, diverged_definition_for(MOB_ID)).await;
    match result {
        Err(MobRuntimeError::CompositionProvenance(MobCompositionProvenanceError::Divergent {
            fields,
            ..
        })) => {
            // The FIELD PATH, not the table it lives in. An operator who is
            // about to declare through this refusal has to be able to see what
            // moved, and "profiles diverged" on one changed pin is the message
            // that gets declared through unread.
            assert!(
                fields.iter().any(|field| field.starts_with("profiles.")),
                "the refusal must name the diverged field PATH so the operator can act: \
                 {fields:?}"
            );
            assert!(
                !fields.iter().any(|field| field == "profiles"),
                "reporting the bare table instead of the path is the regression this asserts \
                 against: {fields:?}"
            );
        }
        Err(other) => panic!("expected a composition divergence refusal, got: {other}"),
        Ok(_) => panic!(
            "a changed definition booted successfully: the resume silently used the \
             composition recorded at first create"
        ),
    }
}

/// A `lead` profile, optionally carrying keys the TOML parse warns about and
/// ignores, one in each shape that warning takes: an unknown key on the profile
/// table, and an unknown key under its `tools` table (`mob_tasks`, which configs
/// written for earlier releases carry).
fn lead_toml(mob_id: &str, with_ignored_keys: bool, peer_description: &str) -> String {
    let (profile_key, tools_key) = if with_ignored_keys {
        ("display_name = \"Incident lead\"\n", "mob_tasks = true\n")
    } else {
        ("", "")
    };
    format!(
        r#"
[mob]
id = "{mob_id}"

[profiles.lead]
model = "gpt-5.5"
external_addressable = true
peer_description = "{peer_description}"
{profile_key}
[profiles.lead.tools]
comms = true
{tools_key}"#
    )
}

/// The keys [`lead_toml`] adds, as the parse reports them.
const IGNORED_LEAD_KEYS: [&str; 2] = ["display_name", "tools.mob_tasks"];

/// Parse a mob TOML the way both gateways do (`MobDefinition::from_toml`, which
/// logs the ignored-key warning), after checking that the parse reports exactly
/// `ignored` as the keys it ignores, so each pin below exercises that warning
/// and not a key the profile accepts.
fn definition_ignoring(toml: &str, ignored: &[&str]) -> MobDefinition {
    let parsed = MobDefinition::parse_toml(toml).expect("parse mob definition");
    let reported: Vec<&str> = parsed
        .unknown_profile_keys
        .iter()
        .flat_map(|unknown| unknown.keys.iter().map(String::as_str))
        .collect();
    assert_eq!(
        reported, ignored,
        "the keys the parse warns about and ignores"
    );
    MobDefinition::from_toml(toml).expect("parse mob definition")
}

/// Create a persistent store from `created`, shut that runtime down, then boot
/// `resumed` against the same path. That takes the resume branch, which
/// verifies the composition before the mob actuates.
async fn create_then_resume(
    root: &Path,
    created: MobDefinition,
    resumed: MobDefinition,
) -> Result<MobRuntime, MobRuntimeError> {
    let mob_path = root.join("mob.sqlite3");
    let session_root = root.join("sessions");
    let first = boot(&mob_path, &session_root, created)
        .await
        .expect("the first bootstrap creates the store");
    first
        .handle()
        .shutdown()
        .await
        .expect("shutdown the creating runtime");
    drop(first);
    boot(&mob_path, &session_root, resumed).await
}

/// The parse warns that it ignores a profile key such as `tools.mob_tasks`.
/// Acting on that warning must not read as a composition change: the typed
/// parse drops the key before the definition is recorded or compared, so a
/// store created from a config with the key resumes from the config without
/// it.
#[tokio::test]
async fn dropping_an_ignored_profile_key_resumes_the_store() {
    const MOB_ID: &str = "ignored-profile-key-dropped";
    let temp = tempfile::tempdir().expect("temp dir");
    let resumed = create_then_resume(
        temp.path(),
        definition_ignoring(
            &lead_toml(MOB_ID, true, "Incident lead"),
            &IGNORED_LEAD_KEYS,
        ),
        definition_ignoring(&lead_toml(MOB_ID, false, "Incident lead"), &[]),
    )
    .await
    .unwrap_or_else(|error| {
        panic!("dropping keys the parse ignores must not be a divergence: {error}")
    });
    resumed
        .handle()
        .shutdown()
        .await
        .expect("shutdown the resumed runtime");
}

/// The reverse edit: a store created from a config without the ignored keys
/// resumes from a config that adds them.
#[tokio::test]
async fn adding_an_ignored_profile_key_resumes_the_store() {
    const MOB_ID: &str = "ignored-profile-key-added";
    let temp = tempfile::tempdir().expect("temp dir");
    let resumed = create_then_resume(
        temp.path(),
        definition_ignoring(&lead_toml(MOB_ID, false, "Incident lead"), &[]),
        definition_ignoring(
            &lead_toml(MOB_ID, true, "Incident lead"),
            &IGNORED_LEAD_KEYS,
        ),
    )
    .await
    .unwrap_or_else(|error| {
        panic!("adding keys the parse ignores must not be a divergence: {error}")
    });
    resumed
        .handle()
        .shutdown()
        .await
        .expect("shutdown the resumed runtime");
}

/// Ignored keys never mask a real change: an edit that drops them and also
/// changes an accepted key is refused, naming only the accepted key.
#[tokio::test]
async fn a_meaningful_change_beside_a_dropped_ignored_key_still_diverges() {
    const MOB_ID: &str = "ignored-profile-key-meaningful-change";
    let temp = tempfile::tempdir().expect("temp dir");
    let result = create_then_resume(
        temp.path(),
        definition_ignoring(
            &lead_toml(MOB_ID, true, "Incident lead"),
            &IGNORED_LEAD_KEYS,
        ),
        definition_ignoring(&lead_toml(MOB_ID, false, "Incident commander"), &[]),
    )
    .await;
    match result {
        Err(MobRuntimeError::CompositionProvenance(MobCompositionProvenanceError::Divergent {
            fields,
            ..
        })) => assert_eq!(
            fields,
            vec!["profiles.lead.peer_description".to_string()],
            "the refusal names the changed accepted key and never an ignored one"
        ),
        Err(other) => panic!("expected a composition divergence refusal, got: {other}"),
        Ok(_) => panic!(
            "a changed peer_description resumed: the comparison no longer sees a real \
             change made in the same edit that drops ignored keys"
        ),
    }
}

/// Insert `key = value` into profile `profile` of a serialized definition.
fn insert_profile_key(definition: &mut serde_json::Value, profile: &str, key: &str, value: &str) {
    definition
        .get_mut("profiles")
        .and_then(|profiles| profiles.get_mut(profile))
        .and_then(serde_json::Value::as_object_mut)
        .expect("the recorded definition carries the inline profile")
        .insert(
            key.to_string(),
            serde_json::Value::String(value.to_string()),
        );
}

/// meerkat 0.9 refuses a profile `system_prompt`, so an upgraded config must
/// drop it. That edit never reads as a divergence: MobKit 0.8.31 and earlier
/// dropped the key at parse, so no store they wrote records it, and this pins
/// the worst case anyway. A recorded composition that carries the key, in its
/// `MobCreated` event and its manifest, resumes from the config without it,
/// because both sides are compared as typed definitions.
#[tokio::test]
async fn a_recorded_profile_system_prompt_does_not_diverge_the_config_without_it() {
    const MOB_ID: &str = "recorded-profile-system-prompt";
    let temp = tempfile::tempdir().expect("temp dir");
    let mob_path = temp.path().join("mob.sqlite3");
    let session_root = temp.path().join("sessions");
    let config = lead_toml(MOB_ID, false, "Incident lead");

    let first = boot(&mob_path, &session_root, definition_ignoring(&config, &[]))
        .await
        .expect("the first bootstrap creates the store");
    first
        .handle()
        .shutdown()
        .await
        .expect("shutdown the creating runtime");
    drop(first);

    let connection = rusqlite::Connection::open(&mob_path).expect("open the mob event log");
    let mut created = Vec::new();
    {
        let mut rows = connection
            .prepare("SELECT cursor, event_json FROM mob_events ORDER BY cursor")
            .expect("prepare the event read");
        let events = rows
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .expect("read the events");
        for event in events {
            let (cursor, bytes) = event.expect("read one event");
            // A stored row is `{"schema_version": .., "event": <MobEvent>}`.
            let mut stored: serde_json::Value =
                serde_json::from_slice(&bytes).expect("decode one event");
            if stored["event"]["kind"]["type"] == "mob_created" {
                insert_profile_key(
                    &mut stored["event"]["kind"]["definition"],
                    "lead",
                    "system_prompt",
                    "You lead the incident.",
                );
                created.push((
                    cursor,
                    serde_json::to_vec(&stored).expect("encode the event"),
                ));
            }
        }
    }
    assert_eq!(created.len(), 1, "the log records one MobCreated event");
    for (cursor, bytes) in created {
        let rewritten = connection
            .execute(
                "UPDATE mob_events SET event_json = ?1 WHERE cursor = ?2",
                rusqlite::params![bytes, cursor],
            )
            .expect("record the key in the MobCreated definition");
        assert_eq!(rewritten, 1, "the MobCreated row now records the key");
    }
    drop(connection);

    let manifest = manifest_path(&mob_path);
    let mut recorded: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest).expect("read the manifest"))
            .expect("decode the manifest");
    insert_profile_key(
        &mut recorded["definition"],
        "lead",
        "system_prompt",
        "You lead the incident.",
    );
    std::fs::write(
        &manifest,
        serde_json::to_vec_pretty(&recorded).expect("encode the manifest"),
    )
    .expect("record the key in the manifest");

    let resumed = boot(&mob_path, &session_root, definition_ignoring(&config, &[]))
        .await
        .unwrap_or_else(|error| {
            panic!("a recorded system_prompt must not make the config without it diverge: {error}")
        });
    resumed
        .handle()
        .shutdown()
        .await
        .expect("shutdown the resumed runtime");
}

#[tokio::test]
async fn released_synthesized_profile_definition_resumes_unchanged_operator_config() {
    const MOB_ID: &str = "legacy-synthesized-profile-resume";
    let temp = tempfile::tempdir().expect("temp dir");
    let mob_path = temp.path().join("mob.sqlite3");
    let session_root = temp.path().join("sessions");
    let mut supplied = base_definition_for(MOB_ID);
    supplied
        .profiles
        .get_mut(&ProfileName::from("lead"))
        .and_then(meerkat_mob::ProfileBinding::as_inline_mut)
        .expect("inline lead profile")
        .provider_params = Some(
        meerkat_core::lifecycle::run_primitive::ProviderParamsOverride {
            temperature: Some(0.2),
            ..Default::default()
        },
    );
    let mut released = supplied.clone();
    let profile = released
        .profiles
        .get_mut(&ProfileName::from("lead"))
        .and_then(meerkat_mob::ProfileBinding::as_inline_mut)
        .expect("inline lead profile");
    profile.provider = Some(meerkat_core::Provider::OpenAI);
    profile.resume_overrides = vec![
        meerkat_mob::ResumeOverrideField::Model,
        meerkat_mob::ResumeOverrideField::Provider,
        meerkat_mob::ResumeOverrideField::ProviderParams,
    ];

    let first = boot(&mob_path, &session_root, released.clone())
        .await
        .expect("create using the released synthesized representation");
    first
        .handle()
        .shutdown()
        .await
        .expect("shutdown released runtime");
    drop(first);
    let manifest_path = manifest_path(&mob_path);
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).expect("read manifest"))
            .expect("decode manifest");
    let manifest = manifest.as_object_mut().expect("manifest object");
    manifest.insert(
        "created_by_meta".to_string(),
        serde_json::Value::String("0.8.28".to_string()),
    );
    manifest.remove("legacy_synthesized_profile_normalization");
    std::fs::write(
        manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("encode released manifest"),
    )
    .expect("remove field absent from released manifests");

    let resumed = boot(&mob_path, &session_root, supplied)
        .await
        .expect("unchanged operator config must resume a pre-0.8.29 durable mob");
    assert_eq!(
        resumed.handle().definition(),
        &released,
        "resume must still build the exact canonical event-log definition"
    );
    resumed
        .handle()
        .shutdown()
        .await
        .expect("shutdown resumed runtime");
}

#[tokio::test]
async fn manifestless_released_definition_refuses_without_vintage_proof() {
    const MOB_ID: &str = "legacy-manifestless-profile-resume";
    let temp = tempfile::tempdir().expect("temp dir");
    let mob_path = temp.path().join("mob.sqlite3");
    let session_root = temp.path().join("sessions");
    let supplied = base_definition_for(MOB_ID);
    let mut released = supplied.clone();
    let profile = released
        .profiles
        .get_mut(&ProfileName::from("lead"))
        .and_then(meerkat_mob::ProfileBinding::as_inline_mut)
        .expect("inline lead profile");
    profile.provider = Some(meerkat_core::Provider::OpenAI);
    profile.resume_overrides = vec![
        meerkat_mob::ResumeOverrideField::Model,
        meerkat_mob::ResumeOverrideField::Provider,
    ];

    let first = boot(&mob_path, &session_root, released.clone())
        .await
        .expect("create released runtime");
    first
        .handle()
        .shutdown()
        .await
        .expect("shutdown released runtime");
    drop(first);
    std::fs::remove_file(manifest_path(&mob_path)).expect("remove manifest to model old storage");

    assert!(matches!(
        boot(&mob_path, &session_root, supplied).await,
        Err(MobRuntimeError::CompositionProvenance(
            MobCompositionProvenanceError::Divergent { .. }
        ))
    ));
    assert!(
        !manifest_path(&mob_path).exists(),
        "an unproven legacy representation must not mint provenance"
    );
}

#[tokio::test]
async fn current_writer_does_not_treat_explicit_unmask_as_legacy_equivalence() {
    const MOB_ID: &str = "current-explicit-unmask";
    let temp = tempfile::tempdir().expect("temp dir");
    let mob_path = temp.path().join("mob.sqlite3");
    let session_root = temp.path().join("sessions");
    let supplied = base_definition_for(MOB_ID);
    let mut explicitly_masked = supplied.clone();
    let profile = explicitly_masked
        .profiles
        .get_mut(&ProfileName::from("lead"))
        .and_then(meerkat_mob::ProfileBinding::as_inline_mut)
        .expect("inline lead profile");
    profile.provider = Some(meerkat_core::Provider::OpenAI);
    profile.resume_overrides = vec![
        meerkat_mob::ResumeOverrideField::Model,
        meerkat_mob::ResumeOverrideField::Provider,
    ];

    let first = boot(&mob_path, &session_root, explicitly_masked)
        .await
        .expect("create explicitly masked runtime");
    first
        .handle()
        .shutdown()
        .await
        .expect("shutdown explicitly masked runtime");
    drop(first);

    let manifest_path = manifest_path(&mob_path);
    let manifest: MobCompositionManifest =
        serde_json::from_slice(&std::fs::read(&manifest_path).expect("read current manifest"))
            .expect("decode current manifest");
    assert_eq!(
        manifest.legacy_synthesized_profile_normalization,
        Some(false),
        "current writers must explicitly exclude their stores from legacy fallback"
    );

    assert!(matches!(
        boot(&mob_path, &session_root, supplied).await,
        Err(MobRuntimeError::CompositionProvenance(
            MobCompositionProvenanceError::Divergent { .. }
        ))
    ));
}

#[tokio::test]
async fn declared_definition_epoch_adds_profile_and_survives_runtime_restart() {
    const MOB_ID: &str = "definition-epoch-restart";
    let temp = tempfile::tempdir().expect("temp dir");
    let mob_path = temp.path().join("mob.sqlite3");
    let session_root = temp.path().join("sessions");
    let initial = base_definition_for(MOB_ID);
    let updated = diverged_definition_for(MOB_ID);
    let mut canonical_updated = updated.clone();
    auto_mark_declared_resume_overrides(&mut canonical_updated);

    let first = boot(&mob_path, &session_root, initial.clone())
        .await
        .expect("create initial runtime");
    first
        .handle()
        .shutdown()
        .await
        .expect("shutdown initial runtime");
    drop(first);

    let (storage, _) =
        persistent_mob_storage(mob_path.clone()).expect("reopen persistent mob storage");
    let receipt = declare_spec_update(&storage, MOB_ID, &updated, 1)
        .await
        .expect("advance canonical definition epoch");
    assert_eq!(receipt.previous_revision, 1);
    assert_eq!(receipt.committed_revision, 2);
    assert_eq!(receipt.projection_revision, 2);
    drop(storage);

    let resumed = boot(&mob_path, &session_root, updated.clone())
        .await
        .expect("resume updated canonical definition");
    assert!(
        resumed
            .handle()
            .definition()
            .profiles
            .contains_key(&ProfileName::from("worker")),
        "the added profile must survive a real Meta runtime drop and resume"
    );
    let events = resumed
        .handle()
        .events()
        .replay_all()
        .await
        .expect("replay canonical mob events");
    let update_events = events
        .iter()
        .filter(|event| matches!(event.kind, MobEventKind::MobDefinitionUpdated { .. }))
        .collect::<Vec<_>>();
    let created_events = events
        .iter()
        .filter(|event| matches!(event.kind, MobEventKind::MobCreated { .. }))
        .collect::<Vec<_>>();
    assert_eq!(
        created_events.len(),
        1,
        "epoch 1 must remain the singular canonical MobCreated event"
    );
    assert!(matches!(
        &created_events[0].kind,
        MobEventKind::MobCreated { definition } if definition.as_ref() == &initial
    ));
    assert_eq!(
        update_events.len(),
        1,
        "the ceremony must append exactly one canonical definition update"
    );
    assert_eq!(update_events[0].cursor, receipt.event_cursor);
    assert_eq!(update_events[0].cursor, created_events[0].cursor + 1);
    assert!(matches!(
        &update_events[0].kind,
        MobEventKind::MobDefinitionUpdated { epoch: 2, definition }
            if definition.as_ref() == &canonical_updated
    ));
    assert!(
        events
            .iter()
            .all(|event| !matches!(event.kind, MobEventKind::MobCompleted)),
        "a definition update must not fabricate lifecycle completion"
    );
    resumed
        .handle()
        .shutdown()
        .await
        .expect("shutdown resumed runtime");
    drop(resumed);

    let storage = MobStorage::persistent(&mob_path).expect("inspect persistent mob storage");
    assert_eq!(
        storage
            .created_definition()
            .await
            .expect("read canonical definition"),
        Some(canonical_updated)
    );
    assert!(matches!(
        storage
            .definition_projection_health()
            .await
            .expect("read definition projection health"),
        Some(MobDefinitionProjectionHealth::Healthy {
            authority_epoch: 2,
            projection_revision: 2,
        })
    ));
}

#[tokio::test]
async fn released_projection_only_profile_update_is_atomically_repaired_and_resumes() {
    const MOB_ID: &str = "definition-epoch-released-residue";
    let temp = tempfile::tempdir().expect("temp dir");
    let mob_path = temp.path().join("mob.sqlite3");
    let session_root = temp.path().join("sessions");
    let initial = base_definition_for(MOB_ID);
    let updated = diverged_definition_for(MOB_ID);
    let mut canonical_updated = updated.clone();
    auto_mark_declared_resume_overrides(&mut canonical_updated);

    let first = boot(&mob_path, &session_root, initial)
        .await
        .expect("create initial runtime");
    first
        .handle()
        .shutdown()
        .await
        .expect("shutdown initial runtime");
    drop(first);

    overwrite_definition_projection(&mob_path, &canonical_updated, 2);
    let storage = MobStorage::persistent(&mob_path).expect("reopen released mob state");
    assert!(matches!(
        storage
            .definition_projection_health()
            .await
            .expect("read released projection health"),
        Some(MobDefinitionProjectionHealth::Diverged {
            authority_epoch: 1,
            projection_revision: 2,
            kind: MobDefinitionProjectionMismatchKind::ProjectionAhead,
        })
    ));

    let wrong_revision = declare_spec_update(&storage, MOB_ID, &updated, 2)
        .await
        .expect_err("only the released expected-revision witness may repair residue");
    assert!(matches!(
        wrong_revision,
        SpecUpdateError::DefinitionProjectionDisagreement {
            authority_epoch: 1,
            projection_revision: 2,
            kind: MobDefinitionProjectionMismatchKind::ProjectionAhead,
            ..
        }
    ));

    let receipt = declare_spec_update(&storage, MOB_ID, &updated, 1)
        .await
        .expect("repair released projection-only residue");
    assert_eq!(receipt.previous_revision, 1);
    assert_eq!(receipt.committed_revision, 2);
    assert_eq!(receipt.projection_revision, 2);
    assert!(matches!(
        storage
            .definition_projection_health()
            .await
            .expect("read repaired projection health"),
        Some(MobDefinitionProjectionHealth::Healthy {
            authority_epoch: 2,
            projection_revision: 2,
        })
    ));
    drop(storage);

    let resumed = boot(&mob_path, &session_root, updated)
        .await
        .expect("resume repaired released definition");
    assert!(
        resumed
            .handle()
            .definition()
            .profiles
            .contains_key(&ProfileName::from("worker")),
        "the profile persisted by the released updater must survive repair and resume"
    );
    let events = resumed
        .handle()
        .events()
        .replay_all()
        .await
        .expect("replay repaired canonical events");
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                &event.kind,
                MobEventKind::MobDefinitionUpdated {
                    epoch: 2,
                    definition,
                } if definition.as_ref() == &canonical_updated
            ))
            .count(),
        1,
        "repair must append exactly one canonical epoch-2 event"
    );
    assert!(
        events
            .iter()
            .all(|event| !matches!(event.kind, MobEventKind::MobCompleted)),
        "repair must not fabricate lifecycle completion"
    );
    resumed
        .handle()
        .shutdown()
        .await
        .expect("shutdown repaired runtime");
}

#[tokio::test]
async fn non_released_projection_ahead_shape_remains_refused() {
    const MOB_ID: &str = "definition-epoch-non-released-residue";
    let temp = tempfile::tempdir().expect("temp dir");
    let mob_path = temp.path().join("mob.sqlite3");
    let session_root = temp.path().join("sessions");
    let initial = base_definition_for(MOB_ID);
    let updated = diverged_definition_for(MOB_ID);
    let mut canonical_updated = updated.clone();
    auto_mark_declared_resume_overrides(&mut canonical_updated);

    let first = boot(&mob_path, &session_root, initial.clone())
        .await
        .expect("create initial runtime");
    first
        .handle()
        .shutdown()
        .await
        .expect("shutdown initial runtime");
    drop(first);

    overwrite_definition_projection(&mob_path, &canonical_updated, 3);
    let storage = MobStorage::persistent(&mob_path).expect("reopen divergent mob state");
    let error = declare_spec_update(&storage, MOB_ID, &updated, 1)
        .await
        .expect_err("projection-ahead shapes outside exact released residue must refuse");
    assert!(matches!(
        error,
        SpecUpdateError::DefinitionProjectionDisagreement {
            authority_epoch: 1,
            projection_revision: 3,
            kind: MobDefinitionProjectionMismatchKind::ProjectionAhead,
            ..
        }
    ));
    assert_eq!(
        storage
            .created_definition()
            .await
            .expect("read canonical definition"),
        Some(initial)
    );
    drop(storage);
    let connection = rusqlite::Connection::open(&mob_path).expect("inspect canonical events");
    let event_count: u64 = connection
        .query_row("SELECT COUNT(*) FROM mob_events", [], |row| row.get(0))
        .expect("count canonical events");
    assert_eq!(event_count, 1, "refusal must not mint authority");
}

async fn assert_sqlite_event_custom_spec_residue_refuses(
    mob_id: &str,
    custom_projection_matches_declared: bool,
) {
    let temp = tempfile::tempdir().expect("temp dir");
    let mob_path = temp.path().join("mob.sqlite3");
    let session_root = temp.path().join("sessions");
    let initial = base_definition_for(mob_id);
    let updated = diverged_definition_for(mob_id);
    let mut canonical_updated = updated.clone();
    auto_mark_declared_resume_overrides(&mut canonical_updated);

    let first = boot(&mob_path, &session_root, initial.clone())
        .await
        .expect("create paired SQLite mob");
    first
        .handle()
        .shutdown()
        .await
        .expect("shutdown paired SQLite mob");
    drop(first);
    overwrite_definition_projection(&mob_path, &canonical_updated, 2);

    let sqlite = SqliteMobStores::open(&mob_path).expect("open split SQLite event authority");
    let events = Arc::new(sqlite.event_store());
    let specs = Arc::new(InMemoryMobSpecStore::new());
    let mut custom_projection = canonical_updated.clone();
    if !custom_projection_matches_declared {
        let lead = custom_projection
            .profiles
            .get(&ProfileName::from("lead"))
            .expect("lead profile")
            .clone();
        custom_projection
            .profiles
            .insert(ProfileName::from("reviewer"), lead);
    }
    assert_eq!(
        specs
            .put_spec(&initial.id, &initial, None)
            .await
            .expect("seed custom spec projection"),
        1
    );
    assert_eq!(
        specs
            .put_spec(&custom_projection.id, &custom_projection, Some(1))
            .await
            .expect("model custom projection-only update"),
        2
    );
    let custom_projection_before = specs
        .get_spec(&custom_projection.id)
        .await
        .expect("read custom spec projection")
        .expect("custom spec projection exists");
    let sqlite_projection_before = sqlite
        .spec_store()
        .get_spec(&canonical_updated.id)
        .await
        .expect("read internal SQLite projection")
        .expect("internal SQLite projection exists");
    let event_cursor_before = events.latest_cursor().await.expect("read event cursor");
    let event_count_before = events
        .replay_all()
        .await
        .expect("read canonical events")
        .len();
    let storage = MobStorage::custom(
        events.clone(),
        Arc::new(sqlite.run_store()),
        specs.clone(),
        Arc::new(sqlite.identity_store()),
        Arc::new(sqlite.identity_status_store()),
    );

    let error = declare_spec_update(&storage, mob_id, &updated, 1)
        .await
        .expect_err("SQLite-event/custom-spec bundles cannot repair projection residue");
    assert!(matches!(
        error,
        SpecUpdateError::DefinitionProjectionDisagreement {
            authority_epoch: 1,
            projection_revision: 2,
            kind: MobDefinitionProjectionMismatchKind::ProjectionAhead,
            ..
        }
    ));
    assert_eq!(
        events.latest_cursor().await.expect("reread event cursor"),
        event_cursor_before,
        "split-store refusal must not advance the canonical event cursor"
    );
    assert_eq!(
        events
            .replay_all()
            .await
            .expect("reread canonical events")
            .len(),
        event_count_before,
        "split-store refusal must not append a canonical event"
    );
    assert_eq!(
        specs
            .get_spec(&custom_projection.id)
            .await
            .expect("reread custom spec projection")
            .expect("custom spec projection remains"),
        custom_projection_before,
        "split-store refusal must not move the custom projection"
    );
    assert_eq!(
        sqlite
            .spec_store()
            .get_spec(&canonical_updated.id)
            .await
            .expect("reread internal SQLite projection")
            .expect("internal SQLite projection remains"),
        sqlite_projection_before,
        "split-store refusal must not move the internal SQLite projection"
    );
    assert_eq!(
        storage
            .created_definition()
            .await
            .expect("read canonical definition"),
        Some(initial),
        "split-store refusal must not advance canonical authority"
    );
}

#[tokio::test]
async fn exact_sqlite_event_custom_spec_residue_remains_fail_closed() {
    assert_sqlite_event_custom_spec_residue_refuses(
        "definition-epoch-split-store-exact-residue",
        true,
    )
    .await;
}

#[tokio::test]
async fn mismatched_sqlite_event_custom_spec_residue_remains_fail_closed() {
    assert_sqlite_event_custom_spec_residue_refuses(
        "definition-epoch-split-store-mismatched-residue",
        false,
    )
    .await;
}

#[tokio::test]
async fn stale_definition_update_refuses_and_exact_retry_converges() {
    const MOB_ID: &str = "definition-epoch-cas";
    let temp = tempfile::tempdir().expect("temp dir");
    let mob_path = temp.path().join("mob.sqlite3");
    let session_root = temp.path().join("sessions");
    let initial = base_definition_for(MOB_ID);
    let updated = diverged_definition_for(MOB_ID);

    let first = boot(&mob_path, &session_root, initial)
        .await
        .expect("create initial runtime");
    first
        .handle()
        .shutdown()
        .await
        .expect("shutdown initial runtime");
    drop(first);

    let storage = MobStorage::persistent(&mob_path).expect("reopen persistent mob storage");
    let committed = declare_spec_update(&storage, MOB_ID, &updated, 1)
        .await
        .expect("first update");
    let retry = declare_spec_update(&storage, MOB_ID, &updated, 1)
        .await
        .expect("exact retry must converge");
    assert_eq!(retry.committed_revision, 2);
    assert_eq!(retry.projection_revision, 2);
    assert_eq!(retry.event_cursor, committed.event_cursor);

    let mut stale_successor = updated.clone();
    let lead = stale_successor
        .profiles
        .get(&ProfileName::from("lead"))
        .expect("lead profile")
        .clone();
    stale_successor
        .profiles
        .insert(ProfileName::from("reviewer"), lead);
    let error = declare_spec_update(&storage, MOB_ID, &stale_successor, 1)
        .await
        .expect_err("stale concurrent update must refuse");
    assert!(matches!(
        error,
        SpecUpdateError::RevisionMoved {
            proposed_at: 1,
            found: 2,
            ..
        }
    ));
    drop(storage);

    let resumed = boot(&mob_path, &session_root, updated)
        .await
        .expect("resume exact retry result");
    let events = resumed
        .handle()
        .events()
        .replay_all()
        .await
        .expect("replay canonical mob events");
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event.kind,
                MobEventKind::MobDefinitionUpdated { epoch: 2, .. }
            ))
            .count(),
        1,
        "an exact retry must not append a second canonical event"
    );
    resumed
        .handle()
        .shutdown()
        .await
        .expect("shutdown resumed runtime");
}

#[tokio::test]
async fn concurrent_definition_epoch_refuses_a_stale_verified_resume() {
    const MOB_ID: &str = "definition-epoch-resume-race";
    let temp = tempfile::tempdir().expect("temp dir");
    let mob_path = temp.path().join("mob.sqlite3");
    let session_root = temp.path().join("sessions");
    let initial = base_definition_for(MOB_ID);
    let updated = diverged_definition_for(MOB_ID);

    let first = boot(&mob_path, &session_root, initial)
        .await
        .expect("create initial runtime");
    first
        .handle()
        .shutdown()
        .await
        .expect("shutdown initial runtime");
    drop(first);

    let storage = MobStorage::persistent(&mob_path).expect("reopen persistent mob storage");
    let stale_snapshot = storage
        .created_definition_snapshot()
        .await
        .expect("read canonical definition snapshot")
        .expect("canonical definition exists");
    assert_eq!(stale_snapshot.epoch(), 1);
    let stale_event_cursor = stale_snapshot.event_cursor();
    let receipt = declare_spec_update(&storage, MOB_ID, &updated, 1)
        .await
        .expect("advance definition after preflight");

    let result = MobBuilder::for_resume_verified(storage.clone(), stale_snapshot)
        .with_session_service(session_service(&session_root).await)
        .allow_ephemeral_sessions(true)
        .resume()
        .await;
    let error = match result {
        Err(error) => error,
        Ok(handle) => {
            handle
                .shutdown()
                .await
                .expect("stop unexpectedly resumed mob");
            panic!("a resume bound to the stale definition snapshot actuated")
        }
    };
    assert!(matches!(
        error,
        meerkat_mob::MobError::MobDefinitionAuthorityChanged {
            expected_epoch: 1,
            expected_event_cursor,
            actual_epoch: Some(2),
            actual_event_cursor: Some(actual_cursor),
            ..
        } if expected_event_cursor == stale_event_cursor
            && actual_cursor == receipt.event_cursor
    ));

    drop(storage);
    let connection = rusqlite::Connection::open(&mob_path).expect("inspect canonical events");
    let (event_count, latest_cursor): (u64, u64) = connection
        .query_row("SELECT COUNT(*), MAX(cursor) FROM mob_events", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .expect("count canonical events after stale refusal");
    assert_eq!(
        latest_cursor, receipt.event_cursor,
        "stale verified resume must append no event"
    );
    assert_eq!(
        event_count, 2,
        "only MobCreated and the declared MobDefinitionUpdated may exist"
    );
}

#[tokio::test]
async fn injected_epoch_failure_leaves_no_resume_visible_split_authority() {
    const MOB_ID: &str = "definition-epoch-atomic";
    let temp = tempfile::tempdir().expect("temp dir");
    let mob_path = temp.path().join("mob.sqlite3");
    let session_root = temp.path().join("sessions");
    let initial = base_definition_for(MOB_ID);
    let updated = diverged_definition_for(MOB_ID);

    let first = boot(&mob_path, &session_root, initial.clone())
        .await
        .expect("create initial runtime");
    first
        .handle()
        .shutdown()
        .await
        .expect("shutdown initial runtime");
    drop(first);

    let storage = MobStorage::persistent(&mob_path).expect("reopen persistent mob storage");
    let connection = rusqlite::Connection::open(&mob_path).expect("open fault injector");
    connection
        .execute_batch(
            "CREATE TRIGGER fail_definition_projection
                     BEFORE UPDATE OF spec_json ON mob_specs
                     BEGIN
                       SELECT RAISE(ABORT, 'forced projection failure');
                     END;",
        )
        .expect("install projection failure");
    let error = declare_spec_update(&storage, MOB_ID, &updated, 1)
        .await
        .expect_err("injected projection failure must fail the update");
    assert!(matches!(error, SpecUpdateError::UpdateFailed { .. }));
    connection
        .execute_batch("DROP TRIGGER fail_definition_projection;")
        .expect("remove projection failure");
    drop(connection);
    let stored = storage
        .created_definition()
        .await
        .expect("read canonical definition")
        .expect("canonical definition");
    assert_eq!(stored.id, initial.id);
    assert!(!stored.profiles.contains_key(&ProfileName::from("worker")));
    assert!(matches!(
        storage
            .definition_projection_health()
            .await
            .expect("read definition projection health"),
        Some(MobDefinitionProjectionHealth::Healthy {
            authority_epoch: 1,
            projection_revision: 1,
        })
    ));
    drop(storage);

    let resumed = boot(&mob_path, &session_root, initial)
        .await
        .expect("the pre-update definition remains resume-valid");
    assert!(
        !resumed
            .handle()
            .definition()
            .profiles
            .contains_key(&ProfileName::from("worker"))
    );
    resumed
        .handle()
        .shutdown()
        .await
        .expect("shutdown resumed runtime");
}

#[tokio::test]
async fn projection_content_disagreement_is_a_typed_refusal() {
    const MOB_ID: &str = "definition-epoch-disagreement";
    let temp = tempfile::tempdir().expect("temp dir");
    let mob_path = temp.path().join("mob.sqlite3");
    let session_root = temp.path().join("sessions");
    let initial = base_definition_for(MOB_ID);
    let updated = diverged_definition_for(MOB_ID);

    let first = boot(&mob_path, &session_root, initial)
        .await
        .expect("create initial runtime");
    first
        .handle()
        .shutdown()
        .await
        .expect("shutdown initial runtime");
    drop(first);

    let connection = rusqlite::Connection::open(&mob_path).expect("open disagreement injector");
    let mut projected = updated.clone();
    auto_mark_declared_resume_overrides(&mut projected);
    connection
        .execute(
            "UPDATE mob_specs SET spec_json = ?1 WHERE mob_id = ?2",
            (
                serde_json::to_vec(&serde_json::json!({
                    "definition": projected,
                    "revision": 1,
                }))
                .expect("serialize stored spec"),
                MOB_ID,
            ),
        )
        .expect("inject projection disagreement");
    drop(connection);

    let storage = MobStorage::persistent(&mob_path).expect("reopen persistent mob storage");
    let error = declare_spec_update(&storage, MOB_ID, &updated, 1)
        .await
        .expect_err("content disagreement must refuse before mutation");
    assert!(matches!(
        error,
        SpecUpdateError::DefinitionProjectionDisagreement {
            authority_epoch: 1,
            projection_revision: 1,
            kind: MobDefinitionProjectionMismatchKind::DefinitionMismatch,
            ..
        }
    ));
}
/// Durable storage supplied through the PUBLIC bootstrap API with a non-empty
/// event log and no provenance declaration must fail closed, not resume
/// unverified.
///
/// This is the external-embedder case. `MobBootstrapSpec::new` accepts
/// arbitrary `MobStorage`, so the default provenance cannot be an ephemeral
/// claim: that would assert a fact the constructor cannot know and let a
/// caller's durable database resume with no composition verification. Scanning
/// Meta's own callers does not close this - the crate is a library.
#[tokio::test]
async fn nonempty_storage_without_provenance_fails_closed() {
    const MOB_ID: &str = "persistence-unproven";
    let temp = tempfile::tempdir().expect("temp dir");
    let mob_path = temp.path().join("mob.sqlite3");
    let session_root = temp.path().join("sessions");

    let first = boot(&mob_path, &session_root, base_definition_for(MOB_ID))
        .await
        .expect("first bootstrap");
    drop(first);

    // Same path, opened directly, provenance never declared.
    let undeclared = MobStorage::persistent(&mob_path).expect("reopen persistent storage");
    let result = MobRuntime::bootstrap(
        MobBootstrapSpec::new(
            base_definition_for(MOB_ID),
            undeclared,
            session_service(&session_root).await,
        )
        .with_options(options()),
    )
    .await;
    match result {
        Err(MobRuntimeError::CompositionProvenance(
            MobCompositionProvenanceError::UnprovenStorage,
        )) => {}
        Err(other) => panic!("expected an unproven-storage refusal, got: {other}"),
        Ok(_) => panic!("undeclared persistent storage resumed with no verification"),
    }
}

/// An ephemeral launch keeps working unchanged: in-memory storage is empty at
/// bootstrap, so it takes the create path and needs no provenance.
#[tokio::test]
async fn declared_ephemeral_storage_still_boots() {
    const MOB_ID: &str = "persistence-ephemeral";
    let temp = tempfile::tempdir().expect("temp dir");
    let session_root = temp.path().join("sessions");
    let runtime = MobRuntime::bootstrap(
        MobBootstrapSpec::new(
            base_definition_for(MOB_ID),
            MobStorage::in_memory(),
            session_service(&session_root).await,
        )
        .with_mob_storage_provenance(MobStorageProvenance::declared_ephemeral())
        .with_options(options()),
    )
    .await
    .expect("ephemeral bootstrap must still work");
    assert!(
        runtime
            .handle()
            .list_members_including_retiring()
            .await
            .is_empty()
    );
}

/// A pre-seeded in-memory storage may resume when the caller declares what it
/// is. The refusal above targets silence, not in-process reuse: a storage built
/// in this same process has no composition outliving it to disagree with.
#[tokio::test]
async fn declared_ephemeral_storage_may_resume_in_process() {
    const MOB_ID: &str = "persistence-inproc-resume";
    let temp = tempfile::tempdir().expect("temp dir");
    let session_root = temp.path().join("sessions");

    let storage = MobStorage::in_memory();
    let first = MobRuntime::bootstrap(
        MobBootstrapSpec::new(
            base_definition_for(MOB_ID),
            storage.clone(),
            session_service(&session_root).await,
        )
        .with_declared_ephemeral_mob_storage()
        .with_options(options()),
    )
    .await
    .expect("first ephemeral bootstrap");
    first
        .handle()
        .shutdown()
        .await
        .expect("shutdown first runtime");
    drop(first);

    assert!(
        !storage
            .is_event_log_empty()
            .await
            .expect("read event log emptiness"),
        "the in-memory log must be non-empty for this test to exercise the resume path"
    );

    // Same storage, now non-empty, declared for what it is: permitted.
    let second = MobRuntime::bootstrap(
        MobBootstrapSpec::new(
            base_definition_for(MOB_ID),
            storage,
            session_service(&session_root).await,
        )
        .with_declared_ephemeral_mob_storage()
        .with_options(options()),
    )
    .await
    .expect("a declared in-process storage must be allowed to resume");
    drop(second);
}

async fn boot_raw_activation_fixture(root: &Path, mob_id: &str) -> UnifiedRuntime {
    std::fs::create_dir_all(root).expect("activation fixture root");
    let definition = definition_with(&format!(
        r#"
[mob]
id = "{mob_id}"

[[wiring.role_wiring]]
a = "lead"
b = "worker"

[profiles.lead]
model = "gpt-5.5"
runtime_mode = "turn_driven"

[profiles.lead.tools]
comms = true

[profiles.worker]
model = "gpt-5.5"
runtime_mode = "turn_driven"

[profiles.worker.tools]
comms = true
"#
    ));
    let (storage, provenance) =
        persistent_mob_storage(root.join("mob.sqlite3")).expect("persistent ledger");
    let spec = MobBootstrapSpec::persistent(definition, storage, provenance, root.to_path_buf(), 8)
        .await
        .expect("persistent fixture with the agent-tool identity slot")
        // Creates no agent child mob. Explicit ProcessBound keeps its in-process
        // restart and complete-cleanup oracle, so this fixture does NOT test the
        // default-persistent shutdown (child-root retention until process exit).
        .with_child_mob_storage(meerkat_meta::ChildMobStorage::ProcessBound)
        .with_options(options());
    UnifiedRuntime::bootstrap(
        spec,
        MetaConfig {
            modules: Vec::new(),
            discovery: DiscoverySpec {
                namespace: mob_id.to_string(),
                modules: Vec::new(),
            },
            pre_spawn: Vec::new(),
        },
        Duration::from_secs(2),
    )
    .await
    .expect("raw prepared runtime")
}

#[tokio::test]
async fn classic_activation_consumes_stopped_obligation_and_reconciles_deferred_topology() {
    let temp = tempfile::tempdir().expect("temp dir");
    const MOB_ID: &str = "classic-pending-activation-topology";
    let first = boot_raw_activation_fixture(temp.path(), MOB_ID).await;
    assert_eq!(
        first.mob_handle().status().await.expect("first state"),
        meerkat_mob::MobState::Running
    );
    let shutdown = first.shutdown().await;
    assert!(shutdown.cleanup_completed(), "{shutdown:?}");
    drop(first);

    let mut second = boot_raw_activation_fixture(temp.path(), MOB_ID).await;
    assert_eq!(
        second.mob_handle().status().await.expect("prepared state"),
        meerkat_mob::MobState::Stopped,
        "this regression must exercise the durable stopped ledger"
    );
    assert!(
        second.bootstrap_edges_report().await.is_none(),
        "topology must remain deferred before activation"
    );
    second
        .activate_without_identity_context()
        .await
        .expect("classic composition consumes pending activation");
    assert_eq!(
        second.mob_handle().status().await.expect("activated state"),
        meerkat_mob::MobState::Running
    );
    let report = second
        .bootstrap_edges_report()
        .await
        .expect("configured topology must be reconciled after activation");
    assert!(report.is_complete(), "{report:?}");
    second
        .activate_without_identity_context()
        .await
        .expect("an already consumed obligation is a no-op");
    assert_eq!(second.bootstrap_edges_report().await, Some(report));
    let shutdown = second.shutdown().await;
    assert!(shutdown.cleanup_completed(), "{shutdown:?}");
}

#[tokio::test]
async fn classic_activation_refuses_installed_identity_authority_without_consuming_pending() {
    let temp = tempfile::tempdir().expect("temp dir");
    const MOB_ID: &str = "identity-guard-pending-activation";
    let first = boot_raw_activation_fixture(temp.path(), MOB_ID).await;
    let shutdown = first.shutdown().await;
    assert!(shutdown.cleanup_completed(), "{shutdown:?}");
    drop(first);

    let mut second = boot_raw_activation_fixture(temp.path(), MOB_ID).await;
    assert_eq!(
        second.mob_handle().status().await.expect("prepared state"),
        meerkat_mob::MobState::Stopped
    );
    let identity_runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: Arc::new(LocalContinuityStore::in_memory().expect("continuity")),
        lease_provider: Arc::new(LocalLeaseProvider::new()),
        runtime_instance_id: MOB_ID.to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: None,
        default_timeout: None,
    }));
    let context = Arc::new(IdentityFirstRuntimeContext::new(
        identity_runtime,
        Arc::new(MutableRosterProvider::new(Vec::new())),
        None,
        None,
        Some(second.mob_handle().definition().clone()),
    ));
    second.attach_identity_first_context(context.clone());
    let error = second
        .activate_without_identity_context()
        .await
        .expect_err("classic finalization must reject installed identity authority");
    assert!(
        matches!(error, MobRuntimeError::InvalidConfig(_)),
        "{error:?}"
    );
    assert_eq!(
        second.mob_handle().status().await.expect("refused state"),
        meerkat_mob::MobState::Stopped
    );
    assert!(second.bootstrap_edges_report().await.is_none());
    second
        .install_and_bootstrap_identity_first_context(context, &[])
        .await
        .expect("identity installer must retain and consume the refused pending token");
    assert_eq!(
        second
            .mob_handle()
            .status()
            .await
            .expect("identity activation state"),
        meerkat_mob::MobState::Running
    );
    let shutdown = second.shutdown().await;
    assert!(shutdown.cleanup_completed(), "{shutdown:?}");
}

fn activation_identity_roster() -> Vec<DurableAgentSpec> {
    [("lead-1", "lead"), ("worker-1", "worker")]
        .into_iter()
        .map(|(identity, profile)| DurableAgentSpec {
            identity: MetaAgentIdentity::parse(identity).expect("valid fixture identity"),
            profile: ProfileName::from(profile),
            addressability: AgentAddressability::Addressable,
            display_name: None,
            labels: BTreeMap::new(),
            context: None,
            additional_instructions: Vec::new(),
            initial_message: None,
            runtime_mode_override: None,
            backend: None,
            binding: None,
            placement: None,
        })
        .collect()
}

fn activation_identity_context(
    runtime: &UnifiedRuntime,
    root: &Path,
    roster: &[DurableAgentSpec],
    mode: IdentityBootstrapMode,
) -> (Arc<IdentityFirstRuntimeContext>, Arc<MobSessionBridge>) {
    let bridge = Arc::new(MobSessionBridge::with_session_service(
        runtime.mob_handle(),
        runtime
            .mob_runtime()
            .session_service()
            .cloned()
            .expect("persistent fixture session service"),
    ));
    let identity_runtime = Arc::new(
        IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(
                LocalContinuityStore::open(root.join("identity-continuity.sqlite3"))
                    .expect("persistent identity continuity"),
            ),
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: runtime.mob_handle().mob_id().to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(bridge.clone()),
            default_timeout: None,
        })
        .with_runtime_services(AgentRuntimeServices::new(runtime.mob_handle())),
    );
    let topology =
        DefinitionWiringEdgeDiscovery::from_definition(runtime.mob_handle().definition())
            .expect("fixture declares a role wire");
    let context = Arc::new(IdentityFirstRuntimeContext::new_with_bootstrap_mode(
        identity_runtime,
        Arc::new(MutableRosterProvider::new(roster.to_vec())),
        Some(Arc::new(EdgeDiscoveryTopologyAdapter::new(topology))),
        None,
        Some(runtime.mob_handle().definition().clone()),
        mode,
    ));
    (context, bridge)
}

#[tokio::test]
async fn eager_identity_restore_reports_edges_after_persisted_members_attach() {
    let temp = tempfile::tempdir().expect("temp dir");
    const MOB_ID: &str = "eager-restore-edge-observation";
    let roster = activation_identity_roster();
    let expected_edge = DesiredPeerEdge::new("lead-1", "worker-1").expect("fixture edge");

    let mut first = boot_raw_activation_fixture(temp.path(), MOB_ID).await;
    let (context, bridge) = activation_identity_context(
        &first,
        temp.path(),
        &roster,
        IdentityBootstrapMode::EagerMaterialize,
    );
    let created = first
        .install_and_bootstrap_identity_first_context(context.clone(), &roster)
        .await
        .expect("seed wired identity roster");
    assert_eq!(created.outcomes.len(), 2);
    assert!(
        created
            .outcomes
            .values()
            .all(|outcome| matches!(outcome, RestoreOutcome::Created { .. })),
        "both fixture members must really materialize: {:?}",
        created.outcomes
    );
    assert_eq!(
        bridge
            .current_member_wires()
            .await
            .expect("seeded physical wire")
            .len(),
        1
    );
    let mut original_sessions = BTreeMap::new();
    for spec in &roster {
        original_sessions.insert(
            spec.identity.clone(),
            context
                .runtime()
                .status(&spec.identity)
                .await
                .expect("seed identity")
                .session_id
                .expect("seed session"),
        );
    }
    let shutdown = first.shutdown().await;
    assert!(shutdown.cleanup_completed(), "{shutdown:?}");
    drop(context);
    drop(bridge);
    drop(first);

    let mut second = boot_raw_activation_fixture(temp.path(), MOB_ID).await;
    assert_eq!(
        second.mob_handle().status().await.expect("prepared state"),
        meerkat_mob::MobState::Stopped
    );
    assert!(second.bootstrap_edges_report().await.is_none());
    let persisted_members = second.mob_handle().list_members_including_retiring().await;
    assert_eq!(persisted_members.len(), 2);
    assert!(
        persisted_members
            .iter()
            .all(|member| member.wired_to.len() == 1)
    );

    let (context, bridge) = activation_identity_context(
        &second,
        temp.path(),
        &roster,
        IdentityBootstrapMode::EagerMaterialize,
    );
    // Before the identities attach, a fresh bridge has recorded no binding,
    // and it names the persisted wire through the incarnations Core committed
    // for both identity members: the read the binding epoch makes.
    let before_attachment = bridge
        .current_member_wires()
        .await
        .expect("a fresh bridge names the persisted wire before member attachment");
    assert_eq!(before_attachment.len(), 1, "{before_attachment:?}");
    let (runtime_a, runtime_b) = &before_attachment[0];
    let mut named = [runtime_a.as_str(), runtime_b.as_str()];
    named.sort_unstable();
    assert!(
        named[0].contains("lead-1") && named[1].contains("worker-1"),
        "the wire is the persisted lead-worker wire: {before_attachment:?}"
    );
    let restored = second
        .install_and_bootstrap_identity_first_context(context.clone(), &roster)
        .await
        .expect("restore and attach both wired identities");
    assert!(
        restored
            .outcomes
            .values()
            .all(|outcome| matches!(outcome, RestoreOutcome::Resumed { .. })),
        "restore must retain the persisted members: {:?}",
        restored.outcomes
    );
    for spec in &roster {
        let status = context
            .runtime()
            .status(&spec.identity)
            .await
            .expect("restored identity");
        assert_eq!(
            status.session_id.as_ref(),
            original_sessions.get(&spec.identity)
        );
    }
    assert!(context.runtime().identity_bootstrap_status().ready);
    assert_eq!(
        bridge
            .current_member_wires()
            .await
            .expect("restored physical wire"),
        before_attachment,
        "attachment keeps the incarnations the fresh bridge named"
    );
    let report = second
        .bootstrap_edges_report()
        .await
        .expect("final bootstrap edge observation");
    // Clean up before the decisive assertion so the expected RED run does
    // not retain an active runtime or its process-global comms registration.
    let shutdown = second.shutdown().await;
    assert!(shutdown.cleanup_completed(), "{shutdown:?}");
    assert!(
        report.is_complete(),
        "successful eager attachment must replace the pre-attachment missing-binding observation: {report:?}"
    );
    assert_eq!(report.desired_edges, vec![expected_edge.clone()]);
    assert_eq!(report.retained_edges, vec![expected_edge]);
}

#[tokio::test]
async fn lazy_identity_restore_keeps_unmaterialized_edges_pending() {
    let temp = tempfile::tempdir().expect("temp dir");
    const MOB_ID: &str = "lazy-restore-edge-observation";
    let first = boot_raw_activation_fixture(temp.path(), MOB_ID).await;
    let shutdown = first.shutdown().await;
    assert!(shutdown.cleanup_completed(), "{shutdown:?}");
    drop(first);

    let mut second = boot_raw_activation_fixture(temp.path(), MOB_ID).await;
    assert_eq!(
        second.mob_handle().status().await.expect("prepared state"),
        meerkat_mob::MobState::Stopped
    );
    assert!(second.bootstrap_edges_report().await.is_none());
    let roster = activation_identity_roster();
    let (context, bridge) = activation_identity_context(
        &second,
        temp.path(),
        &roster,
        IdentityBootstrapMode::LazyMaterialize,
    );
    second
        .install_and_bootstrap_identity_first_context(context.clone(), &roster)
        .await
        .expect("register lazy identity roster");
    let bootstrap = context.runtime().identity_bootstrap_status();
    assert!(bootstrap.complete);
    assert!(!bootstrap.ready);
    assert_eq!(bootstrap.counts.dormant, 2);
    assert_eq!(bootstrap.counts.active, 0);
    assert!(second.mob_handle().list_members().await.is_empty());
    assert!(
        bridge
            .current_member_wires()
            .await
            .expect("empty physical topology")
            .is_empty()
    );
    let report = second
        .bootstrap_edges_report()
        .await
        .expect("pending bootstrap edge observation");
    let shutdown = second.shutdown().await;
    assert!(shutdown.cleanup_completed(), "{shutdown:?}");
    assert!(
        !report.is_complete(),
        "unmaterialized endpoints cannot be reported wired"
    );
    assert!(report.failures.is_empty(), "{report:?}");
    assert_eq!(
        report.skipped_missing_members,
        vec![DesiredPeerEdge::new("lead-1", "worker-1").expect("fixture edge")]
    );
}
