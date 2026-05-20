// @generated — protocol helpers for `auth_lease_lifecycle_publication`
// Composition: auth_lease_bundle, Producer: auth_machine, Effect: EmitLifecycleEvent
// Closure policy: AckRequired
// Liveness: informative publication: AuthMachine's own transitions carry the authoritative phase fact; runtime owner refreshes the lease-state projection under task-scheduling fairness

use crate::auth_machine::dsl::{AuthLifecyclePhase, AuthMachineEffect, AuthMachineTransition};

struct GeneratedAuthorityBridgeToken;

static GENERATED_AUTHORITY_BRIDGE_TOKEN: GeneratedAuthorityBridgeToken =
    GeneratedAuthorityBridgeToken;

fn generated_authority_bridge_token() -> &'static (dyn std::any::Any + Send + Sync) {
    &GENERATED_AUTHORITY_BRIDGE_TOKEN
}

#[doc(hidden)]
#[allow(improper_ctypes_definitions, unsafe_code)]
#[unsafe(export_name = concat!("__meerkat_runtime_generated_authority_bridge_token_is_valid_v1_auth_lease_lifecycle_publication_", env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX")))]
pub extern "Rust" fn generated_authority_bridge_token_is_valid(
    token: &(dyn std::any::Any + Send + Sync),
) -> bool {
    token.is::<GeneratedAuthorityBridgeToken>()
}

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
        #[allow(improper_ctypes_definitions, unsafe_code)]
        unsafe extern "Rust" {
            #[link_name = concat!("__meerkat_core_runtime_generated_auth_lease_transition_build_v1_", env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX"))]
            fn core_runtime_generated_auth_lease_transition_build(
                token: &'static (dyn std::any::Any + Send + Sync),
                lease_key: meerkat_core::handles::LeaseKey,
                expires_at: u64,
                generation: u64,
                credential_published_at_millis: Option<u64>,
            ) -> Result<meerkat_core::handles::AuthLeaseTransition, String>;
        }
        #[allow(unsafe_code)]
        unsafe {
            core_runtime_generated_auth_lease_transition_build(
                generated_authority_bridge_token(),
                lease_key,
                expires_at,
                self.credential_generation,
                self.credential_published_at_millis,
            )
        }
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
