//! Gateway RPC waits over a scripted ticket bridge (moved from
//! `tests/identity_first_turn_tickets.rs`): these tests build an
//! `rpc::IdentityFirstContext` over a standalone, bridge-controlled
//! `IdentityRuntime` with the live runtime's owner, which uses the
//! pub(crate) view constructors (D67). Assertions are unchanged.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::redundant_clone
)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::identity_first::contracts::{ContinuityStore, LeaseProvider};
use crate::identity_first::core_test_bridge::{bind_live, register_live};
use crate::identity_first::core_test_members::CoreTestMembers;
use crate::identity_first::{
    AgentAddressability, AgentBuildDraft, AgentIdentity, AgentRuntimeId, BridgeAdmissionError,
    BridgeDelivery, BridgeError, BridgeTurnReceipt, DurabilityPolicy, DurableAgentSpec,
    GovernedDelivery, GovernedDeliveryCustody, GovernedInputError, IdentityRuntime,
    IdentityRuntimeConfig, LocalContinuityStore, LocalLeaseProvider, MemberInspection,
    MutableRosterProvider, ResumeRejectionKind, ResumeSessionOutcome, SessionBridge,
    SessionSnapshot, TurnOutcome, TurnOutput, TurnTicket, TurnTracking,
};
use async_trait::async_trait;
use meerkat_client::{LlmError, LlmEvent, LlmRequest};
use meerkat_core::types::{HandlingMode, StopReason};
use serde_json::{Value, json};

fn make_identity(name: &str) -> AgentIdentity {
    AgentIdentity::parse(name).unwrap()
}

/// A turn-driven member: meerkat reports per-turn completion for it.
fn make_spec(name: &str) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: make_identity(name),
        profile: meerkat_mob::ProfileName::from("default"),
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

fn content(text: &str) -> meerkat_core::ContentInput {
    meerkat_core::ContentInput::Text(text.to_string())
}

/// One admission the bridge saw: the interaction id and idempotency key it
/// carried.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Admission {
    interaction_id: Option<String>,
    idempotency_key: Option<String>,
}

type Finisher = tokio::sync::oneshot::Sender<Result<TurnOutput, String>>;

/// Admits deliveries on both lanes and records what each carried. A tracked
/// turn completes only when the test finishes it, by admission order. Like
/// meerkat, a tracked admission of an idempotency key this member already
/// admitted is deduplicated onto the earlier (ended) admission and completes
/// at once without a run result of its own. `inspect_member` reports the
/// latest output any finished turn committed, like the session's
/// `output_preview`.
struct TicketBridge {
    tracks: bool,
    resolution_fails: bool,
    /// When set, every tracked admission is refused before admission, typed,
    /// as meerkat refuses one for a member that actually runs in this mode.
    refuses_tracked_in: std::sync::Mutex<Option<meerkat_mob::MobRuntimeMode>>,
    refused: AtomicUsize,
    /// When set, the next ingress delivery first moves the member onto a
    /// fresh session (a delivery repair or respawn that rotates it).
    rotate_on_next_ingress: std::sync::atomic::AtomicBool,
    session: std::sync::Mutex<meerkat_core::types::SessionId>,
    ingress: std::sync::Mutex<Vec<Admission>>,
    tracked: std::sync::Mutex<Vec<Admission>>,
    finishers: std::sync::Mutex<Vec<Option<Finisher>>>,
    /// Like meerkat's runtime ledger: one per session.
    seen_keys: std::sync::Mutex<HashSet<(AgentRuntimeId, String, String)>>,
    latest_output: std::sync::Mutex<Option<String>>,
    /// Every `inspect_member` call: the member-session read a completion
    /// poll must not make.
    inspections: AtomicUsize,
    /// Every read of the session actor's controller for a governed bind: the
    /// governed path's first bridge call, before its binder binds.
    governed_controller_reads: AtomicUsize,
    /// Every governed delivery through the external door.
    external_governed: AtomicUsize,
    /// Every governed delivery through the trusted host's internal door.
    internal_governed: AtomicUsize,
    /// The members Core holds behind this bridge: the identity runtime reads
    /// each identity's status, runtime id and session from them.
    core: CoreTestMembers,
    /// When set, Core refuses every resume for good (the session is
    /// archived, not revivable): a wake ends as a final refusal.
    refuses_resume: std::sync::atomic::AtomicBool,
}

impl TicketBridge {
    fn new(tracks: bool, session: meerkat_core::types::SessionId) -> Arc<Self> {
        Self::build(tracks, false, session)
    }

    fn build(
        tracks: bool,
        resolution_fails: bool,
        session: meerkat_core::types::SessionId,
    ) -> Arc<Self> {
        Arc::new(Self {
            tracks,
            resolution_fails,
            refuses_tracked_in: std::sync::Mutex::new(None),
            refused: AtomicUsize::new(0),
            rotate_on_next_ingress: std::sync::atomic::AtomicBool::new(false),
            session: std::sync::Mutex::new(session),
            ingress: std::sync::Mutex::new(Vec::new()),
            tracked: std::sync::Mutex::new(Vec::new()),
            finishers: std::sync::Mutex::new(Vec::new()),
            seen_keys: std::sync::Mutex::new(HashSet::new()),
            latest_output: std::sync::Mutex::new(None),
            inspections: AtomicUsize::new(0),
            governed_controller_reads: AtomicUsize::new(0),
            external_governed: AtomicUsize::new(0),
            internal_governed: AtomicUsize::new(0),
            core: CoreTestMembers::default(),
            refuses_resume: std::sync::atomic::AtomicBool::new(false),
        })
    }

    fn current_session(&self) -> meerkat_core::types::SessionId {
        self.session.lock().unwrap().clone()
    }

    /// Rotate the member onto a fresh session (a repair's fresh spawn, a
    /// legacy respawn): later deliveries resolve onto it, and its runtime
    /// ledger has seen no idempotency key.
    fn rotate_session(&self) -> meerkat_core::types::SessionId {
        let rotated = meerkat_core::types::SessionId::new();
        *self.session.lock().unwrap() = rotated.clone();
        rotated
    }

