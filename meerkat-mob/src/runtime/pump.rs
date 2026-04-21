//! Scheduler pump utilities.
//!
//! `pump_schedulers_to_exhaustion` applies `PumpNodeScheduler` and
//! `PumpFrameScheduler` repeatedly until neither produces a grant. This is the
//! canonical way to drain the scheduler queues after a batch of
//! `RegisterReadyFrame` or `RegisterPendingBodyFrame` calls.

use crate::error::MobError;
use crate::ids::{FrameId, LoopInstanceId};
use meerkat_machine_kernels::legacy::{KernelInput, KernelState, KernelValue};
use meerkat_machine_kernels::legacy_generated::flow_run;
use std::collections::BTreeMap;

/// A scheduler grant produced by pumping.
#[derive(Debug, Clone)]
pub enum SchedulerGrant {
    /// A ready frame was granted a node execution slot.
    NodeSlot(FrameId),
    /// A pending body frame was granted its start.
    BodyFrameStart(LoopInstanceId),
}

/// Apply `PumpNodeScheduler` and `PumpFrameScheduler` repeatedly until neither
/// emits a grant. Returns the final run kernel state and all emitted grants.
///
/// `max_pumps` limits the total number of pump iterations (node + frame pumps
/// each count as one attempt). Use a generous value such as 100 to prevent
/// infinite loops in case of a machine bug.
pub fn pump_schedulers_to_exhaustion(
    run_state: &KernelState,
    max_pumps: usize,
) -> Result<(KernelState, Vec<SchedulerGrant>), MobError> {
    let mut state = run_state.clone();
    let mut grants = Vec::new();
    let mut round = 0;

    loop {
        if round >= max_pumps {
            break;
        }
        let mut any_grant = false;

        // Try PumpNodeScheduler
        let node_pump = KernelInput {
            variant: "PumpNodeScheduler".into(),
            fields: BTreeMap::new(),
        };
        if let Ok(outcome) = flow_run::transition(&state, &node_pump) {
            let node_grants: Vec<SchedulerGrant> = outcome
                .effects
                .iter()
                .filter(|e| e.variant == "GrantNodeSlot")
                .filter_map(|e| e.fields.get("frame_id"))
                .filter_map(|v| {
                    if let KernelValue::String(fid) = v {
                        Some(SchedulerGrant::NodeSlot(FrameId::from(fid.as_str())))
                    } else {
                        None
                    }
                })
                .collect();
            if !node_grants.is_empty() {
                grants.extend(node_grants);
                state = outcome.next_state;
                any_grant = true;
            }
        }

        // Try PumpFrameScheduler
        let frame_pump = KernelInput {
            variant: "PumpFrameScheduler".into(),
            fields: BTreeMap::new(),
        };
        if let Ok(outcome) = flow_run::transition(&state, &frame_pump) {
            let frame_grants: Vec<SchedulerGrant> = outcome
                .effects
                .iter()
                .filter(|e| e.variant == "GrantBodyFrameStart")
                .filter_map(|e| e.fields.get("loop_instance_id"))
                .filter_map(|v| {
                    if let KernelValue::String(lid) = v {
                        Some(SchedulerGrant::BodyFrameStart(LoopInstanceId::from(
                            lid.as_str(),
                        )))
                    } else {
                        None
                    }
                })
                .collect();
            if !frame_grants.is_empty() {
                grants.extend(frame_grants);
                state = outcome.next_state;
                any_grant = true;
            }
        }

        round += 1;
        if !any_grant {
            break;
        }
    }

    Ok((state, grants))
}
