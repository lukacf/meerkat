//! A held identity bootstrap: a fresh roster member is constructed and held
//! before its publication while the host reads its finished surface; the
//! host then publishes it or aborts it. Core's mob is the member's only
//! custody: an aborted member is never published, so Core's member history
//! gains no binding for the identity and a later held pass holds it again.
//! Every custody assertion reads Core (its roster and its member history),
//! never a Meta store. Turn-driven members on a machine whose grant owner
//! refuses all work: no first prompt is admitted.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use meerkat_authorization::clock::HostAuthorizationClock;
use meerkat_authorization::grant_policy::{
    AdmittedWorkPolicyOwner, OperationPolicyOwner, WorkOwnerAllowance,
};
use meerkat_authorization::grants::{LocalGrantAuthority, LocalGrantConfiguration};
use meerkat_authorization::policy::{LocalPolicyAllowance, LocalPolicyPurpose};
use meerkat_authorization::publication::LocalAuthorizationPublication;
use meerkat_authorization_contracts::evidence::EvidenceId;
use meerkat_authorization_contracts::work_association::InputAuthorityAssociation;
use meerkat_core::auth::{PrincipalKind, PrincipalRef};
use meerkat_core::{OperationAuthorizationError, PreparedAuthorizationBinding};
use meerkat_meta::UnifiedRuntime;
use meerkat_meta::identity_first::{
    AgentAddressability, AgentIdentity, ContinuityStore, DurabilityPolicy, DurableAgentSpec,
    IdentityFirstRuntimeContext, IdentityRuntime, IdentityRuntimeConfig, LocalContinuityStore,
    LocalLeaseProvider, MobSessionBridge, RosterContext, RosterError, RosterProvider,
    SessionBridge,
};
use meerkat_runtime::input_authority::NativeAdmissionError;
use meerkat_runtime::meerkat_machine::NativeGrantWorkConfiguration;

struct RefusingOwner;

impl AdmittedWorkPolicyOwner for RefusingOwner {
    fn authorize_admitted_work(
        &self,
        _association: &InputAuthorityAssociation,
        _binding: &PreparedAuthorizationBinding,
        _purpose: LocalPolicyPurpose,
        _now_ms: u64,
    ) -> Result<WorkOwnerAllowance, OperationAuthorizationError> {
        Err(OperationAuthorizationError::Unavailable)
    }
}

impl OperationPolicyOwner for RefusingOwner {
    fn authorize_operation(
        &self,
        _association: &InputAuthorityAssociation,
        _binding: &PreparedAuthorizationBinding,
        _purpose: LocalPolicyPurpose,
        _now_ms: u64,
    ) -> Result<LocalPolicyAllowance, OperationAuthorizationError> {
        Err(OperationAuthorizationError::Unavailable)
    }
}

fn principal(name: &str) -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::ServiceAccount,
        name,
        meerkat_core::TrustDomainId::new("meta-held").expect("trust domain"),
    )
    .expect("service principal")
}

fn host_configuration() -> NativeGrantWorkConfiguration {
    let ingress: Arc<meerkat_runtime::meerkat_machine::NativeIngressCheck> =
        Arc::new(|_, _, _, _| -> Result<(), NativeAdmissionError> {
            Err(OperationAuthorizationError::Unavailable.into())
        });
    NativeGrantWorkConfiguration {
        grants: Arc::new(
            LocalGrantAuthority::new(
                LocalGrantConfiguration {
                    root: principal("grant-owner"),
                    namespace: EvidenceId::new("meta-held-grants").expect("namespace"),
                    generation: 1,
                },
                LocalAuthorizationPublication::new(),
                Arc::new(HostAuthorizationClock),
            )
            .expect("host grant owner"),
        ),
        ingress,
        invocation_owner: Arc::new(RefusingOwner),
        operation_owner: Arc::new(RefusingOwner),
        retained_resume: None,
        require_scoped_destination: true,
        configured_mcp_attachment: None,
    }
}

struct AllowsEverything;

impl meerkat_core::ToolConsequencePolicySnapshot for AllowsEverything {
    fn provenance(&self) -> meerkat_core::PolicyEvaluationProvenance {
        meerkat_core::PolicyEvaluationProvenance {
            revision: meerkat_core::PolicyRevision(1),
            digest: meerkat_core::PolicyDigest::from_canonical_bytes(b"allows-everything"),
        }
    }

    fn evaluate(
        &self,
        _request: &meerkat_core::ToolConsequenceRequest,
    ) -> meerkat_core::ToolConsequenceVerdict {
        meerkat_core::ToolConsequenceVerdict::Allow
    }
}

struct HostPolicy(meerkat_core::PolicyProviderId);

impl meerkat_core::ToolConsequenceNarrowingPolicy for HostPolicy {
    fn provider_id(&self) -> &meerkat_core::PolicyProviderId {
        &self.0
    }

    fn generation(&self) -> meerkat_core::PolicyProviderGeneration {
        meerkat_core::PolicyProviderGeneration(1)
    }

    fn snapshot(
        &self,
        _policy_id: &meerkat_core::PolicyId,
    ) -> Result<
        Arc<dyn meerkat_core::ToolConsequencePolicySnapshot>,
        meerkat_core::ToolConsequenceFailure,
    > {
        Ok(Arc::new(AllowsEverything))
    }
}

fn host_registry() -> Arc<meerkat_core::ToolConsequencePolicyRegistry> {
    Arc::new(
        meerkat_core::ToolConsequencePolicyRegistry::new(
            vec![Arc::new(HostPolicy(
                meerkat_core::PolicyProviderId::new("host").expect("provider id"),
            ))],
            meerkat_core::PolicyEvaluationSupervisorConfig::default(),
            None,
        )
        .expect("host policy registry"),
    )
}

fn host_binding() -> meerkat_core::ApplicationToolPolicyBinding {
    meerkat_core::ApplicationToolPolicyBinding::Provider {
        provider_id: meerkat_core::PolicyProviderId::new("host").expect("provider id"),
        policy_id: meerkat_core::PolicyId::new("member-tools").expect("policy id"),
    }
}

/// Selects each member's own session document as its context and the
/// host's policy for its actual prepared surface. A parking hook holds each
/// destination selection (inside the member's construction) until released.
#[derive(Default)]
struct SelectingHook {
    prepared: AtomicUsize,
    entered: AtomicUsize,
    parked: Option<tokio::sync::watch::Receiver<bool>>,
}

impl SelectingHook {
    fn parking(release: tokio::sync::watch::Receiver<bool>) -> Self {
        Self {
            parked: Some(release),
            ..Self::default()
        }
    }
}

fn destination(
    identity: &meerkat_mob::MemberRuntimeBuildIdentity,
) -> meerkat_mob::MemberDestinationSelection {
    let owner = PrincipalRef::in_domain(
        PrincipalKind::SharedAgent,
        identity.member().member.as_str(),
        meerkat_core::TrustDomainId::new("meta-held").expect("trust domain"),
    )
    .expect("member principal");
    meerkat_mob::MemberDestinationSelection {
        logical_owner: owner.clone(),
        context: identity.session_document().clone(),
        context_generation: 1,
        audience: meerkat_mob::AudienceRef::Principal { principal: owner },
    }
}

#[async_trait::async_trait]
impl meerkat_mob::MemberRuntimeRegistrationHook for SelectingHook {
    async fn select_destination(
        &self,
        identity: &meerkat_mob::MemberRuntimeBuildIdentity,
    ) -> Result<meerkat_mob::MemberDestinationSelection, meerkat_mob::MobError> {
        if let Some(release) = self.parked.as_ref() {
            self.entered.fetch_add(1, Ordering::SeqCst);
            let _ = release.clone().wait_for(|released| *released).await;
        }
        Ok(destination(identity))
    }

