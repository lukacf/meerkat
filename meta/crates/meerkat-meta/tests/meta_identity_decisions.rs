#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! A change of agents that a person decides.
//!
//! A change whose outcome under the host's rules is `require_approval` is
//! held as a durable decision of kind `identity.<action>`, decided by a
//! person through `meta/decisions/decide`, and carried out by the decision
//! kind's executor in Core's executor loop. Each row boots the gateway's
//! runtime composition on a temporary state directory (persistent stores,
//! an approver map that lets two chat people decide for `team:owners`, the
//! `chat` decision ingress declared) and drives the holder of decisions
//! with a scripted gate: it normalizes a request against a policy whose
//! every change needs approval, and hands the normalized request to the
//! holder, as the family's implementation does. What the rows read back
//! they read from the decision store and from Core.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use meerkat_client::{LlmError, LlmEvent, LlmRequest};
use meerkat_core::approval::ApprovalConsentDeliveryState;
use meerkat_core::approval::decision::{
    ApproverId, DecisionHoldRequest, DecisionId, DecisionKind, DecisionListFilter,
    DecisionOperationId, DecisionPhase, DecisionPrincipalKey, DecisionReference, DecisionRenderer,
    DecisionRendering, DecisionRequestBody, DecisionSubjectKey, PresentationAudience,
    PresentationRecord, StoredDecision,
};
use meerkat_core::types::StopReason;
use meerkat_core::{Message, PrincipalKind, PrincipalRef, TrustDomainId};
use meerkat_meta::identity_creation::{
    AdoptIdentitySpec, AgentTools, ArchiveIdentitySpec, ChangeCommit, ChangeCommitted,
    ChangeConsent, ChangeRequester, ConsequenceLine, CoreChangeExecutor, CreateIdentitySpec,
    ExecutorError, HeldChangeHost, HeldIdentityChange, HeldTarget, HookRefusal,
    IDENTITY_DECISION_KINDS, IdentityChangeExecutor, IdentityCreationPolicy,
    IdentityDecisionNotice, IdentityDecisionNotices, IdentityDecisions, IdentityEntryFacts,
    IdentityOutcome, IdentityRefusal, IdentityRequest, KeyedSendNotices, NormalizedRequest,
    OperationId, ProfileTools, RemoveIdentitySpec, RequestContext, RequestEnvelope, Requester,
    RequesterFacts, Sha256Digest, ToolConfigurations, UnarchiveIdentitySpec, UpdateIdentitySpec,
    WithdrawRequest, consent_of_plan, core_member_name, host_consequences_of, normalize_adopt,
    normalize_archive, normalize_create, normalize_remove, normalize_unarchive, normalize_update,
    render_held_v1,
};
use meerkat_meta::identity_first::contracts::RosterProvider;
use meerkat_meta::identity_first::{
    AgentAddressability, AgentIdentity, DurableAgentSpec, RosterContext, RosterError,
};
use meerkat_meta::{
    AccessControlConfig, AccessController, DecisionNoticeSink, InMemoryMetadataStore,
    RuntimeDecisions, StdioDecisionConnection, UnifiedRuntime, UnifiedRuntimeBuilder,
};
use meerkat_mob::{IdentityEntryRead, MobDefinition, MobHandle, ProfileName};
use meerkat_runtime::decision::{
    DecisionExecutorLoop, DecisionPassOutcome, DurableDecisionService,
};
use serde_json::{Value, json};
use tokio::sync::{Notify, mpsc};

#[path = "support/llm_usage.rs"]
mod llm_usage;

/// The identity the host's roster declares.
const DECLARED: &str = "host-a";
/// The host surface the rows ask on.
const SURFACE: &str = "ops";
/// The approver id the policy names.
const APPROVERS: &str = "team:owners";
/// The chat people the approver map lets decide for [`APPROVERS`].
const OWNER: &str = "U123";
const SECOND_OWNER: &str = "U456";
/// Bounds a failure only; a passing row never waits for it.
const BOUND: Duration = Duration::from_secs(60);

static NEXT_MOB: AtomicU64 = AtomicU64::new(0);

const APPROVER_MAP: &str = r#"
enabled = false

[approvers."team:owners"]
subjects = ["owner@example.org"]
ingress_subjects = [
  { ingress = "chat", subject = "U123" },
  { ingress = "chat", subject = "U456" },
]
"#;

/// The mob's definition: the profile `worker`, with `tools` as its tool
/// table.
fn definition(mob_id: &str, tools: &str) -> MobDefinition {
    MobDefinition::from_toml(&format!(
        r#"
[mob]
id = "{mob_id}"

[profiles.worker]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.worker.tools]
{tools}

[profiles.lead]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.lead.tools]
comms = true
mob = true
"#
    ))
    .expect("parse the definition")
}

const TOOLS: &str = "comms = true";
const TOOLS_WITH_SHELL: &str = "comms = true\nshell = true";

fn fresh_mob_id() -> String {
    format!(
        "meta-identity-decisions-{}-{}",
        std::process::id(),
        NEXT_MOB.fetch_add(1, Ordering::Relaxed)
    )
}

fn now_ms() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_millis(),
    )
    .expect("ms")
}

fn spec(name: &str) -> DurableAgentSpec {
    DurableAgentSpec {
        identity: AgentIdentity::parse(name).unwrap(),
        profile: ProfileName::from("worker"),
        addressability: AgentAddressability::Addressable,
        display_name: None,
        labels: BTreeMap::new(),
        context: None,
        additional_instructions: vec![format!("REQUESTER-WRITTEN brief of {name}")],
        initial_message: None,
        runtime_mode_override: None,
        backend: None,
        binding: None,
        placement: None,
    }
}

struct DeclaredRoster;

#[async_trait]
impl RosterProvider for DeclaredRoster {
    async fn roster(&self, _context: &RosterContext) -> Result<Vec<DurableAgentSpec>, RosterError> {
        Ok(vec![spec(DECLARED)])
    }
}

fn controller() -> meerkat_core::ControllerModelSelection {
    meerkat_core::ControllerModelSelection::new(
        meerkat_core::SessionLlmIdentity {
            model: "gpt-5.5".into(),
            provider: meerkat_core::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: None,
        },
        serde_json::from_value(
            serde_json::json!({"realm": "meta-identity-decisions", "account": "recording"}),
        )
        .expect("nonsecret identity of the fixture transport"),
        "recording-profile".into(),
        "recording-transport".into(),
    )
}

/// The members' model: every request is kept and its last message sent on
/// the row's channel, and every request answers.
#[derive(Clone)]
struct RecordingClient {
    asked: mpsc::UnboundedSender<String>,
    requests: Arc<Mutex<Vec<String>>>,
}

impl meerkat_client::LlmClient for RecordingClient {
    fn controller_model_selection(&self) -> Option<meerkat_core::ControllerModelSelection> {
        Some(controller())
    }

    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>> {
        self.requests
            .lock()
            .unwrap()
            .push(serde_json::to_string(request).unwrap_or_default());
        if let Some(last) = request
            .messages
            .iter()
            .rev()
            .find(|message| !matches!(message, Message::SystemNotice(_)))
        {
            let _ = self
                .asked
                .send(serde_json::to_string(last).unwrap_or_default());
        }
        let [usage, done] =
            llm_usage::usage_then_done(request, meerkat::Provider::OpenAI, StopReason::EndTurn);
        Box::pin(async_stream::stream! {
            yield Ok(LlmEvent::TextDelta { delta: "noted".to_string(), meta: None });
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

/// What the seam was asked with.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Asked {
    operation_id: String,
    requester: ChangeRequester,
    /// The decision whose permit the commit carried, when it carried one.
    decided_by: Option<String>,
    configured: bool,
}

/// The seam the holder commits through, observed: every commit it is asked
/// is kept, the commits that landed are counted, and a row may cut the
/// process after a commit landed, before its answer is given.
struct ObservedSeam {
    inner: Option<Arc<dyn IdentityChangeExecutor>>,
    asked: Mutex<Vec<Asked>>,
    landed: AtomicUsize,
    cut_after_commit: AtomicBool,
    cut_reached: Notify,
}

impl ObservedSeam {
    /// The seam's own executor over Core.
    fn over_core() -> Arc<Self> {
        Self::new(Some(Arc::new(CoreChangeExecutor::new())))
    }

    /// A seam that commits nothing and answers a committed change.
    fn scripted() -> Arc<Self> {
        Self::new(None)
    }

    fn new(inner: Option<Arc<dyn IdentityChangeExecutor>>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            asked: Mutex::new(Vec::new()),
            landed: AtomicUsize::new(0),
            cut_after_commit: AtomicBool::new(false),
            cut_reached: Notify::new(),
        })
    }

    fn landed(&self) -> usize {
        self.landed.load(Ordering::SeqCst)
    }

    fn asked(&self) -> Vec<Asked> {
        self.asked.lock().unwrap().clone()
    }
}

#[async_trait]
impl IdentityChangeExecutor for ObservedSeam {
    async fn commit(&self, commit: ChangeCommit<'_>) -> Result<ChangeCommitted, ExecutorError> {
        self.asked.lock().unwrap().push(Asked {
            operation_id: commit.operation_id.to_string(),
            requester: commit.requester.clone(),
            decided_by: match commit.consent {
                ChangeConsent::Decided(permit) => Some(permit.decision_id().to_string()),
                _ => None,
            },
            configured: commit.configured_target,
        });
        let committed = match &self.inner {
            Some(inner) => inner.commit(commit).await?,
            None => ChangeCommitted::new(
                IdentityOutcome::Done {
                    operation_id: commit.operation_id.clone(),
                    agents: Vec::new(),
                },
                false,
            ),
        };
        if matches!(committed.outcome, IdentityOutcome::Done { .. }) && !committed.replayed {
            self.landed.fetch_add(1, Ordering::SeqCst);
            if self.cut_after_commit.load(Ordering::SeqCst) {
                self.cut_reached.notify_one();
                std::future::pending::<()>().await;
            }
        }
        Ok(committed)
    }
}

/// The gate's side of the holder: the row's one mob, and the agents the row
/// says exist.
#[derive(Default)]
struct ScriptedHost {
    mob: Mutex<Option<MobHandle>>,
    targets: Mutex<BTreeMap<String, HeldTarget>>,
}

#[async_trait]
impl HeldChangeHost for ScriptedHost {
    fn mob(&self, mob_id: &str) -> Option<MobHandle> {
        self.mob
            .lock()
            .unwrap()
            .clone()
            .filter(|mob| mob.mob_id().as_str() == mob_id)
    }

    async fn target(
        &self,
        _mob: &MobHandle,
        identity: &str,
    ) -> Result<Option<HeldTarget>, IdentityRefusal> {
        Ok(self.targets.lock().unwrap().get(identity).cloned())
    }
}

/// Every requester can be told: each notice is kept.
#[derive(Default)]
struct RecordedNotices(Mutex<Vec<IdentityDecisionNotice>>);

#[async_trait]
impl IdentityDecisionNotices for RecordedNotices {
    fn routable(&self, _requester: &Requester) -> bool {
        true
    }

