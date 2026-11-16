//! Rows of the cross-mob control channel's member verbs for a member that is
//! kept apart from other callers.
//!
//! A peer gateway's control request names a member of this gateway's mob. A
//! member that is kept apart is answered exactly as a name no member has,
//! for every verb that names a member, whatever the caller's grant lists:
//! nothing is read of it, nothing is wired to it and nothing reaches its
//! session. A member that is not kept apart is answered as before.

#![allow(clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use meerkat_mob::{AgentIdentity, SpawnMemberSpec};

use crate::UnifiedRuntime;
use crate::auth::peer_keys::GatewayPeerKeys;
use crate::runtime::cross_mob_control::{
    ControlAuthorizer, ControlAuthzDenial, ControlGrant, ControlGrantTable, ControlListenAddr,
    ControlMemberScope, ControlRequest, ControlResponse, ControlVerb, RemoteControlClient,
    sign_control_request_as_caller,
};
use crate::runtime::cross_mob_remote::RemoteEndpoint;

/// A member every caller may reach.
const OPEN: &str = "lobby";
/// A member that is kept apart.
const APART: &str = "vault";
/// A name no member has.
const NOBODY: &str = "nobody";

const ANSWER: Duration = Duration::from_secs(30);

/// A gateway on the far side: its key, and where it dials the host.
struct Peer {
    keys: GatewayPeerKeys,
    endpoint: RemoteEndpoint,
    audience: String,
}

impl Peer {
    async fn ask(&self, mut request: ControlRequest) -> ControlResponse {
        sign_control_request_as_caller(&self.keys, Some(&self.audience), &mut request);
        RemoteControlClient::send(&self.endpoint, &request, ANSWER)
            .await
            .expect("the control exchange")
    }
}

/// A runtime of one mob with the members [`OPEN`] and [`APART`], the second
/// seated with the label that keeps a member apart.
async fn runtime_with_both_members() -> UnifiedRuntime {
    let definition = meerkat_mob::MobDefinition::from_toml(&format!(
        "[mob]\nid = \"control-member-verbs-{}\"\n\n[profiles.worker]\nmodel = \"gpt-5.5\"\nexternal_addressable = true\n\n[profiles.worker.tools]\ncomms = true\n",
        uuid::Uuid::new_v4().simple()
    ))
    .expect("mob definition");
    let runtime = UnifiedRuntime::builder()
        .definition(definition)
        .comms(true)
        .default_llm_client(Arc::new(meerkat_client::TestClient::default()))
        .build()
        .await
        .expect("runtime builds");
    let handle = runtime.mob_handle();
    Box::pin(handle.spawn_spec(SpawnMemberSpec::new("worker", AgentIdentity::from(OPEN))))
        .await
        .expect("the open member");
    let apart_labels = BTreeMap::from([(
        crate::access::PRIVATE_TO_LABEL.to_string(),
        "someone".to_string(),
    )]);
    Box::pin(handle.spawn_spec(
        SpawnMemberSpec::new("worker", AgentIdentity::from(APART)).with_labels(apart_labels),
    ))
    .await
    .expect("the member kept apart");
    // The runtime reads who its members are kept apart from in the
    // background from its start: the rows begin once that read is done.
    let record = runtime.mob_runtime().private_owners();
    record.first_attempts_settled(Duration::from_secs(30)).await;
    assert!(
        !record.first_read_pending(),
        "the runtime's first read of its members has succeeded"
    );
    runtime
}

/// Bind `runtime`'s control listener with one grant for one peer gateway:
/// every verb, over `members`.
async fn listen(runtime: &UnifiedRuntime, members: ControlMemberScope) -> Peer {
    let keys = GatewayPeerKeys::ephemeral();
    let mut table = ControlGrantTable::new();
    table.insert(
        keys.pubkey_bytes(),
        ControlGrant::new("peer-gateway", ControlVerb::all().iter().copied(), members),
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
    Peer {
        keys,
        endpoint: RemoteEndpoint::Tcp(
            advertised
                .strip_prefix("tcp://")
                .expect("a tcp listener")
                .to_string(),
        ),
        audience,
    }
}

/// A member of a far mob, as a wire request describes it.
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
}

/// The verbs of the member plane, by name.
const MEMBER_VERBS: [&str; 4] = ["lookup_member", "inject", "wire", "unwire"];

