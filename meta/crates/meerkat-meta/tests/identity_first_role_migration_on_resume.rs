//! A declared role migration is applied on the boot path, never dropped.
//!
//! Boot 1 materializes a member under its original profile. Boot 2 restarts on
//! the same durable state - so the member is in the mob's event log and the
//! mob actor's explicit resume restores it under its durable role - with a
//! roster that moves it to a new profile and an activation that declares the
//! migration. After boot 2 the member must run the new profile: its roster role
//! and its resolved tools are the new profile's. A declaration must never be a
//! silent no-op.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};
use std::pin::Pin;
use std::sync::Arc;

use meerkat_client::{LlmError, LlmEvent, LlmRequest};
use meerkat_core::types::StopReason;
use meerkat_meta::identity_first::orchestrator::{RestoreFlowResult, RestoreOutcome};
use meerkat_meta::identity_first::{
    AgentAddressability, AgentIdentity, AgentRuntimeServices, ContinuityStore, DurabilityPolicy,
    DurableAgentSpec, IdentityFirstRuntimeContext, IdentityRuntime, IdentityRuntimeConfig,
    IdentityRuntimeError, LocalContinuityStore, LocalLeaseProvider, MobSessionBridge,
    MutableRosterProvider, RoleMigrationDeclaration, WakeHold,
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

/// The member's materialized session, or a panic naming the outcome.
fn materialized_session(result: &RestoreFlowResult) -> meerkat_core::types::SessionId {
    let identity = AgentIdentity::parse(IDENTITY).unwrap();
    match result.outcomes.get(&identity) {
        Some(RestoreOutcome::Created { binding, .. } | RestoreOutcome::Resumed { binding, .. }) => {
            binding
                .session_id
                .clone()
                .expect("a materialized member is bound to a session")
        }
        other => panic!("{IDENTITY} did not materialize: {other:?}"),
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

async fn resolved_tools(
    unified: &meerkat_meta::UnifiedRuntime,
    session_id: meerkat_core::types::SessionId,
) -> BTreeSet<String> {
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

    // Boot 1: the member is materialized under its original profile.
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
