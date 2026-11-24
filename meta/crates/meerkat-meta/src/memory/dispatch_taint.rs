//! Dispatch-ordered content-trust marking for mob member agents (§10.1,
//! closes the first-ingestion race - see the `taint` module docs).
//!
//! The observe-only agent-event stream is asynchronous, so a memory write in
//! the same turn as the session's FIRST untrusted tool ingestion could reach
//! the store before the taint observer processed the tool event. This module
//! joins content trust into the member's synchronous execution path instead:
//!
//! - meerkat 0.8.14 fires `HookPoint::PostToolExecution` synchronously with
//!   the typed `ToolProvenance` (the loop blocks on the report:
//!   meerkat-core/src/agent/state.rs:5010-5035 at the 0.8.14 pin, payload
//!   provenance at meerkat-core/src/hooks.rs:273), but the mob member build
//!   path has no hook-engine carrier: `SessionBuildOptions` cannot ship an
//!   `Arc<dyn HookEngine>`, `FactoryAgentBuilder` injects no hook-engine
//!   default (meerkat/src/service_factory.rs `build_agent`), and
//!   `AgentBuildConfig.hook_engine_override` is set only by the standalone
//!   facade builder (meerkat/src/agent_builder.rs:187) - never reachable
//!   from `MobSessionService` member creates. The sanctioned per-build seam
//!   that IS synchronous with the loop is
//!   `SessionBuildOptions.agent_llm_client_decorator`.
//!
//!   THIS MODULE DOES NOT COLLAPSE ONTO THE HOOK POINTS THAT LANDED. An
//!   earlier version of this comment said "re-audit those seams when a hook
//!   slot lands upstream; this module then collapses onto it". Six slots
//!   landed in meerkat 0.8.31 - RuntimeInputAccepted, RuntimeInputRejected,
//!   RuntimeInputDeduplicated, PeerIngressCommitted and two more - and they
//!   are POST-COMMIT OBSERVE-ONLY, which is the one thing this module cannot
//!   use. meerkat's own hooks reference says it plainly: "Post-commit hooks
//!   are not synchronous policy seams. They cannot close an in-turn race or
//!   gate a same-turn write. Keep using synchronous points such as
//!   `PostToolExecution` when policy must join the agent loop before it
//!   advances."
//!
//!   Closing an in-turn race is this module's entire purpose, so the correct
//!   upstream target remains a SYNCHRONOUS hook carrier reachable from a mob
//!   member build - `PostToolExecution` with a hook-engine slot on
//!   `SessionBuildOptions`. Until that exists, the decorator seam stays.
//! - [`TaintObservingLlmClient`] therefore wraps the member's final
//!   agent-facing LLM client. Before delegating each call it classifies the
//!   tool results newly present in the request - joining the tool name
//!   against the request's typed `ToolDef.provenance` catalog - and marks
//!   the [`SessionTaintTracker`]. After the call returns it classifies the
//!   typed `ServerToolContent` blocks (provider-executed web search /
//!   grounding) before the loop can see them.
//!
//! Ordering guarantee: an LLM-authored memory write is a tool call in some
//! response R, and content derived from an untrusted tool result T can only
//! appear in R if T rode the request that produced R - which this wrapper
//! classified before sending. Provider-executed server tools are classified
//! before the loop can dispatch any same-response tool call. Either way the
//! tracker is marked strictly before the write reaches the store's gate.
//! The async observer stays wired as belt-and-suspenders (it also serves
//! session-rotation mirroring); this join simply gets there first.
//!
//! The hook path is observe-and-mark only: it never denies, never mutates
//! the request or response, and does no I/O (the tracker is in-memory).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, RwLock};

use meerkat_core::model_fallback::ModelFallbackRequest;
use meerkat_core::service::{CreateSessionRequest, SessionBuildOptions};
use meerkat_core::types::{AssistantBlock, Message, ToolDef};
use meerkat_core::{
    AgentError, AgentLlmClient, AgentLlmClientDecorator, AgentLlmFallbackSkippedTarget,
    AgentLlmFallbackSwitch, AgentLlmRequestAttempt, AssistantMessageId, CompiledSchema,
    ControllerFactsUnavailable, ControllerModelClient, ControllerModelFacts,
    ControllerModelSelection, LlmRequestAuthorization, LlmStreamResult, OutputSchema,
    ProviderParamsOverride, ProviderRequestPressure, SchemaError, SessionLlmIdentity,
    TargetCacheLoweringCapability, TargetCacheLoweringIssuer,
};

use crate::member_comms_id;
use crate::memory::taint::SessionTaintTracker;

/// The session builder's own default agent-LLM-client decorator cell
/// (`FactoryAgentBuilder::default_agent_llm_client_decorator`): the same
/// shared cell the factory reads when a request carries no decorator.
pub type HostDefaultDecoratorSource = Arc<RwLock<Option<AgentLlmClientDecorator>>>;

/// Late-bound tracker slot shared between the member pre-build seam (which
/// installs the decorator at bootstrap, before the memory stack exists) and
/// the memory-stack attach (which fills it). Cheap to clone; clones share
/// the slot. An unfilled slot makes every installed decorator a pure
/// pass-through, so compositions without the taint firewall pay nothing.
///
/// It also carries the session builder's default decorator cell, when the
/// composition binds it: the member decorator composes over the effective
/// host decorator, and when the request has none that is the builder's
/// default.
///
/// A launch that hosts mobs beside its launch mob keeps one tracker per
/// memory: its own, and one for each mob it composes beside its launch mob.
/// Which of them observes a member is asked at each of the member's model
/// calls, of the launch's record of its mobs
/// (`PrivateOwners::memory_home_of`) for the mob
/// the member's build is bound to: never of a name, a label or an id a
/// caller supplied.
#[derive(Clone, Default)]
pub struct DispatchTaintSlot {
    inner: Arc<RwLock<Option<SessionTaintTracker>>>,
    host_default: Arc<RwLock<Option<HostDefaultDecoratorSource>>>,
    /// What says which memory the members of a mob are members of: the
    /// launch's record of its mobs. A slot without one observes every
    /// member build with its one tracker.
    homes: Arc<RwLock<Option<crate::private_owners::PrivateOwners>>>,
    /// The trackers of the mobs the launch composes beside its launch mob,
    /// by mob id, and what makes one ([`HostedMobTrackers`]).
    of_hosted_mobs: Arc<RwLock<HostedMobTrackers>>,
}

/// The trackers of the mobs a launch composes beside its launch mob. A
/// mob's tracker is made when it is first asked for and kept for the life
/// of the process, so what a member took in stays marked while its mob is
/// unloaded and loaded again, as the launch mob's marks stay.
#[derive(Default)]
struct HostedMobTrackers {
    /// Makes the tracker of a mob that has none yet; none until the launch
    /// hands its memory over for the mobs it hosts.
    make: Option<Arc<dyn Fn() -> SessionTaintTracker + Send + Sync>>,
    by_mob: BTreeMap<String, SessionTaintTracker>,
}

