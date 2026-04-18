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
use meerkat_runtime::{
    ApplyMode, RoutingDisposition, WakeMode, post_admission_signal_from_accept_outcome,
};

fn runtime_id() -> LogicalRuntimeId {
    LogicalRuntimeId::new("peer-handling-mode-contract")
}

fn bind_running(driver: &mut EphemeralRuntimeDriver) {
    driver.contract_set_control_projection(
        meerkat_runtime::RuntimeState::Running,
        Some(RunId::new()),
        Some(meerkat_runtime::RuntimeState::Idle),
    );
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
        payload: None,
        blocks: None,
        handling_mode,
    })
}

#[tokio::test]
async fn running_queue_peer_message_interrupts_yielding() {
    let mut driver = EphemeralRuntimeDriver::new(runtime_id());
    bind_running(&mut driver);

    let input = peer_message_input(None);
    let policy = DefaultPolicyTable::resolve(&input, false);
    assert_eq!(policy.apply_mode, ApplyMode::StageRunStart);
    assert_eq!(policy.wake_mode, WakeMode::InterruptYielding);
    assert_eq!(policy.routing_disposition, RoutingDisposition::Queue);

    let outcome = driver.accept_input(input).await.expect("accept input");
    assert!(outcome.is_accepted());
    assert_eq!(
        post_admission_signal_from_accept_outcome(&outcome, false),
        PostAdmissionSignal::InterruptYielding
    );
    assert_eq!(
        driver.take_post_admission_signal(),
        PostAdmissionSignal::None
    );
}

#[tokio::test]
async fn running_steer_peer_message_requests_immediate_processing() {
    let mut driver = EphemeralRuntimeDriver::new(runtime_id());
    bind_running(&mut driver);

    let input = peer_message_input(Some(HandlingMode::Steer));
    let policy = DefaultPolicyTable::resolve(&input, false);
    assert_eq!(policy.apply_mode, ApplyMode::StageRunBoundary);
    assert_eq!(policy.wake_mode, WakeMode::InterruptYielding);
    assert_eq!(policy.routing_disposition, RoutingDisposition::Steer);

    let outcome = driver.accept_input(input).await.expect("accept input");
    assert!(outcome.is_accepted());
    let signal = post_admission_signal_from_accept_outcome(&outcome, true);
    assert_eq!(signal, PostAdmissionSignal::RequestImmediateProcessing);
    assert!(signal.should_wake());
    assert_eq!(
        driver.take_post_admission_signal(),
        PostAdmissionSignal::None
    );
}
