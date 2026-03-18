use std::sync::Arc;

use meerkat_mob::runtime::FlowRunKernel;
use meerkat_mob::{
    FlowId, InMemoryMobEventStore, InMemoryMobRunStore, MobEventKind, MobEventStore, MobId,
    MobRunStatus, MobRunStore,
};

#[tokio::test]
async fn flow_run_kernel_persists_pending_and_terminal_truth_for_machine_verify() {
    let run_store = Arc::new(InMemoryMobRunStore::new());
    let events = Arc::new(InMemoryMobEventStore::new());
    let kernel = FlowRunKernel::new(
        MobId::from("mob-owner-test"),
        run_store.clone(),
        events.clone(),
    );

    let run_id = kernel
        .create_pending_run(
            FlowId::from("demo"),
            serde_json::json!({"entry":"owner-test"}),
        )
        .await
        .expect("create pending run");
    let pending = run_store
        .get_run(&run_id)
        .await
        .expect("load pending run")
        .expect("pending run should exist");
    assert_eq!(pending.status, MobRunStatus::Pending);

    let _ = kernel
        .terminalize_completed(run_id.clone(), FlowId::from("demo"))
        .await
        .expect("terminalize completed");

    let completed = run_store
        .get_run(&run_id)
        .await
        .expect("load completed run")
        .expect("completed run should exist");
    assert_eq!(completed.status, MobRunStatus::Completed);

    let replay = run_store
        .list_runs(&MobId::from("mob-owner-test"), None)
        .await
        .expect("list runs");
    assert_eq!(
        replay.len(),
        1,
        "owner test should observe one durable run record"
    );

    let events = events.replay_all().await.expect("replay events");
    assert!(events.iter().any(|event| matches!(
        &event.kind,
        MobEventKind::FlowCompleted { run_id: event_run_id, .. } if event_run_id == &run_id
    )));
}