    async fn expected_destination(
        &self,
        identity: &meerkat_mob::MemberRuntimeBuildIdentity,
    ) -> Result<meerkat_mob::MemberDestinationSelection, meerkat_mob::MobError> {
        Ok(destination(identity))
    }

    async fn verify_member_surface(
        &self,
        _identity: &meerkat_mob::MemberRuntimeBuildIdentity,
        _surface: &[meerkat_core::ToolDef],
    ) -> Result<(), meerkat_mob::MobError> {
        Ok(())
    }

    async fn prepare_member_surface(
        &self,
        _identity: &meerkat_mob::MemberRuntimeBuildIdentity,
        surface: &meerkat_core::PreparedToolSurface,
    ) -> Result<meerkat_core::ToolSurfaceSelection, meerkat_mob::MobError> {
        self.prepared.fetch_add(1, Ordering::SeqCst);
        Ok(meerkat_core::ToolSurfaceSelection {
            binding: host_binding(),
            compiled_against: surface.clone(),
        })
    }
}

const MODEL: &str = "gpt-5.5";
const ROUTE_ENDPOINT: &str = "https://meta-held-route.invalid/v1/responses";
const ROUTE_WIRE_MODEL: &str = "gpt-5.5-routed";

/// The controller selection a [`RoutedAgentClient`] pins for the member model.
fn route_selection() -> meerkat_core::ControllerModelSelection {
    meerkat_core::ControllerModelSelection::new(
        meerkat_core::SessionLlmIdentity {
            model: MODEL.into(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        },
        serde_json::from_value(
            serde_json::json!({"realm": "meta-held", "account": "routed-controller"}),
        )
        .expect("nonsecret identity of the fixture transport"),
        "routed-profile".into(),
        "routed-transport".into(),
    )
}

/// The request-free route facts of [`route_selection`].
fn route_facts() -> meerkat_core::ControllerModelFacts {
    meerkat_core::ControllerModelFacts::new(
        route_selection(),
        Arc::from(ROUTE_ENDPOINT),
        Arc::from(ROUTE_WIRE_MODEL),
    )
}

/// A member's agent client with a controller it pins: the selection and
/// request-free route facts a governed provider client reports. Everything
/// else is the wrapped client's; held members run no turn.
struct RoutedAgentClient {
    inner: Arc<dyn meerkat_core::AgentLlmClient>,
}

#[async_trait::async_trait]
impl meerkat_core::AgentLlmClient for RoutedAgentClient {
    fn controller_model_selection(&self) -> Option<meerkat_core::ControllerModelSelection> {
        Some(route_selection())
    }

    fn controller_model_facts(
        &self,
    ) -> Result<meerkat_core::ControllerModelFacts, meerkat_core::ControllerFactsUnavailable> {
        Ok(route_facts())
    }

    fn pin_controller(self: Arc<Self>) -> Option<meerkat_core::ControllerModelClient> {
        Some(meerkat_core::ControllerModelClient::new(
            route_selection(),
            self,
        ))
    }

    fn request_attempt_authority(&self) -> meerkat_core::RequestAttemptAuthority {
        self.inner.request_attempt_authority()
    }

    async fn stream_response(
        &self,
        messages: &[meerkat_core::Message],
        tools: &[Arc<meerkat_core::ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&meerkat_core::lifecycle::run_primitive::ProviderParamsOverride>,
    ) -> Result<meerkat_core::agent::LlmStreamResult, meerkat_core::AgentError> {
        self.inner
            .stream_response(messages, tools, max_tokens, temperature, provider_params)
            .await
    }

    fn provider(&self) -> meerkat_core::Provider {
        self.inner.provider()
    }

    fn model(&self) -> &str {
        self.inner.model()
    }
}

/// A host session hook whose members' agent clients are
/// [`RoutedAgentClient`]s, under Meta's own member decorators.
struct RoutedControllerHook;

#[async_trait::async_trait]
impl meerkat_meta::SessionHook for RoutedControllerHook {
    async fn before_create(
        &self,
        req: &mut meerkat_core::service::CreateSessionRequest,
    ) -> Result<(), meerkat_core::service::SessionError> {
        req.build
            .get_or_insert_with(Default::default)
            .agent_llm_client_decorator =
            Some(Arc::new(|inner| Arc::new(RoutedAgentClient { inner })));
        Ok(())
    }
}

/// The raw transport form of [`RoutedAgentClient`]: a provider client
/// reporting the same selection and request-free route for the member
/// model. As the runtime's default client it reaches the member's adapter
/// through Meta's raw client wrappers. Held members run no turn.
struct RoutedRawClient;

#[async_trait::async_trait]
impl meerkat_llm_core::LlmClient for RoutedRawClient {
    fn controller_model_selection(&self) -> Option<meerkat_core::ControllerModelSelection> {
        Some(route_selection())
    }

    fn plain_model_route(
        &self,
        logical_model: &str,
    ) -> Result<meerkat_llm_core::PlainModelRoute, meerkat_core::ControllerFactsUnavailable> {
        if logical_model != MODEL {
            return Err(meerkat_core::ControllerFactsUnavailable);
        }
        meerkat_llm_core::PlainModelRoute::new(ROUTE_ENDPOINT, ROUTE_WIRE_MODEL)
    }

    fn stream<'a>(
        &'a self,
        _request: &'a meerkat_llm_core::LlmRequest,
    ) -> meerkat_llm_core::LlmStream<'a> {
        panic!("a held member runs no turn")
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::OpenAI
    }

    async fn health_check(&self) -> Result<(), meerkat_llm_core::LlmError> {
        Ok(())
    }
}

struct Roster(Vec<DurableAgentSpec>);

#[async_trait::async_trait]
impl RosterProvider for Roster {
    async fn roster(&self, _context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        Ok(self.0.clone())
    }
}

fn member(identity: &AgentIdentity) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: identity.clone(),
        profile: "worker".into(),
        addressability: AgentAddressability::Addressable,
        display_name: None,
        labels: BTreeMap::new(),
        context: None,
        additional_instructions: Vec::new(),
        initial_message: None,
        runtime_mode_override: Some(meerkat_mob::MobRuntimeMode::TurnDriven),
        backend: None,
        binding: None,
        placement: None,
    }
}

/// `identity`'s latest binding in Core's member history since the latest
/// mob reset; `None` when Core never spawned it since then.
async fn core_last_binding(
    runtime: &UnifiedRuntime,
    identity: &AgentIdentity,
) -> Option<meerkat_mob::MemberBinding> {
    MobSessionBridge::new(runtime.mob_handle())
        .last_member_binding(identity)
        .await
        .expect("Core's member history reads")
}

/// Whether Core's member history binds `identity` to a live member.
async fn core_binds_live(runtime: &UnifiedRuntime, identity: &AgentIdentity) -> bool {
    core_last_binding(runtime, identity)
        .await
        .is_some_and(|binding| binding.state() == meerkat_mob::MemberBindingState::Live)
}

/// A governed runtime with the identity context installed, not bootstrapped.
async fn installed_runtime(
    dir: &tempfile::TempDir,
    mob_id: &str,
    roster: &[DurableAgentSpec],
    hook: Arc<SelectingHook>,
) -> UnifiedRuntime {
    installed_runtime_with(dir, mob_id, roster, hook, None, None).await
}

