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
