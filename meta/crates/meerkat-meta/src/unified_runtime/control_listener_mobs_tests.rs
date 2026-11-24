//! Rows of the cross-mob control listener on a host of more than one mob.
//!
//! A peer gateway names the mob a request is for with the audience it signs.
//! The listener serves the request on that mob's own entry: its handle, its
//! identity runtime and its session service. A mob the caller's grant does
//! not reach is answered exactly as a mob the host does not have, and
//! authorization loads no mob.
//!
//! Two fixtures. The first is a launch that declares its mob to its mob set
//! and has the set build a second mob of the host, as the entries' own rows
//! do: its requests go through the runtime's own listener and its own
//! resolver. The second gives the listener a resolver that serves a second
//! composition and records each question, for the rows that count what a
//! request asked and what it loaded. A launch lets a request resolve the
//! mobs its mob set built for the host; no row opens anything for it.

#![allow(clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use meerkat_mob::MobId;

use crate::UnifiedRuntime;
use crate::auth::peer_keys::GatewayPeerKeys;
use crate::runtime::cross_mob_control::{
    ControlAuthorizer, ControlAuthzDenial, ControlGrant, ControlGrantTable, ControlHandler,
    ControlListenAddr, ControlMemberScope, ControlMobScope, ControlRequest, ControlResponse,
    ControlVerb, HOST_OWN_KEY_CONTROL_CODE, MobHandleControlHandler, RemoteControlClient,
    sign_control_request_as_caller,
};
use crate::runtime::cross_mob_remote::RemoteEndpoint;
use crate::runtime::keyed_inject::{
    KeyedInject, KeyedInjectCallerScope, KeyedInjectFuture, KeyedInjectReceiver,
    KeyedInjectRefusal, KeyedInjectServing, KeyedInjectStatusAnswer, NativeReceipt, NativeStatus,
};
use crate::unified_runtime::MobResolver;
use crate::unified_runtime::identity_install_phase_tests::runtime as plain_runtime;
use crate::unified_runtime::mob_entries_tests::{
    Sessions, configured_agent, definition, identity_context, launch, second_mob, set_of,
    shut_down, transcript, transcript_showing,
};

/// The label of the one grant a row files for its peer gateway.
const PEER: &str = "peer-gateway";

/// How long a row waits for one answer of the listener. A request that
/// loads a mob answers after the load.
const ANSWER: Duration = Duration::from_secs(30);

/// A gateway on the far side: its key, and where it dials the host.
struct Peer {
    keys: GatewayPeerKeys,
    endpoint: RemoteEndpoint,
}

impl Peer {
    /// One control exchange: `request` signed with this gateway's key for
    /// `audience`, and the listener's answer.
    async fn ask(&self, audience: Option<&str>, request: ControlRequest) -> ControlResponse {
        self.ask_as(&self.keys, audience, request).await
    }

    /// [`Self::ask`], signed with `keys`.
    async fn ask_as(
        &self,
        keys: &GatewayPeerKeys,
        audience: Option<&str>,
        mut request: ControlRequest,
    ) -> ControlResponse {
        sign_control_request_as_caller(keys, audience, &mut request);
        RemoteControlClient::send(&self.endpoint, &request, ANSWER)
            .await
            .expect("the control exchange")
    }

    /// One control exchange that carries no caller envelope.
    async fn ask_unsigned(&self, request: ControlRequest) -> ControlResponse {
        RemoteControlClient::send(&self.endpoint, &request, ANSWER)
            .await
            .expect("the control exchange")
    }
}

fn listen_address() -> ControlListenAddr {
    ControlListenAddr::parse("tcp://127.0.0.1:0").expect("a control address")
}

fn endpoint_of(advertised: &str) -> RemoteEndpoint {
    RemoteEndpoint::Tcp(
        advertised
            .strip_prefix("tcp://")
            .expect("a tcp listener")
            .to_string(),
    )
}

/// A grant for every verb and every member, over `mobs`.
fn grant_over(mobs: Option<ControlMobScope>) -> ControlGrant {
    let grant = ControlGrant::new(
        PEER,
        ControlVerb::all().iter().copied(),
        ControlMemberScope::All,
    );
    match mobs {
        Some(mobs) => grant.with_mobs(mobs),
        None => grant,
    }
}

/// An authorizer bound to `default_mob` that files `grant` under `keys`.
fn authorizer_of(
    keys: &GatewayPeerKeys,
    default_mob: &str,
    grant: ControlGrant,
) -> ControlAuthorizer {
    let mut table = ControlGrantTable::new();
    table.insert(keys.pubkey_bytes(), grant);
    ControlAuthorizer::with_grants_for_audience(table, default_mob)
}

fn lookup(member: &str) -> ControlRequest {
    ControlRequest::LookupMember {
        remote_member: member.to_string(),
        nonce: Some("n".to_string()),
        caller: None,
    }
}

fn inject(member: &str, text: &str) -> ControlRequest {
    ControlRequest::Inject {
        remote_member: member.to_string(),
        content: serde_json::to_value(meerkat_core::ContentInput::Text(text.to_string()))
            .expect("content"),
        nonce: Some("n".to_string()),
        caller: None,
    }
}

fn describe() -> ControlRequest {
    ControlRequest::HostDescribe {
        nonce: Some("n".to_string()),
        caller: None,
    }
}

fn health() -> ControlRequest {
    ControlRequest::HostHealth {
        nonce: Some("n".to_string()),
        caller: None,
    }
}

/// A member of a far mob, as a wire request describes it: a transport key
/// of its own, the peer id that key gives, a dialable address.
struct FarMember {
    comms_name: String,
    peer_id: String,
    pubkey_b64: String,
}

impl FarMember {
    fn new(comms_name: &str) -> Self {
        let key = GatewayPeerKeys::ephemeral();
        Self {
            comms_name: comms_name.to_string(),
            peer_id: meerkat_core::comms::PeerId::from_ed25519_pubkey(&key.pubkey_bytes())
                .to_string(),
            pubkey_b64: key.pubkey_b64(),
        }
    }

    fn wire(&self, member: &str) -> ControlRequest {
        ControlRequest::Wire {
            remote_member: member.to_string(),
            local_peer_spec_address: "tcp://127.0.0.1:9".to_string(),
            local_comms_name: self.comms_name.clone(),
            local_peer_id: self.peer_id.clone(),
            local_pubkey_b64: Some(self.pubkey_b64.clone()),
            nonce: Some("n".to_string()),
            caller: None,
        }
    }

    fn unwire(&self, member: &str) -> ControlRequest {
        ControlRequest::Unwire {
            remote_member: member.to_string(),
            local_peer_spec_address: "tcp://127.0.0.1:9".to_string(),
            local_comms_name: self.comms_name.clone(),
            local_peer_id: self.peer_id.clone(),
            local_pubkey_b64: Some(self.pubkey_b64.clone()),
            nonce: Some("n".to_string()),
            caller: None,
        }
    }
}

/// The clock of the scripted keyed inject plane.
const NOW_MS: u64 = 1_800_000_000_000;

fn keyed(member: &str, delivery_key: &str, body: &str) -> ControlRequest {
    ControlRequest::InjectKeyed {
        remote_member: member.to_string(),
        delivery_key: delivery_key.to_string(),
        content_digest: crate::mob_announcements::types::content_digest(body),
        summary: "1 announcement".to_string(),
        body: body.to_string(),
        issued_at_ms: NOW_MS,
        nonce: "n".to_string(),
        caller: None,
    }
}

fn keyed_status(member: &str, delivery_key: &str) -> ControlRequest {
    ControlRequest::InjectStatus {
        remote_member: member.to_string(),
        delivery_key: delivery_key.to_string(),
        with_input: false,
        issued_at_ms: NOW_MS,
        nonce: "n".to_string(),
        caller: None,
    }
}

/// One request of each control verb for `member`, named by its verb.
fn one_of_each_verb(member: &str) -> Vec<ControlRequest> {
    let far = FarMember::new("far-mob/worker/far");
    vec![
        far.wire(member),
        far.unwire(member),
        inject(member, "a word"),
        lookup(member),
        describe(),
        health(),
        keyed(member, "batch-1", "a body"),
        keyed_status(member, "batch-1"),
    ]
}

/// The code of a refusal, when `response` is one the listener's grant
/// policy or its handler wrote unsigned.
fn refused_with(response: &ControlResponse) -> Option<&str> {
    match response {
        ControlResponse::Err { code, .. } => Some(code.as_str()),
        _ => None,
    }
}

/// Whether the listener's grant policy refused the request: `response` is
/// one of the authorization refusals.
fn refused_by_grant(response: &ControlResponse) -> bool {
    refused_with(response).is_some_and(ControlAuthzDenial::is_denial_code)
}

/// A keyed inject plane a row scripts: it commits each delivery under its
/// key and writes into the receipt which mob's receiver took it. Its
/// receiver for another mob shares the log and has bindings of its own.
struct ScriptedPlane {
    /// Which mob this receiver serves, as the row names it.
    mob: String,
    /// The receivers of the other mobs of the host, by mob id.
    others: Mutex<std::collections::BTreeMap<String, Arc<ScriptedPlane>>>,
    /// `(mob, member, delivery key)` of every delivery any receiver of the
    /// plane took, in order.
    log: Arc<Mutex<Vec<(String, String, String)>>>,
    committed: Mutex<std::collections::BTreeMap<String, NativeReceipt>>,
}

impl ScriptedPlane {
    fn of_the_default_mob() -> Arc<Self> {
        Arc::new(Self {
            mob: "default".to_string(),
            others: Mutex::default(),
            log: Arc::default(),
            committed: Mutex::default(),
        })
    }

    /// Give the plane a receiver for the mob `mob_id`, named `mob` in the
    /// log.
    fn serving(self: &Arc<Self>, mob_id: &MobId, mob: &str) -> Arc<Self> {
        let other = Arc::new(Self {
            mob: mob.to_string(),
            others: Mutex::default(),
            log: Arc::clone(&self.log),
            committed: Mutex::default(),
        });
        self.others
            .lock()
            .expect("the plane's receivers")
            .insert(mob_id.to_string(), Arc::clone(&other));
        other
    }

    fn log(&self) -> Vec<(String, String, String)> {
        self.log.lock().expect("the plane's log").clone()
    }
}

impl KeyedInjectReceiver for ScriptedPlane {
    fn window_ms(&self) -> u64 {
        60_000
    }

    fn now_ms(&self) -> u64 {
        NOW_MS
    }

