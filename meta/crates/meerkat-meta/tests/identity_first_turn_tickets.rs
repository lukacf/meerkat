#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::redundant_clone
)]
//! Per-admission completion: a delivery tracked by a [`TurnTicket`] reports
//! THAT turn and its own output, however much other traffic reaches the
//! identity.
//!
//! The defect: `send_and_wait` / `wait_for_output(after=...)` waited until
//! the identity-wide completion cursor passed the caller's pre-delivery
//! baseline, then returned the session's latest `output_preview`. A
//! concurrent delivery (a peer message, a scheduled turn, a fork completion
//! wake) completing first satisfied the wait, and the caller got someone
//! else's output.
//!
//! The gateway RPC waits over a scripted ticket bridge live in-crate
//! (`src/rpc/turn_ticket_rpc_tests.rs`): they use pub(crate) view
//! constructors (D67).
//!
//! The first half drives [`IdentityRuntime`] over a scripted bridge whose
//! tracked turns complete only when a test finishes them, in admission order.
//! The second half drives the gateway RPC over a real runtime whose LLM
//! replies to each message by name, for both runtime modes.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use meerkat_client::{LlmError, LlmEvent, LlmRequest};
use meerkat_core::types::{HandlingMode, StopReason};
use meerkat_meta::identity_first::contracts::{ContinuityStore, LeaseProvider};
use meerkat_meta::identity_first::{
    AgentAddressability, AgentBuildDraft, AgentIdentity, AgentRuntimeId, BridgeAdmissionError,
    BridgeDelivery, BridgeError, BridgeTurnReceipt, CompletionProgress, CorrelationId,
    DeliveryErrorClass, DispatchIdempotencyKey, DispatchInput, DispatchOrigin, DurabilityPolicy,
    DurableAgentSpec, IdentityLifecycleState, IdentityRuntime, IdentityRuntimeConfig,
    LocalContinuityStore, LocalLeaseProvider, MemberInspection, MutableRosterProvider,
    ResumeSessionOutcome, SessionBridge, SessionSnapshot, TurnOutcome, TurnOutput, TurnTicket,
    TurnTracking, TurnUntrackable,
};
use serde_json::{Value, json};

#[path = "support/core_members.rs"]
mod core_members;
#[path = "support/llm_usage.rs"]
mod llm_usage;

use core_members::CoreMembers;

