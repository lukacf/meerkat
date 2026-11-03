//! Keyed inject between gateways, over a real control listener.
//!
//! The serving side here is the same keyed inject dispatch the gateway's control
//! handler runs, in front of a scripted continuation owner; the calling side
//! is the production [`RemoteMemberDelivery`] and [`RemoteMobProxy`]. What
//! these pin: only a signed answer the caller's pinned key verifies is
//! evidence, requests are bound to their audience, a grant must name the
//! verbs, the freshness window holds exactly at its edges, and a replay can
//! never add a second commit.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use base64::Engine as _;
use ed25519_dalek::Signer as _;

use super::availability::{
    BlockCause, DepartureEvidence, EvidenceClass, MemberDeparture, SuspendCause,
};
use super::native::{NativeStatus, NativeStatusError, NativeSubmitError};
use super::remote::{Clock, RemoteMemberDelivery, validate_delivery, within_deadline};
use super::types::*;
use crate::auth::peer_keys::GatewayPeerKeys;
use crate::contact_directory::ContactDirectory;
use crate::runtime::cross_mob_control::AuthorizedCaller;
use crate::runtime::cross_mob_control::{
    ControlAuthorizer, ControlGrant, ControlGrantTable, ControlHandler, ControlMemberScope,
    ControlRequest, ControlResponse, ControlSignerSlot, ControlVerb,
    control_response_signing_payload, serve_tcp_control_with_authorizer,
    sign_control_request_as_caller, verify_control_response,
};
use crate::runtime::keyed_inject::{
    KeyedInject, KeyedInjectCallError, KeyedInjectCallerScope, KeyedInjectFuture,
    KeyedInjectReceiver, KeyedInjectRefusal, KeyedInjectRefusalCode, KeyedInjectServing,
    KeyedInjectSlot, KeyedInjectStatusAnswer, NativeReceipt, NotServedCause, StaleRequestCause,
    answer_keyed_inject_request,
};

const PEER: &str = "mob-b";
const MEMBER: &str = "agent:a";
const WINDOW_MS: u64 = 300_000;
const NOW_MS: u64 = 1_800_000_000_000;

/// A continuation owner in miniature, keyed exactly as the gateway's
/// receiver keys it (the batch id under the caller's scope): one binding per
/// key, identical replays answered with the original receipt, other content
/// refused.
#[derive(Default)]
struct Bindings {
    by_key: BTreeMap<String, (String, NativeReceipt)>,
    submits: usize,
}

struct ScriptedReceiver {
    now: AtomicU64,
    started_at: AtomicU64,
    bindings: Mutex<Bindings>,
    refuse_next: Mutex<Option<KeyedInjectRefusal>>,
}

impl ScriptedReceiver {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            now: AtomicU64::new(NOW_MS),
            started_at: AtomicU64::new(0),
            bindings: Mutex::new(Bindings::default()),
            refuse_next: Mutex::new(None),
        })
    }

    fn commits(&self) -> usize {
        self.bindings.lock().unwrap().by_key.len()
    }

    fn submits(&self) -> usize {
        self.bindings.lock().unwrap().submits
    }
}

impl KeyedInjectReceiver for ScriptedReceiver {
    fn window_ms(&self) -> u64 {
        WINDOW_MS
    }

    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }

    fn started_at_ms(&self) -> u64 {
        self.started_at.load(Ordering::SeqCst)
    }

    fn deliver<'a>(
        &'a self,
        scope: &'a KeyedInjectCallerScope,
        delivery: KeyedInject<'a>,
        _deadline_ms: u64,
    ) -> KeyedInjectFuture<'a, std::result::Result<NativeReceipt, KeyedInjectRefusal>> {
        Box::pin(async move {
            if let Some(refusal) = self.refuse_next.lock().unwrap().take() {
                return Err(refusal);
            }
            validate_delivery(&delivery)?;
            let key = scope.native_key(delivery.delivery_key);
            let exact = format!("{}|{}", delivery.content_digest, delivery.summary);
            let mut bindings = self.bindings.lock().unwrap();
            bindings.submits += 1;
            if let Some((bound, receipt)) = bindings.by_key.get(&key) {
                if *bound == exact {
                    return Ok(receipt.clone());
                }
                return Err(KeyedInjectRefusal::new(
                    KeyedInjectRefusalCode::Conflict,
                    "the key is bound to other content",
                ));
            }
            let receipt = NativeReceipt {
                delivery_id: format!("delivery:{key}"),
                address: format!("rt:{}:1", delivery.member),
                submission_digest: format!("submission:{key}:{exact}"),
                committed_at_ms: self.now_ms(),
            };
            bindings.by_key.insert(key, (exact, receipt.clone()));
            Ok(receipt)
        })
    }

    fn status<'a>(
        &'a self,
        scope: &'a KeyedInjectCallerScope,
        _member: &'a str,
        delivery_key: &'a str,
        with_input: bool,
    ) -> KeyedInjectFuture<'a, std::result::Result<KeyedInjectStatusAnswer, KeyedInjectRefusal>>
    {
        Box::pin(async move {
            let bindings = self.bindings.lock().unwrap();
            let status = match bindings.by_key.get(&scope.native_key(delivery_key)) {
                None => NativeStatus::NotCommitted,
                Some((_, receipt)) => NativeStatus::Applied {
                    receipt: receipt.clone(),
                    session_id: "session-1".into(),
                    input_id: "input-1".into(),
                },
            };
            let input = (with_input && matches!(status, NativeStatus::Applied { .. }))
                .then(|| serde_json::json!({ "phase": "consumed" }));
            Ok(KeyedInjectStatusAnswer { status, input })
        })
    }
}

/// The keyed inject dispatch the gateway's handler runs; nothing else.
struct KeyedInjectHandler {
    slot: KeyedInjectSlot,
}

impl ControlHandler for KeyedInjectHandler {
    fn handle(
        &self,
        request: ControlRequest,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ControlResponse> + Send + '_>> {
        Box::pin(async move { answer_keyed_inject_request(&self.slot, request, None).await })
    }

