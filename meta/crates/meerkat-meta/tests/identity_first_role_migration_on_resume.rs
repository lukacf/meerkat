//! A declared role migration is applied on the boot path, never dropped.
//!
//! Boot 1 seats a member under its original profile. Boot 2 restarts on the
//! same durable state - so the member is in the mob's event log and the mob
//! restores it, seated, under its durable role - with a roster that moves it
//! to a new profile and an activation that declares the migration. After boot
//! 2 the member must run the new profile: its roster role and, once Core
//! materializes it, its resolved tools are the new profile's. A declaration
//! must never be a silent no-op.
//!
//! On a runtime whose sessions live in memory only there is no session to
//! carry over: a roster refresh moves the member onto a fresh session under
//! the new profile, declared or not, and the previous role answers nothing
//! more.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use std::pin::Pin;
use std::sync::Arc;

use meerkat_client::{LlmError, LlmEvent, LlmRequest};
use meerkat_core::types::StopReason;
use meerkat_meta::identity_first::orchestrator::{RestoreFlowResult, RestoreOutcome};
use meerkat_meta::identity_first::{
    AgentAddressability, AgentIdentity, AgentRuntimeServices, ContinuityStore, DurabilityPolicy,
    DurableAgentSpec, IdentityFirstRuntimeContext, IdentityLifecycleState, IdentityRuntime,
    IdentityRuntimeConfig, IdentityRuntimeError, LocalContinuityStore, LocalLeaseProvider,
    MobSessionBridge, MutableRosterProvider, RoleMigrationDeclaration, RosterProvider, WakeHold,
};
use meerkat_meta::mob_composition_manifest::persistent_mob_storage;
use meerkat_meta::{DiscoverySpec, MetaConfig, MobBootstrapOptions, MobBootstrapSpec};
use meerkat_mob::{MobDefinition, ProfileName};

#[path = "support/llm_usage.rs"]
mod llm_usage;

const IDENTITY: &str = "identity:member-1";
const FROM_ROLE: &str = "identity";
const TO_ROLE: &str = "identity-member";

fn definition(mob_id: &str) -> MobDefinition {
    MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "{mob_id}"

[profiles.identity]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.identity.tools]
comms = true
mob = true

[profiles.identity-member]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.identity-member.tools]
comms = true
"#
    ))
    .expect("parse the team definition")
}

fn spec(profile: &str) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: AgentIdentity::parse(IDENTITY).unwrap(),
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
    }
}

/// Answers every turn; no turn runs here, but the runtime needs a client.
#[derive(Clone, Default)]
struct OkClient;

impl meerkat_client::LlmClient for OkClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        let [usage, done] =
            llm_usage::usage_then_done(request, meerkat::Provider::OpenAI, StopReason::EndTurn);
        Box::pin(async_stream::stream! {
            yield Ok(LlmEvent::TextDelta { delta: "ok".to_string(), meta: None });
            yield Ok(usage);
            yield Ok(done);
        })
    }

    fn provider(&self) -> meerkat::Provider {
        meerkat::Provider::OpenAI
    }

    fn health_check<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

/// The role prompt (an inline skill) of the original profile and of the new
/// one in [`prompted_definition`]: a request carries the marker of the
/// profile its member was built from.
const FROM_MARKER: &str = "ROLE-MARKER-ORIGINAL-PROFILE";
const TO_MARKER: &str = "ROLE-MARKER-NEW-PROFILE";

/// [`definition`] with a role prompt per profile.
fn prompted_definition(mob_id: &str) -> MobDefinition {
    MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "{mob_id}"

[profiles.identity]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"
skills = ["original-role"]

[profiles.identity.tools]
comms = true

[profiles.identity-member]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"
skills = ["new-role"]

[profiles.identity-member.tools]
comms = true

[skills.original-role]
source = "inline"
content = "{FROM_MARKER}"

[skills.new-role]
source = "inline"
content = "{TO_MARKER}"
"#
    ))
    .expect("parse the prompted team definition")
}

/// Answers every turn and keeps every request, serialized, in arrival
/// order. While its gate is closed, each turn's answer waits for it to
/// open, so the turn keeps running.
#[derive(Clone)]
struct RecordingClient {
    requests: Arc<std::sync::Mutex<Vec<String>>>,
    gate: Arc<tokio::sync::watch::Sender<bool>>,
}

