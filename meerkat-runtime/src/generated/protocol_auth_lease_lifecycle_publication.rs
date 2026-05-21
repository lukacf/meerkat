// @generated — protocol helpers for `auth_lease_lifecycle_publication`
// Composition: auth_lease_bundle, Producer: auth_machine, Effect: EmitLifecycleEvent
// Closure policy: PublicationOnly
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
    expires_at: Option<u64>,
    credential_generation: u64,
    credential_published_at_millis: Option<u64>,
    transition_claimed: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl AuthLeaseLifecyclePublicationObligation {
    pub fn new_state(&self) -> &AuthLifecyclePhase {
        &self.new_state
    }

    pub fn expires_at(&self) -> &Option<u64> {
        &self.expires_at
    }

    pub fn credential_generation(&self) -> u64 {
        self.credential_generation
    }

    pub fn credential_published_at_millis(&self) -> &Option<u64> {
        &self.credential_published_at_millis
    }
}

pub(crate) struct AuthLeaseLifecyclePublicationScope {
    lease_key: meerkat_core::handles::LeaseKey,
    new_state: AuthLifecyclePhase,
    expires_at: Option<u64>,
    credential_generation: u64,
    credential_published_at_millis: Option<u64>,
}

impl AuthLeaseLifecyclePublicationScope {
    pub(crate) fn from_authority(
        lease_key: meerkat_core::handles::LeaseKey,
        authority: &crate::auth_machine::dsl::AuthMachineAuthority,
    ) -> Self {
        let state = authority.state();
        Self {
            lease_key,
            new_state: state.lifecycle_phase,
            expires_at: state.expires_at,
            credential_generation: state.credential_generation,
            credential_published_at_millis: state.credential_published_at_millis,
        }
    }

    fn validate_obligation(
        &self,
        obligation: &AuthLeaseLifecyclePublicationObligation,
    ) -> Result<(), String> {
        if self.new_state != obligation.new_state {
            return Err(format!(
                "generated auth lease lifecycle publication state {:?} does not match authority state {:?}",
                obligation.new_state, self.new_state
            ));
        }
        if self.expires_at != obligation.expires_at {
            return Err(format!(
                "generated auth lease lifecycle publication expires_at {:?} does not match authority expires_at {:?}",
                obligation.expires_at, self.expires_at
            ));
        }
        if self.credential_generation != obligation.credential_generation {
            return Err(format!(
                "generated auth lease lifecycle publication generation {} does not match authority generation {}",
                obligation.credential_generation, self.credential_generation
            ));
        }
        if self.credential_published_at_millis != obligation.credential_published_at_millis {
            return Err(format!(
                "generated auth lease lifecycle publication credential publication time {:?} does not match authority publication time {:?}",
                obligation.credential_published_at_millis, self.credential_published_at_millis
            ));
        }
        Ok(())
    }
}

impl AuthLeaseLifecyclePublicationObligation {
    #[allow(unsafe_code)]
    pub(crate) fn into_auth_lease_transition(
        &self,
        scope: AuthLeaseLifecyclePublicationScope,
    ) -> Result<meerkat_core::handles::AuthLeaseTransition, String> {
        if self
            .transition_claimed
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return Err("generated auth lease lifecycle publication was already consumed".into());
        }
        scope.validate_obligation(self)?;
        let phase = match self.new_state {
            AuthLifecyclePhase::Valid => meerkat_core::handles::AuthLeasePhase::Valid,
            AuthLifecyclePhase::Expiring => meerkat_core::handles::AuthLeasePhase::Expiring,
            AuthLifecyclePhase::Expired => meerkat_core::handles::AuthLeasePhase::Expired,
            AuthLifecyclePhase::Refreshing => meerkat_core::handles::AuthLeasePhase::Refreshing,
            AuthLifecyclePhase::ReauthRequired => {
                meerkat_core::handles::AuthLeasePhase::ReauthRequired
            }
            AuthLifecyclePhase::Released => meerkat_core::handles::AuthLeasePhase::Released,
        };
        #[allow(improper_ctypes_definitions, unsafe_code)]
        unsafe extern "Rust" {
            #[link_name = concat!("__meerkat_core_runtime_generated_auth_lease_transition_build_v1_", env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX"))]
            fn core_runtime_generated_auth_lease_transition_build(
                token: &'static (dyn std::any::Any + Send + Sync),
                lease_key: meerkat_core::handles::LeaseKey,
                phase: meerkat_core::handles::AuthLeasePhase,
                expires_at: u64,
                generation: u64,
                credential_published_at_millis: Option<u64>,
            ) -> Result<meerkat_core::handles::AuthLeaseTransition, String>;
        }
        #[allow(unsafe_code)]
        unsafe {
            core_runtime_generated_auth_lease_transition_build(
                generated_authority_bridge_token(),
                scope.lease_key,
                phase,
                self.expires_at.unwrap_or(u64::MAX),
                self.credential_generation,
                self.credential_published_at_millis,
            )
        }
    }
}

#[allow(unsafe_code)]
pub fn generated_auth_lease_handle(
    handle: std::sync::Arc<crate::handles::RuntimeAuthLeaseHandle>,
) -> Result<meerkat_core::handles::GeneratedAuthLeaseHandle, String> {
    #[allow(improper_ctypes_definitions, unsafe_code)]
    unsafe extern "Rust" {
        #[link_name = concat!("__meerkat_core_runtime_generated_auth_lease_handle_build_v1_", env!("MEERKAT_GENERATED_AUTHORITY_BRIDGE_SYMBOL_SUFFIX"))]
        fn core_runtime_generated_auth_lease_handle_build(
            token: &'static (dyn std::any::Any + Send + Sync),
            handle: std::sync::Arc<dyn meerkat_core::handles::AuthLeaseHandle>,
        ) -> Result<meerkat_core::handles::GeneratedAuthLeaseHandle, String>;
    }
    let handle: std::sync::Arc<dyn meerkat_core::handles::AuthLeaseHandle> = handle;
    #[allow(unsafe_code)]
    unsafe {
        core_runtime_generated_auth_lease_handle_build(generated_authority_bridge_token(), handle)
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
                expires_at,
                credential_generation,
                credential_published_at_millis,
            } => Some(AuthLeaseLifecyclePublicationObligation {
                new_state: new_state.clone(),
                expires_at: expires_at.clone(),
                credential_generation: *credential_generation,
                credential_published_at_millis: credential_published_at_millis.clone(),
                transition_claimed: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            }),
            _ => None,
        })
        .collect()
}