    fn handle_admitted(
        &self,
        request: ControlRequest,
        caller: Option<AuthorizedCaller>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ControlResponse> + Send + '_>> {
        Box::pin(
            async move { answer_keyed_inject_request(&self.slot, request, caller.as_ref()).await },
        )
    }
}

fn signer(keys: &Arc<GatewayPeerKeys>) -> ControlSignerSlot {
    Arc::new(RwLock::new(Some(Arc::clone(keys))))
}

fn keyed_inject_grant(caller: &GatewayPeerKeys, verbs: &[ControlVerb]) -> ControlGrantTable {
    let mut table = ControlGrantTable::new();
    file_keyed_inject_grant(&mut table, "calling-gateway", caller, verbs);
    table
}

fn file_keyed_inject_grant(
    table: &mut ControlGrantTable,
    label: &str,
    caller: &GatewayPeerKeys,
    verbs: &[ControlVerb],
) {
    table.insert(
        caller.pubkey_bytes(),
        ControlGrant::new(
            label,
            verbs.iter().copied(),
            ControlMemberScope::members([MEMBER]),
        ),
    );
}

struct Peer {
    address: String,
    receiver: Arc<ScriptedReceiver>,
}

/// Serve the keyed inject plane on a loopback port, signing answers with
/// `answer_keys` and admitting callers through `authorizer`.
async fn serve(answer_keys: &Arc<GatewayPeerKeys>, authorizer: ControlAuthorizer) -> Peer {
    let receiver = ScriptedReceiver::new();
    let slot: KeyedInjectSlot = Arc::new(RwLock::new(KeyedInjectServing::Serving(Arc::clone(
        &receiver,
    )
        as Arc<dyn KeyedInjectReceiver>)));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("tcp://{}", listener.local_addr().unwrap());
    tokio::spawn(serve_tcp_control_with_authorizer(
        listener,
        Arc::new(KeyedInjectHandler { slot }),
        signer(answer_keys),
        Arc::new(authorizer),
    ));
    Peer { address, receiver }
}

fn contacts(mob_id: &str, address: &str, pinned: &GatewayPeerKeys) -> ContactDirectory {
    ContactDirectory::from_toml(&format!(
        "[mobs]\n\"{mob_id}\" = {{ transport = \"{address}\", pubkey = \"{}\" }}\n",
        pinned.pubkey_b64()
    ))
    .unwrap()
}

fn caller(
    directory: ContactDirectory,
    keys: &Arc<GatewayPeerKeys>,
    clock_ms: Arc<AtomicU64>,
) -> RemoteMemberDelivery {
    let clock: Clock = Arc::new(move || clock_ms.load(Ordering::SeqCst));
    RemoteMemberDelivery::with_clock(directory, signer(keys), WINDOW_MS, clock)
}

/// Whether a submission's outcome is unknown because the peer answered
/// unsigned, with this code kept as text.
fn unsigned(outcome: &std::result::Result<NativeReceipt, NativeSubmitError>, code: &str) -> bool {
    matches!(
        outcome,
        Err(NativeSubmitError::OutcomeUnknown {
            cause: SuspendCause::PeerUnanswered { code: Some(answered) },
            evidence: EvidenceClass::Unverified,
            ..
        }) if answered == code
    )
}

fn remote_batch(id: &str, body_text: &str) -> Batch {
    let id = BatchId::new(id).unwrap();
    let body = render_batch(
        &id,
        None,
        &[],
        &[format!("- [ios] payments #1 key=k\n  {body_text}\n")],
    );
    Batch {
        member: Member::new(
            MobId::new(PEER).unwrap(),
            MemberIdentity::new(MEMBER).unwrap(),
        ),
        items: vec![BatchItem {
            subscription_id: SubscriptionId::new("s1").unwrap(),
            subscription_revision: 1,
            mob_id: MobId::new("payments").unwrap(),
            sequence: 1,
            source_key: SourceKey::new("k").unwrap(),
        }],
        gaps: Vec::new(),
        route: MemberRoute::Peer,
        content_digest: content_digest(&body),
        body,
        id,
        predecessor: None,
        successor: None,
        state: BatchState::Submitting { attempt: 1 },
        sealed_at_ms: NOW_MS,
        updated_at_ms: NOW_MS,
    }
}

/// A granted, pinned, fresh pair: the baseline every other case perturbs.
async fn granted_pair() -> (Peer, RemoteMemberDelivery, Arc<AtomicU64>) {
    let peer_keys = Arc::new(GatewayPeerKeys::ephemeral());
    let caller_keys = Arc::new(GatewayPeerKeys::ephemeral());
    let peer = serve(
        &peer_keys,
        ControlAuthorizer::with_grants_for_audience(
            keyed_inject_grant(
                &caller_keys,
                &[ControlVerb::InjectKeyed, ControlVerb::InjectStatus],
            ),
            PEER,
        ),
    )
    .await;
    let clock = Arc::new(AtomicU64::new(NOW_MS));
    let delivery = caller(
        contacts(PEER, &peer.address, &peer_keys),
        &caller_keys,
        Arc::clone(&clock),
    );
    (peer, delivery, clock)
}

#[tokio::test]
async fn a_remote_batch_commits_once_and_every_replay_returns_the_original_receipt() {
    let (peer, delivery, clock) = granted_pair().await;
    let batch = remote_batch("batch-1", "card declined");
    let first = delivery.submit(&batch).await.unwrap();
    // A later attempt is a new request (new nonce, new issue time) for the
    // same immutable batch: the same commit answers it.
    clock.fetch_add(1_000, Ordering::SeqCst);
    let second = delivery.submit(&batch).await.unwrap();
    assert_eq!(first, second);
    assert_eq!(peer.receiver.commits(), 1);
    assert_eq!(peer.receiver.submits(), 2);
    match delivery.status(&batch).await.unwrap() {
        NativeStatus::Applied { receipt, .. } => assert_eq!(receipt, first),
        other => panic!("expected applied, got {other:?}"),
    }
    assert_eq!(
        delivery.input(&batch).await.unwrap(),
        Some(serde_json::json!({ "phase": "consumed" }))
    );
    // Other content under the same batch id is a signed conflict, which
    // settles the batch rather than retrying it forever.
    let mut changed = remote_batch("batch-1", "a different announcement");
    changed.state = BatchState::Submitting { attempt: 2 };
    assert!(matches!(
        delivery.submit(&changed).await,
        Err(NativeSubmitError::Blocked {
            cause: BlockCause::Conflict,
            ..
        })
    ));
    assert_eq!(peer.receiver.commits(), 1);
}

