// @generated — protocol helpers for `auth_lease_lifecycle_publication`
// Composition: auth_lease_bundle, Producer: auth_machine, Effect: EmitLifecycleEvent
// Closure policy: AckRequired
// Liveness: informative publication: AuthMachine's own transitions carry the authoritative phase fact; runtime owner refreshes the lease-state projection under task-scheduling fairness

use crate::auth_machine::dsl::{AuthLifecyclePhase, AuthMachineEffect, AuthMachineTransition};

#[derive(Debug, Clone)]
pub struct AuthLeaseLifecyclePublicationObligation {
    new_state: AuthLifecyclePhase,
    credential_generation: u64,
    credential_published_at_millis: Option<u64>,
    transition_claimed: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl AuthLeaseLifecyclePublicationObligation {
    pub fn new_state(&self) -> &AuthLifecyclePhase {
        &self.new_state
    }

    pub fn credential_generation(&self) -> u64 {
        self.credential_generation
    }

    pub fn credential_published_at_millis(&self) -> &Option<u64> {
        &self.credential_published_at_millis
    }
}

impl AuthLeaseLifecyclePublicationObligation {
    #[allow(unsafe_code)]
    pub(crate) fn into_auth_lease_transition(
        &self,
        lease_key: meerkat_core::handles::LeaseKey,
        expires_at: u64,
    ) -> Result<meerkat_core::handles::AuthLeaseTransition, String> {
        if self
            .transition_claimed
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return Err("generated auth lease lifecycle publication was already consumed".into());
        }
        struct GeneratedAuthLeaseTransitionParts {
            lease_key: meerkat_core::handles::LeaseKey,
            expires_at: u64,
            generation: u64,
            credential_published_at_millis: Option<u64>,
        }
        // This unsafe impl is generated inside the one-use AuthMachine lifecycle
        // publication handoff after the typed obligation has been consumed.
        #[allow(unsafe_code)]
        unsafe impl meerkat_core::handles::GeneratedAuthLeaseTransitionParts
            for GeneratedAuthLeaseTransitionParts
        {
            fn lease_key(&self) -> &meerkat_core::handles::LeaseKey {
                &self.lease_key
            }
            fn expires_at(&self) -> u64 {
                self.expires_at
            }
            fn generation(&self) -> u64 {
                self.generation
            }
            fn credential_published_at_millis(&self) -> Option<u64> {
                self.credential_published_at_millis
            }
        }
        Ok(unsafe {
            meerkat_core::handles::AuthLeaseTransition::from_generated_auth_lease_publication(
                GeneratedAuthLeaseTransitionParts {
                    lease_key,
                    expires_at,
                    generation: self.credential_generation,
                    credential_published_at_millis: self.credential_published_at_millis,
                },
            )
        })
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
                credential_published_at_millis: credential_published_at_millis.clone(),
                transition_claimed: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            }),
            _ => None,
        })
        .collect()
}