impl DispatchTaintSlot {
    /// From now on which memory observes a member build is asked of
    /// `owners`, the launch's record of its mobs, for the mob the build is
    /// bound to.
    pub(crate) fn decide_by(&self, owners: crate::private_owners::PrivateOwners) {
        *self
            .homes
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(owners);
    }

    /// From now on a mob the launch composes beside its launch mob gets a
    /// tracker of its own, made by `make` when it is first asked for.
    pub(crate) fn make_hosted_mob_trackers_with(
        &self,
        make: Arc<dyn Fn() -> SessionTaintTracker + Send + Sync>,
    ) {
        self.of_hosted_mobs
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .make = Some(make);
    }

    /// The tracker of the mob `mob_id`, which the launch composes beside
    /// its launch mob: the one the mob's memory answers its write gate
    /// from and the one its members' model calls mark. `None` while the
    /// launch has handed no memory over for the mobs it hosts.
    pub(crate) fn tracker_of_hosted_mob(&self, mob_id: &str) -> Option<SessionTaintTracker> {
        let mut trackers = self
            .of_hosted_mobs
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(tracker) = trackers.by_mob.get(mob_id) {
            return Some(tracker.clone());
        }
        let tracker = trackers.make.as_ref()?();
        trackers.by_mob.insert(mob_id.to_string(), tracker.clone());
        Some(tracker)
    }

    /// Forget the tracker of the mob `mob_id`: the mob is destroyed, or a
    /// mob is created anew under its id, and the new mob's members start
    /// unmarked.
    pub(crate) fn forget_hosted_mob(&self, mob_id: &str) {
        self.of_hosted_mobs
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .by_mob
            .remove(mob_id);
    }

    /// The tracker that observes a member of the mob `mob_id` now; `None`
    /// for a member no memory observes.
    ///
    /// A member of the launch's memory is observed by the launch's
    /// tracker, a member of a mob with a memory of its own by that mob's,
    /// and a member of a mob whose members are members of no memory by
    /// none. A member of a mob whose memory is not decided is observed by
    /// the launch's tracker, as every member was before a launch hosted a
    /// second mob: an observer that cannot tell keeps observing, so no
    /// model call goes unobserved while the launch's record is still
    /// reading.
    fn tracker_of(&self, mob_id: Option<&str>) -> Option<SessionTaintTracker> {
        use crate::private_owners::MemoryHome;
        let homes = self
            .homes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let (Some(mob_id), Some(homes)) = (mob_id, homes) else {
            return self.tracker();
        };
        match homes.memory_home_of(mob_id) {
            MemoryHome::Launch | MemoryHome::NotKnown => self.tracker(),
            MemoryHome::Hosted(mob) => self.tracker_of_hosted_mob(&mob),
            MemoryHome::Nowhere => None,
        }
    }

    /// Bind the live tracker. Called once when the memory stack attaches;
    /// decorators installed earlier (bootstrap members) pick it up on their
    /// next LLM call because they read the slot per call.
    pub fn fill(&self, tracker: SessionTaintTracker) {
        *self
            .inner
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(tracker);
    }

    fn tracker(&self) -> Option<SessionTaintTracker> {
        self.inner
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Bind the session builder's own default decorator cell, the one its
    /// factory reads. Member creates then compose taint over that default
    /// when the request carries no decorator of its own.
    pub fn bind_host_default_decorator(&self, source: HostDefaultDecoratorSource) {
        *self
            .host_default
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(source);
    }

    /// The builder default decorator currently selected, read from the bound
    /// cell at create time as the factory would.
    fn host_default_decorator(&self) -> Option<AgentLlmClientDecorator> {
        let source = self
            .host_default
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()?;
        source
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl std::fmt::Debug for DispatchTaintSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DispatchTaintSlot")
            .field("filled", &self.tracker().is_some())
            .field("host_default", &self.host_default_decorator().is_some())
            .finish()
    }
}

/// Resolve the tracker identity for one member session create, in the
/// spelling the write gate keys on (`MemoryAuthor::Agent { identity }`).
///
/// The mob member binding is the one honest source: meerkat-mob stamps it on
/// every member build, and it also overwrites the `agent_identity` label
/// with the encoded roster id, so labels carry no extra information here.
/// Decoding the roster id yields the public alias; identity-first internal
/// members roster under the encoded DURABLE identity, so decoding lands on that
/// identity directly. A generated `rt:{identity}:{generation}` alias still
/// normalizes to the same durable identity, but it is incarnation detail and no
/// longer names a roster row.
fn member_taint_identity(req: &CreateSessionRequest) -> Option<String> {
    let binding = req.build.as_ref()?.mob_member_binding.as_ref()?;
    Some(member_comms_id::logical_memory_identity(&binding.member))
}

/// Install (or compose over) the request's LLM-client decorator so the built
/// member agent carries the dispatch-time taint join. No-op for requests
/// that are not member builds (no mob member binding) - the
/// bridge/supervisor session keeps its plain client.
///
/// Taint wraps the effective host decorator, which runs once: the request's
/// own decorator when it has one, otherwise the session builder's default
/// (the bound cell). The factory applies its default only to a request with
/// no decorator, so supplying the default here does not run it twice.
///
/// Every member build carries the join, whichever mob it is a member of:
/// which memory observes the member is asked at each of its model calls
/// ([`DispatchTaintSlot::tracker_of`]), for the mob its build is bound to.
pub(crate) fn attach_member_taint_decorator(
    req: &mut CreateSessionRequest,
    slot: &DispatchTaintSlot,
) {
    let Some(identity) = member_taint_identity(req) else {
        return;
    };
    // `member_taint_identity` proved `build` present (the binding rides it).
    let build = req.build.get_or_insert_with(SessionBuildOptions::default);
    let mob_id = build
        .mob_member_binding
        .as_ref()
        .map(|binding| binding.mob_id.clone());
    let prior = build
        .agent_llm_client_decorator
        .take()
        .or_else(|| slot.host_default_decorator());
    let slot = slot.clone();
    build.agent_llm_client_decorator = Some(Arc::new(move |client| {
        let client = match prior.as_ref() {
            Some(prior) => prior(client),
            None => client,
        };
        Arc::new(
            TaintObservingLlmClient::new(client, identity.clone(), slot.clone())
                .of_a_member_of(mob_id.clone()),
        )
    }));
}

/// Observe-and-mark wrapper over the member's final agent-facing LLM client.
/// Never denies, never mutates: every method delegates verbatim; the only
/// side effect is marking the in-memory taint tracker. "Every method"
/// includes each `AgentLlmClient` method with a trait default: a default
/// the wrapper does not override answers for the wrapper, not for the client
/// it wraps (the controller surface defaults to none, which strips a
/// governed member of its controller).
pub struct TaintObservingLlmClient {
    inner: Arc<dyn AgentLlmClient>,
    identity: String,
    /// The mob the member's build is bound to, as the mob set wrote it on
    /// the build; `None` for a client that observes with the slot's own
    /// tracker.
    mob_id: Option<String>,
    slot: DispatchTaintSlot,
    /// Messages already classified. The transcript is append-only within a
    /// session; a shrink (compaction rebuild) resets the cursor and rescans
    /// - marking is idempotent, so a rescan only costs time.
    scanned: Mutex<usize>,
}

impl TaintObservingLlmClient {
    pub fn new(inner: Arc<dyn AgentLlmClient>, identity: String, slot: DispatchTaintSlot) -> Self {
        Self {
            inner,
            identity,
            mob_id: None,
            slot,
            scanned: Mutex::new(0),
        }
    }