impl Default for RecordingClient {
    fn default() -> Self {
        Self {
            requests: Arc::default(),
            gate: Arc::new(tokio::sync::watch::channel(true).0),
        }
    }
}

impl RecordingClient {
    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }

    /// Hold every turn's answer until [`Self::open`].
    fn close(&self) {
        self.gate.send_replace(false);
    }

    fn open(&self) {
        self.gate.send_replace(true);
    }

    /// The first request carrying `needle`, waited for while the turn that
    /// asks it is admitted and starts.
    async fn request_carrying(&self, needle: &str) -> String {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            if let Some(request) = self
                .requests()
                .into_iter()
                .find(|request| request.contains(needle))
            {
                return request;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "no request carries {needle}: {:?}",
                self.requests()
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }
}

impl meerkat_client::LlmClient for RecordingClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> Result<Vec<meerkat_core::Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        self.requests
            .lock()
            .unwrap()
            .push(serde_json::to_string(request).expect("serialize the request"));
        let [usage, done] =
            llm_usage::usage_then_done(request, meerkat::Provider::OpenAI, StopReason::EndTurn);
        let mut gate = self.gate.subscribe();
        Box::pin(async_stream::stream! {
            let _ = gate.wait_for(|open| *open).await;
            yield Ok(LlmEvent::TextDelta { delta: "ok".to_string(), meta: None });
            yield Ok(usage);
            yield Ok(done);
        })
    }

    fn provider(&self) -> meerkat::Provider {
        meerkat::Provider::OpenAI
    }

    fn health_check<'life0, 'async_trait>(
        &'life0 self,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<(), LlmError>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async { Ok(()) })
    }
}

struct Booted {
    unified: meerkat_meta::UnifiedRuntime,
    result: Result<RestoreFlowResult, String>,
}

/// Boot on durable state the way a deployment does (persistent mob storage, a
/// SQLite session store, a durable continuity store), activated through
/// `install_and_bootstrap_identity_first_context`, with this activation's
/// role-migration declarations on the session bridge.
async fn boot(
    state_path: &std::path::Path,
    mob_id: &str,
    roster: &[DurableAgentSpec],
    role_migrations: Vec<RoleMigrationDeclaration>,
) -> Booted {
    std::fs::create_dir_all(state_path).expect("state root");
    let (storage, provenance) = persistent_mob_storage(state_path.join("mob.sqlite3"))
        .expect("open persistent mob storage");
    let spec = MobBootstrapSpec::persistent(
        definition(mob_id),
        storage,
        provenance,
        state_path.to_path_buf(),
        16,
    )
    .await
    .expect("compose persistent Meta stores")
    // Creates no agent child mob. It restarts in this process, which the
    // durable child-mob default refuses as a duplicate owner; explicit
    // ProcessBound keeps this test's cold-recovery oracle.
    .with_child_mob_storage(meerkat_meta::ChildMobStorage::ProcessBound)
    .with_options(MobBootstrapOptions {
        allow_ephemeral_sessions: true,
        notify_orchestrator_on_resume: true,
        default_llm_client: Some(Arc::new(OkClient)),
    });
    let mut unified = meerkat_meta::UnifiedRuntime::bootstrap(
        spec,
        MetaConfig {
            modules: Vec::new(),
            discovery: DiscoverySpec {
                namespace: "role-migration-on-resume".to_string(),
                modules: Vec::new(),
            },
            pre_spawn: Vec::new(),
        },
        std::time::Duration::from_secs(10),
    )
    .await
    .expect("bootstrap the UnifiedRuntime");
    let bridge = Arc::new(
        MobSessionBridge::with_session_service(
            unified.mob_handle(),
            unified
                .mob_runtime()
                .session_service()
                .cloned()
                .expect("the persistent runtime has a session service"),
        )
        .with_role_migration_declarations(role_migrations),
    );
    let store = Arc::new(
        LocalContinuityStore::open(state_path.join("continuity.sqlite3"))
            .expect("continuity store"),
    );
    let identity_rt = Arc::new(
        IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: store as Arc<dyn ContinuityStore>,
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "role-migration-on-resume".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(bridge),
            default_timeout: None,
        })
        .with_runtime_services(AgentRuntimeServices::new(unified.mob_handle())),
    );
    let context = Arc::new(IdentityFirstRuntimeContext::new(
        identity_rt,
        Arc::new(MutableRosterProvider::new(roster.to_vec())),
        None,
        None,
        Some(definition(mob_id)),
    ));
    let result = unified
        .install_and_bootstrap_identity_first_context(context, roster)
        .await
        .map_err(|error| error.to_string());
    Booted { unified, result }
}

