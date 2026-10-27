//! Customizer tools that follow an identity across meerkat-side rebuilds (#563).
//!
//! The tools a host returns from `AgentCustomizer::customize_build` used to
//! reach a member only as the per-spawn `external_tools` overlay of the one
//! spawn that ran the customizer. meerkat-mob keeps that overlay in memory
//! and never persists it, so a restart restore, an adopted occupant
//! (`MemberAlreadyExists`), a respawn or a delivery-time repair rebuilt the
//! member without them: the host's handlers stayed registered, but the live
//! agent no longer advertised the tools.
//!
//! Instead, every identity member carries ONE stable, dynamic dispatcher
//! ([`IdentityCustomizerTools`]) for its whole life. Each successful
//! `customize_build` publishes its result into that dispatcher, swapping the
//! tool list and the dispatcher that serves it (for gateway hosts, the
//! handler scope) together. meerkat composes per-spawn overlays through a
//! dynamic composite that re-reads `tools()` on every turn, so:
//!
//! - an occupant that is adopted instead of respawned already holds the
//!   identity's dispatcher, and publishing re-attaches the current tools
//!   live, without a respawn;
//! - [`CustomizerToolsSpawnCustomizer`], installed on the mob whenever an
//!   agent customizer exists, attaches the same dispatcher to every
//!   meerkat-side build of a registered identity member (restart restore,
//!   explicit resume, respawn, delivery-time repair). It is synchronous and
//!   makes no host call: it attaches the stable dispatcher rather than
//!   re-running the host.
//!
//! Members that are not roster identities (helpers, forks, flow-provisioned
//! or raw-spawned members under their own ids) are never registered and get
//! no customizer tools: `customize_build` is a contract over roster
//! identities (it takes a `DurableAgentSpec`). A raw spawn that targets a
//! roster identity's member id does get that identity's current tools.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use meerkat_core::types::{ToolCallView, ToolDef};
use meerkat_core::{
    AgentToolDispatcher, EphemeralToolBindingFingerprint, ResolvedToolExecutionPlan,
    ToolCatalogCapabilities, ToolCatalogEntry, ToolDispatchContext, ToolDispatchOutcome, ToolError,
    ToolExecutionOwnerWitness, ToolExecutionResolutionContext, ToolExecutionResolutionError,
    ToolUnavailableReason,
};
use meerkat_mob::{
    AgentIdentity as MobAgentIdentity, MobError, SpawnCustomizationContext, SpawnMemberCustomizer,
    SpawnMemberSpec,
};

use super::types::AgentIdentity;

/// The session's mob operator authority, as the agent factory binds it.
type SessionMobAuthority =
    Option<Arc<std::sync::RwLock<meerkat_core::service::MobToolAuthorityContext>>>;

/// One `customize_build` result: the dispatcher serving the published tools
/// (`None` when the build declared none) and the publication generation.
/// `mob_authority` is the last session mob operator authority bound on this
/// dispatcher (outer `None` until one is bound); each later publication is
/// bound with it before it serves.
#[derive(Clone)]
struct Published {
    generation: u64,
    dispatcher: Option<Arc<dyn AgentToolDispatcher>>,
    mob_authority: Option<SessionMobAuthority>,
}

/// The stable, dynamic customizer-tool dispatcher of one identity member.
///
/// Every read takes one snapshot of the current publication, so the tools a
/// turn sees and the dispatcher that serves a call come from the same
/// `customize_build` result. A call to a tool the current publication does
/// not advertise is refused typed (`ToolError::NotFound`), never routed to a
/// dispatcher (or handler scope) from an earlier publication.
///
/// Execution plans follow the same rule. A plan is resolved by the dispatcher
/// that serves the tool (so its own owner fencing and its argument-sensitive
/// mode and deadlines are kept), then witnessed against this publication; a
/// later publication, even with identical tool metadata, refuses that plan
/// (`ExecutionOwnerChanged`) and the caller resolves again.
pub struct IdentityCustomizerTools {
    member_id: MobAgentIdentity,
    current: RwLock<Published>,
    /// Distinguishes this dispatcher's plan witnesses from every other
    /// execution authority in the process.
    execution_authority_id: u64,
}

