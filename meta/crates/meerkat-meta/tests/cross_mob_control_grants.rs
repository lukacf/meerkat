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
    ControlAuthorizer, ControlGrant, ControlGrantTable, ControlHandler, ControlMemberScope,
    ControlRequest, ControlResponse, ControlVerb, serve_tcp_control_with_authorizer,
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