/// Boot a runtime whose sessions live in memory only
/// (`MobBootstrapSpec::ephemeral`: Core's session service keeps no session
/// beyond its member) over `roster`'s specs, its session bridge wired as the
/// gateways wire it, activated through
/// `install_and_bootstrap_identity_first_context`.
async fn boot_in_memory(
    store_path: &std::path::Path,
    mob_id: &str,
    roster: &Arc<MutableRosterProvider>,
    client: RecordingClient,
) -> meerkat_meta::UnifiedRuntime {
    let spec = MobBootstrapSpec::ephemeral(
        prompted_definition(mob_id),
        meerkat_mob::MobStorage::in_memory(),
        store_path.to_path_buf(),
        4,
        None,
    )
    .expect("compose the in-memory Meta stores")
    .with_options(MobBootstrapOptions {
        allow_ephemeral_sessions: true,
        notify_orchestrator_on_resume: true,
        default_llm_client: Some(Arc::new(client)),
    });
    let mut unified = meerkat_meta::UnifiedRuntime::bootstrap(
        spec,
        MetaConfig {
            modules: Vec::new(),
            discovery: DiscoverySpec {
                namespace: "role-migration-in-memory".to_string(),
                modules: Vec::new(),
            },
            pre_spawn: Vec::new(),
        },
        std::time::Duration::from_secs(10),
    )
    .await
    .expect("bootstrap the in-memory UnifiedRuntime");
    let bridge = Arc::new(MobSessionBridge::with_session_service(
        unified.mob_handle(),
        unified
            .mob_runtime()
            .session_service()
            .cloned()
            .expect("the in-memory runtime has a session service"),
    ));
    let identity_rt = Arc::new(
        IdentityRuntime::new(IdentityRuntimeConfig {
            continuity_store: Arc::new(LocalContinuityStore::in_memory().expect("continuity store"))
                as Arc<dyn ContinuityStore>,
            lease_provider: Arc::new(LocalLeaseProvider::new()),
            runtime_instance_id: "role-migration-in-memory".to_string(),
            has_runtime_store: true,
            durability_policy: DurabilityPolicy::SyncWriteThrough,
            bridge: Some(bridge),
            default_timeout: None,
        })
        .with_runtime_services(AgentRuntimeServices::new(unified.mob_handle())),
    );
    let context = Arc::new(IdentityFirstRuntimeContext::new(
        identity_rt,
        Arc::clone(roster) as Arc<dyn RosterProvider>,
        None,
        None,
        Some(prompted_definition(mob_id)),
    ));
    unified
        .install_and_bootstrap_identity_first_context(context, &roster.snapshot())
        .await
        .expect("bootstrap the roster");
    unified
}

/// The session the member is seated on, or a panic naming the outcome.
fn materialized_session(result: &RestoreFlowResult) -> meerkat_core::types::SessionId {
    let identity = AgentIdentity::parse(IDENTITY).unwrap();
    match result.outcomes.get(&identity) {
        Some(RestoreOutcome::Created { binding, .. } | RestoreOutcome::Resumed { binding, .. }) => {
            binding
                .session_id
                .clone()
                .expect("a seated member is bound to a session")
        }
        other => panic!("{IDENTITY} was not seated: {other:?}"),
    }
}

/// The member's roster role, as the mob reports it.
async fn roster_role(unified: &meerkat_meta::UnifiedRuntime) -> ProfileName {
    let members = unified.mob_handle().list_members().await;
    let [member] = members.as_slice() else {
        panic!("exactly one member is rostered: {members:?}");
    };
    member.role.clone()
}