    /// This client is a member's of the mob `mob_id`: the memory that
    /// observes it is that mob's members', asked at each call.
    fn of_a_member_of(mut self, mob_id: Option<String>) -> Self {
        self.mob_id = mob_id;
        self
    }

    /// The tracker that observes this client now.
    fn tracker(&self) -> Option<SessionTaintTracker> {
        self.slot.tracker_of(self.mob_id.as_deref())
    }

    /// Classify every tool result newly appended since the last call. Tool
    /// results carry only `tool_use_id`; the paired assistant `ToolUse`
    /// block (same appended window - results never precede their call)
    /// supplies the name, and the request's tool catalog supplies the typed
    /// provenance for the dispatch-time MCP attribution.
    fn mark_request_ingestions(
        &self,
        tracker: &SessionTaintTracker,
        messages: &[Message],
        tools: &[Arc<ToolDef>],
    ) {
        let mut scanned = self
            .scanned
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let start = if *scanned > messages.len() {
            0
        } else {
            *scanned
        };
        let mut names: HashMap<&str, &str> = HashMap::new();
        for message in &messages[start..] {
            match message {
                Message::BlockAssistant(assistant) => {
                    for block in &assistant.blocks {
                        match block {
                            AssistantBlock::ToolUse { id, name, .. } => {
                                names.insert(id.as_str(), name.as_str());
                            }
                            // Server-tool evidence persisted into the
                            // transcript: re-marks idempotently, and covers
                            // resumed sessions whose in-memory taint state
                            // was lost with the process.
                            AssistantBlock::ServerToolContent { kind, .. } => {
                                tracker.observe_dispatched_server_tool(&self.identity, kind);
                            }
                            _ => {}
                        }
                    }
                }
                Message::ToolResults { results, .. } => {
                    for result in results {
                        // A result whose call fell outside the window has no
                        // name to classify on; the observe-stream fallback
                        // still covers it. Errors classify like successes -
                        // an error body is attacker-influenced text too.
                        let Some(name) = names.get(result.tool_use_id.as_str()) else {
                            continue;
                        };
                        let provenance = tools
                            .iter()
                            .find(|tool| tool.name.as_ref() == *name)
                            .and_then(|tool| tool.provenance.as_ref());
                        tracker.observe_dispatched_tool_result(&self.identity, name, provenance);
                    }
                }
                _ => {}
            }
        }
        *scanned = messages.len();
    }

    /// Classify the provider-executed server-tool content of one response
    /// before the loop can see it.
    fn mark_response_server_tools(&self, result: &LlmStreamResult) {
        if let Some(tracker) = self.tracker() {
            for block in result.blocks() {
                if let AssistantBlock::ServerToolContent { kind, .. } = block {
                    tracker.observe_dispatched_server_tool(&self.identity, kind);
                }
            }
        }
    }

    /// The same observation over another client of this member (its pinned
    /// controller child or a live-bridge fork), with its own cursor.
    fn observe(&self, inner: Arc<dyn AgentLlmClient>) -> Self {
        Self::new(inner, self.identity.clone(), self.slot.clone())
            .of_a_member_of(self.mob_id.clone())
    }
}

struct TaintObservingRequestAttempt {
    client: Arc<TaintObservingLlmClient>,
    inner: Arc<dyn AgentLlmRequestAttempt>,
    messages: Arc<Vec<Message>>,
    tools: Arc<[Arc<ToolDef>]>,
}

#[async_trait::async_trait]
impl AgentLlmRequestAttempt for TaintObservingRequestAttempt {
    fn request_pressure(&self) -> Result<Option<ProviderRequestPressure>, AgentError> {
        self.inner.request_pressure()
    }

    async fn stream_response(
        &self,
        assistant_message_id: AssistantMessageId,
    ) -> Result<LlmStreamResult, AgentError> {
        if let Some(tracker) = self.client.tracker() {
            self.client
                .mark_request_ingestions(&tracker, &self.messages, &self.tools);
        }
        let result = self.inner.stream_response(assistant_message_id).await?;
        if let Some(tracker) = self.client.tracker() {
            for block in result.blocks() {
                if let AssistantBlock::ServerToolContent { kind, .. } = block {
                    tracker.observe_dispatched_server_tool(&self.client.identity, kind);
                }
            }
        }
        Ok(result)
    }
}

#[async_trait::async_trait]
impl AgentLlmClient for TaintObservingLlmClient {
    fn prepare_request_attempt(
        self: Arc<Self>,
        messages: Arc<Vec<Message>>,
        tools: Arc<[Arc<ToolDef>]>,
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<ProviderParamsOverride>,
    ) -> Result<Arc<dyn AgentLlmRequestAttempt>, AgentError> {
        let inner = self.inner.clone().prepare_request_attempt(
            messages.clone(),
            tools.clone(),
            max_tokens,
            temperature,
            provider_params,
        )?;
        Ok(Arc::new(TaintObservingRequestAttempt {
            client: self,
            inner,
            messages,
            tools,
        }))
    }

    // Forward the inner client's request-attempt authority. `AgentLlmClient`
    // gives this method a DEFAULT returning `LegacySplit`, so a decorator that
    // omits it compiles cleanly and silently downgrades every wrapped client.
    // meerkat 0.8.31 rejects resume for a client reporting LegacySplit when the
    // inner adapter is Unified: production measured 72 identities marked Broken at boot
    // on the candidate and 0 across three 0.8.30 runs. A decorator must report
    // what it wraps, never what it is.
    fn request_attempt_authority(&self) -> meerkat_core::RequestAttemptAuthority {
        self.inner.request_attempt_authority()
    }

    // The controller surface. Its trait defaults are "none" or a refusal, so
    // a wrapper that omits these strips the member of its governed
    // controller: the actor's controller pin is `None` and governed
    // admission refuses the member as malformed.
    fn controller_model_selection(&self) -> Option<ControllerModelSelection> {
        self.inner.controller_model_selection()
    }