// ===========================================================================
// Scripted bridge
// ===========================================================================

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
    /// Every member build (`create_session`) or resume (`resume_session`).
    materializations: AtomicUsize,
    /// The members Core holds, as this double's creates, resumes and retires
    /// leave them (or as a test bound them).
    core: CoreMembers,
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
            materializations: AtomicUsize::new(0),
            core: CoreMembers::default(),
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

    fn ingress_admissions(&self) -> Vec<Admission> {
        self.ingress.lock().unwrap().clone()
    }

    fn tracked_admissions(&self) -> Vec<Admission> {
        self.tracked.lock().unwrap().clone()
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

    /// Commit `text` as the latest session output without finishing any
    /// tracked turn: an untracked (foreign) turn finishing.
    fn commit_foreign_output(&self, text: &str) {
        *self.latest_output.lock().unwrap() = Some(text.to_string());
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
    async fn create_session(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        _draft: &AgentBuildDraft,
    ) -> Result<meerkat_meta::identity_first::MemberRuntimeBinding, BridgeError> {
        let agent_runtime_id = AgentRuntimeId::parse(&format!("rt:{identity}:0"))
            .map_err(|error| BridgeError::Mob(error.to_string()))?;
        let session_id = &meerkat_core::types::SessionId::new();
        self.materializations.fetch_add(1, Ordering::SeqCst);
        let binding = meerkat_meta::identity_first::MemberRuntimeBinding {
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
        let agent_runtime_id = AgentRuntimeId::parse(&format!("rt:{identity}:0"))
            .map_err(|error| BridgeError::Mob(error.to_string()))?;
        self.materializations.fetch_add(1, Ordering::SeqCst);
        self.core.bind(
            identity,
            &meerkat_meta::identity_first::MemberRuntimeBinding {
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
    ) -> Result<Option<meerkat_meta::identity_first::MemberRuntimeBinding>, BridgeError> {
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
    ) -> Result<Option<meerkat_meta::identity_first::MemberRuntimeBinding>, BridgeError> {
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

    fn tracks_turn_output(&self) -> bool {
        self.tracks
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

    async fn checkpoint_session(
        &self,
        _runtime_id: &AgentRuntimeId,
        _session_id: &meerkat_core::types::SessionId,
    ) -> Result<SessionSnapshot, BridgeError> {
        Ok(SessionSnapshot { data: Vec::new() })
    }

    async fn retire_member(&self, runtime_id: &AgentRuntimeId) -> Result<(), BridgeError> {
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

/// Register an Active identity bound to `session`, the session the bridge
/// resolves every turn onto (so no delivery looks like a rotation): Core
/// holds its member live on that session, at fence 1. Without a bridge only
/// the spec is registered.
async fn register_spec(
    runtime: &IdentityRuntime,
    bridge: Option<&TicketBridge>,
    spec: DurableAgentSpec,
    session: &meerkat_core::types::SessionId,
) -> AgentIdentity {
    let identity = spec.identity.clone();
    if let Some(bridge) = bridge {
        bridge
            .core
            .bind_live_at_fence(&identity, &spec.profile, 0, Some(session.clone()), 1);
    }
    runtime.register(spec).await;
    identity
}

async fn register_bound(
    runtime: &IdentityRuntime,
    bridge: Option<&TicketBridge>,
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

fn unknown_ticket() -> TurnTicket {
    TurnTicket::parse(&uuid::Uuid::new_v4().to_string()).unwrap()
}

fn correlated(text: &str, key: &str, correlation: &str) -> DispatchInput {
    DispatchInput {
        content: content(text),
        origin: DispatchOrigin::Connector,
        correlation_id: Some(CorrelationId::new(correlation)),
        idempotency_key: Some(DispatchIdempotencyKey::new(key)),
    }
}

/// What the SDKs' old `send_and_wait` did: wait until the identity-wide
/// cursor passes `baseline`, then return the session's latest output.
async fn old_cursor_wait(
    runtime: &IdentityRuntime,
    identity: &AgentIdentity,
    baseline: meerkat_meta::identity_first::CompletionCursor,
) -> Option<String> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let cursor = runtime.completion_cursor(identity).await;
            let inspection = runtime.inspect(identity).await.expect("inspect");
            if cursor.progress_since(baseline) == CompletionProgress::Completed {
                return inspection.output_preview;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the cursor passes the baseline")
}

/// Two tracked sends and one untracked (foreign) delivery to ONE identity,
/// completing foreign first, then B, then A, each with distinct output. Each
/// ticket reports its own turn's output. The foreign completion moves the
/// identity-wide cursor past A's baseline, and the old cursor wait returns
/// the foreign output for A; A's ticket stays pending until A finishes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_deliveries_to_one_identity_each_get_their_own_output() {
    let session = meerkat_core::types::SessionId::new();
    let bridge = TicketBridge::new(true, session.clone());
    let runtime = make_runtime(Some(bridge.clone()));
    let keeper = register_bound(&runtime, Some(&bridge), "keeper", &session).await;

    let a = runtime
        .send_with_turn_ticket(&keeper, None, &content("alpha"), HandlingMode::Queue, None)
        .await
        .expect("send A");
    let b = runtime
        .send_with_turn_ticket(&keeper, None, &content("beta"), HandlingMode::Queue, None)
        .await
        .expect("send B");
    runtime
        .send_admission_tracked(
            &keeper,
            None,
            &content("foreign"),
            HandlingMode::Queue,
            None,
        )
        .await
        .expect("foreign send");
    let (ticket_a, ticket_b) = (tracked(&a.turn), tracked(&b.turn));
    assert_ne!(ticket_a, ticket_b);
    assert_eq!(bridge.tracked_admissions().len(), 2);
    assert_eq!(bridge.ingress_admissions().len(), 1);
    assert_eq!(
        runtime.turn_outcome(&keeper, ticket_a),
        TurnOutcome::Pending
    );

    // The foreign turn completes first.
    bridge.commit_foreign_output("foreign says hi");
    runtime.record_turn_completed(&keeper).await;
    assert_eq!(
        old_cursor_wait(&runtime, &keeper, a.admission.completion_baseline)
            .await
            .as_deref(),
        Some("foreign says hi"),
        "control: the identity-wide cursor wait is satisfied by the foreign turn"
    );
    assert_eq!(
        runtime.turn_outcome(&keeper, ticket_a),
        TurnOutcome::Pending,
        "a foreign completion must never satisfy A's ticket"
    );

    bridge.finish(1, Ok("B says hi"));
    runtime.record_turn_completed(&keeper).await;
    bridge.finish(0, Ok("A says hi"));
    runtime.record_turn_completed(&keeper).await;

    assert_eq!(
        runtime
            .wait_for_turn(&keeper, ticket_a, Duration::from_secs(5))
            .await,
        completed("A says hi")
    );
    assert_eq!(
        runtime
            .wait_for_turn(&keeper, ticket_b, Duration::from_secs(5))
            .await,
        completed("B says hi")
    );
}

/// A tracked turn that fails is `Failed` with its reason; a ticket nobody
/// admitted, or one read for another identity, is `Unknown`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_turn_is_failed_and_an_unknown_ticket_is_unknown() {
    let session = meerkat_core::types::SessionId::new();
    let bridge = TicketBridge::new(true, session.clone());
    let runtime = make_runtime(Some(bridge.clone()));
    let keeper = register_bound(&runtime, Some(&bridge), "keeper", &session).await;
    let other = register_bound(&runtime, Some(&bridge), "other", &session).await;

    let a = runtime
        .send_with_turn_ticket(&keeper, None, &content("alpha"), HandlingMode::Queue, None)
        .await
        .expect("send A");
    let ticket = tracked(&a.turn);
    bridge.finish(0, Err("the model refused"));
    match runtime
        .wait_for_turn(&keeper, ticket, Duration::from_secs(5))
        .await
    {
        TurnOutcome::Failed { reason } => assert!(reason.contains("the model refused"), "{reason}"),
        other => panic!("expected Failed, got {other:?}"),
    }
    assert_eq!(
        runtime.turn_outcome(&other, ticket),
        TurnOutcome::Unknown,
        "a ticket belongs to the identity it was admitted for"
    );
    assert_eq!(
        runtime
            .wait_for_turn(&keeper, unknown_ticket(), Duration::from_secs(5))
            .await,
        TurnOutcome::Unknown
    );
}

/// A bridge that cannot report per-turn output: the send is delivered once,
/// on the ingress lane, and reported as untracked with a typed reason.
#[tokio::test]
async fn an_untrackable_turn_is_delivered_once_on_the_ingress_lane() {
    let session = meerkat_core::types::SessionId::new();
    let bridge = TicketBridge::new(false, session.clone());
    let runtime = make_runtime(Some(bridge.clone()));
    let keeper = register_bound(&runtime, Some(&bridge), "keeper", &session).await;

    let sent = runtime
        .send_with_turn_ticket(&keeper, None, &content("alpha"), HandlingMode::Queue, None)
        .await
        .expect("send");
    assert_eq!(
        sent.turn,
        TurnTracking::Unavailable(TurnUntrackable::BridgeCannotReportOutput)
    );
    assert_eq!(bridge.ingress_admissions().len(), 1);
    assert!(bridge.tracked_admissions().is_empty());

    // No bridge at all: Core's mob is out of reach, so the send is refused
    // typed and nothing is admitted or tracked.
    let bare = make_runtime(None);
    let lone = register_bound(&bare, None, "lone", &session).await;
    let refused = bare
        .send_with_turn_ticket(&lone, None, &content("alpha"), HandlingMode::Queue, None)
        .await
        .expect_err("a send without a session bridge is refused");
    assert!(
        matches!(
            &refused,
            meerkat_meta::identity_first::IdentityRuntimeError::Internal(detail)
                if detail.contains("no session bridge is installed")
        ),
        "{refused:?}"
    );
}

/// The caller's interaction id rides into the admission unchanged but never
/// names the ticket: two sends reusing one interaction id get distinct
/// tickets, and each reports its own turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reused_interaction_id_never_shares_a_ticket() {
    let session = meerkat_core::types::SessionId::new();
    let bridge = TicketBridge::new(true, session.clone());
    let runtime = make_runtime(Some(bridge.clone()));
    let keeper = register_bound(&runtime, Some(&bridge), "keeper", &session).await;

    let interaction = uuid::Uuid::new_v4().to_string();
    let mut tickets = Vec::new();
    for text in ["alpha", "beta"] {
        let sent = runtime
            .send_with_turn_ticket(
                &keeper,
                None,
                &content(text),
                HandlingMode::Queue,
                Some(&interaction),
            )
            .await
            .expect("send");
        tickets.push(tracked(&sent.turn));
    }
    assert_ne!(tickets[0], tickets[1]);
    assert!(
        tickets
            .iter()
            .all(|ticket| ticket.to_string() != interaction)
    );
    assert!(
        bridge
            .tracked_admissions()
            .iter()
            .all(|admission| admission.interaction_id.as_deref() == Some(interaction.as_str())),
        "the caller's interaction id rides unchanged"
    );

    bridge.finish(1, Ok("beta reply"));
    assert_eq!(
        runtime
            .wait_for_turn(&keeper, tickets[1], Duration::from_secs(5))
            .await,
        completed("beta reply")
    );
    assert_eq!(
        runtime.turn_outcome(&keeper, tickets[0]),
        TurnOutcome::Pending
    );
    bridge.finish(0, Ok("alpha reply"));
    assert_eq!(
        runtime
            .wait_for_turn(&keeper, tickets[0], Duration::from_secs(5))
            .await,
        completed("alpha reply")
    );

    // A non-UUID interaction id no longer prevents tracking.
    let sent = runtime
        .send_with_turn_ticket(
            &keeper,
            None,
            &content("gamma"),
            HandlingMode::Queue,
            Some("not-a-uuid"),
        )
        .await
        .expect("send");
    tracked(&sent.turn);
}

/// One correlation fanned out to two identities (per-target idempotency
/// keys): two distinct turns, two distinct tickets. The second identity's
/// first look is `Pending`, never `Unknown`, and neither ticket can report
/// the other identity's output, whichever finishes first.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fanned_out_correlation_gets_a_ticket_per_identity() {
    let session = meerkat_core::types::SessionId::new();
    let bridge = TicketBridge::new(true, session.clone());
    let runtime = make_runtime(Some(bridge.clone()));
    let luka = register_bound(&runtime, Some(&bridge), "luka", &session).await;
    let erin = register_bound(&runtime, Some(&bridge), "erin", &session).await;

    let correlation = "telegram:primary/i:769";
    let to_luka = runtime
        .dispatch_with_turn_ticket(&luka, None, &correlated("hi", "luka-769", correlation))
        .await
        .expect("dispatch to luka");
    let to_erin = runtime
        .dispatch_with_turn_ticket(&erin, None, &correlated("hi", "erin-769", correlation))
        .await
        .expect("dispatch to erin");
    let (luka_ticket, erin_ticket) = (tracked(&to_luka.turn), tracked(&to_erin.turn));
    assert_ne!(luka_ticket, erin_ticket);
    // Both admissions carry the one (canonicalized) correlation as their
    // interaction id, as meerkat requires; the tickets are not derived from it.
    let carried = bridge.tracked_admissions();
    assert_eq!(carried[0].interaction_id, carried[1].interaction_id);
    assert_eq!(
        runtime.turn_outcome(&erin, erin_ticket),
        TurnOutcome::Pending
    );

    bridge.finish(1, Ok("erin's reply"));
    assert_eq!(
        runtime
            .wait_for_turn(&erin, erin_ticket, Duration::from_secs(5))
            .await,
        completed("erin's reply")
    );
    assert_eq!(
        runtime.turn_outcome(&luka, luka_ticket),
        TurnOutcome::Pending,
        "erin's completion must never settle luka's ticket"
    );
    bridge.finish(0, Ok("luka's reply"));
    assert_eq!(
        runtime
            .wait_for_turn(&luka, luka_ticket, Duration::from_secs(5))
            .await,
        completed("luka's reply")
    );
}

/// Two dispatches to ONE identity under one correlation but different
/// idempotency keys are two turns: the second's ticket never reads the
/// first's (already settled) output.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reused_correlation_on_one_identity_is_two_turns() {
    let session = meerkat_core::types::SessionId::new();
    let bridge = TicketBridge::new(true, session.clone());
    let runtime = make_runtime(Some(bridge.clone()));
    let keeper = register_bound(&runtime, Some(&bridge), "keeper", &session).await;

    let first = runtime
        .dispatch_with_turn_ticket(&keeper, None, &correlated("one", "msg-1", "chat-42"))
        .await
        .expect("first dispatch");
    let first = tracked(&first.turn);
    bridge.finish(0, Ok("first reply"));
    assert_eq!(
        runtime
            .wait_for_turn(&keeper, first, Duration::from_secs(5))
            .await,
        completed("first reply")
    );

    let second = runtime
        .dispatch_with_turn_ticket(&keeper, None, &correlated("two", "msg-2", "chat-42"))
        .await
        .expect("second dispatch");
    let second = tracked(&second.turn);
    assert_ne!(first, second);
    assert_eq!(
        runtime.turn_outcome(&keeper, second),
        TurnOutcome::Pending,
        "the second turn must not read the first turn's output"
    );
    bridge.finish(1, Ok("second reply"));
    assert_eq!(
        runtime
            .wait_for_turn(&keeper, second, Duration::from_secs(5))
            .await,
        completed("second reply")
    );
}

/// An idempotent re-dispatch after the original completed names the
/// ORIGINAL's ticket and reads the original's output; it is delivered on the
/// ingress lane (where the runtime deduplicates it), so no waiter can report
/// the runtime's "no result of its own" terminal over that output. A
/// deduplicated re-dispatch whose original this registry never tracked is
/// tracked on its own and completes as `NoOwnResult`, never as "no text".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_redispatch_names_the_original_ticket_and_never_overwrites_its_output() {
    let session = meerkat_core::types::SessionId::new();
    let bridge = TicketBridge::new(true, session.clone());
    let runtime = make_runtime(Some(bridge.clone()));
    let keeper = register_bound(&runtime, Some(&bridge), "keeper", &session).await;

    let original = runtime
        .dispatch_with_turn_ticket(&keeper, None, &correlated("work", "evt-9", "evt-9"))
        .await
        .expect("original dispatch");
    let original = tracked(&original.turn);
    bridge.finish(0, Ok("answer"));
    assert_eq!(
        runtime
            .wait_for_turn(&keeper, original, Duration::from_secs(5))
            .await,
        completed("answer")
    );

    let retry = runtime
        .dispatch_with_turn_ticket(&keeper, None, &correlated("work", "evt-9", "evt-9"))
        .await
        .expect("retried dispatch");
    assert_eq!(tracked(&retry.turn), original);
    assert_eq!(bridge.tracked_admissions().len(), 1);
    assert_eq!(bridge.ingress_admissions().len(), 1);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        runtime.turn_outcome(&keeper, original),
        completed("answer"),
        "the original's output survives the re-dispatch"
    );

    // The original was untracked, so the registry cannot name it.
    runtime
        .dispatch_admission_tracked(&keeper, None, &correlated("other", "evt-10", "evt-10"))
        .await
        .expect("untracked original");
    let retry = runtime
        .dispatch_with_turn_ticket(&keeper, None, &correlated("other", "evt-10", "evt-10"))
        .await
        .expect("tracked re-dispatch");
    assert_eq!(
        runtime
            .wait_for_turn(&keeper, tracked(&retry.turn), Duration::from_secs(5))
            .await,
        TurnOutcome::Completed {
            output: TurnOutput::NoOwnResult
        }
    );
}

/// A tracked admission refused typed for the member's LIVE runtime mode
/// (`UnsupportedForMode`, before anything is submitted, as the bridge refuses
/// an autonomous_host member and as meerkat itself refuses) sends the delivery
/// once on the ingress lane instead, reported untracked with the typed
/// reason, for a send and a dispatch alike. The runtime never decides the
/// mode from its own spec: this member's spec even says turn_driven.
#[tokio::test]
async fn a_live_mode_refusal_falls_back_to_the_ingress_lane_once() {
    use meerkat_mob::MobRuntimeMode::{AutonomousHost, TurnDriven};
    for (mode, expected) in [
        (AutonomousHost, TurnUntrackable::AutonomousHost),
        (
            TurnDriven,
            TurnUntrackable::RefusedByRuntime { mode: TurnDriven },
        ),
    ] {
        let session = meerkat_core::types::SessionId::new();
        let bridge = TicketBridge::new(true, session.clone());
        *bridge.refuses_tracked_in.lock().unwrap() = Some(mode);
        let runtime = make_runtime(Some(bridge.clone()));
        let keeper = register_bound(&runtime, Some(&bridge), "keeper", &session).await;

        let sent = runtime
            .send_with_turn_ticket(&keeper, None, &content("alpha"), HandlingMode::Queue, None)
            .await
            .expect("the send is delivered on the ingress lane");
        assert_eq!(sent.turn, TurnTracking::Unavailable(expected));
        assert!(expected.delivered());
        let dispatched = runtime
            .dispatch_with_turn_ticket(&keeper, None, &correlated("beta", "evt-1", "chat-1"))
            .await
            .expect("the dispatch is delivered on the ingress lane");
        assert_eq!(dispatched.turn, TurnTracking::Unavailable(expected));
        assert_eq!(bridge.refused.load(Ordering::SeqCst), 2);
        assert_eq!(
            bridge.ingress_admissions().len(),
            2,
            "each delivered exactly once"
        );
        assert!(bridge.tracked_admissions().is_empty());
        assert!(
            runtime
                .member_health(&keeper)
                .await
                .expect("health")
                .last_delivery_error
                .is_none(),
            "the fallback delivery succeeded; the refusal is not a delivery failure"
        );
    }
    assert_eq!(TurnUntrackable::AutonomousHost.code(), "autonomous_host");
    assert_eq!(
        TurnUntrackable::RefusedByRuntime { mode: TurnDriven }.code(),
        "runtime_refused"
    );
    assert!(!TurnUntrackable::NotDelivered.delivered());
}

/// meerkat deduplicates only within one session's runtime ledger, and a
/// session rotation keeps the incarnation's runtime id. So after a rotation a
/// re-dispatch of a completed tracked key is NOT given the original's ticket:
/// it runs as a turn of its own on the new session and reports its own
/// output.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_redispatch_after_a_session_rotation_runs_and_reports_its_own_turn() {
    let session = meerkat_core::types::SessionId::new();
    let bridge = TicketBridge::new(true, session.clone());
    let runtime = make_runtime(Some(bridge.clone()));
    let keeper = register_bound(&runtime, Some(&bridge), "keeper", &session).await;

    let original = runtime
        .dispatch_with_turn_ticket(&keeper, None, &correlated("work", "evt-7", "evt-7"))
        .await
        .expect("original dispatch");
    let original = tracked(&original.turn);
    bridge.finish(0, Ok("first answer"));
    assert_eq!(
        runtime
            .wait_for_turn(&keeper, original, Duration::from_secs(5))
            .await,
        completed("first answer")
    );

    // The member moves onto a fresh session and Core rebinds its live member
    // there (same runtime id); the identity follows Core's binding.
    let rotated = bridge.rotate_session();
    bridge.core.move_live_session(&keeper, rotated.clone());
    runtime
        .send_admission_tracked(&keeper, None, &content("ping"), HandlingMode::Queue, None)
        .await
        .expect("delivery after the rotation");
    assert_eq!(
        runtime
            .status(&keeper)
            .await
            .expect("status")
            .session_id
            .as_ref(),
        Some(&rotated),
        "the identity follows the rotated session"
    );

    let retry = runtime
        .dispatch_with_turn_ticket(&keeper, None, &correlated("work", "evt-7", "evt-7"))
        .await
        .expect("re-dispatch after the rotation");
    let retry = tracked(&retry.turn);
    assert_ne!(retry, original, "the rotated ledger never saw the key");
    assert_eq!(bridge.tracked_admissions().len(), 2);
    bridge.finish(1, Ok("second answer"));
    assert_eq!(
        runtime
            .wait_for_turn(&keeper, retry, Duration::from_secs(5))
            .await,
        completed("second answer")
    );
    assert_eq!(
        runtime.turn_outcome(&keeper, original),
        completed("first answer")
    );
}

/// A re-dispatch that the registry would name by the original's ticket, but
/// whose delivery lands on a different session (delivery repair rotates the
/// member onto a fresh session while keeping its runtime id), is NOT given
/// the original's ticket: the new session's ledger never saw the key, so the
/// delivery ran as its own turn, and it is reported untracked with the typed
/// reason. The original's ticket keeps the original's output.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_redispatch_rotated_onto_a_new_session_is_not_given_the_original_ticket() {
    let session = meerkat_core::types::SessionId::new();
    let bridge = TicketBridge::new(true, session.clone());
    let runtime = make_runtime(Some(bridge.clone()));
    let keeper = register_bound(&runtime, Some(&bridge), "keeper", &session).await;

    let original = runtime
        .dispatch_with_turn_ticket(&keeper, None, &correlated("work", "evt-8", "evt-8"))
        .await
        .expect("original dispatch");
    let original = tracked(&original.turn);
    bridge.finish(0, Ok("answer"));
    assert_eq!(
        runtime
            .wait_for_turn(&keeper, original, Duration::from_secs(5))
            .await,
        completed("answer")
    );

    bridge.rotate_on_next_ingress.store(true, Ordering::SeqCst);
    let retry = runtime
        .dispatch_with_turn_ticket(&keeper, None, &correlated("work", "evt-8", "evt-8"))
        .await
        .expect("re-dispatch");
    assert_eq!(
        retry.turn,
        TurnTracking::Unavailable(TurnUntrackable::SessionRotated)
    );
    assert!(TurnUntrackable::SessionRotated.delivered());
    assert_eq!(
        bridge.ingress_admissions().len(),
        1,
        "delivered exactly once, on the rotated session"
    );
    assert_eq!(
        runtime.turn_outcome(&keeper, original),
        completed("answer"),
        "the original's ticket still names the original turn"
    );
}