/// The member's live tools: Core materializes the member first (Core's
/// wake), since a seated member holds no live session to read until then.
async fn resolved_tools(
    unified: &meerkat_meta::UnifiedRuntime,
    session_id: meerkat_core::types::SessionId,
) -> BTreeSet<String> {
    unified
        .identity_runtime()
        .expect("identity-first runtime")
        .materialize(&AgentIdentity::parse(IDENTITY).unwrap())
        .await
        .expect("Core materializes the member");
    meerkat_meta::mob_handle_runtime::resolved_tools_for_session(
        unified.mob_runtime().session_service(),
        IDENTITY,
        session_id,
    )
    .await
    .expect("resolve the member's tools")
    .tools
    .into_iter()
    .collect()
}

/// Tools the original profile grants and the new profile does not.
const ORIGINAL_ONLY_TOOLS: &[&str] = &["spawn_member", "wire_members"];

#[tokio::test(flavor = "multi_thread")]
async fn a_declared_role_migration_is_applied_when_the_mob_restores_the_member() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state_path = temp.path().join("state");
    let mob_id = "role-migration-on-resume";

    // Boot 1: the member is seated under its original profile.
    let booted = boot(&state_path, mob_id, &[spec(FROM_ROLE)], Vec::new()).await;
    let result = booted.result.as_ref().expect("boot 1 restores");
    let session_id = materialized_session(result);
    assert_eq!(roster_role(&booted.unified).await.as_str(), FROM_ROLE);
    let original_tools = resolved_tools(&booted.unified, session_id.clone()).await;
    for tool in ORIGINAL_ONLY_TOOLS {
        assert!(
            original_tools.contains(*tool),
            "boot 1: the original profile grants {tool}: {original_tools:?}"
        );
    }
    booted.unified.shutdown().await;
    drop(booted);

    // Boot 2: the roster moves the member to the new profile and the
    // activation declares the migration from the original one.
    let booted = boot(
        &state_path,
        mob_id,
        &[spec(TO_ROLE)],
        vec![RoleMigrationDeclaration {
            identity: AgentIdentity::parse(IDENTITY).unwrap(),
            from_role: ProfileName::from(FROM_ROLE),
        }],
    )
    .await;
    let result = booted.result.as_ref().expect("boot 2 restores");
    let migrated_session = materialized_session(result);
    assert_eq!(
        migrated_session, session_id,
        "the migration resumes the member's own session"
    );
    assert_eq!(
        roster_role(&booted.unified).await.as_str(),
        TO_ROLE,
        "after the restart the member runs the declared profile"
    );
    let migrated_tools = resolved_tools(&booted.unified, migrated_session).await;
    for tool in ORIGINAL_ONLY_TOOLS {
        assert!(
            !migrated_tools.contains(*tool),
            "boot 2: the new profile does not grant {tool}: {migrated_tools:?}"
        );
    }
    booted.unified.shutdown().await;
    drop(booted);

    // Boot 3 declares nothing: the declaration was boot-scoped, but the
    // migration it applied is durable, so the member comes back on the new
    // profile.
    let booted = boot(&state_path, mob_id, &[spec(TO_ROLE)], Vec::new()).await;
    let result = booted.result.as_ref().expect("boot 3 restores");
    let session = materialized_session(result);
    assert_eq!(session, session_id, "boot 3 resumes the same session");
    assert_eq!(
        roster_role(&booted.unified).await.as_str(),
        TO_ROLE,
        "the applied migration is durable"
    );
    let tools = resolved_tools(&booted.unified, session).await;
    for tool in ORIGINAL_ONLY_TOOLS {
        assert!(
            !tools.contains(*tool),
            "boot 3: the new profile does not grant {tool}: {tools:?}"
        );
    }
    booted.unified.shutdown().await;
}

