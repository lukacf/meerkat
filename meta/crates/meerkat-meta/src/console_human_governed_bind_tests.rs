//! Bind-level tests of the Console's per-send context on a member that
//! admits governed input: the real scoped grant composition of
//! `tests/support/governed_mob.rs` behind the identity runtime (with the
//! session owner's controller pin source), driven through the Console's
//! governed send path with a verified Console human.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../tests/support/governed_mob.rs"]
mod governed_support;

#[path = "console_governed_losing_bridge.rs"]
mod losing;

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use governed_support::{Fixture, MEMBER, association, evidence, principal};
use meerkat_authorization_contracts::work_association::InputAuthorityAssociation;
use meerkat_core::{ContentInput, SessionId};
use serde_json::json;

use crate::console_aggregator::{
    ConsoleFrameStatus, ConsoleInteractionAccepted, ConsoleSendError, ConsoleSendRequest,
    ConsoleTimelineQuery, MetaConsoleAggregator,
};
use crate::console_governed_ingress::{ConsoleGovernedIngress, ConsoleSendContext};
use crate::identity_first::{
    AgentAddressability, AgentIdentity, BoundGovernedInput, DurabilityPolicy, DurableAgentSpec,
    GovernedAdmission, GovernedBindContext, GovernedInputBinder, GovernedSettlement,
    IdentityRuntime, IdentityRuntimeConfig, LocalContinuityStore, LocalLeaseProvider,
    MobSessionBridge, SessionBridge,
};
use crate::runtime::AuthenticatedConsoleHuman;

const WAIT: Duration = Duration::from_secs(30);
const HUMAN: &str = "Please summarize the reviewed request.";

/// The identity runtime over the fixture's governed member, as the
/// governed-ingress fixture builds it: the session owner pins the actor's
/// controller, and the member is registered Active on its session.
async fn governed_identity_runtime(fixture: &Fixture) -> (Arc<IdentityRuntime>, AgentIdentity) {
    governed_identity_runtime_as(fixture, AgentAddressability::Addressable).await
}

/// [`governed_identity_runtime`] with the identity registered `addressability`.
async fn governed_identity_runtime_as(
    fixture: &Fixture,
    addressability: AgentAddressability,
) -> (Arc<IdentityRuntime>, AgentIdentity) {
    governed_identity_runtime_with(addressability, Arc::new(mob_bridge(fixture))).await
}

/// The fixture's mob session bridge, its session owner pinning the actor's
/// controller.
fn mob_bridge(fixture: &Fixture) -> MobSessionBridge {
    MobSessionBridge::new(fixture.handle.clone())
        .with_controller_pin_source(fixture.service.clone())
}

/// [`governed_identity_runtime`] whose session bridge loses a governed
/// delivery's answer (`lose`), counting in `native_accepted` the real native
/// admissions of new work it saw before it did.
async fn governed_identity_runtime_losing(
    fixture: &Fixture,
    lose: losing::LostAnswer,
    native_accepted: Arc<AtomicUsize>,
) -> (Arc<IdentityRuntime>, AgentIdentity) {
    governed_identity_runtime_with(
        AgentAddressability::Addressable,
        Arc::new(losing::LosingBridge {
            inner: mob_bridge(fixture),
            lose,
            native_accepted,
        }),
    )
    .await
}

/// [`governed_identity_runtime_as`] over `bridge`.
async fn governed_identity_runtime_with(
    addressability: AgentAddressability,
    bridge: Arc<dyn SessionBridge>,
) -> (Arc<IdentityRuntime>, AgentIdentity) {
    let runtime = Arc::new(IdentityRuntime::new(IdentityRuntimeConfig {
        continuity_store: Arc::new(LocalContinuityStore::in_memory().expect("continuity store")),
        lease_provider: Arc::new(LocalLeaseProvider::new()),
        runtime_instance_id: "console-governed-bind".to_string(),
        has_runtime_store: true,
        durability_policy: DurabilityPolicy::SyncWriteThrough,
        bridge: Some(bridge),
        default_timeout: None,
    }));
    // Core's journal binds the fixture's governed member (spawned under the
    // identity's own roster id): the runtime reads it from there.
    let identity = AgentIdentity::parse(MEMBER).unwrap();
    runtime
        .register(DurableAgentSpec {
            identity: identity.clone(),
            profile: meerkat_mob::ProfileName::from("worker"),
            addressability,
            display_name: None,
            labels: BTreeMap::new(),
            context: None,
            additional_instructions: Vec::new(),
            initial_message: None,
            runtime_mode_override: Some(meerkat_mob::MobRuntimeMode::TurnDriven),
            backend: None,
            binding: None,
            placement: None,
        })
        .await
        .expect("the identity registers");
    (runtime, identity)
}

/// What the application reviewed before the human sent it, keyed by the
/// send's client key: the exact content and the member session it was
/// reviewed for.
#[derive(Clone)]
struct Review {
    content: String,
    session_id: SessionId,
}

/// What a reviewed send's binder saw at bind.
#[derive(Debug, Clone, PartialEq)]
struct SeenAtBind {
    /// The send key the hook built the binder for.
    send_key: String,
    /// The session the runtime bound for.
    session_id: SessionId,
    /// The input's own header key.
    input_key: Option<String>,
    /// The input arrived raw (no association, no ingress context yet).
    raw: bool,
}

/// An application host that pins each Console send to the original it
/// reviewed: the per-send hook finds the review by the send's client key
/// (a correlation handle only), and the binder checks the bound session and
/// the actual content against it at bind, records the actual input once
/// (its first-bind compare-and-set) and binds that stock input.
struct ReviewingHost {
    fixture: Arc<Fixture>,
    reviews: Mutex<HashMap<String, Review>>,
    seen: Arc<Mutex<Vec<SeenAtBind>>>,
    bound: Arc<Mutex<Option<meerkat_core::lifecycle::InputId>>>,
    admitted: Arc<Mutex<Vec<GovernedAdmission>>>,
    /// While set, every bind answers "not now" (the source is held).
    held: Arc<AtomicBool>,
    /// The binders' wait budget, when the host sets one.
    budget: Arc<Mutex<Option<Duration>>>,
    /// The input id each bind was handed, in order.
    ids: Arc<Mutex<Vec<meerkat_core::lifecycle::InputId>>>,
}

impl ReviewingHost {
    fn new(fixture: &Arc<Fixture>, reviews: &[(&str, Review)]) -> Arc<Self> {
        Arc::new(Self {
            fixture: Arc::clone(fixture),
            reviews: Mutex::new(
                reviews
                    .iter()
                    .map(|(key, review)| ((*key).to_string(), review.clone()))
                    .collect(),
            ),
            seen: Arc::default(),
            bound: Arc::default(),
            admitted: Arc::default(),
            held: Arc::default(),
            budget: Arc::default(),
            ids: Arc::default(),
        })
    }

    /// Hold the source: every bind answers "not now" until `release`,
    /// within `budget`.
    fn hold(&self, budget: Duration) {
        *self.budget.lock().unwrap() = Some(budget);
        self.held.store(true, Ordering::SeqCst);
    }

    fn release(&self) {
        self.held.store(false, Ordering::SeqCst);
    }
}

impl ConsoleGovernedIngress for ReviewingHost {
    fn governs(&self, _identity: &AgentIdentity) -> bool {
        true
    }

    fn binder_for(
        &self,
        _identity: &AgentIdentity,
        _human: &AuthenticatedConsoleHuman,
    ) -> Result<Arc<dyn GovernedInputBinder>, String> {
        Err("this host binds a Console send only with its per-send context".to_string())
    }

    fn binder_for_send(
        &self,
        _identity: &AgentIdentity,
        _human: &AuthenticatedConsoleHuman,
        send: &ConsoleSendContext,
    ) -> Result<Arc<dyn GovernedInputBinder>, String> {
        let review = self
            .reviews
            .lock()
            .unwrap()
            .get(&send.idempotency_key)
            .cloned()
            .ok_or_else(|| format!("{} was not reviewed", send.idempotency_key))?;
        Ok(Arc::new(ReviewedBinder {
            fixture: Arc::clone(&self.fixture),
            send_key: send.idempotency_key.clone(),
            review,
            seen: Arc::clone(&self.seen),
            bound: Arc::clone(&self.bound),
            admitted: Arc::clone(&self.admitted),
            held: Arc::clone(&self.held),
            budget: *self.budget.lock().unwrap(),
            ids: Arc::clone(&self.ids),
        }))
    }
}

struct ReviewedBinder {
    fixture: Arc<Fixture>,
    send_key: String,
    review: Review,
    seen: Arc<Mutex<Vec<SeenAtBind>>>,
    bound: Arc<Mutex<Option<meerkat_core::lifecycle::InputId>>>,
    admitted: Arc<Mutex<Vec<GovernedAdmission>>>,
    held: Arc<AtomicBool>,
    budget: Option<Duration>,
    ids: Arc<Mutex<Vec<meerkat_core::lifecycle::InputId>>>,
}

#[async_trait::async_trait]
impl GovernedInputBinder for ReviewedBinder {
    async fn try_bind(
        &self,
        context: &GovernedBindContext,
        input: meerkat_runtime::Input,
    ) -> Result<BoundGovernedInput, crate::identity_first::GovernedBindError> {
        self.ids.lock().unwrap().push(input.id().clone());
        if self.held.load(Ordering::SeqCst) {
            let header = input.header();
            self.seen.lock().unwrap().push(SeenAtBind {
                send_key: self.send_key.clone(),
                session_id: context.session_id.clone(),
                input_key: header.idempotency_key.as_ref().map(|key| key.0.clone()),
                raw: header.authority_association.is_none() && header.ingress_context.is_none(),
            });
            let held = Arc::clone(&self.held);
            return Err(crate::identity_first::GovernedBindError::NotNow(
                crate::identity_first::GovernedNotNow {
                    condition: meerkat_core::authorization::ControllerReadinessFailure::SourceHeld,
                    until: meerkat_core::authorization::NotNowWait::until_clear(move || {
                        held.load(Ordering::SeqCst)
                    }),
                },
            ));
        }
        self.bind_reviewed(context, input)
            .await
            .map_err(|detail| crate::identity_first::GovernedBindRefusal::denied(detail).into())
    }

    fn wait_budget(&self) -> crate::identity_first::GovernedWaitBudget {
        match self.budget {
            Some(budget) => crate::identity_first::GovernedWaitBudget::Bounded(budget),
            None => crate::identity_first::GovernedWaitBudget::Bounded(
                meerkat_core::authorization::OWNER_CHANGE_WAIT_BOUND,
            ),
        }
    }

    fn retain_unresolved(&self, _settlement: Box<dyn GovernedSettlement>, _detail: &str) {}
}