/// Post-admission session resolution failing does not turn a successful turn
/// into a failure: the ticket reports the turn's own output, and the
/// resolution failure is recorded, typed, on the delivery-error channel.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_session_resolution_failure_keeps_the_turns_own_output() {
    let session = meerkat_core::types::SessionId::new();
    let bridge = TicketBridge::build(true, true, session.clone());
    let runtime = make_runtime(Some(bridge.clone()));
    let keeper = register_bound(&runtime, Some(&bridge), "keeper", &session).await;

    let sent = runtime
        .send_with_turn_ticket(&keeper, None, &content("alpha"), HandlingMode::Queue, None)
        .await
        .expect("the admission succeeded");
    let ticket = tracked(&sent.turn);
    let health = runtime.member_health(&keeper).await.expect("health");
    let recorded = health
        .last_delivery_error
        .expect("the resolution failure is recorded");
    assert_eq!(recorded.class, DeliveryErrorClass::Completion);
    assert!(
        recorded.detail.contains(&ticket.to_string()),
        "{}",
        recorded.detail
    );

    bridge.finish(0, Ok("alpha reply"));
    assert_eq!(
        runtime
            .wait_for_turn(&keeper, ticket, Duration::from_secs(5))
            .await,
        completed("alpha reply")
    );
}

