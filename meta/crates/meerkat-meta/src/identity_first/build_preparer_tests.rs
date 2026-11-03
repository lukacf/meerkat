//! Rows for Meta's member build preparation (design rows R109-R112, R121):
//! the hold of a build that comes before the identity runtime is composed,
//! the composition that publishes and releases it, the inline preparation of
//! a registered identity's build after it, the typed refusal of a failed
//! preparation, and the tool record each reported build writes.
//!
//! Core's request and report types are Core's to construct, so these rows
//! drive the preparer's own decision methods with the values Core hands it;
//! the rows that run a mob end to end live in the integration tests.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use meerkat_core::types::SessionId;
use meerkat_mob::{MemberBuildPreparationRefusal, MemberBuildPrepared, MemberIdentityManagement};

use super::{IdentityBuildPreparer, PUBLISHED_ONCE_COMPOSED};
use crate::identity_first::bridge::{
    BridgeAdmissionError, BridgeDelivery, BridgeError, MemberRuntimeBinding, ResumeSessionOutcome,
    SessionBridge,
};
use crate::identity_first::contracts::AgentCustomizer;
use crate::identity_first::core_test_bridge::bind_live;
use crate::identity_first::core_test_members::CoreTestMembers;
use crate::identity_first::customizer_tools::CustomizerToolRegistry;
use crate::identity_first::local_lease::LocalLeaseProvider;
use crate::identity_first::local_store::LocalContinuityStore;
use crate::identity_first::{
    AgentAddressability, AgentBuildContext, AgentBuildDraft, AgentIdentity, AgentRuntimeId,
    CustomizerError, DurabilityPolicy, DurableAgentSpec, IdentityRuntime, IdentityRuntimeConfig,
    IdentityRuntimeError, SessionSnapshot,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn identity(name: &str) -> AgentIdentity {
    AgentIdentity::parse(name).expect("valid identity")
}

fn member_of(identity: &AgentIdentity) -> meerkat_mob::AgentIdentity {
    crate::member_comms_id::mob_member_id(identity.as_str())
}

fn spec(identity: &AgentIdentity) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: identity.clone(),
        profile: meerkat_mob::ProfileName::from("personal"),
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

fn management(
    answer: MemberIdentityManagement,
) -> impl std::future::Future<Output = Result<MemberIdentityManagement, meerkat_mob::MobError>> + Send
{
    std::future::ready(Ok(answer))
}

/// A host customizer that counts its builds, records the context of each,
/// and fails when `refuse` is set.
#[derive(Default)]
struct CountingCustomizer {
    builds: AtomicUsize,
    contexts: Mutex<Vec<AgentBuildContext>>,
    refuse: Option<fn() -> CustomizerError>,
}

#[async_trait::async_trait]
impl AgentCustomizer for CountingCustomizer {
    async fn customize_build(
        &self,
        context: &AgentBuildContext,
        _spec: &DurableAgentSpec,
        _draft: &mut AgentBuildDraft,
    ) -> Result<(), CustomizerError> {
        self.builds.fetch_add(1, Ordering::SeqCst);
        self.contexts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(context.clone());
        match self.refuse {
            Some(refusal) => Err(refusal()),
            None => Ok(()),
        }
    }
}

/// Core holding members live, and recording each run-start hold release
/// the identity runtime asks for.
#[derive(Default)]
struct HoldingCore {
    core: CoreTestMembers,
    released: Mutex<Vec<String>>,
}

impl HoldingCore {
    fn released(&self) -> Vec<String> {
        self.released
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[async_trait::async_trait]
impl SessionBridge for HoldingCore {
    async fn create_session(
        &self,
        identity: &AgentIdentity,
        _spec: &DurableAgentSpec,
        _draft: &AgentBuildDraft,
    ) -> Result<MemberRuntimeBinding, BridgeError> {
        Err(BridgeError::Mob(format!(
            "{identity}: a build preparation creates no member"
        )))
    }

    async fn resume_session(
        &self,
        identity: &AgentIdentity,
        _spec: &DurableAgentSpec,
        _draft: &AgentBuildDraft,
        _session_id: &SessionId,
        _resume_from_role: Option<&meerkat_mob::ProfileName>,
    ) -> Result<ResumeSessionOutcome, BridgeError> {
        Err(BridgeError::Mob(format!(
            "{identity}: a build preparation resumes no session"
        )))
    }

    async fn live_member(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<MemberRuntimeBinding>, BridgeError> {
        self.core.live_member(identity)
    }

    async fn member_binding_history(
        &self,
    ) -> Result<meerkat_mob::MemberBindingHistory, BridgeError> {
        self.core.history()
    }

    async fn deliver_admitted(
        &self,
        runtime_id: &AgentRuntimeId,
        _delivery: BridgeDelivery,
    ) -> Result<SessionId, BridgeError> {
        Err(BridgeError::Mob(format!(
            "{runtime_id}: a build preparation delivers nothing"
        )))
    }

    async fn checkpoint_session(
        &self,
        runtime_id: &AgentRuntimeId,
        _session_id: &SessionId,
    ) -> Result<SessionSnapshot, BridgeError> {
        Err(BridgeError::Mob(format!(
            "{runtime_id}: a build preparation checkpoints nothing"
        )))
    }

    async fn retire_member(&self, runtime_id: &AgentRuntimeId) -> Result<(), BridgeError> {
        self.core.retire(runtime_id);
        Ok(())
    }

    async fn release_member_run_starts_for_published_tools(
        &self,
        member_id: &meerkat_mob::AgentIdentity,
    ) -> Result<(), BridgeError> {
        self.released
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(member_id.to_string());
        Ok(())
    }
}

/// An identity runtime over `core` with `customizer` installed and a
/// customizer tool registry, as a composition that composes one has it.
async fn runtime_with(
    core: &Arc<HoldingCore>,
    customizer: Option<Arc<CountingCustomizer>>,
) -> Result<(Arc<IdentityRuntime>, Arc<CustomizerToolRegistry>), Box<dyn std::error::Error>> {
    let runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: Arc::new(LocalContinuityStore::in_memory()?),
        lease_provider: Arc::new(LocalLeaseProvider::new()),
        runtime_instance_id: "build-preparation-rows".to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(core.clone()),
        default_timeout: None,
    }));
    let registry = CustomizerToolRegistry::new();
    runtime
        .set_customizer_tool_registry(Some(Arc::clone(&registry)))
        .await;
    runtime
        .set_agent_customizer(customizer.map(|customizer| customizer as Arc<dyn AgentCustomizer>))
        .await;
    Ok((runtime, registry))
}

/// R111: a member Core builds before the identity runtime exists is held
/// (`HoldUntilReleased`, Core stages `ToolsNotPublished`), and the hold is
/// pending on the runtime once the preparer is bound to it.
#[tokio::test]
async fn r111_a_build_before_the_runtime_is_bound_is_held_and_pending_on_the_runtime_at_bind()
-> TestResult {
    let alice = identity("personal:alice");
    let preparer = IdentityBuildPreparer::new(true);
    let prepared = preparer
        .prepare_identity_member(
            &member_of(&alice),
            alice.clone(),
            None,
            management(MemberIdentityManagement::CommandDriven),
        )
        .await;
    assert_eq!(prepared, Ok(MemberBuildPrepared::HoldUntilReleased));

    let core = Arc::new(HoldingCore::default());
    let (runtime, _registry) = runtime_with(&core, None).await?;
    preparer.bind(&runtime, None).await;
    let pending = runtime
        .customizer_tools_pending_for(&alice)
        .ok_or("the held build is pending on the runtime")?;
    assert_eq!(pending.reason, PUBLISHED_ONCE_COMPOSED);
    Ok(())
}

/// R111: the composition (the roster's registration) publishes each held
/// member's customizer tools with the runtime's real context, releases its
/// run-start hold exactly once, and clears its pending record; a later
/// composition releases nothing again.
#[tokio::test]
async fn r111_the_composition_publishes_a_held_member_and_releases_its_hold_once() -> TestResult {
    let alice = identity("personal:alice");
    let core = Arc::new(HoldingCore::default());
    bind_live(
        &core.core,
        &alice,
        &meerkat_mob::ProfileName::from("personal"),
        1,
        Some(SessionId::new()),
    )?;
    let customizer = Arc::new(CountingCustomizer::default());
    let (runtime, registry) = runtime_with(&core, Some(Arc::clone(&customizer))).await?;
    let preparer = IdentityBuildPreparer::new(true);
    preparer.bind(&runtime, None).await;

    // Bound, not composed yet: the build is held.
    let prepared = preparer
        .prepare_identity_member(
            &member_of(&alice),
            alice.clone(),
            Some(Arc::clone(&runtime)),
            management(MemberIdentityManagement::CommandDriven),
        )
        .await;
    assert_eq!(prepared, Ok(MemberBuildPrepared::HoldUntilReleased));
    assert_eq!(customizer.builds.load(Ordering::SeqCst), 0);

    runtime.register(spec(&alice)).await;
    runtime.compose_member_builds().await;

    assert_eq!(customizer.builds.load(Ordering::SeqCst), 1);
    assert_eq!(registry.ensure(&alice).generation(), 1, "published once");
    assert_eq!(core.released(), vec![member_of(&alice).to_string()]);
    assert!(runtime.customizer_tools_pending_for(&alice).is_none());
    let contexts = customizer
        .contexts
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert_eq!(
        contexts[0].active_peers,
        vec![alice.clone()],
        "the composition builds with the registered roster, never an empty context"
    );

    runtime.compose_member_builds().await;
    assert_eq!(
        core.released().len(),
        1,
        "a later composition releases nothing"
    );
    Ok(())
}

/// R109: once the runtime is composed, a registered identity's build is
/// prepared inline: the host builds once, its tools are published before
/// Core's provision, and nothing is held or released.
#[tokio::test]
async fn r109_after_the_composition_a_registered_identity_is_prepared_inline_and_never_held()
-> TestResult {
    let alice = identity("personal:alice");
    let core = Arc::new(HoldingCore::default());
    let customizer = Arc::new(CountingCustomizer::default());
    let (runtime, registry) = runtime_with(&core, Some(Arc::clone(&customizer))).await?;
    let preparer = IdentityBuildPreparer::new(true);
    preparer.bind(&runtime, None).await;
    runtime.register(spec(&alice)).await;
    runtime.compose_member_builds().await;

    let prepared = preparer
        .prepare_identity_member(
            &member_of(&alice),
            alice.clone(),
            Some(Arc::clone(&runtime)),
            management(MemberIdentityManagement::CommandDriven),
        )
        .await;
    assert_eq!(prepared, Ok(MemberBuildPrepared::Prepared));
    assert_eq!(customizer.builds.load(Ordering::SeqCst), 1);
    assert_eq!(registry.ensure(&alice).generation(), 1);
    assert!(
        core.released().is_empty(),
        "an inline preparation holds nothing"
    );
    assert!(runtime.customizer_tools_pending_for(&alice).is_none());
    Ok(())
}

/// R112: a preparation whose host build fails is refused retryable (the
/// member stays dormant), and the identity's pending record keeps the typed
/// reason: a delivery Core refused `HostPreparationPending` reports the
/// host's artifact refusal, typed, never a generic retry.
#[tokio::test]
async fn r112_a_failed_preparation_is_refused_retryable_and_a_delivery_reports_it_typed()
-> TestResult {
    let alice = identity("personal:alice");
    let core = Arc::new(HoldingCore::default());
    let customizer = Arc::new(CountingCustomizer {
        refuse: Some(|| CustomizerError::BuildFailed("the host is down".to_string())),
        ..CountingCustomizer::default()
    });
    let (runtime, _registry) = runtime_with(&core, Some(customizer)).await?;
    let preparer = IdentityBuildPreparer::new(true);
    preparer.bind(&runtime, None).await;
    runtime.register(spec(&alice)).await;
    runtime.compose_member_builds().await;

    let prepared = preparer
        .prepare_identity_member(
            &member_of(&alice),
            alice.clone(),
            Some(Arc::clone(&runtime)),
            management(MemberIdentityManagement::CommandDriven),
        )
        .await;
    let Err(MemberBuildPreparationRefusal::Retryable { reason }) = prepared else {
        return Err(format!("expected a retryable refusal, got {prepared:?}").into());
    };
    assert!(reason.contains("the host is down"), "{reason}");

    let refused = runtime.admission_error(
        &alice,
        BridgeAdmissionError::MemberWakeRefused {
            identity: member_of(&alice),
            refusal: meerkat_mob::MemberWakeRefusal::HostPreparationPending,
            detail: "host preparation pending".to_string(),
        },
    );
    match refused {
        IdentityRuntimeError::IdentityWakeRetryable { detail, .. } => {
            assert!(detail.contains("the host is down"), "{detail}");
        }
        other => return Err(format!("expected the recorded reason, got {other:?}").into()),
    }
    Ok(())
}

/// After the composition, a member of an identity the roster does not list
/// is held until the roster pass retires it, while a host-adopted
/// identity's member is not Meta's to hold.
#[tokio::test]
async fn an_unlisted_identitys_member_is_held_and_a_host_adopted_one_is_prepared() -> TestResult {
    let gone = identity("personal:gone");
    let adopted = identity("personal:adopted");
    let core = Arc::new(HoldingCore::default());
    let customizer = Arc::new(CountingCustomizer::default());
    let (runtime, _registry) = runtime_with(&core, Some(Arc::clone(&customizer))).await?;
    let preparer = IdentityBuildPreparer::new(true);
    preparer.bind(&runtime, None).await;
    runtime.compose_member_builds().await;

    let held = preparer
        .prepare_identity_member(
            &member_of(&gone),
            gone.clone(),
            Some(Arc::clone(&runtime)),
            management(MemberIdentityManagement::CommandDriven),
        )
        .await;
    assert_eq!(held, Ok(MemberBuildPrepared::HoldUntilReleased));
    let prepared = preparer
        .prepare_identity_member(
            &member_of(&adopted),
            adopted.clone(),
            Some(Arc::clone(&runtime)),
            management(MemberIdentityManagement::IntentManaged),
        )
        .await;
    assert_eq!(prepared, Ok(MemberBuildPrepared::Prepared));
    assert_eq!(
        customizer.builds.load(Ordering::SeqCst),
        0,
        "no roster spec, no host build"
    );

    // A composition publishes nothing for the unlisted identity.
    runtime.compose_member_builds().await;
    assert!(runtime.customizer_tools_pending_for(&gone).is_some());
    assert!(core.released().is_empty());
    Ok(())
}

/// A member of an identity a host adopted is never held, before the runtime
/// is bound or before it is composed: no roster publication or retirement
/// covers it, so nothing would ever release the hold.
#[tokio::test]
async fn a_host_adopted_identitys_member_is_never_held_before_the_composition() -> TestResult {
    let adopted = identity("personal:adopted");
    let preparer = IdentityBuildPreparer::new(true);
    let unbound = preparer
        .prepare_identity_member(
            &member_of(&adopted),
            adopted.clone(),
            None,
            management(MemberIdentityManagement::IntentManaged),
        )
        .await;
    assert_eq!(unbound, Ok(MemberBuildPrepared::Prepared));

    let core = Arc::new(HoldingCore::default());
    let (runtime, _registry) = runtime_with(&core, None).await?;
    preparer.bind(&runtime, None).await;
    let bound = preparer
        .prepare_identity_member(
            &member_of(&adopted),
            adopted.clone(),
            Some(Arc::clone(&runtime)),
            management(MemberIdentityManagement::IntentManaged),
        )
        .await;
    assert_eq!(bound, Ok(MemberBuildPrepared::Prepared));
    assert!(
        runtime.customizer_tools_pending_for(&adopted).is_none(),
        "nothing is pending for a composition to release"
    );
    Ok(())
}

/// R121: every build Core reports is written to the identity's tool record,
/// sorted and deduplicated, with a record revision that counts the builds; a
/// report that arrives before the preparer is bound is written at the bind.
/// A composition without a customizer prepares every build at once.
#[tokio::test]
async fn r121_each_reported_build_is_recorded_and_a_report_before_the_bind_is_kept() -> TestResult {
    let alice = identity("personal:alice");
    let session = SessionId::new();
    let preparer = IdentityBuildPreparer::new(false);
    let prepared = preparer
        .prepare_identity_member(
            &member_of(&alice),
            alice.clone(),
            None,
            management(MemberIdentityManagement::CommandDriven),
        )
        .await;
    assert_eq!(
        prepared,
        Ok(MemberBuildPrepared::Prepared),
        "nothing to publish, nothing held"
    );
    preparer
        .record_member_build(
            &member_of(&alice),
            session.clone(),
            vec![
                "search".to_string(),
                "fetch".to_string(),
                "search".to_string(),
            ],
        )
        .await;

    let core = Arc::new(HoldingCore::default());
    let (runtime, _registry) = runtime_with(&core, None).await?;
    preparer.bind(&runtime, None).await;
    let record = runtime
        .last_built_tools(&alice)
        .await?
        .ok_or("the report before the bind is recorded at the bind")?;
    assert_eq!(record.session_id, session);
    assert_eq!(
        record.tools,
        vec!["fetch".to_string(), "search".to_string()]
    );
    assert_eq!(record.record_revision, 1);

    // The next build replaces the record; a build the preparer never
    // prepared as an identity's is not recorded.
    preparer
        .prepare_identity_member(
            &member_of(&alice),
            alice.clone(),
            Some(Arc::clone(&runtime)),
            management(MemberIdentityManagement::CommandDriven),
        )
        .await
        .map_err(|refusal| refusal.reason().to_string())?;
    preparer
        .record_member_build(
            &member_of(&alice),
            session.clone(),
            vec!["fetch".to_string()],
        )
        .await;
    preparer
        .record_member_build(
            &member_of(&identity("personal:stranger")),
            session,
            vec!["other".to_string()],
        )
        .await;
    let record = runtime
        .last_built_tools(&alice)
        .await?
        .ok_or("the record stays")?;
    assert_eq!(record.tools, vec!["fetch".to_string()]);
    assert_eq!(record.record_revision, 2);
    assert!(
        runtime
            .last_built_tools(&identity("personal:stranger"))
            .await?
            .is_none()
    );
    Ok(())
}

/// A build Core reports without asking the preparer first (a spawn Core
/// commits live: an embodied seat, a held create) is still recorded for the
/// registered identity its member id names, so every built member has a
/// tool record; a member that is no registered identity's is not recorded.
/// The unregistered member's id names a well-formed identity, so only the
/// registration check keeps its build out of the records: a preparer that
/// recorded every member it could name would write it.
#[tokio::test]
async fn a_build_core_reports_unprepared_is_recorded_for_its_registered_identity() -> TestResult {
    let alice = identity("personal:alice");
    let unregistered = identity("personal:helper-1");
    let session = SessionId::new();
    let core = Arc::new(HoldingCore::default());
    let (runtime, _registry) = runtime_with(&core, None).await?;
    let preparer = IdentityBuildPreparer::new(true);
    preparer.bind(&runtime, None).await;
    runtime.register(spec(&alice)).await;

    preparer
        .record_member_build(
            &member_of(&alice),
            session.clone(),
            vec!["search".to_string()],
        )
        .await;
    preparer
        .record_member_build(
            &member_of(&unregistered),
            session.clone(),
            vec!["other".to_string()],
        )
        .await;

    let record = runtime
        .last_built_tools(&alice)
        .await?
        .ok_or("the unprepared build of a registered identity is recorded")?;
    assert_eq!(record.session_id, session);
    assert_eq!(record.tools, vec!["search".to_string()]);
    assert!(
        runtime.last_built_tools(&unregistered).await?.is_none(),
        "the build of a member no registered identity names is not recorded"
    );
    Ok(())
}