/// A declared role migration of a member whose session no wake has created
/// yet: boot 1 seats it and never wakes it, so Core holds the session unborn.
/// There is no session to restamp, so the move is Core's rotation onto the
/// declared profile, on a fresh session that no wake has created either.
/// Nothing is resumed and nothing is refused. Before, the migration resumed
/// the never-created session, its collision repair found no resume source
/// and refused, and the identity read broken. A migration of a session a
/// wake created keeps that session
/// (`a_declared_role_migration_is_applied_when_the_mob_restores_the_member`).
#[tokio::test(flavor = "multi_thread")]
async fn a_declared_role_migration_of_a_seat_never_woken_rotates_it_onto_the_new_profile() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state_path = temp.path().join("state");
    let mob_id = "role-migration-unborn-seat";

    // Boot 1: the member is seated under its original profile and never
    // woken.
    let booted = boot(&state_path, mob_id, &[spec(FROM_ROLE)], Vec::new()).await;
    let seated = materialized_session(booted.result.as_ref().expect("boot 1 restores"));
    assert!(
        booted.unified.mob_handle().session_unborn(&seated),
        "boot 1 seats the member and creates no session"
    );
    booted.unified.shutdown().await;
    drop(booted);

    // Boot 2: the roster moves the member to the new profile, and the
    // activation declares the migration from the original one.
    let booted = boot(
        &state_path,
        mob_id,
        &[spec(TO_ROLE)],
        vec![RoleMigrationDeclaration {
            identity: AgentIdentity::parse(IDENTITY).unwrap(),
            from_role: ProfileName::from(FROM_ROLE),
        }],
    )
    .await;
    let moved = materialized_session(booted.result.as_ref().expect("boot 2 restores"));
    assert_ne!(moved, seated, "the move is a rotation onto a fresh session");
    assert!(
        booted.unified.mob_handle().session_unborn(&moved),
        "the rotation builds nothing: the fresh session is unborn too"
    );
    assert_eq!(
        roster_role(&booted.unified).await.as_str(),
        TO_ROLE,
        "the member runs the declared profile"
    );
    booted.unified.shutdown().await;
}

/// A declaration whose predecessor role is not the member's durable role does
/// not describe this member. It is refused typed, before any destructive step:
/// the member the mob restored is not retired.
#[tokio::test(flavor = "multi_thread")]
async fn a_declared_role_migration_from_the_wrong_role_is_refused_before_any_retire() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state_path = temp.path().join("state");
    let mob_id = "role-migration-wrong-predecessor";

    let booted = boot(&state_path, mob_id, &[spec(FROM_ROLE)], Vec::new()).await;
    let session_id = materialized_session(booted.result.as_ref().expect("boot 1 restores"));
    booted.unified.shutdown().await;
    drop(booted);

    let booted = boot(
        &state_path,
        mob_id,
        &[spec(TO_ROLE)],
        vec![RoleMigrationDeclaration {
            identity: AgentIdentity::parse(IDENTITY).unwrap(),
            from_role: ProfileName::from("predecessor"),
        }],
    )
    .await;
    let result = booted.result.as_ref().expect("boot 2 runs its restore");
    match result
        .outcomes
        .get(&AgentIdentity::parse(IDENTITY).unwrap())
    {
        Some(RestoreOutcome::Refused(error)) => match error.as_ref() {
            IdentityRuntimeError::WakeHeld {
                hold: WakeHold::RoleMigrationNotApplied,
                detail,
                ..
            } => assert!(
                detail.contains(&format!(
                    "declared role migration of {IDENTITY} from 'predecessor' to '{TO_ROLE}' \
                     was not applied"
                )) && detail.contains("not the declared predecessor role"),
                "the refusal names the migration and why it was not applied: {detail}"
            ),
            other => panic!("the refusal must be the held role migration: {other:?}"),
        },
        other => panic!("the mismatched declaration must be refused: {other:?}"),
    }
    // The host sees it on the activation result: `meta/init` returns this
    // bootstrap status as `identity_bootstrap`. Read it once it is terminal:
    // any pass that runs is in flight until it settles.
    let (status, timed_out) = booted
        .unified
        .identity_runtime()
        .expect("identity-first runtime")
        .wait_identity_bootstrap_terminal(std::time::Duration::from_secs(30))
        .await;
    assert!(!timed_out, "the bootstrap status settles: {status:?}");
    assert!(
        !status.ready,
        "a refused migration leaves the boot not ready"
    );
    assert_eq!(status.counts.broken, 1);
    let wire = serde_json::to_value(&status).expect("serialize the bootstrap status");
    let entry = &wire["identities"][IDENTITY];
    assert_eq!(entry["state"], "broken", "{wire}");
    assert_eq!(
        entry["restore"]["kind"], "identity_role_migration_refused",
        "the activation result carries the typed refusal: {wire}"
    );
    let members = booted.unified.mob_handle().list_members().await;
    let [member] = members.as_slice() else {
        panic!("the restored member was not retired: {members:?}");
    };
    assert_eq!(member.role.as_str(), FROM_ROLE);
    assert_eq!(
        booted
            .unified
            .mob_handle()
            .resolve_bridge_session_id(&member.agent_identity)
            .await,
        Some(session_id),
        "the restored member still runs its own session"
    );
    booted.unified.shutdown().await;
}