// ===========================================================================
// Real runtime, through the gateway RPC
// ===========================================================================

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

    fn calls_for(&self, name: &str) -> usize {
        self.calls_by_name
            .lock()
            .unwrap()
            .get(name)
            .copied()
            .unwrap_or(0)
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
        let [usage, done] =
            llm_usage::usage_then_done(request, meerkat::Provider::OpenAI, StopReason::EndTurn);
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
    runtime: &meerkat_meta::UnifiedRuntime,
    ctx: &meerkat_meta::rpc::IdentityFirstContext,
    method: &str,
    params: Value,
) -> Value {
    let request = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    let reply = meerkat_meta::rpc::handle_unified_rpc_json(
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
    runtime: &meerkat_meta::UnifiedRuntime,
    ctx: &meerkat_meta::rpc::IdentityFirstContext,
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

async fn await_turn(
    runtime: &meerkat_meta::UnifiedRuntime,
    ctx: &meerkat_meta::rpc::IdentityFirstContext,
    ticket: &str,
) -> Value {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let result = turn_result(runtime, ctx, ticket).await;
            if result["state"] != "pending" {
                return result;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the turn settles")
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
    ctx: &meerkat_meta::rpc::IdentityFirstContext,
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
    meerkat_meta::UnifiedRuntime,
    meerkat_meta::rpc::IdentityFirstContext,
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
    runtime: meerkat_meta::UnifiedRuntime,
    ctx: meerkat_meta::rpc::IdentityFirstContext,
    scratch: tempfile::TempDir,
    mob_id: String,
    roster: Arc<MutableRosterProvider>,
}

/// What a test reads off the live meerkat roster entry.
struct LiveMember {
    identity: meerkat_mob::ids::AgentIdentity,
    role: meerkat_mob::ProfileName,
    labels: BTreeMap<String, String>,
    /// The runtime mode meerkat actually checks.
    runtime_mode: meerkat_mob::MobRuntimeMode,
    session: Option<meerkat_core::types::SessionId>,
}

impl LiveMob {
    /// The one member's live meerkat roster entry.
    async fn live_member(&self) -> LiveMember {
        let members = self.runtime.mob_handle().list_all_members().await;
        assert_eq!(members.len(), 1, "one live member");
        let entry = members.into_iter().next().expect("one live member");
        LiveMember {
            session: entry.bridge_session_id().cloned(),
            identity: entry.agent_identity,
            role: entry.role,
            labels: entry.labels,
            runtime_mode: entry.runtime_mode,
        }
    }
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
    let runtime = meerkat_meta::UnifiedRuntime::builder()
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
    let ctx = meerkat_meta::rpc::IdentityFirstContext {
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
        mob_id,
        roster,
    }
}

/// Wait until the model has been called for the input named `name`, i.e. the
/// delivery reached a turn, whichever lane carried it.
async fn wait_for_model_call(client: &NamedReplyClient, name: &str) {
    tokio::time::timeout(Duration::from_secs(30), async {
        while client.calls_for(name) == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the model never saw {name:?}"));
}

/// An untracked delivery to a live member: delivered exactly once (one model
/// call for it, still one after the runtime settles), with the typed code.
async fn assert_delivered_once_untracked(
    client: &NamedReplyClient,
    result: &Value,
    name: &str,
    code: &str,
) {
    assert!(result["turn"].is_null(), "{result}");
    assert_eq!(result["turn_unavailable"]["code"], code, "{result}");
    assert_eq!(result["turn_unavailable"]["delivered"], true, "{result}");
    wait_for_model_call(client, name).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        client.calls_for(name),
        1,
        "{name} was delivered exactly once"
    );
}

/// Poll the session's latest output (`output_preview`, which is what the old
/// identity-wide wait returns once satisfied) until `accept` holds for it.
/// Tests never gate on the identity-wide completion cursor itself: it moves
/// only once the identity health monitor has subscribed to the member's event
/// stream, and that timing is not part of what these tests assert.
async fn wait_for_preview(
    runtime: &meerkat_meta::UnifiedRuntime,
    ctx: &meerkat_meta::rpc::IdentityFirstContext,
    accept: impl Fn(&str) -> bool,
) -> String {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let inspection = rpc(
                runtime,
                ctx,
                "meta/inspect_identity",
                json!({"identity": "keeper"}),
            )
            .await;
            if let Some(preview) = inspection["output_preview"].as_str()
                && accept(preview)
            {
                return preview.to_string();
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the session commits the expected output")
}

/// End to end on a real runtime: a foreign dispatch to `keeper` is admitted
/// first and held in the model; two tracked sends queue behind it (the member
/// is turn_driven, so turns run one at a time). When the foreign turn
/// finishes, the session's latest output is the FOREIGN reply while the first
/// send's own turn is still pending. The test does not run the old
/// identity-wide cursor wait (the SDKs' previous `send_and_wait`), whose
/// timing depends on the identity health monitor's subscription; it shows the
/// state that wait reads once satisfied, which could only hand back a foreign
/// output. Every sync point is typed (tickets, the model holding a turn).
/// Each ticket reports its own turn's reply.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_gateway_sends_each_get_their_own_reply() {
    let client = NamedReplyClient::default();
    let release_foreign = client.hold("foreign");
    let release_alpha = client.hold("alpha");
    let (runtime, ctx, _scratch) =
        live_runtime(meerkat_mob::MobRuntimeMode::TurnDriven, &client).await;

    // The foreign dispatch is admitted first and held in the model. It is
    // tracked only so the test can observe its completion typed.
    let foreign = rpc(
        &runtime,
        &ctx,
        "meta/dispatch",
        json!({
            "identity": "keeper",
            "dispatch_input": {"content": "foreign", "origin": "system"},
            "track_turn": true,
        }),
    )
    .await;
    let foreign_ticket = foreign["turn"]["ticket"]
        .as_str()
        .unwrap_or_else(|| panic!("the foreign dispatch is tracked: {foreign}"))
        .to_string();
    // The model is holding the foreign turn before the tracked sends go out,
    // so their baselines are read while it is still running.
    wait_for_model_call(&client, "foreign").await;
    let alpha = rpc(
        &runtime,
        &ctx,
        "meta/send",
        json!({"identity": "keeper", "content": "alpha", "track_turn": true}),
    )
    .await;
    let beta = rpc(
        &runtime,
        &ctx,
        "meta/send",
        json!({"identity": "keeper", "content": "beta", "track_turn": true}),
    )
    .await;
    let alpha_ticket = alpha["turn"]["ticket"]
        .as_str()
        .unwrap_or_else(|| panic!("alpha is tracked: {alpha}"))
        .to_string();
    let beta_ticket = beta["turn"]["ticket"]
        .as_str()
        .unwrap_or_else(|| panic!("beta is tracked: {beta}"))
        .to_string();
    assert_eq!(
        turn_result(&runtime, &ctx, &alpha_ticket).await["state"],
        "pending"
    );

    // The foreign turn finishes; alpha is held in the model and beta is
    // queued behind it, so nothing else can commit output meanwhile.
    release_foreign.send(()).expect("release the dispatch");
    assert_eq!(
        await_turn(&runtime, &ctx, &foreign_ticket).await["output"],
        "reply to: foreign"
    );
    // Control: the state the old identity-wide wait reads once satisfied. The
    // session's latest output becomes the FOREIGN reply (this wait fails by
    // timing out if it never does) while alpha's own turn is still pending,
    // so a cursor-satisfied wait at this point can only hand back a foreign
    // output.
    wait_for_preview(&runtime, &ctx, |preview| preview == "reply to: foreign").await;
    assert_eq!(
        turn_result(&runtime, &ctx, &alpha_ticket).await["state"],
        "pending",
        "the foreign completion must not settle alpha's ticket"
    );

    release_alpha.send(()).expect("release alpha");
    let alpha_result = await_turn(&runtime, &ctx, &alpha_ticket).await;
    assert_eq!(alpha_result["state"], "completed", "{alpha_result}");
    assert_eq!(alpha_result["output_status"], "text", "{alpha_result}");
    assert_eq!(alpha_result["output"], "reply to: alpha", "{alpha_result}");
    let beta_result = await_turn(&runtime, &ctx, &beta_ticket).await;
    assert_eq!(beta_result["state"], "completed", "{beta_result}");
    assert_eq!(beta_result["output"], "reply to: beta", "{beta_result}");
    assert_eq!(
        turn_result(&runtime, &ctx, &uuid::Uuid::new_v4().to_string()).await["state"],
        "unknown"
    );
    runtime.shutdown().await;
}

/// The same gateway calls against an `autonomous_host` member (meerkat's
/// default mode, which examples 001, 002 and 004 run). A tracked send or
/// dispatch is refused for the live mode before anything is submitted and
/// delivered once on the ingress lane, untracked. The assertions are only
/// what that fallback guarantees: the typed code, exactly one model call per
/// input, and that the identity-wide wait (all an untracked caller has)
/// returns one of this identity's replies. It does NOT promise which one: an
/// autonomous member's own kickoff turn, or the other delivery, can satisfy
/// it (the SDK helpers warn about exactly this). On this meerkat pin an
/// autonomous inbox delivery carries no interaction or run id, so there is no
/// typed per-input correlation to assert.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_autonomous_member_through_the_gateway_is_delivered_once_untracked() {
    let client = NamedReplyClient::default();
    let (runtime, ctx, _scratch) =
        live_runtime(meerkat_mob::MobRuntimeMode::AutonomousHost, &client).await;

    let sent = rpc(
        &runtime,
        &ctx,
        "meta/send",
        json!({"identity": "keeper", "content": "alpha", "track_turn": true}),
    )
    .await;
    let dispatched = rpc(
        &runtime,
        &ctx,
        "meta/dispatch",
        json!({
            "identity": "keeper",
            "dispatch_input": {"content": "beta", "origin": "system"},
            "track_turn": true,
        }),
    )
    .await;
    assert_delivered_once_untracked(&client, &sent, "alpha", "autonomous_host").await;
    assert_delivered_once_untracked(&client, &dispatched, "beta", "autonomous_host").await;
    // What a satisfied fallback wait returns is the session's latest output:
    // one of THIS identity's replies (to alpha, to beta, or to its own
    // kickoff turn), never a promise about which.
    let fallback = wait_for_preview(&runtime, &ctx, |preview| !preview.is_empty()).await;
    assert!(
        ["reply to: alpha", "reply to: beta", "reply to: other"].contains(&fallback.as_str()),
        "the fallback returned something this identity never said: {fallback}"
    );
    runtime.shutdown().await;
}

/// meerkat's own deduplication, end to end on a turn-driven member. An
/// idempotent re-dispatch of a tracked original names the original's ticket
/// and reads its reply, without a second turn. A tracked re-dispatch of an
/// UNTRACKED original (which the registry cannot name) is deduplicated by
/// meerkat onto the ended original: it completes with `no_own_result`, the
/// typed "consumed without a run result of its own", never an empty output
/// and never the original's reply.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_live_redispatch_reads_the_original_reply_or_no_own_result() {
    let client = NamedReplyClient::default();
    let (runtime, ctx, _scratch) =
        live_runtime(meerkat_mob::MobRuntimeMode::TurnDriven, &client).await;
    let dispatch = |content: &str, key: &str, track: bool| {
        json!({
            "identity": "keeper",
            "dispatch_input": {
                "content": content,
                "origin": "connector",
                "idempotency_key": key,
                "correlation_id": format!("chat/{key}"),
            },
            "track_turn": track,
        })
    };

    let original = rpc(
        &runtime,
        &ctx,
        "meta/dispatch",
        dispatch("alpha", "evt-1", true),
    )
    .await;
    let ticket = original["turn"]["ticket"]
        .as_str()
        .unwrap_or_else(|| panic!("the original is tracked: {original}"))
        .to_string();
    let first = await_turn(&runtime, &ctx, &ticket).await;
    assert_eq!(first["output"], "reply to: alpha", "{first}");
    let retry = rpc(
        &runtime,
        &ctx,
        "meta/dispatch",
        dispatch("alpha", "evt-1", true),
    )
    .await;
    assert_eq!(retry["turn"]["ticket"], ticket.as_str(), "{retry}");
    tokio::time::sleep(Duration::from_millis(200)).await;
    let again = turn_result(&runtime, &ctx, &ticket).await;
    assert_eq!(again["output"], "reply to: alpha", "{again}");
    assert_eq!(client.calls_for("alpha"), 1, "the retry ran no second turn");

    let untracked = rpc(
        &runtime,
        &ctx,
        "meta/dispatch",
        dispatch("beta", "evt-2", false),
    )
    .await;
    assert!(untracked.get("turn").is_none(), "{untracked}");
    wait_for_model_call(&client, "beta").await;
    // The untracked original has ENDED before the retry: then the runtime's
    // dedup reports no result of the retry's own. (While the original is
    // still running, the retry shares the one input's completion and reports
    // the original's result.) A tracked sentinel queued behind it on this
    // turn_driven member completes only after the original's turn has.
    let sentinel = rpc(
        &runtime,
        &ctx,
        "meta/send",
        json!({"identity": "keeper", "content": "sentinel", "track_turn": true}),
    )
    .await;
    let sentinel_ticket = sentinel["turn"]["ticket"]
        .as_str()
        .unwrap_or_else(|| panic!("the sentinel is tracked: {sentinel}"))
        .to_string();
    await_turn(&runtime, &ctx, &sentinel_ticket).await;
    let retry = rpc(
        &runtime,
        &ctx,
        "meta/dispatch",
        dispatch("beta", "evt-2", true),
    )
    .await;
    let retry_ticket = retry["turn"]["ticket"]
        .as_str()
        .unwrap_or_else(|| panic!("the retry is tracked: {retry}"))
        .to_string();
    let deduplicated = await_turn(&runtime, &ctx, &retry_ticket).await;
    assert_eq!(deduplicated["state"], "completed", "{deduplicated}");
    assert_eq!(
        deduplicated["output_status"], "no_own_result",
        "{deduplicated}"
    );
    assert!(deduplicated["output"].is_null(), "{deduplicated}");
    assert_eq!(
        client.calls_for("beta"),
        1,
        "meerkat deduplicated the retry"
    );
    runtime.shutdown().await;
}

/// Trackability follows the LIVE member's runtime mode, the value meerkat
/// checks, never Meta's desired spec or profile table, on a real mob:
///
/// - spawned `autonomous_host`, then the profile table says `turn_driven`
///   (no respawn): the member still runs autonomous, so a tracked send is
///   refused for the live mode and delivered once, untracked, code
///   `autonomous_host`;
/// - spawned `turn_driven`, then the profile table says `autonomous_host`
///   (no respawn): the member still runs turn_driven, so the send is tracked
///   and reports its own reply;
/// - a same-profile `runtime_mode_override` hot reload from autonomous_host to
///   turn_driven (roster change plus `meta/reconcile_identity`, which swaps
///   the spec without a respawn): delivered once, untracked;
/// - a delivery-repair-shaped respawn (role, labels and `Resume` only, which
///   drops the override): the member comes back `autonomous_host` although
///   its spec pins turn_driven, and the send is delivered once, untracked.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn trackability_follows_the_live_runtime_mode() {
    use meerkat_mob::MobRuntimeMode::{AutonomousHost, TurnDriven};

    // Spawned autonomous_host; profile table flipped to turn_driven.
    let client = NamedReplyClient::default();
    let live = live_runtime_with(AutonomousHost, None, &client).await;
    let first = rpc(
        &live.runtime,
        &live.ctx,
        "meta/send",
        json!({"identity": "keeper", "content": "foreign", "track_turn": true}),
    )
    .await;
    assert_delivered_once_untracked(&client, &first, "foreign", "autonomous_host").await;
    flip_profile_table(&live.ctx, worker_definition(&live.mob_id, TurnDriven));
    assert_eq!(live.live_member().await.runtime_mode, AutonomousHost);
    let sent = rpc(
        &live.runtime,
        &live.ctx,
        "meta/send",
        json!({"identity": "keeper", "content": "alpha", "track_turn": true}),
    )
    .await;
    assert_delivered_once_untracked(&client, &sent, "alpha", "autonomous_host").await;
    assert!(
        sent["turn_unavailable"]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("respawn")),
        "{sent}"
    );
    live.runtime.shutdown().await;

    // Spawned turn_driven; profile table flipped to autonomous_host.
    let client = NamedReplyClient::default();
    let live = live_runtime_with(TurnDriven, None, &client).await;
    let tracked = rpc(
        &live.runtime,
        &live.ctx,
        "meta/send",
        json!({"identity": "keeper", "content": "foreign", "track_turn": true}),
    )
    .await;
    let ticket = tracked["turn"]["ticket"]
        .as_str()
        .unwrap_or_else(|| panic!("a turn_driven member is tracked: {tracked}"))
        .to_string();
    assert_eq!(
        await_turn(&live.runtime, &live.ctx, &ticket).await["output"],
        "reply to: foreign"
    );
    flip_profile_table(&live.ctx, worker_definition(&live.mob_id, AutonomousHost));
    let tracked = rpc(
        &live.runtime,
        &live.ctx,
        "meta/send",
        json!({"identity": "keeper", "content": "beta", "track_turn": true}),
    )
    .await;
    let ticket = tracked["turn"]["ticket"]
        .as_str()
        .unwrap_or_else(|| panic!("the live member still runs turn_driven: {tracked}"))
        .to_string();
    assert_eq!(
        await_turn(&live.runtime, &live.ctx, &ticket).await["output"],
        "reply to: beta"
    );
    live.runtime.shutdown().await;

    // Same-profile override hot reload, autonomous_host -> turn_driven.
    let client = NamedReplyClient::default();
    let live = live_runtime_with(AutonomousHost, None, &client).await;
    let first = rpc(
        &live.runtime,
        &live.ctx,
        "meta/send",
        json!({"identity": "keeper", "content": "foreign", "track_turn": true}),
    )
    .await;
    assert_delivered_once_untracked(&client, &first, "foreign", "autonomous_host").await;
    let mut reloaded = make_spec("keeper");
    reloaded.profile = "worker".into();
    reloaded.runtime_mode_override = Some(TurnDriven);
    live.roster.upsert(reloaded);
    rpc(
        &live.runtime,
        &live.ctx,
        "meta/reconcile_identity",
        json!({}),
    )
    .await;
    assert_eq!(
        live.live_member().await.runtime_mode,
        AutonomousHost,
        "the hot reload swapped the spec without respawning"
    );
    let sent = rpc(
        &live.runtime,
        &live.ctx,
        "meta/send",
        json!({"identity": "keeper", "content": "alpha", "track_turn": true}),
    )
    .await;
    assert_delivered_once_untracked(&client, &sent, "alpha", "autonomous_host").await;
    live.runtime.shutdown().await;

    // A repair-shaped respawn drops the override. The profile declares
    // autonomous_host; only the spec override pins turn_driven.
    let client = NamedReplyClient::default();
    let live = live_runtime_with(AutonomousHost, Some(TurnDriven), &client).await;
    let tracked = rpc(
        &live.runtime,
        &live.ctx,
        "meta/send",
        json!({"identity": "keeper", "content": "foreign", "track_turn": true}),
    )
    .await;
    let ticket = tracked["turn"]["ticket"]
        .as_str()
        .unwrap_or_else(|| panic!("spawned with the turn_driven override: {tracked}"))
        .to_string();
    await_turn(&live.runtime, &live.ctx, &ticket).await;
    let member = live.live_member().await;
    assert_eq!(member.runtime_mode, TurnDriven);
    let session = member.session.clone().expect("bridge session");
    let handle = live.runtime.mob_handle();
    handle
        .retire(member.identity.clone())
        .await
        .expect("repair retire");
    let mut respawn =
        meerkat_mob::SpawnMemberSpec::new(member.role.clone(), member.identity.clone())
            .with_labels(member.labels.clone());
    respawn.launch_mode = meerkat_mob::MemberLaunchMode::Resume {
        bridge_session_id: session,
        resume_from_role: None,
    };
    handle.spawn_spec(respawn).await.expect("repair respawn");
    assert_eq!(
        live.live_member().await.runtime_mode,
        AutonomousHost,
        "the repair-shaped respawn dropped the override"
    );
    let sent = rpc(
        &live.runtime,
        &live.ctx,
        "meta/send",
        json!({"identity": "keeper", "content": "alpha", "track_turn": true}),
    )
    .await;
    assert_delivered_once_untracked(&client, &sent, "alpha", "autonomous_host").await;
    live.runtime.shutdown().await;
}

