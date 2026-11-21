//! Rows of the keyed inject plane, and of the control channel's member
//! verbs, for a durable identity that is kept apart from other callers.
//!
//! A peer gateway's keyed delivery names a durable identity this gateway
//! hosts. An identity that is kept apart is answered exactly as a name no
//! identity has: its delivery is refused as for an unknown member, and its
//! status reads as no commit. Nothing reaches its continuation owner.
//!
//! The member verbs answer such an identity the same way, by its name and
//! by a generated alias of it, on a runtime that has an identity runtime.

#![allow(clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;

use super::remote::KeyedInjectServer;
use super::types::content_digest;
use crate::auth::peer_keys::GatewayPeerKeys;
use crate::runtime::cross_mob_control::{
    ControlAuthorizer, ControlAuthzDenial, ControlGrant, ControlGrantTable, ControlListenAddr,
    ControlMemberScope, ControlRequest, ControlResponse, ControlVerb, RemoteControlClient,
    sign_control_request_as_caller,
};
use crate::runtime::cross_mob_remote::RemoteEndpoint;
use crate::runtime::keyed_inject::{
    KeyedInject, KeyedInjectCallerScope, KeyedInjectReceiver, NativeStatus,
};

/// An identity every caller may reach.
const OPEN: &str = "agent:lobby";
/// An identity that is kept apart.
const APART: &str = "agent:vault";
/// A name no identity has.
const NOBODY: &str = "agent:nobody";
/// An identity whose registered row does not say it is kept apart: a row
/// has the runtime's record of its members say so.
const RECORDED: &str = "agent:shelf";

const NOW_MS: u64 = 1_800_000_000_000;
const WINDOW_MS: u64 = 300_000;

/// A host's roster of two identities: [`OPEN`], and [`APART`] declared with
/// the label that keeps an identity apart.
struct TwoIdentities;

#[async_trait]
impl crate::identity_first::contracts::RosterProvider for TwoIdentities {
    async fn roster(
        &self,
        _context: &crate::identity_first::RosterContext,
    ) -> Result<Vec<crate::identity_first::DurableAgentSpec>, crate::identity_first::RosterError>
    {
        let agent = |name: &str, labels: BTreeMap<String, String>| {
            crate::identity_first::DurableAgentSpec {
                identity: crate::identity_first::AgentIdentity::parse(name)
                    .expect("parse identity"),
                profile: meerkat_mob::ProfileName::from("personal"),
                addressability: crate::identity_first::AgentAddressability::Addressable,
                display_name: None,
                labels,
                context: None,
                additional_instructions: Vec::new(),
                initial_message: None,
                runtime_mode_override: None,
                backend: None,
                binding: None,
                placement: None,
            }
        };
        Ok(vec![
            agent(OPEN, BTreeMap::new()),
            agent(RECORDED, BTreeMap::new()),
            agent(
                APART,
                BTreeMap::from([(
                    crate::access::PRIVATE_TO_LABEL.to_string(),
                    "someone".to_string(),
                )]),
            ),
        ])
    }
}

/// The keyed inject server of an identity-first gateway whose roster is
/// [`TwoIdentities`], over the gateway's own delivery seam, with the
/// runtime that owns it.
async fn server(
    state: &std::path::Path,
    mob_id: &str,
) -> (crate::UnifiedRuntime, KeyedInjectServer) {
    let runtime = Box::pin(
        super::tests::identity_first_test_builder(state, mob_id, "")
            .await
            .roster_provider(Arc::new(TwoIdentities))
            .build(),
    )
    .await
    .expect("build the identity-first runtime");
    let local = Arc::new(super::local_delivery(&runtime).expect("the gateway's native seam"));
    let server = KeyedInjectServer::with_clock(local, WINDOW_MS, Arc::new(|| NOW_MS));
    (runtime, server)
}

fn delivery<'a>(member: &'a str, digest: &'a str) -> KeyedInject<'a> {
    KeyedInject {
        member,
        delivery_key: "batch-1",
        content_digest: digest,
        summary: "1 announcement",
        body: "a body",
    }
}