/// A refused declaration is a terminal verdict for the spec it was refused
/// against. A later pass over the same roster (a host refresh here) keeps
/// the member the refusal preserved instead of retiring it, and holds the
/// same typed refusal (main CI run 38596087789).
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_role_migration_is_not_retired_by_a_later_pass() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let state_path = temp.path().join("state");
    let mob_id = "role-migration-refused-later-pass";
    let identity = AgentIdentity::parse(IDENTITY).unwrap();

    let booted = boot(&state_path, mob_id, &[spec(FROM_ROLE)], Vec::new()).await;
    let session_id = materialized_session(booted.result.as_ref().expect("boot 1 restores"));
    booted.unified.shutdown().await;
    drop(booted);

    let booted = boot(
        &state_path,
        mob_id,
        &[spec(TO_ROLE)],
        vec![RoleMigrationDeclaration {
            identity: identity.clone(),
            from_role: ProfileName::from("predecessor"),
        }],
    )
    .await;
    match booted
        .result
        .as_ref()
        .expect("boot 2 runs its restore")
        .outcomes
        .get(&identity)
    {
        Some(RestoreOutcome::Refused(error)) => assert!(
            matches!(
                error.as_ref(),
                IdentityRuntimeError::WakeHeld {
                    hold: WakeHold::RoleMigrationNotApplied,
                    ..
                }
            ),
            "the mismatched declaration must be held: {error:?}"
        ),
        other => panic!("the mismatched declaration must be refused: {other:?}"),
    }
    let context = booted
        .unified
        .identity_first_context()
        .expect("identity-first context");
    let refreshed = context
        .refresh_desired_topology()
        .await
        .expect("a later pass over the same roster runs");
    match refreshed.outcomes.get(&identity) {
        Some(RestoreOutcome::Refused(error)) => assert!(
            matches!(
                error.as_ref(),
                IdentityRuntimeError::WakeHeld {
                    hold: WakeHold::RoleMigrationNotApplied,
                    ..
                }
            ),
            "the later pass keeps the typed refusal: {error:?}"
        ),
        other => panic!("the later pass must keep the refusal: {other:?}"),
    }
    let members = booted.unified.mob_handle().list_members().await;
    let [member] = members.as_slice() else {
        panic!("the refused member was retired by the later pass: {members:?}");
    };
    assert_eq!(member.role.as_str(), FROM_ROLE);
    assert_eq!(
        booted
            .unified
            .mob_handle()
            .resolve_bridge_session_id(&member.agent_identity)
            .await,
        Some(session_id),
        "the kept member still runs its own session"
    );
    let (status, timed_out) = booted
        .unified
        .identity_runtime()
        .expect("identity-first runtime")
        .wait_identity_bootstrap_terminal(std::time::Duration::from_secs(30))
        .await;
    assert!(!timed_out, "the bootstrap status settles: {status:?}");
    assert_eq!(status.counts.broken, 1);
    let wire = serde_json::to_value(&status).expect("serialize the bootstrap status");
    assert_eq!(
        wire["identities"][IDENTITY]["restore"]["kind"], "identity_role_migration_refused",
        "the status keeps the typed refusal: {wire}"
    );
    booted.unified.shutdown().await;
}