/// Process-wide source of execution authority ids for identity dispatchers.
static NEXT_EXECUTION_AUTHORITY_ID: AtomicU64 = AtomicU64::new(1);

impl IdentityCustomizerTools {
    fn new(member_id: MobAgentIdentity) -> Self {
        Self {
            member_id,
            current: RwLock::new(Published {
                generation: 0,
                dispatcher: None,
                mob_authority: None,
            }),
            execution_authority_id: NEXT_EXECUTION_AUTHORITY_ID.fetch_add(1, Ordering::Relaxed),
        }
    }

    /// The roster member id this dispatcher belongs to.
    pub fn member_id(&self) -> &MobAgentIdentity {
        &self.member_id
    }

    fn snapshot(&self) -> Published {
        self.current
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Publish one `customize_build` result. `None` clears the tools (the
    /// build declared none). Returns the new publication generation.
    pub fn publish(&self, dispatcher: Option<Arc<dyn AgentToolDispatcher>>) -> u64 {
        let mut current = self
            .current
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        current.generation = current.generation.saturating_add(1);
        if let (Some(dispatcher), Some(authority)) = (dispatcher.as_ref(), &current.mob_authority) {
            dispatcher.bind_session_mob_tool_authority(authority.clone());
        }
        current.dispatcher = dispatcher;
        current.generation
    }

    /// How many times a result has been published (0: none yet).
    pub fn generation(&self) -> u64 {
        self.snapshot().generation
    }

    fn owner_for(published: &Published, tool_name: &str) -> Option<Arc<dyn AgentToolDispatcher>> {
        let dispatcher = published.dispatcher.as_ref()?;
        dispatcher
            .tools()
            .iter()
            .any(|tool| tool.name.as_ref() == tool_name)
            .then(|| Arc::clone(dispatcher))
    }

    /// The binding epoch of `tool_name` in `published`: the publication
    /// generation over the serving dispatcher's own epoch.
    fn binding_epoch_in(published: &Published, tool_name: &str) -> u64 {
        let inner = Self::owner_for(published, tool_name)
            .map_or(0, |owner| owner.execution_binding_epoch(tool_name));
        (published.generation << 32) | (inner & 0xFFFF_FFFF)
    }

    /// The binding fingerprint of `tool_name` in `published`: the serving
    /// dispatcher's catalog entry under this publication's epoch, with the
    /// serving dispatcher's own fingerprint as its dependency.
    fn binding_fingerprint_in(
        published: &Published,
        tool_name: &str,
    ) -> Result<EphemeralToolBindingFingerprint, ToolExecutionResolutionError> {
        let not_found = || ToolExecutionResolutionError::NotFound {
            tool_name: tool_name.to_string(),
        };
        let owner = Self::owner_for(published, tool_name).ok_or_else(not_found)?;
        let catalog = owner.tool_catalog();
        let entry = catalog
            .iter()
            .find(|entry| entry.tool.name == tool_name)
            .ok_or_else(not_found)?;
        let child = owner.execution_binding_fingerprint(tool_name)?;
        Ok(
            meerkat_core::ephemeral_tool_catalog_binding_fingerprint(entry)
                .with_live_authority(0, Self::binding_epoch_in(published, tool_name))
                .with_dependency(&child),
        )
    }

    fn execution_authority_key(&self) -> String {
        format!("identity-customizer-tools:{}", self.execution_authority_id)
    }

    fn not_advertised(&self, name: &str) -> ToolError {
        tracing::debug!(
            member_id = %self.member_id,
            tool = name,
            "customizer tool call refused: the current customize_build publication does not \
             advertise it"
        );
        ToolError::NotFound {
            name: name.to_string(),
        }
    }
}

#[async_trait::async_trait]
impl AgentToolDispatcher for IdentityCustomizerTools {
    /// The current publication is bound with the session's mob operator
    /// authority, and so is every later one: a host's tools may hold
    /// operator tools that admit against it.
    fn bind_session_mob_tool_authority(&self, authority: SessionMobAuthority) {
        let mut current = self
            .current
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(dispatcher) = current.dispatcher.as_ref() {
            dispatcher.bind_session_mob_tool_authority(authority.clone());
        }
        current.mob_authority = Some(authority);
    }

    async fn resolve_tool_application(
        &self,
        source_tool: &str,
        request: &meerkat_core::ToolApplicationRequest,
        invocation: &serde_json::Value,
        context: &ToolDispatchContext,
    ) -> Result<meerkat_core::tool_application::ToolApplicationResolution, ToolError> {
        let published = self.snapshot();
        let owner = Self::owner_for(&published, source_tool)
            .ok_or_else(|| self.not_advertised(source_tool))?;
        let changed =
            || ToolError::unavailable(source_tool, ToolUnavailableReason::ExecutionOwnerChanged);
        let before =
            Self::binding_fingerprint_in(&published, source_tool).map_err(|_| changed())?;
        let mut result = owner
            .resolve_tool_application(source_tool, request, invocation, context)
            .await?;
        let current = self.snapshot();
        if current.generation != published.generation
            || Self::binding_fingerprint_in(&current, source_tool).as_ref() != Ok(&before)
        {
            return Err(changed());
        }
        if let meerkat_core::tool_application::ToolApplicationResolution::Call {
            name,
            binding,
            ..
        } = &mut result
        {
            let witness = ToolExecutionOwnerWitness::new(
                self.execution_authority_key(),
                published.generation.to_string(),
                Self::binding_fingerprint_in(&published, name).map_err(ToolError::from)?,
            )
            .map_err(|_| changed())?;
            *binding = binding
                .clone()
                .with_owner_witness(witness)
                .map_err(ToolError::from)?;
        }
        Ok(result)
    }

    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        match self.snapshot().dispatcher {
            Some(dispatcher) => dispatcher.tools(),
            None => Arc::from([]),
        }
    }

    fn tool_catalog_capabilities(&self) -> ToolCatalogCapabilities {
        match self.snapshot().dispatcher {
            Some(dispatcher) => dispatcher.tool_catalog_capabilities(),
            // An empty publication is its own exact (empty) registry.
            None => ToolCatalogCapabilities {
                exact_catalog: true,
                may_require_catalog_control_plane: false,
            },
        }
    }

    fn tool_catalog(&self) -> Arc<[ToolCatalogEntry]> {
        match self.snapshot().dispatcher {
            Some(dispatcher) => dispatcher.tool_catalog(),
            None => Arc::from([]),
        }
    }

    fn tool_mutation_class(&self, tool_name: &str) -> meerkat_core::ToolMutationClass {
        match Self::owner_for(&self.snapshot(), tool_name) {
            Some(owner) => owner.tool_mutation_class(tool_name),
            None => meerkat_core::ToolMutationClass::Unknown,
        }
    }

    fn live_bridge_effect_kind(&self, tool_name: &str) -> meerkat_core::LiveBridgeEffectKind {
        match Self::owner_for(&self.snapshot(), tool_name) {
            Some(owner) => owner.live_bridge_effect_kind(tool_name),
            None => meerkat_core::LiveBridgeEffectKind::ExternalIo,
        }
    }

    fn review_entry_support(
        &self,
        tool_name: &str,
    ) -> meerkat_core::approval::review::ReviewEntrySupport {
        Self::owner_for(&self.snapshot(), tool_name)
            .map(|owner| owner.review_entry_support(tool_name))
            .unwrap_or_default()
    }

    /// A mutable authority: every publication advances the epoch, including
    /// an identical-metadata replacement (a new handler scope behind the same
    /// tool names), so a binding fingerprinted before it is never mistaken
    /// for the current one.
    fn execution_binding_epoch(&self, tool_name: &str) -> u64 {
        Self::binding_epoch_in(&self.snapshot(), tool_name)
    }

    fn execution_binding_fingerprint(
        &self,
        tool_name: &str,
    ) -> Result<EphemeralToolBindingFingerprint, ToolExecutionResolutionError> {
        Self::binding_fingerprint_in(&self.snapshot(), tool_name)
    }

    /// Resolve through the dispatcher that serves the tool, then witness the
    /// publication it was resolved against. The serving dispatcher's plan is
    /// kept whole: its own owner witnesses, its mode and its deadlines.
    fn resolve_execution_plan(
        &self,
        call: ToolCallView<'_>,
        dispatch_context: &ToolDispatchContext,
        resolution_context: &ToolExecutionResolutionContext,
    ) -> Result<ResolvedToolExecutionPlan, ToolExecutionResolutionError> {
        let published = self.snapshot();
        let Some(owner) = Self::owner_for(&published, call.name) else {
            let _ = self.not_advertised(call.name);
            return Err(ToolExecutionResolutionError::NotFound {
                tool_name: call.name.to_string(),
            });
        };
        let before = Self::binding_fingerprint_in(&published, call.name)?;
        let plan = owner.resolve_execution_plan(call, dispatch_context, resolution_context)?;
        // A publication (or an owner rebinding) during resolution: the plan
        // may belong to neither binding.
        let current = self.snapshot();
        if current.generation != published.generation
            || Self::binding_fingerprint_in(&current, call.name).as_ref() != Ok(&before)
        {
            return Err(ToolExecutionResolutionError::Unavailable {
                tool_name: call.name.to_string(),
                reason: ToolUnavailableReason::ExecutionOwnerChanged,
            });
        }
        let witness = ToolExecutionOwnerWitness::new(
            self.execution_authority_key(),
            published.generation.to_string(),
            before,
        )?;
        plan.with_owner_witness(witness)
    }

    /// Validate against the dispatcher that serves the tool now.
    fn validate_resolved_execution_plan(
        &self,
        call: ToolCallView<'_>,
        resolution_context: &ToolExecutionResolutionContext,
        plan: &ResolvedToolExecutionPlan,
    ) -> Result<(), ToolExecutionResolutionError> {
        match Self::owner_for(&self.snapshot(), call.name) {
            Some(owner) => owner.validate_resolved_execution_plan(call, resolution_context, plan),
            None => Err(ToolExecutionResolutionError::NotFound {
                tool_name: call.name.to_string(),
            }),
        }
    }

    fn pending_catalog_sources(&self) -> Arc<[String]> {
        match self.snapshot().dispatcher {
            Some(dispatcher) => dispatcher.pending_catalog_sources(),
            None => Arc::from([]),
        }
    }

    async fn dispatch(&self, call: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        match Self::owner_for(&self.snapshot(), call.name) {
            Some(owner) => owner.dispatch(call).await,
            None => Err(self.not_advertised(call.name)),
        }
    }

    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        match Self::owner_for(&self.snapshot(), call.name) {
            Some(owner) => owner.dispatch_with_context(call, context).await,
            None => Err(self.not_advertised(call.name)),
        }
    }

    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: &ResolvedToolExecutionPlan,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        let published = self.snapshot();
        let Some(owner) = Self::owner_for(&published, call.name) else {
            return Err(self.not_advertised(call.name));
        };
        // Only a plan resolved against THIS publication: an earlier one, even
        // with identical tool metadata, may name another handler scope.
        let owner_changed =
            || ToolError::unavailable(call.name, ToolUnavailableReason::ExecutionOwnerChanged);
        let witness = plan
            .owner_witness(&self.execution_authority_key())
            .ok_or_else(owner_changed)?;
        let current =
            Self::binding_fingerprint_in(&published, call.name).map_err(|_| owner_changed())?;
        if witness.owner_key() != published.generation.to_string()
            || witness.binding_fingerprint() != &current
        {
            return Err(owner_changed());
        }
        owner
            .dispatch_resolved_with_context(call, context, plan)
            .await
    }
}