/// One request of the member verb `verb` for `member`.
fn request(verb: &str, member: &str, far: &FarMember) -> ControlRequest {
    let wire = |wire: bool| {
        let (remote_member, local_peer_spec_address, local_comms_name, local_peer_id) = (
            member.to_string(),
            "tcp://127.0.0.1:9".to_string(),
            far.comms_name.clone(),
            far.peer_id.clone(),
        );
        let (local_pubkey_b64, nonce) = (Some(far.pubkey_b64.clone()), Some("n".to_string()));
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

/// What an answer says once the member's name is taken out of it: the
/// variant, and for a refusal its code and its text.
fn shape(answer: &ControlResponse, member: &str) -> String {
    format!("{answer:?}").replace(member, "<member>")
}

/// Whether the listener's grant policy refused the request.
fn refused_by_grant(answer: &ControlResponse) -> bool {
    matches!(answer, ControlResponse::Err { code, .. } if ControlAuthzDenial::is_denial_code(code))
}

/// Whether any member of the runtime's mob is wired to the far member.
async fn anyone_wired_to(runtime: &UnifiedRuntime, far: &FarMember) -> bool {
    runtime
        .mob_handle()
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

/// The compared pair, per member verb: a request for the member kept apart
/// is answered exactly as the same request for a name no member has, under
/// a grant that names both. Nothing is wired and nothing is admitted.
#[tokio::test]
async fn a_member_kept_apart_answers_each_member_verb_as_a_name_no_member_has() {
    let runtime = runtime_with_both_members().await;
    let peer = listen(&runtime, ControlMemberScope::members([OPEN, APART, NOBODY])).await;
    let far = FarMember::new("far-mob/worker/far");

    for verb in MEMBER_VERBS {
        let for_the_member = peer.ask(request(verb, APART, &far)).await;
        let for_no_member = peer.ask(request(verb, NOBODY, &far)).await;
        assert!(
            !refused_by_grant(&for_no_member),
            "{verb}: the grant reaches the name: {for_no_member:?}"
        );
        assert!(
            matches!(for_no_member, ControlResponse::Err { .. }),
            "{verb}: a name no member has is refused: {for_no_member:?}"
        );
        assert_eq!(
            shape(&for_the_member, APART),
            shape(&for_no_member, NOBODY),
            "{verb}: the member kept apart is answered as a name no member has"
        );
    }
    assert!(
        !anyone_wired_to(&runtime, &far).await,
        "nothing was wired to the member kept apart"
    );
    runtime.shutdown().await;
}

/// A grant that lists no members reaches every member a caller may reach,
/// and still not a member kept apart.
#[tokio::test]
async fn a_grant_without_members_does_not_reach_a_member_kept_apart() {
    let runtime = runtime_with_both_members().await;
    let peer = listen(&runtime, ControlMemberScope::All).await;
    let far = FarMember::new("far-mob/worker/far");

    for verb in MEMBER_VERBS {
        let for_the_member = peer.ask(request(verb, APART, &far)).await;
        let for_no_member = peer.ask(request(verb, NOBODY, &far)).await;
        assert_eq!(
            shape(&for_the_member, APART),
            shape(&for_no_member, NOBODY),
            "{verb}: a grant for every member does not reach the member kept apart"
        );
    }
    assert!(!anyone_wired_to(&runtime, &far).await);
    runtime.shutdown().await;
}

/// A member that is not kept apart is answered as before: it is found, an
/// inject is admitted into its session, and a wire and an unwire change its
/// roster entry.
#[tokio::test]
async fn a_member_every_caller_may_reach_answers_as_before() {
    let runtime = runtime_with_both_members().await;
    let peer = listen(&runtime, ControlMemberScope::All).await;
    let far = FarMember::new("far-mob/worker/far");
    let session = runtime
        .mob_handle()
        .resolve_bridge_session_id(&AgentIdentity::from(OPEN))
        .await
        .expect("the open member's session");

    let found = peer.ask(request("lookup_member", OPEN, &far)).await;
    assert!(
        matches!(found, ControlResponse::Member { .. }),
        "the open member is found: {found:?}"
    );
    let injected = peer.ask(request("inject", OPEN, &far)).await;
    assert_eq!(
        injected,
        ControlResponse::Injected {
            session_id: session.to_string(),
            sig_b64: None,
        },
        "an inject is admitted into the open member's session"
    );
    let wired = peer.ask(request("wire", OPEN, &far)).await;
    assert_eq!(wired, ControlResponse::Ok { sig_b64: None });
    assert!(
        anyone_wired_to(&runtime, &far).await,
        "the wire is in the open member's roster entry"
    );
    let unwired = peer.ask(request("unwire", OPEN, &far)).await;
    assert_eq!(unwired, ControlResponse::Ok { sig_b64: None });
    assert!(!anyone_wired_to(&runtime, &far).await);
    runtime.shutdown().await;
}

/// While the runtime still owes the first read of which members are kept
/// apart, it cannot tell: every member is answered as a name no member has
/// until the read has succeeded, and as before afterwards.
#[tokio::test]
async fn while_the_first_read_is_owed_every_member_is_answered_as_no_member() {
    let runtime = runtime_with_both_members().await;
    let peer = listen(&runtime, ControlMemberScope::All).await;
    let far = FarMember::new("far-mob/worker/far");

    let owed = runtime
        .mob_runtime()
        .private_owners()
        .owe_a_first_read_in_tests();
    for verb in MEMBER_VERBS {
        let for_the_open_member = peer.ask(request(verb, OPEN, &far)).await;
        let for_no_member = peer.ask(request(verb, NOBODY, &far)).await;
        assert_eq!(
            shape(&for_the_open_member, OPEN),
            shape(&for_no_member, NOBODY),
            "{verb}: no member is told from a name no member has while the read is owed"
        );
    }
    assert!(!anyone_wired_to(&runtime, &far).await);

    drop(owed);
    let found = peer.ask(request("lookup_member", OPEN, &far)).await;
    assert!(
        matches!(found, ControlResponse::Member { .. }),
        "once the read succeeded the open member is found: {found:?}"
    );
    runtime.shutdown().await;
}