    fn deliver<'a>(
        &'a self,
        scope: &'a KeyedInjectCallerScope,
        delivery: KeyedInject<'a>,
        _deadline_ms: u64,
    ) -> KeyedInjectFuture<'a, Result<NativeReceipt, KeyedInjectRefusal>> {
        Box::pin(async move {
            let key = scope.native_key(delivery.delivery_key);
            self.log.lock().expect("the plane's log").push((
                self.mob.clone(),
                delivery.member.to_string(),
                delivery.delivery_key.to_string(),
            ));
            let mut committed = self.committed.lock().expect("the receiver's commits");
            let receipt = committed
                .entry(key.clone())
                .or_insert_with(|| NativeReceipt {
                    delivery_id: format!("delivery:{key}"),
                    address: format!("{}:{}", self.mob, delivery.member),
                    submission_digest: format!("submission:{key}"),
                    committed_at_ms: NOW_MS,
                });
            Ok(receipt.clone())
        })
    }

    fn status<'a>(
        &'a self,
        scope: &'a KeyedInjectCallerScope,
        _member: &'a str,
        delivery_key: &'a str,
        _with_input: bool,
    ) -> KeyedInjectFuture<'a, Result<KeyedInjectStatusAnswer, KeyedInjectRefusal>> {
        Box::pin(async move {
            let committed = self.committed.lock().expect("the receiver's commits");
            let status = match committed.get(&scope.native_key(delivery_key)) {
                None => NativeStatus::NotCommitted,
                Some(receipt) => NativeStatus::Pending {
                    receipt: receipt.clone(),
                    admitted: None,
                },
            };
            Ok(KeyedInjectStatusAnswer {
                status,
                input: None,
            })
        })
    }

    fn for_mob<'a>(
        &'a self,
        mob_id: &'a MobId,
    ) -> KeyedInjectFuture<'a, Option<Arc<dyn KeyedInjectReceiver>>> {
        Box::pin(async move {
            self.others
                .lock()
                .expect("the plane's receivers")
                .get(mob_id.as_str())
                .cloned()
                .map(|receiver| receiver as Arc<dyn KeyedInjectReceiver>)
        })
    }
}

/// The receipt of a keyed delivery the listener answered, when it answered
/// one.
fn receipt_of(response: &ControlResponse) -> Option<&NativeReceipt> {
    match response {
        ControlResponse::InjectReceipt { receipt, .. } => Some(receipt),
        _ => None,
    }
}

/// The native status of a keyed delivery the listener answered, when it
/// answered one.
fn status_of(response: &ControlResponse) -> Option<&NativeStatus> {
    match response {
        ControlResponse::InjectStatus { status, .. } => Some(status),
        _ => None,
    }
}

/// A host of two mobs, each with an agent `desk` of its own: the default
/// mob also has `front`, the second mob `back`. Its mob set built the
/// second mob for the host, and its requests resolve that mob.
struct Host {
    _temp: tempfile::TempDir,
    runtime: UnifiedRuntime,
    sessions: Sessions,
    default_id: MobId,
    second_id: MobId,
    second: meerkat_mob::MobHandle,
}

impl Host {
    async fn of_two_mobs() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let (mut runtime, sessions) = launch(&temp, definition("host")).await;
        let default_id = runtime.mob_handle().mob_id().clone();
        let second_id = second_mob(&runtime, definition("second")).await;
        let second = set_of(&runtime)
            .handle_for(&second_id)
            .await
            .expect("the set serves the second mob");
        let of_the_default = identity_context(runtime.mob_runtime(), "default");
        Box::pin(runtime.install_and_bootstrap_identity_first_context(
            Arc::clone(&of_the_default),
            &[configured_agent("desk"), configured_agent("front")],
        ))
        .await
        .expect("the default mob's identity bootstrap");
        {
            let mob = runtime
                .resolve_mob(Some(&second_id))
                .await
                .expect("the second mob of the host resolves");
            let of_the_second = identity_context(mob.mob_runtime(), "second");
            mob.attach_identity_first_context(Arc::clone(&of_the_second))
                .await
                .expect("a host attaches an identity context to its second mob");
            Box::pin(
                of_the_second
                    .bootstrap_roster(&[configured_agent("desk"), configured_agent("back")]),
            )
            .await
            .expect("the second mob's roster");
        }
        Self {
            _temp: temp,
            runtime,
            sessions,
            default_id,
            second_id,
            second,
        }
    }

    /// Bind the host's control listener with `grant` filed for one peer
    /// gateway, as a gateway binds it: the authorizer's audience is the
    /// host's default mob.
    async fn listen(&self, grant: ControlGrant) -> Peer {
        let keys = GatewayPeerKeys::ephemeral();
        let authorizer = authorizer_of(&keys, self.default_id.as_str(), grant);
        self.listen_with(keys, authorizer).await
    }

    async fn listen_with(&self, keys: GatewayPeerKeys, authorizer: ControlAuthorizer) -> Peer {
        let advertised = self
            .runtime
            .start_control_listener_with_authorizer(&listen_address(), Arc::new(authorizer))
            .await
            .expect("the control listener binds");
        Peer {
            keys,
            endpoint: endpoint_of(&advertised),
        }
    }

    /// What the control handler logic answers `request` over the entry of
    /// the mob `mob_id`, asked directly: no listener, no grant.
    async fn straight_from(&self, mob_id: &MobId, request: ControlRequest) -> ControlResponse {
        let mob = self
            .runtime
            .mob_resolver()
            .resolve(Some(mob_id))
            .await
            .expect("the mob resolves");
        let mut handler = match mob.identity_runtime() {
            Some(identities) => {
                MobHandleControlHandler::with_identity_runtime(mob.mob_handle(), identities)
            }
            None => MobHandleControlHandler::new(mob.mob_handle()),
        };
        if let Some(service) = mob.mob_runtime().session_service() {
            handler = handler.with_session_service(Arc::clone(service));
        }
        handler.handle(request).await
    }

    /// The session of the agent `desk` of the mob behind `handle`.
    async fn desk_session(handle: &meerkat_mob::MobHandle) -> meerkat_core::types::SessionId {
        handle
            .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from("desk"))
            .await
            .expect("the desk's session")
    }

    /// Whether a member of the mob behind `handle` is wired to the far
    /// member `far`.
    async fn wired_to(handle: &meerkat_mob::MobHandle, far: &FarMember) -> bool {
        handle
            .list_members_including_retiring()
            .await
            .iter()
            .any(|member| {
                member
                    .wired_to
                    .iter()
                    .any(|peer| peer.as_str() == far.comms_name)
            })
    }

    async fn shut_down(self) {
        shut_down(&self.runtime, &[&self.second_id]).await;
    }
}

/// A signed request reaches the mob its audience names. Two mobs of one
/// host each have an agent `desk`: a lookup signed for the second mob
/// answers the second mob's `desk`, one signed for the default mob the
/// default mob's, and the two are two members. An agent only one mob has
/// is found in that mob and in no other.
#[tokio::test]
async fn control_request_reaches_the_mob_its_audience_names() {
    let host = Host::of_two_mobs().await;
    let peer = host.listen(grant_over(Some(ControlMobScope::All))).await;
    let (default_mob, second_mob) = (host.default_id.clone(), host.second_id.clone());

    let of_the_default = host.straight_from(&default_mob, lookup("desk")).await;
    let of_the_second = host.straight_from(&second_mob, lookup("desk")).await;
    assert!(
        matches!(of_the_default, ControlResponse::Member { .. })
            && matches!(of_the_second, ControlResponse::Member { .. })
            && of_the_default != of_the_second,
        "two members, one name: {of_the_default:?}, {of_the_second:?}"
    );

    let for_the_default = peer.ask(Some(default_mob.as_str()), lookup("desk")).await;
    let for_the_second = peer.ask(Some(second_mob.as_str()), lookup("desk")).await;
    assert_eq!(
        for_the_default, of_the_default,
        "a request signed for the default mob answers the default mob's member"
    );
    assert_eq!(
        for_the_second, of_the_second,
        "a request signed for the second mob answers the second mob's member"
    );

    // An agent only one mob has: found there, and nowhere else.
    for (member, audience, found) in [
        ("front", &default_mob, true),
        ("front", &second_mob, false),
        ("back", &second_mob, true),
        ("back", &default_mob, false),
    ] {
        let answer = peer.ask(Some(audience.as_str()), lookup(member)).await;
        assert_eq!(
            matches!(answer, ControlResponse::Member { .. }),
            found,
            "{member} for {audience}: {answer:?}"
        );
        assert!(
            !refused_by_grant(&answer),
            "the grant reaches both mobs: {answer:?}"
        );
    }
    host.shut_down().await;
}

/// Every verb that names a member acts on the member of the mob the signed
/// audience names, and on no other mob's member of that name: an inject
/// lands in that mob's `desk` session alone, a wire and an unwire change
/// that mob's roster alone.
#[tokio::test]
async fn every_member_verb_reaches_the_mob_of_the_signed_audience_and_no_other() {
    const TO_THE_SECOND: &str = "a word for the desk of the second mob";
    const TO_THE_DEFAULT: &str = "a word for the desk of the default mob";
    let host = Host::of_two_mobs().await;
    let peer = host.listen(grant_over(Some(ControlMobScope::All))).await;
    let (default_mob, second_mob) = (host.default_id.clone(), host.second_id.clone());
    let default_handle = host.runtime.mob_handle();
    let default_desk = Host::desk_session(&default_handle).await;
    let second_desk = Host::desk_session(&host.second).await;
    assert_ne!(default_desk, second_desk, "two members, one name");

    // Inject.
    let to_the_second = peer
        .ask(Some(second_mob.as_str()), inject("desk", TO_THE_SECOND))
        .await;
    assert_eq!(
        to_the_second,
        ControlResponse::Injected {
            session_id: second_desk.to_string(),
            sig_b64: None,
        },
        "an inject signed for the second mob is admitted by the second mob's member"
    );
    let to_the_default = peer
        .ask(Some(default_mob.as_str()), inject("desk", TO_THE_DEFAULT))
        .await;
    assert_eq!(
        to_the_default,
        ControlResponse::Injected {
            session_id: default_desk.to_string(),
            sig_b64: None,
        },
        "an inject signed for the default mob is admitted by the default mob's member"
    );
    transcript_showing(&host.sessions, &second_desk, TO_THE_SECOND).await;
    transcript_showing(&host.sessions, &default_desk, TO_THE_DEFAULT).await;
    let (of_the_default_desk, of_the_second_desk) = (
        transcript(&host.sessions, &default_desk).await,
        transcript(&host.sessions, &second_desk).await,
    );
    assert!(
        of_the_second_desk.contains(TO_THE_SECOND) && !of_the_second_desk.contains(TO_THE_DEFAULT),
        "the second mob's member got its own inject alone: {of_the_second_desk}"
    );
    assert!(
        of_the_default_desk.contains(TO_THE_DEFAULT)
            && !of_the_default_desk.contains(TO_THE_SECOND),
        "the default mob's member got its own inject alone: {of_the_default_desk}"
    );

    // Wire, then unwire, for the second mob.
    let far = FarMember::new("far-mob/worker/far");
    let wired = peer.ask(Some(second_mob.as_str()), far.wire("desk")).await;
    assert_eq!(
        wired,
        ControlResponse::Ok { sig_b64: None },
        "a wire signed for the second mob"
    );
    assert!(
        Host::wired_to(&host.second, &far).await,
        "the edge is in the second mob's roster"
    );
    assert!(
        !Host::wired_to(&default_handle, &far).await,
        "and the default mob's member of that name is untouched"
    );
    let unwired = peer
        .ask(Some(second_mob.as_str()), far.unwire("desk"))
        .await;
    assert_eq!(
        unwired,
        ControlResponse::Ok { sig_b64: None },
        "an unwire signed for the second mob"
    );
    assert!(
        !Host::wired_to(&host.second, &far).await,
        "the edge left the second mob's roster"
    );

    // Wire for the default mob: the second mob's member is untouched.
    let other_far = FarMember::new("far-mob/worker/other");
    let wired = peer
        .ask(Some(default_mob.as_str()), other_far.wire("desk"))
        .await;
    assert_eq!(wired, ControlResponse::Ok { sig_b64: None });
    assert!(Host::wired_to(&default_handle, &other_far).await);
    assert!(
        !Host::wired_to(&host.second, &other_far).await,
        "a wire signed for the default mob leaves the second mob's member untouched"
    );
    host.shut_down().await;
}