/// The fake OpenAI key members built on the provider default resolve their
/// provider identity with: the same value the gateway fixtures give their
/// child processes, not a secret.
const FIXTURE_OPENAI_KEY: &str = "sk-regression-test";

/// Agent config whose global realm binds OpenAI to [`FIXTURE_OPENAI_KEY`],
/// so members never depend on an ambient `OPENAI_API_KEY`. They never call
/// the provider: no prompt is admitted on this machine.
fn fixture_provider_config() -> meerkat::Config {
    let mut config = meerkat::Config::default();
    config.realm.insert(
        meerkat_core::connection::GLOBAL_REALM_SLUG.to_string(),
        meerkat_core::RealmConfigSection::from_inline_api_keys(&[("openai", FIXTURE_OPENAI_KEY)]),
    );
    config
}

/// [`installed_runtime`] whose members are built on `client` and created
/// through `session_hook`, when given. Without a client, members are built
/// on the provider default with the fixture's own key.
async fn installed_runtime_with(
    dir: &tempfile::TempDir,
    mob_id: &str,
    roster: &[DurableAgentSpec],
    hook: Arc<SelectingHook>,
    client: Option<Arc<dyn meerkat_llm_core::LlmClient>>,
    session_hook: Option<Arc<dyn meerkat_meta::SessionHook>>,
) -> UnifiedRuntime {
    let definition = meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"{mob_id}\"\n\n[profiles.worker]\nmodel = \"{MODEL}\"\nruntime_mode = \"turn_driven\"\n\n[profiles.worker.tools]\ncomms = true\n"
    ))
    .expect("mob definition");
    let mut builder = UnifiedRuntime::builder()
        .definition(definition)
        .persistent_state(dir.path().join("state"))
        .native_grant_work_configuration(host_configuration())
        .member_runtime_registration_hook(hook)
        .tool_consequence_policy_registry(host_registry());
    builder = match client {
        Some(client) => builder.default_llm_client(client),
        None => builder.meerkat_config(fixture_provider_config()),
    };
    if let Some(session_hook) = session_hook {
        builder = builder.session_hook(session_hook);
    }
    let mut runtime = builder.build().await.expect("build");
    // The identity runtime ignores the continuity store and the lease
    // provider (Core's mob is the authority); the config still names them.
    let identity_runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: Arc::new(LocalContinuityStore::in_memory().expect("continuity store"))
            as Arc<dyn ContinuityStore>,
        lease_provider: Arc::new(LocalLeaseProvider::new()) as _,
        runtime_instance_id: format!("{mob_id}-instance"),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(Arc::new(MobSessionBridge::new(runtime.mob_handle()))),
        default_timeout: None,
    }));
    // The held-completion marks live in the state directory, where the
    // builder and the gateway put them.
    identity_runtime.set_held_completion_state_dir(Some(dir.path().join("state")));
    runtime.install_identity_first_context(Arc::new(IdentityFirstRuntimeContext::new(
        identity_runtime,
        Arc::new(Roster(roster.to_vec())),
        None,
        None,
        Some(runtime.mob_handle().definition().clone()),
    )));
    runtime
}

fn mob_id(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::new_v4().simple())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_held_roster_member_is_published_after_its_host_completes() {
    let dir = tempfile::tempdir().unwrap();
    let identity = AgentIdentity::parse("held-activate").unwrap();
    let roster = vec![member(&identity)];
    let hook = Arc::new(SelectingHook::default());
    let mut runtime =
        installed_runtime(&dir, &mob_id("held-activate"), &roster, hook.clone()).await;
    let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = Arc::clone(&observed);
    let result = runtime
        .bootstrap_installed_identity_first_context_held(
            &roster,
            tokio::time::Instant::now() + Duration::from_mins(1),
            Box::new(move |members, _completion| {
                Box::pin(async move {
                    *seen.lock().unwrap() = members;
                    Ok(())
                })
            }),
        )
        .await;
    result.expect("the held roster bootstraps");
    let members = observed.lock().unwrap().clone();
    assert_eq!(members.len(), 1, "the fresh member was held");
    assert_eq!(members[0].identity, identity);
    assert_eq!(members[0].finished_surface.binding, host_binding());
    assert_eq!(hook.prepared.load(Ordering::SeqCst), 1, "constructed once");
    assert_eq!(
        runtime.mob_handle().list_members().await.len(),
        1,
        "the activated member is published"
    );
    assert!(
        core_binds_live(&runtime, &identity).await,
        "Core's member history binds the published member"
    );
    let _ = runtime.shutdown().await;
}

/// Bootstrap one fresh member held on `client`, the runtime's default
/// client, created through `session_hook` when given, and activate it;
/// returns the runtime and the held members its host received.
async fn held_and_activated(
    dir: &tempfile::TempDir,
    prefix: &str,
    client: Arc<dyn meerkat_llm_core::LlmClient>,
    session_hook: Option<Arc<dyn meerkat_meta::SessionHook>>,
) -> (
    UnifiedRuntime,
    Vec<meerkat_meta::identity_first::HeldIdentityMember>,
) {
    let identity = AgentIdentity::parse(prefix).unwrap();
    let roster = vec![member(&identity)];
    let mut runtime = installed_runtime_with(
        dir,
        &mob_id(prefix),
        &roster,
        Arc::new(SelectingHook::default()),
        Some(client),
        session_hook,
    )
    .await;
    let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = Arc::clone(&observed);
    runtime
        .bootstrap_installed_identity_first_context_held(
            &roster,
            tokio::time::Instant::now() + Duration::from_mins(1),
            Box::new(move |members, _completion| {
                Box::pin(async move {
                    *seen.lock().unwrap() = members;
                    Ok(())
                })
            }),
        )
        .await
        .expect("the held roster bootstraps");
    let members = observed.lock().unwrap().clone();
    assert_eq!(members.len(), 1, "the fresh member was held");
    assert_eq!(members[0].identity, identity);
    (runtime, members)
}

/// A raw test client that pins no controller.
fn test_client() -> Arc<dyn meerkat_llm_core::LlmClient> {
    Arc::new(meerkat_client::TestClient::for_provider(
        meerkat_core::Provider::OpenAI,
    ))
}

/// The route the session owner's own pin of the one published member's
/// actor reports.
async fn session_owner_route(runtime: &UnifiedRuntime) -> meerkat_mob::ControllerRouteObservation {
    let published = runtime
        .mob_handle()
        .list_members()
        .await
        .pop()
        .expect("the activated member is published");
    let session_id = runtime
        .mob_handle()
        .get_member(&published.agent_identity)
        .await
        .expect("roster read")
        .expect("the published member")
        .bridge_session_id()
        .cloned()
        .expect("a session-backed member");
    let owner = runtime
        .mob_runtime()
        .controller_pin_source()
        .expect("the concrete session owner");
    let actor = owner
        .live_actor(&session_id)
        .await
        .expect("the published actor");
    let pinned = owner
        .pin_actor(&actor)
        .await
        .expect("the published actor's own pin");
    meerkat_mob::ControllerRouteObservation::Reported(
        meerkat::controller_route::controller_route_report(&pinned)
            .expect("the owner's pinned client reports its route"),
    )
}