    fn record(
        &self,
        runtime_id: &AgentRuntimeId,
        delivery: &BridgeDelivery,
        lane: &std::sync::Mutex<Vec<Admission>>,
    ) -> bool {
        let key = delivery
            .delivery_identity
            .as_ref()
            .map(|carrier| carrier.idempotency_key.clone());
        lane.lock().unwrap().push(Admission {
            interaction_id: delivery.interaction_id.clone(),
            idempotency_key: key.clone(),
        });
        let session = self.current_session().to_string();
        key.is_some_and(|key| {
            !self
                .seen_keys
                .lock()
                .unwrap()
                .insert((runtime_id.clone(), session, key))
        })
    }

    /// Finish the `index`th tracked admission.
    fn finish(&self, index: usize, outcome: Result<&str, &str>) {
        let finisher = self
            .finishers
            .lock()
            .unwrap()
            .get_mut(index)
            .and_then(Option::take)
            .expect("no pending tracked turn at this admission index");
        if let Ok(text) = outcome {
            *self.latest_output.lock().unwrap() = Some(text.to_string());
        }
        let _ = finisher.send(
            outcome
                .map(|text| TurnOutput::Text {
                    text: text.to_string(),
                    truncated: false,
                })
                .map_err(ToString::to_string),
        );
    }
}

#[async_trait]
impl SessionBridge for TicketBridge {
    /// Core's successor transition (reset, `delete_identity`): the live
    /// member is replaced by its successor on a fresh session, one
    /// generation up.
    async fn reset_member_to_successor(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        _draft: &AgentBuildDraft,
    ) -> Result<crate::identity_first::MemberRuntimeBinding, BridgeError> {
        let generation = self
            .core
            .last_member_binding(identity)?
            .map_or(0, |binding| binding.generation().get() + 1);
        let binding = crate::identity_first::MemberRuntimeBinding {
            agent_runtime_id: AgentRuntimeId::parse(&format!("rt:{identity}:{generation}"))
                .map_err(|error| BridgeError::Mob(error.to_string()))?,
            session_id: Some(meerkat_core::types::SessionId::new()),
        };
        self.core.bind(identity, &binding, &spec.profile);
        Ok(binding)
    }
    async fn create_session(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        _draft: &AgentBuildDraft,
    ) -> Result<crate::identity_first::MemberRuntimeBinding, BridgeError> {
        let agent_runtime_id = AgentRuntimeId::parse(&format!("rt:{identity}:0"))
            .map_err(|error| BridgeError::Mob(error.to_string()))?;
        let session_id = &meerkat_core::types::SessionId::new();
        let binding = crate::identity_first::MemberRuntimeBinding {
            agent_runtime_id,
            session_id: Some(session_id.clone()),
        };
        self.core.bind(identity, &binding, &spec.profile);
        Ok(binding)
    }

    async fn resume_session(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        _draft: &AgentBuildDraft,
        session_id: &meerkat_core::types::SessionId,
        _resume_from_role: Option<&meerkat_mob::ProfileName>,
    ) -> Result<ResumeSessionOutcome, BridgeError> {
        if self.refuses_resume.load(Ordering::SeqCst) {
            return Err(BridgeError::ResumeRejected {
                kind: ResumeRejectionKind::Unavailable {
                    reason:
                        meerkat_mob::error::SessionResumeUnavailableReason::ArchivedNotRevivable,
                },
                session_id: session_id.clone(),
                detail: "the session is archived and cannot be revived".to_string(),
            });
        }
        let agent_runtime_id = AgentRuntimeId::parse(&format!("rt:{identity}:0"))
            .map_err(|error| BridgeError::Mob(error.to_string()))?;
        self.core.bind(
            identity,
            &crate::identity_first::MemberRuntimeBinding {
                agent_runtime_id: agent_runtime_id.clone(),
                session_id: Some(session_id.clone()),
            },
            &spec.profile,
        );
        Ok(ResumeSessionOutcome::Resumed {
            session_id: session_id.clone(),
            agent_runtime_id,
        })
    }

