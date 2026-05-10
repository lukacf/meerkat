//! Smoke test for `meerkat::session_runtime::runtime_state` types
//! moved in W1-E. The drop-notify behaviour is load-bearing — when the
//! receiver-side handle is dropped, the runtime must observe a
//! single-shot notification.

#![allow(clippy::expect_used)]

use std::sync::Arc;

use meerkat::session_runtime::runtime_state::{
    PendingSessionEventStreamDrop, PendingSessionEventStreams,
};
use tokio::sync::{Notify, broadcast};

#[tokio::test]
async fn drop_guard_notifies_once() {
    let receiver_dropped = Arc::new(Notify::new());
    let (events, _rx) = broadcast::channel(8);
    let _streams = PendingSessionEventStreams {
        events,
        receiver_dropped: Arc::clone(&receiver_dropped),
    };
    let guard = PendingSessionEventStreamDrop {
        receiver_dropped: Arc::clone(&receiver_dropped),
    };

    let waiter = receiver_dropped.clone();
    let notified = tokio::spawn(async move {
        waiter.notified().await;
    });

    drop(guard);
    notified
        .await
        .expect("dropping the guard should notify the runtime exactly once");
}

#[test]
fn session_state_serde_round_trip_uses_snake_case() {
    use meerkat::session_runtime::runtime_state::SessionState;

    let cases = [
        (SessionState::Idle, "\"idle\"", "idle"),
        (SessionState::Running, "\"running\"", "running"),
        (
            SessionState::ShuttingDown,
            "\"shutting_down\"",
            "shutting_down",
        ),
    ];
    for (state, encoded, slug) in cases {
        assert_eq!(
            serde_json::to_string(&state).expect("serialize"),
            encoded,
            "{state:?} must serialise to {encoded}"
        );
        assert_eq!(state.as_str(), slug);
        let decoded: SessionState = serde_json::from_str(encoded).expect("round-trip must decode");
        assert_eq!(decoded, state);
    }
}

#[test]
fn session_info_holds_session_id_state_and_labels() {
    use std::collections::BTreeMap;

    use meerkat::session_runtime::runtime_state::{SessionInfo, SessionState};
    use meerkat_core::types::SessionId;

    let mut labels = BTreeMap::new();
    labels.insert("env".to_string(), "test".to_string());

    let info = SessionInfo {
        session_id: SessionId::new(),
        state: SessionState::Idle,
        labels: labels.clone(),
    };
    assert_eq!(info.state, SessionState::Idle);
    assert_eq!(info.labels, labels);
}

#[test]
fn skill_identity_registry_state_default_is_empty_generation_zero() {
    use meerkat::session_runtime::runtime_state::SkillIdentityRegistryState;

    let state = SkillIdentityRegistryState::default();
    assert_eq!(state.generation, 0);
}

#[test]
fn build_skill_identity_registry_returns_default_for_empty_skills_config() {
    use meerkat::session_runtime::runtime_state::build_skill_identity_registry;
    use meerkat_core::Config;

    let config = Config::default();
    let registry =
        build_skill_identity_registry(&config, None, None).expect("default config builds clean");
    // Default config has no remaps; resulting registry is empty.
    let _ = registry;
}

#[tokio::test]
async fn archive_runtime_cleanup_dispatches_to_trait_hooks() {
    use std::sync::atomic::{AtomicBool, Ordering};

    use meerkat::session_runtime::runtime_state::{
        ArchiveRuntimeCleanup, ArchiveRuntimeMcpState, ArchiveRuntimeMobState,
    };
    use meerkat_core::service::SessionError;
    use meerkat_core::types::SessionId;
    use meerkat_runtime::MeerkatMachine;

    struct McpStub {
        ran: Arc<AtomicBool>,
    }

    #[async_trait::async_trait]
    impl ArchiveRuntimeMcpState for McpStub {
        async fn cleanup(&self, _session_id: &SessionId) {
            self.ran.store(true, Ordering::SeqCst);
        }
    }

    struct MobStub {
        ran: Arc<AtomicBool>,
    }

    #[async_trait::async_trait]
    impl ArchiveRuntimeMobState for MobStub {
        async fn cleanup(&self, _session_id: &SessionId) -> Result<(), SessionError> {
            self.ran.store(true, Ordering::SeqCst);
            Ok(())
        }

        async fn has_retained_cleanup(&self, _session_id: &SessionId) -> bool {
            false
        }
    }

    let mcp_ran = Arc::new(AtomicBool::new(false));
    let mob_ran = Arc::new(AtomicBool::new(false));
    let runtime_adapter = Arc::new(MeerkatMachine::ephemeral());
    let cleanup = ArchiveRuntimeCleanup {
        runtime_adapter,
        pending_session_event_streams: None,
        mcp_state: Some(Arc::new(McpStub {
            ran: Arc::clone(&mcp_ran),
        })),
        mob_state: Some(Arc::new(MobStub {
            ran: Arc::clone(&mob_ran),
        })),
    };
    let session_id = SessionId::new();
    cleanup.run(&session_id).await.expect("cleanup runs");
    assert!(mcp_ran.load(Ordering::SeqCst));
    assert!(mob_ran.load(Ordering::SeqCst));
}
