//! The identity-first install and bootstrap phases: an installed context is
//! visible (its public view answers, shutdown finds it) before anything is
//! registered, activated, restored or supervised; bootstrap then completes it.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use crate::identity_first::{
    DurabilityPolicy, DurableAgentSpec, IdentityFirstRuntimeContext, IdentityRuntime,
    IdentityRuntimeConfig, LocalContinuityStore, LocalLeaseProvider, MobSessionBridge,
    RosterContext, RosterError, RosterProvider,
};
use crate::runtime::RuntimeOptions;
use crate::{
    DiscoverySpec, InMemoryMetadataStore, MetaConfig, MobBootstrapOptions, MobBootstrapSpec,
};

use super::UnifiedRuntime;

struct EmptyRoster;

#[async_trait::async_trait]
impl RosterProvider for EmptyRoster {
    async fn roster(&self, _context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        Ok(Vec::new())
    }
}

pub(super) async fn runtime(temp: &tempfile::TempDir) -> UnifiedRuntime {
    // A distinct mob per test: parallel tests share the process-wide
    // participant name registry.
    let definition = meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"identity-install-phase-{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
        uuid::Uuid::new_v4().simple()
    ))
    .expect("mob definition");
    let spec = MobBootstrapSpec::ephemeral(
        definition,
        meerkat_mob::MobStorage::in_memory(),
        temp.path().to_path_buf(),
        4,
        None,
    )
    .expect("ephemeral spec")
    .with_options(MobBootstrapOptions {
        allow_ephemeral_sessions: true,
        notify_orchestrator_on_resume: true,
        default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
    });
    UnifiedRuntime::bootstrap_with_options(
        spec,
        MetaConfig {
            modules: Vec::new(),
            discovery: DiscoverySpec {
                namespace: "identity-install-phase".to_string(),
                modules: Vec::new(),
            },
            pre_spawn: Vec::new(),
        },
        Vec::new(),
        Duration::from_secs(2),
        RuntimeOptions::default(),
        Arc::new(InMemoryMetadataStore::new()),
    )
    .await
    .expect("bootstrap runtime")
}

pub(super) fn context(runtime: &UnifiedRuntime) -> Arc<IdentityFirstRuntimeContext> {
    context_with_roster(runtime, Arc::new(EmptyRoster))
}

/// An identity context over `runtime`'s mob whose host configures the
/// agents `roster` answers.
pub(super) fn context_with_roster(
    runtime: &UnifiedRuntime,
    roster: Arc<dyn RosterProvider>,
) -> Arc<IdentityFirstRuntimeContext> {
    let identity_runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: Arc::new(LocalContinuityStore::in_memory().expect("continuity store")),
        lease_provider: Arc::new(LocalLeaseProvider::new()),
        runtime_instance_id: "identity-install-phase".to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(Arc::new(MobSessionBridge::new(runtime.mob_handle()))),
        default_timeout: None,
    }));
    Arc::new(IdentityFirstRuntimeContext::new(
        identity_runtime,
        roster,
        None,
        None,
        Some(runtime.mob_handle().definition().clone()),
    ))
}

/// The row of an agent a host configures: the name `name`, the fixture
/// mob's `worker` profile, nothing else.
pub(super) fn configured_agent(name: &str) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: crate::identity_first::AgentIdentity::parse(name).expect("identity"),
        profile: meerkat_mob::ProfileName::from("worker"),
        addressability: crate::identity_first::AgentAddressability::Addressable,
        display_name: None,
        labels: std::collections::BTreeMap::new(),
        context: None,
        additional_instructions: Vec::new(),
        initial_message: None,
        runtime_mode_override: None,
        backend: None,
        binding: None,
        placement: None,
    }
}

/// A runtime whose mob is built without agent mob tools: a host's own
/// session service, and nothing installed on the spec.
async fn runtime_without_agent_mob_tools(temp: &tempfile::TempDir) -> UnifiedRuntime {
    let session_path = temp.path().join("sessions");
    std::fs::create_dir_all(&session_path).expect("session path");
    let session_service: Arc<dyn meerkat_mob::MobSessionService> =
        Arc::new(meerkat::build_ephemeral_service(
            meerkat::AgentFactory::new(&session_path),
            meerkat::Config::default(),
            16,
        ));
    let definition = meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"identity-install-no-tools-{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
        uuid::Uuid::new_v4().simple()
    ))
    .expect("mob definition");
    let spec = MobBootstrapSpec::new(
        definition,
        meerkat_mob::MobStorage::in_memory(),
        session_service,
    )
    .with_options(MobBootstrapOptions {
        allow_ephemeral_sessions: true,
        notify_orchestrator_on_resume: true,
        default_llm_client: Some(Arc::new(meerkat_client::TestClient::default())),
    });
    UnifiedRuntime::bootstrap_with_options(
        spec,
        MetaConfig {
            modules: Vec::new(),
            discovery: DiscoverySpec {
                namespace: "identity-install-no-tools".to_string(),
                modules: Vec::new(),
            },
            pre_spawn: Vec::new(),
        },
        Vec::new(),
        Duration::from_secs(2),
        RuntimeOptions::default(),
        Arc::new(InMemoryMetadataStore::new()),
    )
    .await
    .expect("bootstrap runtime")
}