async fn identity_cursor(
    runtime: &meerkat_meta::UnifiedRuntime,
    ctx: &meerkat_meta::rpc::IdentityFirstContext,
) -> meerkat_meta::identity_first::CompletionCursor {
    let read = rpc(
        runtime,
        ctx,
        "meta/completion_cursor",
        json!({"identity": "keeper"}),
    )
    .await;
    serde_json::from_value(read["completion_cursor"].clone()).expect("completion cursor")
}

/// Poll the identity-wide cursor until it has moved past `baseline`.
/// The deadline bounds the reads as well as polling. On timeout, return the
/// latest completed observation, or `baseline` if no read completed.
/// An incarnation change is a failure, never progress.
async fn cursor_after(
    runtime: &meerkat_meta::UnifiedRuntime,
    ctx: &meerkat_meta::rpc::IdentityFirstContext,
    baseline: meerkat_meta::identity_first::CompletionCursor,
    within: Duration,
) -> meerkat_meta::identity_first::CompletionCursor {
    let deadline = tokio::time::Instant::now() + within;
    let mut latest = baseline;
    match tokio::time::timeout_at(deadline, async {
        loop {
            let cursor = identity_cursor(runtime, ctx).await;
            latest = cursor;
            match cursor.progress_since(baseline) {
                CompletionProgress::Completed => return cursor,
                CompletionProgress::IncarnationChanged => {
                    panic!("incarnation changed: baseline {baseline:?}, now {cursor:?}")
                }
                _ => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
    })
    .await
    {
        Ok(cursor) => cursor,
        Err(_) => latest,
    }
}

/// Send one TRACKED warm-up turn and wait for its own completion by ticket.
async fn tracked_warm_up(
    runtime: &meerkat_meta::UnifiedRuntime,
    ctx: &meerkat_meta::rpc::IdentityFirstContext,
) {
    let sent = rpc(
        runtime,
        ctx,
        "meta/send",
        json!({"identity": "keeper", "content": "warm-up", "track_turn": true}),
    )
    .await;
    let ticket = sent["turn"]["ticket"]
        .as_str()
        .unwrap_or_else(|| panic!("the warm-up is tracked: {sent}"))
        .to_string();
    assert_eq!(
        await_turn(runtime, ctx, &ticket).await["state"],
        "completed"
    );
}

/// Corroborate that the identity health monitor is observing completions.
/// Warm-ups run one at a time and finish by their own tickets. Each send
/// triggers a machine change that lets the monitor attempt to subscribe.
///
/// Two consecutive single-increment windows show cursor activity, not that
/// the monitor has consumed every completed warm-up. Delayed observations can
/// still supply a later increment. Exact per-turn attribution requires the
/// typed subscription/drain barrier tracked in Meta issue #455; this helper
/// neither supplies that barrier nor repairs missed pre-subscription events.
async fn establish_cursor_subscription(
    runtime: &meerkat_meta::UnifiedRuntime,
    ctx: &meerkat_meta::rpc::IdentityFirstContext,
) {
    let mut exact_in_a_row = 0;
    for _ in 0..30 {
        let before = identity_cursor(runtime, ctx).await;
        tracked_warm_up(runtime, ctx).await;
        let after = cursor_after(runtime, ctx, before, Duration::from_secs(3)).await;
        if after == before.advanced() {
            exact_in_a_row += 1;
            if exact_in_a_row == 2 {
                return;
            }
        } else {
            exact_in_a_row = 0;
        }
    }
    panic!(
        "the identity health monitor never observed two consecutive single-increment warm-up windows"
    );
}

/// Corroborate identity-wide cursor activity across tracked and untracked
/// sends: ticket tracking is additive and must not replace cursor updates.
/// After tracked warm-ups finish, each selected send observes exactly one
/// increment and a baseline equal to the previous observed cursor.
/// Delayed warm-up observations can still satisfy those checks, so this is
/// not exact per-turn attribution until Meta issue #455 supplies the typed
/// subscription/drain barrier (see [`establish_cursor_subscription`]).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_tracked_turn_advances_the_identity_wide_cursor() {
    let client = NamedReplyClient::default();
    let (runtime, ctx, _scratch) =
        live_runtime(meerkat_mob::MobRuntimeMode::TurnDriven, &client).await;
    establish_cursor_subscription(&runtime, &ctx).await;

    let mut expected = identity_cursor(&runtime, &ctx).await;
    for (content, track) in [("alpha", true), ("beta", false), ("foreign", true)] {
        let sent = rpc(
            &runtime,
            &ctx,
            "meta/send",
            json!({"identity": "keeper", "content": content, "track_turn": track}),
        )
        .await;
        let baseline: meerkat_meta::identity_first::CompletionCursor =
            serde_json::from_value(sent["completion_baseline"].clone()).expect("baseline");
        assert_eq!(
            baseline, expected,
            "{content}: nothing else moved the cursor before this send"
        );
        if track {
            let ticket = sent["turn"]["ticket"]
                .as_str()
                .unwrap_or_else(|| panic!("tracked: {sent}"))
                .to_string();
            assert_eq!(
                await_turn(&runtime, &ctx, &ticket).await["state"],
                "completed"
            );
        }
        let after = cursor_after(&runtime, &ctx, baseline, Duration::from_secs(30)).await;
        if after == baseline {
            let status = tokio::time::timeout(
                Duration::from_secs(3),
                ctx.runtime.status(&make_identity("keeper")),
            )
            .await;
            panic!(
                "{content} (tracked={track}): the cursor never passed {baseline:?}; status {:?}",
                status.map(|status| status.map(|status| (status.state, status.generation))),
            );
        }
        assert_eq!(
            after,
            baseline.advanced(),
            "{content} (tracked={track}): exactly one completion counted"
        );
        expected = after;
    }
    runtime.shutdown().await;
}

/// #479: `meta/interact` refuses a caller-supplied interaction id that is
/// still in flight on the identity, typed, before delivering anything: two
/// in-flight runs under one transcript interaction id would make both runs'
/// frames and terminals ambiguous. Once the first interaction settled, the id
/// may be sent again (and still gets a ticket of its own).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interact_refuses_a_caller_interaction_id_still_in_flight() {
    let client = NamedReplyClient::default();
    let release_alpha = client.hold("alpha");
    let (runtime, ctx, _scratch) =
        live_runtime(meerkat_mob::MobRuntimeMode::TurnDriven, &client).await;
    let interaction = uuid::Uuid::new_v4().to_string();
    let interact = |text: &'static str| {
        let runtime = &runtime;
        let ctx = &ctx;
        let interaction = interaction.clone();
        async move {
            let request = json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "meta/interact",
                "params": {
                    "identity": "keeper",
                    "content": text,
                    "interaction_id": interaction,
                    "track_turn": true,
                },
            });
            let reply = meerkat_meta::rpc::handle_unified_rpc_json(
                runtime,
                &request.to_string(),
                Duration::from_secs(30),
                None,
                Some(ctx),
            )
            .await;
            serde_json::from_str::<Value>(&reply).expect("json-rpc reply")
        }
    };

    let first = interact("alpha").await;
    let first_ticket = first["result"]["turn"]["ticket"]
        .as_str()
        .unwrap_or_else(|| panic!("the first interact is tracked: {first}"))
        .to_string();
    assert_eq!(first["result"]["interaction_id"], json!(interaction));
    wait_for_model_call(&client, "alpha").await;

    let reused = interact("beta").await;
    assert_eq!(reused["error"]["code"], json!(-32602), "{reused}");
    assert_eq!(
        reused["error"]["data"]["code"],
        json!("interaction_id_in_flight"),
        "{reused}"
    );
    assert_eq!(
        reused["error"]["data"]["interaction_id"],
        json!(interaction)
    );
    assert!(reused.get("result").is_none(), "{reused}");

    release_alpha.send(()).expect("release alpha");
    assert_eq!(
        await_turn(&runtime, &ctx, &first_ticket).await["state"],
        "completed"
    );
    // The refused send was never delivered.
    assert_eq!(client.calls_for("beta"), 0);

    // Once the console projected the first run's terminal, the settled id
    // is accepted again, as a turn of its own.
    let again = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            runtime
                .drain_mob_agent_events()
                .await
                .expect("drain member events");
            let reply = interact("beta").await;
            if reply.get("error").is_none() {
                return reply;
            }
            assert_eq!(
                reply["error"]["data"]["code"],
                json!("interaction_id_in_flight"),
                "{reply}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the settled interaction id is accepted again");
    let again_ticket = again["result"]["turn"]["ticket"]
        .as_str()
        .unwrap_or_else(|| panic!("the reuse is tracked: {again}"))
        .to_string();
    assert_ne!(again_ticket, first_ticket);
    assert_eq!(
        await_turn(&runtime, &ctx, &again_ticket).await["output"],
        "reply to: beta"
    );
    runtime.shutdown().await;
}