/// The keyed inject verbs reach the receiver of the mob the signed audience
/// names: a delivery signed for the second mob is committed by the second
/// mob's receiver and its status is read there; the default mob's receiver
/// takes no delivery signed for another mob, and holds no commit under that
/// key. An identical replay returns the original receipt.
#[tokio::test]
async fn keyed_inject_verbs_reach_the_receiver_of_the_audiences_mob() {
    let host = Host::of_two_mobs().await;
    let (default_mob, second_mob) = (host.default_id.clone(), host.second_id.clone());
    let plane = ScriptedPlane::of_the_default_mob();
    plane.serving(&second_mob, "second");
    assert!(
        host.runtime.seal_keyed_inject(KeyedInjectServing::Serving(
            Arc::clone(&plane) as Arc<dyn KeyedInjectReceiver>
        )),
        "the row seals the host's keyed inject plane"
    );
    let peer = host.listen(grant_over(Some(ControlMobScope::All))).await;

    let delivered = peer
        .ask(
            Some(second_mob.as_str()),
            keyed("desk", "batch-1", "a body"),
        )
        .await;
    assert_eq!(
        receipt_of(&delivered).map(|receipt| receipt.address.as_str()),
        Some("second:desk"),
        "a delivery signed for the second mob is committed by that mob's receiver: {delivered:?}"
    );
    let replayed = peer
        .ask(
            Some(second_mob.as_str()),
            keyed("desk", "batch-1", "a body"),
        )
        .await;
    assert_eq!(
        receipt_of(&replayed),
        receipt_of(&delivered),
        "an identical replay returns the original receipt"
    );
    let in_the_second = peer
        .ask(Some(second_mob.as_str()), keyed_status("desk", "batch-1"))
        .await;
    assert!(
        matches!(status_of(&in_the_second), Some(NativeStatus::Pending { receipt, .. })
            if Some(receipt) == receipt_of(&delivered)),
        "the status signed for the second mob is that mob's: {in_the_second:?}"
    );
    let in_the_default = peer
        .ask(Some(default_mob.as_str()), keyed_status("desk", "batch-1"))
        .await;
    assert_eq!(
        status_of(&in_the_default),
        Some(&NativeStatus::NotCommitted),
        "the default mob holds no commit under the key: {in_the_default:?}"
    );
    assert_eq!(
        plane.log(),
        vec![
            (
                "second".to_string(),
                "desk".to_string(),
                "batch-1".to_string()
            );
            2
        ],
        "no receiver but the second mob's took the delivery"
    );

    let to_the_default = peer
        .ask(
            Some(default_mob.as_str()),
            keyed("desk", "batch-2", "a body"),
        )
        .await;
    assert_eq!(
        receipt_of(&to_the_default).map(|receipt| receipt.address.as_str()),
        Some("default:desk"),
        "a delivery signed for the default mob is the default mob's: {to_the_default:?}"
    );
    host.shut_down().await;
}

/// A mob the caller's grant does not reach answers exactly as a mob the
/// host does not have, for every verb: the same request gets the same
/// answer while the host serves the mob and after the mob is gone. The
/// answer is the one refusal a request minted for another gateway gets.
#[tokio::test]
async fn a_mob_the_grant_does_not_reach_answers_as_a_mob_the_host_does_not_have() {
    let host = Host::of_two_mobs().await;
    // A grant without mobs reaches the default mob only.
    let peer = host.listen(grant_over(None)).await;
    let second_mob = host.second_id.clone();
    let requests = one_of_each_verb("desk");

    let mut while_hosted = Vec::new();
    for request in &requests {
        while_hosted.push(peer.ask(Some(second_mob.as_str()), request.clone()).await);
    }
    Box::pin(set_of(&host.runtime).mob_destroy(&second_mob))
        .await
        .expect("the set destroys the second mob");
    let mut when_gone = Vec::new();
    for request in &requests {
        when_gone.push(peer.ask(Some(second_mob.as_str()), request.clone()).await);
    }

    for ((request, hosted), gone) in requests.iter().zip(&while_hosted).zip(&when_gone) {
        let verb = request.verb();
        assert_eq!(
            hosted, gone,
            "{verb}: a mob outside the grant and a mob the host does not have get one answer"
        );
        assert_eq!(
            refused_with(hosted),
            Some("audience_mismatch"),
            "{verb}: {hosted:?}"
        );
    }
    shut_down(&host.runtime, &[]).await;
}

/// The audience check reads the mob set at each request: a mob the set
/// builds for the host after the listener started is an audience of the
/// host, and a mob the set destroyed no longer is.
#[tokio::test]
async fn audience_check_reads_the_mob_set() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (runtime, _sessions) = launch(&temp, definition("host")).await;
    let default_id = runtime.mob_handle().mob_id().clone();
    let keys = GatewayPeerKeys::ephemeral();
    let advertised = runtime
        .start_control_listener_with_authorizer(
            &listen_address(),
            Arc::new(authorizer_of(
                &keys,
                default_id.as_str(),
                grant_over(Some(ControlMobScope::All)),
            )),
        )
        .await
        .expect("the control listener binds");
    let peer = Peer {
        keys,
        endpoint: endpoint_of(&advertised),
    };
    let later = definition("later");
    let later_id = later.id.clone();

    let before = peer.ask(Some(later_id.as_str()), lookup("desk")).await;
    assert_eq!(
        refused_with(&before),
        Some("audience_mismatch"),
        "the host does not have the mob yet: {before:?}"
    );

    assert_eq!(second_mob(&runtime, later).await, later_id);
    let served = peer.ask(Some(later_id.as_str()), lookup("desk")).await;
    assert!(
        !refused_by_grant(&served),
        "a mob created after the listener started is an audience of the host: {served:?}"
    );

    Box::pin(set_of(&runtime).mob_destroy(&later_id))
        .await
        .expect("the set destroys the mob");
    let after = peer.ask(Some(later_id.as_str()), lookup("desk")).await;
    assert_eq!(
        after, before,
        "a destroyed mob is answered as the mob the host never had"
    );
    shut_down(&runtime, &[]).await;
}

/// A mob a member created is no audience of the host for a peer gateway,
/// also under a grant for every mob: a request signed for it is answered
/// as one for a mob the host does not have.
#[tokio::test]
async fn a_mob_a_member_created_is_no_audience_of_the_host() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (runtime, _sessions) = launch(&temp, definition("host")).await;
    let default_id = runtime.mob_handle().mob_id().clone();
    Box::pin(runtime.spawn(meerkat_mob::SpawnMemberSpec::new("worker", "owner")))
        .await
        .expect("the owner member");
    let owner_session = runtime
        .mob_handle()
        .resolve_bridge_session_id(&meerkat_mob::AgentIdentity::from("owner"))
        .await
        .expect("the owner member's session");
    let child_id = Box::pin(
        set_of(&runtime).mob_create_definition_with_owner_bridge_session(
            definition("child"),
            owner_session,
            true,
            false,
        ),
    )
    .await
    .expect("a mob the member owns");
    let keys = GatewayPeerKeys::ephemeral();
    let advertised = runtime
        .start_control_listener_with_authorizer(
            &listen_address(),
            Arc::new(authorizer_of(
                &keys,
                default_id.as_str(),
                grant_over(Some(ControlMobScope::All)),
            )),
        )
        .await
        .expect("the control listener binds");
    let peer = Peer {
        keys,
        endpoint: endpoint_of(&advertised),
    };

    let for_the_child = peer.ask(Some(child_id.as_str()), lookup("owner")).await;
    let for_no_mob = peer.ask(Some("no-such-mob"), lookup("owner")).await;
    assert_eq!(
        refused_with(&for_the_child),
        Some("audience_mismatch"),
        "a mob a member created is no audience of the host: {for_the_child:?}"
    );
    assert_eq!(refused_with(&for_no_mob), Some("audience_mismatch"));
    // Positive control: the owner is found in the default mob.
    let in_the_default = peer.ask(Some(default_id.as_str()), lookup("owner")).await;
    assert!(
        matches!(in_the_default, ControlResponse::Member { .. }),
        "got {in_the_default:?}"
    );
    shut_down(&runtime, &[&child_id]).await;
}

/// A gateway refuses a request made with its own key whatever mob the
/// request is signed for and whatever its key's grant says.
#[tokio::test]
async fn a_gateway_refuses_its_own_key_for_every_mob() {
    let mut host = Host::of_two_mobs().await;
    let own_keys = GatewayPeerKeys::ephemeral();
    host.runtime.set_gateway_peer_keys(own_keys.clone());
    let authorizer = authorizer_of(
        &own_keys,
        host.default_id.as_str(),
        grant_over(Some(ControlMobScope::All)),
    );
    let peer = host.listen_with(own_keys, authorizer).await;

    for audience in [host.default_id.clone(), host.second_id.clone()] {
        for request in [lookup("desk"), inject("desk", "a word"), describe()] {
            let verb = request.verb();
            let answer = peer.ask(Some(audience.as_str()), request).await;
            assert_eq!(
                refused_with(&answer),
                Some(HOST_OWN_KEY_CONTROL_CODE),
                "{verb} for {audience}: {answer:?}"
            );
        }
    }
    host.shut_down().await;
}

/// A runtime as a launch builds it serves a second mob of the host through
/// its control listener, with nothing opened for it: under a grant that
/// reaches every mob a request signed for the second mob is served on that
/// mob (a member the mob has is found, a name it does not have is refused
/// by that mob's handler, and the member is no member of the default mob).
/// Under a grant that does not reach the mob the same request is answered
/// as one for a mob the host does not have.
#[tokio::test]
async fn a_launchs_listener_serves_a_second_mob_of_the_host_under_a_grant_that_reaches_it() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (runtime, _sessions) = launch(&temp, definition("host")).await;
    let default_id = runtime.mob_handle().mob_id().clone();
    let second_id = second_mob(&runtime, definition("second")).await;
    let second = set_of(&runtime)
        .handle_for(&second_id)
        .await
        .expect("the set serves the second mob");
    let seated =
        Box::pin(second.spawn_spec(meerkat_mob::SpawnMemberSpec::new("worker", "back"))).await;
    // The launch's record has read the mob the host created before a
    // request asks: a request that met that reading under way would wait
    // for it, and the row asks for the answers after it.
    runtime
        .mob_runtime()
        .private_owners()
        .read_of_the_mob_awaited(second_id.as_str(), Duration::from_secs(30))
        .await;
    // One listener, two peer gateways: one whose grant reaches every mob,
    // one whose grant names no mob and so reaches the default mob only.
    let (reaching, outside) = (GatewayPeerKeys::ephemeral(), GatewayPeerKeys::ephemeral());
    let mut table = ControlGrantTable::new();
    table.insert(
        reaching.pubkey_bytes(),
        grant_over(Some(ControlMobScope::All)),
    );
    table.insert(
        outside.pubkey_bytes(),
        ControlGrant::new(
            "another-peer-gateway",
            ControlVerb::all().iter().copied(),
            ControlMemberScope::All,
        ),
    );
    let advertised = runtime
        .start_control_listener_with_authorizer(
            &listen_address(),
            Arc::new(ControlAuthorizer::with_grants_for_audience(
                table,
                default_id.as_str(),
            )),
        )
        .await
        .expect("the control listener binds");
    let peer = Peer {
        keys: reaching,
        endpoint: endpoint_of(&advertised),
    };

    let for_the_member = peer.ask(Some(second_id.as_str()), lookup("back")).await;
    let for_no_member = peer.ask(Some(second_id.as_str()), lookup("nobody")).await;
    let in_the_default = peer.ask(Some(default_id.as_str()), lookup("back")).await;
    let outside_the_grant = peer
        .ask_as(&outside, Some(second_id.as_str()), lookup("back"))
        .await;
    let for_no_mob = peer
        .ask_as(&outside, Some("no-such-mob"), lookup("back"))
        .await;
    shut_down(&runtime, &[&second_id]).await;

    assert_ne!(
        refused_with(&for_the_member),
        Some("audience_mismatch"),
        "a launch's listener serves a second mob of the host under a grant that reaches it: \
         {for_the_member:?}"
    );
    assert!(
        seated.is_ok(),
        "a member of the second mob: {:?}",
        seated.as_ref().err()
    );
    assert!(
        matches!(for_the_member, ControlResponse::Member { .. }),
        "the request is served on the second mob, whose member is found: {for_the_member:?}"
    );
    assert!(
        matches!(for_no_member, ControlResponse::Err { .. }) && !refused_by_grant(&for_no_member),
        "a name no member of the second mob has is refused by that mob's handler: \
         {for_no_member:?}"
    );
    assert!(
        !matches!(in_the_default, ControlResponse::Member { .. })
            && !refused_by_grant(&in_the_default),
        "the member is the second mob's and no member of the default mob: {in_the_default:?}"
    );
    assert_eq!(
        refused_with(&outside_the_grant),
        Some("audience_mismatch"),
        "a grant that does not reach the second mob: {outside_the_grant:?}"
    );
    // The refusal names the id the caller sent and nothing else of a mob.
    assert_eq!(
        format!("{outside_the_grant:?}").replace(second_id.as_str(), "<mob>"),
        format!("{for_no_mob:?}").replace("no-such-mob", "<mob>"),
        "a mob outside the grant and a mob the host does not have get one answer"
    );
}

