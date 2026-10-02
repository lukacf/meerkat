//! Credential custody joins the existing lease owner to native admission.
//!
//! Lock order: acquire the per-lease guard before normal asynchronous native
//! entry. Native map, session mutation and driver locks follow. A direct driver
//! caller already holds native custody, so it only tries the lease once. Release
//! observers only try native custody for the controller-reference scan. The
//! existing normalized lease guard, not native custody, excludes admission
//! through OAuth cleanup and the generated Release. No native guard spans I/O.
use std::sync::Arc;

use crate::input::Input;
use crate::input_authority::{NativeWorkAuthorizationHost, NativeWorkAuthorizationSlot};
use crate::traits::{ControllerReadinessFailure, RuntimeDriverError};
#[cfg(not(target_arch = "wasm32"))]
use meerkat_core::handles::{CredentialUseDisposition, CredentialUseIntent};
use meerkat_core::handles::{GeneratedAuthLeaseHandle, LeaseKey};

pub(crate) fn unavailable(reason: ControllerReadinessFailure) -> RuntimeDriverError {
    RuntimeDriverError::ControllerReadinessUnavailable { reason }
}

/// One process-only attachment in the existing native authorization slot.
/// The actual machine owns credential selection; policy callbacks cannot choose
/// a second lifecycle owner. A detached driver does not keep the machine alive.
pub(crate) struct NativeWorkAuthorizationAttachment {
    host: Arc<dyn NativeWorkAuthorizationHost>,
    owner: std::sync::Weak<super::MeerkatMachineShared>,
}

impl NativeWorkAuthorizationAttachment {
    pub(crate) fn new(
        host: Arc<dyn NativeWorkAuthorizationHost>,
        owner: &super::MeerkatMachine,
    ) -> Self {
        Self {
            host,
            owner: Arc::downgrade(&owner.shared),
        }
    }

    pub(crate) fn host(&self) -> &Arc<dyn NativeWorkAuthorizationHost> {
        &self.host
    }

    fn credential_authority(&self) -> Result<GeneratedAuthLeaseHandle, RuntimeDriverError> {
        let owner = self
            .owner
            .upgrade()
            .ok_or_else(|| unavailable(ControllerReadinessFailure::AuthorityUnavailable))?;
        owner.require_governed_execution_custody()?;
        let authority = owner
            .auth_lease
            .read()
            .map_err(|_| unavailable(ControllerReadinessFailure::AuthorityUnavailable))?
            .clone();
        Ok(authority)
    }
}

/// Stack-local custody, not a serializable permission or a reusable admission.
pub(crate) enum NativeCredentialCustody {
    Ungoverned,
    #[cfg(not(target_arch = "wasm32"))]
    Governed {
        guard: meerkat_core::AuthLoginLifecycleGuard,
        host: Arc<dyn NativeWorkAuthorizationHost>,
        authority: GeneratedAuthLeaseHandle,
        input_id: meerkat_core::InputId,
    },
}

impl NativeCredentialCustody {
    fn selected(
        slot: &NativeWorkAuthorizationSlot,
        input: &Input,
    ) -> Result<
        Option<(
            Arc<dyn NativeWorkAuthorizationHost>,
            GeneratedAuthLeaseHandle,
            LeaseKey,
        )>,
        RuntimeDriverError,
    > {
        if slot.get().is_none() && input.header().authority_association.is_none() {
            return Ok(None);
        }
        let host = slot
            .get()
            .ok_or_else(|| unavailable(ControllerReadinessFailure::AuthorityUnavailable))?;
        let ingress = input
            .header()
            .ingress_context
            .as_ref()
            .ok_or_else(crate::input_authority::unavailable)?;
        ingress.verify_submission(input)?;
        let controller = ingress
            .controller_client()
            .ok_or_else(crate::input_authority::unavailable)?;
        let key = LeaseKey::from_credential_identity(controller.selection().credential());
        let authority = host.credential_authority()?;
        Ok(Some((host.host().clone(), authority, key)))
    }

