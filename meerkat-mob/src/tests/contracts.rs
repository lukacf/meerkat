//! Contract tests verifying assumptions about Meerkat platform APIs.
//!
//! These tests prove that the comms primitives meerkat-mob depends on
//! behave as expected. Each test is tagged with a CONTRACT-MOB-NNN
//! identifier matching `.rct/mob/spec.yaml`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::redundant_clone
)]

use async_trait::async_trait;
use meerkat_comms::CommsRuntime;
use meerkat_core::agent::CommsRuntime as CoreCommsRuntime;
use meerkat_core::comms::{
    CommsCommand, InputStreamMode, PeerDirectorySource, PeerName, SendReceipt, TrustedPeerSpec,
};
use meerkat_core::service::{
    CreateSessionRequest, SessionBuildOptions, SessionError, SessionInfo, SessionQuery,
    SessionService, SessionSummary, SessionUsage, SessionView, StartTurnRequest,
};
use meerkat_core::types::{RunResult, SessionId, Usage};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::sync::RwLock;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// CONTRACT-MOB-002: PeerRequest/PeerResponse round-trip via CommsRuntime
// ---------------------------------------------------------------------------

#[tokio::test]
async fn contract_mob_002_peer_request_response_round_trip() {
    let suffix = Uuid::new_v4().simple().to_string();
    let sender_name = format!("c002-sender-{suffix}");
    let receiver_name = format!("c002-receiver-{suffix}");

    let sender = CommsRuntime::inproc_only(&sender_name).unwrap();
    let receiver = CommsRuntime::inproc_only(&receiver_name).unwrap();

    // Establish bidirectional trust
    let peer_spec = TrustedPeerSpec::new(
        &receiver_name,
        receiver.public_key().to_peer_id(),
        format!("inproc://{receiver_name}"),
    )
    .expect("valid peer spec");
    CoreCommsRuntime::add_trusted_peer(&sender, peer_spec)
        .await
        .expect("add sender->receiver trust");

    let reverse_spec = TrustedPeerSpec::new(
        &sender_name,
        sender.public_key().to_peer_id(),
        format!("inproc://{sender_name}"),
    )
    .expect("valid reverse spec");
    CoreCommsRuntime::add_trusted_peer(&receiver, reverse_spec)
        .await
        .expect("add receiver->sender trust");

    // Send PeerRequest from sender to receiver
    let request_cmd = CommsCommand::PeerRequest {
        to: PeerName::new(receiver_name.clone()).expect("valid peer name"),
        intent: "mob.ping".to_string(),
        params: serde_json::json!({"seq": 1}),
        stream: InputStreamMode::None,
    };
    let receipt = CoreCommsRuntime::send(&sender, request_cmd)
        .await
        .expect("PeerRequest send should succeed");
    assert!(
        matches!(receipt, SendReceipt::PeerRequestSent { .. }),
        "expected PeerRequestSent, got: {receipt:?}"
    );

    // Receiver drains inbox and sees the request
    let interactions = CoreCommsRuntime::drain_inbox_interactions(&receiver).await;
    assert_eq!(
        interactions.len(),
        1,
        "receiver should see exactly one interaction"
    );
    let request_interaction = &interactions[0];
    assert_eq!(request_interaction.from, sender_name);

    let request_id = match &request_interaction.content {
        meerkat_core::InteractionContent::Request { intent, params } => {
            assert_eq!(intent, "mob.ping");
            assert_eq!(params["seq"], 1);
            request_interaction.id
        }
        other => panic!("expected Request interaction, got: {other:?}"),
    };

    // Receiver sends PeerResponse back
    let response_cmd = CommsCommand::PeerResponse {
        to: PeerName::new(sender_name.clone()).expect("valid peer name"),
        in_reply_to: request_id,
        status: meerkat_core::ResponseStatus::Completed,
        result: serde_json::json!({"pong": true}),
    };
    let resp_receipt = CoreCommsRuntime::send(&receiver, response_cmd)
        .await
        .expect("PeerResponse send should succeed");
    assert!(
        matches!(resp_receipt, SendReceipt::PeerResponseSent { .. }),
        "expected PeerResponseSent, got: {resp_receipt:?}"
    );

    // Sender drains inbox and sees the response
    let sender_interactions = CoreCommsRuntime::drain_inbox_interactions(&sender).await;
    assert_eq!(
        sender_interactions.len(),
        1,
        "sender should see exactly one response interaction"
    );
    match &sender_interactions[0].content {
        meerkat_core::InteractionContent::Response {
            in_reply_to,
            status,
            result,
        } => {
            assert_eq!(*in_reply_to, request_id);
            assert_eq!(*status, meerkat_core::ResponseStatus::Completed);
            assert_eq!(result["pong"], true);
        }
        other => panic!("expected Response interaction, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// CONTRACT-MOB-003: Inproc namespace isolation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn contract_mob_003_inproc_namespace_isolation() {
    let suffix = Uuid::new_v4().simple().to_string();

    // Create two agents in namespace "mob:alpha"
    let alpha_a_name = format!("c003-alpha-a-{suffix}");
    let alpha_b_name = format!("c003-alpha-b-{suffix}");
    let alpha_a =
        CommsRuntime::inproc_only_scoped(&alpha_a_name, Some("mob:alpha".to_string())).unwrap();
    let alpha_b =
        CommsRuntime::inproc_only_scoped(&alpha_b_name, Some("mob:alpha".to_string())).unwrap();

    // Create one agent in namespace "mob:beta"
    let beta_name = format!("c003-beta-{suffix}");
    let _beta = CommsRuntime::inproc_only_scoped(&beta_name, Some("mob:beta".to_string())).unwrap();

    // alpha_a's peers should see alpha_b but NOT beta
    // (inproc_only_scoped does not require_peer_auth, but the router
    //  only looks up peers in its own namespace)
    let alpha_a_peers = CoreCommsRuntime::peers(&alpha_a).await;
    let alpha_a_peer_names: Vec<String> = alpha_a_peers
        .iter()
        .map(|e| e.name.as_string().clone())
        .collect();

    // alpha_b should NOT be visible either, because inproc_only_scoped
    // uses require_peer_auth=true, meaning only trusted peers show up
    // by default. But the critical assertion is that beta never shows up.
    assert!(
        !alpha_a_peer_names.contains(&beta_name),
        "beta namespace agent must not appear in alpha namespace peers"
    );

    // Now add alpha_b as trusted peer of alpha_a (within the same namespace)
    let spec = TrustedPeerSpec::new(
        &alpha_b_name,
        alpha_b.public_key().to_peer_id(),
        format!("inproc://{alpha_b_name}"),
    )
    .expect("valid spec");
    CoreCommsRuntime::add_trusted_peer(&alpha_a, spec)
        .await
        .expect("add trusted peer within namespace");

    let alpha_a_peers_after = CoreCommsRuntime::peers(&alpha_a).await;
    let peer_names_after: Vec<String> = alpha_a_peers_after
        .iter()
        .map(|e| e.name.as_string().clone())
        .collect();

    assert!(
        peer_names_after.contains(&alpha_b_name),
        "alpha_b should be visible after trusting within same namespace"
    );
    assert!(
        !peer_names_after.contains(&beta_name),
        "beta should still not be visible after trusting alpha_b"
    );
}

// ---------------------------------------------------------------------------
// CONTRACT-MOB-004: add_trusted_peer is idempotent upsert
// ---------------------------------------------------------------------------

#[tokio::test]
async fn contract_mob_004_add_trusted_peer_is_idempotent() {
    let suffix = Uuid::new_v4().simple().to_string();
    let runtime_name = format!("c004-runtime-{suffix}");
    let peer_name = format!("c004-peer-{suffix}");

    let runtime = CommsRuntime::inproc_only(&runtime_name).unwrap();
    let peer = CommsRuntime::inproc_only(&peer_name).unwrap();

    let make_spec = || {
        TrustedPeerSpec::new(
            &peer_name,
            peer.public_key().to_peer_id(),
            format!("inproc://{peer_name}"),
        )
        .expect("valid spec")
    };

    // Add the same peer twice
    CoreCommsRuntime::add_trusted_peer(&runtime, make_spec())
        .await
        .expect("first add should succeed");
    CoreCommsRuntime::add_trusted_peer(&runtime, make_spec())
        .await
        .expect("second (idempotent) add should succeed");

    // Verify no duplicates in peers list
    let peers = CoreCommsRuntime::peers(&runtime).await;
    let matching: Vec<_> = peers
        .iter()
        .filter(|e| e.name.as_str() == peer_name)
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "idempotent add should not create duplicates; found {} entries",
        matching.len()
    );
}

// ---------------------------------------------------------------------------
// CONTRACT-MOB-005: remove_trusted_peer revokes send capability
// ---------------------------------------------------------------------------

#[tokio::test]
async fn contract_mob_005_remove_trusted_peer_revokes_send() {
    let suffix = Uuid::new_v4().simple().to_string();
    let sender_name = format!("c005-sender-{suffix}");
    let receiver_name = format!("c005-receiver-{suffix}");

    let sender = CommsRuntime::inproc_only(&sender_name).unwrap();
    let receiver = CommsRuntime::inproc_only(&receiver_name).unwrap();

    // Establish trust
    let spec = TrustedPeerSpec::new(
        &receiver_name,
        receiver.public_key().to_peer_id(),
        format!("inproc://{receiver_name}"),
    )
    .expect("valid spec");
    CoreCommsRuntime::add_trusted_peer(&sender, spec)
        .await
        .expect("add trusted peer");

    // Verify send works before removal
    let cmd = CommsCommand::PeerMessage {
        to: PeerName::new(receiver_name.clone()).expect("valid peer name"),
        body: "before removal".to_string(),
    };
    let receipt = CoreCommsRuntime::send(&sender, cmd).await;
    assert!(
        matches!(receipt, Ok(SendReceipt::PeerMessageSent { .. })),
        "send should succeed before removal"
    );

    // Drain the message so it doesn't interfere
    let _ = CoreCommsRuntime::drain_inbox_interactions(&receiver).await;

    // Remove trusted peer
    let peer_id = receiver.public_key().to_peer_id();
    let removed = CoreCommsRuntime::remove_trusted_peer(&sender, &peer_id)
        .await
        .expect("remove should succeed");
    assert!(removed, "should return true for existing peer");

    // Verify peers() no longer returns the removed peer
    let peers_after = CoreCommsRuntime::peers(&sender).await;
    assert!(
        !peers_after.iter().any(|e| e.name.as_str() == receiver_name),
        "removed peer should not appear in peers()"
    );

    // Verify send fails after removal
    let cmd_after = CommsCommand::PeerMessage {
        to: PeerName::new(receiver_name.clone()).expect("valid peer name"),
        body: "after removal".to_string(),
    };
    let result = CoreCommsRuntime::send(&sender, cmd_after).await;
    assert!(
        matches!(result, Err(meerkat_core::SendError::PeerNotFound(_))),
        "send should fail with PeerNotFound after removal, got: {result:?}"
    );
}

// ---------------------------------------------------------------------------
// CONTRACT-MOBX-001: trust operations accept backend-provided addresses
// ---------------------------------------------------------------------------

#[tokio::test]
async fn contract_mobx_001_trust_accepts_non_inproc_addresses_and_preserves_peer_id() {
    let suffix = Uuid::new_v4().simple().to_string();
    let runtime_name = format!("c0x1-runtime-{suffix}");
    let peer_name = format!("c0x1-peer-{suffix}");

    let runtime = CommsRuntime::inproc_only(&runtime_name).unwrap();
    let peer = CommsRuntime::inproc_only(&peer_name).unwrap();
    let peer_id = peer.public_key().to_peer_id();
    let backend_address = format!("https://backend.example.invalid/mesh/{peer_name}");

    let spec = TrustedPeerSpec::new(&peer_name, peer_id.clone(), backend_address.clone())
        .expect("valid trusted peer spec");
    CoreCommsRuntime::add_trusted_peer(&runtime, spec)
        .await
        .expect("add trusted peer should accept backend-provided address");

    let peers_after_add = CoreCommsRuntime::peers(&runtime).await;
    let entry = peers_after_add
        .iter()
        .find(|entry| entry.name.as_str() == peer_name)
        .expect("trusted peer should be listed");
    assert_eq!(
        entry.peer_id, peer_id,
        "peer_id semantics must remain stable for remove operations"
    );
    assert_eq!(
        entry.address, backend_address,
        "runtime should preserve backend-provided address string"
    );

    let removed = CoreCommsRuntime::remove_trusted_peer(&runtime, &peer_id)
        .await
        .expect("remove_trusted_peer should succeed by peer_id");
    assert!(
        removed,
        "remove_trusted_peer should return true for existing peer"
    );

    let peers_after_remove = CoreCommsRuntime::peers(&runtime).await;
    assert!(
        peers_after_remove
            .iter()
            .all(|entry| entry.name.as_str() != peer_name),
        "peer should no longer appear after remove_trusted_peer"
    );
}

// ---------------------------------------------------------------------------
// CONTRACT-MOB-006: PeerMeta labels discoverable via peers()
// ---------------------------------------------------------------------------

/// Helper: build a `ResolvedCommsConfig` suitable for contract tests.
///
/// Uses `require_peer_auth: false` so that inproc-registered peers
/// (with their PeerMeta) appear in `peers()` without needing to
/// manipulate private trust state.
fn test_config(
    name: &str,
    tmp: &tempfile::TempDir,
    namespace: Option<String>,
) -> meerkat_comms::ResolvedCommsConfig {
    meerkat_comms::ResolvedCommsConfig {
        enabled: true,
        name: name.to_string(),
        inproc_namespace: namespace,
        listen_uds: None,
        listen_tcp: None,
        event_listen_tcp: None,
        #[cfg(unix)]
        event_listen_uds: None,
        identity_dir: tmp.path().join("identity"),
        trusted_peers_path: tmp.path().join("trusted_peers.json"),
        comms_config: meerkat_comms::CommsConfig::default(),
        auth: meerkat_core::CommsAuthMode::Open,
        require_peer_auth: false,
        allow_external_unauthenticated: false,
    }
}

#[tokio::test]
async fn contract_mob_006_peer_meta_labels_discoverable_via_peers() {
    let suffix = Uuid::new_v4().simple().to_string();
    let ns = format!("c006-{suffix}");
    let runtime_name = format!("c006-runtime-{suffix}");
    let peer_name = format!("c006-peer-{suffix}");

    // Create a peer runtime in the same namespace so it registers with meta
    let peer_tmp = tempfile::tempdir().unwrap();
    let peer_config = test_config(&peer_name, &peer_tmp, Some(ns.clone()));
    let _peer = CommsRuntime::new(peer_config).await.unwrap();

    // Now manually register the peer with rich meta in the inproc registry
    // (simulating what the mob runtime does when it sets up PeerMeta with labels)
    let meta = meerkat_core::PeerMeta::default()
        .with_description("test worker")
        .with_label("mob_id", "test-mob")
        .with_label("role", "coder");

    // Unregister the default entry and re-register with meta
    let peer_pubkey = _peer.public_key();
    meerkat_comms::InprocRegistry::global().unregister(&peer_pubkey);

    let (_, inbox_sender) = meerkat_comms::Inbox::new();
    meerkat_comms::InprocRegistry::global().register_with_meta_in_namespace(
        &ns,
        &peer_name,
        peer_pubkey,
        inbox_sender,
        meta.clone(),
    );

    // Create a runtime that sees inproc peers (require_peer_auth=false)
    let runtime_tmp = tempfile::tempdir().unwrap();
    let runtime_config = test_config(&runtime_name, &runtime_tmp, Some(ns));
    let runtime = CommsRuntime::new(runtime_config).await.unwrap();

    // Retrieve via peers() and verify labels are present
    let peers = CoreCommsRuntime::peers(&runtime).await;
    let matching: Vec<_> = peers
        .iter()
        .filter(|e| e.name.as_str() == peer_name)
        .collect();
    assert_eq!(matching.len(), 1, "peer should appear exactly once");

    let entry = matching[0];
    assert_eq!(entry.meta, meta);
    assert_eq!(
        entry.meta.description.as_deref(),
        Some("test worker"),
        "description should be preserved"
    );
    assert_eq!(
        entry.meta.labels.get("mob_id").map(String::as_str),
        Some("test-mob"),
        "mob_id label should be preserved"
    );
    assert_eq!(
        entry.meta.labels.get("role").map(String::as_str),
        Some("coder"),
        "role label should be preserved"
    );
    assert_eq!(
        entry.source,
        PeerDirectorySource::Inproc,
        "source should be Inproc since peer is only visible via registry"
    );

    // Clean up global registry
    meerkat_comms::InprocRegistry::global().unregister(&peer_pubkey);
    meerkat_comms::InprocRegistry::global().unregister(&runtime.public_key());
}

// ---------------------------------------------------------------------------
// CONTRACT-MOB-001 / CONTRACT-MOB-007 session contract harness
// ---------------------------------------------------------------------------

struct ContractSessionService {
    sessions: RwLock<HashMap<SessionId, Arc<CommsRuntime>>>,
}

impl ContractSessionService {
    fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
        }
    }

    async fn comms(&self, id: &SessionId) -> Option<Arc<CommsRuntime>> {
        self.sessions.read().await.get(id).cloned()
    }
}