/// #468 review: the waits observe runtime shutdown and end typed
/// (`shutting_down`) instead of holding the request to its deadline.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn server_side_waits_end_typed_on_shutdown() {
    let client = NamedReplyClient::default();
    let release = client.hold("alpha");
    let live = live_runtime_with(
        meerkat_mob::MobRuntimeMode::TurnDriven,
        Some(meerkat_mob::MobRuntimeMode::TurnDriven),
        &client,
    )
    .await;
    let baseline = identity_cursor(&live.runtime, &live.ctx).await;
    let sent = rpc(
        &live.runtime,
        &live.ctx,
        "meta/send",
        json!({"identity": "keeper", "content": "alpha", "track_turn": true}),
    )
    .await;
    let ticket = sent["turn"]["ticket"]
        .as_str()
        .unwrap_or_else(|| panic!("tracked: {sent}"))
        .to_string();
    wait_for_model_call(&client, "alpha").await;

    let completion = rpc(
        &live.runtime,
        &live.ctx,
        "meta/wait_for_completion",
        json!({
            "identity": "keeper",
            "after": {"epoch": baseline.epoch.get(), "turns": baseline.turns},
            "timeout_ms": 600_000,
        }),
    );
    let turn = rpc(
        &live.runtime,
        &live.ctx,
        "meta/wait_for_turn",
        json!({"identity": "keeper", "ticket": ticket, "timeout_ms": 600_000}),
    );
    let shutdown = async {
        tokio::task::yield_now().await;
        live.runtime.shutdown().await;
    };
    let (completion, turn, ()) = tokio::time::timeout(Duration::from_mins(1), async {
        tokio::join!(completion, turn, shutdown)
    })
    .await
    .expect("shutdown ends both waits");
    assert_eq!(completion["outcome"], "shutting_down", "{completion}");
    assert_eq!(turn["wait"], "shutting_down", "{turn}");
    drop(release);
}

