//! Smoke test for `meerkat::session_runtime::runtime_state` types
//! moved in W1-E. The drop-notify behaviour is load-bearing — when the
//! receiver-side handle is dropped, the runtime must observe a
//! single-shot notification.

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