/// The customizer-tool dispatchers of every registered identity member,
/// keyed by roster member id (the deterministic `mob_member_id(identity)`).
#[derive(Default)]
pub struct CustomizerToolRegistry {
    entries: RwLock<BTreeMap<MobAgentIdentity, Arc<IdentityCustomizerTools>>>,
}

impl CustomizerToolRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn member_id(identity: &AgentIdentity) -> MobAgentIdentity {
        crate::member_comms_id::mob_member_id(identity.as_str())
    }

    /// Register `identity` (idempotent) and return its stable dispatcher.
    pub fn ensure(&self, identity: &AgentIdentity) -> Arc<IdentityCustomizerTools> {
        let member_id = Self::member_id(identity);
        if let Some(existing) = self
            .entries
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&member_id)
        {
            return Arc::clone(existing);
        }
        let mut entries = self
            .entries
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(
            entries
                .entry(member_id.clone())
                .or_insert_with(|| Arc::new(IdentityCustomizerTools::new(member_id))),
        )
    }

    /// Register a roster member id directly (idempotent).
    pub fn ensure_member(&self, member_id: &MobAgentIdentity) -> Arc<IdentityCustomizerTools> {
        if let Some(existing) = self.for_member(member_id) {
            return existing;
        }
        let mut entries = self
            .entries
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Arc::clone(
            entries
                .entry(member_id.clone())
                .or_insert_with(|| Arc::new(IdentityCustomizerTools::new(member_id.clone()))),
        )
    }

    /// The registered dispatcher for a roster member id, if any.
    pub fn for_member(&self, member_id: &MobAgentIdentity) -> Option<Arc<IdentityCustomizerTools>> {
        self.entries
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(member_id)
            .cloned()
    }

    /// Publish one `customize_build` result for `identity` (registering it
    /// if needed) and return the identity's stable dispatcher, which is what
    /// every spawn of that identity attaches.
    pub fn publish(
        &self,
        identity: &AgentIdentity,
        dispatcher: Option<Arc<dyn AgentToolDispatcher>>,
    ) -> Arc<IdentityCustomizerTools> {
        let entry = self.ensure(identity);
        entry.publish(dispatcher);
        entry
    }
}

