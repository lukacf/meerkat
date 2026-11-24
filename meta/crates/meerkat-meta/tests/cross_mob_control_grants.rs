#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::uninlined_format_args
)]
//! Scoped grants on the cross-mob control channel, exercised through the
//! crate's PUBLIC surface over a real socket.
//!
//! The unit suite in `src/runtime/cross_mob_control.rs` covers the policy
//! algebra (verbs, member scope, signature binding, config parsing). What
//! this file pins is the part a unit test cannot: that an embedder can
//! reach the grant types, mount them on a listener, and that a refusal
//! happens BEFORE the `ControlHandler` runs - the property that makes
//! `ControlRequest::Inject` on an exposed port safe rather than merely
//! discouraged.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use meerkat_meta::GatewayPeerKeys;
use meerkat_meta::contact_directory::{ContactEntry, MobTransport};
use meerkat_meta::runtime::cross_mob_control::{
    ControlAuthorizer, ControlAuthzDenial, ControlGrant, ControlGrantTable, ControlHandler,
    ControlMemberScope, ControlMobScope, ControlRequest, ControlResponse, ControlSignerSlot,
    ControlVerb, HOST_OWN_KEY_CONTROL_CODE, serve_tcp_control_with_authorizer,
    unsigned_control_signer,
};
use meerkat_meta::runtime::cross_mob_remote::{RemoteMobError, RemoteMobProxy};

/// Counts every request that reaches the handler, so a test can assert
/// that a refused request never got that far.
struct CountingHandler {
    dispatched: Arc<AtomicUsize>,
}

impl ControlHandler for CountingHandler {
    fn handle(
        &self,
        request: ControlRequest,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ControlResponse> + Send + '_>> {
        let dispatched = Arc::clone(&self.dispatched);
        Box::pin(async move {
            dispatched.fetch_add(1, Ordering::SeqCst);
            match request {
                ControlRequest::Inject { remote_member, .. } => ControlResponse::Injected {
                    session_id: format!("session-for-{}", remote_member),
                    sig_b64: None,
                },
                _ => ControlResponse::Member {
                    peer_id: "peer-id".to_string(),
                    comms_name: "mob/role/member".to_string(),
                    pubkey_b64: None,
                    advertised_address: None,
                    sig_b64: None,
                },
            }
        })
    }
}

fn contact(mob_id: &str, addr: &str) -> ContactEntry {
    ContactEntry {
        mob_id: mob_id.to_string(),
        transport: MobTransport::Tcp(addr.to_string()),
        pubkey: None,
        require_signed_control: None,
    }
}

/// A caller granted `inject` on exactly one member: the granted call
/// lands, everything else is refused, and nothing refused ever reaches
/// the handler.
#[tokio::test]
async fn scoped_grant_admits_only_its_verb_and_member() {
    let caller_keys = Arc::new(GatewayPeerKeys::ephemeral());
    let mut table = ControlGrantTable::new();
    table.insert(
        caller_keys.pubkey_bytes(),
        ControlGrant::new(
            "ops-mob",
            [ControlVerb::Inject],
            ControlMemberScope::members(["bob"]),
        ),
    );
    let authorizer = Arc::new(ControlAuthorizer::with_grants(table));

    let dispatched = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let handler: Arc<dyn ControlHandler> = Arc::new(CountingHandler {
        dispatched: Arc::clone(&dispatched),
    });
    let server = tokio::spawn(serve_tcp_control_with_authorizer(
        listener,
        handler,
        unsigned_control_signer(),
        authorizer,
    ));

    let entry = contact("remote", &addr.to_string());
    let proxy = RemoteMobProxy::from_entry_with_caller(&entry, Some(Arc::clone(&caller_keys)))
        .expect("tcp is supported")
        .expect("tcp yields a proxy");

    let session_id = proxy
        .inject_message("bob", serde_json::json!({"text": "hello"}))
        .await
        .expect("the granted verb on the granted member is admitted");
    assert_eq!(session_id, "session-for-bob");
    assert_eq!(dispatched.load(Ordering::SeqCst), 1);

    // Same verb, different member.
    let wrong_member = proxy
        .inject_message("carol", serde_json::json!({"text": "hello"}))
        .await
        .expect_err("carol is outside the member scope");
    assert!(
        matches!(
            wrong_member,
            RemoteMobError::Rejected { ref code, .. } if code == "member_not_granted"
        ),
        "got {:?}",
        wrong_member
    );

    // Granted member, ungranted verb.
    let wrong_verb = proxy
        .lookup_member("bob")
        .await
        .expect_err("lookup_member is outside the verb scope");
    assert!(
        matches!(
            wrong_verb,
            RemoteMobError::Rejected { ref code, .. } if code == "verb_not_granted"
        ),
        "got {:?}",
        wrong_verb
    );

    // The point of authorizing at the listener seam: a refused request is
    // never handed to the handler, so it cannot reach a member's session
    // and cannot disclose whether that member exists.
    assert_eq!(
        dispatched.load(Ordering::SeqCst),
        1,
        "refused requests must not reach the handler"
    );
    server.abort();
}