/// A held member carries the controller route its actor's actual pinned
/// client reported under the hold, through Meta's session-service
/// decorators: that client's route, and the same report the session owner's
/// own pin of the published (same) actor gives.
#[tokio::test(flavor = "multi_thread")]
async fn a_held_member_carries_the_route_its_actor_pins() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, members) = held_and_activated(
        &dir,
        "held-route",
        test_client(),
        Some(Arc::new(RoutedControllerHook)),
    )
    .await;
    let report = members[0]
        .controller_route
        .report()
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "the actual pinned client's route report, not {:?}",
                members[0].controller_route
            )
        });
    assert_eq!(
        report.key,
        route_facts().route_key(),
        "the pinned client's route"
    );
    assert_eq!(report.model, MODEL);
    assert_eq!(report.wire_model, ROUTE_WIRE_MODEL);
    assert_eq!(report.endpoint, ROUTE_ENDPOINT);
    assert_eq!(
        members[0].controller_route,
        session_owner_route(&runtime).await,
        "the held member carries exactly what the session owner reports"
    );
    let _ = runtime.shutdown().await;
}

/// A held member built on a raw default client carries the route that
/// client's controller reports: Meta's raw client wrappers carry the
/// client's actual controller selection and request-free route to the
/// member's actor, which pins it.
#[tokio::test(flavor = "multi_thread")]
async fn a_held_member_on_a_raw_default_client_carries_its_route() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, members) =
        held_and_activated(&dir, "held-raw-route", Arc::new(RoutedRawClient), None).await;
    let report = members[0]
        .controller_route
        .report()
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "the raw client's route report, not {:?}",
                members[0].controller_route
            )
        });
    let route = meerkat_llm_core::PlainModelRoute::new(ROUTE_ENDPOINT, ROUTE_WIRE_MODEL)
        .expect("the fixture route");
    assert_eq!(
        report.key,
        meerkat_core::ControllerModelFacts::new(
            route_selection(),
            Arc::from(route.endpoint()),
            Arc::from(route.wire_model()),
        )
        .route_key(),
        "the raw client's lowered route"
    );
    assert_eq!(report.model, MODEL);
    assert_eq!(report.wire_model, ROUTE_WIRE_MODEL);
    assert_eq!(
        members[0].controller_route,
        session_owner_route(&runtime).await,
        "the held member carries exactly what the session owner reports"
    );
    let _ = runtime.shutdown().await;
}

/// A member whose client pins no controller (a raw test client reports no
/// selection) is held with the session owner's typed `Unavailable`,
/// unchanged: neither a substituted report nor the decorators' `Unsupported`
/// default.
#[tokio::test(flavor = "multi_thread")]
async fn a_held_member_without_route_facts_carries_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, members) = held_and_activated(&dir, "held-no-route", test_client(), None).await;
    assert_eq!(
        members[0].controller_route,
        meerkat_mob::ControllerRouteObservation::Unavailable
    );
    let _ = runtime.shutdown().await;
}

/// A host that refuses its application aborts the held member: Core never
/// publishes it, so Core's member history gains no binding for the identity.
/// The failed bootstrap shuts its runtime down; the next runtime over the
/// same state directory and mob finds the identity fresh (a binding would
/// refuse the pass as `ExistingContinuity`) and holds it again.
#[tokio::test(flavor = "multi_thread")]
async fn an_aborted_held_roster_member_leaves_no_binding_and_is_held_again() {
    let identity = AgentIdentity::parse("held-abort").unwrap();
    let roster = vec![member(&identity)];
    let dir = tempfile::tempdir().unwrap();
    let mob = mob_id("held-abort");

    let mut runtime =
        installed_runtime(&dir, &mob, &roster, Arc::new(SelectingHook::default())).await;
    let held = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&held);
    let error = runtime
        .bootstrap_installed_identity_first_context_held(
            &roster,
            tokio::time::Instant::now() + Duration::from_mins(1),
            Box::new(move |members, _completion| {
                Box::pin(async move {
                    counted.store(members.len(), Ordering::SeqCst);
                    Err("the application bundle failed".to_string())
                })
            }),
        )
        .await
        .expect_err("an aborted held bootstrap fails");
    assert!(
        error.to_string().contains("the application bundle failed"),
        "{error}"
    );
    assert_eq!(held.load(Ordering::SeqCst), 1, "the fresh member was held");
    assert!(
        runtime.mob_handle().list_members().await.is_empty(),
        "the aborted member is not published"
    );
    // The failed bootstrap already shut this runtime down. Dropping it also
    // releases the state directory's claim before the next runtime opens the
    // same directory, even if that shutdown did not run to its end.
    let _ = runtime.shutdown().await;
    drop(runtime);

    // The same state directory and mob: Core's member history kept no
    // binding, so the next held pass holds the identity again.
    let mut runtime =
        installed_runtime(&dir, &mob, &roster, Arc::new(SelectingHook::default())).await;
    let held = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&held);
    runtime
        .bootstrap_installed_identity_first_context_held(
            &roster,
            tokio::time::Instant::now() + Duration::from_mins(1),
            Box::new(move |members, _completion| {
                Box::pin(async move {
                    counted.store(members.len(), Ordering::SeqCst);
                    Ok(())
                })
            }),
        )
        .await
        .expect("the identity is held again and published");
    assert_eq!(
        held.load(Ordering::SeqCst),
        1,
        "the identity was held again"
    );
    assert_eq!(runtime.mob_handle().list_members().await.len(), 1);
    assert!(
        core_binds_live(&runtime, &identity).await,
        "Core's member history binds the published member"
    );
    let _ = runtime.shutdown().await;
}

/// Hold `roster` with a held bootstrap whose host refuses its completion:
/// the pass aborts, its members are never published (Core binds none of
/// them), and the failed bootstrap shuts the runtime down.
async fn abort_held_bootstrap(runtime: &mut UnifiedRuntime, roster: &[DurableAgentSpec]) {
    runtime
        .bootstrap_installed_identity_first_context_held(
            roster,
            tokio::time::Instant::now() + Duration::from_mins(1),
            Box::new(|_members, _completion| {
                Box::pin(async { Err("the application bundle failed".to_string()) })
            }),
        )
        .await
        .expect_err("an aborted held bootstrap fails");
}

/// Add `spec` held to the running roster of `context`, with a host that
/// refuses its completion: the add aborts, and its member is never
/// published.
async fn abort_held_add(
    context: &meerkat_meta::identity_first::PublicIdentityFirstContext,
    spec: DurableAgentSpec,
) {
    let start = context
        .add_member_held(
            spec,
            tokio::time::Instant::now() + Duration::from_mins(1),
            |_members, _completion| async { Err("the application bundle failed".to_string()) },
        )
        .await
        .expect("the held add starts");
    match start {
        meerkat_meta::identity_first::HeldBootstrapStart::Unsettled(settlement) => {
            settlement
                .settled()
                .await
                .expect_err("the host's refusal aborts the add");
        }
        meerkat_meta::identity_first::HeldBootstrapStart::Activated(result) => {
            panic!("the refused add activated: {:?}", result.outcomes)
        }
    }
}

/// Whether `error` is the typed refusal of an identity that owes its host a
/// held completion.
fn owes_held_completion(error: &meerkat_meta::identity_first::IdentityRuntimeError) -> bool {
    matches!(
        error,
        meerkat_meta::identity_first::IdentityRuntimeError::HeldCompletionOwed { .. }
    )
}