fn run_result(session_id: SessionId, text: &str) -> RunResult {
    RunResult {
        text: text.to_string(),
        session_id,
        usage: Usage::default(),
        turns: 1,
        tool_calls: 0,
        structured_output: None,
        schema_warnings: None,
        skill_diagnostics: None,
    }
}

#[async_trait]
impl SessionService for ContractSessionService {
    async fn create_session(&self, req: CreateSessionRequest) -> Result<RunResult, SessionError> {
        let session_id = SessionId::new();
        let comms_name = req
            .build
            .as_ref()
            .and_then(|b| b.comms_name.clone())
            .unwrap_or_else(|| format!("contract-session-{}", Uuid::new_v4().simple()));

        let comms = CommsRuntime::inproc_only(&comms_name).map_err(|e| {
            SessionError::Agent(meerkat_core::error::AgentError::InternalError(format!(
                "failed to create comms runtime: {e}"
            )))
        })?;

        self.sessions
            .write()
            .await
            .insert(session_id.clone(), Arc::new(comms));

        Ok(run_result(session_id, "session created"))
    }

    async fn start_turn(
        &self,
        id: &SessionId,
        _req: StartTurnRequest,
    ) -> Result<RunResult, SessionError> {
        let sessions = self.sessions.read().await;
        if !sessions.contains_key(id) {
            return Err(SessionError::NotFound { id: id.clone() });
        }
        Ok(run_result(id.clone(), "turn completed"))
    }