    // The route the wrapped client serves, for the provider-neutral
    // reasoning effort option. The trait's default is no route, and a level
    // set on a member whose client reports none is refused as unknown.
    fn reasoning_route(&self) -> Option<meerkat_core::ReasoningRouteObservation> {
        self.inner.reasoning_route()
    }

    // The default tag of the wrapped client. The trait's default is no tag,
    // and a level resolved for a member whose client hides its tag would
    // replace that client's default fields instead of joining them.
    fn default_provider_tag(&self) -> Option<meerkat_core::lifecycle::run_primitive::ProviderTag> {
        self.inner.default_provider_tag()
    }

    fn controller_model_facts(&self) -> Result<ControllerModelFacts, ControllerFactsUnavailable> {
        self.inner.controller_model_facts()
    }

    async fn prepare_controller_credential(&self) -> Result<(), meerkat_core::auth::AuthError> {
        self.inner.prepare_controller_credential().await
    }

    // Governed controller requests run through the pinned client, not this
    // one, so the pin keeps the observation: the wrapped client's pinned
    // child (its immutable selection), observed for the same member.
    fn pin_controller(self: Arc<Self>) -> Option<ControllerModelClient> {
        let pinned = Arc::clone(&self.inner).pin_controller()?;
        let observed: Arc<dyn AgentLlmClient> = Arc::new(self.observe(Arc::clone(pinned.client())));
        Some(ControllerModelClient::new(
            pinned.selection().clone(),
            observed,
        ))
    }

    // The default refuses an admitted work authorization rather than drop
    // it; the wrapped client is the one that decides.
    fn prepare_request_attempt_authorized(
        self: Arc<Self>,
        messages: Arc<Vec<Message>>,
        tools: Arc<[Arc<ToolDef>]>,
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<ProviderParamsOverride>,
        authorization: Option<LlmRequestAuthorization>,
    ) -> Result<Arc<dyn AgentLlmRequestAttempt>, AgentError> {
        let inner = self.inner.clone().prepare_request_attempt_authorized(
            messages.clone(),
            tools.clone(),
            max_tokens,
            temperature,
            provider_params,
            authorization,
        )?;
        Ok(Arc::new(TaintObservingRequestAttempt {
            client: self,
            inner,
            messages,
            tools,
        }))
    }