    pub(crate) async fn acquire(
        slot: &NativeWorkAuthorizationSlot,
        input: &Input,
        _persistent: bool,
    ) -> Result<Self, RuntimeDriverError> {
        let Some((host, authority, key)) = Self::selected(slot, input)? else {
            return Ok(Self::Ungoverned);
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            #[cfg(not(test))]
            let guard = meerkat_core::acquire_auth_login_lifecycle_guard(&key).await;
            #[cfg(test)]
            let guard = acquisition_test_observer::observe(
                input.id(),
                meerkat_core::acquire_auth_login_lifecycle_guard(&key),
            )
            .await;
            Ok(Self::Governed {
                guard,
                host,
                authority,
                input_id: input.id().clone(),
            })
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (host, authority, key);
            Err(unavailable(ControllerReadinessFailure::UnsupportedScope))
        }
    }

    pub(crate) fn try_acquire(
        slot: &NativeWorkAuthorizationSlot,
        input: &Input,
    ) -> Result<Self, RuntimeDriverError> {
        let Some((host, authority, key)) = Self::selected(slot, input)? else {
            return Ok(Self::Ungoverned);
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let guard = meerkat_core::try_acquire_auth_login_lifecycle_guard(&key)
                .ok_or_else(|| unavailable(ControllerReadinessFailure::Busy))?;
            Ok(Self::Governed {
                guard,
                host,
                authority,
                input_id: input.id().clone(),
            })
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (host, authority, key);
            Err(unavailable(ControllerReadinessFailure::UnsupportedScope))
        }
    }

    /// Call only under the actual native admission custody. The guard is held
    /// until the accepted row (or its synchronous rollback) has been published.
    pub(crate) fn validate(
        &self,
        slot: &NativeWorkAuthorizationSlot,
        input: &Input,
    ) -> Result<(), RuntimeDriverError> {
        match self {
            Self::Ungoverned
                if slot.get().is_none() && input.header().authority_association.is_none() =>
            {
                Ok(())
            }
            Self::Ungoverned => Err(unavailable(ControllerReadinessFailure::AuthorityChanged)),
            #[cfg(not(target_arch = "wasm32"))]
            Self::Governed {
                guard,
                host,
                authority,
                input_id,
            } => {
                let Some((current_host, current_authority, current_key)) =
                    Self::selected(slot, input)?
                else {
                    return Err(unavailable(ControllerReadinessFailure::AuthorityChanged));
                };
                if input_id != input.id()
                    || !Arc::ptr_eq(host, &current_host)
                    || !Arc::ptr_eq(&authority.clone_handle(), &current_authority.clone_handle())
                    || guard.lease_key() != &current_key
                {
                    return Err(unavailable(ControllerReadinessFailure::AuthorityChanged));
                }
                let disposition = authority
                    .resolve_credential_use_admission(
                        &current_key,
                        CredentialUseIntent::HoldAuthority,
                    )
                    .map_err(|_| unavailable(ControllerReadinessFailure::AuthorityUnavailable))?;
                if disposition != CredentialUseDisposition::Authorized {
                    return Err(unavailable(
                        ControllerReadinessFailure::CredentialUnusable { disposition },
                    ));
                }
                Ok(())
            }
        }
    }