impl ReviewedBinder {
    async fn bind_reviewed(
        &self,
        context: &GovernedBindContext,
        input: meerkat_runtime::Input,
    ) -> Result<BoundGovernedInput, String> {
        let header = input.header();
        let input_key = header.idempotency_key.clone();
        self.seen.lock().unwrap().push(SeenAtBind {
            send_key: self.send_key.clone(),
            session_id: context.session_id.clone(),
            input_key: input_key.as_ref().map(|key| key.0.clone()),
            raw: header.authority_association.is_none() && header.ingress_context.is_none(),
        });
        if context.session_id != self.review.session_id {
            return Err("the send is bound for another session than the reviewed one".to_string());
        }
        let reviewed_content = matches!(
            &input,
            meerkat_runtime::Input::Prompt(prompt)
                if matches!(&prompt.content, ContentInput::Text(text) if *text == self.review.content)
        );
        if !reviewed_content {
            return Err("the content is not the reviewed content".to_string());
        }
        {
            let mut bound = self.bound.lock().unwrap();
            if bound.is_some() {
                return Err("the reviewed original was already bound".to_string());
            }
            *bound = Some(input.id().clone());
        }
        let key = input_key.ok_or("the stock Console input carries its key")?;
        let candidate = association(
            &self.fixture.runtime,
            self.fixture.controller.clone(),
            self.fixture.operation.clone(),
            context.controller.selection().clone(),
        );
        let bound = meerkat_runtime::meerkat_machine::bind_scoped_original(
            input,
            InputAuthorityAssociation::new(candidate).map_err(|error| format!("{error:?}"))?,
            key,
            principal("requester"),
            principal("ingress"),
            meerkat_core::connection::RealmId::parse("governed-mob").expect("realm"),
            evidence("original-authentication"),
            context.controller.clone(),
        )
        .map_err(|error| format!("{error:?}"))?;
        Ok(BoundGovernedInput {
            input: bound,
            settlement: Box::new(RecordedSettlement(Arc::clone(&self.admitted))),
            entry_fence: None,
        })
    }
}

struct RecordedSettlement(Arc<Mutex<Vec<GovernedAdmission>>>);

impl GovernedSettlement for RecordedSettlement {
    fn accepted(self: Box<Self>, admission: &GovernedAdmission) -> Result<(), String> {
        self.0.lock().unwrap().push(admission.clone());
        Ok(())
    }
}

fn request(key: &str, content: &str) -> ConsoleSendRequest {
    ConsoleSendRequest {
        identity: MEMBER.to_string(),
        content: json!(content),
        origin: "console".to_string(),
        idempotency_key: key.to_string(),
        handling_mode: None,
        origin_kind: None,
        skill_refs: Vec::new(),
    }
}

/// Send through the Console's governed send path (the one `/console/send`
/// runs after its gate) as the verified human, with `host` installed.
async fn send(
    aggregator: &MetaConsoleAggregator,
    runtime: &Arc<IdentityRuntime>,
    host: &Arc<ReviewingHost>,
    request: ConsoleSendRequest,
) -> Result<ConsoleInteractionAccepted, ConsoleSendError> {
    let governance = super::ConsoleSendGovernance {
        ingress: Some(host.clone()),
        human: Some(super::console_human_input_tests::current_console_human()),
        ..Default::default()
    };
    super::console_send_governed_with_identity_first_fallback(
        aggregator,
        Arc::clone(runtime),
        None,
        request,
        &governance,
    )
    .await
    .map_err(super::CheckedConsoleSendError::into_send_error)
}