    async fn stream_response_authorized(
        &self,
        messages: &[Message],
        tools: &[Arc<ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&ProviderParamsOverride>,
        authorization: Option<LlmRequestAuthorization>,
    ) -> Result<LlmStreamResult, AgentError> {
        if let Some(tracker) = self.tracker() {
            self.mark_request_ingestions(&tracker, messages, tools);
        }
        let result = self
            .inner
            .stream_response_authorized(
                messages,
                tools,
                max_tokens,
                temperature,
                provider_params,
                authorization,
            )
            .await?;
        self.mark_response_server_tools(&result);
        Ok(result)
    }

    // The wrapper does not transform the request, so the wrapped client's
    // lowering is the one this request would use.
    fn target_cache_lowering_capabilities(
        &self,
        issuer: &TargetCacheLoweringIssuer,
        messages: &[Message],
        tools: &[Arc<ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&ProviderParamsOverride>,
    ) -> Result<Vec<TargetCacheLoweringCapability>, AgentError> {
        self.inner.target_cache_lowering_capabilities(
            issuer,
            messages,
            tools,
            max_tokens,
            temperature,
            provider_params,
        )
    }

    // A live-bridge fork of this member is observed for the same member.
    fn fork_noncommitting_live_bridge(&self) -> Result<Arc<dyn AgentLlmClient>, AgentError> {
        let fork = self.inner.fork_noncommitting_live_bridge()?;
        Ok(Arc::new(self.observe(fork)))
    }

    async fn stream_response(
        &self,
        messages: &[Message],
        tools: &[Arc<ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&ProviderParamsOverride>,
    ) -> Result<LlmStreamResult, AgentError> {
        if let Some(tracker) = self.tracker() {
            self.mark_request_ingestions(&tracker, messages, tools);
        }
        let result = self
            .inner
            .stream_response(messages, tools, max_tokens, temperature, provider_params)
            .await?;
        self.mark_response_server_tools(&result);
        Ok(result)
    }

    fn request_pressure(
        &self,
        messages: &[Message],
        tools: &[Arc<ToolDef>],
        max_tokens: u32,
        temperature: Option<f32>,
        provider_params: Option<&ProviderParamsOverride>,
    ) -> Result<Option<ProviderRequestPressure>, AgentError> {
        self.inner
            .request_pressure(messages, tools, max_tokens, temperature, provider_params)
    }

    fn provider(&self) -> meerkat_core::Provider {
        self.inner.provider()
    }

    fn model(&self) -> &str {
        self.inner.model()
    }

    fn prepare_model_fallback(
        &self,
        failure: &AgentError,
        request: &ModelFallbackRequest<'_>,
    ) -> Result<AgentLlmFallbackSwitch, Vec<AgentLlmFallbackSkippedTarget>> {
        self.inner.prepare_model_fallback(failure, request)
    }

    fn commit_model_fallback(
        &self,
        previous_identity: &SessionLlmIdentity,
        target_identity: &SessionLlmIdentity,
    ) -> Result<(), AgentError> {
        self.inner
            .commit_model_fallback(previous_identity, target_identity)
    }

    fn active_model_fallback_identity(&self) -> Option<SessionLlmIdentity> {
        self.inner.active_model_fallback_identity()
    }

    fn compile_model_fallback_schema(
        &self,
        target_identity: &SessionLlmIdentity,
        output_schema: &OutputSchema,
    ) -> Result<CompiledSchema, AgentError> {
        self.inner
            .compile_model_fallback_schema(target_identity, output_schema)
    }

    fn begin_stream_output_observation(&self) {
        self.inner.begin_stream_output_observation();
    }

    fn stream_output_observed(&self) -> bool {
        self.inner.stream_output_observed()
    }

    fn stream_activity_count(&self) -> Option<u64> {
        self.inner.stream_activity_count()
    }

    fn compile_schema(&self, output_schema: &OutputSchema) -> Result<CompiledSchema, SchemaError> {
        self.inner.compile_schema(output_schema)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::memory::taint::ContentTrustConfig;
    use meerkat_core::types::{
        BlockAssistantMessage, ContentBlock, ServerToolKind, StopReason, ToolProvenance,
        ToolResult, ToolSourceKind, Usage,
    };
    use serde_json::value::RawValue;

    struct ScriptedInner {
        blocks: Vec<AssistantBlock>,
    }

    #[async_trait::async_trait]
    impl AgentLlmClient for ScriptedInner {
        async fn stream_response(
            &self,
            _messages: &[Message],
            _tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&ProviderParamsOverride>,
        ) -> Result<LlmStreamResult, AgentError> {
            Ok(LlmStreamResult::new(
                self.blocks.clone(),
                StopReason::EndTurn,
                Usage::default(),
            ))
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::OpenAI
        }

        fn model(&self) -> &'static str {
            "gpt-5.5"
        }
    }

    fn tool_use(id: &str, name: &str) -> AssistantBlock {
        AssistantBlock::ToolUse {
            id: id.to_string(),
            name: name.to_string(),
            args: RawValue::from_string("{}".to_string()).expect("raw args"),
            meta: None,
        }
    }

    fn assistant(blocks: Vec<AssistantBlock>) -> Message {
        Message::BlockAssistant(BlockAssistantMessage::new(blocks, StopReason::ToolUse))
    }

    fn tool_results(id: &str, text: &str) -> Message {
        Message::tool_results(vec![ToolResult {
            host_metadata: Default::default(),
            settlement_failures: Default::default(),
            tool_use_id: id.to_string(),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            is_error: false,
        }])
    }

    fn mcp_tool(name: &str, server: &str) -> Arc<ToolDef> {
        Arc::new(ToolDef {
            audience: Default::default(),
            name: name.into(),
            description: String::new(),
            input_schema: serde_json::json!({"type": "object"}),
            provenance: Some(ToolProvenance {
                kind: ToolSourceKind::Mcp,
                source_id: server.into(),
                artifact: None,
            }),
        })
    }

    async fn drive(client: &TaintObservingLlmClient, messages: &[Message], tools: &[Arc<ToolDef>]) {
        client
            .stream_response(messages, tools, 128, None, None)
            .await
            .expect("scripted call succeeds");
    }

    // The decorator marks an unqualified MCP tool via typed provenance from
    // the request catalog, synchronously with the call that carries the
    // result - the gate sees taint with no observer in the process at all.
    #[tokio::test]
    async fn marks_unqualified_mcp_tool_via_request_catalog_provenance() {
        let tracker = SessionTaintTracker::new(ContentTrustConfig::default());
        let slot = DispatchTaintSlot::default();
        slot.fill(tracker.clone());
        let client = TaintObservingLlmClient::new(
            Arc::new(ScriptedInner { blocks: vec![] }),
            "identity:a".to_string(),
            slot,
        );

        let messages = vec![
            assistant(vec![tool_use("call-1", "scrape_page")]),
            tool_results("call-1", "attacker text"),
        ];
        let tools = vec![mcp_tool("scrape_page", "scraper")];
        drive(&client, &messages, &tools).await;

        let taint = tracker
            .identity_taint("identity:a")
            .expect("MCP result must mark before the call proceeds");
        assert!(
            taint.source.contains("MCP server 'scraper'"),
            "{}",
            taint.source
        );
    }

    // Absence fallback at the decorator level: a catalog entry without
    // provenance classifies by name - a plain trusted name does not mark, an
    // always-untrusted web name does.
    #[tokio::test]
    async fn falls_back_to_name_classification_without_provenance() {
        let tracker = SessionTaintTracker::new(ContentTrustConfig::default());
        let slot = DispatchTaintSlot::default();
        slot.fill(tracker.clone());
        let client = TaintObservingLlmClient::new(
            Arc::new(ScriptedInner { blocks: vec![] }),
            "identity:a".to_string(),
            slot,
        );

        let plain = Arc::new(ToolDef {
            audience: Default::default(),
            name: "lookup".into(),
            description: String::new(),
            input_schema: serde_json::json!({"type": "object"}),
            provenance: None,
        });
        let messages = vec![
            assistant(vec![tool_use("call-1", "lookup")]),
            tool_results("call-1", "fine"),
        ];
        drive(&client, &messages, std::slice::from_ref(&plain)).await;
        assert!(tracker.identity_taint("identity:a").is_none());

        let messages = vec![
            assistant(vec![tool_use("call-1", "lookup")]),
            tool_results("call-1", "fine"),
            assistant(vec![tool_use("call-2", "web_fetch")]),
            tool_results("call-2", "attacker text"),
        ];
        drive(&client, &messages, std::slice::from_ref(&plain)).await;
        assert!(
            tracker.identity_taint("identity:a").is_some(),
            "web builtins classify untrusted with no catalog entry at all"
        );
    }

    // Server-tool blocks in the RESPONSE mark before the wrapper returns -
    // i.e., before the loop can dispatch any same-response tool call.
    #[tokio::test]
    async fn marks_server_tool_content_from_the_response() {
        let tracker = SessionTaintTracker::new(ContentTrustConfig::default());
        let slot = DispatchTaintSlot::default();
        slot.fill(tracker.clone());
        let client = TaintObservingLlmClient::new(
            Arc::new(ScriptedInner {
                blocks: vec![AssistantBlock::ServerToolContent {
                    id: None,
                    kind: ServerToolKind::WebSearch,
                    content: serde_json::json!({"results": []}),
                    meta: None,
                }],
            }),
            "identity:a".to_string(),
            slot,
        );
        drive(&client, &[], &[]).await;
        let taint = tracker.identity_taint("identity:a").expect("marks");
        assert!(taint.source.contains("web_search"), "{}", taint.source);
    }

    // An unfilled slot is a pure pass-through; a late fill picks up marking
    // without rebuilding the client (bootstrap members build before the
    // memory stack attaches).
    #[tokio::test]
    async fn unfilled_slot_is_inert_and_late_fill_activates() {
        let slot = DispatchTaintSlot::default();
        let client = TaintObservingLlmClient::new(
            Arc::new(ScriptedInner { blocks: vec![] }),
            "identity:a".to_string(),
            slot.clone(),
        );
        let messages = vec![
            assistant(vec![tool_use("call-1", "web_fetch")]),
            tool_results("call-1", "attacker text"),
        ];
        drive(&client, &messages, &[]).await;

        let tracker = SessionTaintTracker::new(ContentTrustConfig::default());
        slot.fill(tracker.clone());
        assert!(tracker.identity_taint("identity:a").is_none());
        // The cursor never advanced while unfilled: the same history is
        // classified on the first tracked call.
        drive(&client, &messages, &[]).await;
        assert!(tracker.identity_taint("identity:a").is_some());
    }

    /// The record of a launch whose mob is `launch` and which composes
    /// `second` beside it, with a mob a member of each created and a mob
    /// no member created.
    fn a_launch_of_two_mobs() -> crate::private_owners::PrivateOwners {
        let owners = crate::private_owners::PrivateOwners::default();
        owners.bind_launch_mob("launch");
        owners.hosted_mobs().name("second");
        owners.hold_the_creation_in_tests("of-launch", Some("launch"));
        owners.hold_the_creation_in_tests("of-second", Some("second"));
        owners.hold_the_creation_in_tests("unowned", None);
        owners
    }

    /// One model call of the member `name` of the mob `mob_id` that
    /// carries a result of an outside tool.
    async fn takes_in(slot: &DispatchTaintSlot, mob_id: &str, name: &str) {
        let client = TaintObservingLlmClient::new(
            Arc::new(ScriptedInner { blocks: vec![] }),
            name.to_string(),
            slot.clone(),
        )
        .of_a_member_of(Some(mob_id.to_string()));
        let messages = vec![
            assistant(vec![tool_use("call-1", "web_fetch")]),
            tool_results("call-1", "attacker text"),
        ];
        drive(&client, &messages, &[]).await;
    }

    fn a_tracker() -> SessionTaintTracker {
        SessionTaintTracker::new(ContentTrustConfig::default())
    }

    // Which memory observes a member is the one its mob's members are
    // members of: a mob with a memory of its own marks its own tracker
    // alone, a mob with none marks nobody, and the launch mob, the mobs
    // its members created and a mob whose memory is not decided mark the
    // launch's, as every member did before a launch hosted a second mob.
    #[tokio::test]
    async fn a_member_is_observed_by_the_memory_its_mobs_members_are_members_of() {
        let slot = DispatchTaintSlot::default();
        let of_the_launch = a_tracker();
        slot.fill(of_the_launch.clone());
        slot.decide_by(a_launch_of_two_mobs());
        slot.make_hosted_mob_trackers_with(Arc::new(a_tracker));
        let of_the_second = slot
            .tracker_of_hosted_mob("second")
            .expect("the second mob's tracker");

        for (mob, name, marks_the_launchs, marks_the_seconds) in [
            ("second", "in-second", false, true),
            ("of-second", "in-of-second", false, false),
            ("launch", "in-launch", true, false),
            ("of-launch", "in-of-launch", true, false),
            ("unowned", "in-unowned", true, false),
            ("never-read", "in-never-read", true, false),
        ] {
            takes_in(&slot, mob, name).await;
            assert_eq!(
                (
                    of_the_launch.identity_taint(name).is_some(),
                    of_the_second.identity_taint(name).is_some()
                ),
                (marks_the_launchs, marks_the_seconds),
                "a member of the mob {mob}: the launch's tracker, the second mob's"
            );
        }
    }

    // A hosted mob's tracker is one value for the life of the process: it
    // is made at the first ask, also when that ask is a member's model
    // call before the mob's memory is composed, and it is the one a later
    // ask answers, so a mark survives the mob's unload and load. Only a
    // forget of the mob (its destroy, a create under its id) starts anew.
    #[tokio::test]
    async fn a_hosted_mobs_tracker_is_kept_until_the_mob_is_forgotten() {
        let slot = DispatchTaintSlot::default();
        slot.fill(a_tracker());
        slot.decide_by(a_launch_of_two_mobs());

        // No memory handed over for hosted mobs yet: nobody observes.
        takes_in(&slot, "second", "early").await;
        assert!(slot.tracker_of_hosted_mob("second").is_none());

        slot.make_hosted_mob_trackers_with(Arc::new(a_tracker));
        takes_in(&slot, "second", "desk").await;
        let composed_later = slot
            .tracker_of_hosted_mob("second")
            .expect("the second mob's tracker");
        assert!(
            composed_later.identity_taint("desk").is_some(),
            "the tracker a later composition is given holds the earlier mark"
        );

        slot.forget_hosted_mob("second");
        let of_the_new_mob = slot
            .tracker_of_hosted_mob("second")
            .expect("the new mob's tracker");
        assert!(
            of_the_new_mob.identity_taint("desk").is_none(),
            "a mob created anew under the id starts unmarked"
        );
    }

    // A slot no record was bound to observes every member build with its
    // one tracker, whatever mob the build names: a composition of one mob.
    #[tokio::test]
    async fn a_slot_with_no_record_observes_every_member_with_its_one_tracker() {
        let slot = DispatchTaintSlot::default();
        let tracker = a_tracker();
        slot.fill(tracker.clone());
        takes_in(&slot, "any-mob", "desk").await;
        assert!(tracker.identity_taint("desk").is_some());
    }

    // The controller surface reaches the wrapped client, and the pinned
    // controller (the client governed controller requests actually run
    // through) is still observed for the member.
    #[tokio::test]
    async fn forwards_the_controller_and_its_pin_stays_observed() {
        use super::controller_test_support::{PinnedInner, authorization, selection};
        let inner = Arc::new(PinnedInner::default());
        let tracker = SessionTaintTracker::new(ContentTrustConfig::default());
        let slot = DispatchTaintSlot::default();
        slot.fill(tracker.clone());
        let wrapped = Arc::new(TaintObservingLlmClient::new(
            inner.clone(),
            "identity:c".to_string(),
            slot,
        ));
        let selection = selection();

        assert_eq!(
            wrapped.controller_model_selection(),
            Some(selection.clone())
        );
        assert_eq!(
            wrapped
                .controller_model_facts()
                .expect("controller facts")
                .selection(),
            &selection
        );
        wrapped
            .prepare_controller_credential()
            .await
            .expect("controller credential maintenance reaches the wrapped client");
        // The same check `Agent::pin_controller_client` applies.
        let pin = Arc::clone(&wrapped)
            .pin_controller()
            .expect("the wrapped client's controller pin");
        assert_eq!(pin.selection(), &selection);
        assert_eq!(
            pin.client().controller_model_selection().as_ref(),
            Some(&selection)
        );
        Arc::clone(&wrapped)
            .prepare_request_attempt_authorized(
                Arc::new(Vec::new()),
                Arc::from(Vec::new()),
                128,
                None,
                None,
                Some(authorization()),
            )
            .expect("an authorized attempt is the wrapped client's decision");
        wrapped
            .fork_noncommitting_live_bridge()
            .expect("the wrapped client's live-bridge fork");

        let messages = vec![
            assistant(vec![tool_use("call-1", "scrape_page")]),
            tool_results("call-1", "attacker text"),
        ];
        let tools = vec![mcp_tool("scrape_page", "scraper")];
        pin.client()
            .stream_response_authorized(&messages, &tools, 128, None, None, Some(authorization()))
            .await
            .expect("authorized controller request");
        assert!(
            tracker.identity_taint("identity:c").is_some(),
            "a request through the pinned controller must still mark the member"
        );
        assert_eq!(
            inner
                .authorized_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            2,
            "both authorized calls reached the wrapped client"
        );
    }

    // The route the wrapped client reports for the provider-neutral
    // `reasoning_effort` option reaches the member's agent. The trait's
    // default is no route, so a decorator that omits the forward compiles
    // and leaves every level set on the member without a route to check it
    // against. The same holds for the wrapped client's default provider tag:
    // the trait's default is no tag, and a level resolved for the member
    // would then replace its client's default fields.
    #[test]
    fn forwards_the_wrapped_clients_reasoning_route() {
        use super::controller_test_support::{PinnedInner, default_provider_tag, reasoning_route};
        let wrapped = TaintObservingLlmClient::new(
            Arc::new(PinnedInner::default()),
            "identity:r".to_string(),
            DispatchTaintSlot::default(),
        );
        assert_eq!(wrapped.reasoning_route(), Some(reasoning_route()));
        assert_eq!(wrapped.default_provider_tag(), Some(default_provider_tag()));
    }

    #[test]
    fn member_identity_resolves_binding_to_the_write_gate_spelling() {
        // Identity-first internal member: the binding carries the ENCODED
        // generated runtime alias; the write gate keys on the durable
        // identity, so the alias must normalize to it.
        let mut req = CreateSessionRequest {
            model: "gpt-5.5".to_string(),
            prompt: meerkat_core::ContentInput::Text("hi".to_string()),
            injected_context: Vec::new(),
            system_prompt: meerkat_core::config::SystemPromptOverride::Inherit,
            max_tokens: None,
            event_tx: None,
            initial_turn: meerkat_core::service::InitialTurnPolicy::Defer,
            deferred_prompt_policy: meerkat_core::service::DeferredPromptPolicy::default(),
            build: Some(SessionBuildOptions {
                mob_member_binding: Some(meerkat_core::MobMemberBinding {
                    mob_id: "mob-1".to_string(),
                    role: "worker".to_string(),
                    member: member_comms_id::mob_member_id_str("rt:review:singleton:0")
                        .into_owned(),
                }),
                ..SessionBuildOptions::default()
            }),
            labels: None,
        };
        assert_eq!(
            member_taint_identity(&req).as_deref(),
            Some("review:singleton"),
            "rt:{{identity}}:{{generation}} normalizes to the durable identity"
        );

        // Classic member: the decoded binding alias IS the recorder's key.
        let binding = req
            .build
            .as_mut()
            .and_then(|build| build.mob_member_binding.as_mut())
            .expect("binding");
        binding.member = "helper".to_string();
        assert_eq!(member_taint_identity(&req).as_deref(), Some("helper"));

        // Identity-first external binding: the roster id encodes the durable
        // identity directly (no rt: shape to strip).
        let binding = req
            .build
            .as_mut()
            .and_then(|build| build.mob_member_binding.as_mut())
            .expect("binding");
        binding.member = member_comms_id::mob_member_id_str("review:singleton").into_owned();
        assert_eq!(
            member_taint_identity(&req).as_deref(),
            Some("review:singleton")
        );

        // A non-numeric trailing segment is not a generation: the alias is
        // some other rt:-prefixed name and stays whole (conservative).
        let binding = req
            .build
            .as_mut()
            .and_then(|build| build.mob_member_binding.as_mut())
            .expect("binding");
        binding.member = member_comms_id::mob_member_id_str("rt:oddly:named").into_owned();
        assert_eq!(
            member_taint_identity(&req).as_deref(),
            Some("rt:oddly:named")
        );

        // No binding: not a member build, no decorator.
        req.build = None;
        assert_eq!(member_taint_identity(&req), None);
        let mut req = req;
        attach_member_taint_decorator(&mut req, &DispatchTaintSlot::default());
        assert!(req.build.is_none(), "non-member requests stay untouched");
    }
}

/// Every production `AgentLlmClient` decorator must report the authority of the
/// client it wraps, not its own.
///
/// `AgentLlmClient::request_attempt_authority` has a DEFAULT returning
/// `LegacySplit`. A decorator that omits it therefore compiles cleanly and
/// silently downgrades everything it wraps - no error, no warning, no test
/// failure. meerkat 0.8.31 rejects `materialize resume` for a client reporting
/// LegacySplit over a Unified adapter, which production measured as 72 identities
/// marked Broken at boot on the candidate against 0 on three 0.8.30 runs.
///
/// This test exists because the compiler cannot express that requirement. It
/// covers the wrappers that exist today; a NEW decorator added later inherits
/// the same default and is not caught here. That gap is structural and tracked
/// separately - do not read this test as protecting future wrappers.
#[cfg(test)]
mod request_attempt_authority_forwarding {
    use super::*;
    use std::sync::Arc;

    /// An inner client reporting the NON-default authority, so a wrapper that
    /// drops the call is distinguishable from one that forwards it. A double
    /// returning `LegacySplit` would make this test pass either way.
    #[derive(Debug)]
    struct UnifiedInner;

    #[async_trait::async_trait]
    impl AgentLlmClient for UnifiedInner {
        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::OpenAI
        }

        fn model(&self) -> &'static str {
            "gpt-5.5"
        }

        async fn stream_response(
            &self,
            _messages: &[Message],
            _tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&ProviderParamsOverride>,
        ) -> Result<meerkat_core::agent::LlmStreamResult, meerkat_core::AgentError> {
            unreachable!("authority forwarding never streams")
        }

        fn request_attempt_authority(&self) -> meerkat_core::RequestAttemptAuthority {
            meerkat_core::RequestAttemptAuthority::Unified
        }
    }

    #[test]
    fn taint_observer_forwards_inner_authority() {
        let inner = Arc::new(UnifiedInner);
        let wrapped = TaintObservingLlmClient::new(
            inner,
            "authority-forwarding".to_string(),
            DispatchTaintSlot::default(),
        );
        assert_eq!(
            wrapped.request_attempt_authority(),
            meerkat_core::RequestAttemptAuthority::Unified,
            "decorator reported its own authority instead of the wrapped client's; \
             the default would have made this LegacySplit and silently downgraded \
             every session it decorates",
        );
    }
}

/// A client with a governed controller, for the decorator forwarding tests:
/// it reports a selection, facts and a pin of itself, accepts authorized
/// requests and records the messages it is sent.
#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
pub(crate) mod controller_test_support {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use meerkat_core::authorization::{
        ModelAuthorizationUse, OperationRefusalKind, OperationRefused,
        PreparedAuthorizationBinding, PreparedOperationAuthorization, WorkAuthorization,
        WorkAuthorizationContext,
    };
    use meerkat_core::types::{Message, StopReason, ToolDef, Usage};
    use meerkat_core::{
        AgentError, AgentLlmClient, AgentLlmRequestAttempt, AssistantMessageId,
        ControllerFactsUnavailable, ControllerModelClient, ControllerModelFacts,
        ControllerModelSelection, LlmRequestAuthorization, LlmStreamResult, ProviderParamsOverride,
        ProviderRequestPressure,
    };