    async fn deliver(
        &self,
        notice: &IdentityDecisionNotice,
    ) -> Option<ApprovalConsentDeliveryState> {
        self.0.lock().unwrap().push(notice.clone());
        Some(ApprovalConsentDeliveryState::Admitted {
            session_id: meerkat_core::SessionId::new(),
            input_id: format!("notice-{}", notice.decision_id),
        })
    }
}

impl RecordedNotices {
    fn all(&self) -> Vec<IdentityDecisionNotice> {
        self.0.lock().unwrap().clone()
    }
}

/// The lines the host states, or its refusal: a row changes them.
type HostLines = Arc<Mutex<Result<Vec<String>, String>>>;

#[derive(Default)]
struct StdioNotices;

#[async_trait]
impl DecisionNoticeSink for StdioNotices {
    async fn deliver(&self, _line: String) -> bool {
        true
    }
}

/// One booted gateway runtime, the holder of decisions over it and what
/// the rows observe.
struct Rig {
    runtime: Arc<UnifiedRuntime>,
    connection: StdioDecisionConnection,
    decisions: IdentityDecisions,
    seam: Arc<ObservedSeam>,
    host: Arc<ScriptedHost>,
    notices: Arc<RecordedNotices>,
    lines: HostLines,
    policy: IdentityCreationPolicy,
    asked: mpsc::UnboundedReceiver<String>,
    requests: Arc<Mutex<Vec<String>>>,
}

/// The policy the scripted gate normalizes under: every change needs a
/// person's approval for [`APPROVERS`], within `timeout_ms`.
fn approval_policy(timeout_ms: u64) -> IdentityCreationPolicy {
    IdentityCreationPolicy::parse(&json!({
        "enabled": true,
        "approval": {"rules": [{
            "change": ["create", "descriptive", "profile", "remove", "archive", "adopt"],
            "outcome": {
                "kind": "require_approval",
                "approver": {"id": APPROVERS},
                "timeout_ms": timeout_ms,
            },
        }]},
    }))
    .expect("the policy parses")
    .expect("the policy is enabled")
}

/// A policy that allows every change: the rows that need an agent to
/// exist create it under it.
fn allow_policy() -> IdentityCreationPolicy {
    IdentityCreationPolicy::parse(&json!({
        "enabled": true,
        "approval": {"rules": [{
            "change": ["create", "descriptive", "remove", "archive"],
            "outcome": {"kind": "allow"},
        }]},
    }))
    .expect("the policy parses")
    .expect("the policy is enabled")
}

fn chat_declared() -> meerkat_meta::DecisionIngressDeclarations {
    serde_json::from_value(json!({"ingresses": [
        {"ingress": "chat", "attests": "person", "max_clock_skew_ms": 5000},
    ]}))
    .expect("ingress declarations")
}

/// How a row boots: the profile's tool table, the policy's timeout, and
/// the seam the holder commits through.
struct Boot {
    tools: &'static str,
    timeout_ms: u64,
    seam: Arc<ObservedSeam>,
}

impl Default for Boot {
    fn default() -> Self {
        Self {
            tools: TOOLS,
            timeout_ms: 60 * 60 * 1000,
            seam: ObservedSeam::over_core(),
        }
    }
}

async fn boot(state: &Path, mob_id: &str) -> Rig {
    boot_with(state, mob_id, Boot::default()).await
}

async fn boot_with(state: &Path, mob_id: &str, options: Boot) -> Rig {
    let (asked_tx, asked) = mpsc::unbounded_channel();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let access: AccessControlConfig = toml::from_str(APPROVER_MAP).expect("map");
    let builder = UnifiedRuntimeBuilder::default()
        .definition(definition(mob_id, options.tools))
        .persistent_state(state)
        .continuity_from_state_dir(state)
        .await
        .expect("open the state-dir identity substrate")
        .roster_provider(Arc::new(DeclaredRoster))
        .identity_runtime_instance_id("meta-identity-decisions")
        .decision_ingresses(chat_declared())
        .access_controller(AccessController::new(access).expect("controller"))
        .default_llm_client(Arc::new(RecordingClient {
            asked: asked_tx,
            requests: Arc::clone(&requests),
        }));
    let runtime = Arc::new(
        Box::pin(builder.build())
            .await
            .expect("build the gateway-shaped UnifiedRuntime"),
    );
    let host = Arc::new(ScriptedHost::default());
    *host.mob.lock().unwrap() = Some(runtime.mob_handle());
    let notices = Arc::new(RecordedNotices::default());
    let lines: HostLines = Arc::new(Mutex::new(Ok(vec![
        "HOST-LINE the new agents may read the ledger".to_owned(),
    ])));
    let decisions = holder(
        runtime.durable_decisions(),
        &options.seam,
        &host,
        &notices,
        &lines,
    );
    let connection = StdioDecisionConnection::new(
        runtime.durable_decisions().realm(),
        Arc::new(StdioNotices) as Arc<dyn DecisionNoticeSink>,
    );
    Rig {
        runtime,
        connection,
        decisions,
        seam: options.seam,
        host,
        notices,
        lines,
        policy: approval_policy(options.timeout_ms),
        asked,
        requests,
    }
}

/// A holder of decisions over `decisions`, as the gate's wiring makes one.
fn holder(
    decisions: &RuntimeDecisions,
    seam: &Arc<ObservedSeam>,
    host: &Arc<ScriptedHost>,
    notices: &Arc<RecordedNotices>,
    lines: &HostLines,
) -> IdentityDecisions {
    let lines = Arc::clone(lines);
    let hook = move |_request: &IdentityRequest| -> Result<Vec<ConsequenceLine>, HookRefusal> {
        match &*lines.lock().unwrap() {
            Ok(lines) => Ok(lines.iter().map(ConsequenceLine::new).collect()),
            Err(reason) => Err(HookRefusal::new(reason.clone())),
        }
    };
    IdentityDecisions::new(
        decisions,
        Arc::clone(seam) as Arc<dyn IdentityChangeExecutor>,
        Arc::clone(host) as Arc<dyn HeldChangeHost>,
    )
    .with_consequences(host_consequences_of(hook))
    .with_notices(Arc::clone(notices) as Arc<dyn IdentityDecisionNotices>)
}

fn host_facts() -> RequesterFacts {
    RequesterFacts::host(SURFACE)
}

/// A person who asks, named by `key`.
fn human_facts(key: &str) -> RequesterFacts {
    let mut facts = RequesterFacts::host(SURFACE);
    facts.requester = Requester::Human {
        principal: key.to_owned(),
    };
    facts
}

/// The principal key of the chat person `subject`, as a decide by that
/// person carries it.
fn chat_person_key(subject: &str) -> String {
    DecisionPrincipalKey::of(
        &PrincipalRef::in_domain(
            PrincipalKind::Human,
            subject,
            TrustDomainId::new("meta-ingress-chat").expect("domain"),
        )
        .expect("person"),
    )
    .expect("key")
    .to_string()
}

fn normalized_create(
    policy: &IdentityCreationPolicy,
    requester: &RequesterFacts,
    specs: &[DurableAgentSpec],
) -> NormalizedRequest {
    normalized_create_with(policy, requester, specs, RequestEnvelope::default())
}

/// The mob's profiles, as normalization is given them.
fn mob_profiles() -> BTreeSet<String> {
    BTreeSet::from(["worker".to_owned(), "lead".to_owned()])
}

/// A normalized update of `entry`: its instructions change, and with
/// `profile` it moves to that profile.
fn normalized_update(
    policy: &IdentityCreationPolicy,
    requester: &RequesterFacts,
    entry: &IdentityEntryFacts,
    profile: Option<&str>,
) -> NormalizedRequest {
    let profiles = mob_profiles();
    let peers = BTreeMap::new();
    let mut update = UpdateIdentitySpec::new(entry.identity.as_str(), entry.spec_digest.clone());
    update.set.additional_instructions = Some(vec!["A new brief.".to_owned()]);
    update.set.profile = profile.map(ProfileName::from);
    normalize_update(
        policy,
        &RequestContext {
            requester,
            profiles: &profiles,
            peers: &peers,
        },
        RequestEnvelope::default(),
        entry,
        &update,
    )
    .expect("the update normalizes")
}

fn normalized_create_with(
    policy: &IdentityCreationPolicy,
    requester: &RequesterFacts,
    specs: &[DurableAgentSpec],
    envelope: RequestEnvelope,
) -> NormalizedRequest {
    let profiles = mob_profiles();
    let peers = BTreeMap::new();
    let specs: Vec<CreateIdentitySpec> =
        specs.iter().cloned().map(CreateIdentitySpec::new).collect();
    normalize_create(
        policy,
        &RequestContext {
            requester,
            profiles: &profiles,
            peers: &peers,
        },
        envelope,
        &specs,
    )
    .expect("the create normalizes")
}

fn operation(requester: &RequesterFacts, request_id: &str) -> OperationId {
    OperationId::for_request(&requester.requester, request_id).expect("a request id")
}

/// The decision id and the deadline of a held change.
fn pending(outcome: &IdentityOutcome) -> (String, u64) {
    let IdentityOutcome::PendingDecision {
        decision_id,
        deadline_ms,
        ..
    } = outcome
    else {
        panic!("the change is held: {outcome:?}");
    };
    (decision_id.clone(), *deadline_ms)
}

/// The kind token of a refused change.
fn refused(outcome: &IdentityOutcome) -> String {
    outcome
        .refusal()
        .unwrap_or_else(|| panic!("the change is refused: {outcome:?}"))
        .kind_token()
        .to_owned()
}

fn member(name: &str) -> meerkat_mob::AgentIdentity {
    core_member_name(name).expect("an agent name")
}

impl Rig {
    fn mob(&self) -> MobHandle {
        self.runtime.mob_handle()
    }

    fn owner(&self) -> DurableDecisionService {
        self.runtime.durable_decisions().owner().clone()
    }

    fn row(&self, decision_id: &str) -> StoredDecision {
        self.owner()
            .get(&DecisionId::new(decision_id).expect("a decision id"))
            .expect("the decision is read")
    }

    /// The scripted gate: normalize a create of `names` asked by
    /// `requester` as `request_id`, and hold it.
    async fn ask_create(
        &self,
        requester: &RequesterFacts,
        request_id: &str,
        names: &[&str],
    ) -> IdentityOutcome {
        let specs: Vec<DurableAgentSpec> = names.iter().map(|name| spec(name)).collect();
        let normalized = normalized_create(&self.policy, requester, &specs);
        self.decisions
            .hold(&self.mob(), &operation(requester, request_id), &normalized)
            .await
    }