/// A held pass that never published its member leaves Core no binding of
/// it, and its mark only as the rollback mirror: written for a rollback to
/// 0.9.10, never read here. Held creation follows the host's hold in each
/// process, so an ordinary bootstrap of the next runtime over the same state
/// directory holds nothing: the identity is seated and created as any other
/// identity, and the mark stays as it was.
///
/// RED before the marks stopped being read: the ordinary bootstrap refused
/// the identity from its mark (`HeldCompletionOwed`).
#[tokio::test(flavor = "multi_thread")]
async fn an_aborted_held_member_is_seated_normally_after_an_ordinary_restart() {
    let identity = AgentIdentity::parse("held-ordinary-restart").unwrap();
    let roster = vec![member(&identity)];
    let dir = tempfile::tempdir().unwrap();
    let mob = mob_id("held-ordinary-restart");
    let marks = dir
        .path()
        .join("state")
        .join(meerkat_meta::identity_first::HELD_COMPLETION_OWED_DIR);
    let marked = || std::fs::read_dir(&marks).map(Iterator::count).unwrap_or(0);

    let mut runtime =
        installed_runtime(&dir, &mob, &roster, Arc::new(SelectingHook::default())).await;
    abort_held_bootstrap(&mut runtime, &roster).await;
    assert_eq!(marked(), 1, "the rollback mirror marks the aborted member");
    let _ = runtime.shutdown().await;
    drop(runtime);

    // An ordinary bootstrap over the same directory: its host holds nothing.
    let mut runtime =
        installed_runtime(&dir, &mob, &roster, Arc::new(SelectingHook::default())).await;
    let mut restored = runtime
        .bootstrap_installed_identity_first_context(&roster)
        .await
        .expect("the ordinary bootstrap runs");
    let outcome = restored.outcomes.remove(&identity);
    assert!(
        matches!(
            &outcome,
            Some(
                meerkat_meta::identity_first::RestoreOutcome::Created { .. }
                    | meerkat_meta::identity_first::RestoreOutcome::Dormant { .. }
            )
        ),
        "{outcome:?}"
    );
    let identities = runtime.identity_runtime().expect("the identity runtime");
    identities
        .materialize(&identity)
        .await
        .expect("the identity is created as any other");
    drop(identities);
    assert!(
        core_binds_live(&runtime, &identity).await,
        "Core binds the created member"
    );
    assert_eq!(
        marked(),
        1,
        "nothing here reads or removes the rollback mirror"
    );
    let _ = runtime.shutdown().await;
}

/// A held pass that never published its member, and a restart whose host
/// holds the identity again through its held bootstrap: the identity is
/// still owed to its host's held protocol. The restart's pass holds it
/// again, fresh (Core never created it), and Core binds nothing for it until
/// that pass publishes it.
#[tokio::test(flavor = "multi_thread")]
async fn an_aborted_held_member_stays_owed_to_a_restart_whose_bootstrap_holds_it_again() {
    let identity = AgentIdentity::parse("held-bootstrap-again").unwrap();
    let roster = vec![member(&identity)];
    let dir = tempfile::tempdir().unwrap();
    let mob = mob_id("held-bootstrap-again");

    let mut runtime =
        installed_runtime(&dir, &mob, &roster, Arc::new(SelectingHook::default())).await;
    abort_held_bootstrap(&mut runtime, &roster).await;
    let _ = runtime.shutdown().await;
    drop(runtime);

    let mut runtime =
        installed_runtime(&dir, &mob, &roster, Arc::new(SelectingHook::default())).await;
    let mob_handle = runtime.mob_handle().clone();
    let held = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = Arc::clone(&held);
    let unbound_while_held = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let unbound = Arc::clone(&unbound_while_held);
    let held_identity = identity.clone();
    runtime
        .bootstrap_installed_identity_first_context_held(
            &roster,
            tokio::time::Instant::now() + Duration::from_mins(1),
            Box::new(move |members, _completion| {
                Box::pin(async move {
                    *seen.lock().unwrap() = members
                        .iter()
                        .map(|member| member.identity.clone())
                        .collect::<Vec<_>>();
                    let binding = MobSessionBridge::new(mob_handle)
                        .last_member_binding(&held_identity)
                        .await
                        .map_err(|error| error.to_string())?;
                    unbound.store(binding.is_none(), Ordering::SeqCst);
                    Ok(())
                })
            }),
        )
        .await
        .expect("the restart's held pass publishes the member");
    assert_eq!(
        held.lock().unwrap().clone(),
        vec![identity.clone()],
        "the restart's pass held the identity again"
    );
    assert!(
        unbound_while_held.load(Ordering::SeqCst),
        "Core bound nothing for the identity before its held pass published it"
    );
    assert!(
        core_binds_live(&runtime, &identity).await,
        "the held pass published the member"
    );
    let _ = runtime.shutdown().await;
}

/// A held add the host's completion refuses never publishes its member.
/// Within the add's process the identity stays owed to its host's held
/// protocol: every other wake and the reset are refused typed
/// (`HeldCompletionOwed`), a delete finds nothing Core spawned, and Core
/// binds nothing for it.
#[tokio::test(flavor = "multi_thread")]
async fn an_aborted_held_add_stays_owed_within_its_process() {
    let running = AgentIdentity::parse("held-add-process-running").unwrap();
    let added = AgentIdentity::parse("held-add-process-added").unwrap();
    let roster = vec![member(&running)];
    let dir = tempfile::tempdir().unwrap();
    let mob = mob_id("held-add-process");

    let mut runtime =
        installed_runtime(&dir, &mob, &roster, Arc::new(SelectingHook::default())).await;
    runtime
        .bootstrap_installed_identity_first_context_held(
            &roster,
            tokio::time::Instant::now() + Duration::from_mins(1),
            Box::new(|_members, _completion| Box::pin(async { Ok(()) })),
        )
        .await
        .expect("the running member is published");
    let context = runtime
        .identity_first_context()
        .expect("the identity context");
    abort_held_add(&context, member(&added)).await;

    let identities = runtime.identity_runtime().expect("the identity runtime");
    let woken = identities.materialize(&added).await;
    assert!(
        woken.as_ref().is_err_and(owes_held_completion),
        "a wake outside a held pass is refused: {woken:?}"
    );
    let sent = identities
        .send(
            &added,
            &meerkat_core::ContentInput::Text("a delivery outside a held pass".to_string()),
        )
        .await;
    assert!(
        sent.as_ref().is_err_and(owes_held_completion),
        "a delivery outside a held pass is refused: {sent:?}"
    );
    let reset = identities.reset(&added).await;
    assert!(
        reset.as_ref().is_err_and(owes_held_completion),
        "the reset is refused: {reset:?}"
    );
    let deleted = identities.delete_identity(&added).await;
    assert!(
        matches!(deleted, Ok(None)),
        "a delete finds nothing Core spawned: {deleted:?}"
    );
    drop(identities);
    drop(context);
    assert!(
        core_last_binding(&runtime, &added).await.is_none(),
        "Core never spawned the owed member"
    );
    let _ = runtime.shutdown().await;
}

/// A name the host declares held is reserved while its held creation is
/// owed: a raw spawn under it (a host's spawn without the identity entry's
/// custody) is refused typed (`IdentityMember`), and Core holds no member of
/// it. The reservation is Core's held creation entry, which stands from the
/// declaration on; a host must not raw-spawn under a name it declares held.
/// A pin of Core's entry guard.
#[tokio::test(flavor = "multi_thread")]
async fn a_raw_spawn_under_a_held_name_is_refused_while_its_held_creation_is_owed() {
    let running = AgentIdentity::parse("held-name-running").unwrap();
    let held = AgentIdentity::parse("held-name-reserved").unwrap();
    let roster = vec![member(&running)];
    let dir = tempfile::tempdir().unwrap();
    let mob = mob_id("held-name");

    let mut runtime =
        installed_runtime(&dir, &mob, &roster, Arc::new(SelectingHook::default())).await;
    runtime
        .bootstrap_installed_identity_first_context_held(
            &roster,
            tokio::time::Instant::now() + Duration::from_mins(1),
            Box::new(|_members, _completion| Box::pin(async { Ok(()) })),
        )
        .await
        .expect("the running member is published");
    let context = runtime
        .identity_first_context()
        .expect("the identity context");
    abort_held_add(&context, member(&held)).await;

    let raw = runtime
        .mob_handle()
        .spawn_spec(meerkat_mob::SpawnMemberSpec::host_root(
            "worker",
            meerkat_meta::member_comms_id::roster_member_id_for_identity(held.as_str()),
        ))
        .await;
    assert!(
        matches!(raw, Err(meerkat_mob::MobError::IdentityMember { .. })),
        "a raw spawn under the held name is refused typed: {raw:?}"
    );
    drop(context);
    assert!(
        core_last_binding(&runtime, &held).await.is_none(),
        "Core holds no member of the held name"
    );
    let _ = runtime.shutdown().await;
}