/// The compared pair for the two keyed verbs: a delivery to the identity
/// kept apart is refused exactly as a delivery to a name no identity has,
/// and its status reads exactly as that name's. An identity every caller
/// may reach is not refused as unknown.
#[tokio::test(flavor = "multi_thread")]
async fn an_identity_kept_apart_answers_the_keyed_verbs_as_a_name_no_identity_has() {
    let state = tempfile::tempdir().expect("state dir");
    let (runtime, server) = server(state.path(), "mob-k-apart").await;
    let scope = KeyedInjectCallerScope::for_grant_label("peer-gateway");
    let digest = content_digest("a body");
    let deadline = NOW_MS + WINDOW_MS;

    let to_the_identity = server
        .deliver(&scope, delivery(APART, &digest), deadline)
        .await;
    let to_no_identity = server
        .deliver(&scope, delivery(NOBODY, &digest), deadline)
        .await;
    let refusal_of_no_identity = format!("{:?}", to_no_identity.as_ref().err());
    assert!(
        to_no_identity.is_err(),
        "a delivery to a name no identity has is refused: {to_no_identity:?}"
    );
    assert_eq!(
        format!("{:?}", to_the_identity.as_ref().err()).replace(APART, "<identity>"),
        refusal_of_no_identity.replace(NOBODY, "<identity>"),
        "a delivery to the identity kept apart is refused as one to a name no identity has"
    );

    let of_the_identity = server
        .status(&scope, APART, "batch-1", true)
        .await
        .map(|answer| (answer.status, answer.input));
    let of_no_identity = server
        .status(&scope, NOBODY, "batch-1", true)
        .await
        .map(|answer| (answer.status, answer.input));
    assert_eq!(
        format!("{of_the_identity:?}").replace(APART, "<identity>"),
        format!("{of_no_identity:?}").replace(NOBODY, "<identity>"),
        "the status of the identity kept apart reads as that of a name no identity has"
    );
    assert!(
        matches!(of_no_identity, Ok((NativeStatus::NotCommitted, None))),
        "a name no identity has holds no commit: {of_no_identity:?}"
    );

    // An identity every caller may reach is not answered as unknown.
    let to_the_open_identity = server
        .deliver(&scope, delivery(OPEN, &digest), deadline)
        .await;
    assert_ne!(
        format!("{:?}", to_the_open_identity.as_ref().err()).replace(OPEN, "<identity>"),
        refusal_of_no_identity.replace(NOBODY, "<identity>"),
        "an identity every caller may reach is not refused as unknown: {to_the_open_identity:?}"
    );
    runtime.shutdown().await;
}

// The member verbs of the control channel, for the same identities: the
// runtime has an identity runtime, and each identity is registered and has
// no member until something reaches it.

/// A gateway on the far side of the control channel: its key, where it
/// dials the host, and the mob it signs its requests for.
struct ControlPeer {
    keys: GatewayPeerKeys,
    endpoint: RemoteEndpoint,
    audience: String,
}

impl ControlPeer {
    async fn ask(&self, mut request: ControlRequest) -> ControlResponse {
        sign_control_request_as_caller(&self.keys, Some(&self.audience), &mut request);
        RemoteControlClient::send(&self.endpoint, &request, std::time::Duration::from_secs(30))
            .await
            .expect("the control exchange")
    }
}

/// The identity-first runtime of [`server`], with its control listener
/// bound for one peer gateway whose grant names every verb and reaches
/// every member.
async fn listening(state: &std::path::Path, mob_id: &str) -> (crate::UnifiedRuntime, ControlPeer) {
    let (runtime, _server) = server(state, mob_id).await;
    // The runtime reads who its members are kept apart from in the
    // background from its start, and answers every name as no member
    // until that read has succeeded: the rows begin once it has.
    let record = runtime.mob_runtime().private_owners();
    record
        .first_attempts_settled(std::time::Duration::from_secs(30))
        .await;
    assert!(
        !record.first_read_pending(),
        "the runtime's first read of its members has succeeded"
    );
    let keys = GatewayPeerKeys::ephemeral();
    let mut table = ControlGrantTable::new();
    table.insert(
        keys.pubkey_bytes(),
        ControlGrant::new(
            "peer-gateway",
            ControlVerb::all().iter().copied(),
            ControlMemberScope::All,
        ),
    );
    let audience = runtime.mob_id();
    let advertised = runtime
        .start_control_listener_with_authorizer(
            &ControlListenAddr::parse("tcp://127.0.0.1:0").expect("a control address"),
            Arc::new(ControlAuthorizer::with_grants_for_audience(
                table,
                audience.clone(),
            )),
        )
        .await
        .expect("the control listener binds");
    let endpoint = RemoteEndpoint::Tcp(
        advertised
            .strip_prefix("tcp://")
            .expect("a tcp listener")
            .to_string(),
    );
    (
        runtime,
        ControlPeer {
            keys,
            endpoint,
            audience,
        },
    )
}

/// The verbs of the member plane, by name.
const MEMBER_VERBS: [&str; 4] = ["lookup_member", "inject", "wire", "unwire"];