/// A mob has one identity runtime slot. With an identity context installed
/// on a mob that was built without agent mob tools, the mob wrapper answers
/// the identity runtime its entry's plane answers, and after shutdown both
/// answer none.
#[tokio::test]
async fn with_an_identity_context_and_no_agent_tools_a_mob_answers_one_identity_runtime() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut runtime = runtime_without_agent_mob_tools(&temp).await;
    assert!(
        runtime.mob_runtime().agent_mob_mcp_state().is_none(),
        "the fixture has no agent mob tools"
    );
    assert!(runtime.mob_runtime().installed_identity_runtime().is_none());

    let context = context(&runtime);
    runtime.install_identity_first_context(Arc::clone(&context));
    let plane = runtime.default_mob().identity_plane.attached();
    assert!(
        plane
            .as_ref()
            .is_some_and(|attached| Arc::ptr_eq(attached, &context.runtime)),
        "the entry's plane answers the installed identity runtime"
    );
    let wrapper = runtime.mob_runtime().installed_identity_runtime();
    assert!(
        wrapper
            .as_ref()
            .is_some_and(|installed| Arc::ptr_eq(installed, &context.runtime)),
        "the mob wrapper answers the identity runtime its entry's plane answers; it answered {}",
        if wrapper.is_some() {
            "another runtime"
        } else {
            "none"
        }
    );
    drop((plane, wrapper, context));

    tokio::time::timeout(Duration::from_secs(30), runtime.shutdown())
        .await
        .expect("the installed context shuts down");
    assert!(runtime.default_mob().identity_plane.attached().is_none());
    assert!(runtime.mob_runtime().installed_identity_runtime().is_none());
}

/// The installed context is the public identity view a host binds before
/// bootstrap; bootstrapping it afterwards completes and shuts down clean.
#[tokio::test]
async fn an_installed_context_is_visible_before_bootstrap_starts_anything() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut runtime = runtime(&temp).await;
    assert!(runtime.identity_runtime().is_none());

    let context = context(&runtime);
    runtime.install_identity_first_context(Arc::clone(&context));
    let view = runtime
        .identity_runtime()
        .expect("the installed identity view");
    assert!(
        std::ptr::eq(std::ptr::from_ref(&*view), Arc::as_ptr(&context.runtime)),
        "the installed runtime"
    );
    drop(view);

    runtime
        .bootstrap_installed_identity_first_context(&[])
        .await
        .expect("bootstrap the installed context");
    tokio::time::timeout(Duration::from_secs(30), runtime.shutdown())
        .await
        .expect("the bootstrapped context shuts down");
}

/// A host refusal between the phases leaves the ordinary shutdown to clean
/// the installed context up.
#[tokio::test]
async fn an_installed_context_refused_before_bootstrap_shuts_down_clean() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut runtime = runtime(&temp).await;
    runtime.install_identity_first_context(context(&runtime));
    tokio::time::timeout(Duration::from_secs(30), runtime.shutdown())
        .await
        .expect("the installed context shuts down");
}

/// Bootstrap has nothing to complete without an installed context.
#[tokio::test]
async fn bootstrap_without_an_installed_context_is_refused() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut runtime = runtime(&temp).await;
    assert!(
        runtime
            .bootstrap_installed_identity_first_context(&[])
            .await
            .is_err()
    );
    runtime.shutdown().await;
}

/// On a mob built without agent mob tools the stop detaches the family of
/// the mob's long-term agents from its identity runtime, as on a mob built
/// with them: a holder of the identity runtime reads none after the stop.
#[tokio::test]
async fn the_stop_detaches_the_identity_family_of_a_mob_without_agent_tools() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut runtime = runtime_without_agent_mob_tools(&temp).await;
    let context = context(&runtime);
    runtime.install_identity_first_context(Arc::clone(&context));
    assert!(
        context.runtime.identity_family().is_some(),
        "the install attaches the family's reads"
    );

    tokio::time::timeout(Duration::from_secs(30), runtime.shutdown())
        .await
        .expect("the installed context shuts down");
    assert!(
        context.runtime.identity_family().is_none(),
        "the stop detached the family from the identity runtime"
    );
}