    /// Record that a host channel showed the decision to its approvers.
    fn present(&self, decision_id: &str) {
        let row = self.row(decision_id);
        self.owner()
            .present(
                &row.decision_id,
                &PresentationRecord::complete(
                    PresentationAudience::Approver {
                        approver_id: ApproverId::new(APPROVERS).expect("approver"),
                    },
                    &row.descriptor.rendering,
                    None,
                    DecisionReference::new("chat:owners").expect("channel"),
                    DecisionReference::new(format!("message-{}", now_ms())).expect("message"),
                    now_ms(),
                ),
            )
            .expect("present");
    }

    /// One `meta/decisions/*` call on the stdio host's connection.
    async fn call(&self, method: &str, params: Value) -> Value {
        let line = json!({"jsonrpc": "2.0", "id": "decisions", "method": method, "params": params})
            .to_string();
        let answer = self
            .connection
            .handle_line(&self.runtime, &line)
            .await
            .expect("a decision method answers");
        serde_json::from_str(&answer).expect("answer JSON")
    }

    /// The chat person `subject` decides through `meta/decisions/decide`,
    /// attested by the `chat` ingress.
    async fn chat_decides(&self, decision_id: &str, subject: &str, approve: bool) -> Value {
        let row = self.row(decision_id);
        self.call(
            "meta/decisions/decide",
            json!({
                "decision_id": decision_id,
                "approve": approve,
                "presented_digest": row.descriptor.subject_digest.to_string(),
                "attestation": {
                    "ingress": "chat",
                    "subject": subject,
                    "scope": "chat-session-1",
                    "event_ref": format!("chat:event:{decision_id}:{subject}:{approve}"),
                    "observed_at_ms": now_ms(),
                },
            }),
        )
        .await
    }

    /// Present the decision and have [`OWNER`] approve it.
    async fn approve(&self, decision_id: &str) {
        self.present(decision_id);
        let answer = self.chat_decides(decision_id, OWNER, true).await;
        assert!(answer["error"].is_null(), "{answer:#?}");
        assert_eq!(self.row(decision_id).state.phase, DecisionPhase::Approved);
    }

    /// Core's executor loop for the `identity.*` kinds over this runtime.
    fn executor_loop(&self) -> DecisionExecutorLoop {
        self.decisions
            .executor_loop(self.runtime.decision_approver_currentness())
            .expect("the executor loop starts")
    }

    /// Whether Core holds `name` as an agent.
    fn exists(&self, name: &str) -> bool {
        matches!(
            self.mob().identity_entry(&member(name)),
            Ok(IdentityEntryRead::Entry(_))
        )
    }

    /// How many requests the members' model was sent.
    fn model_requests(&self) -> usize {
        self.requests.lock().unwrap().len()
    }

    /// How many decisions of the `identity.*` kinds are in `phase`.
    fn count(&self, phase: DecisionPhase) -> u64 {
        IDENTITY_DECISION_KINDS
            .into_iter()
            .map(|kind| {
                self.owner()
                    .count(
                        &DecisionListFilter::default()
                            .with_kind(DecisionKind::new(kind).expect("kind"))
                            .with_phase(phase),
                    )
                    .expect("count")
            })
            .sum()
    }

    /// Create `name` through the seam under a rule that allows it, and
    /// answer its spec digest.
    async fn create_allowed(&self, name: &str) -> Sha256Digest {
        let facts = host_facts();
        let normalized = normalized_create(&allow_policy(), &facts, &[spec(name)]);
        let operation_id = operation(&facts, &format!("allowed-create-{name}"));
        let mob = self.mob();
        let committed = CoreChangeExecutor::new()
            .commit(ChangeCommit::new(
                &mob,
                &operation_id,
                &ChangeRequester::host(SURFACE),
                consent_of_plan(&normalized.plan).expect("the rule allows the create"),
                &normalized.request,
            ))
            .await
            .expect("the seam answers");
        let IdentityOutcome::Done { agents, .. } = &committed.outcome else {
            panic!("the create of {name} is done: {:?}", committed.outcome);
        };
        agents[0].spec_digest.clone().expect("a spec digest")
    }

    /// Wait until a member's turn is asked something that contains
    /// `marker`; `false` when none is within the bound.
    async fn asked(&mut self, marker: &str) -> bool {
        tokio::time::timeout(BOUND, async {
            while let Some(asked) = self.asked.recv().await {
                if asked.contains(marker) {
                    return true;
                }
            }
            false
        })
        .await
        .unwrap_or(false)
    }

    async fn shutdown(self) {
        let _ = self.runtime.shutdown().await;
    }

    /// Shut down before a reopen over the same state directory in this
    /// process: stand the durable child root down, bounded, until it is
    /// released.
    async fn shutdown_for_reopen(self) {
        let mut outcome = self.runtime.shutdown().await.child_mob_stand_down;
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while !child_root_released(&outcome) && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
            outcome = self
                .runtime
                .mob_runtime()
                .stand_down_child_mobs(deadline)
                .await;
        }
        assert!(
            child_root_released(&outcome),
            "the durable child root is released before the reopen: {outcome:?}"
        );
    }
}

fn child_root_released(outcome: &meerkat_meta::ChildMobStandDownOutcome) -> bool {
    matches!(
        outcome,
        meerkat_meta::ChildMobStandDownOutcome::Released { .. }
            | meerkat_meta::ChildMobStandDownOutcome::NotConfigured
    )
}

/// Passes of `executors` until one acts on nothing: what each pass did to
/// each decision, in order.
async fn drive(executors: &mut DecisionExecutorLoop) -> Vec<DecisionPassOutcome> {
    let mut did = Vec::new();
    for _ in 0..6 {
        let pass = executors.pass().await;
        assert!(pass.read_failures.is_empty(), "{:?}", pass.read_failures);
        if pass.outcomes.is_empty() {
            break;
        }
        did.extend(pass.outcomes.into_iter().map(|(_, outcome)| outcome));
    }
    did
}

/// Hold, decide, carry out, tell: a create of two agents that needs
/// approval is ONE decision of kind `identity.create` that answers
/// `pending_decision`; once a person approved it the executor loop commits
/// both agents through the seam under the decision's permit, as the
/// recorded requester, exactly once; the requester is told the committed
/// outcome; and the same request asked again answers that outcome.
///
/// RED before the holder held: the change was refused
/// `approval_unavailable` and no decision existed.
#[tokio::test(flavor = "multi_thread")]
async fn a_change_that_needs_approval_is_held_decided_carried_out_and_told() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), &fresh_mob_id()).await;
    let facts = host_facts();

    let held = rig.ask_create(&facts, "team-1", &["scout", "scribe"]).await;
    let (decision_id, deadline_ms) = pending(&held);
    let row = rig.row(&decision_id);
    assert_eq!(row.descriptor.kind.as_str(), "identity.create");
    assert_eq!(
        row.descriptor.subject_key.as_str(),
        format!(
            "{}/{}",
            rig.mob().mob_id(),
            operation(&facts, "team-1").as_str()
        ),
        "the decision's subject is the operation in its mob"
    );
    assert_eq!(row.descriptor.approver_id.as_str(), APPROVERS);
    assert_eq!(row.descriptor.deadline_at_ms, deadline_ms);
    assert_eq!(
        rig.count(DecisionPhase::Open),
        1,
        "a create of two agents is one decision"
    );
    let body = HeldIdentityChange::of(&row).expect("the decision holds the normalized request");
    assert_eq!(
        body.request.action.identities(),
        vec!["scout", "scribe"],
        "the one decision covers both agents"
    );
    assert_eq!(
        row.descriptor.request_digest.as_str(),
        body.request.request_digest().unwrap().as_str(),
        "the decision binds the request's own digest"
    );
    assert_eq!(rig.seam.landed(), 0, "a hold commits nothing");

    rig.approve(&decision_id).await;
    let mut executors = rig.executor_loop();
    let did = drive(&mut executors).await;
    assert_eq!(
        did,
        vec![
            DecisionPassOutcome::Executed,
            DecisionPassOutcome::NoticeSettled
        ],
        "the loop carries the change out and tells its requester"
    );
    assert!(
        rig.exists("scout") && rig.exists("scribe"),
        "both agents exist"
    );
    assert_eq!(rig.seam.landed(), 1, "one commit for the one decision");
    assert_eq!(
        rig.seam.asked(),
        vec![Asked {
            operation_id: operation(&facts, "team-1").to_string(),
            requester: ChangeRequester::host(SURFACE),
            decided_by: Some(decision_id.clone()),
            configured: false,
        }],
        "the commit is the recorded requester's, under the decision's permit"
    );
    assert_eq!(rig.row(&decision_id).state.phase, DecisionPhase::Executed);

    let notices = rig.notices.all();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!(notices[0].decision_id, decision_id);
    assert_eq!(notices[0].requester, facts.requester);
    assert_eq!(notices[0].action, "create");
    assert_eq!(notices[0].agents, vec!["scout", "scribe"]);
    let told: Vec<&str> = notices[0]
        .outcome
        .agents()
        .iter()
        .map(|agent| agent.name.as_str())
        .collect();
    assert_eq!(
        (notices[0].outcome.token(), told),
        ("done", vec!["scout", "scribe"]),
        "the notice carries the committed outcome"
    );

    let again = rig.ask_create(&facts, "team-1", &["scout", "scribe"]).await;
    assert_eq!(
        again, notices[0].outcome,
        "the same request answers the committed outcome"
    );
    assert_eq!(
        drive(&mut executors).await,
        Vec::new(),
        "nothing is owed after"
    );
    assert_eq!(rig.seam.landed(), 1);
    rig.shutdown().await;
}

/// A held create writes the decision and nothing else: no agent exists in
/// Core, no record of one, no member's model is asked anything, and the
/// executor loop's passes over the open decision change none of that.
///
/// RED before the holder held: nothing was held, so the row's first
/// assertion (a pending decision) failed.
#[tokio::test(flavor = "multi_thread")]
async fn a_held_create_wakes_nothing_and_builds_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), &fresh_mob_id()).await;

    let held = rig.ask_create(&host_facts(), "quiet-1", &["scout"]).await;
    let (decision_id, _) = pending(&held);
    let mut executors = rig.executor_loop();
    assert_eq!(drive(&mut executors).await, Vec::new());

    assert!(
        !rig.exists("scout"),
        "no agent exists while the change is held"
    );
    assert!(
        rig.mob()
            .identity_record(&member("scout"))
            .await
            .expect("Core's record is read")
            .is_none(),
        "Core holds no record of the agent"
    );
    assert_eq!(rig.model_requests(), 0, "no member was woken");
    assert_eq!(rig.seam.asked(), Vec::new(), "the seam was not asked");
    assert_eq!(rig.row(&decision_id).state.phase, DecisionPhase::Open);
    assert_eq!(rig.notices.all().len(), 0);
    rig.shutdown().await;
}