/// On a runtime whose sessions live in memory only, a roster refresh that
/// moves an identity to another profile moves it onto a fresh session under
/// the new profile, with no migration declared: there is no session to
/// carry over, and Core's release keeps such a member materialized, so a
/// release alone would leave the previous role answering. A message sent
/// to the member on the mob handle, outside Meta, and the next turn through
/// Meta both run under the new profile, and no request after the reprofile
/// is built from the previous one. The member is idle at the refresh.
#[tokio::test(flavor = "multi_thread")]
async fn an_in_memory_reprofile_moves_the_member_to_a_fresh_session_under_the_new_profile() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let identity = AgentIdentity::parse(IDENTITY).unwrap();
    let client = RecordingClient::default();
    let roster = Arc::new(MutableRosterProvider::new(vec![spec(FROM_ROLE)]));
    let unified = boot_in_memory(
        &temp.path().join("store"),
        "role-migration-in-memory",
        &roster,
        client.clone(),
    )
    .await;
    let identities = unified.identity_runtime().expect("identity-first runtime");

    // A turn under the original profile.
    identities
        .send_awaiting_commit(
            &identity,
            &meerkat_core::ContentInput::Text("FIRST-TURN".to_string()),
        )
        .await
        .expect("the first turn runs");
    let first = client.request_carrying("FIRST-TURN").await;
    assert!(
        first.contains(FROM_MARKER) && !first.contains(TO_MARKER),
        "the first turn runs under the original profile: {first}"
    );
    let before = identities.status(&identity).await.expect("identity status");
    let asked_before = client.requests().len();

    // The roster moves the identity to the new profile; nothing declares a
    // migration.
    roster.upsert(spec(TO_ROLE));
    let refreshed = unified
        .identity_first_context()
        .expect("identity-first context")
        .refresh_desired_topology()
        .await
        .expect("the roster refresh runs");

    // A message sent to the member on the mob handle, outside Meta, reaches
    // the new profile, on a session that carries nothing of the previous one.
    meerkat_meta::send_message_on_mob(&unified.mob_handle(), IDENTITY, "PEER-TURN")
        .await
        .expect("Core admits the message");
    let peer = client.request_carrying("PEER-TURN").await;
    assert!(
        peer.contains(TO_MARKER) && !peer.contains(FROM_MARKER),
        "the message runs under the new profile, not the previous one: {peer}"
    );
    assert!(
        !peer.contains("FIRST-TURN"),
        "the fresh session carries no transcript of the previous role: {peer}"
    );

    let session = materialized_session(&refreshed);
    assert_ne!(
        Some(&session),
        before.session_id.as_ref(),
        "the reprofile moves the member to a fresh session"
    );
    let after = identities.status(&identity).await.expect("identity status");
    assert_eq!(after.session_id.as_ref(), Some(&session), "{after:?}");
    assert!(
        after.generation.expect("a generation").get()
            > before.generation.expect("a generation").get(),
        "the previous incarnation is retired: {before:?} -> {after:?}"
    );
    assert_ne!(
        after.agent_runtime_id, before.agent_runtime_id,
        "the previous incarnation's runtime id names nothing now: {after:?}"
    );
    assert_eq!(after.state, IdentityLifecycleState::Active, "{after:?}");
    assert!(after.wake_refusal.is_none(), "nothing is held: {after:?}");
    assert_eq!(roster_role(&unified).await.as_str(), TO_ROLE);

    // The next turn through Meta runs under the new profile too.
    identities
        .send_awaiting_commit(
            &identity,
            &meerkat_core::ContentInput::Text("NEXT-TURN".to_string()),
        )
        .await
        .expect("the next turn runs");
    let next = client.request_carrying("NEXT-TURN").await;
    assert!(next.contains(TO_MARKER), "{next}");
    for request in &client.requests()[asked_before..] {
        assert!(
            !request.contains(FROM_MARKER),
            "a request after the reprofile was built from the previous profile: {request}"
        );
    }
    drop(identities);
    unified.shutdown().await;
}