    async fn interrupt(&self, id: &SessionId) -> Result<(), SessionError> {
        let sessions = self.sessions.read().await;
        if !sessions.contains_key(id) {
            return Err(SessionError::NotFound { id: id.clone() });
        }
        Ok(())
    }

    async fn read(&self, id: &SessionId) -> Result<SessionView, SessionError> {
        let sessions = self.sessions.read().await;
        if !sessions.contains_key(id) {
            return Err(SessionError::NotFound { id: id.clone() });
        }
        Ok(SessionView {
            state: SessionInfo {
                session_id: id.clone(),
                created_at: SystemTime::now(),
                updated_at: SystemTime::now(),
                message_count: 0,
                is_active: false,
                last_assistant_text: None,
                labels: Default::default(),
            },
            billing: SessionUsage {
                total_tokens: 0,
                usage: Usage::default(),
            },
        })
    }

    async fn list(&self, _query: SessionQuery) -> Result<Vec<SessionSummary>, SessionError> {
        let sessions = self.sessions.read().await;
        Ok(sessions
            .keys()
            .map(|id| SessionSummary {
                session_id: id.clone(),
                created_at: SystemTime::now(),
                updated_at: SystemTime::now(),
                message_count: 0,
                total_tokens: 0,
                is_active: false,
                labels: Default::default(),
            })
            .collect())
    }