/// A request signed for the id of the host's default mob reaches the
/// default mob whatever audience the listener's authorizer was built for.
/// A host that binds its listener under another name for itself still has
/// one default mob: the mob set's.
#[tokio::test]
async fn the_default_mobs_id_reaches_the_default_mob_under_any_bound_audience() {
    let host = Host::of_two_mobs().await;
    let keys = GatewayPeerKeys::ephemeral();
    let authorizer = authorizer_of(
        &keys,
        "the-hosts-other-name",
        grant_over(Some(ControlMobScope::All)),
    );
    let peer = host.listen_with(keys, authorizer).await;
    let of_the_default = host
        .straight_from(&host.default_id.clone(), lookup("front"))
        .await;
    assert!(
        matches!(of_the_default, ControlResponse::Member { .. }),
        "got {of_the_default:?}"
    );

    let by_the_bound_name = peer
        .ask(Some("the-hosts-other-name"), lookup("front"))
        .await;
    let by_the_mobs_id = peer
        .ask(Some(host.default_id.as_str()), lookup("front"))
        .await;
    assert_eq!(by_the_bound_name, of_the_default);
    assert_eq!(
        by_the_mobs_id, of_the_default,
        "the default mob's own id names the default mob"
    );
    host.shut_down().await;
}

/// Two compositions and a resolver that serves the second as a second mob
/// of the first and records each question: the fixture of the rows that
/// count what a request asked of the mob set and what it loaded.
struct Recorded {
    _temps: (tempfile::TempDir, tempfile::TempDir),
    first: UnifiedRuntime,
    second: UnifiedRuntime,
    resolver: MobResolver,
    second_id: MobId,
}

impl Recorded {
    /// `loaded`: whether the second mob stands as loaded, or as a mob the
    /// set knows and has not loaded.
    async fn new(loaded: bool) -> Self {
        let temps = (
            tempfile::tempdir().expect("tempdir"),
            tempfile::tempdir().expect("tempdir"),
        );
        let first = plain_runtime(&temps.0).await;
        let second = plain_runtime(&temps.1).await;
        let resolver = first
            .mob_resolver()
            .serving_a_second_mob_in_tests(&second.mob_resolver());
        let resolver = if loaded {
            resolver
        } else {
            resolver.not_loaded_in_tests()
        };
        let second_id = second.mob_handle().mob_id().clone();
        Self {
            _temps: temps,
            first,
            second,
            resolver,
            second_id,
        }
    }

    /// Bind the first composition's control listener over the recording
    /// resolver.
    async fn listen_with(&self, keys: GatewayPeerKeys, authorizer: ControlAuthorizer) -> Peer {
        let advertised = self
            .first
            .start_control_listener_over(
                &listen_address(),
                Arc::new(authorizer),
                self.resolver.clone(),
            )
            .await
            .expect("the control listener binds");
        Peer {
            keys,
            endpoint: endpoint_of(&advertised),
        }
    }

    async fn listen(&self, grant: ControlGrant) -> Peer {
        let keys = GatewayPeerKeys::ephemeral();
        let authorizer = authorizer_of(&keys, self.first.mob_id().as_str(), grant);
        self.listen_with(keys, authorizer).await
    }

    /// The mobs the listener asked the mob set about, without a load.
    fn asked(&self) -> Vec<Option<MobId>> {
        self.resolver.asked_whether_served_in_tests()
    }

    /// The mobs the listener resolved: a resolve is what loads a mob.
    fn resolved(&self) -> Vec<Option<MobId>> {
        self.resolver.resolved_for_in_tests()
    }

    async fn shut_down(self) {
        self.first.shutdown().await;
        self.second.shutdown().await;
    }
}

/// The host is not asked about a mob the caller's grant does not reach,
/// and no such request resolves a mob: the refusal is decided from the
/// grant alone, for every verb.
#[tokio::test]
async fn the_host_is_not_asked_about_a_mob_the_grant_does_not_reach() {
    let fixture = Recorded::new(true).await;
    let peer = fixture.listen(grant_over(None)).await;

    for request in one_of_each_verb("desk") {
        let verb = request.verb();
        let answer = peer.ask(Some(fixture.second_id.as_str()), request).await;
        assert_eq!(
            refused_with(&answer),
            Some("audience_mismatch"),
            "{verb}: {answer:?}"
        );
    }
    assert_eq!(
        fixture.asked(),
        Vec::<Option<MobId>>::new(),
        "the mob set is not asked about a mob the grant does not reach"
    );
    assert_eq!(
        fixture.resolved(),
        Vec::<Option<MobId>>::new(),
        "and no mob is resolved"
    );
    fixture.shut_down().await;
}

/// A request the grant policy refuses loads no mob. A grant that reaches
/// the second mob and lacks the verb, or the member, asks the mob set
/// whether the mob is served (which loads nothing) and resolves no mob.
#[tokio::test]
async fn refused_request_loads_no_mob() {
    let fixture = Recorded::new(false).await;
    let grant = ControlGrant::new(
        PEER,
        [ControlVerb::LookupMember],
        ControlMemberScope::members(["front"]),
    )
    .with_mobs(ControlMobScope::All);
    let peer = fixture.listen(grant).await;
    let second = fixture.second_id.clone();

    let lacks_the_verb = peer
        .ask(Some(second.as_str()), inject("front", "a word"))
        .await;
    assert_eq!(
        refused_with(&lacks_the_verb),
        Some("verb_not_granted"),
        "got {lacks_the_verb:?}"
    );
    let lacks_the_member = peer.ask(Some(second.as_str()), lookup("desk")).await;
    assert_eq!(
        refused_with(&lacks_the_member),
        Some("member_not_granted"),
        "got {lacks_the_member:?}"
    );
    let no_such_mob = peer.ask(Some("no-such-mob"), lookup("front")).await;
    assert_eq!(
        refused_with(&no_such_mob),
        Some("audience_mismatch"),
        "got {no_such_mob:?}"
    );
    assert_eq!(
        fixture.asked(),
        vec![
            Some(second.clone()),
            Some(second.clone()),
            Some(MobId::from("no-such-mob")),
        ],
        "one question per request whose grant reaches the id, answered without a load"
    );
    assert_eq!(
        fixture.resolved(),
        Vec::<Option<MobId>>::new(),
        "a refused request resolves no mob, so it loads none"
    );
    fixture.shut_down().await;
}

/// The host verbs name no member: a host verb signed for a second mob that
/// is not loaded is answered from the host's own facts and loads no mob.
#[tokio::test]
async fn host_describe_loads_no_mob() {
    let fixture = Recorded::new(false).await;
    let peer = fixture.listen(grant_over(Some(ControlMobScope::All))).await;
    let second = fixture.second_id.clone();
    let first_id = fixture.first.mob_id();

    for request in [describe(), health()] {
        let verb = request.verb();
        let for_the_second = peer.ask(Some(second.as_str()), request.clone()).await;
        let for_the_default = peer.ask(Some(first_id.as_str()), request).await;
        assert!(
            !refused_by_grant(&for_the_second),
            "{verb}: a mob in reach is an audience of the host: {for_the_second:?}"
        );
        assert_eq!(
            for_the_second, for_the_default,
            "{verb}: the host's facts are the same whatever mob the request is signed for"
        );
    }
    assert_eq!(
        fixture.resolved(),
        Vec::<Option<MobId>>::new(),
        "a host verb resolves no mob, so it loads none"
    );
    fixture.shut_down().await;
}

/// A verb that names a member, signed for a mob of the host that is not
/// loaded, resolves that mob: the one lookup that loads it. The keyed
/// inject verbs do too.
///
/// The count of this row is the fixture's, not a mob set's. Its resolver
/// is a stand-in whose second mob never stands as loaded, also after a
/// resolve. The handler of a member verb reads its mob again after its
/// wait from what is loaded, and resolves a second time only where the mob
/// is not loaded then: here always, on a mob set never but for a mob that
/// was closed meanwhile (the next row counts one). A keyed verb's handler
/// does not wait and resolves once.
#[tokio::test]
async fn control_request_wakes_a_mob_that_is_not_loaded() {
    let fixture = Recorded::new(false).await;
    let second = fixture.second_id.clone();
    let plane = ScriptedPlane::of_the_default_mob();
    plane.serving(&second, "second");
    assert!(
        fixture.first.seal_keyed_inject(KeyedInjectServing::Serving(
            Arc::clone(&plane) as Arc<dyn KeyedInjectReceiver>
        )),
        "the row seals the host's keyed inject plane"
    );
    let peer = fixture.listen(grant_over(Some(ControlMobScope::All))).await;

    let answer = peer.ask(Some(second.as_str()), lookup("desk")).await;
    assert!(
        !refused_by_grant(&answer),
        "the grant reaches the mob: {answer:?}"
    );
    assert_eq!(
        fixture.resolved(),
        vec![Some(second.clone()), Some(second.clone())],
        "a lookup resolves the mob it is signed for, and once more after its wait because \
         this fixture's mob does not stand as loaded then"
    );

    let keyed_answer = peer
        .ask(Some(second.as_str()), keyed_status("desk", "batch-1"))
        .await;
    assert_eq!(
        status_of(&keyed_answer),
        Some(&NativeStatus::NotCommitted),
        "the second mob's receiver answers its own status: {keyed_answer:?}"
    );
    assert_eq!(
        fixture.resolved().len(),
        3,
        "a keyed inject verb resolves the mob it is signed for, once"
    );
    assert!(
        fixture
            .resolved()
            .iter()
            .all(|resolved| resolved.as_ref() == Some(&second)),
        "no request resolved another mob: {:?}",
        fixture.resolved()
    );
    fixture.shut_down().await;
}