    async fn live_member(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<crate::identity_first::MemberRuntimeBinding>, BridgeError> {
        self.core.live_member(identity)
    }

    async fn live_member_scope(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberDeliveryScope>, BridgeError> {
        self.core.live_member_scope(identity)
    }

    async fn ensure_member_live(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<crate::identity_first::MemberRuntimeBinding>, BridgeError> {
        self.core.live_member(identity)
    }

    async fn member_binding_history(
        &self,
    ) -> Result<meerkat_mob::MemberBindingHistory, BridgeError> {
        self.core.history()
    }

    async fn last_member_binding(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<meerkat_mob::MemberBinding>, BridgeError> {
        self.core.last_member_binding(identity)
    }

    async fn identity_management(
        &self,
        _identity: &AgentIdentity,
    ) -> Result<meerkat_mob::MemberIdentityManagement, BridgeError> {
        Ok(meerkat_mob::MemberIdentityManagement::CommandDriven)
    }

    async fn deliver_admitted(
        &self,
        runtime_id: &AgentRuntimeId,
        delivery: BridgeDelivery,
    ) -> Result<meerkat_core::types::SessionId, BridgeError> {
        if self.rotate_on_next_ingress.swap(false, Ordering::SeqCst) {
            self.rotate_session();
        }
        self.record(runtime_id, &delivery, &self.ingress);
        Ok(self.current_session())
    }

    async fn begin_delivery_with_output(
        &self,
        runtime_id: &AgentRuntimeId,
        delivery: BridgeDelivery,
    ) -> Result<BridgeTurnReceipt, BridgeAdmissionError> {
        if let Some(mode) = *self.refuses_tracked_in.lock().unwrap() {
            self.refused.fetch_add(1, Ordering::SeqCst);
            return Err(BridgeAdmissionError::UnsupportedForMode {
                identity: meerkat_mob::ids::AgentIdentity::from(runtime_id.as_str()),
                mode,
                detail: format!(
                    "unsupported for runtime mode {mode}: tracked turn completion is not \
                     supported by autonomous inbox delivery"
                ),
            });
        }
        let deduplicated = self.record(runtime_id, &delivery, &self.tracked);
        let (finish, finished) = tokio::sync::oneshot::channel();
        if deduplicated {
            // meerkat's KeyOnly dedup onto an ended admission: a successful
            // terminal with no run result of this admission's own.
            let _ = finish.send(Ok(TurnOutput::NoOwnResult));
            self.finishers.lock().unwrap().push(None);
        } else {
            self.finishers.lock().unwrap().push(Some(finish));
        }
        let session_result = if self.resolution_fails {
            Err("the machine-state projection has no session for this member".to_string())
        } else {
            Ok(self.current_session())
        };
        Ok(BridgeTurnReceipt::with_output(session_result, async move {
            finished
                .await
                .unwrap_or_else(|_| Err("the turn was dropped".to_string()))
        }))
    }

    async fn governed_controller(
        &self,
        _runtime_id: &AgentRuntimeId,
        _session_id: &meerkat_core::types::SessionId,
    ) -> Result<meerkat_core::ControllerModelClient, BridgeAdmissionError> {
        self.governed_controller_reads
            .fetch_add(1, Ordering::SeqCst);
        Err(BridgeAdmissionError::GovernedInput(
            GovernedInputError::Unsupported("counted, no controller".to_string()),
        ))
    }

    async fn begin_governed_delivery(
        &self,
        _runtime_id: &AgentRuntimeId,
        _session_id: &meerkat_core::types::SessionId,
        _input: meerkat_runtime::Input,
        custody: GovernedDeliveryCustody,
    ) -> Result<GovernedDelivery, BridgeAdmissionError> {
        self.external_governed.fetch_add(1, Ordering::SeqCst);
        drop(custody);
        Err(BridgeAdmissionError::GovernedInput(
            GovernedInputError::Unsupported("counted, not delivered".to_string()),
        ))
    }

    async fn begin_internal_governed_delivery(
        &self,
        _runtime_id: &AgentRuntimeId,
        _session_id: &meerkat_core::types::SessionId,
        _input: meerkat_runtime::Input,
        custody: GovernedDeliveryCustody,
    ) -> Result<GovernedDelivery, BridgeAdmissionError> {
        self.internal_governed.fetch_add(1, Ordering::SeqCst);
        drop(custody);
        Err(BridgeAdmissionError::GovernedInput(
            GovernedInputError::Unsupported("counted, not delivered".to_string()),
        ))
    }

    fn tracks_turn_output(&self) -> bool {
        self.tracks
    }

    async fn checkpoint_session(
        &self,
        _runtime_id: &AgentRuntimeId,
        _session_id: &meerkat_core::types::SessionId,
    ) -> Result<SessionSnapshot, BridgeError> {
        Ok(SessionSnapshot { data: Vec::new() })
    }

    async fn retire_member(&self, runtime_id: &AgentRuntimeId) -> Result<(), BridgeError> {
        // Core retires the member; its history keeps the binding retired.
        self.core.retire(runtime_id);
        Ok(())
    }

    async fn inspect_member(
        &self,
        _runtime_id: &AgentRuntimeId,
    ) -> Result<MemberInspection, BridgeError> {
        self.inspections.fetch_add(1, Ordering::SeqCst);
        Ok(MemberInspection {
            output_preview: self.latest_output.lock().unwrap().clone(),
            preview_unavailable: None,
            is_final: false,
            peer_reachable_count: 0,
        })
    }
}

fn make_runtime(bridge: Option<Arc<dyn SessionBridge>>) -> Arc<IdentityRuntime> {
    let store: Arc<dyn ContinuityStore> = Arc::new(LocalContinuityStore::in_memory().unwrap());
    let lease: Arc<dyn LeaseProvider> = Arc::new(LocalLeaseProvider::new());
    Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: store,
        lease_provider: lease,
        runtime_instance_id: "turn-ticket-test".to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge,
        default_timeout: None,
    }))
}

/// Register an identity Core holds live at generation 0, bound to
/// `session`: the session the bridge resolves every turn onto (so no
/// delivery looks like a rotation). Core's fence of that member is 1.
async fn register_spec(
    runtime: &IdentityRuntime,
    bridge: &TicketBridge,
    spec: DurableAgentSpec,
    session: &meerkat_core::types::SessionId,
) -> AgentIdentity {
    let identity = spec.identity.clone();
    register_live(runtime, &bridge.core, spec, 0, session.clone())
        .await
        .unwrap();
    identity
}

async fn register_bound(
    runtime: &IdentityRuntime,
    bridge: &TicketBridge,
    name: &str,
    session: &meerkat_core::types::SessionId,
) -> AgentIdentity {
    register_spec(runtime, bridge, make_spec(name), session).await
}

fn tracked(turn: &TurnTracking) -> TurnTicket {
    match turn {
        TurnTracking::Tracked(ticket) => *ticket,
        TurnTracking::Unavailable(reason) => panic!("the turn was not tracked: {reason}"),
    }
}

fn completed(text: &str) -> TurnOutcome {
    TurnOutcome::Completed {
        output: TurnOutput::Text {
            text: text.to_string(),
            truncated: false,
        },
    }
}

/// Replies to each message by the name it carries ("reply to: alpha"), and
/// holds a turn whose name is held until the test releases it. The names are
/// read off every message after the last assistant message (the run's new
/// input), so a turn-driven member's user message, an autonomous member's
/// inbox notice, and several inputs an autonomous member batched into one run
/// all count, each once per model call.
#[derive(Clone, Default)]
struct NamedReplyClient {
    holds: Arc<std::sync::Mutex<HashMap<&'static str, tokio::sync::oneshot::Receiver<()>>>>,
    calls: Arc<AtomicUsize>,
    calls_by_name: Arc<std::sync::Mutex<HashMap<&'static str, usize>>>,
}

const NAMES: [&str; 3] = ["foreign", "alpha", "beta"];

impl NamedReplyClient {
    fn hold(&self, name: &'static str) -> tokio::sync::oneshot::Sender<()> {
        let (release, held) = tokio::sync::oneshot::channel();
        self.holds.lock().unwrap().insert(name, held);
        release
    }
}

impl meerkat_client::LlmClient for NamedReplyClient {
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
        self.calls.fetch_add(1, Ordering::SeqCst);
        let new_input_from = request
            .messages
            .iter()
            .rposition(|message| matches!(message, meerkat_core::Message::BlockAssistant(_)))
            .map_or(0, |last_reply| last_reply + 1);
        let new_input = request.messages[new_input_from..]
            .iter()
            .filter_map(|message| serde_json::to_string(message).ok())
            .collect::<String>();
        let names = NAMES
            .into_iter()
            .filter(|name| new_input.contains(name))
            .collect::<Vec<_>>();
        {
            let mut calls = self.calls_by_name.lock().unwrap();
            for name in &names {
                *calls.entry(name).or_default() += 1;
            }
        }
        let name = names.first().copied().unwrap_or("other");
        let held = self.holds.lock().unwrap().remove(name);
        let [usage, done] = crate::mob_handle_runtime::test_llm_usage::usage_then_done(
            request,
            meerkat::Provider::OpenAI,
            StopReason::EndTurn,
        );
        Box::pin(async_stream::stream! {
            if let Some(held) = held {
                let _ = held.await;
            }
            yield Ok(LlmEvent::TextDelta { delta: format!("reply to: {name}"), meta: None });
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

async fn rpc(
    runtime: &crate::UnifiedRuntime,
    ctx: &crate::rpc::IdentityFirstContext,
    method: &str,
    params: Value,
) -> Value {
    let request = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    let reply = crate::rpc::handle_unified_rpc_json(
        runtime,
        &request.to_string(),
        Duration::from_secs(30),
        None,
        Some(ctx),
    )
    .await;
    let reply: Value = serde_json::from_str(&reply).expect("json-rpc reply");
    assert!(reply.get("error").is_none(), "{method} failed: {reply}");
    reply["result"].clone()
}

async fn turn_result(
    runtime: &crate::UnifiedRuntime,
    ctx: &crate::rpc::IdentityFirstContext,
    ticket: &str,
) -> Value {
    rpc(
        runtime,
        ctx,
        "meta/turn_result",
        json!({"identity": "keeper", "ticket": ticket}),
    )
    .await
}

/// The live test mob: one `worker` profile declaring `runtime_mode`.
fn worker_definition(
    mob_id: &str,
    runtime_mode: meerkat_mob::MobRuntimeMode,
) -> meerkat_mob::MobDefinition {
    let mode = match runtime_mode {
        meerkat_mob::MobRuntimeMode::TurnDriven => "turn_driven",
        meerkat_mob::MobRuntimeMode::AutonomousHost => "autonomous_host",
    };
    meerkat_mob::MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "{mob_id}"
[profiles.worker]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "{mode}"
[profiles.worker.tools]
comms = true
"#
    ))
    .expect("mob definition")
}

/// Point Meta's profile table (the reset-roster context, which is where it
/// resolves a member's runtime mode) at `definition`, WITHOUT respawning
/// anything: the member keeps running in the mode it was spawned with.
fn flip_profile_table(
    ctx: &crate::rpc::IdentityFirstContext,
    definition: meerkat_mob::MobDefinition,
) {
    ctx.runtime
        .set_reset_roster_provider_context(Some(ctx.roster_provider.clone()), Some(definition));
}

/// A live runtime whose one identity, `keeper`, runs in `runtime_mode` (the
/// profile's declared mode; `turn_driven` is also pinned as the spec
/// override so Meta and meerkat resolve it alike).
async fn live_runtime(
    runtime_mode: meerkat_mob::MobRuntimeMode,
    client: &NamedReplyClient,
) -> (
    crate::UnifiedRuntime,
    crate::rpc::IdentityFirstContext,
    tempfile::TempDir,
) {
    let live = live_runtime_with(
        runtime_mode,
        (runtime_mode == meerkat_mob::MobRuntimeMode::TurnDriven).then_some(runtime_mode),
        client,
    )
    .await;
    (live.runtime, live.ctx, live.scratch)
}

/// One live test mob.
struct LiveMob {
    runtime: crate::UnifiedRuntime,
    ctx: crate::rpc::IdentityFirstContext,
    scratch: tempfile::TempDir,
    roster: Arc<MutableRosterProvider>,
}

/// [`live_runtime`] with the profile declaring `runtime_mode` and the spec
/// pinning `override_mode` (or nothing), keeping the mob id and the mutable
/// roster so a test can change the profile table or the spec.
async fn live_runtime_with(
    runtime_mode: meerkat_mob::MobRuntimeMode,
    override_mode: Option<meerkat_mob::MobRuntimeMode>,
    client: &NamedReplyClient,
) -> LiveMob {
    let scratch = tempfile::tempdir().expect("scratch dir");
    let mob_id = format!("turn-tickets-{}", uuid::Uuid::new_v4());
    let definition = worker_definition(&mob_id, runtime_mode);
    let mut spec = make_spec("keeper");
    spec.profile = "worker".into();
    spec.runtime_mode_override = override_mode;
    let roster = Arc::new(MutableRosterProvider::new(vec![spec]));
    let runtime = crate::UnifiedRuntime::builder()
        .definition(definition)
        .continuity_store(Arc::new(LocalContinuityStore::in_memory().unwrap()))
        .lease_provider(Arc::new(LocalLeaseProvider::new()))
        .roster_provider(roster.clone())
        .scratch_dir(scratch.path())
        .ephemeral_runtime_store(true)
        .identity_runtime_instance_id("turn-tickets-live")
        .default_llm_client(Arc::new(client.clone()))
        .build()
        .await
        .expect("build runtime");
    let identity_runtime = runtime.identity_runtime().expect("identity runtime");
    let ctx = crate::rpc::IdentityFirstContext {
        runtime: identity_runtime,
        roster_provider: roster.clone(),
        topology_provider: None,
        customizer: None,
        agent_memory_provider: None,
        mob_definition: Some(runtime.mob_handle().definition().clone()),
        transcript_edit_service: None,
        compaction_floors: None,
    };
    // Meta resolves an unpinned member's mode from this profile table, as
    // the gateway installs it at boot.
    flip_profile_table(&ctx, worker_definition(&mob_id, runtime_mode));
    LiveMob {
        runtime,
        ctx,
        scratch,
        roster,
    }
}

/// #468: `meta/completion_cursor` reads the identity's completion cursor
/// without touching the member session. `inspect_identity` reads the member
/// (an execution snapshot on its session task, which on meerkat 0.8.45+ can
/// hold a staged run); a completion poll on the cursor RPC never does, and
/// it reports the same cursor.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn completion_cursor_rpc_never_inspects_the_member() {
    let client = NamedReplyClient::default();
    let live = live_runtime_with(
        meerkat_mob::MobRuntimeMode::TurnDriven,
        Some(meerkat_mob::MobRuntimeMode::TurnDriven),
        &client,
    )
    .await;
    let session = meerkat_core::types::SessionId::new();
    let bridge = TicketBridge::new(true, session.clone());
    let identity_runtime = make_runtime(Some(bridge.clone()));
    let watcher = register_bound(&identity_runtime, &bridge, "watcher", &session).await;
    let ctx = crate::rpc::IdentityFirstContext {
        runtime: crate::identity_first::PublicIdentityRuntime::new(
            identity_runtime.clone(),
            live.runtime.mob_runtime().clone(),
        ),
        roster_provider: live.roster.clone(),
        topology_provider: None,
        customizer: None,
        agent_memory_provider: None,
        mob_definition: None,
        transcript_edit_service: None,
        compaction_floors: None,
    };
    let cursor_of = |value: &Value| -> crate::identity_first::CompletionCursor {
        serde_json::from_value(value["completion_cursor"].clone()).expect("completion cursor")
    };

