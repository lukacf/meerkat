#![allow(clippy::expect_used, clippy::unwrap_used)]

use chrono::Utc;
use meerkat_core::lifecycle::{InputId, RunId};
use meerkat_core::types::HandlingMode;
use meerkat_runtime::driver::ephemeral::{EphemeralRuntimeDriver, PostAdmissionSignal};
use meerkat_runtime::identifiers::LogicalRuntimeId;
use meerkat_runtime::input::{
    Input, InputDurability, InputHeader, InputOrigin, InputVisibility, PeerConvention, PeerInput,
};
use meerkat_runtime::policy_table::DefaultPolicyTable;
use meerkat_runtime::traits::RuntimeDriver;
use meerkat_runtime::{ApplyMode, RoutingDisposition, WakeMode};

fn runtime_id() -> LogicalRuntimeId {
    LogicalRuntimeId::new("peer-handling-mode-contract")
}

fn peer_message_input(handling_mode: Option<HandlingMode>) -> Input {
    Input::Peer(PeerInput {
        header: InputHeader {
            id: InputId::new(),
            timestamp: Utc::now(),
            source: InputOrigin::Peer {
                peer_id: "peer-1".into(),
                runtime_id: None,
            },
            durability: InputDurability::Durable,
            visibility: InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        convention: Some(PeerConvention::Message),
        body: "hello".into(),
        blocks: None,
        handling_mode,
    })
}

#[tokio::test]
async fn running_queue_peer_message_stays_passive() {
    let mut driver = EphemeralRuntimeDriver::new(runtime_id());
    driver.start_run(RunId::new()).expect("start run");

    let input = peer_message_input(None);
    let policy = DefaultPolicyTable::resolve(&input, false);
    assert_eq!(policy.apply_mode, ApplyMode::StageRunStart);
    assert_eq!(policy.wake_mode, WakeMode::None);
    assert_eq!(policy.routing_disposition, RoutingDisposition::Queue);

    let outcome = driver.accept_input(input).await.expect("accept input");
    assert!(outcome.is_accepted());
    assert_eq!(
        driver.take_post_admission_signal(),
        PostAdmissionSignal::None
    );
    assert!(!driver.take_wake_requested());
}

#[tokio::test]
async fn running_steer_peer_message_requests_immediate_processing() {
    let mut driver = EphemeralRuntimeDriver::new(runtime_id());
    driver.start_run(RunId::new()).expect("start run");

    let input = peer_message_input(Some(HandlingMode::Steer));
    let policy = DefaultPolicyTable::resolve(&input, false);
    assert_eq!(policy.apply_mode, ApplyMode::StageRunBoundary);
    assert_eq!(policy.wake_mode, WakeMode::None);
    assert_eq!(policy.routing_disposition, RoutingDisposition::Steer);

    let outcome = driver.accept_input(input).await.expect("accept input");
    assert!(outcome.is_accepted());
    let signal = driver.take_post_admission_signal();
    assert_eq!(signal, PostAdmissionSignal::RequestImmediateProcessing);
    assert!(signal.should_wake());
}