/// A verb that names a member resolves its mob once where the mob stands
/// as loaded after that resolve, as every mob a mob set has loaded does:
/// what the handler reads of the mob after its wait is read from what is
/// loaded, which is no resolve and loads nothing.
#[tokio::test]
async fn a_member_verb_resolves_its_mob_once_where_the_mob_stands_as_loaded() {
    let fixture = Recorded::new(true).await;
    let second = fixture.second_id.clone();
    let peer = fixture.listen(grant_over(Some(ControlMobScope::All))).await;

    let answer = peer.ask(Some(second.as_str()), lookup("desk")).await;
    assert!(
        !refused_by_grant(&answer),
        "the grant reaches the mob: {answer:?}"
    );
    assert_eq!(
        fixture.resolved(),
        vec![Some(second.clone())],
        "a lookup resolves the mob it is signed for, once"
    );

    let again = peer.ask(Some(second.as_str()), lookup("nobody")).await;
    assert!(
        !refused_by_grant(&again),
        "the grant reaches the mob: {again:?}"
    );
    assert_eq!(
        fixture.resolved(),
        vec![Some(second.clone()), Some(second.clone())],
        "one resolve for each member verb"
    );
    fixture.shut_down().await;
}

/// The listener routes on the mob its authorizer resolved and on nothing
/// else. A listener that enforces no grants, and one whose authorizer was
/// told no audience, never check the audience: every request is the
/// default mob's, whatever its envelope names, and the mob set is asked
/// nothing.
#[tokio::test]
async fn the_listener_routes_on_the_authorizers_answer_not_on_the_envelope() {
    for open in [true, false] {
        let fixture = Recorded::new(true).await;
        let keys = GatewayPeerKeys::ephemeral();
        let authorizer = if open {
            ControlAuthorizer::open()
        } else {
            let mut table = ControlGrantTable::new();
            table.insert(keys.pubkey_bytes(), grant_over(Some(ControlMobScope::All)));
            ControlAuthorizer::with_grants(table)
        };
        let peer = fixture.listen_with(keys, authorizer).await;
        let first_id = fixture.first.mob_id();
        let of_the_default = peer.ask(Some(first_id.as_str()), lookup("desk")).await;

        let naming_the_second = peer
            .ask(Some(fixture.second_id.as_str()), lookup("desk"))
            .await;
        assert_eq!(
            naming_the_second, of_the_default,
            "open {open}: an audience the listener does not check selects no mob"
        );
        if open {
            let unsigned = peer.ask_unsigned(lookup("desk")).await;
            assert_eq!(unsigned, of_the_default);
        }
        assert_eq!(
            (fixture.asked(), fixture.resolved()),
            (Vec::new(), Vec::new()),
            "open {open}: the mob set is asked nothing"
        );
        fixture.shut_down().await;
    }
}

/// What the control listener leaves in the runtime holds the default mob's
/// observer, never a public owner. The listener's task keeps a resolver of
/// the host's mobs for its life; a resolver that carried the default mob's
/// public owner would keep the mob owned for as long as the listener ran.
/// So a runtime dropped without `shutdown()` while its listener is up still
/// has its idle mob close admission for the released last owner, and
/// settles.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dropped_runtime_settles_with_its_control_listener_up() {
    let definition = meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"control-listener-drop-{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\n",
        uuid::Uuid::new_v4().simple()
    ))
    .expect("mob definition");
    let runtime = UnifiedRuntime::builder()
        .definition(definition)
        .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
        .build()
        .await
        .expect("runtime builds");
    let keys = GatewayPeerKeys::ephemeral();
    let default_id = runtime.mob_id();
    let advertised = runtime
        .start_control_listener_with_authorizer(
            &listen_address(),
            Arc::new(authorizer_of(
                &keys,
                default_id.as_str(),
                grant_over(Some(ControlMobScope::All)),
            )),
        )
        .await
        .expect("the control listener binds");
    // The listener serves: its task is up and holds what it holds.
    let peer = Peer {
        keys,
        endpoint: endpoint_of(&advertised),
    };
    let answer = peer.ask(Some(default_id.as_str()), lookup("nobody")).await;
    assert!(
        !refused_by_grant(&answer),
        "the listener serves the default mob: {answer:?}"
    );

    let mut closed = runtime.mob_handle().observer().admission_closed();
    drop(runtime);
    let reason = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(reason) = *closed.borrow_and_update() {
                return Some(reason);
            }
            if closed.changed().await.is_err() {
                return None;
            }
        }
    })
    .await
    .expect("the dropped runtime's idle mob closes its admission");
    assert_eq!(
        reason,
        Some(meerkat_mob::MobAdmissionCloseReason::LastOwnerReleased)
    );
}

/// The clock the host's own keyed inject plane reads: the system's, in
/// milliseconds.
fn clock_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

/// A keyed delivery for `member` issued at `issued_at_ms`.
fn keyed_at(member: &str, delivery_key: &str, body: &str, issued_at_ms: u64) -> ControlRequest {
    ControlRequest::InjectKeyed {
        remote_member: member.to_string(),
        delivery_key: delivery_key.to_string(),
        content_digest: crate::mob_announcements::types::content_digest(body),
        summary: "1 announcement".to_string(),
        body: body.to_string(),
        issued_at_ms,
        nonce: "n".to_string(),
        caller: None,
    }
}

/// A status read of `member`'s delivery `delivery_key` issued at
/// `issued_at_ms`.
fn keyed_status_at(member: &str, delivery_key: &str, issued_at_ms: u64) -> ControlRequest {
    ControlRequest::InjectStatus {
        remote_member: member.to_string(),
        delivery_key: delivery_key.to_string(),
        with_input: false,
        issued_at_ms,
        nonce: "n".to_string(),
        caller: None,
    }
}

/// The code and the typed cause of a keyed inject refusal, when `response`
/// is one the serving side signed.
fn keyed_refusal(response: &ControlResponse) -> Option<(&str, Option<&str>)> {
    match response {
        ControlResponse::InjectRefused { code, cause, .. } => {
            Some((code.as_str(), cause.as_deref()))
        }
        _ => None,
    }
}

/// A host of two mobs whose default mob has no identity runtime, with the
/// keyed inject plane a gateway's composition seals. Its second mob has no
/// identity runtime either until a row attaches one.
struct PlainHost {
    _temp: tempfile::TempDir,
    runtime: UnifiedRuntime,
    _sessions: Sessions,
    default_id: MobId,
    second_id: MobId,
}

impl PlainHost {
    async fn of_two_mobs() -> Self {
        let temp = tempfile::tempdir().expect("tempdir");
        let (runtime, sessions) = launch(&temp, definition("host")).await;
        let default_id = runtime.mob_handle().mob_id().clone();
        let second_id = second_mob(&runtime, definition("second")).await;
        Self {
            _temp: temp,
            runtime,
            _sessions: sessions,
            default_id,
            second_id,
        }
    }

    /// Attach an identity runtime to the second mob, with the agents `desk`
    /// and `back`.
    async fn identities_for_the_second_mob(&self) {
        let mob = self
            .runtime
            .resolve_mob(Some(&self.second_id))
            .await
            .expect("the second mob of the host resolves");
        let of_the_second = identity_context(mob.mob_runtime(), "second");
        mob.attach_identity_first_context(Arc::clone(&of_the_second))
            .await
            .expect("a host attaches an identity context to its second mob");
        Box::pin(
            of_the_second.bootstrap_roster(&[configured_agent("desk"), configured_agent("back")]),
        )
        .await
        .expect("the second mob's roster");
    }

    /// Seal the host's keyed inject plane as a gateway's composition does.
    fn compose(&self, plane: crate::mob_announcements::KeyedInjectPlane) {
        crate::mob_announcements::compose_keyed_inject_plane(&self.runtime, plane)
            .expect("the host's keyed inject plane is composed");
    }

    /// Bind the host's control listener with a grant for every verb, every
    /// member and every mob filed for one peer gateway.
    async fn listen(&self) -> Peer {
        let keys = GatewayPeerKeys::ephemeral();
        let authorizer = authorizer_of(
            &keys,
            self.default_id.as_str(),
            grant_over(Some(ControlMobScope::All)),
        );
        let advertised = self
            .runtime
            .start_control_listener_with_authorizer(&listen_address(), Arc::new(authorizer))
            .await
            .expect("the control listener binds");
        Peer {
            keys,
            endpoint: endpoint_of(&advertised),
        }
    }

    async fn shut_down(self) {
        shut_down(&self.runtime, &[&self.second_id]).await;
    }
}

/// The window the rows' planes serve under.
const WINDOW_MS: u64 = 60_000;

/// A second mob with durable identities is served keyed injects on a host
/// whose default mob has none: the status of a delivery signed for the
/// second mob is that mob's own answer, and the default mob answers
/// `not_served` with its cause as before.
#[tokio::test]
async fn a_second_mob_with_identities_is_served_keyed_injects_where_the_default_mob_has_none() {
    let host = PlainHost::of_two_mobs().await;
    host.identities_for_the_second_mob().await;
    host.compose(crate::mob_announcements::KeyedInjectPlane::Serve {
        window_ms: WINDOW_MS,
    });
    let peer = host.listen().await;

    let in_the_second = peer
        .ask(
            Some(host.second_id.as_str()),
            keyed_status_at("desk", "batch-1", clock_ms()),
        )
        .await;
    assert_eq!(
        status_of(&in_the_second),
        Some(&NativeStatus::NotCommitted),
        "the second mob's own continuation owner answers for its identity: {in_the_second:?}"
    );
    let in_the_default = peer
        .ask(
            Some(host.default_id.as_str()),
            keyed_status_at("desk", "batch-1", clock_ms()),
        )
        .await;
    assert_eq!(
        keyed_refusal(&in_the_default),
        Some(("not_served", Some("not_identity_first"))),
        "the default mob has no durable identities: {in_the_default:?}"
    );
    host.shut_down().await;
}

/// A default mob without durable identities answers both keyed verbs
/// `not_served` with the cause `not_identity_first`, as it did before the
/// host served a second mob; so does a second mob without them.
#[tokio::test]
async fn the_default_mob_without_identities_answers_not_served_as_before() {
    let host = PlainHost::of_two_mobs().await;
    host.compose(crate::mob_announcements::KeyedInjectPlane::Serve {
        window_ms: WINDOW_MS,
    });
    let peer = host.listen().await;
    for mob in [host.default_id.clone(), host.second_id.clone()] {
        let delivered = peer
            .ask(
                Some(mob.as_str()),
                keyed_at("desk", "batch-1", "a body", clock_ms()),
            )
            .await;
        assert_eq!(
            keyed_refusal(&delivered),
            Some(("not_served", Some("not_identity_first"))),
            "a delivery for {mob}: {delivered:?}"
        );
        let status = peer
            .ask(
                Some(mob.as_str()),
                keyed_status_at("desk", "batch-1", clock_ms()),
            )
            .await;
        assert_eq!(
            keyed_refusal(&status),
            Some(("not_served", Some("not_identity_first"))),
            "a status read for {mob}: {status:?}"
        );
    }
    host.shut_down().await;
}