    let baseline = identity_runtime.completion_cursor(&watcher).await;
    for _ in 0..20 {
        let read = rpc(
            &live.runtime,
            &ctx,
            "meta/completion_cursor",
            json!({"identity": "watcher"}),
        )
        .await;
        assert_eq!(read["identity"], "watcher");
        assert_eq!(read["state"], "active");
        assert_eq!(cursor_of(&read), baseline);
    }
    identity_runtime.record_turn_completed(&watcher).await;
    let read = rpc(
        &live.runtime,
        &ctx,
        "meta/completion_cursor",
        json!({"identity": "watcher"}),
    )
    .await;
    assert_eq!(cursor_of(&read), baseline.advanced());
    assert_eq!(
        bridge.inspections.load(Ordering::SeqCst),
        0,
        "a cursor read must never inspect the member session"
    );

    // Control: inspect_identity reports the same cursor, by reading the member.
    let inspected = rpc(
        &live.runtime,
        &ctx,
        "meta/inspect_identity",
        json!({"identity": "watcher"}),
    )
    .await;
    assert_eq!(cursor_of(&inspected), baseline.advanced());
    assert_eq!(bridge.inspections.load(Ordering::SeqCst), 1);

    // An identity nobody registered is a typed unknown identity, not a zero
    // cursor.
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "meta/completion_cursor",
        "params": {"identity": "nobody"},
    });
    let reply = crate::rpc::handle_unified_rpc_json(
        &live.runtime,
        &request.to_string(),
        Duration::from_secs(30),
        None,
        Some(&ctx),
    )
    .await;
    let reply: Value = serde_json::from_str(&reply).expect("json-rpc reply");
    assert_eq!(reply["error"]["code"], json!(-32001), "{reply}");
    live.runtime.shutdown().await;
}