/// The frame's settled delivery status.
async fn settled(
    aggregator: &MetaConsoleAggregator,
    accepted: &ConsoleInteractionAccepted,
) -> ConsoleFrameStatus {
    tokio::time::timeout(WAIT, async {
        loop {
            let page = aggregator
                .query_timeline(ConsoleTimelineQuery {
                    identity: Some(accepted.identity.clone()),
                    ..Default::default()
                })
                .await
                .unwrap();
            if let Some(status) = page
                .frames
                .iter()
                .find(|frame| frame.id == accepted.input_frame_id)
                .map(|frame| frame.status)
                .filter(|status| {
                    matches!(
                        status,
                        ConsoleFrameStatus::Delivered | ConsoleFrameStatus::DeliveryFailed
                    )
                })
            {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("console delivery settles")
}

fn stock_key(accepted: &ConsoleInteractionAccepted) -> Option<String> {
    Some(format!(
        "console:{}:{}",
        accepted.interaction_id, accepted.input_frame_id
    ))
}

/// A binder the per-send hook pinned to a reviewed original refuses at bind
/// when the bound session or the actual content is not the reviewed one:
/// the send was accepted, its frame fails, and nothing is admitted or
/// reaches the model. A key with no review is refused by the hook itself,
/// before anything is reserved.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn console_human_governed_bind_refuses_a_send_that_is_not_the_reviewed_original() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("warn"))
        .with_test_writer()
        .try_init();
    let fixture = Arc::new(Fixture::new().await);
    let (runtime, _identity) = governed_identity_runtime(&fixture).await;
    let aggregator = MetaConsoleAggregator::in_memory();
    let host = ReviewingHost::new(
        &fixture,
        &[
            (
                "reviewed-content",
                Review {
                    content: "another reviewed text".to_string(),
                    session_id: fixture.session_id.clone(),
                },
            ),
            (
                "reviewed-session",
                Review {
                    content: HUMAN.to_string(),
                    session_id: SessionId::new(),
                },
            ),
        ],
    );

    let content = send(
        &aggregator,
        &runtime,
        &host,
        request("reviewed-content", HUMAN),
    )
    .await
    .expect("accepted for the verified human");
    assert_eq!(
        settled(&aggregator, &content).await,
        ConsoleFrameStatus::DeliveryFailed
    );
    let session = send(
        &aggregator,
        &runtime,
        &host,
        request("reviewed-session", HUMAN),
    )
    .await
    .expect("accepted for the verified human");
    assert_eq!(
        settled(&aggregator, &session).await,
        ConsoleFrameStatus::DeliveryFailed
    );
    assert_eq!(
        *host.seen.lock().unwrap(),
        [
            SeenAtBind {
                send_key: "reviewed-content".to_string(),
                session_id: fixture.session_id.clone(),
                input_key: stock_key(&content),
                raw: true,
            },
            SeenAtBind {
                send_key: "reviewed-session".to_string(),
                session_id: fixture.session_id.clone(),
                input_key: stock_key(&session),
                raw: true,
            },
        ],
        "both refusals came from the reviewed binder, at bind, on the stock input"
    );

    let unreviewed = send(&aggregator, &runtime, &host, request("unreviewed", HUMAN)).await;
    assert!(
        matches!(
            &unreviewed,
            Err(ConsoleSendError::GovernedRefused(reason)) if reason == "unreviewed was not reviewed"
        ),
        "{unreviewed:?}"
    );
    assert_eq!(host.seen.lock().unwrap().len(), 2, "no bind for it");

    assert!(host.bound.lock().unwrap().is_none(), "nothing was bound");
    assert!(host.admitted.lock().unwrap().is_empty(), "nothing admitted");
    assert!(
        fixture.provider.requests().is_empty(),
        "nothing reached the model"
    );
}

/// The reviewed send binds: the binder sees the stock Console input (raw,
/// the Console's own `console:{interaction}:{frame}` key) for the member's
/// bound session, records its actual `InputId` once, and the member's
/// native admission admits exactly that input, whose turn then reaches the
/// model with the human's content.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn console_human_governed_bind_admits_the_reviewed_stock_input() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("warn"))
        .with_test_writer()
        .try_init();
    let fixture = Arc::new(Fixture::new().await);
    let (runtime, _identity) = governed_identity_runtime(&fixture).await;
    let aggregator = MetaConsoleAggregator::in_memory();
    let host = ReviewingHost::new(
        &fixture,
        &[(
            "reviewed-original",
            Review {
                content: HUMAN.to_string(),
                session_id: fixture.session_id.clone(),
            },
        )],
    );

    let accepted = send(
        &aggregator,
        &runtime,
        &host,
        request("reviewed-original", HUMAN),
    )
    .await
    .expect("accepted for the verified human");
    assert_eq!(
        settled(&aggregator, &accepted).await,
        ConsoleFrameStatus::Delivered
    );
    assert_eq!(
        *host.seen.lock().unwrap(),
        [SeenAtBind {
            send_key: "reviewed-original".to_string(),
            session_id: fixture.session_id.clone(),
            input_key: stock_key(&accepted),
            raw: true,
        }],
        "the binder saw the stock Console input for the member's session"
    );
    let bound = host.bound.lock().unwrap().clone().expect("recorded once");
    let admitted = host.admitted.lock().unwrap().clone();
    assert_eq!(admitted.len(), 1, "{admitted:?}");
    assert_eq!(
        admitted[0].input_id, bound,
        "native admission admitted exactly the stock input the binder recorded"
    );
    assert_eq!(admitted[0].session_id, fixture.session_id);
    assert!(!admitted[0].deduplicated);
    governed_support::wait_until("the admitted turn reaches the model", || {
        !fixture.provider.requests().is_empty()
    })
    .await;
    assert!(
        fixture
            .provider
            .requests()
            .iter()
            .any(|messages| { serde_json::to_string(messages).unwrap().contains(HUMAN) }),
        "the model saw the human's content"
    );
}

/// The Console's public send selects no governed door and no work origin. A
/// request body that also names the trusted host's dispatch door or internal
/// work deserializes to the same Console send (fields it does not define are
/// ignored; `origin` only labels the frame), so to an internal-only identity
/// on a member that admits no external work it is refused `NotAddressable`
/// before the host's binder binds anything, in steer (answered at once) and
/// in queue (its frame fails); nothing enters native admission or reaches
/// the model.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn console_human_governed_send_naming_the_dispatch_door_is_refused_for_an_internal_only_identity()
 {
    let fixture = Arc::new(Fixture::non_addressable().await);
    let (runtime, _identity) =
        governed_identity_runtime_as(&fixture, AgentAddressability::InternalOnly).await;
    let aggregator = MetaConsoleAggregator::in_memory();
    let review = Review {
        content: HUMAN.to_string(),
        session_id: fixture.session_id.clone(),
    };
    let host = ReviewingHost::new(
        &fixture,
        &[("door-steer", review.clone()), ("door-queue", review)],
    );
    let body = |key: &str, mode: &str| -> ConsoleSendRequest {
        serde_json::from_value(json!({
            "identity": MEMBER,
            "content": HUMAN,
            "origin": "dispatch",
            "idempotency_key": key,
            "handling_mode": mode,
            "door": "dispatch",
            "governed_door": "trusted_host_internal",
            "work_origin": "internal",
            "dispatch": true,
        }))
        .expect("the Console send body ignores fields it does not define")
    };

    let steer = send(&aggregator, &runtime, &host, body("door-steer", "steer")).await;
    assert!(
        matches!(&steer, Err(ConsoleSendError::NotAddressable(identity)) if identity == MEMBER),
        "{steer:?}"
    );
    let queued = send(&aggregator, &runtime, &host, body("door-queue", "queue"))
        .await
        .expect("accepted for the verified human");
    assert_eq!(
        settled(&aggregator, &queued).await,
        ConsoleFrameStatus::DeliveryFailed
    );

    assert!(
        host.seen.lock().unwrap().is_empty(),
        "the binder never bound"
    );
    assert!(host.bound.lock().unwrap().is_none(), "nothing was bound");
    assert!(host.admitted.lock().unwrap().is_empty(), "nothing admitted");
    assert_eq!(
        fixture.adapter.gate.entered(),
        0,
        "no native admission was entered"
    );
    assert!(
        fixture.provider.requests().is_empty(),
        "nothing reached the model"
    );
}

/// The wait budget a held Console send is given in these rows.
const HOLD_BUDGET: Duration = Duration::from_millis(300);

/// The send frame once its dispatch answered and ended: held (`accepted`
/// with a `delivery_hold`, its dispatch owner gone) or settled (`delivered`,
/// `delivery_failed`).
async fn answered_frame(
    aggregator: &MetaConsoleAggregator,
    accepted: &ConsoleInteractionAccepted,
) -> crate::console_aggregator::ConsoleFrame {
    tokio::time::timeout(WAIT, async {
        loop {
            let page = aggregator
                .query_timeline(ConsoleTimelineQuery {
                    identity: Some(accepted.identity.clone()),
                    ..Default::default()
                })
                .await
                .unwrap();
            if let Some(frame) = page
                .frames
                .into_iter()
                .find(|frame| frame.id == accepted.input_frame_id)
                .filter(|frame| {
                    (frame.payload.get("delivery_hold").is_some()
                        && !aggregator.send_dispatch_claimed(&frame.id))
                        || matches!(
                            frame.status,
                            ConsoleFrameStatus::Delivered | ConsoleFrameStatus::DeliveryFailed
                        )
                })
            {
                return frame;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the console dispatch answers")
}

/// A governed Console send whose every bind answers "not now"
/// until its budget ends is held, not failed: its frame stays `accepted`
/// with a `delivery_hold` (the condition, retryable), and nothing is
/// submitted. Sending the same Console key again once the source clears
/// dispatches the same input (the same id and key) and admits it once; the
/// frame is delivered.
///
/// RED before the fix: the held send marks the frame `delivery_failed`, and the
/// replay replays the failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn console_human_governed_send_held_past_its_budget_is_held_then_replayed_once() {
    let fixture = Arc::new(Fixture::new().await);
    let (runtime, _identity) = governed_identity_runtime(&fixture).await;
    let aggregator = MetaConsoleAggregator::in_memory();
    let host = ReviewingHost::new(
        &fixture,
        &[(
            "held-original",
            Review {
                content: HUMAN.to_string(),
                session_id: fixture.session_id.clone(),
            },
        )],
    );
    host.hold(HOLD_BUDGET);

    let accepted = send(
        &aggregator,
        &runtime,
        &host,
        request("held-original", HUMAN),
    )
    .await
    .expect("accepted for the verified human");
    let held = answered_frame(&aggregator, &accepted).await;
    assert_eq!(
        held.status,
        ConsoleFrameStatus::Accepted,
        "a held send is not failed: {held:?}"
    );
    let hold = &held.payload["delivery_hold"];
    assert_eq!(hold["condition"], "source_held", "{hold}");
    assert_eq!(hold["retryable"], true, "{hold}");
    assert!(
        hold["binds"].as_u64().is_some_and(|binds| binds >= 1),
        "{hold}"
    );
    assert!(host.admitted.lock().unwrap().is_empty(), "nothing admitted");
    assert!(host.bound.lock().unwrap().is_none(), "nothing bound");

    // The source clears; the human sends the same key again.
    host.release();
    let replayed = send(
        &aggregator,
        &runtime,
        &host,
        request("held-original", HUMAN),
    )
    .await
    .expect("the replay is accepted");
    assert_eq!(replayed.input_frame_id, accepted.input_frame_id);
    assert_eq!(replayed.interaction_id, accepted.interaction_id);
    assert_eq!(
        settled(&aggregator, &replayed).await,
        ConsoleFrameStatus::Delivered
    );
    let admitted = host.admitted.lock().unwrap().clone();
    assert_eq!(admitted.len(), 1, "admitted once: {admitted:?}");
    let ids = host.ids.lock().unwrap().clone();
    assert!(
        ids.len() >= 2 && ids.iter().all(|id| *id == ids[0]),
        "every bind was handed the same input id: {ids:?}"
    );
    assert_eq!(admitted[0].input_id, ids[0]);
    assert!(
        host.seen
            .lock()
            .unwrap()
            .iter()
            .all(|seen| seen.input_key == stock_key(&accepted)),
        "every bind saw the send's own key"
    );
}

/// The binds the host's binders were handed, every send's.
fn bind_count(host: &ReviewingHost) -> usize {
    host.ids.lock().unwrap().len()
}

/// Admission evidence the store does not give: every read fails, as a
/// store read error does (a read past its bound answers the same).
struct UnreadableEvidence;

#[async_trait::async_trait]
impl crate::console_aggregator::SendAdmissionReader for UnreadableEvidence {
    async fn admitted_input(
        &self,
        _session_id: &SessionId,
        _idempotency_key: &str,
    ) -> Result<Option<crate::console_aggregator::AdmittedInput>, String> {
        Err("the console store did not answer".to_string())
    }

    async fn admitted_input_by_id(
        &self,
        _session_id: &SessionId,
        _input_id: &meerkat_core::lifecycle::InputId,
    ) -> Result<Option<crate::console_aggregator::AdmittedInput>, String> {
        Err("the console store did not answer".to_string())
    }
}

/// A held governed send to `key`: sent while the source is held, its
/// dispatch answered held. Answers its acceptance and its held frame.
async fn held_send(
    aggregator: &MetaConsoleAggregator,
    runtime: &Arc<IdentityRuntime>,
    host: &Arc<ReviewingHost>,
    key: &str,
) -> (
    ConsoleInteractionAccepted,
    crate::console_aggregator::ConsoleFrame,
) {
    host.hold(HOLD_BUDGET);
    let accepted = send(aggregator, runtime, host, request(key, HUMAN))
        .await
        .expect("accepted for the verified human");
    let held = answered_frame(aggregator, &accepted).await;
    assert_eq!(held.status, ConsoleFrameStatus::Accepted, "{held:?}");
    assert!(held.payload.get("delivery_hold").is_some(), "{held:?}");
    (accepted, held)
}

fn reviewed(fixture: &Fixture, key: &'static str) -> [(&'static str, Review); 1] {
    [(
        key,
        Review {
            content: HUMAN.to_string(),
            session_id: fixture.session_id.clone(),
        },
    )]
}

/// A held send whose frame recorded no session, as the held
/// first send of an identity with no Core binding yet leaves it (the
/// Console records the identity's session at acceptance; a lazily created
/// identity's member is seated inside the dispatch). Its replay can read no
/// admission evidence: the send names no session to read. Its hold still
/// records that nothing was submitted, so the replay dispatches the same
/// input again: the member's one session binds it, it is admitted once and
/// delivered, and the identity stays on that session. The held frame is
/// built through the Console's own reservation (no session) and hold mark,
/// as the held dispatch leaves it; the replay runs the real send path, with
/// the runtime's admission evidence readable.
///
/// RED before the fix: the replay returns the acceptance and dispatches
/// nothing; the send stays accepted with its hold.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_held_send_that_recorded_no_session_is_dispatched_again_on_replay() {
    let fixture = Arc::new(Fixture::new().await);
    let (runtime, identity) = governed_identity_runtime(&fixture).await;
    let aggregator = MetaConsoleAggregator::in_memory();
    aggregator.read_send_admission_from(fixture.machine.clone());
    let host = ReviewingHost::new(&fixture, &reviewed(&fixture, "unbound-original"));
    let scope = super::ConsoleSendGovernance::default().scope;
    let accepted = match aggregator
        .reserve_identity_first_interaction_scoped(
            request("unbound-original", HUMAN),
            None,
            &scope,
            None,
        )
        .await
        .expect("the Console reserves the send")
    {
        crate::console_aggregator::ScopedReservation::Fresh(accepted, claim) => {
            // The held dispatch ended: its owner is gone.
            drop(claim);
            accepted
        }
        other => panic!("a fresh reservation: {other:?}"),
    };
    assert_eq!(accepted.session_id, None, "the send recorded no session");
    // Its dispatch began, and was held before it submitted anything.
    aggregator
        .mark_interaction_dispatching(&accepted.input_frame_id)
        .await
        .expect("the dispatch begins");
    aggregator
        .mark_interaction_delivery_held(
            &accepted.input_frame_id,
            &crate::identity_first::GovernedHold {
                condition: meerkat_core::authorization::ControllerReadinessFailure::SourceHeld,
                waited: HOLD_BUDGET,
                binds: 1,
                input_id: meerkat_core::lifecycle::InputId::new(),
                idempotency_key: None,
            },
        )
        .await
        .expect("the hold is recorded");

    let replayed = send(
        &aggregator,
        &runtime,
        &host,
        request("unbound-original", HUMAN),
    )
    .await
    .expect("the replay is accepted");
    assert_eq!(replayed.input_frame_id, accepted.input_frame_id);
    assert_eq!(
        settled(&aggregator, &replayed).await,
        ConsoleFrameStatus::Delivered,
        "the replay of the held send settles"
    );
    let admitted = host.admitted.lock().unwrap().clone();
    assert_eq!(admitted.len(), 1, "admitted once: {admitted:?}");
    let seen = host.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "one dispatch bound it: {seen:?}");
    assert_eq!(seen[0].session_id, fixture.session_id);
    assert_eq!(seen[0].input_key, stock_key(&accepted));
    let status = runtime.status(&identity).await.expect("status");
    assert_eq!(
        status.session_id.as_ref(),
        Some(&fixture.session_id),
        "the member was seated once: its one session"
    );
}

/// A held send whose replay cannot read the runtime's
/// admission evidence (a store read error; a read past its bound answers
/// the same) is dispatched again: its hold records that nothing was
/// submitted since its held dispatch, and Core admits its keyed input at
/// most once. It is delivered and admitted once, and a replay of the
/// delivered send dispatches nothing.
///
/// RED before the fix: the replay dispatches nothing and the send stays
/// accepted with its hold.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_held_send_whose_evidence_cannot_be_read_is_dispatched_again_once() {
    let fixture = Arc::new(Fixture::new().await);
    let (runtime, _identity) = governed_identity_runtime(&fixture).await;
    let aggregator = MetaConsoleAggregator::in_memory();
    aggregator.read_send_admission_from(Arc::new(UnreadableEvidence));
    let host = ReviewingHost::new(&fixture, &reviewed(&fixture, "unread-original"));
    let (accepted, _held) = held_send(&aggregator, &runtime, &host, "unread-original").await;

    host.release();
    let replayed = send(
        &aggregator,
        &runtime,
        &host,
        request("unread-original", HUMAN),
    )
    .await
    .expect("the replay is accepted");
    assert_eq!(replayed.input_frame_id, accepted.input_frame_id);
    assert_eq!(
        settled(&aggregator, &replayed).await,
        ConsoleFrameStatus::Delivered
    );
    assert_eq!(host.admitted.lock().unwrap().len(), 1, "admitted once");
    let binds = bind_count(&host);
    send(
        &aggregator,
        &runtime,
        &host,
        request("unread-original", HUMAN),
    )
    .await
    .expect("a replay of the delivered send");
    tokio::time::sleep(HOLD_BUDGET).await;
    assert_eq!(
        bind_count(&host),
        binds,
        "a replay of the delivered send dispatches nothing"
    );
    assert_eq!(
        host.admitted.lock().unwrap().len(),
        1,
        "still admitted once"
    );
}

/// A held send replayed while no registered runtime serves
/// its frame (no admission evidence at all) and its source is still held is
/// dispatched again once: that dispatch's binds answer "not now" until its
/// budget ends, so the send is held again, its new hold recorded, and
/// nothing is submitted. Nothing dispatches it after that without another
/// replay: no loop.
///
/// RED before the fix: the replay dispatches nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_held_send_replayed_without_evidence_while_still_held_is_held_again_once() {
    let fixture = Arc::new(Fixture::new().await);
    let (runtime, _identity) = governed_identity_runtime(&fixture).await;
    let aggregator = MetaConsoleAggregator::in_memory();
    let host = ReviewingHost::new(&fixture, &reviewed(&fixture, "still-held-original"));
    let (accepted, first) = held_send(&aggregator, &runtime, &host, "still-held-original").await;
    let before = bind_count(&host);

    // The source is still held; the human sends the same key again.
    let replayed = send(
        &aggregator,
        &runtime,
        &host,
        request("still-held-original", HUMAN),
    )
    .await
    .expect("the replay is accepted");
    assert_eq!(replayed.input_frame_id, accepted.input_frame_id);
    let again = answered_frame(&aggregator, &replayed).await;
    let after = bind_count(&host);
    assert!(
        after > before,
        "the replay dispatched the send once more: {before} -> {after} binds"
    );
    assert_eq!(
        again.status,
        ConsoleFrameStatus::Accepted,
        "held again, not failed: {again:?}"
    );
    assert_eq!(again.payload["delivery_hold"]["condition"], "source_held");
    assert!(
        again.frame_version > first.frame_version,
        "the new hold is recorded: {first:?} then {again:?}"
    );
    assert!(host.admitted.lock().unwrap().is_empty(), "nothing admitted");
    assert!(host.bound.lock().unwrap().is_none(), "nothing bound");

    tokio::time::sleep(HOLD_BUDGET * 3).await;
    assert_eq!(
        bind_count(&host),
        after,
        "nothing dispatches the held send without a replay"
    );
    host.release();
}