#[tokio::test]
async fn an_answer_signed_by_any_key_but_the_pinned_one_is_not_evidence() {
    let real_peer_keys = Arc::new(GatewayPeerKeys::ephemeral());
    let impostor_keys = Arc::new(GatewayPeerKeys::ephemeral());
    let caller_keys = Arc::new(GatewayPeerKeys::ephemeral());
    // The listener answers with a key other than the one the caller pinned.
    let peer = serve(
        &impostor_keys,
        ControlAuthorizer::with_grants_for_audience(
            keyed_inject_grant(
                &caller_keys,
                &[ControlVerb::InjectKeyed, ControlVerb::InjectStatus],
            ),
            PEER,
        ),
    )
    .await;
    let delivery = caller(
        contacts(PEER, &peer.address, &real_peer_keys),
        &caller_keys,
        Arc::new(AtomicU64::new(NOW_MS)),
    );
    let batch = remote_batch("batch-1", "card declined");
    match delivery.submit(&batch).await {
        Err(NativeSubmitError::OutcomeUnknown {
            cause: SuspendCause::AnswerFailedVerification,
            evidence: EvidenceClass::Unverified,
            detail,
        }) => {
            assert!(detail.contains("does not verify"), "{detail}");
        }
        other => panic!("an unverifiable receipt must leave the outcome unknown: {other:?}"),
    }
    assert_eq!(
        peer.receiver.commits(),
        1,
        "the peer did commit; the caller just cannot know it from this answer"
    );
    assert!(
        delivery.status(&batch).await.is_err(),
        "an unverifiable status is no evidence either"
    );
}

#[test]
fn a_signature_over_other_request_bytes_does_not_verify() {
    let keys = GatewayPeerKeys::ephemeral();
    let receipt = NativeReceipt {
        delivery_id: "d".into(),
        address: "a".into(),
        submission_digest: "s".into(),
        committed_at_ms: 1,
    };
    let mut response = ControlResponse::InjectReceipt {
        receipt,
        sig_b64: None,
    };
    let answered = br#"{"op":"inject_keyed","nonce":"first"}"#;
    let replayed = br#"{"op":"inject_keyed","nonce":"second"}"#;
    let payload = control_response_signing_payload(answered, &response).unwrap();
    let signature = keys.signing_key().sign(payload.as_bytes());
    if let ControlResponse::InjectReceipt { sig_b64, .. } = &mut response {
        *sig_b64 = Some(base64::engine::general_purpose::STANDARD.encode(signature.to_bytes()));
    }
    assert_eq!(
        verify_control_response(&keys.pubkey_bytes(), answered, &response),
        Ok(()),
        "the positive control: the answer verifies for the request it answered"
    );
    assert!(
        verify_control_response(&keys.pubkey_bytes(), replayed, &response).is_err(),
        "the same signed answer cannot settle another request"
    );
    let mut unsigned = response;
    if let ControlResponse::InjectReceipt { sig_b64, .. } = &mut unsigned {
        *sig_b64 = None;
    }
    assert!(
        verify_control_response(&keys.pubkey_bytes(), answered, &unsigned).is_err(),
        "an unsigned keyed inject answer never verifies"
    );
}

#[tokio::test]
async fn a_request_minted_for_another_audience_is_refused() {
    let peer_keys = Arc::new(GatewayPeerKeys::ephemeral());
    let caller_keys = Arc::new(GatewayPeerKeys::ephemeral());
    let peer = serve(
        &peer_keys,
        ControlAuthorizer::with_grants_for_audience(
            keyed_inject_grant(
                &caller_keys,
                &[ControlVerb::InjectKeyed, ControlVerb::InjectStatus],
            ),
            PEER,
        ),
    )
    .await;
    // The caller files the same listener under another mob id, so it signs
    // its requests for that audience.
    let delivery = caller(
        contacts("mob-c", &peer.address, &peer_keys),
        &caller_keys,
        Arc::new(AtomicU64::new(NOW_MS)),
    );
    let mut batch = remote_batch("batch-1", "card declined");
    batch.member = Member::new(
        MobId::new("mob-c").unwrap(),
        MemberIdentity::new(MEMBER).unwrap(),
    );
    let outcome = delivery.submit(&batch).await;
    assert!(
        unsigned(&outcome, "audience_mismatch"),
        "expected an audience refusal: {outcome:?}"
    );
    assert_eq!(peer.receiver.submits(), 0, "nothing reached the owner");
}