/// One request of the member verb `verb` for `member`, wiring a member of
/// a far mob where the verb wires.
fn member_request(verb: &str, member: &str, far: &GatewayPeerKeys) -> ControlRequest {
    let wire = |wire: bool| {
        let (remote_member, local_peer_spec_address, local_comms_name, local_peer_id) = (
            member.to_string(),
            "tcp://127.0.0.1:9".to_string(),
            "far-mob/worker/far".to_string(),
            meerkat_core::comms::PeerId::from_ed25519_pubkey(&far.pubkey_bytes()).to_string(),
        );
        let (local_pubkey_b64, nonce) = (Some(far.pubkey_b64()), Some("n".to_string()));
        if wire {
            ControlRequest::Wire {
                remote_member,
                local_peer_spec_address,
                local_comms_name,
                local_peer_id,
                local_pubkey_b64,
                nonce,
                caller: None,
            }
        } else {
            ControlRequest::Unwire {
                remote_member,
                local_peer_spec_address,
                local_comms_name,
                local_peer_id,
                local_pubkey_b64,
                nonce,
                caller: None,
            }
        }
    };
    match verb {
        "lookup_member" => ControlRequest::LookupMember {
            remote_member: member.to_string(),
            nonce: Some("n".to_string()),
            caller: None,
        },
        "inject" => ControlRequest::Inject {
            remote_member: member.to_string(),
            content: serde_json::to_value(meerkat_core::ContentInput::Text(
                "a word from a peer".to_string(),
            ))
            .expect("content"),
            nonce: Some("n".to_string()),
            caller: None,
        },
        "wire" => wire(true),
        "unwire" => wire(false),
        other => panic!("no member verb {other}"),
    }
}

/// What an answer says once the identity's name is taken out of it, as it
/// is spelled and as the roster keys its member.
fn shape(answer: &ControlResponse, identity: &str) -> String {
    format!("{answer:?}")
        .replace(identity, "<identity>")
        .replace(
            crate::member_comms_id::mob_member_id(identity).as_str(),
            "<identity>",
        )
}

/// Whether the listener's grant policy refused the request.
fn refused_by_grant(answer: &ControlResponse) -> bool {
    matches!(answer, ControlResponse::Err { code, .. } if ControlAuthzDenial::is_denial_code(code))
}

/// The numbers a row spells a generated alias with: small ones, and one no
/// counter holds.
const GENERATIONS: [&str; 4] = ["0", "1", "7", "99999999999999999999999"];

/// The compared pair by name, per member verb: a request for the identity
/// kept apart is answered exactly as the same request for a name no
/// identity has. The identity is registered and has no member: nothing
/// makes it one, and nothing reaches it.
#[tokio::test(flavor = "multi_thread")]
async fn an_identity_kept_apart_answers_each_member_verb_as_a_name_no_identity_has() {
    let state = tempfile::tempdir().expect("state dir");
    let (runtime, peer) = listening(state.path(), "mob-m-apart").await;
    let far = GatewayPeerKeys::ephemeral();

    for verb in MEMBER_VERBS {
        let for_the_identity = peer.ask(member_request(verb, APART, &far)).await;
        let for_no_identity = peer.ask(member_request(verb, NOBODY, &far)).await;
        assert!(
            !refused_by_grant(&for_no_identity),
            "{verb}: the grant reaches the name: {for_no_identity:?}"
        );
        assert!(
            matches!(for_no_identity, ControlResponse::Err { .. }),
            "{verb}: a name no identity has is refused: {for_no_identity:?}"
        );
        assert_eq!(
            shape(&for_the_identity, APART),
            shape(&for_no_identity, NOBODY),
            "{verb}: the identity kept apart is answered as a name no identity has"
        );
    }

    // An identity every caller may reach is not answered as a name no
    // identity has.
    let of_the_open_identity = peer.ask(member_request("lookup_member", OPEN, &far)).await;
    let of_no_identity = peer
        .ask(member_request("lookup_member", NOBODY, &far))
        .await;
    assert_ne!(
        shape(&of_the_open_identity, OPEN),
        shape(&of_no_identity, NOBODY),
        "an identity every caller may reach is not answered as unknown: {of_the_open_identity:?}"
    );
    runtime.shutdown().await;
}