/// A live test mob plus a separate identity runtime on a [`TicketBridge`],
/// wired into an RPC context: the bridge stands in for the member session, so
/// a test sees every member read.
async fn ticket_bridge_rpc() -> (
    LiveMob,
    Arc<TicketBridge>,
    Arc<IdentityRuntime>,
    crate::rpc::IdentityFirstContext,
    meerkat_core::types::SessionId,
) {
    let client = NamedReplyClient::default();
    let live = live_runtime_with(
        meerkat_mob::MobRuntimeMode::TurnDriven,
        Some(meerkat_mob::MobRuntimeMode::TurnDriven),
        &client,
    )
    .await;
    let session = meerkat_core::types::SessionId::new();
    let bridge = TicketBridge::new(true, session.clone());
    let identity_runtime = make_runtime(Some(bridge.clone()));
    let ctx = crate::rpc::IdentityFirstContext {
        runtime: crate::identity_first::PublicIdentityRuntime::new(
            identity_runtime.clone(),
            live.runtime.mob_runtime().clone(),
        ),
        roster_provider: live.roster.clone(),
        topology_provider: None,
        customizer: None,
        agent_memory_provider: None,
        mob_definition: None,
        transcript_edit_service: None,
        compaction_floors: None,
    };
    (live, bridge, identity_runtime, ctx, session)
}

/// One raw JSON-RPC exchange (errors included).
async fn rpc_reply(
    runtime: &crate::UnifiedRuntime,
    ctx: &crate::rpc::IdentityFirstContext,
    method: &str,
    params: Value,
) -> Value {
    let request = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    let reply = crate::rpc::handle_unified_rpc_json(
        runtime,
        &request.to_string(),
        Duration::from_secs(30),
        None,
        Some(ctx),
    )
    .await;
    serde_json::from_str(&reply).expect("json-rpc reply")
}

