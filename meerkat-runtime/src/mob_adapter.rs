//! MobRuntimeAdapter — bridges mob provisioning to v9 RuntimeDriver lifecycle.
//!
//! When a mob member is spawned, the adapter registers a RuntimeDriver for that
//! session. When retired, the adapter retires/unregisters the driver. Flow steps
//! are delivered as FlowStepInput through accept_input().
//!
//! This adapter is optional — mob works without it (existing SessionService path).
//! When present, it enables v9 input lifecycle tracking for mob members.

use meerkat_core::lifecycle::InputId;
use meerkat_core::types::ContentInput;
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
    instructions: ContentInput,
    flow_id: &str,
    step_index: usize,
    turn_metadata: Option<meerkat_core::lifecycle::run_primitive::RuntimeTurnMetadata>,
) -> Input {
    let instructions_text = instructions.text_content();
    let blocks = if instructions.has_images() {
        Some(instructions.into_blocks())
    } else {
        None
    };
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
        instructions: instructions_text,
        blocks,
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
    instructions: impl Into<ContentInput>,
    flow_id: &str,
    step_index: usize,
) -> Result<crate::AcceptOutcome, RuntimeDriverError> {
    let input = create_flow_step_input(step_id, instructions.into(), flow_id, step_index, None);
    adapter.accept_input(session_id, input).await
}

/// Retire a mob member's runtime.
///
/// If the session is attached to a live `RuntimeLoop`, queued inputs remain
/// pending for drain. For plain registered sessions without a loop, retirement
/// abandons queued work because nothing can execute the drain path.
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
        let input = create_flow_step_input("s", "i".into(), "f", 0, None);
        let policy = DefaultPolicyTable::resolve(&input, true);
        assert_eq!(policy.apply_mode, crate::ApplyMode::StageRunStart);
        assert_eq!(policy.wake_mode, crate::WakeMode::WakeIfIdle);
    }

    #[tokio::test]
    async fn retire_without_runtime_loop_abandons_pending_inputs() {
        let adapter = RuntimeSessionAdapter::ephemeral();
        let sid = SessionId::new();
        register_mob_member(&adapter, sid.clone()).await;

        // Accept an input first
        deliver_flow_step(&adapter, &sid, "s1", "do it", "f1", 0)
            .await
            .unwrap();

        // No RuntimeLoop is attached for plain registration, so retirement
        // abandons queued work instead of leaving it pending forever.
        let report = retire_mob_member(&adapter, &sid).await.unwrap();
        assert_eq!(report.inputs_abandoned, 1);
        assert_eq!(report.inputs_pending_drain, 0);
    }

    #[tokio::test]
    async fn create_flow_step_input_preserves_multimodal_blocks() -> Result<(), String> {
        let input = create_flow_step_input(
            "s",
            ContentInput::Blocks(vec![
                meerkat_core::types::ContentBlock::Text {
                    text: "inspect image".into(),
                },
                meerkat_core::types::ContentBlock::Image {
                    media_type: "image/png".into(),
                    data: "abc123".into(),
                    source_path: None,
                },
            ]),
            "f",
            0,
            None,
        );

        let flow_step = match input {
            Input::FlowStep(flow_step) => flow_step,
            other => return Err(format!("expected flow step input, got {other:?}")),
        };
        assert_eq!(flow_step.instructions, "inspect image\n[image: image/png]");
        assert_eq!(flow_step.blocks.as_ref().map(Vec::len), Some(2));
        Ok(())
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