/// A gateway with no keypair cannot authenticate, and a gateway whose key
/// is not filed holds no grant. Both are refused typed, and neither
/// reaches the handler.
#[tokio::test]
async fn unauthenticated_and_unknown_callers_are_refused() {
    let granted_keys = Arc::new(GatewayPeerKeys::ephemeral());
    let stranger_keys = Arc::new(GatewayPeerKeys::ephemeral());
    let mut table = ControlGrantTable::new();
    table.insert(
        granted_keys.pubkey_bytes(),
        ControlGrant::new(
            "ops-mob",
            ControlVerb::all().iter().copied(),
            ControlMemberScope::All,
        ),
    );
    let authorizer = Arc::new(ControlAuthorizer::with_grants(table));

    let dispatched = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let handler: Arc<dyn ControlHandler> = Arc::new(CountingHandler {
        dispatched: Arc::clone(&dispatched),
    });
    let server = tokio::spawn(serve_tcp_control_with_authorizer(
        listener,
        handler,
        unsigned_control_signer(),
        authorizer,
    ));

    let entry = contact("remote", &addr.to_string());

    let anonymous = RemoteMobProxy::from_entry(&entry)
        .expect("tcp is supported")
        .expect("tcp yields a proxy");
    let anonymous_error = anonymous
        .inject_message("bob", serde_json::json!({"text": "hello"}))
        .await
        .expect_err("a caller with no keypair cannot authenticate");
    assert!(
        matches!(
            anonymous_error,
            RemoteMobError::Rejected { ref code, .. } if code == "unauthenticated_caller"
        ),
        "got {:?}",
        anonymous_error
    );

    let stranger = RemoteMobProxy::from_entry_with_caller(&entry, Some(stranger_keys))
        .expect("tcp is supported")
        .expect("tcp yields a proxy");
    let stranger_error = stranger
        .inject_message("bob", serde_json::json!({"text": "hello"}))
        .await
        .expect_err("an unfiled caller holds no grant");
    assert!(
        matches!(
            stranger_error,
            RemoteMobError::Rejected { ref code, .. } if code == "caller_not_granted"
        ),
        "got {:?}",
        stranger_error
    );

    assert_eq!(dispatched.load(Ordering::SeqCst), 0);
    server.abort();
}

/// Rollout compatibility in the direction that matters for a mixed fleet:
/// a listener with NO grant table keeps serving both authenticated and
/// unauthenticated callers unchanged. Caller authorization is inert until
/// an operator installs grants.
#[tokio::test]
async fn open_listener_is_unchanged_for_both_caller_shapes() {
    let caller_keys = Arc::new(GatewayPeerKeys::ephemeral());
    let dispatched = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let handler: Arc<dyn ControlHandler> = Arc::new(CountingHandler {
        dispatched: Arc::clone(&dispatched),
    });
    let server = tokio::spawn(serve_tcp_control_with_authorizer(
        listener,
        handler,
        unsigned_control_signer(),
        Arc::new(ControlAuthorizer::open()),
    ));

    let entry = contact("remote", &addr.to_string());
    let signed = RemoteMobProxy::from_entry_with_caller(&entry, Some(caller_keys))
        .expect("tcp is supported")
        .expect("tcp yields a proxy");
    signed
        .lookup_member("bob")
        .await
        .expect("an open listener ignores the credential");

    let unsigned = RemoteMobProxy::from_entry(&entry)
        .expect("tcp is supported")
        .expect("tcp yields a proxy");
    unsigned
        .lookup_member("bob")
        .await
        .expect("an open listener still serves unauthenticated callers");

    assert_eq!(dispatched.load(Ordering::SeqCst), 2);
    server.abort();
}