/// #468: `meta/wait_for_completion` is a server-side wait that resolves on
/// the completion itself (the runtime's typed completion signal), with no
/// client or server poll loop, and never reads the member session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_for_completion_rpc_resolves_on_the_completion_signal() {
    let (live, bridge, identity_runtime, ctx, session) = ticket_bridge_rpc().await;
    let watcher = register_bound(&identity_runtime, &bridge, "watcher", &session).await;
    let baseline = identity_runtime.completion_cursor(&watcher).await;

    let wait = rpc(
        &live.runtime,
        &ctx,
        "meta/wait_for_completion",
        json!({
            "identity": "watcher",
            "after": {"epoch": baseline.epoch.get(), "turns": baseline.turns},
            "timeout_ms": 600_000,
        }),
    );
    let complete = async {
        // Let the wait park on the signal first.
        tokio::task::yield_now().await;
        identity_runtime.record_turn_completed(&watcher).await;
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(wait, complete)
    })
    .await
    .expect("the wait resolves on the completion, far inside its 600 s deadline");
    assert_eq!(result["outcome"], "completed", "{result}");
    assert_eq!(result["identity"], "watcher");
    let cursor: crate::identity_first::CompletionCursor =
        serde_json::from_value(result["completion_cursor"].clone()).expect("cursor");
    assert_eq!(cursor, baseline.advanced());
    assert_eq!(bridge.inspections.load(Ordering::SeqCst), 0);

    // Already past the baseline: resolves at once.
    let result = rpc(
        &live.runtime,
        &ctx,
        "meta/wait_for_completion",
        json!({
            "identity": "watcher",
            "after": {"epoch": baseline.epoch.get(), "turns": baseline.turns},
        }),
    )
    .await;
    assert_eq!(result["outcome"], "completed", "{result}");
    live.runtime.shutdown().await;
}

/// The caller's deadline is only a deadline: reaching it is the typed
/// `timed_out` outcome carrying the cursor, and an incarnation change wakes
/// the wait as `incarnation_changed`, never as progress.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_for_completion_rpc_reports_deadline_and_incarnation_change() {
    let (live, bridge, identity_runtime, ctx, session) = ticket_bridge_rpc().await;
    let watcher = register_bound(&identity_runtime, &bridge, "watcher", &session).await;
    let baseline = identity_runtime.completion_cursor(&watcher).await;
    let after = json!({"epoch": baseline.epoch.get(), "turns": baseline.turns});

    let result = rpc(
        &live.runtime,
        &ctx,
        "meta/wait_for_completion",
        json!({"identity": "watcher", "after": after, "timeout_ms": 50}),
    )
    .await;
    assert_eq!(result["outcome"], "timed_out", "{result}");

    let wait = rpc(
        &live.runtime,
        &ctx,
        "meta/wait_for_completion",
        json!({"identity": "watcher", "after": after, "timeout_ms": 600_000}),
    );
    let rotate = async {
        tokio::task::yield_now().await;
        // A new runtime incarnation: Core respawned the member at
        // generation 1 (fence 2). The re-registration wakes the wait.
        bind_live(
            &bridge.core,
            &watcher,
            &make_spec("watcher").profile,
            1,
            Some(session.clone()),
        )
        .unwrap();
        identity_runtime.register(make_spec("watcher")).await;
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(wait, rotate)
    })
    .await
    .expect("the incarnation change wakes the wait");
    assert_eq!(result["outcome"], "incarnation_changed", "{result}");
    assert_eq!(result["completion_cursor"]["epoch"], 2);

    // A baseline names an incarnation of a registered identity.
    let reply = rpc_reply(
        &live.runtime,
        &ctx,
        "meta/wait_for_completion",
        json!({"identity": "nobody", "after": after, "timeout_ms": 50}),
    )
    .await;
    assert_eq!(reply["error"]["code"], json!(-32001), "{reply}");
    live.runtime.shutdown().await;
}

/// A readiness wait (no `after`) may start before its identity is
/// registered: it resolves on the identity's first recorded completion.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_for_completion_rpc_readiness_waits_for_the_first_completion() {
    let (live, bridge, identity_runtime, ctx, session) = ticket_bridge_rpc().await;
    let wait = rpc(
        &live.runtime,
        &ctx,
        "meta/wait_for_completion",
        json!({"identity": "latecomer", "timeout_ms": 600_000}),
    );
    let materialize = async {
        tokio::task::yield_now().await;
        let latecomer = register_bound(&identity_runtime, &bridge, "latecomer", &session).await;
        tokio::task::yield_now().await;
        identity_runtime.record_turn_completed(&latecomer).await;
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(wait, materialize)
    })
    .await
    .expect("readiness resolves on the first completion");
    assert_eq!(result["outcome"], "completed", "{result}");
    assert_eq!(result["completion_cursor"]["turns"], 1);
    live.runtime.shutdown().await;
}

/// `meta/wait_for_turn` answers like `meta/turn_result`, once the ticket
/// settles: the server waits on the ticket registry's change signal, so the
/// recommended barrier (`track_turn` plus the ticket) involves no polling.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_for_turn_rpc_resolves_when_the_ticket_settles() {
    let (live, bridge, identity_runtime, ctx, session) = ticket_bridge_rpc().await;
    let keeper = register_bound(&identity_runtime, &bridge, "keeper", &session).await;
    let sent = identity_runtime
        .send_with_turn_ticket(&keeper, None, &content("alpha"), HandlingMode::Queue, None)
        .await
        .expect("send");
    let ticket = tracked(&sent.turn).to_string();

    let pending = rpc(
        &live.runtime,
        &ctx,
        "meta/wait_for_turn",
        json!({"identity": "keeper", "ticket": ticket, "timeout_ms": 50}),
    )
    .await;
    assert_eq!(
        pending["state"], "pending",
        "the deadline returns it pending"
    );
    assert_eq!(pending["wait"], "timed_out", "{pending}");

    let wait = rpc(
        &live.runtime,
        &ctx,
        "meta/wait_for_turn",
        json!({"identity": "keeper", "ticket": ticket, "timeout_ms": 600_000}),
    );
    let finish = async {
        tokio::task::yield_now().await;
        bridge.finish(0, Ok("alpha reply"));
    };
    let (result, ()) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(wait, finish)
    })
    .await
    .expect("the settlement wakes the wait");
    assert_eq!(result["state"], "completed", "{result}");
    assert_eq!(result["output"], "alpha reply", "{result}");
    assert_eq!(result["ticket"], json!(ticket));
    assert_eq!(result["wait"], "settled", "{result}");

    let unknown = rpc(
        &live.runtime,
        &ctx,
        "meta/wait_for_turn",
        json!({
            "identity": "keeper",
            "ticket": uuid::Uuid::new_v4().to_string(),
            "timeout_ms": 600_000,
        }),
    )
    .await;
    assert_eq!(
        unknown["state"], "unknown",
        "an unknown ticket answers at once"
    );
    live.runtime.shutdown().await;
}