    async fn archive(&self, id: &SessionId) -> Result<(), SessionError> {
        let removed = self.sessions.write().await.remove(id).is_some();
        if removed {
            Ok(())
        } else {
            Err(SessionError::NotFound { id: id.clone() })
        }
    }
}

fn host_mode_req(comms_name: &str) -> CreateSessionRequest {
    CreateSessionRequest {
        model: "contract-mock".to_string(),
        prompt: "hello".to_string(),
        system_prompt: None,
        max_tokens: None,
        event_tx: None,
        host_mode: true,
        skill_references: None,
        initial_turn: meerkat_core::service::InitialTurnPolicy::RunImmediately,
        build: Some(SessionBuildOptions {
            comms_name: Some(comms_name.to_string()),
            ..Default::default()
        }),
        labels: None,
    }
}

#[tokio::test]
async fn contract_mob_001_host_mode_session_stays_alive() {
    let suffix = Uuid::new_v4().simple().to_string();
    let service = Arc::new(ContractSessionService::new());

    let a_name = format!("c001-a-{suffix}");
    let b_name = format!("c001-b-{suffix}");
    let sid_a = service
        .create_session(host_mode_req(&a_name))
        .await
        .expect("create host-mode session A")
        .session_id;
    let sid_b = service
        .create_session(host_mode_req(&b_name))
        .await
        .expect("create host-mode session B")
        .session_id;

    let comms_a = service.comms(&sid_a).await.expect("comms for A");
    let comms_b = service.comms(&sid_b).await.expect("comms for B");

    // Trust both sides so peer requests can flow.
    let a_to_b = TrustedPeerSpec::new(
        &b_name,
        comms_b.public_key().to_peer_id(),
        format!("inproc://{b_name}"),
    )
    .expect("valid trusted peer spec a->b");
    CoreCommsRuntime::add_trusted_peer(&*comms_a, a_to_b)
        .await
        .expect("trust a->b");

    let b_to_a = TrustedPeerSpec::new(
        &a_name,
        comms_a.public_key().to_peer_id(),
        format!("inproc://{a_name}"),
    )
    .expect("valid trusted peer spec b->a");
    CoreCommsRuntime::add_trusted_peer(&*comms_b, b_to_a)
        .await
        .expect("trust b->a");

    // Verify comms request before additional turns.
    let before_cmd = CommsCommand::PeerRequest {
        to: PeerName::new(b_name.clone()).expect("valid peer name"),
        intent: "mob.contract.before".to_string(),
        params: serde_json::json!({"step": "before_turn"}),
        stream: InputStreamMode::None,
    };
    let before_receipt = CoreCommsRuntime::send(&*comms_a, before_cmd)
        .await
        .expect("send before turn");
    assert!(
        matches!(before_receipt, SendReceipt::PeerRequestSent { .. }),
        "expected peer request send before turn, got: {before_receipt:?}"
    );
    let before_interactions = CoreCommsRuntime::drain_inbox_interactions(&*comms_b).await;
    assert_eq!(
        before_interactions.len(),
        1,
        "receiver should get request before additional turn"
    );

    // Run another turn on A. Host-mode session should remain alive.
    service
        .start_turn(
            &sid_a,
            StartTurnRequest {
                prompt: "follow up".to_string(),
                event_tx: None,
                host_mode: true,
                skill_references: None,
                flow_tool_overlay: None,
            },
        )
        .await
        .expect("start second turn on host-mode session");

    // Verify comms still works after extra turn.
    let after_cmd = CommsCommand::PeerRequest {
        to: PeerName::new(a_name.clone()).expect("valid peer name"),
        intent: "mob.contract.after".to_string(),
        params: serde_json::json!({"step": "after_turn"}),
        stream: InputStreamMode::None,
    };
    let after_receipt = CoreCommsRuntime::send(&*comms_b, after_cmd)
        .await
        .expect("send after turn");
    assert!(
        matches!(after_receipt, SendReceipt::PeerRequestSent { .. }),
        "expected peer request send after turn, got: {after_receipt:?}"
    );
    let after_interactions = CoreCommsRuntime::drain_inbox_interactions(&*comms_a).await;
    assert_eq!(
        after_interactions.len(),
        1,
        "sender should still receive peer request after additional turn"
    );
}