/// A person rejects: nothing changes, the requester is told
/// `decision_closed` with the state `rejected`, and the same request
/// answers that refusal.
///
/// RED before the holder held.
#[tokio::test(flavor = "multi_thread")]
async fn a_rejected_change_changes_nothing_and_its_requester_is_told() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), &fresh_mob_id()).await;
    let facts = host_facts();
    let (decision_id, _) = pending(&rig.ask_create(&facts, "no-1", &["scout"]).await);

    rig.present(&decision_id);
    let answer = rig.chat_decides(&decision_id, OWNER, false).await;
    assert!(answer["error"].is_null(), "{answer:#?}");
    let mut executors = rig.executor_loop();
    assert_eq!(
        drive(&mut executors).await,
        vec![DecisionPassOutcome::NoticeSettled]
    );

    assert!(!rig.exists("scout"), "a rejected create creates nothing");
    assert_eq!(rig.seam.asked(), Vec::new(), "the seam was not asked");
    let notices = rig.notices.all();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!(
        notices[0].outcome.to_json()["kind"],
        json!("decision_closed")
    );
    assert_eq!(notices[0].outcome.to_json()["state"], json!("rejected"));
    let again = rig.ask_create(&facts, "no-1", &["scout"]).await;
    assert_eq!(again.to_json()["state"], json!("rejected"), "{again:?}");
    rig.shutdown().await;
}

/// The decision's deadline is the policy's timeout for the change, and a
/// decision nobody decided by then ends expired: nothing changes and the
/// requester is told `decision_closed` with the state `expired`.
///
/// RED before the holder held.
#[tokio::test(flavor = "multi_thread")]
async fn an_undecided_change_expires_at_its_policy_timeout_and_its_requester_is_told() {
    let temp = tempfile::tempdir().unwrap();
    let timeout_ms = 1500;
    let rig = boot_with(
        temp.path(),
        &fresh_mob_id(),
        Boot {
            timeout_ms,
            ..Boot::default()
        },
    )
    .await;
    let facts = host_facts();
    let (decision_id, deadline_ms) = pending(&rig.ask_create(&facts, "late-1", &["scout"]).await);
    let row = rig.row(&decision_id);
    let window = deadline_ms - row.descriptor.created_at_ms;
    assert!(
        (timeout_ms - 250..=timeout_ms).contains(&window),
        "the deadline is the hold's time plus the policy's timeout: {window} ms"
    );

    tokio::time::sleep(Duration::from_millis(timeout_ms + 300)).await;
    let mut executors = rig.executor_loop();
    assert_eq!(
        drive(&mut executors).await,
        vec![
            DecisionPassOutcome::Expired,
            DecisionPassOutcome::NoticeSettled
        ]
    );
    assert_eq!(rig.row(&decision_id).state.phase, DecisionPhase::Expired);
    assert!(!rig.exists("scout"));
    let notices = rig.notices.all();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!(notices[0].outcome.to_json()["state"], json!("expired"));
    assert_eq!(
        rig.ask_create(&facts, "late-1", &["scout"]).await.to_json()["state"],
        json!("expired")
    );
    rig.shutdown().await;
}

/// The requester withdraws its own held change, and the host withdraws
/// any; another requester is answered `decision_not_found`, as for an
/// operation nothing holds. A withdrawn change changes nothing, its
/// requester is told, and it cannot be withdrawn twice.
///
/// RED before the holder held.
#[tokio::test(flavor = "multi_thread")]
async fn a_held_change_is_withdrawn_by_its_requester_or_the_host_and_by_nobody_else() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), &fresh_mob_id()).await;
    let mob = rig.mob();
    let asker = human_facts(&chat_person_key("U900"));
    let asker_as = ChangeRequester::human(chat_person_key("U900"));
    let other_as = ChangeRequester::human(chat_person_key("U901"));
    let first = operation(&asker, "wd-1");
    let (decision_id, _) = pending(&rig.ask_create(&asker, "wd-1", &["scout"]).await);

    let nothing = operation(&asker, "never-asked");
    assert_eq!(
        rig.decisions
            .withdraw(&mob, &asker_as, &WithdrawRequest::new(nothing))
            .expect_err("nothing holds the operation")
            .kind_token(),
        "decision_not_found"
    );
    assert_eq!(
        rig.decisions
            .withdraw(&mob, &other_as, &WithdrawRequest::new(first.clone()))
            .expect_err("another requester withdraws nothing")
            .kind_token(),
        "decision_not_found"
    );
    assert_eq!(rig.row(&decision_id).state.phase, DecisionPhase::Open);

    let withdrawn = rig
        .decisions
        .withdraw(&mob, &asker_as, &WithdrawRequest::new(first.clone()))
        .expect("the requester withdraws its own change");
    assert_eq!(withdrawn.decision_id, decision_id);
    assert_eq!(rig.row(&decision_id).state.phase, DecisionPhase::Withdrawn);
    let mut executors = rig.executor_loop();
    assert_eq!(
        drive(&mut executors).await,
        vec![DecisionPassOutcome::NoticeSettled]
    );
    let notices = rig.notices.all();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!(notices[0].outcome.to_json()["state"], json!("withdrawn"));
    let twice = rig
        .decisions
        .withdraw(&mob, &asker_as, &WithdrawRequest::new(first))
        .expect_err("a withdrawn change is not withdrawn again");
    assert_eq!(twice.to_refused()["state"], json!("withdrawn"), "{twice}");
    assert!(!rig.exists("scout"));

    let second = operation(&asker, "wd-2");
    let (second_id, _) = pending(&rig.ask_create(&asker, "wd-2", &["scribe"]).await);
    rig.decisions
        .withdraw(
            &mob,
            &ChangeRequester::host(SURFACE),
            &WithdrawRequest::new(second),
        )
        .expect("the host withdraws any held change");
    assert_eq!(rig.row(&second_id).state.phase, DecisionPhase::Withdrawn);
    assert_eq!(rig.seam.asked(), Vec::new(), "nothing was committed");
    rig.shutdown().await;
}

/// The same request id returns the same held state, and writes no second
/// decision: the same decision, the same deadline, whatever the host says
/// by then. Another request under the same id is `operation_reused`, and
/// the operation read answers the held state without a request.
///
/// RED before the holder held.
#[tokio::test(flavor = "multi_thread")]
async fn the_same_request_id_returns_the_held_state_and_another_request_is_operation_reused() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), &fresh_mob_id()).await;
    let facts = host_facts();
    let first = rig.ask_create(&facts, "same-1", &["scout"]).await;
    pending(&first);

    tokio::time::sleep(Duration::from_millis(20)).await;
    *rig.lines.lock().unwrap() = Ok(vec!["HOST-LINE said later".to_owned()]);
    let again = rig.ask_create(&facts, "same-1", &["scout"]).await;
    assert_eq!(again, first, "the same request answers the same held state");
    assert_eq!(rig.count(DecisionPhase::Open), 1, "one decision holds it");

    let other = rig.ask_create(&facts, "same-1", &["scribe"]).await;
    assert_eq!(refused(&other), "operation_reused");

    let read = rig
        .decisions
        .held(&rig.mob(), &operation(&facts, "same-1"))
        .await
        .expect("the held state is read");
    assert_eq!(read, Some(first));
    let unknown = rig
        .decisions
        .held(&rig.mob(), &operation(&facts, "never-asked"))
        .await
        .expect("the held state is read");
    assert_eq!(unknown, None, "nothing holds an operation never asked");
    rig.shutdown().await;
}

/// Eight executor instances drive the same approved decision at once: one
/// of them holds the kind's claim and commits; the change is committed
/// exactly once, one pass records it executed, and a later instance finds
/// nothing to carry out.
///
/// RED before the holder held.
#[tokio::test(flavor = "multi_thread")]
async fn eight_concurrent_drives_commit_an_approved_change_exactly_once() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), &fresh_mob_id()).await;
    let (decision_id, _) = pending(&rig.ask_create(&host_facts(), "once-1", &["scout"]).await);
    rig.approve(&decision_id).await;

    let mut loops: Vec<DecisionExecutorLoop> = (0..8)
        .map(|_| {
            holder(
                rig.runtime.durable_decisions(),
                &rig.seam,
                &rig.host,
                &rig.notices,
                &rig.lines,
            )
            .executor_loop(rig.runtime.decision_approver_currentness())
            .expect("an executor instance starts")
        })
        .collect();
    let mut executed = 0;
    for _ in 0..3 {
        let passes =
            futures::future::join_all(loops.iter_mut().map(|executors| executors.pass())).await;
        executed += passes
            .iter()
            .flat_map(|pass| &pass.outcomes)
            .filter(|(_, outcome)| *outcome == DecisionPassOutcome::Executed)
            .count();
    }
    assert_eq!(executed, 1, "one pass of one instance carried it out");
    assert_eq!(rig.seam.landed(), 1, "exactly one commit");
    assert_eq!(rig.seam.asked().len(), 1, "the seam was asked once");
    assert!(rig.exists("scout"));

    drop(loops);
    let mut later = rig.executor_loop();
    assert_eq!(drive(&mut later).await, Vec::new());
    assert_eq!(rig.seam.landed(), 1, "a later instance commits nothing");
    assert_eq!(rig.row(&decision_id).state.phase, DecisionPhase::Executed);
    rig.shutdown().await;
}

/// The process is cut between Core's commit and the decision's settlement:
/// the change is committed in Core and the decision still waits with its
/// permit outstanding. After a restart the loop's first pass reads Core's
/// receipt, records the decision executed and asks the seam for nothing;
/// the agent exists once, and the requester is told.
///
/// RED before the holder held.
#[tokio::test(flavor = "multi_thread")]
async fn a_process_cut_between_the_commit_and_the_settlement_settles_executed_after_a_restart() {
    let temp = tempfile::tempdir().unwrap();
    let mob_id = fresh_mob_id();
    let rig = boot(temp.path(), &mob_id).await;
    let facts = host_facts();
    let (decision_id, _) = pending(&rig.ask_create(&facts, "cut-1", &["scout"]).await);
    rig.approve(&decision_id).await;

    rig.seam.cut_after_commit.store(true, Ordering::SeqCst);
    let mut executors = rig.executor_loop();
    tokio::select! {
        pass = executors.pass() => panic!("the pass is cut before it settles: {pass:?}"),
        () = rig.seam.cut_reached.notified() => {}
        () = tokio::time::sleep(BOUND) => panic!("the commit did not land"),
    }
    drop(executors);
    assert_eq!(rig.seam.landed(), 1, "Core committed the change");
    assert!(rig.exists("scout"));
    let cut = rig.row(&decision_id);
    assert_eq!(cut.state.phase, DecisionPhase::Approved);
    assert!(cut.state.permit_issued, "the permit is outstanding");
    rig.shutdown_for_reopen().await;

    let rig = boot(temp.path(), &mob_id).await;
    let mut executors = rig.executor_loop();
    assert_eq!(
        drive(&mut executors).await,
        vec![
            DecisionPassOutcome::SettledExecuted,
            DecisionPassOutcome::NoticeSettled
        ],
        "the first pass settles the commit that landed"
    );
    assert_eq!(
        rig.seam.asked(),
        Vec::new(),
        "nothing is committed a second time"
    );
    assert_eq!(rig.row(&decision_id).state.phase, DecisionPhase::Executed);
    assert!(rig.exists("scout"));
    let notices = rig.notices.all();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!(notices[0].outcome.token(), "done");
    assert_eq!(
        rig.ask_create(&facts, "cut-1", &["scout"]).await.token(),
        "done"
    );
    rig.shutdown().await;
}

