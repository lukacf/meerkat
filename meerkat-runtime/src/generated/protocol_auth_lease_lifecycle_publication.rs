// @generated — protocol helpers for `auth_lease_lifecycle_publication`
// Composition: auth_lease_bundle, Producer: auth_machine, Effect: EmitLifecycleEvent
// Closure policy: AckRequired
// Liveness: informative publication: AuthMachine's own transitions carry the authoritative phase fact; runtime owner refreshes the lease-state projection under task-scheduling fairness

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use crate::auth_machine::dsl::{AuthLifecyclePhase, AuthMachineEffect, AuthMachineTransition};

#[derive(Debug, Clone)]
pub struct AuthLeaseLifecyclePublicationObligation {
    new_state: AuthLifecyclePhase,
    credential_generation: u64,
    credential_published_at_millis: Option<u64>,
    transition_claimed: Arc<AtomicBool>,
}

impl AuthLeaseLifecyclePublicationObligation {
    pub fn new_state(&self) -> AuthLifecyclePhase {
        self.new_state.clone()
    }

    pub fn credential_generation(&self) -> u64 {
        self.credential_generation
    }

    pub fn credential_published_at_millis(&self) -> Option<u64> {
        self.credential_published_at_millis
    }

    pub fn into_auth_lease_transition(
        &self,
    ) -> Result<meerkat_core::handles::AuthLeaseTransition, String> {
        meerkat_core::handles::AuthLeaseTransition::from_generated_auth_lease_publication(
            self,
            self.credential_generation,
            self.credential_published_at_millis,
        )
    }
}

impl meerkat_core::handles::generated_auth_lease_transition_authority::Sealed
    for AuthLeaseLifecyclePublicationObligation
{
}

impl meerkat_core::handles::GeneratedAuthLeaseTransitionSource
    for AuthLeaseLifecyclePublicationObligation
{
    fn auth_lease_transition_source_kind(
        &self,
    ) -> meerkat_core::handles::GeneratedAuthLeaseTransitionSourceKind {
        meerkat_core::handles::GeneratedAuthLeaseTransitionSourceKind::AuthMachineLifecyclePublication
    }

    fn authorize_auth_lease_transition(
        &self,
        request: &meerkat_core::handles::GeneratedAuthLeaseTransitionRequest,
    ) -> Result<meerkat_core::handles::GeneratedAuthLeaseTransitionGrant, String> {
        if request.generation() != self.credential_generation {
            return Err(format!(
                "generated auth lease lifecycle publication generation {} does not match requested {}",
                self.credential_generation,
                request.generation()
            ));
        }
        if request.credential_published_at_millis() != self.credential_published_at_millis {
            return Err(format!(
                "generated auth lease lifecycle publication time {:?} does not match requested {:?}",
                self.credential_published_at_millis,
                request.credential_published_at_millis()
            ));
        }
        if self.transition_claimed.swap(true, Ordering::SeqCst) {
            return Err("generated auth lease lifecycle publication was already consumed".into());
        }
        Ok(
            meerkat_core::handles::GeneratedAuthLeaseTransitionGrant::new(
                request,
                self.auth_lease_transition_source_kind(),
            ),
        )
    }
}

pub fn extract_obligations(
    transition: &AuthMachineTransition,
) -> Vec<AuthLeaseLifecyclePublicationObligation> {
    transition
        .effects()
        .iter()
        .filter_map(|effect| match effect {
            AuthMachineEffect::EmitLifecycleEvent {
                new_state,
                credential_generation,
                credential_published_at_millis,
            } => Some(AuthLeaseLifecyclePublicationObligation {
                new_state: new_state.clone(),
                credential_generation: *credential_generation,
                credential_published_at_millis: *credential_published_at_millis,
                transition_claimed: Arc::new(AtomicBool::new(false)),
            }),
            _ => None,
        })
        .collect()
}