/// An aborted held add, and a restart whose ordinary bootstrap lists the
/// added identity: its host no longer holds it, so the identity is seated
/// and created as any other identity.
///
/// RED before the marks stopped being read: the ordinary bootstrap refused
/// the identity from its mark (`HeldCompletionOwed`).
#[tokio::test(flavor = "multi_thread")]
async fn an_aborted_held_add_is_seated_normally_after_an_ordinary_restart() {
    let running = AgentIdentity::parse("held-add-ordinary-running").unwrap();
    let added = AgentIdentity::parse("held-add-ordinary-added").unwrap();
    let roster = vec![member(&running)];
    let dir = tempfile::tempdir().unwrap();
    let mob = mob_id("held-add-ordinary-restart");

    let mut runtime =
        installed_runtime(&dir, &mob, &roster, Arc::new(SelectingHook::default())).await;
    runtime
        .bootstrap_installed_identity_first_context_held(
            &roster,
            tokio::time::Instant::now() + Duration::from_mins(1),
            Box::new(|_members, _completion| Box::pin(async { Ok(()) })),
        )
        .await
        .expect("the running member is published");
    let context = runtime
        .identity_first_context()
        .expect("the identity context");
    abort_held_add(&context, member(&added)).await;
    drop(context);
    let _ = runtime.shutdown().await;
    drop(runtime);

    let listed = vec![member(&running), member(&added)];
    let mut runtime =
        installed_runtime(&dir, &mob, &listed, Arc::new(SelectingHook::default())).await;
    let mut restored = runtime
        .bootstrap_installed_identity_first_context(&listed)
        .await
        .expect("the ordinary bootstrap runs");
    let outcome = restored.outcomes.remove(&added);
    assert!(
        matches!(
            &outcome,
            Some(
                meerkat_meta::identity_first::RestoreOutcome::Created { .. }
                    | meerkat_meta::identity_first::RestoreOutcome::Dormant { .. }
            )
        ),
        "{outcome:?}"
    );
    let identities = runtime.identity_runtime().expect("the identity runtime");
    identities
        .materialize(&added)
        .await
        .expect("the added identity is created as any other");
    drop(identities);
    assert!(
        core_binds_live(&runtime, &added).await,
        "Core binds the created member"
    );
    assert!(
        core_binds_live(&runtime, &running).await,
        "the running member resumes"
    );
    let _ = runtime.shutdown().await;
}

/// An aborted held add, and a restart whose host adds the identity held
/// again: that add's process holds it, so when the re-add aborts too the
/// identity stays owed there. Its wake is refused typed
/// (`HeldCompletionOwed`) and Core binds nothing for it.
#[tokio::test(flavor = "multi_thread")]
async fn an_aborted_held_add_stays_owed_after_a_restart_that_adds_it_held_again() {
    let running = AgentIdentity::parse("held-add-again-running").unwrap();
    let added = AgentIdentity::parse("held-add-again-added").unwrap();
    let roster = vec![member(&running)];
    let dir = tempfile::tempdir().unwrap();
    let mob = mob_id("held-add-again");

    let mut runtime =
        installed_runtime(&dir, &mob, &roster, Arc::new(SelectingHook::default())).await;
    runtime
        .bootstrap_installed_identity_first_context_held(
            &roster,
            tokio::time::Instant::now() + Duration::from_mins(1),
            Box::new(|_members, _completion| Box::pin(async { Ok(()) })),
        )
        .await
        .expect("the running member is published");
    let context = runtime
        .identity_first_context()
        .expect("the identity context");
    abort_held_add(&context, member(&added)).await;
    drop(context);
    let _ = runtime.shutdown().await;
    drop(runtime);

    let mut runtime =
        installed_runtime(&dir, &mob, &roster, Arc::new(SelectingHook::default())).await;
    runtime
        .bootstrap_installed_identity_first_context(&roster)
        .await
        .expect("the ordinary bootstrap runs");
    let context = runtime
        .identity_first_context()
        .expect("the identity context");
    abort_held_add(&context, member(&added)).await;
    let identities = runtime.identity_runtime().expect("the identity runtime");
    let woken = identities.materialize(&added).await;
    assert!(
        woken.as_ref().is_err_and(owes_held_completion),
        "a wake outside a held pass is refused: {woken:?}"
    );
    drop(identities);
    drop(context);
    assert!(
        core_last_binding(&runtime, &added).await.is_none(),
        "Core never spawned the owed member"
    );
    let _ = runtime.shutdown().await;
}

/// After an eager roster pass in the held add's process, an owed member the
/// host lists reads
/// `broken` in the bootstrap status, and its restore progress names the
/// refusal typed (the internal-error code with kind `held_completion_owed`),
/// so a host reads an admission still owed without parsing the error text.
#[tokio::test(flavor = "multi_thread")]
async fn an_owed_listed_member_reads_broken_with_its_typed_restore_refusal() {
    use meerkat_meta::identity_first::{IdentityBootstrapState, IdentityRestoreProgress};
    let running = AgentIdentity::parse("held-owed-status-running").unwrap();
    let added = AgentIdentity::parse("held-owed-status-added").unwrap();
    let roster = vec![member(&running)];
    let dir = tempfile::tempdir().unwrap();
    let mob = mob_id("held-owed-status");

    let mut runtime =
        installed_runtime(&dir, &mob, &roster, Arc::new(SelectingHook::default())).await;
    runtime
        .bootstrap_installed_identity_first_context_held(
            &roster,
            tokio::time::Instant::now() + Duration::from_mins(1),
            Box::new(|_members, _completion| Box::pin(async { Ok(()) })),
        )
        .await
        .expect("the running member is published");
    let context = runtime
        .identity_first_context()
        .expect("the identity context");
    abort_held_add(&context, member(&added)).await;

    // The host lists the added member while its add's process still holds
    // it: the context's next eager pass refuses it, and the member's
    // bootstrap status entry says why, typed.
    let listed = vec![member(&running), member(&added)];
    context
        .bootstrap_roster(&listed)
        .await
        .expect("the eager pass runs");
    let status = runtime
        .identity_runtime()
        .expect("the identity runtime")
        .identity_bootstrap_status();
    let entry = status
        .identities
        .get(&added)
        .expect("the owed member's bootstrap entry");
    assert_eq!(entry.state, IdentityBootstrapState::Broken, "{entry:?}");
    assert_eq!(
        entry.restore,
        Some(IdentityRestoreProgress::Refused {
            code: -32603,
            kind: Some("held_completion_owed".to_string()),
        }),
        "{entry:?}"
    );
    assert!(
        entry
            .error
            .as_deref()
            .is_some_and(|error| error.contains("owes its host a fresh held completion")),
        "{entry:?}"
    );
    assert_eq!(status.counts.broken, 1, "{status:?}");
    assert!(
        !status.ready,
        "an owed member keeps the bootstrap from ready"
    );
    drop(context);
    let _ = runtime.shutdown().await;
}