/// Run the host customizer for each roster identity and publish its tools
/// into `registry`, BEFORE meerkat builds any member (#563). Returns the
/// identities whose `customize_build` failed, with the typed reason; they stay
/// registered with nothing published until their materialization publishes.
/// Each failure is logged per identity.
pub async fn prepublish(
    registry: &CustomizerToolRegistry,
    roster: &[super::types::DurableAgentSpec],
    customizer: &dyn super::contracts::AgentCustomizer,
    runtime_services: super::types::AgentRuntimeServices,
    active_peers: &[AgentIdentity],
    managed_edges: &[super::types::ManagedPeerEdge],
) -> BTreeMap<AgentIdentity, super::types::CustomizerToolsPending> {
    prepublish_logged(
        registry,
        roster,
        customizer,
        runtime_services,
        active_peers,
        managed_edges,
        PrepublishFailureLog::Warn,
    )
    .await
}

/// [`prepublish`] for the mob build's early publication, which runs before
/// the mob handle exists, so a customizer whose tools need it always fails
/// there. The identity bootstrap's publication always follows it with the
/// runtime's services and logs its own failures, so these are logged at
/// DEBUG.
pub(crate) async fn prepublish_before_mob_build(
    registry: &CustomizerToolRegistry,
    roster: &[super::types::DurableAgentSpec],
    customizer: &dyn super::contracts::AgentCustomizer,
    runtime_services: super::types::AgentRuntimeServices,
) -> BTreeMap<AgentIdentity, super::types::CustomizerToolsPending> {
    prepublish_logged(
        registry,
        roster,
        customizer,
        runtime_services,
        &[],
        &[],
        PrepublishFailureLog::DeferredToBootstrap,
    )
    .await
}