/// A host whose configuration serves no keyed injects serves them for no
/// mob: a second mob with durable identities answers `not_served` with the
/// cause `runtime_disabled`, as the default mob does.
#[tokio::test]
async fn a_disabled_plane_serves_no_mob() {
    let host = PlainHost::of_two_mobs().await;
    host.identities_for_the_second_mob().await;
    host.compose(crate::mob_announcements::KeyedInjectPlane::Disabled);
    let peer = host.listen().await;
    for mob in [host.default_id.clone(), host.second_id.clone()] {
        let status = peer
            .ask(
                Some(mob.as_str()),
                keyed_status_at("desk", "batch-1", clock_ms()),
            )
            .await;
        assert_eq!(
            keyed_refusal(&status),
            Some(("not_served", Some("runtime_disabled"))),
            "a status read for {mob}: {status:?}"
        );
    }
    host.shut_down().await;
}

/// The plane is sealed once for the host, and what is a mob's is read when
/// a request arrives: a mob that gets its identity runtime after the seal
/// is served from then on.
#[tokio::test]
async fn an_identity_runtime_attached_after_the_seal_is_served() {
    let host = PlainHost::of_two_mobs().await;
    host.compose(crate::mob_announcements::KeyedInjectPlane::Serve {
        window_ms: WINDOW_MS,
    });
    let peer = host.listen().await;
    let before = peer
        .ask(
            Some(host.second_id.as_str()),
            keyed_status_at("desk", "batch-1", clock_ms()),
        )
        .await;
    assert_eq!(
        keyed_refusal(&before),
        Some(("not_served", Some("not_identity_first"))),
        "the second mob has no durable identities yet: {before:?}"
    );

    host.identities_for_the_second_mob().await;
    let after = peer
        .ask(
            Some(host.second_id.as_str()),
            keyed_status_at("desk", "batch-1", clock_ms()),
        )
        .await;
    assert_eq!(
        status_of(&after),
        Some(&NativeStatus::NotCommitted),
        "the mob's identity runtime is read when the request arrives: {after:?}"
    );
    host.shut_down().await;
}

/// A request outside the plane's window is refused as stale for every mob,
/// before the mob is asked for its identities: a mob without durable
/// identities answers a stale request `stale_request`, and a fresh one
/// `not_served`.
#[tokio::test]
async fn a_stale_request_is_refused_as_stale_before_the_mob_is_asked_for_identities() {
    let host = PlainHost::of_two_mobs().await;
    host.compose(crate::mob_announcements::KeyedInjectPlane::Serve {
        window_ms: WINDOW_MS,
    });
    let peer = host.listen().await;
    let long_ago = clock_ms().saturating_sub(60 * WINDOW_MS);
    for mob in [host.default_id.clone(), host.second_id.clone()] {
        let stale = peer
            .ask(
                Some(mob.as_str()),
                keyed_status_at("desk", "batch-1", long_ago),
            )
            .await;
        assert_eq!(
            keyed_refusal(&stale),
            Some(("stale_request", Some("outside_window"))),
            "a stale status read for {mob}: {stale:?}"
        );
        let fresh = peer
            .ask(
                Some(mob.as_str()),
                keyed_status_at("desk", "batch-1", clock_ms()),
            )
            .await;
        assert_eq!(
            keyed_refusal(&fresh),
            Some(("not_served", Some("not_identity_first"))),
            "a fresh status read for {mob}: {fresh:?}"
        );
        // The plane's own checks of a request come before the mob is asked:
        // a member that is no durable identity in its public form is
        // refused as that.
        let malformed = peer
            .ask(
                Some(mob.as_str()),
                keyed_status_at("", "batch-1", clock_ms()),
            )
            .await;
        assert_eq!(
            keyed_refusal(&malformed).map(|(code, _)| code),
            Some("invalid_request"),
            "a fresh status read for no member of {mob}: {malformed:?}"
        );
    }
    host.shut_down().await;
}

/// The label the host's facts name the host by, when `response` carries
/// the host's facts.
fn host_label_of(response: &ControlResponse) -> Option<&str> {
    match response {
        ControlResponse::Host { facts, .. } => Some(facts.host_label.as_str()),
        _ => None,
    }
}

/// The host's facts name the host by its default mob's id only to a caller
/// whose grant reaches that mob. A caller whose grant reaches another mob
/// alone reads the host's facts under the mob it signed for, and no answer
/// of a host verb names the default mob to it.
#[tokio::test]
async fn host_facts_name_the_default_mob_only_to_a_grant_that_reaches_it() {
    let mut host = Host::of_two_mobs().await;
    host.runtime
        .set_gateway_peer_keys(GatewayPeerKeys::ephemeral());
    let (default_mob, second_mob) = (host.default_id.clone(), host.second_id.clone());
    let wide = GatewayPeerKeys::ephemeral();
    let narrow = GatewayPeerKeys::ephemeral();
    let grant = |label: &str, mobs: ControlMobScope| {
        ControlGrant::new(
            label,
            ControlVerb::all().iter().copied(),
            ControlMemberScope::All,
        )
        .with_mobs(mobs)
    };
    let mut table = ControlGrantTable::new();
    table.insert(
        wide.pubkey_bytes(),
        grant("wide-gateway", ControlMobScope::All),
    );
    table.insert(
        narrow.pubkey_bytes(),
        grant(
            "narrow-gateway",
            ControlMobScope::mobs([second_mob.as_str()]),
        ),
    );
    let peer = host
        .listen_with(
            wide,
            ControlAuthorizer::with_grants_for_audience(table, default_mob.as_str()),
        )
        .await;

    let to_the_wide = peer.ask(Some(second_mob.as_str()), describe()).await;
    assert_eq!(
        host_label_of(&to_the_wide),
        Some(default_mob.as_str()),
        "a grant that reaches the default mob reads the host by it: {to_the_wide:?}"
    );

    for request in [describe(), health()] {
        let verb = request.verb();
        let to_the_narrow = peer
            .ask_as(&narrow, Some(second_mob.as_str()), request)
            .await;
        assert!(
            !refused_by_grant(&to_the_narrow),
            "{verb}: a mob in reach is an audience of the host: {to_the_narrow:?}"
        );
        assert!(
            !format!("{to_the_narrow:?}").contains(default_mob.as_str()),
            "{verb}: the answer names the default mob to a grant that does not reach it: \
             {to_the_narrow:?}"
        );
    }
    let to_the_narrow = peer
        .ask_as(&narrow, Some(second_mob.as_str()), describe())
        .await;
    assert_eq!(
        host_label_of(&to_the_narrow),
        Some(second_mob.as_str()),
        "the host goes by the mob the request was signed for: {to_the_narrow:?}"
    );
    host.shut_down().await;
}

/// The labels a build records for a member it keeps apart.
fn kept_apart_labels() -> std::collections::BTreeMap<String, String> {
    std::collections::BTreeMap::from([(
        crate::access::PRIVATE_TO_LABEL.to_string(),
        "someone".to_string(),
    )])
}

/// Have the record of the mob `mob_id` of `runtime` say that its member
/// `member` is kept apart, as a build of that member would. The member's
/// roster entry and registered row are left as they are: only the mob's
/// own record says so.
async fn record_kept_apart(runtime: &UnifiedRuntime, mob_id: &MobId, member: &str) {
    let mob = runtime
        .resolve_mob(Some(mob_id))
        .await
        .expect("the mob of the host resolves");
    let record = mob.mob_runtime().private_owners();
    record.first_attempts_settled(Duration::from_secs(30)).await;
    assert!(
        !record.first_read_pending(),
        "the mob's first read of its members has succeeded"
    );
    record.record(
        mob_id.as_str(),
        crate::member_comms_id::mob_member_id(member).as_str(),
        Some(&kept_apart_labels()),
    );
}

/// What an answer says once a name is taken out of it, as it is spelled
/// and as the roster keys its member.
fn without_the_name(answer: &ControlResponse, name: &str) -> String {
    format!("{answer:?}").replace(name, "<name>").replace(
        crate::member_comms_id::mob_member_id(name).as_str(),
        "<name>",
    )
}

/// A member of a second mob that the second mob's own record keeps apart
/// is answered, by a member verb signed for that mob, exactly as a name no
/// member of that mob has: the handler built for the request asks the
/// record of the mob it serves. A member the record does not keep apart is
/// found as before.
#[tokio::test]
async fn a_second_mobs_member_its_record_keeps_apart_answers_as_a_name_no_member_has() {
    let host = Host::of_two_mobs().await;
    let second_mob = host.second_id.clone();
    record_kept_apart(&host.runtime, &second_mob, "back").await;
    let peer = host.listen(grant_over(Some(ControlMobScope::All))).await;

    let for_the_member = peer.ask(Some(second_mob.as_str()), lookup("back")).await;
    let for_no_member = peer.ask(Some(second_mob.as_str()), lookup("nobody")).await;
    assert!(
        matches!(for_no_member, ControlResponse::Err { .. }) && !refused_by_grant(&for_no_member),
        "a name no member of the second mob has is refused by the handler: {for_no_member:?}"
    );
    assert_eq!(
        without_the_name(&for_the_member, "back"),
        without_the_name(&for_no_member, "nobody"),
        "the member the second mob's record keeps apart is answered as a name no member has"
    );
    let for_the_other_member = peer.ask(Some(second_mob.as_str()), lookup("desk")).await;
    assert!(
        matches!(for_the_other_member, ControlResponse::Member { .. }),
        "a member the record does not keep apart is found: {for_the_other_member:?}"
    );
    host.shut_down().await;
}

/// An identity of a second mob that the second mob's own record keeps
/// apart is answered, by a keyed delivery signed for that mob through the
/// host's own plane, exactly as a name no identity of that mob has: the
/// delivery seam built for the request asks the record of the mob it
/// serves, and nothing is committed.
#[tokio::test]
async fn a_second_mobs_identity_its_record_keeps_apart_answers_a_keyed_delivery_as_no_identity() {
    let host = PlainHost::of_two_mobs().await;
    host.identities_for_the_second_mob().await;
    let second_mob = host.second_id.clone();
    record_kept_apart(&host.runtime, &second_mob, "back").await;
    host.compose(crate::mob_announcements::KeyedInjectPlane::Serve {
        window_ms: WINDOW_MS,
    });
    let peer = host.listen().await;

    let to_the_identity = peer
        .ask(
            Some(second_mob.as_str()),
            keyed_at("back", "batch-1", "a body", clock_ms()),
        )
        .await;
    let to_no_identity = peer
        .ask(
            Some(second_mob.as_str()),
            keyed_at("nobody", "batch-1", "a body", clock_ms()),
        )
        .await;
    assert_eq!(
        keyed_refusal(&to_no_identity).map(|(code, _)| code),
        Some("unknown_member"),
        "a delivery to a name no identity of the second mob has is refused: {to_no_identity:?}"
    );
    assert_eq!(
        without_the_name(&to_the_identity, "back"),
        without_the_name(&to_no_identity, "nobody"),
        "the identity the second mob's record keeps apart is answered as a name no identity has"
    );
    let of_the_identity = peer
        .ask(
            Some(second_mob.as_str()),
            keyed_status_at("back", "batch-1", clock_ms()),
        )
        .await;
    assert_eq!(
        status_of(&of_the_identity),
        Some(&NativeStatus::NotCommitted),
        "nothing was committed for the identity kept apart: {of_the_identity:?}"
    );
    host.shut_down().await;
}