#[tokio::test]
async fn keyed_inject_verbs_need_a_grant_that_names_them() {
    let peer_keys = Arc::new(GatewayPeerKeys::ephemeral());
    let caller_keys = Arc::new(GatewayPeerKeys::ephemeral());
    let batch = remote_batch("batch-1", "card declined");

    // `*` expands to the member plane only.
    let star = ControlGrantTable::from_toml(&format!(
        "[control_grants.calling-gateway]\npubkey = \"{}\"\nverbs = [\"*\"]\n",
        caller_keys.pubkey_b64()
    ))
    .unwrap()
    .unwrap();
    let peer = serve(
        &peer_keys,
        ControlAuthorizer::with_grants_for_audience(star, PEER),
    )
    .await;
    let delivery = caller(
        contacts(PEER, &peer.address, &peer_keys),
        &caller_keys,
        Arc::new(AtomicU64::new(NOW_MS)),
    );
    let outcome = delivery.submit(&batch).await;
    assert!(
        unsigned(&outcome, "verb_not_granted"),
        "a * grant must not cover keyed inject verbs: {outcome:?}"
    );
    assert_eq!(peer.receiver.submits(), 0);

    // An open listener authorizes nothing, so it refuses them outright.
    let open = serve(&peer_keys, ControlAuthorizer::open()).await;
    let delivery = caller(
        contacts(PEER, &open.address, &peer_keys),
        &caller_keys,
        Arc::new(AtomicU64::new(NOW_MS)),
    );
    let outcome = delivery.submit(&batch).await;
    assert!(
        unsigned(&outcome, "grants_required"),
        "an open listener must refuse keyed inject verbs: {outcome:?}"
    );
    assert_eq!(open.receiver.submits(), 0);

    // Member scope applies: a grant for another member does not reach
    // this one.
    let mut other_member = ControlGrantTable::new();
    other_member.insert(
        caller_keys.pubkey_bytes(),
        ControlGrant::new(
            "calling-gateway",
            [ControlVerb::InjectKeyed],
            ControlMemberScope::members(["agent:other"]),
        ),
    );
    let scoped = serve(
        &peer_keys,
        ControlAuthorizer::with_grants_for_audience(other_member, PEER),
    )
    .await;
    let delivery = caller(
        contacts(PEER, &scoped.address, &peer_keys),
        &caller_keys,
        Arc::new(AtomicU64::new(NOW_MS)),
    );
    let outcome = delivery.submit(&batch).await;
    assert!(
        unsigned(&outcome, "member_not_granted"),
        "expected a member-scope refusal: {outcome:?}"
    );
}

#[tokio::test]
async fn the_freshness_window_admits_its_edges_and_nothing_beyond() {
    let (peer, delivery, clock) = granted_pair().await;
    let cases = [
        ("batch-old-edge", NOW_MS - WINDOW_MS, true),
        ("batch-new-edge", NOW_MS + WINDOW_MS, true),
        ("batch-too-old", NOW_MS - WINDOW_MS - 1, false),
        ("batch-too-new", NOW_MS + WINDOW_MS + 1, false),
    ];
    for (id, issued_at, fresh) in cases {
        clock.store(issued_at, Ordering::SeqCst);
        let batch = remote_batch(id, "card declined");
        let outcome = delivery.submit(&batch).await;
        if fresh {
            assert!(outcome.is_ok(), "{id}: {outcome:?}");
        } else {
            assert_eq!(
                outcome,
                Err(NativeSubmitError::NotCommitted {
                    cause: SuspendCause::ClockSkew {
                        issued_at_ms: issued_at,
                        window_ms: WINDOW_MS,
                    },
                    evidence: EvidenceClass::Verified,
                }),
                "{id}: a stale request is refused, typed with this gateway's own values"
            );
        }
    }
    assert_eq!(
        peer.receiver.commits(),
        2,
        "only the two requests inside the window committed"
    );
}

/// Every signed code a peer answers, through the one reading, to its class:
/// "nothing was committed" waits, an unknown outcome reads the status next,
/// a request the peer rules invalid or a conflict blocks the batch, and only
/// `member_departed` ends anything.
#[tokio::test]
async fn every_signed_refusal_reads_as_its_class_and_only_a_departure_ends() {
    let (peer, delivery, _clock) = granted_pair().await;
    let batch = remote_batch("batch-1", "card declined");
    let verified = |cause| {
        Err(NativeSubmitError::NotCommitted {
            cause,
            evidence: EvidenceClass::Verified,
        })
    };
    for code in KeyedInjectRefusalCode::ALL {
        let expected: std::result::Result<NativeReceipt, NativeSubmitError> = match code {
            KeyedInjectRefusalCode::Unavailable => verified(SuspendCause::PeerComposing),
            // A stale refusal that carries no cause says nothing about the
            // clocks: it is not read as skew.
            KeyedInjectRefusalCode::StaleRequest => verified(SuspendCause::StaleRequest {
                cause: StaleRequestCause::Unknown,
            }),
            KeyedInjectRefusalCode::InvalidRequest => Err(NativeSubmitError::Blocked {
                cause: BlockCause::PeerInvalidRequest,
                detail: "scripted".into(),
            }),
            KeyedInjectRefusalCode::UnknownMember => verified(SuspendCause::MemberUnknown),
            KeyedInjectRefusalCode::UnusableMember => {
                verified(SuspendCause::MemberNotServedHere { cause: None })
            }
            KeyedInjectRefusalCode::Governed => verified(SuspendCause::Governed),
            KeyedInjectRefusalCode::Conflict => Err(NativeSubmitError::Blocked {
                cause: BlockCause::Conflict,
                detail: "scripted".into(),
            }),
            KeyedInjectRefusalCode::OwnerRetired => verified(SuspendCause::MemberNotLive),
            KeyedInjectRefusalCode::NativeUnavailable => Err(NativeSubmitError::OutcomeUnknown {
                cause: SuspendCause::PeerNativeUnavailable,
                evidence: EvidenceClass::Verified,
                detail: "scripted".into(),
            }),
            KeyedInjectRefusalCode::NotServed => verified(SuspendCause::PeerNotServing {
                cause: NotServedCause::Unknown,
            }),
            KeyedInjectRefusalCode::MemberDeparted => Err(NativeSubmitError::MemberGone(
                MemberDeparture::from_signed_departure(MobId::new(PEER).unwrap()),
            )),
        };
        *peer.receiver.refuse_next.lock().unwrap() =
            Some(KeyedInjectRefusal::new(code, "scripted"));
        let outcome = delivery.submit(&batch).await;
        assert_eq!(outcome, expected, "{code}");
        let ends = matches!(outcome, Err(NativeSubmitError::MemberGone(_)));
        assert_eq!(
            ends,
            code == KeyedInjectRefusalCode::MemberDeparted,
            "{code}: only member_departed ends anything"
        );
    }
    assert_eq!(peer.receiver.commits(), 0);
}