    pub(crate) fn selection() -> ControllerModelSelection {
        let binding = meerkat_core::AuthBindingRef {
            realm: meerkat_core::RealmId::parse("controller-realm").unwrap(),
            binding: meerkat_core::BindingId::parse("controller-binding").unwrap(),
            profile: None,
            origin: meerkat_core::BindingOrigin::Configured,
        };
        ControllerModelSelection::new(
            meerkat_core::SessionLlmIdentity {
                model: "controller-model".to_string(),
                provider: meerkat_core::Provider::OpenAI,
                self_hosted_server_id: None,
                provider_params: None,
                auth_binding: Some(binding.clone()),
            },
            meerkat_core::AuthCredentialIdentity::Binding(binding),
            "controller-profile".to_string(),
            "openai".to_string(),
        )
    }

    /// The route the forwarding probes report for the provider-neutral
    /// `reasoning_effort` option. A decorator that does not forward it
    /// answers the trait's default, no route, and a level set on a member
    /// whose client reports no route cannot be checked against anything.
    pub(crate) fn reasoning_route() -> meerkat_core::ReasoningRouteObservation {
        use meerkat_core::model_profile::capabilities::EffortLevel;
        meerkat_core::ReasoningRouteObservation {
            backend: meerkat_core::ReasoningBackend::SelfHosted(
                meerkat_core::provider_matrix::SelfHostedBackendKind::SelfHosted,
            ),
            api: meerkat_core::ModelApi::Responses,
            account: meerkat_core::AccountLevelStatement::NotApplicable,
            operator_levels: Some(vec![EffortLevel::Low, EffortLevel::High]),
        }
    }