/// Two replays of the same held send at the same moment
/// dispatch it once: one becomes the frame's dispatch owner and takes its
/// hold, the other answers the acceptance and dispatches nothing. One bind,
/// one admission, delivered.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_replays_of_a_held_send_dispatch_it_once() {
    let fixture = Arc::new(Fixture::new().await);
    let (runtime, _identity) = governed_identity_runtime(&fixture).await;
    let aggregator = MetaConsoleAggregator::in_memory();
    aggregator.read_send_admission_from(fixture.machine.clone());
    let host = ReviewingHost::new(&fixture, &reviewed(&fixture, "raced-original"));
    let (accepted, _held) = held_send(&aggregator, &runtime, &host, "raced-original").await;

    host.release();
    let seen_before = host.seen.lock().unwrap().len();
    let (first, second) = tokio::join!(
        send(
            &aggregator,
            &runtime,
            &host,
            request("raced-original", HUMAN)
        ),
        send(
            &aggregator,
            &runtime,
            &host,
            request("raced-original", HUMAN)
        ),
    );
    let first = first.expect("the first replay is accepted");
    let second = second.expect("the second replay is accepted");
    assert_eq!(first.input_frame_id, accepted.input_frame_id);
    assert_eq!(second.input_frame_id, accepted.input_frame_id);
    assert_eq!(
        settled(&aggregator, &first).await,
        ConsoleFrameStatus::Delivered
    );
    tokio::time::sleep(HOLD_BUDGET).await;
    let admitted = host.admitted.lock().unwrap().clone();
    assert_eq!(admitted.len(), 1, "admitted once: {admitted:?}");
    let seen = host.seen.lock().unwrap()[seen_before..].to_vec();
    assert_eq!(seen.len(), 1, "one dispatch bound it: {seen:?}");
}

/// A held send replayed after its identity was reset binds
/// the member's new session: exactly one dispatch, on that session, and
/// nothing admitted as the send's input on the old session, where nothing
/// was ever submitted. Core qualifies a governed key by its association,
/// which names the bound session, so a dispatch on the new session is not
/// deduplicated against the old one: the hold is the one record that
/// nothing was submitted. The host reviewed the send for the old session,
/// so it refuses the new one with its typed refusal and the send fails,
/// admitted nowhere: the send's input, by its id (derived from its frame),
/// is on neither session. The old session is read before the reset, while
/// it exists (only the held dispatch could have admitted there; the reset
/// destroys it), the new one after the replay.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_held_send_replayed_after_a_reset_dispatches_once_on_the_new_session() {
    let fixture = Arc::new(Fixture::new().await);
    let (runtime, identity) = governed_identity_runtime(&fixture).await;
    let aggregator = MetaConsoleAggregator::in_memory();
    aggregator.read_send_admission_from(fixture.machine.clone());
    let host = ReviewingHost::new(&fixture, &reviewed(&fixture, "reset-original"));
    let (accepted, _held) = held_send(&aggregator, &runtime, &host, "reset-original").await;
    assert_eq!(
        accepted.session_id,
        Some(fixture.session_id.to_string()),
        "the held send recorded the member's session"
    );
    let input_id = crate::console_aggregator::governed_input_id(
        &accepted.interaction_id,
        &accepted.input_frame_id,
    );
    let not_on = |session: SessionId| {
        let machine = Arc::clone(&fixture.machine);
        let input_id = input_id.clone();
        async move {
            let observed = machine
                .input_terminal_receipt_observation(
                    &session,
                    meerkat_runtime::terminal_status::InteractionSelector::InputId(
                        input_id.clone(),
                    ),
                )
                .await;
            assert!(
                matches!(
                    observed,
                    Ok(None) | Err(meerkat_runtime::RuntimeDriverError::NotFound { .. })
                ),
                "the send's input {input_id} is not on {session}: {observed:?}"
            );
        }
    };
    not_on(fixture.session_id.clone()).await;
    let fresh = runtime
        .reset(&identity)
        .await
        .expect("the reset rotates the member");
    let new_session = fresh.session_id().cloned().expect("the new session");
    assert_ne!(new_session, fixture.session_id);

    host.release();
    let seen_before = host.seen.lock().unwrap().len();
    let replayed = send(
        &aggregator,
        &runtime,
        &host,
        request("reset-original", HUMAN),
    )
    .await
    .expect("the replay is accepted");
    assert_eq!(replayed.input_frame_id, accepted.input_frame_id);
    assert_eq!(
        settled(&aggregator, &replayed).await,
        ConsoleFrameStatus::DeliveryFailed,
        "the host refuses the session it did not review"
    );
    let seen = host.seen.lock().unwrap()[seen_before..].to_vec();
    assert_eq!(seen.len(), 1, "one dispatch: {seen:?}");
    assert_eq!(seen[0].session_id, new_session, "bound on the new session");
    assert!(host.admitted.lock().unwrap().is_empty(), "admitted nowhere");
    let ids = host.ids.lock().unwrap().clone();
    assert!(
        ids.iter().all(|id| *id == input_id),
        "every bind was handed the send's one input, {input_id}: {ids:?}"
    );
    not_on(new_session).await;
}

/// A governed send whose input was admitted, its frame left
/// unsettled (the process that dispatched it stopped before it settled the
/// frame), is settled `delivered` by a replay of its key from the runtime's
/// evidence, and is not dispatched again. Core indexes a governed input
/// under its key as qualified by its association, so the replay reads the
/// input by its own id, which derives from the send's frame.
///
/// RED before the fix: the replay reads the raw keys, finds nothing, and
/// answers the acceptance: the send stays accepted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_admitted_governed_send_settles_delivered_on_replay_without_a_dispatch() {
    let fixture = Arc::new(Fixture::new().await);
    let (runtime, identity) = governed_identity_runtime(&fixture).await;
    let aggregator = MetaConsoleAggregator::in_memory();
    aggregator.read_send_admission_from(fixture.machine.clone());
    let host = ReviewingHost::new(&fixture, &reviewed(&fixture, "admitted-original"));
    let sent = request("admitted-original", HUMAN);
    // The Console accepted the send for the member's session ...
    let session = fixture.session_id.to_string();
    let scope = super::ConsoleSendGovernance::default().scope;
    let accepted = match aggregator
        .reserve_identity_first_interaction_scoped(
            sent.clone(),
            Some(session.as_str()),
            &scope,
            None,
        )
        .await
        .expect("the Console reserves the send")
    {
        crate::console_aggregator::ScopedReservation::Fresh(accepted, claim) => {
            drop(claim);
            accepted
        }
        other => panic!("a fresh reservation: {other:?}"),
    };
    // ... its dispatch admitted the input, and stopped before it settled
    // the frame.
    let binder = host
        .binder_for_send(
            &identity,
            &super::console_human_input_tests::current_console_human(),
            &ConsoleSendContext::for_request(&sent),
        )
        .expect("the host's binder for the send");
    runtime
        .send_console_human_input_tracked(
            &identity,
            None,
            &ContentInput::Text(HUMAN.to_string()),
            meerkat_core::types::HandlingMode::Queue,
            &[],
            &accepted,
            &crate::console_aggregator::ConsoleHumanAttribution::default(),
            Some(binder),
        )
        .await
        .expect("the input is admitted");
    assert_eq!(host.admitted.lock().unwrap().len(), 1, "admitted once");
    let binds = bind_count(&host);

    let replayed = send(&aggregator, &runtime, &host, sent.clone())
        .await
        .expect("the replay is accepted");
    assert_eq!(replayed.input_frame_id, accepted.input_frame_id);
    assert_eq!(
        settled(&aggregator, &replayed).await,
        ConsoleFrameStatus::Delivered,
        "the replay settles the admitted send from the evidence"
    );
    assert_eq!(bind_count(&host), binds, "no dispatch again");
    assert_eq!(
        host.admitted.lock().unwrap().len(),
        1,
        "still admitted once"
    );
}