/// Why a member's customizer tools wait when no publication before the mob
/// build covers them.
const PUBLISHED_WHEN_THE_IDENTITY_MATERIALIZES: &str =
    "customizer tools are published when the identity materializes, after the mob build";

/// Each identity's customizer tools as pending, for identities no
/// publication before the mob build covers: every roster identity of a
/// composition that publishes none before its build (the gateway composes
/// its agent customizer only after it), whose materialization publishes
/// them as it publishes those an early publication could not, and a
/// restored member's identity the roster no longer lists
/// ([`unlisted_stored_identities`]).
pub(crate) fn unpublished_before_mob_build(
    identities: impl IntoIterator<Item = AgentIdentity>,
) -> BTreeMap<AgentIdentity, super::types::CustomizerToolsPending> {
    identities
        .into_iter()
        .map(|identity| {
            (
                identity,
                super::types::CustomizerToolsPending {
                    reason: PUBLISHED_WHEN_THE_IDENTITY_MATERIALIZES.to_string(),
                    artifact_refusal: None,
                },
            )
        })
        .collect()
}

/// The identities of the live members `stored` names that `roster` does
/// not list. `stored` is read from the mob store before the build
/// (`MobBootstrapSpec::stored_command_driven_identity_members`, which leaves
/// out the identities the roster pass never retires, and the durable members
/// spawned at runtime, which the roster never materialized): a resume of a
/// log left Running revives those members like any other, but no
/// publication covers them, so their run starts are held until the roster
/// pass retires them. A member id that names no valid identity is left out;
/// the roster pass refuses it.
pub(crate) fn unlisted_stored_identities(
    roster: &[super::types::DurableAgentSpec],
    stored: &[meerkat_mob::AgentIdentity],
) -> Vec<AgentIdentity> {
    let listed = roster
        .iter()
        .map(|spec| &spec.identity)
        .collect::<std::collections::BTreeSet<_>>();
    let unlisted = stored
        .iter()
        .filter_map(|member| {
            let durable =
                crate::member_comms_id::durable_identity_for_roster_member(member.as_str());
            match AgentIdentity::parse(&durable) {
                Ok(identity) => Some(identity),
                Err(error) => {
                    tracing::warn!(
                        %member,
                        %error,
                        "a stored durable identity member names no valid identity; its run \
                         starts are not held"
                    );
                    None
                }
            }
        })
        .filter(|identity| !listed.contains(identity))
        .collect::<Vec<_>>();
    if !unlisted.is_empty() {
        tracing::info!(
            identities = ?unlisted,
            "the mob build holds the run starts of restored members the roster no longer \
             lists until the roster pass retires them"
        );
    }
    unlisted
}

