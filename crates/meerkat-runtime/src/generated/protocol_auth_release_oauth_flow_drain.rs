// @generated — protocol helpers for `auth_release_oauth_flow_drain`
// Composition: auth_lease_bundle, Producer: auth_machine, Effect: CancelOAuthFlowsForRelease
// Closure policy: AckRequired
// Liveness: eventual feedback: release_lease discharges the drain by firing a terminal Expire* feedback per drained flow id before committing `Release`; the Release guard `oauth_release_drained` is the machine-side completion witness

use crate::auth_machine::dsl::{
    AuthMachineAuthority, AuthMachineEffect, AuthMachineInput, AuthMachineMutator,
    AuthMachineTransition, AuthMachineTransitionError,
};

#[derive(Debug, Clone)]
pub struct AuthReleaseOauthFlowDrainObligation {
    pub browser_flow_ids: std::collections::BTreeSet<String>,
    pub device_flow_ids: std::collections::BTreeSet<String>,
}

#[macro_export]
macro_rules! auth_release_oauth_flow_drain_feedback_input_patterns {
    () => {
        $crate::auth_machine::AuthMachineInput::ExpireOAuthBrowserFlow { .. }
            | $crate::auth_machine::AuthMachineInput::ExpireOAuthDeviceFlow { .. }
    };
}

pub fn extract_obligations(
    transition: &AuthMachineTransition,
) -> Vec<AuthReleaseOauthFlowDrainObligation> {
    transition
        .effects()
        .iter()
        .filter_map(|effect| match effect {
            AuthMachineEffect::CancelOAuthFlowsForRelease {
                browser_flow_ids,
                device_flow_ids,
            } => Some(AuthReleaseOauthFlowDrainObligation {
                browser_flow_ids: browser_flow_ids.clone(),
                device_flow_ids: device_flow_ids.clone(),
            }),
            _ => None,
        })
        .collect()
}

/// Owner feedback naming an obligation member, rejected before it reaches the
/// machine (the member is not in the obligation) or by the machine itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObligationMemberFeedbackError<E> {
    NotObligationMember { field: &'static str },
    Transition(E),
}

pub fn submit_expire_o_auth_browser_flow(
    authority: &mut AuthMachineAuthority,
    obligation: AuthReleaseOauthFlowDrainObligation,
    flow_id: String,
) -> Result<AuthMachineTransition, ObligationMemberFeedbackError<AuthMachineTransitionError>> {
    if !obligation.browser_flow_ids.contains(&flow_id) {
        return Err(ObligationMemberFeedbackError::NotObligationMember {
            field: "browser_flow_ids",
        });
    }
    let transition = authority
        .apply(AuthMachineInput::ExpireOAuthBrowserFlow { flow_id })
        .map_err(ObligationMemberFeedbackError::Transition)?;
    Ok(transition)
}

pub fn submit_expire_o_auth_device_flow(
    authority: &mut AuthMachineAuthority,
    obligation: AuthReleaseOauthFlowDrainObligation,
    flow_id: String,
) -> Result<AuthMachineTransition, ObligationMemberFeedbackError<AuthMachineTransitionError>> {
    if !obligation.device_flow_ids.contains(&flow_id) {
        return Err(ObligationMemberFeedbackError::NotObligationMember {
            field: "device_flow_ids",
        });
    }
    let transition = authority
        .apply(AuthMachineInput::ExpireOAuthDeviceFlow { flow_id })
        .map_err(ObligationMemberFeedbackError::Transition)?;
    Ok(transition)
}