/// Once a governed send's bind answered bound and its input
/// was submitted, a hold is never recorded over it. With its native
/// admission held in flight, marking the send held is refused and its frame
/// is left as it was (status, version, no hold). Released, the send is
/// delivered and admitted once.
///
/// RED before the fix: the mark records a hold over the submitted send.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_submitted_governed_send_records_no_hold() {
    let fixture = Arc::new(Fixture::new().await);
    let (runtime, _identity) = governed_identity_runtime(&fixture).await;
    let aggregator = MetaConsoleAggregator::in_memory();
    let host = ReviewingHost::new(&fixture, &reviewed(&fixture, "submitted-original"));
    fixture.adapter.gate.hold();
    let accepted = send(
        &aggregator,
        &runtime,
        &host,
        request("submitted-original", HUMAN),
    )
    .await
    .expect("accepted for the verified human");
    // The bind answered bound and the input was submitted: its native
    // admission is in flight, held.
    tokio::time::timeout(WAIT, async {
        while fixture.adapter.gate.entered() == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the native admission is in flight");
    let frame = |aggregator: &MetaConsoleAggregator| {
        let aggregator = aggregator.clone();
        let frame_id = accepted.input_frame_id.clone();
        let identity = accepted.identity.clone();
        async move {
            aggregator
                .query_timeline(ConsoleTimelineQuery {
                    identity: Some(identity),
                    ..Default::default()
                })
                .await
                .unwrap()
                .frames
                .into_iter()
                .find(|frame| frame.id == frame_id)
                .expect("the send's frame")
        }
    };
    let before = frame(&aggregator).await;
    let marked = aggregator
        .mark_interaction_delivery_held(
            &accepted.input_frame_id,
            &crate::identity_first::GovernedHold {
                condition: meerkat_core::authorization::ControllerReadinessFailure::SourceHeld,
                waited: HOLD_BUDGET,
                binds: 1,
                input_id: meerkat_core::lifecycle::InputId::new(),
                idempotency_key: None,
            },
        )
        .await;
    let after = frame(&aggregator).await;
    assert!(
        marked.is_err(),
        "a submitted send records no hold: {marked:?}"
    );
    assert_eq!(after.status, before.status);
    assert_eq!(after.frame_version, before.frame_version);
    assert!(after.payload.get("delivery_hold").is_none(), "{after:?}");

    fixture.adapter.gate.release();
    assert_eq!(
        settled(&aggregator, &accepted).await,
        ConsoleFrameStatus::Delivered
    );
    assert_eq!(host.admitted.lock().unwrap().len(), 1, "admitted once");
}

/// A governed send recorded submitted (its bind answered
/// bound) whose input never reached the runtime, because the process
/// stopped between that record and the submission, is settled by a replay
/// of its key. The runtime holds none of its input and no owner is left to
/// finish the submission, so it fails, its outcome unknown (its frame's
/// `delivery_failure`: `outcome_unknown`, retryable), and is never
/// dispatched again. The frame is built through the Console's own
/// reservation and send transitions, as such a stop leaves it.
///
/// RED with the settlement of a lost submission dropped: the replay answers
/// the acceptance and the send stays submitted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_submitted_send_whose_input_never_reached_the_runtime_fails_on_replay() {
    let fixture = Arc::new(Fixture::new().await);
    let (runtime, _identity) = governed_identity_runtime(&fixture).await;
    let aggregator = MetaConsoleAggregator::in_memory();
    aggregator.read_send_admission_from(fixture.machine.clone());
    let host = ReviewingHost::new(&fixture, &reviewed(&fixture, "lost-original"));
    let session = fixture.session_id.to_string();
    let scope = super::ConsoleSendGovernance::default().scope;
    let accepted = match aggregator
        .reserve_identity_first_interaction_scoped(
            request("lost-original", HUMAN),
            Some(session.as_str()),
            &scope,
            None,
        )
        .await
        .expect("the Console reserves the send")
    {
        crate::console_aggregator::ScopedReservation::Fresh(accepted, claim) => {
            // The process stopped: the dispatch owner is gone.
            drop(claim);
            accepted
        }
        other => panic!("a fresh reservation: {other:?}"),
    };
    aggregator
        .mark_interaction_dispatching(&accepted.input_frame_id)
        .await
        .expect("the dispatch begins");
    aggregator
        .mark_interaction_submitted(&accepted.input_frame_id, &fixture.session_id)
        .await
        .expect("the bind answered bound");
    let binds = bind_count(&host);

    let replayed = send(
        &aggregator,
        &runtime,
        &host,
        request("lost-original", HUMAN),
    )
    .await
    .expect("the replay is answered");
    assert_eq!(replayed.input_frame_id, accepted.input_frame_id);
    assert_eq!(
        settled(&aggregator, &replayed).await,
        ConsoleFrameStatus::DeliveryFailed,
        "a lost submission fails"
    );
    let frame = aggregator
        .query_timeline(ConsoleTimelineQuery {
            identity: Some(accepted.identity.clone()),
            ..Default::default()
        })
        .await
        .unwrap()
        .frames
        .into_iter()
        .find(|frame| frame.id == accepted.input_frame_id)
        .expect("the send's frame");
    assert_eq!(
        frame.payload["delivery_failure"],
        json!({"reason": "outcome_unknown", "retryable": true}),
        "{frame:?}"
    );
    assert_eq!(bind_count(&host), binds, "never dispatched again");
    assert!(host.admitted.lock().unwrap().is_empty(), "admitted nowhere");
}

/// The unresolved answer a governed send's frame records.
fn unresolved_record() -> serde_json::Value {
    json!({"reason": "admission_unresolved", "retryable": false})
}