/// Declare `definition` as the stored definition's successor through the
/// spec-update ceremony, the sanctioned path for an edited definition: a
/// persistent mob pins its definition, and a start that carries an
/// undeclared edit is refused as divergent.
async fn declare_definition(state: &Path, definition: MobDefinition) {
    let storage = meerkat_mob::MobStorage::persistent(state.join("event_log.sqlite3"))
        .expect("open the mob store");
    let mob_id = definition.id.as_str().to_string();
    meerkat_meta::spec_update_ceremony::declare_spec_update(&storage, &mob_id, &definition, 1)
        .await
        .expect("declare the edited definition");
}

/// The profile's declared tool configuration changes after the approval:
/// the host declares an updated definition that turns `shell` on for the
/// profile and starts again. The executor renders the request again, the
/// rendering is no longer the one the person approved, and the change is
/// refused `subject_widened`. The decision ends refused, nothing is
/// committed and the requester is told.
///
/// RED before the holder held.
#[tokio::test(flavor = "multi_thread")]
async fn a_tool_configuration_changed_after_the_approval_refuses_subject_widened() {
    let temp = tempfile::tempdir().unwrap();
    let mob_id = fresh_mob_id();
    let rig = boot(temp.path(), &mob_id).await;
    let facts = host_facts();
    let (decision_id, _) = pending(&rig.ask_create(&facts, "tools-1", &["scout"]).await);
    let approved_rendering = rig.row(&decision_id).descriptor.rendering.to_string();
    assert!(
        approved_rendering.contains("Tool families (1):\n- comms\n"),
        "the rendering shows what the profile declares:\n{approved_rendering}"
    );
    rig.approve(&decision_id).await;
    rig.shutdown_for_reopen().await;

    declare_definition(temp.path(), definition(&mob_id, TOOLS_WITH_SHELL)).await;
    let rig = boot_with(
        temp.path(),
        &mob_id,
        Boot {
            tools: TOOLS_WITH_SHELL,
            ..Boot::default()
        },
    )
    .await;
    let now = rig
        .decisions
        .render(
            &rig.mob(),
            &HeldIdentityChange::of(&rig.row(&decision_id))
                .unwrap()
                .request,
        )
        .await
        .expect("the request renders");
    assert!(
        now.as_str()
            .contains("Tool families (2):\n- shell\n- comms\n"),
        "the profile declares shell now:\n{}",
        now.as_str()
    );
    let mut executors = rig.executor_loop();
    assert_eq!(
        drive(&mut executors).await,
        vec![
            DecisionPassOutcome::ExecutionRefused,
            DecisionPassOutcome::NoticeSettled
        ]
    );
    let row = rig.row(&decision_id);
    assert_eq!(row.state.phase, DecisionPhase::ExecutionRefused);
    assert_eq!(
        row.execution_refusal
            .as_ref()
            .map(DecisionReference::as_str),
        Some("subject_widened")
    );
    assert_eq!(rig.seam.asked(), Vec::new(), "the seam was not asked");
    assert!(!rig.exists("scout"));
    let notices = rig.notices.all();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!(refused(&notices[0].outcome), "subject_widened");
    assert_eq!(
        refused(&rig.ask_create(&facts, "tools-1", &["scout"]).await),
        "subject_widened"
    );
    rig.shutdown().await;
}

/// The host's consequence lines are rendered ahead of anything the
/// requester wrote and are inside the digest the person approves. Lines
/// that differ when the change is carried out refuse it `subject_widened`
/// and close the decision; nothing is committed.
///
/// RED before the holder held.
#[tokio::test(flavor = "multi_thread")]
async fn host_lines_that_differ_at_execution_refuse_subject_widened() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), &fresh_mob_id()).await;
    let facts = host_facts();
    let (decision_id, _) = pending(&rig.ask_create(&facts, "lines-1", &["scout"]).await);
    let rendering = rig.row(&decision_id).descriptor.rendering.to_string();
    let host_line = rendering
        .find("HOST-LINE the new agents may read the ledger")
        .expect("the host's line is rendered");
    let tools = rendering
        .find("Agent profiles (1):")
        .expect("the tool configuration is rendered");
    let written = rendering
        .find("REQUESTER-WRITTEN brief of scout")
        .expect("the requester's text is rendered");
    assert!(
        host_line < tools && tools < written,
        "the host's lines, then the tools, then the requester's text:\n{rendering}"
    );
    rig.approve(&decision_id).await;

    *rig.lines.lock().unwrap() = Ok(vec![
        "HOST-LINE the new agents may read AND WRITE the ledger".to_owned(),
    ]);
    let mut executors = rig.executor_loop();
    assert_eq!(
        drive(&mut executors).await,
        vec![
            DecisionPassOutcome::ExecutionRefused,
            DecisionPassOutcome::NoticeSettled
        ]
    );
    let row = rig.row(&decision_id);
    assert_eq!(
        row.execution_refusal
            .as_ref()
            .map(DecisionReference::as_str),
        Some("subject_widened")
    );
    assert_eq!(rig.seam.asked(), Vec::new(), "the seam was not asked");
    assert!(!rig.exists("scout"));
    assert_eq!(refused(&rig.notices.all()[0].outcome), "subject_widened");
    rig.shutdown().await;
}

/// A host that cannot state its lines when the change is carried out does
/// not end the decision: the attempt is released, the approval stands, and
/// the next pass, with the host's lines back, carries the change out. A
/// host that cannot state them when a change is asked refuses it
/// `consequences_unavailable`, and nothing is held.
///
/// RED before the holder held.
#[tokio::test(flavor = "multi_thread")]
async fn a_retryable_refusal_releases_and_never_burns_the_approval() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), &fresh_mob_id()).await;
    let (decision_id, _) = pending(&rig.ask_create(&host_facts(), "retry-1", &["scout"]).await);
    rig.approve(&decision_id).await;
    let stated = rig.lines.lock().unwrap().clone();

    *rig.lines.lock().unwrap() = Err("the ledger service is away".to_owned());
    let mut executors = rig.executor_loop();
    let pass = executors.pass().await;
    assert_eq!(
        pass.outcomes
            .iter()
            .map(|(_, outcome)| outcome.clone())
            .collect::<Vec<_>>(),
        vec![DecisionPassOutcome::Released]
    );
    let row = rig.row(&decision_id);
    assert_eq!(
        row.state.phase,
        DecisionPhase::Approved,
        "the approval stands"
    );
    assert!(!row.state.permit_issued, "no permit is outstanding");
    assert!(!rig.exists("scout"));

    *rig.lines.lock().unwrap() = stated;
    assert_eq!(
        drive(&mut executors).await,
        vec![
            DecisionPassOutcome::Executed,
            DecisionPassOutcome::NoticeSettled
        ]
    );
    assert!(rig.exists("scout"));
    assert_eq!(rig.seam.landed(), 1);

    // The same refusal when a change is asked: the host's lines fail
    // closed before anything is held.
    *rig.lines.lock().unwrap() = Err("the ledger service is away".to_owned());
    let unheld = rig.ask_create(&host_facts(), "retry-2", &["scribe"]).await;
    assert_eq!(refused(&unheld), "consequences_unavailable");
    assert!(unheld.refusal().expect("refused").is_retryable());
    assert_eq!(rig.count(DecisionPhase::Open), 0, "nothing was held");
    rig.shutdown().await;
}

/// An agent that changed after the request was held is Core's to refuse:
/// the held removal names the spec digest its requester read, the agent's
/// spec moves, and Core's commit answers `stale_spec`. The decision ends
/// refused with that kind, and the agent stays.
///
/// RED before the holder held.
#[tokio::test(flavor = "multi_thread")]
async fn a_target_that_moved_after_the_hold_is_refused_stale_spec_and_closes_the_decision() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), &fresh_mob_id()).await;
    let facts = host_facts();
    let digest = rig.create_allowed("keeper").await;
    let entry =
        IdentityEntryFacts::new("keeper", digest.clone(), Some(1), "worker", BTreeMap::new());
    rig.host
        .targets
        .lock()
        .unwrap()
        .insert("keeper".to_owned(), HeldTarget::new(entry.clone(), false));
    let normalized = normalize_remove(
        &rig.policy,
        &facts,
        RequestEnvelope::default(),
        &entry,
        &RemoveIdentitySpec::new("keeper", digest.clone()),
    )
    .expect("the removal normalizes");
    let held = rig
        .decisions
        .hold(&rig.mob(), &operation(&facts, "remove-1"), &normalized)
        .await;
    let (decision_id, _) = pending(&held);
    assert_eq!(
        rig.row(&decision_id).descriptor.kind.as_str(),
        "identity.remove"
    );
    rig.approve(&decision_id).await;

    // The agent's spec moves: a display name is set through Core.
    rig.mob()
        .update_store_owned_identity(
            meerkat_mob::StoreOwnedIdentityUpdate::new(
                meerkat_mob::IdentityOperation::new("move-keeper", "sha256:moved"),
                meerkat_mob::IdentityChangeRequesterInput::host(SURFACE),
                meerkat_mob::CreatedIdentitySpec::new(
                    member("keeper"),
                    ProfileName::from("worker"),
                )
                .with_display_name("The keeper, renamed"),
                digest.as_str(),
            ),
            ChangeConsent::Policy(meerkat_mob::IdentityPolicyOutcome::Allow),
        )
        .await
        .expect("the agent's spec moves");

    let mut executors = rig.executor_loop();
    assert_eq!(
        drive(&mut executors).await,
        vec![
            DecisionPassOutcome::ExecutionRefused,
            DecisionPassOutcome::NoticeSettled
        ]
    );
    let row = rig.row(&decision_id);
    assert_eq!(row.state.phase, DecisionPhase::ExecutionRefused);
    assert_eq!(
        row.execution_refusal
            .as_ref()
            .map(DecisionReference::as_str),
        Some("stale_spec"),
        "Core refused the moved target"
    );
    assert_eq!(rig.seam.asked().len(), 1, "the seam was asked once");
    assert_eq!(rig.seam.landed(), 0, "nothing was committed");
    assert!(rig.exists("keeper"), "the agent stays");
    rig.shutdown().await;
}

