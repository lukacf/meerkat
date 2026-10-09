//! Native owner custody for last-controller grant administration.

use std::collections::HashMap;
use std::sync::Arc;

use meerkat_authorization_contracts::grant_mutation::{
    ControllerCustodyRefusal, ControllerGrantMutationCustody,
};
use meerkat_authorization_contracts::work_association::GrantLineageRef;
use meerkat_core::SessionId;

use super::{DriverEntry, MeerkatMachine, RuntimeSessionEntry};
use crate::tokio::sync::{OwnedMutexGuard, RwLockReadGuard};

/// Actual machine map and driver custody, never a reusable "no users" token.
///
/// This supports a closed storeless machine or its actual governed memory or SQLite owner.
/// A host sharing grants
/// with another machine or detached owner must compose complete custody before
/// calling the grant owner. Persistent custody retains the backend's writer
/// and observes detached inputs as well as every attached native driver.
pub struct NativeControllerGrantMutation<'a> {
    // Drop the physical writer before releasing native admission custody.
    durable: Option<Box<dyn crate::store::RuntimeStoreControllerCustody + 'a>>,
    _sessions: RwLockReadGuard<'a, HashMap<SessionId, RuntimeSessionEntry>>,
    _mutations: Vec<OwnedMutexGuard<()>>,
    drivers: Vec<OwnedMutexGuard<DriverEntry>>,
}

impl NativeControllerGrantMutation<'_> {
    fn driver(entry: &DriverEntry) -> &crate::driver::ephemeral::EphemeralRuntimeDriver {
        match entry {
            DriverEntry::Ephemeral(driver) => driver,
            DriverEntry::Persistent(driver) => driver.inner_ref(),
        }
    }

    /// Interpret stored native lifecycle/input witnesses under the retained
    /// transaction. This is only a veto check; it restores no process client or
    /// permission. Unknown/torn authority requires recovery before mutation.
    fn visit_durable_unfinished(
        &self,
        mut visit: impl FnMut(
            &crate::identifiers::LogicalRuntimeId,
            &crate::input_state::StoredInputState,
        ) -> Result<bool, crate::traits::RuntimeDriverError>,
    ) -> Result<bool, crate::traits::RuntimeDriverError> {
        let Some(durable) = &self.durable else {
            return Ok(false);
        };
        let mut failure = None;
        let result = durable.visit_runtimes(&mut |runtime| {
            let checked = (|| {
                let crate::store::MachineLifecycleObservation::Decoded { record, .. } =
                    &runtime.lifecycle
                else {
                    return Err(crate::input_authority::unavailable());
                };
                let phase = record
                    .runtime_state()
                    .ok_or_else(crate::input_authority::unavailable)?;
                let run = record.run().current_run_id();
                if (phase == crate::RuntimeState::Running) != run.is_some() {
                    return Err(crate::input_authority::unavailable());
                }
                let mut run_has_original = false;
                for row in &runtime.input_states {
                    let in_run = run.is_some() && row.seed.last_run_id.as_ref() == run;
                    run_has_original |= in_run;
                    if !crate::store::input_state_is_recovery_nonterminal(row) && !in_run {
                        continue;
                    }
                    if row.state.authority_contributors.is_empty() {
                        return Err(crate::input_authority::unavailable());
                    }
                    for original in &row.state.authority_contributors {
                        let candidate = original.association().candidate();
                        if candidate.target.logical_runtime.as_str()
                            != runtime.runtime_id.to_string()
                            || candidate.controller_model.is_none()
                            || candidate.controller_grant_lineage.is_empty()
                        {
                            return Err(crate::input_authority::unavailable());
                        }
                    }
                    if visit(&runtime.runtime_id, row)? {
                        return Ok(true);
                    }
                }
                if run.is_some() && !run_has_original {
                    return Err(crate::input_authority::unavailable());
                }
                Ok(false)
            })();
            match checked {
                Ok(found) => found,
                Err(error) => {
                    failure = Some(error);
                    true
                }
            }
        });
        if let Some(error) = failure {
            return Err(error);
        }
        result.map_err(|_| crate::input_authority::unavailable())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn references_credential(
        &self,
        key: Option<&meerkat_core::handles::LeaseKey>,
    ) -> Result<bool, crate::traits::RuntimeDriverError> {
        use crate::meerkat_machine::credential_custody::unavailable;
        use crate::traits::ControllerReadinessFailure;
        for driver in &self.drivers {
            let driver = Self::driver(driver);
            if driver.visit_unfinished_controller_inputs(|row| {
                let controller = row
                    .controller_client
                    .as_ref()
                    .ok_or_else(|| unavailable(ControllerReadinessFailure::AuthorityUnavailable))?;
                if row.authority_contributors.iter().any(|original| {
                    original.association().candidate().controller_model.as_ref()
                        != Some(controller.selection())
                }) {
                    return Err(unavailable(
                        ControllerReadinessFailure::AuthorityUnavailable,
                    ));
                }
                Ok(key.is_none_or(|key| {
                    key == &meerkat_core::handles::LeaseKey::from_credential_identity(
                        controller.selection().credential(),
                    )
                }))
            })? {
                return Ok(true);
            }
        }
        self.visit_durable_unfinished(|_, row| {
            Ok(row.state.authority_contributors.iter().any(|original| {
                original
                    .association()
                    .candidate()
                    .controller_model
                    .as_ref()
                    .is_some_and(|selection| {
                        key.is_none_or(|key| {
                            key == &meerkat_core::handles::LeaseKey::from_credential_identity(
                                selection.credential(),
                            )
                        })
                    })
            }))
        })
    }
}

