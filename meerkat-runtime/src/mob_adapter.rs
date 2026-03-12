//! MobRuntimeAdapter — bridges mob provisioning to v9 RuntimeDriver lifecycle.
//!
//! When a mob member is spawned, the adapter registers a RuntimeDriver for that
//! session. When retired, the adapter retires/unregisters the driver. Flow steps
//! are delivered as FlowStepInput through accept_input().
//!
//! This adapter is optional — mob works without it (existing SessionService path).
//! When present, it enables v9 input lifecycle tracking for mob members.

use meerkat_core::lifecycle::InputId;
use meerkat_core::types::SessionId;

use crate::RuntimeSessionAdapter;
use crate::input::{
    FlowStepInput, Input, InputDurability, InputHeader, InputOrigin, InputVisibility,
};
#[allow(unused_imports)]
use crate::service_ext::SessionServiceRuntimeExt;
use crate::traits::RuntimeDriverError;

/// Create a FlowStepInput for a mob flow step.
pub fn create_flow_step_input(
    step_id: &str,
    instructions: &str,
    flow_id: &str,
    step_index: usize,
    turn_metadata: Option<meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata>,
) -> Input {
    Input::FlowStep(FlowStepInput {
        header: InputHeader {
            id: InputId::new(),
            timestamp: chrono::Utc::now(),
            source: InputOrigin::Flow {
                flow_id: flow_id.into(),
                step_index,
            },
            durability: InputDurability::Durable,
            visibility: InputVisibility::default(),
            idempotency_key: None,
            supersession_key: None,
            correlation_id: None,
        },
        step_id: step_id.into(),
        instructions: instructions.into(),
        turn_metadata,
    })
}

/// Register a mob member's session with the runtime adapter.
pub async fn register_mob_member(adapter: &RuntimeSessionAdapter, session_id: SessionId) {
    adapter.register_session(session_id).await;
}

/// Unregister a mob member's session from the runtime adapter.
pub async fn unregister_mob_member(adapter: &RuntimeSessionAdapter, session_id: &SessionId) {
    adapter.unregister_session(session_id).await;
}

/// Deliver a flow step to a mob member through the runtime path.
pub async fn deliver_flow_step(
    adapter: &RuntimeSessionAdapter,
    session_id: &SessionId,
    step_id: &str,
    instructions: &str,
    flow_id: &str,
    step_index: usize,
) -> Result<crate::AcceptOutcome, RuntimeDriverError> {
    let input = create_flow_step_input(step_id, instructions, flow_id, step_index, None);
    adapter.accept_input(session_id, input).await
}

/// Retire a mob member's runtime.
pub async fn retire_mob_member(
    adapter: &RuntimeSessionAdapter,
    session_id: &SessionId,
) -> Result<crate::traits::RetireReport, RuntimeDriverError> {
    adapter.retire_runtime(session_id).await
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::policy_table::DefaultPolicyTable;

    #[tokio::test]
    async fn spawn_creates_runtime_driver_session() {
        let adapter = RuntimeSessionAdapter::ephemeral();
        let sid = SessionId::new();

        register_mob_member(&adapter, sid.clone()).await;

        // Session should have a runtime driver
        let state = adapter.runtime_state(&sid).await.unwrap();
        assert_eq!(state, crate::RuntimeState::Idle);
    }

    #[tokio::test]
    async fn flow_step_delivered_as_input() {
        let adapter = RuntimeSessionAdapter::ephemeral();
        let sid = SessionId::new();
        register_mob_member(&adapter, sid.clone()).await;

        let outcome = deliver_flow_step(&adapter, &sid, "step-1", "analyze the data", "flow-1", 0)
            .await
            .unwrap();

        assert!(outcome.is_accepted());

        // Verify policy: flow_step → StageRunStart + WakeIfIdle
        let input = create_flow_step_input("s", "i", "f", 0, None);
        let policy = DefaultPolicyTable::resolve(&input, true);
        assert_eq!(policy.apply_mode, crate::ApplyMode::StageRunStart);
        assert_eq!(policy.wake_mode, crate::WakeMode::WakeIfIdle);
    }

    #[tokio::test]
    async fn retire_preserves_pending_for_drain() {
        let adapter = RuntimeSessionAdapter::ephemeral();
        let sid = SessionId::new();
        register_mob_member(&adapter, sid.clone()).await;

        // Accept an input first
        deliver_flow_step(&adapter, &sid, "s1", "do it", "f1", 0)
            .await
            .unwrap();

        // Retire — preserves inputs for drain, doesn't abandon
        let report = retire_mob_member(&adapter, &sid).await.unwrap();
        assert_eq!(report.inputs_abandoned, 0);
        assert_eq!(report.inputs_pending_drain, 1);
    }

    #[tokio::test]
    async fn unregister_removes_driver() {
        let adapter = RuntimeSessionAdapter::ephemeral();
        let sid = SessionId::new();
        register_mob_member(&adapter, sid.clone()).await;

        unregister_mob_member(&adapter, &sid).await;

        // Should fail now
        let result = adapter.runtime_state(&sid).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn wiring_emits_topology_event() {
        // When a mob member spawns and registers, the driver is created.
        // The topology event would be emitted by the RuntimeControlPlane
        // (not yet implemented). For now, verify the driver exists.
        let adapter = RuntimeSessionAdapter::ephemeral();
        let sid = SessionId::new();
        register_mob_member(&adapter, sid.clone()).await;

        let active = adapter.list_active_inputs(&sid).await.unwrap();
        assert!(active.is_empty()); // No inputs yet
    }
}