/// The send frame once its dispatch answered and ended: settled
/// (`delivered`, `delivery_failed`), or left `submitted` with its
/// unresolved answer recorded, its dispatch owner gone.
async fn unresolved_frame(
    aggregator: &MetaConsoleAggregator,
    accepted: &ConsoleInteractionAccepted,
) -> crate::console_aggregator::ConsoleFrame {
    tokio::time::timeout(WAIT, async {
        loop {
            let frame = frame_now(aggregator, accepted).await;
            if matches!(
                frame.status,
                ConsoleFrameStatus::Delivered | ConsoleFrameStatus::DeliveryFailed
            ) || (frame.payload.get("delivery_unresolved").is_some()
                && !aggregator.send_dispatch_claimed(&frame.id))
            {
                return frame;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the console dispatch answers")
}

/// A governed Console steer whose native admission was real but whose
/// answer was lost (the bridge answers an internal error after the native
/// admission accepted the input as new work, as Meta's governed ingress row
/// for an error after admission does): the steer answers HTTP 504
/// `governed_admission_unresolved`, and its frame is not failed. The input
/// may have run, so the send stays `submitted` and records the unresolved
/// answer (`delivery_unresolved`). A replay of its key reads the runtime's
/// evidence, finds the input admitted and settles the send `delivered`,
/// without a dispatch: admitted once.
///
/// RED before the fix: the frame is `delivery_failed` at once, and the
/// replay replays the failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unresolved_steer_after_a_real_admission_stays_submitted_until_a_replay_settles_it() {
    let fixture = Arc::new(Fixture::new().await);
    let native_accepted = Arc::new(AtomicUsize::new(0));
    let (runtime, _identity) = governed_identity_runtime_losing(
        &fixture,
        losing::LostAnswer::PostAdmissionInternal,
        Arc::clone(&native_accepted),
    )
    .await;
    let aggregator = MetaConsoleAggregator::in_memory();
    aggregator.read_send_admission_from(fixture.machine.clone());
    let host = ReviewingHost::new(&fixture, &reviewed(&fixture, "unresolved-steer"));
    let mut sent = request("unresolved-steer", HUMAN);
    sent.handling_mode = Some("steer".to_string());
    let error = send(&aggregator, &runtime, &host, sent.clone())
        .await
        .expect_err("the steer's admission answer was lost");
    let response = super::console_send_error_response(error);
    assert_eq!(response.status(), axum::http::StatusCode::GATEWAY_TIMEOUT);
    let body = json_body(response).await;
    assert_eq!(body["error"], "governed_admission_unresolved", "{body}");
    assert_eq!(
        native_accepted.load(Ordering::SeqCst),
        1,
        "the native admission accepted the input as new work"
    );
    let input = aggregator
        .query_timeline(ConsoleTimelineQuery::default())
        .await
        .unwrap()
        .frames
        .into_iter()
        .find(|frame| frame.kind == "user_input")
        .expect("the steer's input frame");
    assert_eq!(
        input.status,
        ConsoleFrameStatus::Submitted,
        "unresolved, never failed: {input:?}"
    );
    assert_eq!(
        input.payload["delivery_unresolved"],
        unresolved_record(),
        "{input:?}"
    );
    let binds = bind_count(&host);

    let replayed = send(&aggregator, &runtime, &host, sent)
        .await
        .expect("the replay answers the acceptance");
    assert_eq!(replayed.input_frame_id, input.id);
    assert_eq!(
        settled(&aggregator, &replayed).await,
        ConsoleFrameStatus::Delivered,
        "the replay settles the send from the runtime's evidence"
    );
    assert_eq!(bind_count(&host), binds, "never dispatched again");
    assert_eq!(native_accepted.load(Ordering::SeqCst), 1, "admitted once");
}

/// A queued governed send whose submission was lost before native admission
/// settled it (its answer unresolved; here none of its input reached the
/// runtime) stays `submitted` with its unresolved answer recorded. In the
/// process that saw that answer an absent read is not final (the input may
/// still be admitted there): neither a replay of its key nor its recovery
/// fails it, and nothing dispatches it again. A later process's start
/// settles it from evidence as any send a stopped process left submitted:
/// absent there, it fails, its outcome unknown.
///
/// RED before the fix: the frame is `delivery_failed` at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unresolved_send_is_failed_by_an_absent_read_only_in_a_later_process() {
    let fixture = Arc::new(Fixture::new().await);
    let native_accepted = Arc::new(AtomicUsize::new(0));
    let (runtime, _identity) = governed_identity_runtime_losing(
        &fixture,
        losing::LostAnswer::CustodyLost,
        Arc::clone(&native_accepted),
    )
    .await;
    let store: Arc<dyn crate::console_aggregator::ConsoleLogStore> =
        Arc::new(crate::console_aggregator::InMemoryConsoleLogStore::new());
    let aggregator = MetaConsoleAggregator::new(Arc::clone(&store));
    aggregator.read_send_admission_from(fixture.machine.clone());
    let host = ReviewingHost::new(&fixture, &reviewed(&fixture, "unresolved-queued"));
    let accepted = send(
        &aggregator,
        &runtime,
        &host,
        request("unresolved-queued", HUMAN),
    )
    .await
    .expect("the queued send is accepted");
    let frame = unresolved_frame(&aggregator, &accepted).await;
    assert_eq!(
        native_accepted.load(Ordering::SeqCst),
        0,
        "nothing admitted"
    );
    assert_eq!(
        frame.status,
        ConsoleFrameStatus::Submitted,
        "unresolved, never failed: {frame:?}"
    );
    assert_eq!(
        frame.payload["delivery_unresolved"],
        unresolved_record(),
        "{frame:?}"
    );
    let binds = bind_count(&host);

    let replayed = send(
        &aggregator,
        &runtime,
        &host,
        request("unresolved-queued", HUMAN),
    )
    .await
    .expect("the replay answers the acceptance");
    assert_eq!(replayed.input_frame_id, accepted.input_frame_id);
    assert_eq!(
        frame_now(&aggregator, &accepted).await.status,
        ConsoleFrameStatus::Submitted,
        "an absent read in this process never fails it"
    );
    let recovered = aggregator
        .recover_send(
            std::slice::from_ref(&accepted.identity),
            "console",
            "unresolved-queued",
            &super::ConsoleSendGovernance::default().scope,
        )
        .await
        .expect("the recovery answers")
        .expect("the caller's own send");
    assert_eq!(
        recovered.native,
        crate::console_aggregator::NativeSendObservation::Absent
    );
    assert_eq!(
        frame_now(&aggregator, &accepted).await.status,
        ConsoleFrameStatus::Submitted,
        "nor does its recovery here"
    );
    assert_eq!(bind_count(&host), binds, "never dispatched again");

    let later = MetaConsoleAggregator::new(Arc::clone(&store));
    later.read_send_admission_from(fixture.machine.clone());
    let outcome = later.reconcile_interrupted_sends().await;
    assert_eq!(outcome.lost, 1, "{outcome:?}");
    let failed = frame_now(&later, &accepted).await;
    assert_eq!(failed.status, ConsoleFrameStatus::DeliveryFailed);
    assert_eq!(
        failed.payload["delivery_failure"],
        json!({"reason": "outcome_unknown", "retryable": true}),
        "{failed:?}"
    );
    assert_eq!(bind_count(&host), binds, "never dispatched again");
}

/// A governed steer whose request went away after its submission was
/// recorded, and whose submission then answered unresolved (its custody was
/// lost before native admission settled it; here nothing was admitted), is
/// recorded by its runtime-owned operation itself. In the process that saw
/// the unresolved answer an absent read is not final: a replay of its key
/// and its recovery leave the send `submitted`, and its frame records
/// `delivery_unresolved`. Nothing is dispatched again.
///
/// RED before the fix: nothing records the answer once the request is gone,
/// so the replay reads the input absent and fails the send (`outcome_unknown`,
/// retryable), and a new send under a new key could run the turn twice.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unresolved_steer_whose_request_went_away_is_recorded_and_not_failed_here() {
    let fixture = Arc::new(Fixture::new().await);
    let native_accepted = Arc::new(AtomicUsize::new(0));
    let (runtime, _identity) = governed_identity_runtime_losing(
        &fixture,
        losing::LostAnswer::CustodyLost,
        Arc::clone(&native_accepted),
    )
    .await;
    let aggregator = MetaConsoleAggregator::in_memory();
    aggregator.read_send_admission_from(fixture.machine.clone());
    let host = ReviewingHost::new(&fixture, &reviewed(&fixture, "dropped-unresolved"));
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    runtime.set_governed_submitted_hook({
        let entered = Arc::clone(&entered);
        let release = Arc::clone(&release);
        Box::new(move || {
            Box::pin(async move {
                entered.notify_one();
                release.notified().await;
            })
        })
    });
    let mut steer = request("dropped-unresolved", HUMAN);
    steer.handling_mode = Some("steer".to_string());
    let first = tokio::spawn({
        let aggregator = aggregator.clone();
        let runtime = Arc::clone(&runtime);
        let host = Arc::clone(&host);
        let steer = steer.clone();
        async move { send(&aggregator, &runtime, &host, steer).await }
    });
    tokio::time::timeout(WAIT, entered.notified())
        .await
        .expect("the steer's submission is recorded");
    // The steer's request goes away; its runtime-owned operation goes on and
    // answers unresolved, with no request left to record the answer.
    first.abort();
    assert!(
        first.await.is_err_and(|error| error.is_cancelled()),
        "the steer's request went away"
    );
    release.notify_one();
    let frame = tokio::time::timeout(WAIT, async {
        loop {
            let found = aggregator
                .query_timeline(ConsoleTimelineQuery::default())
                .await
                .unwrap()
                .frames
                .into_iter()
                .find(|frame| frame.kind == "user_input");
            if let Some(frame) = found
                && !aggregator.send_dispatch_claimed(&frame.id)
            {
                return frame;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the operation answered and returned");
    assert_eq!(
        native_accepted.load(Ordering::SeqCst),
        0,
        "nothing admitted"
    );
    let binds = bind_count(&host);

    let replayed = send(&aggregator, &runtime, &host, steer)
        .await
        .expect("the replay answers the acceptance");
    assert_eq!(replayed.input_frame_id, frame.id);
    let after = frame_now(&aggregator, &replayed).await;
    assert_eq!(
        after.status,
        ConsoleFrameStatus::Submitted,
        "an absent read in the process that saw the unresolved answer never fails it: {after:?}"
    );
    let recovered = aggregator
        .recover_send(
            std::slice::from_ref(&replayed.identity),
            "console",
            "dropped-unresolved",
            &super::ConsoleSendGovernance::default().scope,
        )
        .await
        .expect("the recovery answers")
        .expect("the caller's own send");
    assert_eq!(
        recovered.native,
        crate::console_aggregator::NativeSendObservation::Absent
    );
    assert_eq!(
        frame_now(&aggregator, &replayed).await.status,
        ConsoleFrameStatus::Submitted,
        "nor does its recovery here"
    );
    // The operation's own record of the answer.
    let recorded = tokio::time::timeout(WAIT, async {
        loop {
            let frame = frame_now(&aggregator, &replayed).await;
            if frame.payload.get("delivery_unresolved").is_some() {
                return frame;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the operation records the unresolved answer");
    assert_eq!(
        recorded.payload["delivery_unresolved"],
        unresolved_record(),
        "{recorded:?}"
    );
    assert_eq!(recorded.status, ConsoleFrameStatus::Submitted);
    assert_eq!(
        native_accepted.load(Ordering::SeqCst),
        0,
        "nothing admitted"
    );
    assert_eq!(bind_count(&host), binds, "never dispatched again");
}

/// A governed Console steer the host refuses answers its typed
/// refusal: the Console's error keeps the governed kind, HTTP 409
/// `governed_input_refused` with the refusal's kind (`denied`) and nothing
/// submitted; nothing was admitted and the frame is `delivery_failed`.
///
/// RED before the fix: the refusal is an untyped dispatch failure (500
/// `internal_error`, no data).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn console_human_governed_steer_refused_answers_its_typed_refusal() {
    let fixture = Arc::new(Fixture::new().await);
    let (runtime, _identity) = governed_identity_runtime(&fixture).await;
    let aggregator = MetaConsoleAggregator::in_memory();
    let host = ReviewingHost::new(
        &fixture,
        &[(
            "refused-steer",
            Review {
                content: "another reviewed text".to_string(),
                session_id: fixture.session_id.clone(),
            },
        )],
    );
    let mut steer = request("refused-steer", HUMAN);
    steer.handling_mode = Some("steer".to_string());

    let error = send(&aggregator, &runtime, &host, steer)
        .await
        .expect_err("the host refuses the steer at bind");
    assert!(
        matches!(
            &error,
            ConsoleSendError::RuntimeOperation(runtime_error)
                if matches!(
                    runtime_error.as_ref(),
                    crate::identity_first::IdentityRuntimeError::GovernedInput(
                        crate::identity_first::GovernedInputError::Refused(refusal)
                    ) if refusal.kind == crate::identity_first::GovernedBindRefusalKind::Denied
                )
        ),
        "the governed refusal keeps its kind: {error:?}"
    );
    let response = super::console_send_error_response(error);
    assert_eq!(response.status(), axum::http::StatusCode::CONFLICT);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("HTTP body");
    let body: serde_json::Value = serde_json::from_slice(&body).expect("JSON body");
    assert_eq!(body["error"], "governed_input_refused", "{body}");
    assert_eq!(body["data"]["kind"], "governed_input_refused", "{body}");
    assert_eq!(body["data"]["refusal"], "denied", "{body}");
    assert_eq!(body["data"]["submitted"], false, "{body}");
    assert!(host.admitted.lock().unwrap().is_empty(), "nothing admitted");
    let page = aggregator
        .query_timeline(ConsoleTimelineQuery::default())
        .await
        .unwrap();
    let input = page
        .frames
        .iter()
        .find(|frame| frame.kind == "user_input")
        .expect("the steer's input frame");
    assert_eq!(
        input.status,
        ConsoleFrameStatus::DeliveryFailed,
        "{input:?}"
    );
}

/// The Console's answers keep a governed send's kind. Held:
/// HTTP 503 `governed_input_held` and JSON-RPC -32052, retryable, with its
/// condition and nothing submitted. Refused: HTTP 409
/// `governed_input_refused` with the refusal's kind.
///
/// RED before the fix: both are untyped dispatch failures (500, -32000, no data).
#[tokio::test]
async fn console_governed_held_and_refused_answers_keep_their_kind() {
    let held = || {
        crate::identity_first::IdentityRuntimeError::GovernedInput(
            crate::identity_first::GovernedInputError::Held(crate::identity_first::GovernedHold {
                condition: meerkat_core::authorization::ControllerReadinessFailure::SourceHeld,
                waited: Duration::from_millis(300),
                binds: 2,
                input_id: meerkat_core::lifecycle::InputId::new(),
                idempotency_key: None,
            }),
        )
    };
    let rpc = super::console_send_rpc_error(
        json!(7),
        super::identity_runtime_error_to_console_send_error(MEMBER, held()),
    );
    assert_eq!(
        rpc["error"]["code"],
        json!(crate::rpc::IDENTITY_HISTORY_UNAVAILABLE_CODE)
    );
    assert_eq!(rpc["error"]["data"]["kind"], "governed_input_held");
    assert_eq!(rpc["error"]["data"]["condition"], "source_held");
    assert_eq!(rpc["error"]["data"]["retryable"], true);
    assert_eq!(rpc["error"]["data"]["submitted"], false);
    let response = super::console_send_error_response(
        super::identity_runtime_error_to_console_send_error(MEMBER, held()),
    );
    assert_eq!(
        response.status(),
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    );
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("HTTP body");
    let body: serde_json::Value = serde_json::from_slice(&body).expect("JSON body");
    assert_eq!(body["error"], "governed_input_held", "{body}");
    assert_eq!(body["data"]["kind"], "governed_input_held", "{body}");

    let refused = crate::identity_first::IdentityRuntimeError::GovernedInput(
        crate::identity_first::GovernedInputError::Refused(
            crate::identity_first::GovernedBindRefusal::closed("the original was cancelled"),
        ),
    );
    let response = super::console_send_error_response(
        super::identity_runtime_error_to_console_send_error(MEMBER, refused),
    );
    assert_eq!(response.status(), axum::http::StatusCode::CONFLICT);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("HTTP body");
    let body: serde_json::Value = serde_json::from_slice(&body).expect("JSON body");
    assert_eq!(body["error"], "governed_input_refused", "{body}");
    assert_eq!(body["data"]["refusal"], "closed", "{body}");
    assert_eq!(body["data"]["retryable"], false, "{body}");
}

/// Wait until the host's binder was asked once.
async fn first_bind(host: &ReviewingHost) {
    tokio::time::timeout(WAIT, async {
        while host.ids.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the host's binder is asked");
}

/// The human's Console access revoked while a governed send
/// waits out a "not now": the next bind verifies the human again and is
/// refused (`denied`) before the host's binder is asked, so the host never
/// binds the send and nothing is admitted; the frame is `delivery_failed`.
///
/// RED with the human's check at each bind dropped: the host binds the send
/// after the revocation, which only the entry fence refuses.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn console_access_revoked_during_a_wait_refuses_the_next_bind_before_the_host() {
    let fixture = Arc::new(Fixture::new().await);
    let (runtime, _identity) = governed_identity_runtime(&fixture).await;
    let aggregator = MetaConsoleAggregator::in_memory();
    let host = ReviewingHost::new(
        &fixture,
        &[(
            "revoked-original",
            Review {
                content: HUMAN.to_string(),
                session_id: fixture.session_id.clone(),
            },
        )],
    );
    host.hold(WAIT);
    let human = super::console_human_input_tests::current_console_human();
    let subject = human.current().expect("the human is current");
    let access = crate::access::AccessController::new(crate::access::AccessControlConfig {
        enabled: true,
        admins: vec!["root@example.test".to_string()],
        rules: vec![crate::access::AccessRule {
            id: "send".to_string(),
            subjects: vec![subject],
            actions: vec![crate::access::ACTION_AGENT_SEND.to_string()],
            agents: vec![MEMBER.to_string()],
            ..crate::access::AccessRule::default()
        }],
        ..crate::access::AccessControlConfig::default()
    })
    .expect("the access configuration");
    let governance = super::ConsoleSendGovernance {
        ingress: Some(host.clone()),
        human: Some(human),
        access: Some(access.clone()),
        ..Default::default()
    };
    let accepted = super::console_send_governed_with_identity_first_fallback(
        &aggregator,
        Arc::clone(&runtime),
        None,
        request("revoked-original", HUMAN),
        &governance,
    )
    .await
    .expect("accepted for the verified human");
    first_bind(&host).await;

    access
        .delete_rule("send")
        .expect("the human's access is revoked");
    host.release();
    assert_eq!(
        settled(&aggregator, &accepted).await,
        ConsoleFrameStatus::DeliveryFailed
    );
    assert!(
        host.bound.lock().unwrap().is_none(),
        "the host never bound the send after the revocation"
    );
    assert!(host.admitted.lock().unwrap().is_empty(), "nothing admitted");
    assert_eq!(
        host.ids.lock().unwrap().len(),
        1,
        "the host's binder was asked once, before the revocation"
    );
}

/// The identity runtime's shutdown (its foreground close) ends
/// a governed send's "not now" wait at once: the send ends well inside its
/// budget, with nothing bound or admitted, and its frame is
/// `delivery_failed`.
///
/// RED with the wait's shutdown arm dropped: the send waits out its budget.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_held_console_send_ends_at_shutdown_with_nothing_submitted() {
    let fixture = Arc::new(Fixture::new().await);
    let (runtime, _identity) = governed_identity_runtime(&fixture).await;
    let aggregator = MetaConsoleAggregator::in_memory();
    let host = ReviewingHost::new(
        &fixture,
        &[(
            "stopped-original",
            Review {
                content: HUMAN.to_string(),
                session_id: fixture.session_id.clone(),
            },
        )],
    );
    host.hold(WAIT);
    let accepted = send(
        &aggregator,
        &runtime,
        &host,
        request("stopped-original", HUMAN),
    )
    .await
    .expect("accepted for the verified human");
    first_bind(&host).await;

    runtime.close_foreground_operations();
    let ended = tokio::time::timeout(Duration::from_secs(1), settled(&aggregator, &accepted)).await;
    assert!(
        matches!(ended, Ok(ConsoleFrameStatus::DeliveryFailed)),
        "the held send ends at shutdown, inside its budget: {ended:?}"
    );
    assert!(host.bound.lock().unwrap().is_none(), "nothing bound");
    assert!(host.admitted.lock().unwrap().is_empty(), "nothing admitted");
}

/// The send's frame as the Console holds it now.
async fn frame_now(
    aggregator: &MetaConsoleAggregator,
    accepted: &ConsoleInteractionAccepted,
) -> crate::console_aggregator::ConsoleFrame {
    aggregator
        .query_timeline(ConsoleTimelineQuery {
            identity: Some(accepted.identity.clone()),
            ..Default::default()
        })
        .await
        .unwrap()
        .frames
        .into_iter()
        .find(|frame| frame.id == accepted.input_frame_id)
        .expect("the send's frame")
}

/// A governed send whose native admission is unresolved.
fn unresolved_admission(
    receipt: Option<Box<crate::identity_first::GovernedSubmissionReceipt>>,
) -> crate::identity_first::IdentityRuntimeError {
    crate::identity_first::IdentityRuntimeError::GovernedInput(
        crate::identity_first::GovernedInputError::AdmissionUnresolved {
            detail: "native admission settled None".to_string(),
            receipt,
        },
    )
}

/// The JSON body of a Console HTTP answer.
async fn json_body(response: axum::response::Response) -> serde_json::Value {
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("HTTP body");
    serde_json::from_slice(&body).expect("JSON body")
}

/// A governed send whose native admission is unresolved (the input may have
/// run) has its own answer, never a refusal: HTTP 504
/// `governed_admission_unresolved`, as an admission timeout answers
/// (`admission_timeout`), with a message saying the input may have run and may
/// be retried only with the same idempotency key or
/// `meta/console/recover_send`, never as a new send. The Console's JSON-RPC
/// answers -32000 with the same message and data. A refusal keeps HTTP 409
/// `governed_input_refused`.
///
/// RED before the fix: the unresolved answer is 409 `governed_input_refused`,
/// "governed input refused".
#[tokio::test]
async fn console_governed_unresolved_admission_answers_its_own_kind() {
    let response = super::console_send_error_response(
        super::identity_runtime_error_to_console_send_error(MEMBER, unresolved_admission(None)),
    );
    assert_eq!(response.status(), axum::http::StatusCode::GATEWAY_TIMEOUT);
    let body = json_body(response).await;
    assert_eq!(body["error"], "governed_admission_unresolved", "{body}");
    assert_eq!(
        body["data"]["kind"], "governed_admission_unresolved",
        "{body}"
    );
    let message = body["message"].as_str().expect("the answer's message");
    for says in [
        "may have run",
        "same idempotency key",
        "meta/console/recover_send",
        "never as a new send",
    ] {
        assert!(message.contains(says), "{says:?} in {message:?}");
    }

    let rpc = super::console_send_rpc_error(
        json!(7),
        super::identity_runtime_error_to_console_send_error(MEMBER, unresolved_admission(None)),
    );
    assert_eq!(rpc["error"]["code"], json!(-32000), "{rpc}");
    assert_eq!(rpc["error"]["message"], json!(message), "{rpc}");
    assert_eq!(
        rpc["error"]["data"]["kind"], "governed_admission_unresolved",
        "{rpc}"
    );

    let refused = crate::identity_first::IdentityRuntimeError::GovernedInput(
        crate::identity_first::GovernedInputError::Refused(
            crate::identity_first::GovernedBindRefusal::denied("the original was not reviewed"),
        ),
    );
    let response = super::console_send_error_response(
        super::identity_runtime_error_to_console_send_error(MEMBER, refused),
    );
    assert_eq!(response.status(), axum::http::StatusCode::CONFLICT);
    let body = json_body(response).await;
    assert_eq!(body["error"], "governed_input_refused", "{body}");
}

/// A governed submission's receipt (its session, raw key, association digest
/// and input id) stays on Meta's internal seams. The structured data of an
/// unresolved admission carries its `kind` and no `receipt` on every wire: the
/// Console's HTTP body, the Console's JSON-RPC error data and the identity
/// JSON-RPC error data.
///
/// RED before the fix: all three carry the receipt.
#[tokio::test]
async fn an_unresolved_admissions_receipt_stays_off_every_wire() {
    let receipt = || {
        Some(Box::new(crate::identity_first::GovernedSubmissionReceipt {
            v: crate::identity_first::GOVERNED_SUBMISSION_RECEIPT_VERSION,
            session_id: SessionId::new(),
            idempotency_key: meerkat_runtime::IdempotencyKey::new("console:interaction:frame"),
            association_digest: meerkat_runtime::AssociationDigest::parse(&format!(
                "v1:{}",
                "ab".repeat(32)
            ))
            .expect("a digest"),
            input_id: meerkat_core::lifecycle::InputId::new(),
        }))
    };
    let http = json_body(super::console_send_error_response(
        super::identity_runtime_error_to_console_send_error(
            MEMBER,
            unresolved_admission(receipt()),
        ),
    ))
    .await;
    let console_rpc = super::console_send_rpc_error(
        json!(7),
        super::identity_runtime_error_to_console_send_error(
            MEMBER,
            unresolved_admission(receipt()),
        ),
    );
    let identity_rpc = crate::rpc::identity_json_rpc_error(&unresolved_admission(receipt()));
    let identity_rpc = identity_rpc.data.expect("the identity RPC's data");
    for (wire, data) in [
        ("console http", &http["data"]),
        ("console rpc", &console_rpc["error"]["data"]),
        ("identity rpc", &identity_rpc),
    ] {
        assert_eq!(
            data["kind"], "governed_admission_unresolved",
            "{wire}: {data}"
        );
        assert!(data.get("receipt").is_none(), "{wire}: {data}");
    }
}

/// A governed steer whose request went away (its client disconnected) after its
/// binder recorded the submission, while its runtime-owned operation still
/// holds the input between that record and native admission, is not failed by a
/// replay of its key: the operation keeps the frame's dispatch until it
/// returns, so the replay answers the acceptance and the frame stays
/// `submitted`. Once the input is admitted and the operation returned, a replay
/// settles the send `delivered` from the runtime's evidence. One bind, admitted
/// once.
///
/// RED before the fix: the request alone owned the dispatch, so the replay
/// found it free and the input not yet admitted, and failed the send.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_steer_whose_request_went_away_after_its_submission_is_not_failed_by_a_replay() {
    let fixture = Arc::new(Fixture::new().await);
    let (runtime, _identity) = governed_identity_runtime(&fixture).await;
    let aggregator = MetaConsoleAggregator::in_memory();
    aggregator.read_send_admission_from(fixture.machine.clone());
    let host = ReviewingHost::new(&fixture, &reviewed(&fixture, "dropped-steer"));
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    runtime.set_governed_submitted_hook({
        let entered = Arc::clone(&entered);
        let release = Arc::clone(&release);
        Box::new(move || {
            Box::pin(async move {
                entered.notify_one();
                release.notified().await;
            })
        })
    });
    let mut steer = request("dropped-steer", HUMAN);
    steer.handling_mode = Some("steer".to_string());
    let first = tokio::spawn({
        let aggregator = aggregator.clone();
        let runtime = Arc::clone(&runtime);
        let host = Arc::clone(&host);
        let steer = steer.clone();
        async move { send(&aggregator, &runtime, &host, steer).await }
    });
    tokio::time::timeout(WAIT, entered.notified())
        .await
        .expect("the steer's submission is recorded");
    // The steer's request goes away; its runtime-owned operation goes on.
    first.abort();
    assert!(
        first.await.is_err_and(|error| error.is_cancelled()),
        "the steer's request went away"
    );

    let replayed = send(&aggregator, &runtime, &host, steer.clone())
        .await
        .expect("the replay is answered");
    let frame = frame_now(&aggregator, &replayed).await;
    assert_eq!(
        frame.status,
        ConsoleFrameStatus::Submitted,
        "a replay while the operation may still submit does not fail the send: {frame:?}"
    );

    release.notify_one();
    tokio::time::timeout(WAIT, async {
        while host.admitted.lock().unwrap().is_empty()
            || aggregator.send_dispatch_claimed(&replayed.input_frame_id)
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the input is admitted and the operation returned");
    let settling = send(&aggregator, &runtime, &host, steer)
        .await
        .expect("the replay is answered");
    assert_eq!(
        settled(&aggregator, &settling).await,
        ConsoleFrameStatus::Delivered
    );
    assert_eq!(host.admitted.lock().unwrap().len(), 1, "admitted once");
    assert_eq!(bind_count(&host), 1, "one bind");
}

/// A held send whose replay recorded its dispatch (`dispatching`) and took its
/// hold, then stopped before it dispatched (the process went away), is left
/// `dispatching` with no hold and no owner. Nothing was submitted (the
/// submission record precedes every submission) and no operation is left, so a
/// replay of its key treats it as held and dispatches it once: delivered,
/// admitted once.
///
/// RED before the fix: with no hold the replay answers the acceptance and
/// dispatches nothing; the send stays `dispatching` for good.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_held_send_stopped_after_its_hold_was_taken_is_dispatched_once_on_replay() {
    let fixture = Arc::new(Fixture::new().await);
    let (runtime, _identity) = governed_identity_runtime(&fixture).await;
    let aggregator = MetaConsoleAggregator::in_memory();
    aggregator.read_send_admission_from(fixture.machine.clone());
    let host = ReviewingHost::new(&fixture, &reviewed(&fixture, "taken-original"));
    let (accepted, _held) = held_send(&aggregator, &runtime, &host, "taken-original").await;
    // A replay recorded the dispatch and took the hold, then stopped.
    aggregator
        .mark_interaction_dispatching(&accepted.input_frame_id)
        .await
        .expect("the replay's dispatch begins");
    assert!(
        aggregator
            .take_interaction_delivery_hold(&accepted.input_frame_id)
            .await
            .expect("the hold's take"),
        "the replay took the hold"
    );
    let stopped = frame_now(&aggregator, &accepted).await;
    assert_eq!(stopped.status, ConsoleFrameStatus::Dispatching);
    assert!(
        stopped.payload.get("delivery_hold").is_none(),
        "{stopped:?}"
    );
    let binds = bind_count(&host);

    host.release();
    let replayed = send(
        &aggregator,
        &runtime,
        &host,
        request("taken-original", HUMAN),
    )
    .await
    .expect("the replay is accepted");
    assert_eq!(
        settled(&aggregator, &replayed).await,
        ConsoleFrameStatus::Delivered
    );
    assert_eq!(host.admitted.lock().unwrap().len(), 1, "admitted once");
    assert_eq!(bind_count(&host), binds + 1, "dispatched once");
}

/// A governed send an earlier version left `accepted` (no hold, no dispatch
/// owner, and none of this version's governed record), whose input that
/// version's dispatch admitted under a random input id, is never dispatched
/// again by a replay of its key: the runtime's evidence of it (by the
/// frame-derived input id) reads absent, and such a frame is settled from
/// evidence only. One bind, admitted once, and the frame stays `accepted`.
///
/// Passes before the fix; RED against a mutant that treats every unowned
/// `accepted` governed frame as held.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_earlier_versions_unowned_accepted_send_is_never_dispatched_again() {
    let fixture = Arc::new(Fixture::new().await);
    let (runtime, identity) = governed_identity_runtime(&fixture).await;
    let aggregator = MetaConsoleAggregator::in_memory();
    aggregator.read_send_admission_from(fixture.machine.clone());
    let host = ReviewingHost::new(&fixture, &reviewed(&fixture, "earlier-original"));
    let sent = request("earlier-original", HUMAN);
    let session = fixture.session_id.to_string();
    let scope = super::ConsoleSendGovernance::default().scope;
    // The earlier version reserved the send as it reserved every send ...
    let accepted = match aggregator
        .reserve_identity_first_interaction_scoped(
            sent.clone(),
            Some(session.as_str()),
            &scope,
            None,
        )
        .await
        .expect("the Console reserves the send")
    {
        crate::console_aggregator::ScopedReservation::Fresh(accepted, claim) => {
            drop(claim);
            accepted
        }
        other => panic!("a fresh reservation: {other:?}"),
    };
    // ... and its dispatch submitted the send's keyed input under a random
    // id; the input was admitted, and the process stopped before it settled
    // the frame.
    let binder = host
        .binder_for_send(
            &identity,
            &super::console_human_input_tests::current_console_human(),
            &ConsoleSendContext::for_request(&sent),
        )
        .expect("the host's binder for the send");
    let mut prompt = meerkat_runtime::PromptInput::from_content_input(
        ContentInput::Text(HUMAN.to_string()),
        Some(
            meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata {
                handling_mode: Some(meerkat_core::types::HandlingMode::Queue),
                ..Default::default()
            },
        ),
    );
    prompt.header.idempotency_key = Some(meerkat_runtime::IdempotencyKey::new(
        crate::console_aggregator::governed_delivery_key(
            &accepted.interaction_id,
            &accepted.input_frame_id,
        ),
    ));
    runtime
        .send_governed_with_turn_ticket(
            &identity,
            None,
            meerkat_runtime::Input::Prompt(prompt),
            binder,
        )
        .await
        .expect("the earlier dispatch is admitted");
    assert_eq!(host.admitted.lock().unwrap().len(), 1, "admitted once");
    let binds = bind_count(&host);

    let replayed = send(&aggregator, &runtime, &host, sent)
        .await
        .expect("the replay is answered");
    tokio::time::sleep(HOLD_BUDGET).await;
    assert_eq!(bind_count(&host), binds, "never dispatched again");
    assert_eq!(
        host.admitted.lock().unwrap().len(),
        1,
        "still admitted once"
    );
    let frame = frame_now(&aggregator, &replayed).await;
    assert_eq!(frame.status, ConsoleFrameStatus::Accepted, "{frame:?}");
}

/// A governed send this version reserved, whose process stopped before its
/// first dispatch began (`accepted`, no hold, no owner), submitted nothing: its
/// frame's governed record says this version reserved it, so a replay of its
/// key treats it as held and dispatches it once: delivered, admitted once.
///
/// RED before the fix: the replay answers the acceptance and dispatches
/// nothing; the send stays `accepted` for good.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_governed_send_stopped_before_its_first_dispatch_is_dispatched_once_on_replay() {
    let fixture = Arc::new(Fixture::new().await);
    let (runtime, _identity) = governed_identity_runtime(&fixture).await;
    let aggregator = MetaConsoleAggregator::in_memory();
    aggregator.read_send_admission_from(fixture.machine.clone());
    let host = ReviewingHost::new(&fixture, &reviewed(&fixture, "unstarted-original"));
    let session = fixture.session_id.to_string();
    let scope = super::ConsoleSendGovernance::default().scope;
    let accepted = match aggregator
        .reserve_governed_interaction_scoped(
            request("unstarted-original", HUMAN),
            Some(session.as_str()),
            &scope,
            None,
        )
        .await
        .expect("the Console reserves the send")
    {
        crate::console_aggregator::ScopedReservation::Fresh(accepted, claim) => {
            // The process stopped before the dispatch began.
            drop(claim);
            accepted
        }
        other => panic!("a fresh reservation: {other:?}"),
    };

    let replayed = send(
        &aggregator,
        &runtime,
        &host,
        request("unstarted-original", HUMAN),
    )
    .await
    .expect("the replay is accepted");
    assert_eq!(replayed.input_frame_id, accepted.input_frame_id);
    assert_eq!(
        settled(&aggregator, &replayed).await,
        ConsoleFrameStatus::Delivered
    );
    assert_eq!(host.admitted.lock().unwrap().len(), 1, "admitted once");
    assert_eq!(bind_count(&host), 1, "dispatched once");
}

/// A held send's replay takes the hold before it dispatches: while the dispatch
/// is in flight (its native admission held), the frame records no hold.
/// Released, the send is delivered, admitted once.
///
/// Passes before the fix; RED against the mutant that dispatches without
/// taking the hold (the hold stays on the frame).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_replayed_held_send_takes_its_hold_before_it_dispatches() {
    let fixture = Arc::new(Fixture::new().await);
    let (runtime, _identity) = governed_identity_runtime(&fixture).await;
    let aggregator = MetaConsoleAggregator::in_memory();
    aggregator.read_send_admission_from(fixture.machine.clone());
    let host = ReviewingHost::new(&fixture, &reviewed(&fixture, "retaken-original"));
    let (accepted, _held) = held_send(&aggregator, &runtime, &host, "retaken-original").await;

    host.release();
    fixture.adapter.gate.hold();
    send(
        &aggregator,
        &runtime,
        &host,
        request("retaken-original", HUMAN),
    )
    .await
    .expect("the replay is accepted");
    tokio::time::timeout(WAIT, async {
        while fixture.adapter.gate.entered() == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the replay's dispatch is in flight");
    let in_flight = frame_now(&aggregator, &accepted).await;
    assert!(
        in_flight.payload.get("delivery_hold").is_none(),
        "the replay took the hold before it dispatched: {in_flight:?}"
    );
    assert_eq!(in_flight.status, ConsoleFrameStatus::Submitted);

    fixture.adapter.gate.release();
    assert_eq!(
        settled(&aggregator, &accepted).await,
        ConsoleFrameStatus::Delivered
    );
    assert_eq!(host.admitted.lock().unwrap().len(), 1, "admitted once");
}

/// A log store that implements none of the send machine's own transitions
/// (as a store written before them, or a wrapper that forwards only what
/// it knew of): it forwards everything else to the in-memory store.
struct NoSendStates(Arc<crate::console_aggregator::InMemoryConsoleLogStore>);

#[async_trait::async_trait]
impl crate::console_aggregator::ConsoleLogStore for NoSendStates {
    async fn append_if_absent(
        &self,
        frame: crate::console_aggregator::NewConsoleFrame,
    ) -> crate::console_aggregator::ConsoleLogResult<crate::console_aggregator::AppendOutcome> {
        self.0.append_if_absent(frame).await
    }

    async fn update_frame_status(
        &self,
        frame_id: &str,
        status: ConsoleFrameStatus,
    ) -> crate::console_aggregator::ConsoleLogResult<Option<crate::console_aggregator::ConsoleFrame>>
    {
        self.0.update_frame_status(frame_id, status).await
    }

    async fn query_frames(
        &self,
        query: ConsoleTimelineQuery,
    ) -> crate::console_aggregator::ConsoleLogResult<crate::console_aggregator::ConsoleTimelinePage>
    {
        self.0.query_frames(query).await
    }

    async fn query_windowed_frames(
        &self,
        query: crate::console_aggregator::ConsoleTimelineWindowQuery,
    ) -> crate::console_aggregator::ConsoleLogResult<
        crate::console_aggregator::ConsoleTimelineWindowPage,
    > {
        self.0.query_windowed_frames(query).await
    }

    async fn frame_by_dedupe_key(
        &self,
        dedupe_key: &str,
    ) -> crate::console_aggregator::ConsoleLogResult<Option<crate::console_aggregator::ConsoleFrame>>
    {
        self.0.frame_by_dedupe_key(dedupe_key).await
    }

    async fn latest_cursor(
        &self,
    ) -> crate::console_aggregator::ConsoleLogResult<Option<crate::console_aggregator::ConsoleCursor>>
    {
        self.0.latest_cursor().await
    }

    async fn clear_frames(&self) -> crate::console_aggregator::ConsoleLogResult<()> {
        self.0.clear_frames().await
    }

    async fn record_source_watermark(
        &self,
        runtime_key: &str,
        source_kind: crate::console_aggregator::ConsoleFrameSourceKind,
        source_cursor: &str,
    ) -> crate::console_aggregator::ConsoleLogResult<()> {
        self.0
            .record_source_watermark(runtime_key, source_kind, source_cursor)
            .await
    }

    async fn source_watermark(
        &self,
        runtime_key: &str,
        source_kind: crate::console_aggregator::ConsoleFrameSourceKind,
    ) -> crate::console_aggregator::ConsoleLogResult<Option<String>> {
        self.0.source_watermark(runtime_key, source_kind).await
    }
}

/// A Console whose log store cannot move a send through the send machine's
/// states (nor record or take a hold) refuses a governed send before anything
/// is reserved, typed: no frame is stranded and the host's binder is never
/// asked. The store's support is checked first.
///
/// RED before the fix: the send is reserved, its dispatch cannot begin, and
/// its frame stays `accepted` with no owner.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_governed_send_through_a_store_without_send_states_is_refused_before_it_is_reserved() {
    let fixture = Arc::new(Fixture::new().await);
    let (runtime, _identity) = governed_identity_runtime(&fixture).await;
    let store = Arc::new(crate::console_aggregator::InMemoryConsoleLogStore::new());
    let aggregator = MetaConsoleAggregator::new(Arc::new(NoSendStates(Arc::clone(&store))));
    let host = ReviewingHost::new(&fixture, &reviewed(&fixture, "unmoved-original"));
    let error = send(
        &aggregator,
        &runtime,
        &host,
        request("unmoved-original", HUMAN),
    )
    .await
    .expect_err("the governed send is refused");
    assert!(
        matches!(&error, ConsoleSendError::GovernedRefused(_)),
        "a typed refusal before anything is reserved: {error:?}"
    );
    let frames = crate::console_aggregator::ConsoleLogStore::query_frames(
        store.as_ref(),
        ConsoleTimelineQuery {
            identity: None,
            conversation_id: None,
            after: None,
            limit: 500,
        },
    )
    .await
    .expect("the store's frames")
    .frames;
    assert!(
        frames.iter().all(|frame| frame.kind != "user_input"),
        "no frame was reserved: {frames:?}"
    );
    assert_eq!(bind_count(&host), 0, "the host's binder was never asked");
}