impl NativeControllerGrantMutation<'_> {
    /// Check a proposed policy against every original contributor still owned
    /// by this native owner, then commit once under the same custody.
    /// A terminal input in an unfinished run still protects its controller.
    ///
    /// The trusted callback must check the actual proposed state against the
    /// exact association and immutable client, including the admitted-work
    /// lifetime contract. It must not re-enter native drivers or perform I/O.
    /// The host separately holds the same grant publication reservation and its
    /// policy-state mutex, publishing successful replacement before unlock.
    /// No absence snapshot, permission or reusable token escapes this method.
    pub fn with_preserved_controllers<T>(
        &mut self,
        mut check_proposed: impl FnMut(
            &meerkat_authorization_contracts::work_association::InputAuthorityAssociation,
            &meerkat_core::ControllerModelClient,
        ) -> Result<(), meerkat_core::OperationAuthorizationError>,
        commit: impl FnOnce() -> T,
    ) -> Result<T, ControllerCustodyRefusal> {
        for driver in &self.drivers {
            let driver = Self::driver(driver);
            let mut refusal = None;
            driver
                .visit_unfinished_controller_inputs(|row| {
                    let pin = row
                        .controller_client
                        .as_ref()
                        .ok_or_else(crate::input_authority::unavailable)?;
                    if pin.client().controller_model_selection().as_ref() != Some(pin.selection()) {
                        return Err(crate::input_authority::unavailable());
                    }
                    for original in &row.authority_contributors {
                        if original.association().candidate().controller_model.as_ref()
                            != Some(pin.selection())
                        {
                            return Err(crate::input_authority::unavailable());
                        }
                        if let Err(error) = check_proposed(original.association(), pin) {
                            refusal = Some(match error {
                                meerkat_core::OperationAuthorizationError::Refused(refusal)
                                    if refusal.kind()
                                        == meerkat_core::OperationRefusalKind::Denied =>
                                {
                                    ControllerCustodyRefusal::ControllerInUse
                                }
                                _ => ControllerCustodyRefusal::Unavailable,
                            });
                            return Ok(true);
                        }
                    }
                    Ok(false)
                })
                .map_err(|_| ControllerCustodyRefusal::Unavailable)?;
            if let Some(refusal) = refusal {
                return Err(refusal);
            }
        }
        let mut refusal = None;
        self.visit_durable_unfinished(|runtime_id, stored| {
            // A detached retained association can veto mutation, but cannot
            // stand in for the immutable actual client needed to validate a
            // proposed policy. Require its exact still-attached native owner.
            let live = self
                .drivers
                .iter()
                .map(|driver| Self::driver(driver))
                .find(|driver| driver.runtime_id() == runtime_id)
                .and_then(|driver| driver.ledger().get(&stored.state.input_id))
                .ok_or_else(crate::input_authority::unavailable)?;
            let pin = live
                .controller_client
                .as_ref()
                .ok_or_else(crate::input_authority::unavailable)?;
            if pin.client().controller_model_selection().as_ref() != Some(pin.selection()) {
                return Err(crate::input_authority::unavailable());
            }
            for original in &stored.state.authority_contributors {
                if original.association().candidate().controller_model.as_ref()
                    != Some(pin.selection())
                    || !live.authority_contributors.iter().any(|current| {
                        current.input_id() == original.input_id()
                            && current.association() == original.association()
                    })
                {
                    return Err(crate::input_authority::unavailable());
                }
                if let Err(error) = check_proposed(original.association(), pin) {
                    refusal = Some(match error {
                        meerkat_core::OperationAuthorizationError::Refused(error)
                            if error.kind() == meerkat_core::OperationRefusalKind::Denied =>
                        {
                            ControllerCustodyRefusal::ControllerInUse
                        }
                        _ => ControllerCustodyRefusal::Unavailable,
                    });
                    return Ok(true);
                }
            }
            Ok(false)
        })
        .map_err(|_| ControllerCustodyRefusal::Unavailable)?;
        if let Some(refusal) = refusal {
            return Err(refusal);
        }
        Ok(commit())
    }
}

