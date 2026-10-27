//! Bind-level tests of the Console's per-send context on a member that
//! admits governed input: the real scoped grant composition of
//! `tests/support/governed_mob.rs` behind the identity runtime (with the
//! session owner's controller pin source), driven through the Console's
//! governed send path with a verified Console human.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../tests/support/governed_mob.rs"]
mod governed_support;

use std::collections::{BTreeMap, HashMap};
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
    let bridge: Arc<dyn SessionBridge> = Arc::new(
        MobSessionBridge::new(fixture.handle.clone())
            .with_controller_pin_source(fixture.service.clone()),
    );
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
        .await;
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
        })
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
}

#[async_trait::async_trait]
impl GovernedInputBinder for ReviewedBinder {
    async fn bind(
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

    fn retain_unresolved(&self, _settlement: Box<dyn GovernedSettlement>, _detail: &str) {}
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