/// A signed code this build cannot read leaves the outcome unknown and ends
/// nothing; `not_served`'s cause is read from its signed field even when its
/// message names another, and an absent cause reads as `unknown`.
#[test]
fn a_refusal_is_read_from_its_typed_fields_never_its_message() {
    let peer = MobId::new(PEER).unwrap();
    assert_eq!(
        RemoteMemberDelivery::submit_refusal(&peer, 7, 9, "brand_new_code", "m".into(), None),
        NativeSubmitError::OutcomeUnknown {
            cause: SuspendCause::PeerRefusedUnknown {
                code: "brand_new_code".into()
            },
            evidence: EvidenceClass::Verified,
            detail: "m".into(),
        }
    );
    assert_eq!(
        RemoteMemberDelivery::submit_refusal(
            &peer,
            7,
            9,
            "not_served",
            "this gateway does not serve keyed injects (runtime_disabled)".into(),
            Some("not_identity_first"),
        ),
        NativeSubmitError::NotCommitted {
            cause: SuspendCause::PeerNotServing {
                cause: NotServedCause::NotIdentityFirst
            },
            evidence: EvidenceClass::Verified,
        }
    );
    assert_eq!(
        RemoteMemberDelivery::submit_refusal(
            &peer,
            7,
            9,
            "not_served",
            "runtime_disabled".into(),
            None
        ),
        NativeSubmitError::NotCommitted {
            cause: SuspendCause::PeerNotServing {
                cause: NotServedCause::Unknown
            },
            evidence: EvidenceClass::Verified,
        }
    );
    // A stale request is read from its signed cause too: only one outside
    // the peer's window is clock skew, with this gateway's own issue time
    // and window, and a deadline the peer could not meet is not, whatever
    // the message says.
    let stale = |cause: Option<&str>| {
        RemoteMemberDelivery::submit_refusal(
            &peer,
            7,
            9,
            "stale_request",
            "check both gateways' clocks".into(),
            cause,
        )
    };
    assert_eq!(
        stale(Some("outside_window")),
        NativeSubmitError::NotCommitted {
            cause: SuspendCause::ClockSkew {
                issued_at_ms: 7,
                window_ms: 9
            },
            evidence: EvidenceClass::Verified,
        }
    );
    for (field, cause) in [
        (Some("deadline_passed"), StaleRequestCause::DeadlinePassed),
        (
            Some("issued_before_start"),
            StaleRequestCause::IssuedBeforeStart,
        ),
        (Some("clock_drift"), StaleRequestCause::Unknown),
        (None, StaleRequestCause::Unknown),
    ] {
        assert_eq!(
            stale(field),
            NativeSubmitError::NotCommitted {
                cause: SuspendCause::StaleRequest { cause },
                evidence: EvidenceClass::Verified,
            },
            "{field:?}"
        );
    }
    match RemoteMemberDelivery::submit_refusal(&peer, 7, 9, "member_departed", "gone".into(), None)
    {
        NativeSubmitError::MemberGone(departure) => assert_eq!(
            departure.evidence(),
            &DepartureEvidence::SignedPeerDeparture { peer: peer.clone() }
        ),
        other => panic!("expected the departure, got {other:?}"),
    }
}

/// Every call that brought no signed answer, through the one reading: only
/// the local failures say the peer was never asked; everything else leaves
/// the outcome unknown, with an unsigned code kept as text.
#[test]
fn every_call_error_reads_as_its_class() {
    let cases = [
        (
            KeyedInjectCallError::Oversize { bytes: 9 },
            "blocked:request_oversize",
        ),
        (
            KeyedInjectCallError::NotAuthenticatable("no key".into()),
            "not_committed:local_not_authenticatable:local",
        ),
        (
            KeyedInjectCallError::Unauthenticated("bad signature".into()),
            "unknown:answer_failed_verification:unverified",
        ),
        (
            KeyedInjectCallError::Rejected {
                code: "member_departed".into(),
                message: "spoofed".into(),
            },
            "unknown:peer_unanswered:unverified",
        ),
        (
            KeyedInjectCallError::Unexpected("garbled".into()),
            "unknown:peer_unanswered:unverified",
        ),
        (
            KeyedInjectCallError::Transport("refused".into()),
            "unknown:peer_unanswered:unverified",
        ),
    ];
    for (error, expected) in cases {
        let read = match RemoteMemberDelivery::call_error(error.clone()) {
            NativeSubmitError::Blocked { cause, .. } => format!("blocked:{}", cause.as_str()),
            NativeSubmitError::NotCommitted { cause, evidence } => {
                format!("not_committed:{}:{}", cause.kind(), evidence.as_str())
            }
            NativeSubmitError::OutcomeUnknown {
                cause, evidence, ..
            } => format!("unknown:{}:{}", cause.kind(), evidence.as_str()),
            NativeSubmitError::MemberGone(_) => "ended".to_string(),
        };
        assert_eq!(read, expected, "{error}");
    }
}

/// A status read that brings no status keeps the batch waiting, except a
/// signed `invalid_request`, which blocks it, and a signed `member_departed`.
#[test]
fn a_status_read_without_a_status_waits_unless_ruled_out() {
    let peer = MobId::new(PEER).unwrap();
    let read = |code: &str| {
        RemoteMemberDelivery::status_error(
            RemoteMemberDelivery::submit_refusal(&peer, 1, 2, code, "m".into(), None),
            Some(code),
        )
    };
    assert!(matches!(
        read("invalid_request"),
        NativeStatusError::Blocked {
            cause: BlockCause::PeerInvalidRequest,
            ..
        }
    ));
    assert!(matches!(
        read("member_departed"),
        NativeStatusError::MemberGone(_)
    ));
    for code in [
        "conflict",
        "unknown_member",
        "not_served",
        "governed",
        "native_unavailable",
        "brand_new_code",
    ] {
        assert!(
            matches!(read(code), NativeStatusError::Unavailable { .. }),
            "{code}"
        );
    }
}