/// More fresh members than the restore pass's construction slots (four by
/// default): every member still reaches its hold, because a held member
/// gives its slot back, and the pass decides only once all are held.
#[tokio::test(flavor = "multi_thread")]
async fn held_members_never_occupy_a_construction_slot() {
    let dir = tempfile::tempdir().unwrap();
    let roster: Vec<_> = (0..6)
        .map(|index| member(&AgentIdentity::parse(&format!("held-slot-{index}")).unwrap()))
        .collect();
    let mut runtime = installed_runtime(
        &dir,
        &mob_id("held-slots"),
        &roster,
        Arc::new(SelectingHook::default()),
    )
    .await;
    let held = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&held);
    runtime
        .bootstrap_installed_identity_first_context_held(
            &roster,
            tokio::time::Instant::now() + Duration::from_mins(1),
            Box::new(move |members, _completion| {
                Box::pin(async move {
                    counted.store(members.len(), Ordering::SeqCst);
                    Ok(())
                })
            }),
        )
        .await
        .expect("every member is held, then published");
    assert_eq!(held.load(Ordering::SeqCst), 6, "all six were held at once");
    assert_eq!(runtime.mob_handle().list_members().await.len(), 6);
    let _ = runtime.shutdown().await;
}

/// A roster with a member Core already spawned (it would resume, not be
/// created) is refused before anything is held or created.
#[tokio::test(flavor = "multi_thread")]
async fn a_roster_with_an_existing_member_is_refused_before_anything_is_held() {
    let existing = AgentIdentity::parse("held-existing").unwrap();
    let fresh = AgentIdentity::parse("held-fresh").unwrap();
    let roster = vec![member(&existing), member(&fresh)];
    let dir = tempfile::tempdir().unwrap();
    let hook = Arc::new(SelectingHook::default());
    let mut runtime = installed_runtime(&dir, &mob_id("held-mixed"), &roster, hook.clone()).await;
    runtime
        .bootstrap_installed_identity_first_context(&[member(&existing)])
        .await
        .expect("the existing member is created");
    assert!(
        core_binds_live(&runtime, &existing).await,
        "Core's member history binds the existing member"
    );
    assert!(
        core_last_binding(&runtime, &fresh).await.is_none(),
        "Core never spawned the fresh member"
    );
    let constructed = hook.prepared.load(Ordering::SeqCst);

    let error = runtime
        .bootstrap_installed_identity_first_context_held(
            &roster,
            tokio::time::Instant::now() + Duration::from_mins(1),
            Box::new(|_members, _completion| Box::pin(async { Ok(()) })),
        )
        .await
        .expect_err("a mixed roster is refused");
    assert!(
        matches!(
            &error,
            meerkat_meta::identity_first::IdentityRuntimeError::HeldBootstrapRefused(
                meerkat_meta::identity_first::HeldBootstrapRefusal::ExistingContinuity { identity }
            ) if identity == &existing
        ),
        "{error}"
    );
    assert_eq!(
        hook.prepared.load(Ordering::SeqCst),
        constructed,
        "the refused pass constructed nothing"
    );
}

/// A caller that goes away while a member is genuinely held (Core holds it
/// at its hold point) abandons the pass: the runtime's owned task aborts the
/// member and waits until Core has settled it, joins the host's callback
/// (which stops once the pass aborts) and retires the pass, so a retry on
/// the same runtime holds and publishes the member with no stale held state.
#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_held_bootstrap_cleans_up_in_its_owner() {
    let dir = tempfile::tempdir().unwrap();
    let identity = AgentIdentity::parse("held-dropped").unwrap();
    let roster = vec![member(&identity)];
    let mut runtime = installed_runtime(
        &dir,
        &mob_id("held-dropped"),
        &roster,
        Arc::new(SelectingHook::default()),
    )
    .await;
    let (held_tx, held_rx) = tokio::sync::oneshot::channel();
    {
        let bootstrap = runtime.bootstrap_installed_identity_first_context_held(
            &roster,
            tokio::time::Instant::now() + Duration::from_mins(1),
            Box::new(move |members, completion| {
                Box::pin(async move {
                    let _ = held_tx.send(members.len());
                    // A host that stops waiting once the pass aborts; the
                    // pass joins this callback before it settles.
                    completion.aborted().await;
                    Err("the pass was aborted".to_string())
                })
            }),
        );
        tokio::select! {
            _ = bootstrap => panic!("the bootstrap settled while its member was held"),
            held = held_rx => assert_eq!(held.expect("held report"), 1),
        }
        // The bootstrap future is dropped here, while the member is held.
    }
    assert!(
        runtime.mob_handle().list_members().await.is_empty(),
        "the abandoned member was never published"
    );

    // The retry waits for the abandoned pass under the runtime's bootstrap
    // controller, then finds the identity fresh in Core: the aborted member
    // left no binding.
    let held = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&held);
    tokio::time::timeout(
        Duration::from_mins(2),
        runtime.bootstrap_installed_identity_first_context_held(
            &roster,
            tokio::time::Instant::now() + Duration::from_mins(1),
            Box::new(move |members, _completion| {
                Box::pin(async move {
                    counted.store(members.len(), Ordering::SeqCst);
                    Ok(())
                })
            }),
        ),
    )
    .await
    .expect("the abandoned pass settles in its owner")
    .expect("a retry holds and publishes the member");
    assert_eq!(held.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.mob_handle().list_members().await.len(), 1);
    assert!(
        core_binds_live(&runtime, &identity).await,
        "Core's member history binds the published member"
    );
    let _ = runtime.shutdown().await;
}