/// A generated alias (`rt:<identity>:<n>`), per member verb and for several
/// numbers: the alias of the identity kept apart is answered exactly as the
/// same alias of a name no identity has. Whether a number is the identity's
/// current one is not told.
#[tokio::test(flavor = "multi_thread")]
async fn a_generated_alias_of_an_identity_kept_apart_answers_as_that_of_a_name_no_identity_has() {
    let state = tempfile::tempdir().expect("state dir");
    let (runtime, peer) = listening(state.path(), "mob-m-apart-alias").await;
    let far = GatewayPeerKeys::ephemeral();

    for verb in MEMBER_VERBS {
        for generation in GENERATIONS {
            let of_the_identity = format!("rt:{APART}:{generation}");
            let of_no_identity = format!("rt:{NOBODY}:{generation}");
            let for_the_identity = peer.ask(member_request(verb, &of_the_identity, &far)).await;
            let for_no_identity = peer.ask(member_request(verb, &of_no_identity, &far)).await;
            assert!(
                !refused_by_grant(&for_no_identity),
                "{verb}: the grant reaches the alias: {for_no_identity:?}"
            );
            assert!(
                matches!(for_no_identity, ControlResponse::Err { .. }),
                "{verb}: an alias of a name no identity has is refused: {for_no_identity:?}"
            );
            assert_eq!(
                shape(&for_the_identity, APART),
                shape(&for_no_identity, NOBODY),
                "{verb}, alias number {generation}: the identity kept apart is answered as a \
                 name no identity has"
            );
        }
    }
    runtime.shutdown().await;
}

/// An identity the runtime's record of its members keeps apart, whose
/// registered row does not say so, is answered by the keyed verbs exactly
/// as a name no identity has: its delivery is refused as that name's, and
/// its status reads as that name's. An identity neither the record nor its
/// row keeps apart is not refused as unknown.
#[tokio::test(flavor = "multi_thread")]
async fn an_identity_the_record_keeps_apart_answers_the_keyed_verbs_as_a_name_no_identity_has() {
    let state = tempfile::tempdir().expect("state dir");
    let (runtime, server) = server(state.path(), "mob-k-record").await;
    let scope = KeyedInjectCallerScope::for_grant_label("peer-gateway");
    let digest = content_digest("a body");
    let deadline = NOW_MS + WINDOW_MS;

    runtime.mob_runtime().private_owners().record(
        &runtime.mob_id(),
        crate::member_comms_id::mob_member_id(RECORDED).as_str(),
        Some(&BTreeMap::from([(
            crate::access::PRIVATE_TO_LABEL.to_string(),
            "someone".to_string(),
        )])),
    );
    let recorded = crate::identity_first::AgentIdentity::parse(RECORDED).expect("parse identity");
    assert!(
        runtime
            .identity_runtime_ref()
            .expect("an identity-first runtime")
            .registered_spec(&recorded)
            .await
            .is_ok_and(|registered| {
                crate::access::private_owner(&registered.spec.labels).is_none()
            }),
        "the identity is registered, and its registered row does not say it is kept apart"
    );

    let to_the_identity = server
        .deliver(&scope, delivery(RECORDED, &digest), deadline)
        .await;
    let to_no_identity = server
        .deliver(&scope, delivery(NOBODY, &digest), deadline)
        .await;
    let refusal_of_no_identity = format!("{:?}", to_no_identity.as_ref().err());
    assert!(
        to_no_identity.is_err(),
        "a delivery to a name no identity has is refused: {to_no_identity:?}"
    );
    assert_eq!(
        format!("{:?}", to_the_identity.as_ref().err()).replace(RECORDED, "<identity>"),
        refusal_of_no_identity.replace(NOBODY, "<identity>"),
        "a delivery to the identity the record keeps apart is refused as one to a name no \
         identity has"
    );

    let of_the_identity = server
        .status(&scope, RECORDED, "batch-1", true)
        .await
        .map(|answer| (answer.status, answer.input));
    let of_no_identity = server
        .status(&scope, NOBODY, "batch-1", true)
        .await
        .map(|answer| (answer.status, answer.input));
    assert_eq!(
        format!("{of_the_identity:?}").replace(RECORDED, "<identity>"),
        format!("{of_no_identity:?}").replace(NOBODY, "<identity>"),
        "the status of the identity the record keeps apart reads as that of a name no identity \
         has"
    );

    // An identity neither the record nor its row keeps apart is not
    // answered as unknown.
    let to_the_open_identity = server
        .deliver(&scope, delivery(OPEN, &digest), deadline)
        .await;
    assert_ne!(
        format!("{:?}", to_the_open_identity.as_ref().err()).replace(OPEN, "<identity>"),
        refusal_of_no_identity.replace(NOBODY, "<identity>"),
        "an identity every caller may reach is not refused as unknown: {to_the_open_identity:?}"
    );
    runtime.shutdown().await;
}