#[tokio::test]
async fn an_unusable_contact_is_refused_before_anything_is_sent() {
    let peer_keys = Arc::new(GatewayPeerKeys::ephemeral());
    let caller_keys = Arc::new(GatewayPeerKeys::ephemeral());
    let member = Member::new(
        MobId::new(PEER).unwrap(),
        MemberIdentity::new(MEMBER).unwrap(),
    );
    let unpinned = ContactDirectory::from_toml(&format!(
        "[mobs]\n\"{PEER}\" = {{ transport = \"tcp://127.0.0.1:9\" }}\n"
    ))
    .unwrap();
    let opted_out = ContactDirectory::from_toml(&format!(
        "[mobs]\n\"{PEER}\" = {{ transport = \"tcp://127.0.0.1:9\", pubkey = \"{}\", \
         require_signed_control = false }}\n",
        peer_keys.pubkey_b64()
    ))
    .unwrap();
    let absent = ContactDirectory::from_toml("[mobs]\n").unwrap();
    for directory in [unpinned, opted_out, absent] {
        let delivery = caller(directory, &caller_keys, Arc::new(AtomicU64::new(NOW_MS)));
        assert!(
            matches!(
                delivery.check_member(&member),
                Err(super::native::MemberProblem::UnusablePeer(_))
            ),
            "a contact that cannot authenticate the peer is refused"
        );
    }
    // No signing key on this gateway.
    let keyless = RemoteMemberDelivery::with_clock(
        contacts(PEER, "tcp://127.0.0.1:9", &peer_keys),
        Arc::new(RwLock::new(None)),
        WINDOW_MS,
        Arc::new(|| NOW_MS),
    );
    assert!(matches!(
        keyless.check_member(&member),
        Err(super::native::MemberProblem::UnusablePeer(_))
    ));
    // A member must be a durable identity in its public form.
    let delivery = caller(
        contacts(PEER, "tcp://127.0.0.1:9", &peer_keys),
        &caller_keys,
        Arc::new(AtomicU64::new(NOW_MS)),
    );
    let encoded = Member::new(
        MobId::new(PEER).unwrap(),
        MemberIdentity::new("mk--agent_ca").unwrap(),
    );
    assert!(matches!(
        delivery.check_member(&encoded),
        Err(super::native::MemberProblem::Invalid(_))
    ));
    assert_eq!(delivery.check_member(&member), Ok(()));
}

#[test]
fn a_tampered_request_fails_its_caller_signature() {
    let caller_keys = GatewayPeerKeys::ephemeral();
    let authorizer = ControlAuthorizer::with_grants_for_audience(
        keyed_inject_grant(&caller_keys, &[ControlVerb::InjectKeyed]),
        PEER,
    );
    let body = "Mob announcements batch-1: 1 new announcement for your subscriptions.\n";
    let mut request = ControlRequest::InjectKeyed {
        remote_member: MEMBER.into(),
        delivery_key: "batch-1".into(),
        content_digest: content_digest(body),
        summary: "1 announcement".into(),
        body: body.into(),
        issued_at_ms: NOW_MS,
        nonce: "n1".into(),
        caller: None,
    };
    sign_control_request_as_caller(&caller_keys, Some(PEER), &mut request);
    assert!(
        authorizer.authorize(&request).is_ok(),
        "the positive control"
    );
    for tamper in 0..4 {
        let mut tampered = request.clone();
        if let ControlRequest::InjectKeyed {
            body,
            issued_at_ms,
            delivery_key,
            remote_member,
            ..
        } = &mut tampered
        {
            match tamper {
                0 => body.push_str("an injected line\n"),
                1 => *issued_at_ms += 1,
                2 => delivery_key.push('x'),
                _ => *remote_member = "agent:other".into(),
            }
        }
        assert!(
            authorizer.authorize(&tampered).is_err(),
            "tamper case {tamper} must not authorize"
        );
    }
}

#[test]
fn a_delivery_whose_body_does_not_match_its_digest_is_invalid() {
    let body = "Mob announcements batch-1: 1 new announcement for your subscriptions.\n";
    let digest = content_digest(body);
    let valid = KeyedInject {
        member: MEMBER,
        delivery_key: "batch-1",
        content_digest: &digest,
        summary: "1 announcement",
        body,
    };
    assert!(validate_delivery(&valid).is_ok());
    let mismatched = KeyedInject {
        body: "something else",
        ..valid
    };
    assert_eq!(
        validate_delivery(&mismatched).unwrap_err().code,
        KeyedInjectRefusalCode::InvalidRequest
    );
    let long_summary = "s".repeat(MAX_ID_BYTES + 1);
    let oversized = KeyedInject {
        summary: &long_summary,
        ..valid
    };
    assert_eq!(
        validate_delivery(&oversized).unwrap_err().code,
        KeyedInjectRefusalCode::InvalidRequest
    );
}

#[tokio::test]
async fn an_unreachable_peer_fails_fast_for_a_moment_and_is_tried_again() {
    let peer_keys = Arc::new(GatewayPeerKeys::ephemeral());
    let caller_keys = Arc::new(GatewayPeerKeys::ephemeral());
    // A port nothing listens on: the connection is refused at once.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("tcp://{}", closed.local_addr().unwrap());
    drop(closed);
    let clock = Arc::new(AtomicU64::new(NOW_MS));
    let delivery = caller(
        contacts(PEER, &address, &peer_keys),
        &caller_keys,
        Arc::clone(&clock),
    );
    let batch = remote_batch("batch-1", "card declined");
    let unanswered = |outcome: &std::result::Result<NativeReceipt, NativeSubmitError>,
                      fast: bool| {
        matches!(
            outcome,
            Err(NativeSubmitError::OutcomeUnknown {
                cause: SuspendCause::PeerUnanswered { code: None },
                evidence: EvidenceClass::Unverified,
                detail,
            }) if detail.contains("moments ago") == fast
        )
    };
    let first = delivery.submit(&batch).await;
    assert!(unanswered(&first, false), "{first:?}");
    let second = delivery.submit(&batch).await;
    assert!(
        unanswered(&second, true),
        "a call inside the window fails without dialing: {second:?}"
    );
    assert!(delivery.status(&batch).await.is_err());
    clock.fetch_add(super::remote::PEER_BACKOFF_MS, Ordering::SeqCst);
    let third = delivery.submit(&batch).await;
    assert!(
        unanswered(&third, false),
        "after the window the peer is dialed again: {third:?}"
    );
}