/// `meta/stop_member_run` addressed by the member's `rt:` runtime alias
/// resolves the durable identity's roster row (the unified path used to key
/// by the undecoded alias). A stale run id is `not_current` and names the
/// live run; stopping that exact run returns `stopped` and ends the turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_member_run_by_runtime_alias_stops_the_exact_live_run() {
    let client = NamedReplyClient::default();
    let release_alpha = client.hold("alpha");
    let (runtime, ctx, _scratch) =
        live_runtime(meerkat_mob::MobRuntimeMode::TurnDriven, &client).await;
    let status = rpc(
        &runtime,
        &ctx,
        "meta/status_identity",
        json!({"identity": "keeper"}),
    )
    .await;
    let alias = status["agent_runtime_id"]
        .as_str()
        .unwrap_or_else(|| panic!("keeper has a runtime alias: {status}"))
        .to_string();
    assert!(alias.starts_with("rt:"), "{alias}");

    let dispatched = rpc(
        &runtime,
        &ctx,
        "meta/dispatch",
        json!({
            "identity": "keeper",
            "dispatch_input": {"content": "alpha", "origin": "system"},
            "track_turn": true,
        }),
    )
    .await;
    let ticket = dispatched["turn"]["ticket"]
        .as_str()
        .unwrap_or_else(|| panic!("the dispatch is tracked: {dispatched}"))
        .to_string();
    wait_for_model_call(&client, "alpha").await;

    let stale = rpc(
        &runtime,
        &ctx,
        "meta/stop_member_run",
        json!({
            "member_id": alias,
            "run_id": "01936f8b-0000-7000-8000-000000000042",
            "reason": "stale selection",
        }),
    )
    .await;
    assert_eq!(stale["receipt"]["outcome"], json!("not_current"), "{stale}");
    let live_run = stale["receipt"]["current_run_id"]
        .as_str()
        .unwrap_or_else(|| panic!("the stale receipt names the live run: {stale}"))
        .to_string();
    assert_eq!(
        client.calls_for("alpha"),
        1,
        "a stale stop never interrupts"
    );

    let stopped = rpc(
        &runtime,
        &ctx,
        "meta/stop_member_run",
        json!({"member_id": alias, "run_id": live_run, "reason": "user pressed stop"}),
    )
    .await;
    assert_eq!(stopped["receipt"]["outcome"], json!("stopped"), "{stopped}");
    assert_eq!(stopped["receipt"]["run_id"], json!(live_run));
    let contributors = stopped["receipt"]["contributors"]
        .as_array()
        .expect("contributors");
    assert!(!contributors.is_empty(), "{stopped}");
    assert!(
        contributors
            .iter()
            .all(|row| row["terminal"] == json!("cancelled")),
        "{stopped}"
    );
    drop(release_alpha);
    let outcome = await_turn(&runtime, &ctx, &ticket).await;
    assert_ne!(outcome["state"], json!("completed"), "{outcome}");
    assert_eq!(client.calls_for("alpha"), 1, "no successor provider call");
}