/// A person cannot approve their own request. The chat person who asked
/// decides through `meta/decisions/decide` and is refused `self_approval`,
/// though the approver map lets them decide for the approver id; the
/// decision stays open, and another person of the map approves it. A
/// person who asks is named by their principal key: any other name is
/// refused before anything is held.
///
/// RED before the holder held.
#[tokio::test(flavor = "multi_thread")]
async fn a_person_cannot_approve_their_own_request() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), &fresh_mob_id()).await;
    let asker = human_facts(&chat_person_key(OWNER));
    let (decision_id, _) = pending(&rig.ask_create(&asker, "own-1", &["scout"]).await);
    assert_eq!(
        rig.row(&decision_id).descriptor.requester.as_str(),
        chat_person_key(OWNER),
        "the decision names the person who asked"
    );

    rig.present(&decision_id);
    let own = rig.chat_decides(&decision_id, OWNER, true).await;
    assert_eq!(
        own["error"]["data"]["decision_refusal"]["kind"],
        json!("self_approval"),
        "{own:#?}"
    );
    assert_eq!(rig.row(&decision_id).state.phase, DecisionPhase::Open);

    let other = rig.chat_decides(&decision_id, SECOND_OWNER, true).await;
    assert!(other["error"].is_null(), "{other:#?}");
    assert_eq!(rig.row(&decision_id).state.phase, DecisionPhase::Approved);

    let unnamed = rig
        .ask_create(&human_facts("someone"), "own-2", &["scribe"])
        .await;
    let refusal = unnamed.refusal().expect("refused");
    assert_eq!(
        (refusal.kind_token(), refusal.field.as_deref()),
        ("invalid_request", Some("requester"))
    );
    assert_eq!(rig.count(DecisionPhase::Open), 0, "nothing more was held");
    rig.shutdown().await;
}

/// An application carries a person's decision from its own channel: the
/// decide names the ingress that attests the person and carries what the
/// channel showed them. Without an attestation the decide is nobody's
/// (this host's operator holds no approver id), and a person the map does
/// not list is refused; the decision stays open through both. The attested
/// decide of a listed person, carrying the presentation, approves it with
/// the person's evidence recorded, and the change is carried out.
///
/// RED before the holder held.
#[tokio::test(flavor = "multi_thread")]
async fn a_host_carried_decision_needs_attested_evidence_of_a_listed_person() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), &fresh_mob_id()).await;
    let (decision_id, _) = pending(&rig.ask_create(&host_facts(), "carried-1", &["scout"]).await);
    let digest = rig.row(&decision_id).descriptor.subject_digest.to_string();

    let bare = rig
        .call(
            "meta/decisions/decide",
            json!({"decision_id": decision_id, "approve": true, "presented_digest": digest}),
        )
        .await;
    assert!(
        !bare["error"].is_null(),
        "a decide with no evidence: {bare:#?}"
    );
    assert_eq!(rig.row(&decision_id).state.phase, DecisionPhase::Open);

    let rendering = rig
        .call(
            "meta/decisions/rendering",
            json!({"decision_id": decision_id}),
        )
        .await;
    let carried = |subject: &str, event: &str| {
        let shown_at_ms = now_ms();
        json!({
            "decision_id": decision_id,
            "approve": true,
            "presented_digest": digest,
            "attestation": {
                "ingress": "chat",
                "subject": subject,
                "scope": "chat-session-7",
                "event_ref": event,
                "observed_at_ms": now_ms(),
                "presentation": {
                    "audience": {"kind": "approver", "approver_id": APPROVERS},
                    "subject_digest": digest,
                    "parts_shown": rendering["result"]["parts"],
                    "channel_ref": "chat:owners",
                    "message_ref": format!("message:{event}"),
                    "at_ms": shown_at_ms,
                },
            },
        })
    };
    let stranger = rig
        .call(
            "meta/decisions/decide",
            carried("U999", "chat:event:stranger"),
        )
        .await;
    assert_eq!(
        stranger["error"]["data"]["decision_refusal"]["kind"],
        json!("not_eligible"),
        "{stranger:#?}"
    );
    assert_eq!(rig.row(&decision_id).state.phase, DecisionPhase::Open);

    let listed = rig
        .call("meta/decisions/decide", carried(OWNER, "chat:event:owner"))
        .await;
    assert!(listed["error"].is_null(), "{listed:#?}");
    let row = rig.row(&decision_id);
    assert_eq!(row.state.phase, DecisionPhase::Approved);
    let evidence = serde_json::to_value(&row.outcome.expect("an outcome").evidence).unwrap();
    assert_eq!(evidence["ingress"], json!("chat"), "{evidence:#?}");
    assert_eq!(
        evidence["decider"],
        json!(chat_person_key(OWNER)),
        "{evidence:#?}"
    );

    let mut executors = rig.executor_loop();
    assert_eq!(
        drive(&mut executors).await,
        vec![
            DecisionPassOutcome::Executed,
            DecisionPassOutcome::NoticeSettled
        ]
    );
    assert!(rig.exists("scout"));
    rig.shutdown().await;
}

/// The executor commits as the requester the request records, never from
/// a session: a held change an agent asked for is committed as that agent
/// by its recorded identity, under the decision's permit, and of a target
/// its host configures when the gate says so.
///
/// RED before the holder held.
#[tokio::test(flavor = "multi_thread")]
async fn the_executor_commits_as_the_recorded_requester_under_the_decisions_permit() {
    let temp = tempfile::tempdir().unwrap();
    let seam = ObservedSeam::scripted();
    let rig = boot_with(
        temp.path(),
        &fresh_mob_id(),
        Boot {
            seam: Arc::clone(&seam),
            ..Boot::default()
        },
    )
    .await;
    let agent = RequesterFacts::agent(DECLARED, "worker", BTreeMap::new());
    let digest = Sha256Digest::of(b"the spec the requester read");
    let entry =
        IdentityEntryFacts::new("keeper", digest.clone(), Some(1), "worker", BTreeMap::new());
    rig.host
        .targets
        .lock()
        .unwrap()
        .insert("keeper".to_owned(), HeldTarget::new(entry.clone(), true));
    let normalized = normalize_remove(
        &rig.policy,
        &agent,
        RequestEnvelope::default(),
        &entry,
        &RemoveIdentitySpec::new("keeper", digest),
    )
    .expect("the removal normalizes");
    let operation_id = operation(&agent, "agent-remove-1");
    let (decision_id, _) = pending(
        &rig.decisions
            .hold(&rig.mob(), &operation_id, &normalized)
            .await,
    );
    assert_eq!(seam.asked(), Vec::new(), "a hold asks the seam nothing");
    rig.approve(&decision_id).await;

    let mut executors = rig.executor_loop();
    let pass = executors.pass().await;
    assert_eq!(
        pass.outcomes
            .iter()
            .map(|(_, outcome)| outcome.clone())
            .collect::<Vec<_>>(),
        vec![DecisionPassOutcome::Executed]
    );
    assert_eq!(
        seam.asked(),
        vec![Asked {
            operation_id: operation_id.to_string(),
            requester: ChangeRequester::agent_recorded(DECLARED),
            decided_by: Some(decision_id),
            configured: true,
        }]
    );
    assert_eq!(
        rig.model_requests(),
        0,
        "the requester was not woken to commit"
    );
    rig.shutdown().await;
}

/// An agent that asked is told by a keyed send to its own name: the
/// decision of its request ends, the notice reaches its next turn with the
/// outcome, and it is delivered once however often the loop passes.
///
/// RED before the holder held.
#[tokio::test(flavor = "multi_thread")]
async fn an_agent_requester_is_told_the_outcome_by_a_keyed_send() {
    let temp = tempfile::tempdir().unwrap();
    let mut rig = boot(temp.path(), &fresh_mob_id()).await;
    let runtime = Arc::clone(&rig.runtime);
    let decisions = IdentityDecisions::new(
        rig.runtime.durable_decisions(),
        Arc::clone(&rig.seam) as Arc<dyn IdentityChangeExecutor>,
        Arc::clone(&rig.host) as Arc<dyn HeldChangeHost>,
    )
    .with_notices(Arc::new(KeyedSendNotices::new(move || {
        runtime.identity_runtime()
    })));
    let agent = RequesterFacts::agent(DECLARED, "worker", BTreeMap::new());
    let normalized = normalized_create(&rig.policy, &agent, &[spec("scout")]);
    let held = decisions
        .hold(&rig.mob(), &operation(&agent, "told-1"), &normalized)
        .await;
    let (decision_id, _) = pending(&held);
    assert_eq!(rig.model_requests(), 0, "holding wakes nobody");

    rig.present(&decision_id);
    let answer = rig.chat_decides(&decision_id, OWNER, false).await;
    assert!(answer["error"].is_null(), "{answer:#?}");
    let mut executors = decisions
        .executor_loop(rig.runtime.decision_approver_currentness())
        .expect("the loop starts");
    let deadline = std::time::Instant::now() + BOUND;
    loop {
        let pass = executors.pass().await;
        if pass
            .outcomes
            .iter()
            .any(|(_, outcome)| *outcome == DecisionPassOutcome::NoticeSettled)
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the notice is admitted: {pass:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        rig.asked("was not carried out (decision_closed)").await,
        "the requesting agent reads the outcome in its next turn"
    );
    assert_eq!(drive(&mut executors).await, Vec::new());
    assert_eq!(
        rig.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|request| request.contains("was not carried out (decision_closed)"))
            .count(),
        1,
        "the notice is delivered once"
    );
    rig.shutdown().await;
}

/// The driver makes one pass when it starts, which reads decision records
/// only (no member's model is asked anything), and no other while the host
/// has no decision of its kinds. A held change makes it pass again, and it
/// ticks while the decision waits; once a person decided through
/// `meta/decisions/decide` the change is carried out with no pass made by
/// hand, and the driver falls quiet again.
///
/// RED before the holder held.
#[tokio::test(flavor = "multi_thread")]
async fn the_driver_is_quiet_on_an_idle_host_and_carries_out_a_decided_change() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), &fresh_mob_id()).await;
    let driver = rig
        .decisions
        .spawn_driver(
            rig.runtime.decision_approver_currentness(),
            Duration::from_millis(25),
        )
        .expect("the driver starts")
        .expect("a host that acts drives its executors");
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(driver.passes(), 1, "an idle host makes its first pass only");
    assert_eq!(rig.model_requests(), 0, "the first pass wakes nobody");

    let (decision_id, _) = pending(&rig.ask_create(&host_facts(), "driven-1", &["scout"]).await);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let while_open = driver.passes();
    assert!(
        while_open >= 3,
        "the driver passes while a decision waits for its deadline: {while_open}"
    );
    assert_eq!(rig.model_requests(), 0, "its passes wake nobody");

    rig.present(&decision_id);
    let answer = rig.chat_decides(&decision_id, OWNER, true).await;
    assert!(answer["error"].is_null(), "{answer:#?}");
    let deadline = std::time::Instant::now() + BOUND;
    while rig.notices.all().is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "the driver carries the change out and tells the requester"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(rig.exists("scout"));
    assert_eq!(rig.seam.landed(), 1);

    // With nothing held the driver falls quiet: its count of passes stops
    // growing (a loaded host may take a few passes to get there).
    let deadline = std::time::Instant::now() + BOUND;
    loop {
        let settled = driver.passes();
        tokio::time::sleep(Duration::from_millis(400)).await;
        if driver.passes() == settled {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "with nothing held the driver passes no more: {} passes",
            driver.passes()
        );
    }
    driver.shutdown().await;
    rig.shutdown().await;
}