// ---------------------------------------------------------------------------
// CONTRACT-MOB-007: SessionService::archive removes from active list
// ---------------------------------------------------------------------------

#[tokio::test]
async fn contract_mob_007_session_archive_removes_from_active_list() {
    let suffix = Uuid::new_v4().simple().to_string();
    let service = Arc::new(ContractSessionService::new());
    let sid = service
        .create_session(host_mode_req(&format!("c007-{suffix}")))
        .await
        .expect("create session")
        .session_id;

    let before_list = service
        .list(SessionQuery::default())
        .await
        .expect("list before archive");
    assert!(
        before_list.iter().any(|s| s.session_id == sid),
        "session should be listed before archive"
    );

    service.archive(&sid).await.expect("archive session");

    let after_list = service
        .list(SessionQuery::default())
        .await
        .expect("list after archive");
    assert!(
        !after_list.iter().any(|s| s.session_id == sid),
        "archived session must not appear in list()"
    );

    let start_result = service
        .start_turn(
            &sid,
            StartTurnRequest {
                prompt: "should fail".to_string(),
                event_tx: None,
                host_mode: false,
                skill_references: None,
                flow_tool_overlay: None,
            },
        )
        .await;
    assert!(
        matches!(start_result, Err(SessionError::NotFound { .. })),
        "start_turn after archive should return NotFound, got: {start_result:?}"
    );
}