/// The bounded start returns the pass's pending settlement at the setup
/// deadline while the member's construction is still blocked: the
/// settlement stays pending for as long as Core owns the member, and
/// resolves once Core has settled it, with nothing published.
#[tokio::test(flavor = "multi_thread")]
async fn a_held_bootstrap_returns_at_its_deadline_while_construction_is_blocked() {
    let dir = tempfile::tempdir().unwrap();
    let identity = AgentIdentity::parse("held-blocked-construction").unwrap();
    let roster = vec![member(&identity)];
    let (release, parked) = tokio::sync::watch::channel(false);
    let hook = Arc::new(SelectingHook::parking(parked));
    let mut runtime = installed_runtime(
        &dir,
        &mob_id("held-blocked-construction"),
        &roster,
        hook.clone(),
    )
    .await;
    let asked = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&asked);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let start = tokio::time::timeout(
        Duration::from_secs(30),
        runtime.bootstrap_installed_identity_first_context_held_until(
            &roster,
            deadline,
            Box::new(move |_members, _completion| {
                Box::pin(async move {
                    counted.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            }),
        ),
    )
    .await
    .expect("the start is bounded by its deadline")
    .expect("the held bootstrap starts");
    assert!(tokio::time::Instant::now() >= deadline);
    let meerkat_meta::identity_first::HeldBootstrapStart::Unsettled(settlement) = start else {
        panic!("a member whose construction is blocked is never activated");
    };
    assert_eq!(
        hook.entered.load(Ordering::SeqCst),
        1,
        "the construction is parked"
    );
    assert_eq!(asked.load(Ordering::SeqCst), 0, "the host was never asked");
    // Core still owns the parked member, so the settlement stays pending for
    // as long as that lasts, not only at the instant the start returned.
    let error = {
        let finish = runtime.finish_held_identity_bootstrap(settlement);
        tokio::pin!(finish);
        assert!(
            tokio::time::timeout(Duration::from_millis(500), &mut finish)
                .await
                .is_err(),
            "the settlement waits while Core still owns the member"
        );
        release.send_replace(true);
        tokio::time::timeout(Duration::from_mins(1), finish)
            .await
            .expect("the settlement completes once the construction returns")
    };
    assert!(
        matches!(
            error,
            meerkat_meta::identity_first::IdentityRuntimeError::HeldBootstrapRefused(
                meerkat_meta::identity_first::HeldBootstrapRefusal::DeadlineElapsed
            )
        ),
        "{error}"
    );
    assert!(runtime.mob_handle().list_members().await.is_empty());
}

/// The bounded start returns at the setup deadline while the host's callback
/// still runs; the callback is then joined, its late `Ok` is recorded as
/// callback settlement only, and nothing is activated.
#[tokio::test(flavor = "multi_thread")]
async fn a_held_bootstrap_returns_at_its_deadline_while_its_callback_runs() {
    let dir = tempfile::tempdir().unwrap();
    let identity = AgentIdentity::parse("held-blocked-callback").unwrap();
    let roster = vec![member(&identity)];
    let mut runtime = installed_runtime(
        &dir,
        &mob_id("held-blocked-callback"),
        &roster,
        Arc::new(SelectingHook::default()),
    )
    .await;
    let (release, released) = tokio::sync::watch::channel(false);
    let returned = Arc::new(AtomicUsize::new(0));
    let callback_returned = Arc::clone(&returned);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let start = tokio::time::timeout(
        Duration::from_secs(30),
        runtime.bootstrap_installed_identity_first_context_held_until(
            &roster,
            deadline,
            Box::new(move |_members, _completion| {
                Box::pin(async move {
                    let _ = released.clone().wait_for(|released| *released).await;
                    callback_returned.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            }),
        ),
    )
    .await
    .expect("the start is bounded by its deadline")
    .expect("the held bootstrap starts");
    let meerkat_meta::identity_first::HeldBootstrapStart::Unsettled(settlement) = start else {
        panic!("a callback still running at the deadline activates nothing");
    };
    assert_eq!(
        returned.load(Ordering::SeqCst),
        0,
        "the callback still runs"
    );
    release.send_replace(true);
    let error = tokio::time::timeout(
        Duration::from_mins(1),
        runtime.finish_held_identity_bootstrap(settlement),
    )
    .await
    .expect("the settlement joins the released callback");
    assert!(
        matches!(
            error,
            meerkat_meta::identity_first::IdentityRuntimeError::HeldBootstrapAborted {
                refusal: meerkat_meta::identity_first::HeldBootstrapRefusal::DeadlineElapsed,
                completion: meerkat_meta::identity_first::HeldCompletionOutcome::Completed,
            }
        ),
        "{error}"
    );
    assert_eq!(returned.load(Ordering::SeqCst), 1);
    assert!(runtime.mob_handle().list_members().await.is_empty());
}

/// A caller lost while the host's callback runs is the pass's terminal
/// `Abort`: the callback's later `Ok` activates nothing, and Core keeps no
/// binding for the member, so a retry on the same runtime holds it again.
#[tokio::test(flavor = "multi_thread")]
async fn a_caller_lost_during_the_callback_never_activates_its_members() {
    let dir = tempfile::tempdir().unwrap();
    let identity = AgentIdentity::parse("held-caller-lost").unwrap();
    let roster = vec![member(&identity)];
    let mut runtime = installed_runtime(
        &dir,
        &mob_id("held-caller-lost"),
        &roster,
        Arc::new(SelectingHook::default()),
    )
    .await;
    let (asked_tx, asked_rx) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::watch::channel(false);
    let acknowledged = Arc::new(AtomicUsize::new(0));
    let callback_acknowledged = Arc::clone(&acknowledged);
    {
        let bootstrap = runtime.bootstrap_installed_identity_first_context_held_until(
            &roster,
            tokio::time::Instant::now() + Duration::from_mins(1),
            Box::new(move |_members, _completion| {
                Box::pin(async move {
                    let _ = asked_tx.send(());
                    let _ = released.clone().wait_for(|released| *released).await;
                    callback_acknowledged.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            }),
        );
        tokio::select! {
            _ = bootstrap => panic!("the bootstrap settled while its host was completing"),
            asked = asked_rx => asked.expect("the host is asked"),
        }
        // The caller goes away here, while the host's callback runs.
    }
    release.send_replace(true);
    tokio::time::timeout(Duration::from_secs(10), async {
        while acknowledged.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the callback acknowledged Ok after the caller was gone");
    // The retry waits for the abandoned pass under the runtime's bootstrap
    // controller. The late `Ok` published nothing: Core holds no binding for
    // the identity (one would refuse the retry as `ExistingContinuity`), so
    // the retry holds it again.
    let held = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&held);
    tokio::time::timeout(
        Duration::from_mins(2),
        runtime.bootstrap_installed_identity_first_context_held(
            &roster,
            tokio::time::Instant::now() + Duration::from_mins(1),
            Box::new(move |members, _completion| {
                Box::pin(async move {
                    counted.store(members.len(), Ordering::SeqCst);
                    Ok(())
                })
            }),
        ),
    )
    .await
    .expect("the abandoned pass settles in its owner")
    .expect("a retry holds and publishes the member");
    assert_eq!(
        held.load(Ordering::SeqCst),
        1,
        "the identity was held again"
    );
    assert_eq!(
        runtime.mob_handle().list_members().await.len(),
        1,
        "only the retry published the member"
    );
    assert!(
        core_binds_live(&runtime, &identity).await,
        "Core's member history binds the published member"
    );
    let _ = runtime.shutdown().await;
}

/// A host that publishes outside the runtime before its callback settles
/// `Ok` after the deadline: the pass still aborts its members, the external
/// publication is left as the host made it (a native abort cannot undo it),
/// the late `Ok` is recorded as callback settlement only, and the callback
/// is never run again.
#[tokio::test(flavor = "multi_thread")]
async fn an_external_publication_before_a_late_ok_is_kept_and_not_replayed() {
    let dir = tempfile::tempdir().unwrap();
    let identity = AgentIdentity::parse("held-external").unwrap();
    let roster = vec![member(&identity)];
    let mut runtime = installed_runtime(
        &dir,
        &mob_id("held-external"),
        &roster,
        Arc::new(SelectingHook::default()),
    )
    .await;
    let external = Arc::new(std::sync::Mutex::new(Vec::new()));
    let published = Arc::clone(&external);
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    let error = runtime
        .bootstrap_installed_identity_first_context_held(
            &roster,
            tokio::time::Instant::now() + Duration::from_secs(5),
            Box::new(move |members, completion| {
                Box::pin(async move {
                    counted.fetch_add(1, Ordering::SeqCst);
                    published
                        .lock()
                        .unwrap()
                        .extend(members.into_iter().map(|member| member.identity));
                    tokio::time::sleep_until(completion.deadline() + Duration::from_millis(100))
                        .await;
                    Ok(())
                })
            }),
        )
        .await
        .expect_err("a late Ok does not activate the pass");
    assert!(
        matches!(
            error,
            meerkat_meta::identity_first::IdentityRuntimeError::HeldBootstrapAborted {
                refusal: meerkat_meta::identity_first::HeldBootstrapRefusal::DeadlineElapsed,
                completion: meerkat_meta::identity_first::HeldCompletionOutcome::Completed,
            }
        ),
        "{error}"
    );
    assert_eq!(
        *external.lock().unwrap(),
        vec![identity],
        "the external publication is kept as the host made it"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the callback is never replayed"
    );
    assert!(runtime.mob_handle().list_members().await.is_empty());
}