/// A decision of an `identity.*` kind that the holder did not write, whose
/// body is another request than the one its digest binds, is never carried
/// out: once approved, the executor refuses it `unreadable_request` without
/// asking the seam, and neither request's agent exists.
#[tokio::test(flavor = "multi_thread")]
async fn a_decision_whose_body_is_not_the_request_it_binds_is_never_carried_out() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), &fresh_mob_id()).await;
    let facts = host_facts();
    let bound = normalized_create(&rig.policy, &facts, &[spec("scout")]).request;
    let carried = normalized_create(&rig.policy, &facts, &[spec("intruder")]).request;
    let operation_id = operation(&facts, "forged-1");
    let mob_id = rig.mob().mob_id().as_str().to_owned();
    let written = rig
        .owner()
        .hold(
            DecisionHoldRequest::new(
                DecisionKind::new("identity.create").expect("kind"),
                DecisionSubjectKey::new(format!("{mob_id}/{operation_id}")).expect("subject"),
                DecisionOperationId::new(operation_id.as_str()).expect("operation"),
                meerkat_core::approval::decision::Sha256Digest::new(
                    bound.request_digest().unwrap().as_str(),
                )
                .expect("digest"),
                DecisionRequestBody::from_value(&json!({
                    "format": meerkat_meta::identity_creation::HELD_CHANGE_FORMAT,
                    "mob_id": mob_id,
                    "request": carried,
                }))
                .expect("body"),
                DecisionPrincipalKey::new(chat_person_key("U900")).expect("requester"),
                ApproverId::new(APPROVERS).expect("approver"),
                now_ms() + 60 * 60 * 1000,
                DecisionRendering::new("Identity request: render-v1\n").expect("rendering"),
                DecisionRenderer::new("identity.render-v1").expect("renderer"),
            ),
            false,
        )
        .expect("a decision written past the holder");
    let decision_id = written.decision_id.to_string();
    assert!(
        HeldIdentityChange::of(&written).is_err(),
        "the body is not the request the decision binds"
    );
    rig.approve(&decision_id).await;

    let mut executors = rig.executor_loop();
    assert_eq!(
        drive(&mut executors).await,
        vec![DecisionPassOutcome::ExecutionRefused]
    );
    assert_eq!(
        rig.row(&decision_id)
            .execution_refusal
            .as_ref()
            .map(DecisionReference::as_str),
        Some("unreadable_request")
    );
    assert_eq!(rig.seam.asked(), Vec::new(), "the seam was not asked");
    assert!(!rig.exists("scout") && !rig.exists("intruder"));
    rig.shutdown().await;
}

/// One request id names one change. A create is held under a request id;
/// an update and a removal asked under the same id are another request and
/// are refused `operation_reused`, and one decision exists.
///
/// RED before the holder looked a held operation up across its kinds: the
/// update was held as a second decision.
#[tokio::test(flavor = "multi_thread")]
async fn another_change_under_a_held_request_id_is_operation_reused() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), &fresh_mob_id()).await;
    let facts = host_facts();
    let held = rig.ask_create(&facts, "one-1", &["scout"]).await;
    pending(&held);
    let digest = Sha256Digest::of(b"the spec the requester read");
    let entry =
        IdentityEntryFacts::new("keeper", digest.clone(), Some(1), "worker", BTreeMap::new());
    rig.host
        .targets
        .lock()
        .unwrap()
        .insert("keeper".to_owned(), HeldTarget::new(entry.clone(), false));
    let operation_id = operation(&facts, "one-1");

    let update = normalized_update(&rig.policy, &facts, &entry, None);
    let updated = rig.decisions.hold(&rig.mob(), &operation_id, &update).await;
    assert_eq!(refused(&updated), "operation_reused");
    let removal = normalize_remove(
        &rig.policy,
        &facts,
        RequestEnvelope::default(),
        &entry,
        &RemoveIdentitySpec::new("keeper", digest),
    )
    .expect("the removal normalizes");
    let removed = rig
        .decisions
        .hold(&rig.mob(), &operation_id, &removal)
        .await;
    assert_eq!(refused(&removed), "operation_reused");

    assert_eq!(
        rig.count(DecisionPhase::Open),
        1,
        "one decision holds the id"
    );
    assert_eq!(
        rig.decisions.held(&rig.mob(), &operation_id).await.unwrap(),
        Some(held),
        "the id still answers the create"
    );
    rig.shutdown().await;
}

/// A person a host names as the proposer of a request cannot approve it.
/// The host asks on its own surface and names the chat person as proposer
/// by their principal key: the decision records that person as its
/// requester, their decide is refused `self_approval`, and another person
/// of the map approves. A request with no proposer is the host's: the same
/// person approves it.
///
/// RED before the holder read the proposer: the decision recorded the host,
/// and the proposer approved their own request.
#[tokio::test(flavor = "multi_thread")]
async fn a_person_a_host_names_as_proposer_cannot_approve_the_request() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), &fresh_mob_id()).await;
    let facts = host_facts();
    let mut envelope = RequestEnvelope::default();
    envelope.proposer = Some(chat_person_key(OWNER));
    let proposed = normalized_create_with(&rig.policy, &facts, &[spec("scout")], envelope);
    let held = rig
        .decisions
        .hold(&rig.mob(), &operation(&facts, "proposed-1"), &proposed)
        .await;
    let (decision_id, _) = pending(&held);
    assert_eq!(
        rig.row(&decision_id).descriptor.requester.as_str(),
        chat_person_key(OWNER),
        "the decision names the person the host named"
    );
    rig.present(&decision_id);
    let own = rig.chat_decides(&decision_id, OWNER, true).await;
    assert_eq!(
        own["error"]["data"]["decision_refusal"]["kind"],
        json!("self_approval"),
        "{own:#?}"
    );
    assert_eq!(rig.row(&decision_id).state.phase, DecisionPhase::Open);
    let other = rig.chat_decides(&decision_id, SECOND_OWNER, true).await;
    assert!(other["error"].is_null(), "{other:#?}");

    let (hosts, _) = pending(&rig.ask_create(&facts, "hosts-1", &["scribe"]).await);
    assert_ne!(
        rig.row(&hosts).descriptor.requester.as_str(),
        chat_person_key(OWNER),
        "a request with no proposer is the host's"
    );
    rig.present(&hosts);
    let approved = rig.chat_decides(&hosts, OWNER, true).await;
    assert!(approved["error"].is_null(), "{approved:#?}");
    rig.shutdown().await;
}

/// A held role change shows the tools of the profile the agent moves TO,
/// and an update that keeps the profile shows the agent's own.
#[tokio::test(flavor = "multi_thread")]
async fn a_held_role_change_shows_the_new_profiles_tools() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), &fresh_mob_id()).await;
    let facts = host_facts();
    let digest = Sha256Digest::of(b"the spec the requester read");
    let entry = IdentityEntryFacts::new("keeper", digest, Some(1), "worker", BTreeMap::new());
    rig.host
        .targets
        .lock()
        .unwrap()
        .insert("keeper".to_owned(), HeldTarget::new(entry.clone(), false));

    let moved = normalized_update(&rig.policy, &facts, &entry, Some("lead"));
    let held = rig
        .decisions
        .hold(&rig.mob(), &operation(&facts, "role-1"), &moved)
        .await;
    let (decision_id, _) = pending(&held);
    let row = rig.row(&decision_id);
    assert_eq!(row.descriptor.kind.as_str(), "identity.update");
    let rendering = row.descriptor.rendering.to_string();
    assert!(
        rendering.contains("\"keeper\" profile \"lead\""),
        "the agent is shown on the profile it moves to:\n{rendering}"
    );
    assert!(
        rendering.contains("Tool families (2):\n- comms\n- mob\n"),
        "with that profile's tools:\n{rendering}"
    );
    assert!(
        !rendering.contains("profile: \"worker\"\n"),
        "and no block of the profile it leaves:\n{rendering}"
    );

    let kept = normalized_update(&rig.policy, &facts, &entry, None);
    let held = rig
        .decisions
        .hold(&rig.mob(), &operation(&facts, "brief-1"), &kept)
        .await;
    let (decision_id, _) = pending(&held);
    let rendering = rig.row(&decision_id).descriptor.rendering.to_string();
    assert!(
        rendering.contains("\"keeper\" profile \"worker\""),
        "an update that keeps the profile shows the agent's own:\n{rendering}"
    );
    rig.shutdown().await;
}

/// The agent a held change names moved to another profile after the
/// approval: its tools are no longer the ones the person was shown, and the
/// change is refused `subject_widened` without asking the seam.
#[tokio::test(flavor = "multi_thread")]
async fn a_target_whose_profile_moved_since_the_hold_is_refused_subject_widened() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), &fresh_mob_id()).await;
    let facts = host_facts();
    let digest = Sha256Digest::of(b"the spec the requester read");
    let entry =
        IdentityEntryFacts::new("keeper", digest.clone(), Some(1), "worker", BTreeMap::new());
    rig.host
        .targets
        .lock()
        .unwrap()
        .insert("keeper".to_owned(), HeldTarget::new(entry.clone(), false));
    let removal = normalize_remove(
        &rig.policy,
        &facts,
        RequestEnvelope::default(),
        &entry,
        &RemoveIdentitySpec::new("keeper", digest.clone()),
    )
    .expect("the removal normalizes");
    let held = rig
        .decisions
        .hold(&rig.mob(), &operation(&facts, "moved-1"), &removal)
        .await;
    let (decision_id, _) = pending(&held);
    rig.approve(&decision_id).await;

    let moved = IdentityEntryFacts::new("keeper", digest, Some(2), "lead", BTreeMap::new());
    rig.host
        .targets
        .lock()
        .unwrap()
        .insert("keeper".to_owned(), HeldTarget::new(moved, false));
    let mut executors = rig.executor_loop();
    assert_eq!(
        drive(&mut executors).await,
        vec![
            DecisionPassOutcome::ExecutionRefused,
            DecisionPassOutcome::NoticeSettled
        ]
    );
    assert_eq!(
        rig.row(&decision_id)
            .execution_refusal
            .as_ref()
            .map(DecisionReference::as_str),
        Some("subject_widened")
    );
    assert_eq!(rig.seam.asked(), Vec::new(), "the seam was not asked");
    rig.shutdown().await;
}