/// On a runtime whose sessions live in memory only, a roster refresh does
/// not rotate a member with work in flight: the rotation retires the
/// member, and a retirement cancels a running turn and every queued input.
/// While a turn runs and an input waits behind it, the refresh leaves the
/// member under its previous role on its session; both complete under that
/// role. A later refresh, once the member is idle, rotates it, and the next
/// turn runs under the new profile.
#[tokio::test(flavor = "multi_thread")]
async fn an_in_memory_reprofile_waits_for_the_members_work_in_flight() {
    let temp = tempfile::TempDir::new().expect("temp dir");
    let identity = AgentIdentity::parse(IDENTITY).unwrap();
    let client = RecordingClient::default();
    let roster = Arc::new(MutableRosterProvider::new(vec![spec(FROM_ROLE)]));
    let unified = boot_in_memory(
        &temp.path().join("store"),
        "role-migration-in-memory-busy",
        &roster,
        client.clone(),
    )
    .await;
    let identities = unified.identity_runtime().expect("identity-first runtime");

    // A turn in flight: its answer waits for the gate, so it keeps running.
    client.close();
    let running = {
        let identities = identities.clone();
        let identity = identity.clone();
        tokio::spawn(async move {
            identities
                .send_awaiting_commit(
                    &identity,
                    &meerkat_core::ContentInput::Text("TURN-ONE".to_string()),
                )
                .await
        })
    };
    let first = client.request_carrying("TURN-ONE").await;
    assert!(
        first.contains(FROM_MARKER),
        "the running turn is the original profile's: {first}"
    );
    // An input queued behind it (admitted, not yet run).
    identities
        .send(
            &identity,
            &meerkat_core::ContentInput::Text("TURN-TWO".to_string()),
        )
        .await
        .expect("the queued input is admitted");
    let before = identities.status(&identity).await.expect("identity status");

    // The roster moves the identity while its work is in flight.
    roster.upsert(spec(TO_ROLE));
    let deferred = unified
        .identity_first_context()
        .expect("identity-first context")
        .refresh_desired_topology()
        .await
        .expect("the roster refresh runs");

    // Both complete under the previous role: nothing was cancelled.
    client.open();
    tokio::time::timeout(std::time::Duration::from_secs(30), running)
        .await
        .expect("the running turn ends")
        .expect("the running turn's task")
        .expect("the running turn commits under the previous role");
    let second = client.request_carrying("TURN-TWO").await;
    assert!(
        second.contains(FROM_MARKER) && !second.contains(TO_MARKER),
        "the queued input runs under the previous role: {second}"
    );
    match deferred.outcomes.get(&identity) {
        Some(RestoreOutcome::Refused(error)) => assert!(
            matches!(
                error.as_ref(),
                IdentityRuntimeError::IdentityWakeRetryable { .. }
            ),
            "the busy member's move waits, retryably: {error:?}"
        ),
        other => panic!("the busy member's move waits: {other:?}"),
    }
    let waited = identities.status(&identity).await.expect("identity status");
    assert_eq!(
        (waited.session_id.as_ref(), waited.generation),
        (before.session_id.as_ref(), before.generation),
        "the busy member was not rotated: {before:?} -> {waited:?}"
    );

    // A later refresh, once the member is idle, rotates it.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    let rotated = loop {
        let refreshed = unified
            .identity_first_context()
            .expect("identity-first context")
            .refresh_desired_topology()
            .await
            .expect("the roster refresh runs");
        match refreshed.outcomes.get(&identity) {
            Some(RestoreOutcome::Created { .. }) => break refreshed,
            Some(RestoreOutcome::Refused(error))
                if matches!(
                    error.as_ref(),
                    IdentityRuntimeError::IdentityWakeRetryable { .. }
                ) && tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            other => panic!("the idle member is rotated: {other:?}"),
        }
    };
    let session = materialized_session(&rotated);
    assert_ne!(
        Some(&session),
        before.session_id.as_ref(),
        "the idle member moves to a fresh session"
    );
    assert_eq!(roster_role(&unified).await.as_str(), TO_ROLE);

    // The next turn runs under the new profile, on a session that carries
    // nothing of the previous role.
    identities
        .send_awaiting_commit(
            &identity,
            &meerkat_core::ContentInput::Text("TURN-THREE".to_string()),
        )
        .await
        .expect("the next turn runs");
    let third = client.request_carrying("TURN-THREE").await;
    assert!(
        third.contains(TO_MARKER) && !third.contains(FROM_MARKER) && !third.contains("TURN-ONE"),
        "the next turn runs under the new profile on a fresh session: {third}"
    );
    drop(identities);
    unified.shutdown().await;
}
