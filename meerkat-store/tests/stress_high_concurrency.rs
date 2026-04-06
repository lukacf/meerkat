#![allow(clippy::expect_used, clippy::unwrap_used)]

use meerkat_core::{Message, Session, UserMessage};
use meerkat_store::SessionFilter;
use meerkat_store::SessionStore;
use meerkat_store::jsonl::JsonlStore;
use std::sync::Arc;
use std::time::{Duration, Instant};

struct PreparedStore {
    _temp_dir: tempfile::TempDir,
    store: Arc<JsonlStore>,
}

fn p95(samples: &mut [Duration]) -> Duration {
    samples.sort_unstable();
    let idx = samples.len().saturating_sub(1) * 95 / 100;
    samples[idx]
}

async fn prepare_store(session_count: usize) -> PreparedStore {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(JsonlStore::new(temp_dir.path().to_path_buf()));
    store.init().await.expect("init store dir");

    for i in 0..session_count {
        let mut session = Session::new();
        session.push(Message::User(UserMessage::text(format!("session-{i}"))));
        store.save(&session).await.expect("save session");
    }

    // Warm the index cache inside JsonlStore.
    let _ = store
        .list(SessionFilter {
            limit: Some(50),
            ..Default::default()
        })
        .await
        .expect("warm list");

    PreparedStore {
        _temp_dir: temp_dir,
        store,
    }
}

async fn sample_list_latencies(
    store: Arc<JsonlStore>,
    rounds: usize,
    concurrency: usize,
) -> Vec<Duration> {
    let mut samples = Vec::with_capacity(rounds * concurrency);

    for _ in 0..rounds {
        let mut handles = Vec::with_capacity(concurrency);
        for _ in 0..concurrency {
            let store = Arc::clone(&store);
            handles.push(tokio::spawn(async move {
                let start = Instant::now();
                let result = tokio::time::timeout(
                    Duration::from_secs(5),
                    store.list(SessionFilter {
                        limit: Some(50),
                        ..Default::default()
                    }),
                )
                .await
                .map_err(|_| "list timeout")?
                .map_err(|_| "list error")?;

                if result.len() != 50 {
                    return Err("unexpected result size");
                }

                Ok::<_, &'static str>(start.elapsed())
            }));
        }

        for handle in handles {
            samples.push(handle.await.expect("task join").expect("task result"));
        }
    }

    samples
}

#[tokio::test]
#[ignore = "Stress test"]
async fn stress_session_listing_p95_is_stable_across_scale() {
    let rounds = 10;
    let concurrency = 50;

    let store_1k = prepare_store(1_000).await;
    let store_10k = prepare_store(10_000).await;

    let mut samples_1k =
        sample_list_latencies(Arc::clone(&store_1k.store), rounds, concurrency).await;
    let mut samples_10k =
        sample_list_latencies(Arc::clone(&store_10k.store), rounds, concurrency).await;

    let p95_1k = p95(&mut samples_1k);
    let p95_10k = p95(&mut samples_10k);

    // Contract: listing the first page should not degrade meaningfully as the store grows.
    // This catches regressions where listing becomes O(N) over all sessions.
    assert!(
        p95_10k <= p95_1k + p95_1k,
        "p95 list time regressed: p95_1k={p95_1k:?}, p95_10k={p95_10k:?}"
    );
}