    /// The route the pinned raw client reports for a replay projection: not
    /// the route it reports without one, so a wrapper that answers for a
    /// projection with its client's request-free route is told apart from
    /// one that asks its client about the projection.
    pub(crate) fn reasoning_route_of_a_projection() -> meerkat_core::ReasoningRouteObservation {
        meerkat_core::ReasoningRouteObservation {
            api: meerkat_core::ModelApi::ChatCompletions,
            ..reasoning_route()
        }
    }

    /// The default provider tag the pinned client reports. A decorator that
    /// does not forward it answers the trait's default, no tag, and a level
    /// resolved for the member would replace its client's default fields.
    pub(crate) fn default_provider_tag() -> meerkat_core::lifecycle::run_primitive::ProviderTag {
        use meerkat_core::lifecycle::run_primitive::{OpenAiProviderTag, ProviderTag};
        ProviderTag::OpenAi(OpenAiProviderTag {
            seed: Some(7),
            ..Default::default()
        })
    }

    struct RefusingPolicy;

    impl WorkAuthorization for RefusingPolicy {
        fn prepare(
            &self,
            _: &PreparedAuthorizationBinding,
        ) -> Result<
            Arc<dyn PreparedOperationAuthorization>,
            meerkat_core::OperationAuthorizationError,
        > {
            Err(OperationRefused::new(OperationRefusalKind::Denied).into())
        }
    }