/// The record of the mob `mob_id` of `runtime`, once its first read has
/// succeeded.
async fn settled_record_of(
    runtime: &UnifiedRuntime,
    mob_id: &MobId,
) -> crate::private_owners::PrivateOwners {
    let record = {
        let mob = runtime
            .resolve_mob(Some(mob_id))
            .await
            .expect("the mob of the host resolves");
        mob.mob_runtime().private_owners().clone()
    };
    record.first_attempts_settled(Duration::from_secs(30)).await;
    assert!(
        !record.first_read_pending(),
        "the mob's first read of its members has succeeded"
    );
    record
}

/// A request for a mob whose record's first read is in flight (a mob the
/// request itself woke owes that read from the moment it is loaded) waits
/// for the read, at most the bound, and is answered from what the read
/// found: a member of that mob is found, not answered as a name no member
/// has.
#[tokio::test]
async fn a_request_waits_for_the_first_read_of_the_mob_it_is_for() {
    let host = Host::of_two_mobs().await;
    let second_mob = host.second_id.clone();
    let record = settled_record_of(&host.runtime, &second_mob).await;
    let peer = host.listen(grant_over(Some(ControlMobScope::All))).await;

    let owed = record.owe_a_first_read_in_tests();
    let read_succeeds = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        drop(owed);
    });
    let found = peer.ask(Some(second_mob.as_str()), lookup("desk")).await;
    read_succeeds.await.expect("the read succeeds");
    assert!(
        matches!(found, ControlResponse::Member { .. }),
        "the request waited for the mob's first read and finds the member: {found:?}"
    );
    host.shut_down().await;
}

/// A first read that stays owed past the bound is waited for no longer:
/// the request is then answered as for a name no member has, as the launch
/// handler answers the default mob's members while its read is owed.
#[tokio::test]
async fn a_first_read_that_stays_owed_answers_a_second_mobs_member_as_no_member() {
    let host = Host::of_two_mobs().await;
    let second_mob = host.second_id.clone();
    let record = settled_record_of(&host.runtime, &second_mob).await;
    let peer = host.listen(grant_over(Some(ControlMobScope::All))).await;

    let _owed = record.owe_a_first_read_in_tests();
    let for_the_member = peer.ask(Some(second_mob.as_str()), lookup("desk")).await;
    let for_no_member = peer.ask(Some(second_mob.as_str()), lookup("nobody")).await;
    assert!(
        matches!(for_no_member, ControlResponse::Err { .. }) && !refused_by_grant(&for_no_member),
        "a name no member of the second mob has is refused by the handler: {for_no_member:?}"
    );
    assert_eq!(
        without_the_name(&for_the_member, "desk"),
        without_the_name(&for_no_member, "nobody"),
        "while the mob's read stays owed a member is answered as a name no member has"
    );
    host.shut_down().await;
}

/// A keyed request for a mob whose record's first read is in flight waits
/// for the read, at most the bound: an identity that the read finds to be
/// kept apart is then answered as a name no identity has, and nothing is
/// committed for it. The keyed verbs refuse nothing for an owed read: with
/// the read still owed after the bound they answer by what the record and
/// the registered row say then.
#[tokio::test]
async fn a_keyed_request_waits_for_the_first_read_of_the_mob_it_is_for() {
    let host = PlainHost::of_two_mobs().await;
    host.identities_for_the_second_mob().await;
    let second_mob = host.second_id.clone();
    let record = settled_record_of(&host.runtime, &second_mob).await;
    host.compose(crate::mob_announcements::KeyedInjectPlane::Serve {
        window_ms: WINDOW_MS,
    });
    let peer = host.listen().await;

    // The mob's first read is in flight, and what it reads says the
    // identity is kept apart: the record knows it once the read succeeded.
    let owed = record.owe_a_first_read_in_tests();
    let read_succeeds = {
        let (record, mob_id) = (record.clone(), second_mob.clone());
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            record.record(
                mob_id.as_str(),
                crate::member_comms_id::mob_member_id("back").as_str(),
                Some(&kept_apart_labels()),
            );
            drop(owed);
        })
    };
    let to_the_identity = peer
        .ask(
            Some(second_mob.as_str()),
            keyed_at("back", "batch-1", "a body", clock_ms()),
        )
        .await;
    read_succeeds.await.expect("the read succeeds");
    let to_no_identity = peer
        .ask(
            Some(second_mob.as_str()),
            keyed_at("nobody", "batch-1", "a body", clock_ms()),
        )
        .await;
    assert_eq!(
        keyed_refusal(&to_no_identity).map(|(code, _)| code),
        Some("unknown_member"),
        "a delivery to a name no identity of the second mob has is refused: {to_no_identity:?}"
    );
    assert_eq!(
        without_the_name(&to_the_identity, "back"),
        without_the_name(&to_no_identity, "nobody"),
        "the request waited for the read and answers the identity it keeps apart as no identity"
    );
    host.shut_down().await;
}

/// An identity of the default mob that the default mob's own record keeps
/// apart is answered, by a keyed delivery through the host's own plane,
/// exactly as a name no identity has: the delivery seam built for a
/// request to the default mob asks that mob's record too.
#[tokio::test]
async fn the_default_mobs_identity_its_record_keeps_apart_answers_a_keyed_delivery_as_no_identity()
{
    let host = Host::of_two_mobs().await;
    let default_mob = host.default_id.clone();
    record_kept_apart(&host.runtime, &default_mob, "front").await;
    crate::mob_announcements::compose_keyed_inject_plane(
        &host.runtime,
        crate::mob_announcements::KeyedInjectPlane::Serve {
            window_ms: WINDOW_MS,
        },
    )
    .expect("the host's keyed inject plane is composed");
    let peer = host.listen(grant_over(Some(ControlMobScope::All))).await;

    let to_the_identity = peer
        .ask(
            Some(default_mob.as_str()),
            keyed_at("front", "batch-1", "a body", clock_ms()),
        )
        .await;
    let to_no_identity = peer
        .ask(
            Some(default_mob.as_str()),
            keyed_at("nobody", "batch-1", "a body", clock_ms()),
        )
        .await;
    assert_eq!(
        keyed_refusal(&to_no_identity).map(|(code, _)| code),
        Some("unknown_member"),
        "a delivery to a name no identity of the default mob has is refused: {to_no_identity:?}"
    );
    assert_eq!(
        without_the_name(&to_the_identity, "front"),
        without_the_name(&to_no_identity, "nobody"),
        "the identity the default mob's record keeps apart is answered as a name no identity has"
    );
    host.shut_down().await;
}

/// How far inside the time a default caller waits for an answer
/// ([`crate::runtime::cross_mob_control::DEFAULT_CONTROL_TIMEOUT`]) the
/// listener answers a request whose mob's first read stays owed.
const INSIDE_A_DEFAULT_CALLERS_WAIT: Duration = Duration::from_secs(1);

/// A request for a member of a mob whose first read stays owed is answered
/// well inside the time a default caller waits for an answer: the wait for
/// the read has a bound of the control channel's own, below that time, so
/// the answer (the member as a name no member has) reaches such a caller.
#[tokio::test]
async fn a_member_verb_whose_mobs_read_stays_owed_is_answered_inside_a_default_callers_wait() {
    let host = Host::of_two_mobs().await;
    let second_mob = host.second_id.clone();
    let record = settled_record_of(&host.runtime, &second_mob).await;
    let peer = host.listen(grant_over(Some(ControlMobScope::All))).await;

    let _owed = record.owe_a_first_read_in_tests();
    let asked = std::time::Instant::now();
    let for_the_member = peer.ask(Some(second_mob.as_str()), lookup("desk")).await;
    let answered_after = asked.elapsed();
    assert!(
        matches!(for_the_member, ControlResponse::Err { .. }) && !refused_by_grant(&for_the_member),
        "while the mob's read stays owed the handler answers the member as a name no member \
         has: {for_the_member:?}"
    );
    assert!(
        answered_after + INSIDE_A_DEFAULT_CALLERS_WAIT
            < crate::runtime::cross_mob_control::DEFAULT_CONTROL_TIMEOUT,
        "the answer comes well inside a default caller's wait: it came after {answered_after:?}"
    );
    host.shut_down().await;
}

/// The same for a keyed verb. The keyed verbs refuse nothing for a read
/// that is still owed, so the status of a delivery nobody made reads as no
/// commit, and it is read well inside a default caller's wait.
#[tokio::test]
async fn a_keyed_verb_whose_mobs_read_stays_owed_is_answered_inside_a_default_callers_wait() {
    let host = PlainHost::of_two_mobs().await;
    host.identities_for_the_second_mob().await;
    let second_mob = host.second_id.clone();
    let record = settled_record_of(&host.runtime, &second_mob).await;
    host.compose(crate::mob_announcements::KeyedInjectPlane::Serve {
        window_ms: WINDOW_MS,
    });
    let peer = host.listen().await;

    let _owed = record.owe_a_first_read_in_tests();
    let asked = std::time::Instant::now();
    let of_the_identity = peer
        .ask(
            Some(second_mob.as_str()),
            keyed_status_at("desk", "batch-1", clock_ms()),
        )
        .await;
    let answered_after = asked.elapsed();
    assert_eq!(
        status_of(&of_the_identity),
        Some(&NativeStatus::NotCommitted),
        "the keyed verbs refuse nothing for a read that is still owed: {of_the_identity:?}"
    );
    assert!(
        answered_after + INSIDE_A_DEFAULT_CALLERS_WAIT
            < crate::runtime::cross_mob_control::DEFAULT_CONTROL_TIMEOUT,
        "the answer comes well inside a default caller's wait: it came after {answered_after:?}"
    );
    host.shut_down().await;
}

/// The handler built for a keyed verb is built without a wait for the mob's
/// first read: a keyed request waits for that read once, in the delivery
/// seam built for it, and not a second time before it. (The handler of a
/// verb that names a member is the one that waits: the rows above.)
#[tokio::test]
async fn the_handler_of_a_keyed_verb_is_built_without_a_wait_for_the_mobs_first_read() {
    use crate::runtime::cross_mob_control::ControlHostedMobRouter;

    let host = PlainHost::of_two_mobs().await;
    host.identities_for_the_second_mob().await;
    let second_mob = host.second_id.clone();
    let record = settled_record_of(&host.runtime, &second_mob).await;
    host.compose(crate::mob_announcements::KeyedInjectPlane::Serve {
        window_ms: WINDOW_MS,
    });
    // What the runtime's own listener serves the mobs beside its default
    // mob with, over the resolver that listener takes.
    let other_mobs = super::HostedControlMobs {
        mobs: host.runtime.mob_resolver_of_its_own_task(),
        keyed_inject: Arc::clone(&host.runtime.keyed_inject_slot),
        candidate_launch: false,
    };

    let _owed = record.owe_a_first_read_in_tests();
    for verb in [ControlVerb::InjectKeyed, ControlVerb::InjectStatus] {
        let built = tokio::time::timeout(
            Duration::from_secs(1),
            other_mobs.handler_for(&second_mob, verb),
        )
        .await;
        assert!(
            matches!(built, Ok(Some(_))),
            "the handler of `{verb}` is built at once while the mob's read is owed"
        );
    }
    drop(other_mobs);
    host.shut_down().await;
}