/// The config seam the deferred gateway plumbing will call: an absent
/// `[control_grants]` section leaves the listener open, a present one
/// enforces. Emptiness is never a fallback to open.
#[test]
fn authorizer_config_seam_distinguishes_absent_from_empty() {
    let absent = ControlAuthorizer::from_toml("[mobs]\nremote = \"inproc\"\n").expect("parse");
    assert!(!absent.is_enforcing(), "absent section must stay open");

    let empty = ControlAuthorizer::from_toml("[control_grants]\n").expect("parse");
    assert!(
        empty.is_enforcing(),
        "a present but empty section is a deny-all policy, not an absent one"
    );

    let keys = GatewayPeerKeys::ephemeral();
    let populated = ControlAuthorizer::from_toml(&format!(
        r#"
        [mobs]
        remote = "inproc"

        [control_grants.ops-mob]
        pubkey = "{}"
        verbs = ["inject"]
        members = ["bob"]
        "#,
        keys.pubkey_b64()
    ))
    .expect("parse");
    assert!(populated.is_enforcing());
}

async fn serve_counting(
    authorizer: ControlAuthorizer,
    signer: ControlSignerSlot,
) -> (
    std::net::SocketAddr,
    Arc<AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    let dispatched = Arc::new(AtomicUsize::new(0));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    let handler: Arc<dyn ControlHandler> = Arc::new(CountingHandler {
        dispatched: Arc::clone(&dispatched),
    });
    let server = tokio::spawn(serve_tcp_control_with_authorizer(
        listener,
        handler,
        signer,
        Arc::new(authorizer),
    ));
    (addr, dispatched, server)
}

fn rejected_code(error: &RemoteMobError) -> Option<&str> {
    match error {
        RemoteMobError::Rejected { code, .. } => Some(code.as_str()),
        _ => None,
    }
}