#[tokio::test]
async fn a_gateway_that_serves_no_keyed_inject_answers_a_signed_unavailable() {
    // An empty slot: a build without the feature, or a gateway that did
    // not enable it. The refusal is still signed, and it settles nothing.
    let peer_keys = Arc::new(GatewayPeerKeys::ephemeral());
    let caller_keys = Arc::new(GatewayPeerKeys::ephemeral());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("tcp://{}", listener.local_addr().unwrap());
    tokio::spawn(serve_tcp_control_with_authorizer(
        listener,
        Arc::new(KeyedInjectHandler {
            slot: crate::runtime::keyed_inject::pending_keyed_inject_slot(),
        }),
        signer(&peer_keys),
        Arc::new(ControlAuthorizer::with_grants_for_audience(
            keyed_inject_grant(
                &caller_keys,
                &[ControlVerb::InjectKeyed, ControlVerb::InjectStatus],
            ),
            PEER,
        )),
    ));
    let delivery = caller(
        contacts(PEER, &address, &peer_keys),
        &caller_keys,
        Arc::new(AtomicU64::new(NOW_MS)),
    );
    let batch = remote_batch("batch-1", "card declined");
    assert_eq!(
        delivery.submit(&batch).await,
        Err(NativeSubmitError::NotCommitted {
            cause: SuspendCause::PeerComposing,
            evidence: EvidenceClass::Verified,
        }),
        "a starting peer's signed refusal: nothing committed, and nothing ends"
    );
}

/// Two gateways granted on one member: neither can commit under the other's
/// batch id, nor be answered about the other's batch.
#[tokio::test]
async fn a_second_granted_caller_neither_takes_over_nor_answers_for_anothers_batch() {
    let peer_keys = Arc::new(GatewayPeerKeys::ephemeral());
    let first_keys = Arc::new(GatewayPeerKeys::ephemeral());
    let second_keys = Arc::new(GatewayPeerKeys::ephemeral());
    let verbs = [ControlVerb::InjectKeyed, ControlVerb::InjectStatus];
    let mut table = ControlGrantTable::new();
    file_keyed_inject_grant(&mut table, "first-gateway", &first_keys, &verbs);
    file_keyed_inject_grant(&mut table, "second-gateway", &second_keys, &verbs);
    let peer = serve(
        &peer_keys,
        ControlAuthorizer::with_grants_for_audience(table, PEER),
    )
    .await;
    let clock = Arc::new(AtomicU64::new(NOW_MS));
    let first = caller(
        contacts(PEER, &peer.address, &peer_keys),
        &first_keys,
        Arc::clone(&clock),
    );
    let second = caller(
        contacts(PEER, &peer.address, &peer_keys),
        &second_keys,
        Arc::clone(&clock),
    );
    let mine = remote_batch("batch-shared", "the first caller's announcement");
    let borrowed = remote_batch("batch-shared", "the second caller's content");

    // The second caller commits its own body under the first caller's id.
    let theirs = second.submit(&borrowed).await.unwrap();
    // The first caller is told nothing about it.
    assert_eq!(
        first.status(&mine).await.unwrap(),
        NativeStatus::NotCommitted
    );
    let own = first.submit(&mine).await.unwrap();
    assert_ne!(own.delivery_id, theirs.delivery_id);
    assert_ne!(own.submission_digest, theirs.submission_digest);
    assert_eq!(peer.receiver.commits(), 2);
    match first.status(&mine).await.unwrap() {
        NativeStatus::Applied { receipt, .. } => assert_eq!(receipt, own),
        other => panic!("expected the first caller's own delivery, got {other:?}"),
    }
    match second.status(&borrowed).await.unwrap() {
        NativeStatus::Applied { receipt, .. } => assert_eq!(receipt, theirs),
        other => panic!("expected the second caller's own delivery, got {other:?}"),
    }
    // A caller's replay of its own key is still its original receipt.
    clock.fetch_add(1_000, Ordering::SeqCst);
    assert_eq!(first.submit(&mine).await.unwrap(), own);
    assert_eq!(peer.receiver.commits(), 2);
}

/// One grant label is one caller: a key rotation under the label keeps its
/// deliveries.
#[tokio::test]
async fn a_key_rotation_under_one_grant_label_keeps_the_callers_deliveries() {
    let peer_keys = Arc::new(GatewayPeerKeys::ephemeral());
    let old_keys = Arc::new(GatewayPeerKeys::ephemeral());
    let new_keys = Arc::new(GatewayPeerKeys::ephemeral());
    let verbs = [ControlVerb::InjectKeyed, ControlVerb::InjectStatus];
    let mut table = ControlGrantTable::new();
    file_keyed_inject_grant(&mut table, "rotating-gateway", &old_keys, &verbs);
    file_keyed_inject_grant(&mut table, "rotating-gateway", &new_keys, &verbs);
    let peer = serve(
        &peer_keys,
        ControlAuthorizer::with_grants_for_audience(table, PEER),
    )
    .await;
    let clock = Arc::new(AtomicU64::new(NOW_MS));
    let before = caller(
        contacts(PEER, &peer.address, &peer_keys),
        &old_keys,
        Arc::clone(&clock),
    );
    let after = caller(
        contacts(PEER, &peer.address, &peer_keys),
        &new_keys,
        Arc::clone(&clock),
    );
    let batch = remote_batch("batch-rotated", "an announcement");
    let receipt = before.submit(&batch).await.unwrap();
    match after.status(&batch).await.unwrap() {
        NativeStatus::Applied { receipt: seen, .. } => assert_eq!(seen, receipt),
        other => panic!("expected the delivery made before the rotation, got {other:?}"),
    }
    assert_eq!(after.submit(&batch).await.unwrap(), receipt);
    assert_eq!(peer.receiver.commits(), 1);
}