/// A keyed request for the default mob waits for that mob's first read in
/// the delivery seam built for it, the only wait on its way (the launch
/// handler waits for nothing): an identity that the read finds to be kept
/// apart is answered as a name no identity has.
#[tokio::test]
async fn a_keyed_request_for_the_default_mob_waits_for_its_records_first_read() {
    let host = Host::of_two_mobs().await;
    let default_mob = host.default_id.clone();
    let record = settled_record_of(&host.runtime, &default_mob).await;
    crate::mob_announcements::compose_keyed_inject_plane(
        &host.runtime,
        crate::mob_announcements::KeyedInjectPlane::Serve {
            window_ms: WINDOW_MS,
        },
    )
    .expect("the host's keyed inject plane is composed");
    let peer = host.listen(grant_over(Some(ControlMobScope::All))).await;

    // The default mob's first read is in flight, and what it reads says the
    // identity is kept apart: the record knows it once the read succeeded.
    let owed = record.owe_a_first_read_in_tests();
    let read_succeeds = {
        let (record, mob_id) = (record.clone(), default_mob.clone());
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            record.record(
                mob_id.as_str(),
                crate::member_comms_id::mob_member_id("front").as_str(),
                Some(&kept_apart_labels()),
            );
            drop(owed);
        })
    };
    let to_the_identity = peer
        .ask(
            Some(default_mob.as_str()),
            keyed_at("front", "batch-1", "a body", clock_ms()),
        )
        .await;
    read_succeeds.await.expect("the read succeeds");
    let to_no_identity = peer
        .ask(
            Some(default_mob.as_str()),
            keyed_at("nobody", "batch-1", "a body", clock_ms()),
        )
        .await;
    assert_eq!(
        keyed_refusal(&to_no_identity).map(|(code, _)| code),
        Some("unknown_member"),
        "a delivery to a name no identity of the default mob has is refused: {to_no_identity:?}"
    );
    assert_eq!(
        without_the_name(&to_the_identity, "front"),
        without_the_name(&to_no_identity, "nobody"),
        "the request waited for the read and answers the identity it keeps apart as no identity"
    );
    host.shut_down().await;
}

/// The record of the mob `mob_id` of `runtime` as it stands after a load in
/// a process that did not build the mob's members: it holds none of them,
/// and the read of the mob is owed until the value handed back is dropped.
async fn record_that_owes_the_read_of(
    runtime: &UnifiedRuntime,
    mob_id: &MobId,
) -> crate::private_owners::OwedMobReadInTests {
    let record = settled_record_of(runtime, mob_id).await;
    // What the set built for the fixture is read first, so that no read of
    // the fixture's own lands in the middle of the row.
    record
        .read_of_the_mob_awaited(mob_id.as_str(), Duration::from_secs(30))
        .await;
    record.forget_mob_in_tests(mob_id.as_str());
    record.owe_the_read_of_in_tests(mob_id.as_str())
}

/// A request for a member of a mob whose own read is owed (the host said it
/// is about to build the mob, and a request can be the one that made it
/// load the mob) waits for that mob's read, at most the bound, and is
/// answered from what the read found. The record does not hold the mob's
/// members: until the mob is read they are not known, and once it is read
/// the member is found.
#[tokio::test]
async fn a_request_waits_for_the_read_of_the_mob_it_is_for() {
    let host = Host::of_two_mobs().await;
    let second_mob = host.second_id.clone();
    let peer = host.listen(grant_over(Some(ControlMobScope::All))).await;

    let owed = record_that_owes_the_read_of(&host.runtime, &second_mob).await;
    let read_lands = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        drop(owed);
    });
    let found = peer.ask(Some(second_mob.as_str()), lookup("desk")).await;
    read_lands.await.expect("the read lands");
    assert!(
        matches!(found, ControlResponse::Member { .. }),
        "the request waited for the read of its mob and finds the member: {found:?}"
    );
    host.shut_down().await;
}

/// The same for a keyed delivery, through the host's own plane: the
/// delivery to an identity of a mob whose own read is owed waits for that
/// mob's read and is then made, where a delivery that did not wait is
/// answered as a read that cannot be made now.
#[tokio::test]
async fn a_keyed_request_waits_for_the_read_of_the_mob_it_is_for() {
    let host = PlainHost::of_two_mobs().await;
    host.identities_for_the_second_mob().await;
    let second_mob = host.second_id.clone();
    host.compose(crate::mob_announcements::KeyedInjectPlane::Serve {
        window_ms: WINDOW_MS,
    });
    let peer = host.listen().await;

    let owed = record_that_owes_the_read_of(&host.runtime, &second_mob).await;
    let read_lands = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        drop(owed);
    });
    let to_the_identity = peer
        .ask(
            Some(second_mob.as_str()),
            keyed_at("desk", "batch-1", "a body", clock_ms()),
        )
        .await;
    read_lands.await.expect("the read lands");
    assert_eq!(
        keyed_refusal(&to_the_identity),
        None,
        "the delivery waited for the read of its mob and is not refused: {to_the_identity:?}"
    );
    assert!(
        receipt_of(&to_the_identity).is_some(),
        "the delivery is made into the second mob: {to_the_identity:?}"
    );
    host.shut_down().await;
}

/// A member that the read of its mob finds to be kept apart is answered,
/// after the wait, as a name no member has, and a member the read names no
/// person for is found by the same peer: the request is answered from what
/// the read found. (The answer for the member kept apart is the closed
/// side's before the read too: the row pins that the read does not open
/// it. What the read finds is recorded here just before the mark is
/// cleared.)
#[tokio::test]
async fn a_member_the_read_of_its_mob_finds_kept_apart_is_a_name_no_member_has_after_the_wait() {
    let host = Host::of_two_mobs().await;
    let second_mob = host.second_id.clone();
    let record = settled_record_of(&host.runtime, &second_mob).await;
    let peer = host.listen(grant_over(Some(ControlMobScope::All))).await;

    let owed = record_that_owes_the_read_of(&host.runtime, &second_mob).await;
    let read_lands = {
        let mob_id = second_mob.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            record.record(
                mob_id.as_str(),
                crate::member_comms_id::mob_member_id("back").as_str(),
                Some(&kept_apart_labels()),
            );
            drop(owed);
        })
    };
    let for_the_member = peer.ask(Some(second_mob.as_str()), lookup("back")).await;
    read_lands.await.expect("the read lands");
    let for_no_member = peer.ask(Some(second_mob.as_str()), lookup("nobody")).await;
    assert!(
        matches!(for_no_member, ControlResponse::Err { .. }) && !refused_by_grant(&for_no_member),
        "a name no member of the second mob has is refused by the handler: {for_no_member:?}"
    );
    assert_eq!(
        without_the_name(&for_the_member, "back"),
        without_the_name(&for_no_member, "nobody"),
        "the member the read finds kept apart is answered as a name no member has"
    );
    let for_the_other_member = peer.ask(Some(second_mob.as_str()), lookup("desk")).await;
    assert!(
        matches!(for_the_other_member, ControlResponse::Member { .. }),
        "a member the read names no person for is found: {for_the_other_member:?}"
    );
    host.shut_down().await;
}

/// A read of the request's mob that stays owed past the bound is waited for
/// no longer: the member is then answered as a name no member has, after
/// the whole bound and well inside the time a default caller waits for an
/// answer.
#[tokio::test]
async fn a_member_verb_whose_mobs_own_read_stays_owed_is_answered_as_no_member_at_the_bound() {
    use crate::runtime::cross_mob_control::{DEFAULT_CONTROL_TIMEOUT, MEMBERS_APART_READ_WAIT};

    let host = Host::of_two_mobs().await;
    let second_mob = host.second_id.clone();
    let peer = host.listen(grant_over(Some(ControlMobScope::All))).await;

    let _owed = record_that_owes_the_read_of(&host.runtime, &second_mob).await;
    let asked = std::time::Instant::now();
    let for_the_member = peer.ask(Some(second_mob.as_str()), lookup("desk")).await;
    let answered_after = asked.elapsed();
    let for_no_member = peer.ask(Some(second_mob.as_str()), lookup("nobody")).await;
    assert!(
        matches!(for_no_member, ControlResponse::Err { .. }) && !refused_by_grant(&for_no_member),
        "a name no member of the second mob has is refused by the handler: {for_no_member:?}"
    );
    assert_eq!(
        without_the_name(&for_the_member, "desk"),
        without_the_name(&for_no_member, "nobody"),
        "while the read of its mob stays owed a member is answered as a name no member has"
    );
    assert!(
        answered_after + Duration::from_millis(100) >= MEMBERS_APART_READ_WAIT,
        "the request waited its bound for the read: it was answered after {answered_after:?}"
    );
    assert!(
        answered_after + INSIDE_A_DEFAULT_CALLERS_WAIT < DEFAULT_CONTROL_TIMEOUT,
        "the answer comes well inside a default caller's wait: it came after {answered_after:?}"
    );
    host.shut_down().await;
}

/// The same for a keyed delivery, through the host's own plane: while the
/// read of its mob stays owed past the bound, a delivery to an identity
/// that exists and one to a name no identity has are both answered as a
/// read that cannot be made now, well inside a default caller's wait, and
/// nothing is committed: once the mob is read the delivery's status reads
/// as no commit.
#[tokio::test]
async fn a_keyed_delivery_whose_mobs_own_read_stays_owed_is_answered_as_a_read_that_cannot_be_made()
{
    use crate::runtime::cross_mob_control::{DEFAULT_CONTROL_TIMEOUT, MEMBERS_APART_READ_WAIT};

    let host = PlainHost::of_two_mobs().await;
    host.identities_for_the_second_mob().await;
    let second_mob = host.second_id.clone();
    host.compose(crate::mob_announcements::KeyedInjectPlane::Serve {
        window_ms: WINDOW_MS,
    });
    let peer = host.listen().await;

    let owed = record_that_owes_the_read_of(&host.runtime, &second_mob).await;
    let asked = std::time::Instant::now();
    let to_the_identity = peer
        .ask(
            Some(second_mob.as_str()),
            keyed_at("desk", "batch-1", "a body", clock_ms()),
        )
        .await;
    let answered_after = asked.elapsed();
    let to_no_identity = peer
        .ask(
            Some(second_mob.as_str()),
            keyed_at("nobody", "batch-1", "a body", clock_ms()),
        )
        .await;
    assert_eq!(
        keyed_refusal(&to_the_identity).map(|(code, _)| code),
        Some("native_unavailable"),
        "while the read of its mob stays owed a delivery is answered as a read that cannot be \
         made now: {to_the_identity:?}"
    );
    assert_eq!(
        without_the_name(&to_the_identity, "desk"),
        without_the_name(&to_no_identity, "nobody"),
        "an identity that exists and a name no identity has are answered alike"
    );
    assert!(
        answered_after + Duration::from_millis(100) >= MEMBERS_APART_READ_WAIT,
        "the delivery waited its bound for the read: it was answered after {answered_after:?}"
    );
    assert!(
        answered_after + INSIDE_A_DEFAULT_CALLERS_WAIT < DEFAULT_CONTROL_TIMEOUT,
        "the answer comes well inside a default caller's wait: it came after {answered_after:?}"
    );

    drop(owed);
    let of_the_identity = peer
        .ask(
            Some(second_mob.as_str()),
            keyed_status_at("desk", "batch-1", clock_ms()),
        )
        .await;
    assert_eq!(
        status_of(&of_the_identity),
        Some(&NativeStatus::NotCommitted),
        "nothing was committed for the delivery that was not made: {of_the_identity:?}"
    );
    host.shut_down().await;
}