/// The `identity_changes.available` member of `GET /console/experience`.
async fn console_offers_identity_changes(app: &axum::Router) -> Option<bool> {
    use tower::ServiceExt;
    let request = axum::http::Request::builder()
        .method("GET")
        .uri("/console/experience")
        .body(axum::body::Body::empty())
        .expect("experience request");
    let response = tokio::time::timeout(Duration::from_secs(30), app.clone().oneshot(request))
        .await
        .ok()?
        .ok()?;
    if response.status() != axum::http::StatusCode::OK {
        return None;
    }
    let body = axum::body::to_bytes(response.into_body(), 1 << 22)
        .await
        .ok()?;
    let body: serde_json::Value = serde_json::from_slice(&body).ok()?;
    body.pointer("/identity_changes/available")
        .and_then(serde_json::Value::as_bool)
}

/// On a mob built without agent mob tools the Console reads the mob's
/// identity runtime where its router was built before the identity context
/// was installed: it offers changes of the mob's long-term agents once the
/// host attached an identity policy, and not before.
#[tokio::test]
async fn the_console_offers_identity_changes_on_a_mob_without_agent_tools() {
    let temp = tempfile::tempdir().expect("tempdir");
    let mut runtime = runtime_without_agent_mob_tools(&temp).await;
    let app =
        runtime.build_reference_app_router(crate::runtime::RuntimeDecisionState::local_console(
            crate::ConsolePolicy {
                require_app_auth: false,
                ..Default::default()
            },
            None,
        ));
    assert_eq!(
        console_offers_identity_changes(&app).await,
        Some(false),
        "no identity context: the Console offers no change"
    );

    runtime.install_identity_first_context(context(&runtime));
    assert_eq!(
        console_offers_identity_changes(&app).await,
        Some(false),
        "an identity context without an identity policy: the reads only"
    );

    let policy = crate::identity_creation::IdentityCreationPolicy::parse(&serde_json::json!({
        "enabled": true,
        "approval": {"rules": [{"change": ["archive"], "outcome": {"kind": "allow"}}]},
    }))
    .expect("the policy parses")
    .expect("the policy is enabled");
    assert!(runtime.attach_identity_policy(
        Arc::new(policy),
        crate::identity_creation::IdentityFamilyOptions::new()
    ));
    assert_eq!(
        console_offers_identity_changes(&app).await,
        Some(true),
        "with the host's identity policy attached the Console offers the changes"
    );

    drop(app);
    tokio::time::timeout(Duration::from_secs(30), runtime.shutdown())
        .await
        .expect("the installed context shuts down");
}

/// On a mob built without agent mob tools the Console's access priming
/// reads the mob's identity runtime: an agent the host configures that
/// is not seated is decided by the access rules, on the profile and labels
/// it is registered with, and not shown to administrators only.
#[tokio::test]
async fn the_access_rules_decide_a_configured_agent_of_a_mob_without_agent_tools() {
    use crate::access::{ACTION_AGENT_VIEW, AccessControlConfig, AccessController, AccessRule};
    const VIEWER: &str = "viewer@example.test";
    const CONFIGURED: &str = "desk";

    let temp = tempfile::tempdir().expect("tempdir");
    let mut runtime = runtime_without_agent_mob_tools(&temp).await;
    let controller = AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec!["root@example.test".to_string()],
        rules: vec![AccessRule {
            id: "viewer-sees-workers".to_string(),
            subjects: vec![VIEWER.to_string()],
            actions: vec![ACTION_AGENT_VIEW.to_string()],
            roles: vec!["worker".to_string()],
            ..AccessRule::default()
        }],
        ..AccessControlConfig::default()
    })
    .expect("access configuration");
    runtime.set_access_controller(controller.clone());

    let context = context(&runtime);
    context
        .runtime
        .register_locked(configured_agent(CONFIGURED))
        .await;

    // Without the identity context the priming has no registration to
    // read: the agent is one the rules know nothing of.
    crate::http_console::prime_access_cache_from_runtime(runtime.mob_runtime(), &controller).await;
    let viewer = controller.view_for_subject(Some(VIEWER));
    assert!(
        !viewer.can_view_agent(CONFIGURED),
        "an agent the access cache does not hold is not shown to a person who is no \
         administrator"
    );

    runtime.install_identity_first_context(Arc::clone(&context));
    crate::http_console::prime_access_cache_from_runtime(runtime.mob_runtime(), &controller).await;
    let viewer = controller.view_for_subject(Some(VIEWER));
    assert!(
        viewer.knows_agent(CONFIGURED),
        "the priming read the configured agent from the mob's identity runtime"
    );
    assert!(
        viewer.can_view_agent(CONFIGURED),
        "the rule that names the agent's profile decides it"
    );
    assert!(
        !viewer.can_view_agent("a-name-no-agent-holds"),
        "a name the cache does not hold stays with administrators"
    );

    drop(context);
    tokio::time::timeout(Duration::from_secs(30), runtime.shutdown())
        .await
        .expect("the installed context shuts down");
}