/// Nothing issued before the serving process started is served after it,
/// even inside the freshness window.
#[tokio::test]
async fn a_request_issued_before_the_serving_process_started_is_stale() {
    let (peer, delivery, clock) = granted_pair().await;
    peer.receiver.started_at.store(NOW_MS, Ordering::SeqCst);
    clock.store(NOW_MS - 1, Ordering::SeqCst);
    let batch = remote_batch("batch-before-start", "an announcement");
    assert_eq!(
        delivery.submit(&batch).await,
        Err(NativeSubmitError::NotCommitted {
            cause: SuspendCause::StaleRequest {
                cause: StaleRequestCause::IssuedBeforeStart,
            },
            evidence: EvidenceClass::Verified,
        }),
        "a request that predates the peer's start is not clock skew"
    );
    assert_eq!(peer.receiver.submits(), 0);
    clock.store(NOW_MS, Ordering::SeqCst);
    delivery.submit(&batch).await.unwrap();
    assert_eq!(peer.receiver.commits(), 1);
}

/// What must happen before a delivery's commit is bounded by the request's
/// deadline: a member preparation that outlasts it is cut off, refused as
/// stale (`deadline_passed`), and the commit never runs.
#[tokio::test]
async fn a_preparation_that_outlasts_the_deadline_commits_nothing() {
    let clock: Clock = Arc::new(super::remote::system_clock);
    let deadline = clock().saturating_add(50);
    let committed = std::sync::atomic::AtomicBool::new(false);
    let outcome = within_deadline(deadline, &clock, async {
        std::future::pending::<()>().await;
        Ok(())
    })
    .await
    .map(|()| committed.store(true, Ordering::SeqCst));
    let refusal = outcome.unwrap_err();
    assert_eq!(refusal.code, KeyedInjectRefusalCode::StaleRequest);
    assert_eq!(
        refusal.cause,
        Some(
            crate::runtime::keyed_inject::KeyedInjectRefusalCause::Stale(
                StaleRequestCause::DeadlinePassed
            )
        )
    );
    assert!(!committed.load(Ordering::SeqCst), "nothing was committed");
}

/// The deadline is checked once more right before the commit: a preparation
/// that returns after it passed commits nothing either, and one that returns
/// in time goes on to the commit.
#[tokio::test]
async fn a_deadline_that_passes_during_preparation_is_checked_before_the_commit() {
    let now = Arc::new(AtomicU64::new(NOW_MS));
    let reader = Arc::clone(&now);
    let clock: Clock = Arc::new(move || reader.load(Ordering::SeqCst));
    let late = within_deadline(NOW_MS + WINDOW_MS, &clock, async {
        now.store(NOW_MS + WINDOW_MS + 1, Ordering::SeqCst);
        Ok(())
    })
    .await
    .unwrap_err();
    assert_eq!(
        late.cause,
        Some(
            crate::runtime::keyed_inject::KeyedInjectRefusalCause::Stale(
                StaleRequestCause::DeadlinePassed
            )
        )
    );
    now.store(NOW_MS, Ordering::SeqCst);
    let on_time = within_deadline(NOW_MS + WINDOW_MS, &clock, async {
        now.store(NOW_MS + WINDOW_MS, Ordering::SeqCst);
        Ok("prepared")
    })
    .await;
    assert_eq!(
        on_time,
        Ok("prepared"),
        "a preparation done exactly at the deadline is in time"
    );
}

/// A keyed inject verb reaching the dispatch without an admitting caller is never
/// served.
#[tokio::test]
async fn a_keyed_inject_verb_without_an_admitting_caller_is_refused() {
    let receiver = ScriptedReceiver::new();
    let slot: KeyedInjectSlot = Arc::new(RwLock::new(KeyedInjectServing::Serving(Arc::clone(
        &receiver,
    )
        as Arc<dyn KeyedInjectReceiver>)));
    let body = "a body".to_string();
    let request = ControlRequest::InjectKeyed {
        remote_member: MEMBER.to_string(),
        delivery_key: "batch-unadmitted".to_string(),
        content_digest: content_digest(&body),
        summary: "1 announcement".to_string(),
        body,
        issued_at_ms: NOW_MS,
        nonce: "n".to_string(),
        caller: None,
    };
    match answer_keyed_inject_request(&slot, request, None).await {
        ControlResponse::InjectRefused { code, .. } => {
            assert_eq!(code, KeyedInjectRefusalCode::Unavailable.as_str());
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert_eq!(receiver.submits(), 0);
}

/// A gateway that predates keyed inject cannot decode the request, and its
/// listener says so unsigned. An unsigned answer is never evidence, and an
/// on-path party could fabricate it: the outcome stays unknown and nothing ends.
#[tokio::test]
async fn an_unsigned_decode_from_an_old_gateway_ends_nothing() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let peer_keys = Arc::new(GatewayPeerKeys::ephemeral());
    let caller_keys = Arc::new(GatewayPeerKeys::ephemeral());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("tcp://{}", listener.local_addr().unwrap());
    let old_peer = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut header = [0u8; 4];
        stream.read_exact(&mut header).await.unwrap();
        let mut request = vec![0u8; u32::from_be_bytes(header) as usize];
        stream.read_exact(&mut request).await.unwrap();
        let answer = serde_json::to_vec(&ControlResponse::Err {
            code: "decode".to_string(),
            message: "unknown variant `inject_keyed`".to_string(),
        })
        .unwrap();
        stream
            .write_all(&u32::try_from(answer.len()).unwrap().to_be_bytes())
            .await
            .unwrap();
        stream.write_all(&answer).await.unwrap();
        stream.flush().await.unwrap();
    });
    let delivery = caller(
        contacts(PEER, &address, &peer_keys),
        &caller_keys,
        Arc::new(AtomicU64::new(NOW_MS)),
    );
    let outcome = delivery
        .submit(&remote_batch("batch-1", "card declined"))
        .await;
    assert!(unsigned(&outcome, "decode"), "{outcome:?}");
    old_peer.await.unwrap();
}