/// A host turn authored from a registered row (`meta/registered_spec`)
/// names that row's digest on `meta/send`. Once a same-profile instructions
/// change commits, the gateway refuses the stale input typed before any
/// delivery; an input naming the current row is delivered.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_send_naming_a_superseded_registered_row_is_refused_before_delivery() {
    let client = NamedReplyClient::default();
    let (runtime, ctx, _scratch) =
        live_runtime(meerkat_mob::MobRuntimeMode::TurnDriven, &client).await;

    let read = rpc(
        &runtime,
        &ctx,
        "meta/registered_spec",
        json!({"identity": "keeper"}),
    )
    .await;
    assert_eq!(read["identity"], "keeper", "{read}");
    assert_eq!(read["spec"]["additional_instructions"], json!([]), "{read}");
    let stale_digest = read["spec_digest"]
        .as_str()
        .unwrap_or_else(|| panic!("the read names the row's digest: {read}"))
        .to_string();

    let mut changed: DurableAgentSpec =
        serde_json::from_value(read["spec"].clone()).expect("the read carries the full row");
    changed.additional_instructions = vec!["Answer in one sentence.".to_string()];
    ctx.runtime
        .update_spec(changed)
        .await
        .expect("the row change commits");

    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "meta/send",
        "params": {
            "identity": "keeper",
            "content": "beta",
            "track_turn": true,
            "expected_spec_digest": stale_digest,
        },
    });
    let reply: Value = serde_json::from_str(
        &meerkat_meta::rpc::handle_unified_rpc_json(
            &runtime,
            &request.to_string(),
            Duration::from_secs(30),
            None,
            Some(&ctx),
        )
        .await,
    )
    .expect("json-rpc reply");
    assert_eq!(reply["error"]["code"], -32006, "{reply}");
    assert_eq!(reply["error"]["data"]["kind"], "registered_spec_changed");
    assert_eq!(reply["error"]["data"]["submitted"], false);

    let current = rpc(
        &runtime,
        &ctx,
        "meta/registered_spec",
        json!({"identity": "keeper"}),
    )
    .await;
    assert_eq!(
        current["spec"]["additional_instructions"],
        json!(["Answer in one sentence."])
    );
    assert_ne!(current["spec_digest"], json!(stale_digest));
    let sent = rpc(
        &runtime,
        &ctx,
        "meta/send",
        json!({
            "identity": "keeper",
            "content": "alpha",
            "track_turn": true,
            "expected_spec_digest": current["spec_digest"],
        }),
    )
    .await;
    assert!(sent["turn"]["ticket"].is_string(), "{sent}");
    wait_for_model_call(&client, "alpha").await;
    assert_eq!(
        client.calls_for("beta"),
        0,
        "the refused input never reached the model"
    );
    runtime.shutdown().await;
}

/// A Dormant identity is not woken for an input authored from a superseded
/// registered row: the row is checked in the lifecycle-owned embodiment
/// decision, under the identity's lifecycle lock, so the refusal spawns,
/// builds or resumes nothing and leaves the identity's readiness untouched.
/// The current row wakes it and is delivered.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_superseded_row_does_not_wake_a_dormant_identity() {
    let session = meerkat_core::types::SessionId::new();
    let bridge = TicketBridge::new(true, session);
    let runtime = make_runtime(Some(bridge.clone()));
    let spec = make_spec("sleeper");
    let identity = spec.identity.clone();
    runtime.register(spec.clone()).await;
    let stale = runtime
        .registered_spec(&identity)
        .await
        .expect("registered row");
    let mut changed = spec;
    changed.additional_instructions = vec!["Answer in one sentence.".to_string()];
    runtime
        .update_spec(changed.clone())
        .await
        .expect("the row change commits");

    let error = runtime
        .send_expecting_registered_spec(
            &identity,
            None,
            &content("alpha"),
            HandlingMode::Queue,
            true,
            &stale.spec_digest,
        )
        .await
        .expect_err("the stale authored input is refused");
    assert!(
        matches!(
            error,
            meerkat_meta::identity_first::IdentityRuntimeError::RegisteredSpecChanged { .. }
        ),
        "{error}"
    );
    let status = runtime.status(&identity).await.expect("status");
    assert_eq!(
        status.state,
        IdentityLifecycleState::Dormant,
        "the refusal did not wake the identity"
    );
    assert!(
        status.agent_runtime_id.is_none(),
        "the refusal spawned no member"
    );
    assert!(
        status.session_id.is_none(),
        "the refusal created no session"
    );
    assert_eq!(
        bridge.materializations.load(Ordering::SeqCst),
        0,
        "the refusal built and resumed nothing"
    );
    assert!(bridge.ingress.lock().unwrap().is_empty());
    assert!(bridge.tracked.lock().unwrap().is_empty());

    let current = runtime
        .registered_spec(&identity)
        .await
        .expect("registered row");
    assert_eq!(current.spec, changed);
    runtime
        .send_expecting_registered_spec(
            &identity,
            None,
            &content("alpha"),
            HandlingMode::Queue,
            true,
            &current.spec_digest,
        )
        .await
        .expect("the current row wakes the identity and is delivered");
    assert_eq!(
        runtime.status(&identity).await.expect("status").state,
        IdentityLifecycleState::Active
    );
    assert_eq!(bridge.materializations.load(Ordering::SeqCst), 1);
    assert_eq!(
        bridge.ingress.lock().unwrap().len() + bridge.tracked.lock().unwrap().len(),
        1,
        "delivered once"
    );
}