/// A grant reaches the gateway's default mob unless it lists mobs, and a
/// grant that lists mobs reaches the default mob only by its id. The mob a
/// request is for is the audience its caller signed (the contact's mob id),
/// and a request for a mob outside the grant's mobs is refused before the
/// handler runs, as a request for a mob the gateway does not have.
#[tokio::test]
async fn grant_mobs_scope_is_enforced_before_the_handler() {
    const DEFAULT_MOB: &str = "mob-default";
    let default_keys = Arc::new(GatewayPeerKeys::ephemeral());
    let listed_keys = Arc::new(GatewayPeerKeys::ephemeral());
    let elsewhere_keys = Arc::new(GatewayPeerKeys::ephemeral());
    let every_keys = Arc::new(GatewayPeerKeys::ephemeral());
    let table = ControlGrantTable::from_toml(&format!(
        r#"
        [control_grants.unlisted]
        pubkey = "{}"
        verbs = ["inject"]

        [control_grants.listed]
        pubkey = "{}"
        verbs = ["inject"]
        mobs = ["mob-default", "mob-second"]

        [control_grants.elsewhere]
        pubkey = "{}"
        verbs = ["inject"]
        mobs = ["mob-second"]

        [control_grants.every]
        pubkey = "{}"
        verbs = ["inject"]
        mobs = ["*"]
        "#,
        default_keys.pubkey_b64(),
        listed_keys.pubkey_b64(),
        elsewhere_keys.pubkey_b64(),
        every_keys.pubkey_b64(),
    ))
    .expect("parse")
    .expect("section is present");
    assert_eq!(
        table
            .get(&elsewhere_keys.pubkey_bytes())
            .expect("filed")
            .mobs(),
        &ControlMobScope::mobs(["mob-second"])
    );
    let (addr, dispatched, server) = serve_counting(
        ControlAuthorizer::with_grants_for_audience(table, DEFAULT_MOB),
        unsigned_control_signer(),
    )
    .await;
    let entry = contact(DEFAULT_MOB, &addr.to_string());
    let proxy_for = |keys: &Arc<GatewayPeerKeys>| {
        RemoteMobProxy::from_entry_with_caller(&entry, Some(Arc::clone(keys)))
            .expect("tcp is supported")
            .expect("tcp yields a proxy")
    };

    for (admitted, keys) in [&default_keys, &listed_keys, &every_keys]
        .into_iter()
        .enumerate()
    {
        proxy_for(keys)
            .inject_message("bob", serde_json::json!({"text": "hello"}))
            .await
            .expect("a grant that reaches the default mob is admitted there");
        assert_eq!(dispatched.load(Ordering::SeqCst), admitted + 1);
    }

    let outside = proxy_for(&elsewhere_keys)
        .inject_message("bob", serde_json::json!({"text": "hello"}))
        .await
        .expect_err("a grant that lists only another mob does not reach the default mob");
    assert_eq!(
        rejected_code(&outside),
        Some("audience_mismatch"),
        "got {:?}",
        outside
    );

    // A mob this gateway does not have is no audience of it, whatever a
    // grant lists.
    let second = contact("mob-second", &addr.to_string());
    for keys in [&elsewhere_keys, &every_keys] {
        let unknown = RemoteMobProxy::from_entry_with_caller(&second, Some(Arc::clone(keys)))
            .expect("tcp is supported")
            .expect("tcp yields a proxy")
            .inject_message("bob", serde_json::json!({"text": "hello"}))
            .await
            .expect_err("the gateway has no such mob");
        assert_eq!(
            rejected_code(&unknown),
            Some("audience_mismatch"),
            "got {:?}",
            unknown
        );
    }
    assert_eq!(
        dispatched.load(Ordering::SeqCst),
        3,
        "refused requests must not reach the handler"
    );
    server.abort();
}

/// A gateway never serves a control request made with its own key: the
/// loopback through its own listener is refused before the grant table is
/// read, with its own code, which is not a grant refusal.
#[tokio::test]
async fn listener_refuses_its_own_key_whatever_the_grants_say() {
    let own_keys = Arc::new(GatewayPeerKeys::ephemeral());
    let peer_keys = Arc::new(GatewayPeerKeys::ephemeral());
    let mut table = ControlGrantTable::new();
    for keys in [&own_keys, &peer_keys] {
        table.insert(
            keys.pubkey_bytes(),
            ControlGrant::new(
                "everything",
                ControlVerb::all().iter().copied(),
                ControlMemberScope::All,
            )
            .with_mobs(ControlMobScope::All),
        );
    }
    let signer: ControlSignerSlot = Arc::new(std::sync::RwLock::new(Some(Arc::clone(&own_keys))));
    let (addr, dispatched, server) = serve_counting(
        ControlAuthorizer::with_grants_for_audience(table, "mob-default"),
        signer,
    )
    .await;
    let entry = contact("mob-default", &addr.to_string());

    let own = RemoteMobProxy::from_entry_with_caller(&entry, Some(Arc::clone(&own_keys)))
        .expect("tcp is supported")
        .expect("tcp yields a proxy")
        .inject_message("bob", serde_json::json!({"text": "hello"}))
        .await
        .expect_err("the gateway's own key is never served");
    assert_eq!(
        rejected_code(&own),
        Some(HOST_OWN_KEY_CONTROL_CODE),
        "got {:?}",
        own
    );
    assert!(!ControlAuthzDenial::is_denial_code(
        HOST_OWN_KEY_CONTROL_CODE
    ));
    assert_eq!(dispatched.load(Ordering::SeqCst), 0);

    RemoteMobProxy::from_entry_with_caller(&entry, Some(peer_keys))
        .expect("tcp is supported")
        .expect("tcp yields a proxy")
        .inject_message("bob", serde_json::json!({"text": "hello"}))
        .await
        .expect("another gateway with the same grant is served");
    assert_eq!(dispatched.load(Ordering::SeqCst), 1);
    server.abort();
}