#[derive(Clone, Copy)]
enum PrepublishFailureLog {
    Warn,
    DeferredToBootstrap,
}

async fn prepublish_logged(
    registry: &CustomizerToolRegistry,
    roster: &[super::types::DurableAgentSpec],
    customizer: &dyn super::contracts::AgentCustomizer,
    runtime_services: super::types::AgentRuntimeServices,
    active_peers: &[AgentIdentity],
    managed_edges: &[super::types::ManagedPeerEdge],
    failure_log: PrepublishFailureLog,
) -> BTreeMap<AgentIdentity, super::types::CustomizerToolsPending> {
    let mut pending = BTreeMap::new();
    for spec in roster {
        let build_context = super::types::AgentBuildContext {
            identity: spec.identity.clone(),
            active_peers: active_peers.to_vec(),
            managed_edges: managed_edges.to_vec(),
            runtime_services: runtime_services.clone(),
        };
        let mut draft = super::types::AgentBuildDraft {
            model: None,
            system_prompt: None,
            additional_instructions: spec.additional_instructions.clone(),
            labels: spec.labels.clone(),
            app_context: spec.context.clone(),
            external_tools: Vec::new(),
            local_external_tools: Default::default(),
            provider_params: None,
            compaction_curator: Default::default(),
        };
        match customizer
            .customize_build(&build_context, spec, &mut draft)
            .await
        {
            Ok(()) => {
                registry.publish(&spec.identity, draft.local_external_tools.dispatcher());
            }
            Err(error) => {
                let reason = format!("pre-activation customize_build failed: {error}");
                let artifact_refusal = match &error {
                    super::types::CustomizerError::ToolArtifactRefused(refusal) => {
                        Some(refusal.as_ref().clone())
                    }
                    _ => None,
                };
                match failure_log {
                    PrepublishFailureLog::Warn => tracing::warn!(
                        identity = %spec.identity,
                        %reason,
                        "restored member's customizer tools are not published; it advertises \
                         none until its materialization publishes them"
                    ),
                    PrepublishFailureLog::DeferredToBootstrap => tracing::debug!(
                        identity = %spec.identity,
                        %reason,
                        "customizer tools not published before the mob build; deferred to the \
                         identity bootstrap's publication, which runs with the runtime's services"
                    ),
                }
                registry.ensure(&spec.identity);
                pending.insert(
                    spec.identity.clone(),
                    super::types::CustomizerToolsPending {
                        reason,
                        artifact_refusal,
                    },
                );
            }
        }
    }
    pending
}