/// Race one `meta/wait_for_completion` against `event`, returning the
/// wait's result; the long deadline proves the event, not the clock, ended it.
async fn wait_racing<F: std::future::Future<Output = ()>>(
    live: &LiveMob,
    ctx: &crate::rpc::IdentityFirstContext,
    params: Value,
    event: F,
) -> Value {
    let wait = rpc(&live.runtime, ctx, "meta/wait_for_completion", params);
    let event = async {
        tokio::task::yield_now().await;
        event.await;
    };
    let (result, ()) =
        tokio::time::timeout(Duration::from_secs(10), async { tokio::join!(wait, event) })
            .await
            .expect("the event ends the wait, far inside its deadline");
    result
}

/// #468 review: a completion wait never runs to its deadline on an identity
/// whose run failed, that went Broken, retired, or was deleted. Each ends the
/// wait with its typed outcome.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_for_completion_ends_typed_on_failure_broken_retire_and_delete() {
    let (live, bridge, identity_runtime, ctx, session) = ticket_bridge_rpc().await;
    let params = |name: &str, baseline: crate::identity_first::CompletionCursor| {
        json!({
            "identity": name,
            "after": {"epoch": baseline.epoch.get(), "turns": baseline.turns},
            "timeout_ms": 600_000,
        })
    };

    // A failed (or cancelled) run.
    let failing = register_bound(&identity_runtime, &bridge, "failing", &session).await;
    let baseline = identity_runtime.completion_cursor(&failing).await;
    let result = wait_racing(&live, &ctx, params("failing", baseline), async {
        identity_runtime.record_turn_failed(&failing).await;
    })
    .await;
    assert_eq!(result["outcome"], "run_failed", "{result}");

    // Broken: Core let the member go and refuses its resume for good, so
    // the wake that follows ends as a final refusal.
    let breaking = register_bound(&identity_runtime, &bridge, "breaking", &session).await;
    let baseline = identity_runtime.completion_cursor(&breaking).await;
    let result = wait_racing(&live, &ctx, params("breaking", baseline), async {
        bridge.refuses_resume.store(true, Ordering::SeqCst);
        bridge
            .core
            .retire(&AgentRuntimeId::parse(&format!("rt:{breaking}:0")).unwrap());
        let refused = identity_runtime.materialize(&breaking).await;
        assert!(refused.is_err(), "the wake must be refused");
    })
    .await;
    assert_eq!(result["outcome"], "broken", "{result}");
    bridge.refuses_resume.store(false, Ordering::SeqCst);

    // Retired.
    let retiree = register_bound(&identity_runtime, &bridge, "retiree", &session).await;
    let baseline = identity_runtime.completion_cursor(&retiree).await;
    let result = wait_racing(&live, &ctx, params("retiree", baseline), async {
        identity_runtime.retire(&retiree).await.expect("retire");
    })
    .await;
    assert_eq!(result["outcome"], "retiring", "{result}");

    // Deleted. A delete starts the identity's fresh life at once: Core
    // replaces the live member by its successor, so the wait ends typed
    // `incarnation_changed`.
    let doomed = register_bound(&identity_runtime, &bridge, "doomed", &session).await;
    let baseline = identity_runtime.completion_cursor(&doomed).await;
    let result = wait_racing(&live, &ctx, params("doomed", baseline), async {
        identity_runtime
            .delete_identity(&doomed)
            .await
            .expect("delete identity");
    })
    .await;
    assert_eq!(result["outcome"], "incarnation_changed", "{result}");
    live.runtime.shutdown().await;
}

/// #306: Meta runs no continuity repair supervisor, so its explicit trigger
/// `meta/request_continuity_repair` is gone. A caller that still sends it
/// gets the typed JSON-RPC method-not-found refusal, never an empty answer,
/// and `meta/capabilities` no longer lists it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn continuity_repair_request_method_is_gone() {
    let (live, _bridge, _identity_runtime, ctx, _session) = ticket_bridge_rpc().await;
    let reply = rpc_reply(
        &live.runtime,
        &ctx,
        "meta/request_continuity_repair",
        json!({}),
    )
    .await;
    assert_eq!(reply["error"]["code"], json!(-32601), "{reply}");
    assert!(reply.get("result").is_none(), "{reply}");

    let capabilities = rpc(&live.runtime, &ctx, "meta/capabilities", json!({})).await;
    let methods = capabilities["methods"]
        .as_array()
        .expect("capabilities list their methods");
    assert!(
        methods.contains(&json!("meta/status_identity")),
        "{methods:?}"
    );
    assert!(
        !methods.contains(&json!("meta/request_continuity_repair")),
        "{methods:?}"
    );
    live.runtime.shutdown().await;
}

/// Fields a wire caller adds to name the trusted host's governed dispatch
/// door or internal work. No request field selects a door or an origin:
/// only the identity runtime's two fixed-purpose Rust entry points do.
fn door_and_origin_fields(params: Value) -> Value {
    let Value::Object(mut fields) = params else {
        panic!("params are an object");
    };
    for (name, value) in [
        ("door", json!("dispatch")),
        ("governed_door", json!("trusted_host_internal")),
        ("work_origin", json!("internal")),
        ("origin", json!("internal")),
        ("dispatch", json!(true)),
    ] {
        fields.insert(name.to_string(), value);
    }
    Value::Object(fields)
}