    pub(crate) fn governed(&self) -> bool {
        !matches!(self, Self::Ungoverned)
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use super::super::{MeerkatMachine, MeerkatMachineShared};
    use super::*;
    use crate::handles::{AuthLeaseReleaseObserver, AuthLeaseReleasePermit, ReleasedOAuthFlows};
    use meerkat_core::handles::DslTransitionError;
    use std::sync::Weak;

    pub(crate) struct NativeCredentialReleaseObserver {
        machine: Weak<MeerkatMachineShared>,
    }

    impl AuthLeaseReleaseObserver for NativeCredentialReleaseObserver {
        fn begin_auth_lease_release<'a>(
            &'a self,
            key: &LeaseKey,
        ) -> Result<Option<Box<dyn AuthLeaseReleasePermit + 'a>>, DslTransitionError> {
            // Once the actual owner is gone it cannot admit or run work. Old
            // exported handles do not keep a second native owner alive.
            let Some(machine) = self.machine.upgrade() else {
                return Ok(None);
            };
            let machine = MeerkatMachine { shared: machine };
            let custody = machine
                .try_controller_custody_for_readiness()
                .map_err(release_error)?;
            if custody
                .references_credential(Some(key))
                .map_err(release_error)?
            {
                return Err(release_error(RuntimeDriverError::ControllerInUse));
            }
            // Full release already owns this exact normalized lease. All
            // supported admission paths must acquire it before publishing a row.
            // Returning no native permit keeps unrelated sessions out of OAuth I/O.
            Ok(None)
        }
        fn auth_lease_released(&self, _: &ReleasedOAuthFlows) -> Result<(), DslTransitionError> {
            Ok(())
        }
    }

    fn release_error(error: RuntimeDriverError) -> DslTransitionError {
        let context = match &error {
            RuntimeDriverError::ControllerInUse => "NativeControllerCredentialRelease::InUse",
            RuntimeDriverError::ControllerReadinessUnavailable {
                reason: ControllerReadinessFailure::Busy,
            } => "NativeControllerCredentialRelease::Busy",
            RuntimeDriverError::ControllerReadinessUnavailable {
                reason: ControllerReadinessFailure::UnsupportedScope,
            } => "NativeControllerCredentialRelease::UnsupportedScope",
            _ => "NativeControllerCredentialRelease::AuthorityUnavailable",
        };
        DslTransitionError::no_matching(context, error.to_string())
    }

    impl MeerkatMachine {
        pub(crate) fn install_native_credential_observer(&self) -> Result<(), RuntimeDriverError> {
            let observer = Arc::new(NativeCredentialReleaseObserver {
                machine: Arc::downgrade(&self.shared),
            });
            let authority = self.generated_auth_lease_handle();
            let actual = (authority.as_handle() as &dyn std::any::Any)
                .downcast_ref::<crate::handles::RuntimeAuthLeaseHandle>()
                .ok_or_else(|| unavailable(ControllerReadinessFailure::AuthorityUnavailable))?;
            let erased: Arc<dyn AuthLeaseReleaseObserver> = observer.clone();
            actual.add_release_observer(Arc::downgrade(&erased));
            self.credential_release_observer
                .set(observer)
                .map_err(|_| unavailable(ControllerReadinessFailure::AuthorityChanged))
        }

        pub(crate) fn with_credential_authority_replacement(
            &self,
            replacement: &Arc<crate::handles::RuntimeAuthLeaseHandle>,
            publish: impl FnOnce(),
        ) -> Result<(), RuntimeDriverError> {
            let Some(observer) = self.credential_release_observer.get() else {
                publish();
                return Ok(());
            };
            let replacement_handle: Arc<dyn meerkat_core::handles::AuthLeaseHandle> =
                replacement.clone();
            if Arc::ptr_eq(
                &self.generated_auth_lease_handle().clone_handle(),
                &replacement_handle,
            ) {
                return Ok(());
            }
            let custody = self.try_controller_custody_for_readiness()?;
            if custody.references_credential(None)? {
                return Err(RuntimeDriverError::ControllerInUse);
            }
            // The replacement's actual registry remains locked through install.
            // Transfer of populated owners needs a separate verified host protocol.
            replacement
                .with_empty_authority(|| {
                    let erased: Arc<dyn AuthLeaseReleaseObserver> = observer.clone();
                    replacement.add_release_observer(Arc::downgrade(&erased));
                    publish();
                })
                .map_err(unavailable)
        }
    }
}
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use native::NativeCredentialReleaseObserver;

#[cfg(target_arch = "wasm32")]
impl super::MeerkatMachine {
    pub(crate) fn install_native_credential_observer(&self) -> Result<(), RuntimeDriverError> {
        Err(unavailable(ControllerReadinessFailure::UnsupportedScope))
    }
}

// This observer reads the real acquisition future only in native unit tests.
#[cfg(all(test, not(target_arch = "wasm32")))]
#[allow(clippy::expect_used, clippy::panic)]
pub(super) mod acquisition_test_observer;