    /// An admitted work authorization for one controller request.
    pub(crate) fn authorization() -> LlmRequestAuthorization {
        LlmRequestAuthorization::new(
            WorkAuthorizationContext::new(
                Arc::new(RefusingPolicy),
                meerkat_core::exact_operation::OperationExecutionScope::Domain,
            ),
            meerkat_core::OperationId::new(),
            ModelAuthorizationUse::ControllerInference,
        )
    }

    #[derive(Default)]
    pub(crate) struct PinnedInner {
        pub(crate) authorized_calls: AtomicUsize,
        pub(crate) seen_messages: Mutex<Vec<Message>>,
    }

    impl PinnedInner {
        fn record(&self, messages: &[Message]) -> LlmStreamResult {
            *self
                .seen_messages
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = messages.to_vec();
            LlmStreamResult::new(Vec::new(), StopReason::EndTurn, Usage::default())
        }
    }

    struct UnstreamedAttempt;

    #[async_trait::async_trait]
    impl AgentLlmRequestAttempt for UnstreamedAttempt {
        fn request_pressure(&self) -> Result<Option<ProviderRequestPressure>, AgentError> {
            Ok(None)
        }

        async fn stream_response(
            &self,
            _assistant_message_id: AssistantMessageId,
        ) -> Result<LlmStreamResult, AgentError> {
            unreachable!("the forwarding tests never stream a prepared attempt")
        }
    }

    #[async_trait::async_trait]
    impl AgentLlmClient for PinnedInner {
        fn controller_model_selection(&self) -> Option<ControllerModelSelection> {
            Some(selection())
        }

        fn reasoning_route(&self) -> Option<meerkat_core::ReasoningRouteObservation> {
            Some(reasoning_route())
        }

        fn default_provider_tag(
            &self,
        ) -> Option<meerkat_core::lifecycle::run_primitive::ProviderTag> {
            Some(default_provider_tag())
        }

        fn controller_model_facts(
            &self,
        ) -> Result<ControllerModelFacts, ControllerFactsUnavailable> {
            Ok(ControllerModelFacts::new(
                selection(),
                Arc::from("https://controller.invalid"),
                Arc::from("controller-wire-model"),
            ))
        }

        async fn prepare_controller_credential(&self) -> Result<(), meerkat_core::auth::AuthError> {
            Ok(())
        }

        fn pin_controller(self: Arc<Self>) -> Option<ControllerModelClient> {
            Some(ControllerModelClient::new(selection(), self))
        }

        fn prepare_request_attempt_authorized(
            self: Arc<Self>,
            _messages: Arc<Vec<Message>>,
            _tools: Arc<[Arc<ToolDef>]>,
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<ProviderParamsOverride>,
            authorization: Option<LlmRequestAuthorization>,
        ) -> Result<Arc<dyn AgentLlmRequestAttempt>, AgentError> {
            if authorization.is_some() {
                self.authorized_calls.fetch_add(1, Ordering::SeqCst);
            }
            Ok(Arc::new(UnstreamedAttempt))
        }

        async fn stream_response(
            &self,
            messages: &[Message],
            _tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&ProviderParamsOverride>,
        ) -> Result<LlmStreamResult, AgentError> {
            Ok(self.record(messages))
        }

        async fn stream_response_authorized(
            &self,
            messages: &[Message],
            _tools: &[Arc<ToolDef>],
            _max_tokens: u32,
            _temperature: Option<f32>,
            _provider_params: Option<&ProviderParamsOverride>,
            authorization: Option<LlmRequestAuthorization>,
        ) -> Result<LlmStreamResult, AgentError> {
            if authorization.is_some() {
                self.authorized_calls.fetch_add(1, Ordering::SeqCst);
            }
            Ok(self.record(messages))
        }

        fn fork_noncommitting_live_bridge(&self) -> Result<Arc<dyn AgentLlmClient>, AgentError> {
            Ok(Arc::new(Self::default()))
        }

        fn provider(&self) -> meerkat_core::Provider {
            meerkat_core::Provider::OpenAI
        }

        fn model(&self) -> &'static str {
            "controller-model"
        }
    }
}