/// An `InternalOnly` identity on the ticket bridge.
async fn register_internal_only(
    runtime: &IdentityRuntime,
    bridge: &TicketBridge,
    name: &str,
    session: &meerkat_core::types::SessionId,
) -> AgentIdentity {
    let mut spec = make_spec(name);
    spec.addressability = AgentAddressability::InternalOnly;
    register_spec(runtime, bridge, spec, session).await
}

/// The public send request handler (`meta/send`) to an internal-only
/// identity, with and without fields naming the dispatch door or internal
/// work, tracked and untracked: every request is the same public send,
/// refused `not addressable` before anything is delivered. No governed door
/// is reached, so no binder can be, and nothing enters the member session.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_public_send_naming_the_dispatch_door_or_internal_work_is_refused_not_addressable() {
    let (live, bridge, identity_runtime, ctx, session) = ticket_bridge_rpc().await;
    register_internal_only(&identity_runtime, &bridge, "helper", &session).await;

    let plain = json!({"identity": "helper", "content": "hello"});
    let tracked_plain = json!({"identity": "helper", "content": "hello", "track_turn": true});
    for params in [
        plain.clone(),
        door_and_origin_fields(plain),
        tracked_plain.clone(),
        door_and_origin_fields(tracked_plain),
    ] {
        let reply = rpc_reply(&live.runtime, &ctx, "meta/send", params.clone()).await;
        assert!(reply.get("result").is_none(), "{params}: {reply}");
        assert_eq!(reply["error"]["code"], -32002, "{params}: {reply}");
        assert_eq!(
            reply["error"]["message"], "not addressable: helper",
            "{params}: {reply}"
        );
    }
    assert!(
        bridge.ingress.lock().unwrap().is_empty(),
        "nothing delivered"
    );
    assert!(
        bridge.tracked.lock().unwrap().is_empty(),
        "nothing delivered"
    );
    assert_eq!(
        bridge.governed_controller_reads.load(Ordering::SeqCst),
        0,
        "no governed bind was entered, so no binder could be called"
    );
    assert_eq!(bridge.external_governed.load(Ordering::SeqCst), 0);
    assert_eq!(bridge.internal_governed.load(Ordering::SeqCst), 0);

    // The counter sees a governed bind being entered: the trusted host's
    // fixed-purpose Rust dispatch reaches the controller read for the same
    // identity (and stops there, this bridge pins no controller), before
    // its binder binds anything.
    let binder = Arc::new(CountingBinder(AtomicUsize::new(0)));
    let refused = identity_runtime
        .dispatch_governed_with_turn_ticket(
            &make_identity("helper"),
            None,
            meerkat_runtime::Input::Prompt(meerkat_runtime::PromptInput::new("hello", None)),
            binder.clone(),
        )
        .await;
    assert!(refused.is_err(), "this bridge pins no controller");
    assert_eq!(bridge.governed_controller_reads.load(Ordering::SeqCst), 1);
    assert_eq!(binder.0.load(Ordering::SeqCst), 0);
    live.runtime.shutdown().await;
}

/// A binder that counts its binds.
struct CountingBinder(AtomicUsize);

#[async_trait]
impl crate::identity_first::GovernedInputBinder for CountingBinder {
    async fn bind(
        &self,
        _context: &crate::identity_first::GovernedBindContext,
        _input: meerkat_runtime::Input,
    ) -> Result<crate::identity_first::BoundGovernedInput, String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err("counted, not bound".to_string())
    }

    fn retain_unresolved(
        &self,
        _settlement: Box<dyn crate::identity_first::GovernedSettlement>,
        _detail: &str,
    ) {
    }
}

/// The ordinary `meta/dispatch` keeps its established path whatever door or
/// origin a request names, at the top level or in `dispatch_input`: it is the
/// ordinary tracked dispatch (an unknown `dispatch_input.origin` is the
/// ordinary `system` origin), delivered through the bridge's ordinary
/// tracked delivery and never through either governed door.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_ordinary_dispatch_naming_a_governed_door_keeps_its_ordinary_path() {
    let (live, bridge, identity_runtime, ctx, session) = ticket_bridge_rpc().await;
    register_internal_only(&identity_runtime, &bridge, "helper", &session).await;

    let plain =
        json!({"identity": "helper", "track_turn": true, "dispatch_input": {"content": "one"}});
    let named = door_and_origin_fields(json!({
        "identity": "helper",
        "track_turn": true,
        "dispatch_input": door_and_origin_fields(json!({"content": "two"})),
    }));
    let mut results = Vec::new();
    for (index, params) in [plain, named].into_iter().enumerate() {
        let result = rpc(&live.runtime, &ctx, "meta/dispatch", params).await;
        let ticket = result["turn"]["ticket"].clone();
        assert!(ticket.is_string(), "{result}");
        bridge.finish(index, Ok("done"));
        let settled = rpc(
            &live.runtime,
            &ctx,
            "meta/wait_for_turn",
            json!({"identity": "helper", "ticket": ticket, "timeout_ms": 10_000}),
        )
        .await;
        assert_eq!(settled["state"], "completed", "{settled}");
        results.push(result);
    }
    let keys = |result: &Value| {
        result
            .as_object()
            .expect("result object")
            .keys()
            .cloned()
            .collect::<Vec<_>>()
    };
    assert_eq!(
        keys(&results[0]),
        keys(&results[1]),
        "the same ordinary dispatch reply"
    );
    assert_eq!(
        bridge.tracked.lock().unwrap().len(),
        2,
        "both through the ordinary tracked delivery"
    );
    assert!(bridge.ingress.lock().unwrap().is_empty());
    assert_eq!(
        bridge.governed_controller_reads.load(Ordering::SeqCst),
        0,
        "no governed bind was entered, so no binder could be called"
    );
    assert_eq!(bridge.external_governed.load(Ordering::SeqCst), 0);
    assert_eq!(bridge.internal_governed.load(Ordering::SeqCst), 0);
    live.runtime.shutdown().await;
}