fn same_dispatcher(a: &Arc<dyn AgentToolDispatcher>, b: &Arc<IdentityCustomizerTools>) -> bool {
    std::ptr::eq(Arc::as_ptr(a).cast::<()>(), Arc::as_ptr(b).cast::<()>())
}

/// Attaches a registered identity member's customizer-tool dispatcher to
/// every meerkat-side build of it: restart restore and explicit resume
/// (`SpawnSource::Resume`), respawn, delivery-time repair, and raw spawns
/// that target the identity's member id. Members that are not registered
/// identities are left untouched.
pub struct CustomizerToolsSpawnCustomizer {
    registry: Arc<CustomizerToolRegistry>,
}

impl CustomizerToolsSpawnCustomizer {
    pub fn new(registry: Arc<CustomizerToolRegistry>) -> Self {
        Self { registry }
    }
}

impl SpawnMemberCustomizer for CustomizerToolsSpawnCustomizer {
    fn customize_spawn(
        &self,
        _ctx: &SpawnCustomizationContext,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError> {
        self.apply(spec);
        Ok(())
    }
}

impl CustomizerToolsSpawnCustomizer {
    /// The customizer body, factored off the trait so tests can drive it:
    /// `SpawnCustomizationContext` is `#[non_exhaustive]` and only
    /// meerkat-mob constructs it. The attachment does not depend on the spawn
    /// source: every build of a registered identity member gets it.
    fn apply(&self, spec: &mut SpawnMemberSpec) {
        // A member Meta built for an identity carries the
        // runtime-authoritative `agent_identity` label (raw member creation
        // may not supply it), and the roster entry keeps it. That marks a
        // restored identity member even when meerkat restores the mob before
        // Meta has seen the roster, so it is registered here, at its first
        // build, and receives the identity's tools when they are published.
        let entry = match self.registry.for_member(&spec.identity) {
            Some(entry) => entry,
            None if spec
                .labels
                .as_ref()
                .and_then(crate::member_comms_id::durable_identity_label)
                .is_some() =>
            {
                self.registry.ensure_member(&spec.identity)
            }
            None => return,
        };
        spec.external_tools = Some(match spec.external_tools.take() {
            None => entry,
            // Meta's own identity spawn already attached it.
            Some(existing) if same_dispatcher(&existing, &entry) => existing,
            // A raw spawn of the identity's member id that brought its own
            // overlay: the customizer tools win name collisions, the rest
            // stays reachable.
            Some(existing) => {
                crate::tool_compose::ComposedExternalTools::over(entry, Some(existing))
            }
        });
    }
}

/// Runs several spawn customizers in order: this crate's one composer.
/// meerkat-mob has a single `SpawnMemberCustomizer` slot; composing keeps a
/// later installer from silently replacing an earlier one. Both hooks reach
/// every customizer, so a customizer's own `customize_resume` still runs when
/// a member is rebuilt on its durable session.
pub struct ComposedSpawnMemberCustomizer {
    chain: meerkat_mob::SpawnMemberCustomizerChain,
}

impl ComposedSpawnMemberCustomizer {
    /// `existing` (if any) runs first, then `added`.
    pub fn over(
        existing: Option<Arc<dyn SpawnMemberCustomizer>>,
        added: Arc<dyn SpawnMemberCustomizer>,
    ) -> Arc<dyn SpawnMemberCustomizer> {
        match existing {
            None => added,
            Some(existing) => Arc::new(Self {
                chain: meerkat_mob::SpawnMemberCustomizerChain::new()
                    .then(existing)
                    .then(added),
            }),
        }
    }
}

impl SpawnMemberCustomizer for ComposedSpawnMemberCustomizer {
    fn customize_spawn(
        &self,
        ctx: &SpawnCustomizationContext,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError> {
        self.chain.customize_spawn(ctx, spec)
    }

    fn customize_resume(
        &self,
        ctx: &SpawnCustomizationContext,
        durable: &meerkat_mob::ResumedMemberView,
        spec: &mut SpawnMemberSpec,
    ) -> Result<(), MobError> {
        self.chain.customize_resume(ctx, durable, spec)
    }
}

#[cfg(test)]
#[path = "customizer_tools_tests.rs"]
mod tests;