impl ControllerGrantMutationCustody for NativeControllerGrantMutation<'_> {
    fn with_unreferenced_controller_grant<T, E>(
        &mut self,
        reference: &GrantLineageRef,
        mutate: impl FnOnce() -> Result<T, E>,
    ) -> Result<Result<T, E>, ControllerCustodyRefusal> {
        for driver in &self.drivers {
            let referenced = Self::driver(driver)
                .unfinished_work_references_controller(reference)
                .map_err(|_| ControllerCustodyRefusal::Unavailable)?;
            if referenced {
                return Err(ControllerCustodyRefusal::ControllerInUse);
            }
        }
        if self
            .visit_durable_unfinished(|_, row| {
                Ok(row.state.authority_contributors.iter().any(|original| {
                    original
                        .association()
                        .candidate()
                        .controller_grant_lineage
                        .contains(reference)
                }))
            })
            .map_err(|_| ControllerCustodyRefusal::Unavailable)?
        {
            return Err(ControllerCustodyRefusal::ControllerInUse);
        }
        // All same-machine admission and registration remains excluded until
        // after the synchronous callback publishes its grant owner mutation.
        Ok(mutate())
    }
}

impl MeerkatMachine {
    /// Borrow every attached native driver for a synchronous grant mutation.
    ///
    /// Try each existing lock once while retaining the session map. Waiting for
    /// a driver or mutation gate here could invert normal lock order; contention
    /// instead refuses this administrative operation without disturbing work.
    /// Ordinary operation checks acquire none of these locks through this API.
    pub fn try_controller_grant_mutation(
        &self,
    ) -> Result<NativeControllerGrantMutation<'_>, ControllerCustodyRefusal> {
        self.try_controller_custody_for_readiness()
            .map_err(|_| ControllerCustodyRefusal::Unavailable)
    }

    pub(super) fn try_controller_custody_for_readiness(
        &self,
    ) -> Result<NativeControllerGrantMutation<'_>, crate::traits::RuntimeDriverError> {
        self.require_governed_execution_custody()?;
        let sessions = self.sessions.try_read().map_err(|_| {
            crate::meerkat_machine::credential_custody::unavailable(
                crate::traits::ControllerReadinessFailure::Busy,
            )
        })?;
        let mut ordered: Vec<_> = sessions.iter().collect();
        ordered.sort_unstable_by_key(|(id, _)| id.to_string());
        let mut mutations = Vec::with_capacity(ordered.len());
        let mut drivers = Vec::with_capacity(ordered.len());
        for (_, entry) in &ordered {
            mutations.push(
                Arc::clone(&entry.mutation_gate)
                    .try_lock_owned()
                    .map_err(|_| {
                        crate::meerkat_machine::credential_custody::unavailable(
                            crate::traits::ControllerReadinessFailure::Busy,
                        )
                    })?,
            );
        }
        for (_, entry) in &ordered {
            drivers.push(Arc::clone(&entry.driver).try_lock_owned().map_err(|_| {
                crate::meerkat_machine::credential_custody::unavailable(
                    crate::traits::ControllerReadinessFailure::Busy,
                )
            })?);
        }
        drop(ordered);
        let durable = self
            .store
            .as_ref()
            .map(|store| {
                let claim = self
                    .execution_custody
                    .as_deref()
                    .ok_or_else(crate::input_authority::unavailable)?;
                store.try_controller_mutation_custody(claim).map_err(|_| {
                    crate::meerkat_machine::credential_custody::unavailable(
                        crate::traits::ControllerReadinessFailure::AuthorityUnavailable,
                    )
                })
            })
            .transpose()?;
        Ok(NativeControllerGrantMutation {
            durable,
            _sessions: sessions,
            _mutations: mutations,
            drivers,
        })
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::input_authority::{NativeIngressContext, tests::TestIngress};
    use crate::input_state::InputAbandonReason;
    use crate::traits::RuntimeDriver;
    use meerkat_authorization_contracts::evidence::EvidenceId;
    use meerkat_authorization_contracts::work_association::InputAuthorityAssociation;

    async fn admitted() -> (MeerkatMachine, SessionId, GrantLineageRef) {
        let machine = MeerkatMachine::ephemeral();
        let host = Arc::new(TestIngress::isolated(machine.generated_auth_lease_handle()));
        let mut prompt = host.input("caller");
        let machine = machine
            .with_native_work_authorization_host(host)
            .expect("configured host");
        let session_id = SessionId::new();
        machine
            .register_session(session_id.clone())
            .await
            .expect("register");
        let (driver, runtime_id) = {
            let sessions = machine.sessions.read().await;
            let entry = sessions.get(&session_id).expect("session");
            (Arc::clone(&entry.driver), entry.runtime_id.clone())
        };
        let ingress = Arc::clone(
            prompt
                .header()
                .ingress_context
                .as_ref()
                .expect("actual ingress"),
        );
        let mut candidate = prompt
            .header()
            .authority_association
            .as_ref()
            .expect("claims")
            .candidate()
            .clone();
        candidate.target.logical_runtime =
            EvidenceId::new(runtime_id.to_string()).expect("runtime id");
        let grant = candidate.controller_grant_lineage[0].clone();
        let selection = candidate
            .controller_model
            .clone()
            .expect("selected controller route");
        prompt.header_mut().authority_association =
            Some(InputAuthorityAssociation::new(candidate).expect("claims"));
        let actual = NativeIngressContext::from_trusted_ingress(
            &prompt,
            ingress.requester().clone(),
            ingress.ingress_actor().clone(),
            ingress.realm().clone(),
            ingress.authentication().clone(),
        )
        .expect("bound final submission")
        .with_controller_client(
            &prompt,
            meerkat_core::ControllerModelClient::new(
                selection.clone(),
                Arc::new(super::archive_tests::SelectedClient(selection)),
            ),
        )
        .expect("same selected controller");
        let prompt = prompt
            .with_ingress_context(actual)
            .expect("exact process context");
        {
            let mut driver = driver.lock().await;
            let DriverEntry::Ephemeral(driver) = &mut *driver else {
                panic!("storeless")
            };
            driver.set_executor_work_authorization_support(true);
            driver
                .accept_input(prompt)
                .await
                .expect("actual native admission");
        }
        (machine, session_id, grant)
    }

    #[tokio::test]
    async fn controller_revoke_is_refused_until_actual_native_terminality() {
        let (machine, session_id, grant) = admitted().await;
        let mut called = false;
        {
            let mut custody = machine
                .try_controller_grant_mutation()
                .expect("actual owner locks");
            let result = custody.with_unreferenced_controller_grant(&grant, || {
                called = true;
                Ok::<_, ()>(())
            });
            assert_eq!(result, Err(ControllerCustodyRefusal::ControllerInUse));
        }
        assert!(!called);
        let driver = Arc::clone(
            &machine
                .sessions
                .read()
                .await
                .get(&session_id)
                .expect("session")
                .driver,
        );
        {
            let mut driver = driver.lock().await;
            let DriverEntry::Ephemeral(driver) = &mut *driver else {
                panic!("storeless")
            };
            assert_eq!(
                driver
                    .abandon_all_non_terminal(InputAbandonReason::Stopped)
                    .expect("actual stop"),
                1
            );
        }
        let mut custody = machine
            .try_controller_grant_mutation()
            .expect("terminal owners");
        assert_eq!(
            custody.with_unreferenced_controller_grant(&grant, || {
                called = true;
                Ok::<_, ()>(())
            }),
            Ok(Ok(()))
        );
        assert!(called);
    }

    #[tokio::test]
    async fn unrelated_qualified_grant_can_change_while_native_work_remains() {
        let (machine, _, mut grant) = admitted().await;
        grant.authority_namespace = EvidenceId::new("unrelated-namespace").expect("namespace");
        let mut custody = machine.try_controller_grant_mutation().expect("owners");
        assert_eq!(
            custody.with_unreferenced_controller_grant(&grant, || Ok::<_, ()>(42)),
            Ok(Ok(42))
        );
    }

    #[tokio::test]
    async fn custody_excludes_new_registration_and_never_waits_for_busy_driver() {
        let (machine, session_id, _) = admitted().await;
        let driver = Arc::clone(
            &machine
                .sessions
                .read()
                .await
                .get(&session_id)
                .expect("session")
                .driver,
        );
        let driver_lock = driver.lock().await;
        assert!(matches!(
            machine.try_controller_grant_mutation(),
            Err(ControllerCustodyRefusal::Unavailable)
        ));
        drop(driver_lock);
        let custody = machine.try_controller_grant_mutation().expect("owners");
        assert!(
            driver.try_lock().is_err(),
            "admission cannot race the callback"
        );
        assert!(
            machine.sessions.try_write().is_err(),
            "registration cannot race the callback"
        );
        drop(custody);
        assert!(driver.try_lock().is_ok());
        assert!(machine.sessions.try_write().is_ok());
    }

    #[tokio::test]
    async fn controller_custody_composes_both_native_maps_before_mutation() {
        let first = MeerkatMachine::ephemeral();
        let (second, _, controller) = admitted().await;
        let mut first_custody = first.try_controller_grant_mutation().expect("first scope");
        let mut second_custody = second
            .try_controller_grant_mutation()
            .expect("second scope");
        let mut mutation_entered = false;
        let result = first_custody.with_unreferenced_controller_grant(&controller, || {
            second_custody.with_unreferenced_controller_grant(&controller, || {
                mutation_entered = true;
                Ok::<_, ()>(())
            })
        });
        assert_eq!(result, Ok(Err(ControllerCustodyRefusal::ControllerInUse)));
        assert!(!mutation_entered);

        // The exact same owners permit a reference outside the retained lineage.
        let mut unrelated = controller;
        unrelated.authority_namespace = EvidenceId::new("other-authority").expect("namespace");
        assert_eq!(
            first_custody.with_unreferenced_controller_grant(&unrelated, || {
                second_custody.with_unreferenced_controller_grant(&unrelated, || {
                    mutation_entered = true;
                    Ok::<_, ()>(42)
                })
            }),
            Ok(Ok(Ok(42)))
        );
        assert!(mutation_entered);
        assert!(first.sessions.try_write().is_err());
        assert!(second.sessions.try_write().is_err());
        drop(second_custody);
        drop(first_custody);
        assert!(first.sessions.try_write().is_ok());
        assert!(second.sessions.try_write().is_ok());
    }

    #[tokio::test]
    async fn controller_custody_busy_second_scope_releases_first_without_callback() {
        let first = MeerkatMachine::ephemeral();
        let (second, session_id, mut unrelated) = admitted().await;
        unrelated.authority_namespace = EvidenceId::new("other-authority").expect("namespace");
        let mut mutation_entered = false;
        {
            let mut first_custody = first.try_controller_grant_mutation().expect("first scope");
            let mut second_custody = second
                .try_controller_grant_mutation()
                .expect("second scope");
            assert_eq!(
                first_custody.with_unreferenced_controller_grant(&unrelated, || {
                    second_custody.with_unreferenced_controller_grant(&unrelated, || {
                        mutation_entered = true;
                        Ok::<_, ()>(())
                    })
                }),
                Ok(Ok(Ok(())))
            );
            assert!(mutation_entered);
        }
        mutation_entered = false;
        let driver = {
            let sessions = second.sessions.read().await;
            Arc::clone(
                &sessions
                    .get(&session_id)
                    .expect("actual second session")
                    .driver,
            )
        };
        let busy_driver = driver.lock().await;
        let result = (|| {
            let mut first_custody = first.try_controller_grant_mutation()?;
            let mut second_custody = second.try_controller_grant_mutation()?;
            first_custody.with_unreferenced_controller_grant(&unrelated, || {
                second_custody.with_unreferenced_controller_grant(&unrelated, || {
                    mutation_entered = true;
                    Ok::<_, ()>(())
                })
            })
        })();
        assert_eq!(result, Err(ControllerCustodyRefusal::Unavailable));
        assert!(!mutation_entered);
        assert!(first.sessions.try_write().is_ok());
        assert!(second.sessions.try_write().is_ok());
        assert!(driver.try_lock().is_err());
        drop(busy_driver);
        assert!(driver.try_lock().is_ok());
    }
}

#[cfg(test)]
#[path = "controller_custody_archive_tests.rs"]
mod archive_tests;
