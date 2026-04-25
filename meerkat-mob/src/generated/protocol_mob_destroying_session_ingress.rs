// @generated — protocol helpers for `mob_destroying_session_ingress`
// Composition: meerkat_mob_seam, Producer: mob, Effect: RequestSessionIngressDetachForMobDestroy
// Closure policy: AckRequired
// Liveness: eventual feedback: the mob destroy path awaits each session's DetachIngress ack before requesting runtime destroy

use crate::machines::mob_machine::{
    AgentRuntimeId, MobId, MobMachineAuthority, MobMachineInput, MobMachineMutator,
    MobMachineTransition, MobMachineTransitionError,
};
use crate::runtime::actor::MobDestroySessionIngressBridgeEffect;

#[derive(Debug, Clone)]
pub struct MobDestroyingSessionIngressObligation {
    pub mob_id: MobId,
    pub agent_runtime_id: AgentRuntimeId,
}

pub fn extract_obligations(
    effects: &[MobDestroySessionIngressBridgeEffect],
) -> Vec<MobDestroyingSessionIngressObligation> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            MobDestroySessionIngressBridgeEffect::RequestSessionIngressDetachForMobDestroy {
                mob_id,
                agent_runtime_id,
            } => Some(MobDestroyingSessionIngressObligation {
                mob_id: mob_id.clone(),
                agent_runtime_id: agent_runtime_id.clone(),
            }),
            _ => None,
        })
        .collect()
}

pub fn submit_session_ingress_detached_for_mob_destroy(
    authority: &mut MobMachineAuthority,
    obligation: MobDestroyingSessionIngressObligation,
) -> Result<MobMachineTransition, MobMachineTransitionError> {
    let transition = authority.apply(MobMachineInput::SessionIngressDetachedForMobDestroy {
        mob_id: obligation.mob_id,
        agent_runtime_id: obligation.agent_runtime_id,
    })?;
    Ok(transition)
}

pub fn submit_session_ingress_detach_failed_for_mob_destroy(
    authority: &mut MobMachineAuthority,
    obligation: MobDestroyingSessionIngressObligation,
    reason: String,
) -> Result<MobMachineTransition, MobMachineTransitionError> {
    let transition = authority.apply(MobMachineInput::SessionIngressDetachFailedForMobDestroy {
        mob_id: obligation.mob_id,
        agent_runtime_id: obligation.agent_runtime_id,
        reason,
    })?;
    Ok(transition)
}