/// A candidate launch holds nothing and carries nothing out: its holder
/// refuses a change that needs approval `candidate_launch`, and starts no
/// driver. A decision held by a host that acts is carried out by a host
/// that acts.
///
/// RED before the holder held: the candidate's refusal was
/// `approval_unavailable`.
#[tokio::test(flavor = "multi_thread")]
async fn a_candidate_launch_holds_nothing_and_drives_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let rig = boot(temp.path(), &fresh_mob_id()).await;
    let candidate = RuntimeDecisions::in_state_dir(
        temp.path(),
        rig.runtime
            .durable_decisions()
            .realm()
            .expect("a realm")
            .clone(),
        chat_declared(),
        Arc::new(InMemoryMetadataStore::default()),
        rig.mob().mob_id().as_str(),
        false,
    );
    let decisions = holder(&candidate, &rig.seam, &rig.host, &rig.notices, &rig.lines);
    let facts = host_facts();
    let normalized = normalized_create(&rig.policy, &facts, &[spec("scout")]);
    let outcome = decisions
        .hold(&rig.mob(), &operation(&facts, "candidate-1"), &normalized)
        .await;
    assert_eq!(refused(&outcome), "candidate_launch");
    assert_eq!(rig.count(DecisionPhase::Open), 0, "nothing was held");
    assert!(
        decisions
            .spawn_driver(
                rig.runtime.decision_approver_currentness(),
                Duration::from_millis(25)
            )
            .expect("a candidate launch is no failure")
            .is_none(),
        "a candidate launch drives no executor"
    );
    rig.shutdown().await;
}

/// A profile as a mob's definition holds one, with `secret` in the
/// configuration of its own MCP server.
fn declared_profile(secret: &str) -> meerkat_mob::Profile {
    serde_json::from_value(json!({
        "model": "gpt-5.5",
        "skills": ["ledger-keeping"],
        "tools": {
            "comms": true,
            "shell": true,
            "read_only": false,
            "deny": ["mob_wire"],
            "mcp_servers": [{
                "name": "ledger",
                "command": "ledger-server",
                "args": ["--token", secret],
                "env": {"LEDGER_TOKEN": secret},
            }],
            "grant_gated": {"families": ["shell"]},
        },
    }))
    .expect("a profile")
}

/// The definition the pure rows declare profiles of: the rows' own, with
/// the text of the skill the profile names.
fn declaring_definition() -> MobDefinition {
    let mut definition = definition("declared", TOOLS);
    definition.skills.insert(
        "ledger-keeping".to_owned(),
        meerkat_mob::definition::SkillSource::Inline {
            content: "SKILL-TEXT keep the ledger balanced".to_owned(),
        },
    );
    definition
}

/// The rendering shows what the mob declares for a profile: its model, its
/// auth binding, the names of its skills and its tool configuration. It
/// never shows the configuration of an MCP server (a credential-shaped
/// value in a server's environment is absent, the server is shown by its
/// name) nor a skill's text. A profile that names no server of the host
/// reaches every server the host offers, and the rendering says so; one
/// that names servers shows them. The digest covers the whole declaration:
/// the server's configuration, the model, the text of a skill the profile
/// names and the wiring rules that name the profile.
#[test]
fn the_rendering_names_an_mcp_server_and_never_shows_its_configuration() {
    let secret = "tok-CREDENTIAL-SHAPED-9f3a71";
    let definition = declaring_definition();
    let declare = |profile: &meerkat_mob::Profile, definition: &MobDefinition| {
        ProfileTools::of_profile("worker", profile, definition, None).expect("declared")
    };
    let declared = declare(&declared_profile(secret), &definition);
    let request =
        normalized_create(&approval_policy(1000), &host_facts(), &[spec("scout")]).request;
    let rendered = |declared: &ProfileTools| {
        let configurations = ToolConfigurations::of_request(
            &request,
            vec![AgentTools::new("scout", "worker")],
            BTreeMap::from([("worker".to_owned(), declared.clone())]),
        )
        .expect("the profiles of the request");
        render_held_v1(&request, None, &configurations).expect("renders")
    };
    let rendering = rendered(&declared);
    let text = rendering.as_str();

    assert!(!text.contains(secret), "no credential is rendered:\n{text}");
    assert!(!text.contains("LEDGER_TOKEN"), "{text}");
    assert!(!text.contains("ledger-server"), "{text}");
    assert!(
        !text.contains("SKILL-TEXT"),
        "no skill text is rendered:\n{text}"
    );
    for shown in [
        "Agent profiles (1):\n- \"scout\" profile \"worker\" declares sha256:",
        "Declaration of profile: \"worker\"\n",
        "Model: \"gpt-5.5\"\n",
        "Auth binding: none\n",
        "Skills (1):\n- \"ledger-keeping\"\n",
        "Tool families (2):\n- shell\n- comms\n",
        "Read only: no\n",
        "Denied tools (1):\n- \"mob\\_wire\"\n",
        "Host MCP servers: every server the host offers\n",
        "Profile MCP servers (1):\n- \"ledger\"\n",
        "Grant gated (1):\n- family \"shell\"\n",
    ] {
        assert!(
            text.contains(shown),
            "the rendering shows {shown:?}:\n{text}"
        );
    }
    assert!(
        text.contains(&format!("Profile digest: {}\n", declared.digest().as_str())),
        "{text}"
    );

    // A profile that names servers of the host shows them, and not "every
    // server".
    let mut narrowed = declared_profile(secret);
    narrowed.tools.mcp = vec!["search".to_owned()];
    let narrowed = rendered(&declare(&narrowed, &definition));
    assert!(
        narrowed
            .as_str()
            .contains("Host MCP servers (1):\n- \"search\"\n"),
        "{}",
        narrowed.as_str()
    );
    assert!(!narrowed.as_str().contains("every server the host offers"));
    narrowed.card().expect("a structured card");

    // The digest covers the whole declaration.
    let mut other_model = declared_profile(secret);
    other_model.model = "gpt-5.4".to_owned();
    let mut other_skill = declaring_definition();
    other_skill.skills.insert(
        "ledger-keeping".to_owned(),
        meerkat_mob::definition::SkillSource::Inline {
            content: "SKILL-TEXT move the ledger offshore".to_owned(),
        },
    );
    let mut other_wiring = declaring_definition();
    other_wiring
        .wiring
        .role_wiring
        .push(meerkat_mob::definition::RoleWiringRule {
            a: ProfileName::from("worker"),
            b: ProfileName::from("lead"),
        });
    let mut unrelated_wiring = declaring_definition();
    unrelated_wiring
        .wiring
        .role_wiring
        .push(meerkat_mob::definition::RoleWiringRule {
            a: ProfileName::from("lead"),
            b: ProfileName::from("lead"),
        });
    for (what, other) in [
        (
            "the server's configuration",
            declare(&declared_profile("tok-ANOTHER-VALUE"), &definition),
        ),
        ("the model", declare(&other_model, &definition)),
        (
            "the text of a skill it names",
            declare(&declared_profile(secret), &other_skill),
        ),
        (
            "a wiring rule that names it",
            declare(&declared_profile(secret), &other_wiring),
        ),
    ] {
        assert_ne!(
            declared.digest(),
            other.digest(),
            "the digest covers {what}"
        );
    }
    assert_eq!(
        declared.digest(),
        declare(&declared_profile(secret), &unrelated_wiring).digest(),
        "a wiring rule that names other profiles is not this profile's"
    );

    // A profile bound to a realm profile is declared by the realm's
    // profile, read now, and is refused when nobody can read it.
    let bound = meerkat_mob::ProfileBinding::RealmRef {
        realm_profile: "shared-keeper".to_owned(),
    };
    let resolved = ProfileTools::declared(
        "worker",
        &bound,
        &definition,
        Some(&declared_profile(secret)),
    )
    .expect("a realm profile the host reads");
    let resolved_text = rendered(&resolved);
    for shown in [
        "Realm profile: \"shared-keeper\"\n",
        "Tool families (2):\n- shell\n- comms\n",
        "Profile MCP servers (1):\n- \"ledger\"\n",
    ] {
        assert!(
            resolved_text.as_str().contains(shown),
            "a realm profile shows {shown:?}:\n{}",
            resolved_text.as_str()
        );
    }
    assert_ne!(
        resolved.digest(),
        ProfileTools::declared(
            "worker",
            &bound,
            &definition,
            Some(&declared_profile("tok-ANOTHER-VALUE")),
        )
        .expect("declared")
        .digest(),
        "the digest covers the realm profile's content"
    );
    let unreadable = ProfileTools::declared("worker", &bound, &definition, None)
        .expect_err("a realm profile nobody can read");
    assert_eq!(unreadable.kind_token(), "unsupported");
    rendering
        .card()
        .expect("the rendering parses as a structured card");

    let unknown = ToolConfigurations::of_request(
        &request,
        vec![AgentTools::new("scout", "worker")],
        BTreeMap::new(),
    )
    .expect_err("a profile with no declaration");
    assert_eq!(unknown.kind_token(), "profile_unknown");

    // Every change of one agent shows the tools of the agent it names.
    let digest = Sha256Digest::of(b"the spec the requester read");
    let entry =
        IdentityEntryFacts::new("keeper", digest.clone(), Some(1), "worker", BTreeMap::new());
    let policy = approval_policy(1000);
    let envelope = RequestEnvelope::default;
    let facts = host_facts();
    let whole = [
        normalize_remove(
            &policy,
            &facts,
            envelope(),
            &entry,
            &RemoveIdentitySpec::new("keeper", digest.clone()),
        ),
        normalize_archive(
            &policy,
            &facts,
            envelope(),
            &entry,
            &ArchiveIdentitySpec::new("keeper", digest.clone()),
        ),
        normalize_unarchive(
            &policy,
            &facts,
            envelope(),
            &entry,
            &UnarchiveIdentitySpec::new("keeper", digest.clone()),
        ),
        normalize_adopt(
            &policy,
            &facts,
            envelope(),
            &entry,
            &AdoptIdentitySpec::new("keeper", digest),
        ),
    ];
    let updates = [
        normalized_update(&policy, &facts, &entry, None),
        normalized_update(&policy, &facts, &entry, Some("lead")),
    ];
    for normalized in whole.into_iter().chain(updates.into_iter().map(Ok)) {
        let request = normalized.expect("the change normalizes").request;
        let configurations = ToolConfigurations::of_request(
            &request,
            vec![AgentTools::new("keeper", "worker")],
            BTreeMap::from([("worker".to_owned(), declared.clone())]),
        )
        .expect("the profiles of the request");
        let rendering = render_held_v1(&request, None, &configurations).expect("renders");
        assert!(
            rendering
                .as_str()
                .contains("Agent profiles (1):\n- \"keeper\" profile \"worker\" declares sha256:"),
            "a held {} shows the agent's tools:\n{}",
            request.action.token(),
            rendering.as_str()
        );
        rendering.card().expect("a structured card");
    }
}
