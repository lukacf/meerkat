//! Local grant composition over the actual native accepted-input owner.
//!
//! No input, session, identity or grant registry is copied here. App invocation
//! and resource decisions remain explicitly installed trusted owners. This
//! adapter supports attached storeless sessions and live memory-backed persistent
//! sessions with exclusive backend execution custody. Restart and unloaded
//! administrative controller scope are not established by this adapter.

use std::sync::{Arc, Weak};

use meerkat_authorization::grant_policy::{
    AdmittedWorkPolicyOwner, GrantBackedWorkPolicy, OperationPolicyOwner, WorkOwnerAllowance,
};
use meerkat_authorization::grants::LocalGrantAuthority;
use meerkat_authorization::policy::LocalPolicyPurpose;
use meerkat_authorization_contracts::work_association::InputAuthorityAssociation;
use meerkat_core::authorization::{
    OperationObservation, OperationObservationError, OperationRefusalKind, OperationRefused,
    PreparedAuthorizationBinding, PreparedOperationAuthorization, WorkAuthorization,
    WorkAuthorizationContext,
};
use meerkat_core::exact_operation::OperationExecutionScope;

use super::{MeerkatMachine, MeerkatMachineShared, dsl};
use crate::driver::ephemeral::SharedIngressDslAuthority;
use crate::identifiers::LogicalRuntimeId;
use crate::input::Input;
use crate::input_authority::{
    NativeAdmissionError, NativeIngressContext, NativeWorkAuthorizationHost, NativeWorkBatch,
    NativeWorkContextError,
};
use crate::traits::RuntimeDriverError;

/// The installed host authenticates the actual transport and authorizes its
/// requester to invoke this exact executor/represented-subject mandate. The
/// callback receives process ingress separately from historical wire claims.
/// It must also establish the admitted controller's usable route/ceiling from
/// actual configured owners; a credential or selected model is not permission.
/// Report unavailable current ingress evidence as `NativeAdmissionError::Readiness`;
/// an operational failure is not a permission refusal.
pub type NativeIngressCheck = dyn Fn(
        &LogicalRuntimeId,
        &Input,
        &NativeIngressContext,
        &InputAuthorityAssociation,
    ) -> Result<(), NativeAdmissionError>
    + Send
    + Sync;

/// Trusted embedding composition. None of these owners is selected by claims.
/// Sharing grants with another machine requires complete controller-mutation
/// custody for that additional scope before enabling administrative removal.
pub struct NativeGrantWorkConfiguration {
    pub grants: Arc<LocalGrantAuthority>,
    pub ingress: Arc<NativeIngressCheck>,
    pub invocation_owner: Arc<dyn AdmittedWorkPolicyOwner>,
    pub operation_owner: Arc<dyn OperationPolicyOwner>,
}

impl std::fmt::Debug for NativeGrantWorkConfiguration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeGrantWorkConfiguration")
            .finish_non_exhaustive()
    }
}

impl MeerkatMachine {
    /// Install the actual native/grant composition before sharing the machine.
    /// Exclusive ownership is checked before constructing any Weak reference.
    /// An empty but already shared machine is not eligible. A persistent backend
    /// must atomically upgrade its sole actual execution claim before setup;
    /// current support is live process-local work, not restored client custody.
    pub fn with_local_grant_authorization(
        mut self,
        configuration: NativeGrantWorkConfiguration,
    ) -> Result<Self, RuntimeDriverError> {
        {
            let shared =
                Arc::get_mut(&mut self.shared).ok_or_else(crate::input_authority::unavailable)?;
            if !shared.sessions.get_mut().is_empty()
                || shared.native_work_authorization_host.get().is_some()
            {
                return Err(crate::input_authority::unavailable());
            }
            shared.upgrade_execution_custody()?;
        }
        // Admission checks the installed owners before an accepted batch
        // exists. Operation preparation gets a separate adapter over that
        // batch's actual generated owner, retaining these same policy owners.
        let policy = Arc::new(GrantBackedWorkPolicy::new(
            Arc::clone(&configuration.grants),
            Arc::clone(&configuration.invocation_owner),
            Arc::clone(&configuration.operation_owner),
        ));
        let host: Arc<dyn NativeWorkAuthorizationHost> =
            Arc::new(NativeGrantWorkAuthorizationHost {
                ingress: configuration.ingress,
                policy,
                grants: configuration.grants,
                invocation_owner: configuration.invocation_owner,
                operation_owner: configuration.operation_owner,
                machine: Arc::downgrade(&self.shared),
            });
        self.shared
            .native_work_authorization_host
            .set(super::credential_custody::NativeWorkAuthorizationAttachment::new(host, &self))
            .map_err(|_| crate::input_authority::unavailable())?;
        self.install_native_credential_observer()?;
        Ok(self)
    }
}

struct NativeGrantWorkAuthorizationHost {
    ingress: Arc<NativeIngressCheck>,
    policy: Arc<GrantBackedWorkPolicy>,
    grants: Arc<LocalGrantAuthority>,
    invocation_owner: Arc<dyn AdmittedWorkPolicyOwner>,
    operation_owner: Arc<dyn OperationPolicyOwner>,
    machine: Weak<MeerkatMachineShared>,
}

impl NativeWorkAuthorizationHost for NativeGrantWorkAuthorizationHost {
    fn authenticate_association(
        &self,
        runtime: &LogicalRuntimeId,
        input: &Input,
        ingress: &NativeIngressContext,
        association: &InputAuthorityAssociation,
    ) -> Result<(), NativeAdmissionError> {
        let candidate = association.candidate();
        let controller = ingress
            .controller_client()
            .ok_or_else(|| NativeAdmissionError::Refused(malformed()))?;
        if candidate.controller_model.as_ref() != Some(controller.selection())
            || controller.client().controller_model_selection().as_ref()
                != Some(controller.selection())
            || ingress.requester() != &candidate.requester
            || ingress.ingress_actor() != &candidate.ingress_actor
            || ingress.realm() != &candidate.ingress_namespace.realm
        {
            return Err(NativeAdmissionError::Refused(malformed()));
        }
        (self.ingress)(runtime, input, ingress, association)?;
        // Read the actual immutable selected child without manufacturing a
        // request. Current account policy and full grant ancestry are separate
        // conjuncts; a credential or a historical selection supplies neither.
        let facts = controller.plain_facts().map_err(|_| {
            NativeAdmissionError::Readiness(
                crate::traits::ControllerReadinessFailure::FactsUnavailable,
            )
        })?;
        self.policy
            .validate_controller_admission(association, &facts)
            .map_err(NativeAdmissionError::from)
    }

    fn work_context(
        &self,
        batch: &NativeWorkBatch,
    ) -> Result<WorkAuthorizationContext, NativeWorkContextError> {
        let controller = batch
            .controller_client()
            .cloned()
            .ok_or(NativeWorkContextError::UnsupportedController)?;
        let associations: Arc<[InputAuthorityAssociation]> = batch
            .contributors()
            .iter()
            .map(|original| original.association().clone())
            .collect::<Vec<_>>()
            .into();
        let run = NativeRunCustody::for_batch(batch, self.machine.clone())?;
        let native_owner = NativeAcceptedWorkOwner {
            run: run.clone(),
            runtime_id: batch.runtime_id().to_string(),
            selected_input_bindings: batch.selected_input_bindings.clone(),
            associations: Arc::clone(&associations),
            invocation_owner: Arc::clone(&self.invocation_owner),
        };
        let policy = Arc::new(GrantBackedWorkPolicy::new(
            Arc::clone(&self.grants),
            Arc::new(native_owner),
            Arc::clone(&self.operation_owner),
        ));
        let context = policy
            .audited_work_context(
                associations,
                batch.execution_scope().clone(),
                Arc::clone(batch.audit_sink()),
            )
            .map_err(|_| NativeWorkContextError::MalformedAcceptedWork)?;
        // This constructs no allowance and performs no current-policy check.
        // It can precede StageForRun. Actual prepare/check require the native
        // owner to have installed this precise Running run.
        let authorization = Arc::new(NativeRunAuthorization {
            inner: Arc::clone(context.authorization()),
            run,
        });
        WorkAuthorizationContext::new(authorization, batch.execution_scope().clone())
            .with_controller_client(controller)
            .map_err(|_| NativeWorkContextError::UnsupportedController)
    }
}

struct NativeAcceptedWorkOwner {
    run: NativeRunCustody,
    runtime_id: String,
    selected_input_bindings: std::collections::BTreeMap<String, (String, String)>,
    associations: Arc<[InputAuthorityAssociation]>,
    invocation_owner: Arc<dyn AdmittedWorkPolicyOwner>,
}

impl AdmittedWorkPolicyOwner for NativeAcceptedWorkOwner {
    fn authorize_admitted_work(
        &self,
        association: &InputAuthorityAssociation,
        binding: &PreparedAuthorizationBinding,
        purpose: LocalPolicyPurpose,
        now_ms: u64,
    ) -> Result<WorkOwnerAllowance, meerkat_core::OperationAuthorizationError> {
        self.run.check_binding(binding)?;
        self.run.check_durability()?;
        let machine = self
            .run
            .machine
            .upgrade()
            .ok_or(meerkat_core::OperationAuthorizationError::Unavailable)?;
        machine
            .require_governed_execution_custody()
            .map_err(|_| meerkat_core::OperationAuthorizationError::Unavailable)?;
        if association.candidate().target.logical_runtime.as_str() != self.runtime_id.as_str()
            || !self.associations.contains(association)
        {
            return Err(malformed().into());
        }
        {
            // Actual owner only: no sessions, mutation or driver coordination
            // gate is needed to read this already accepted batch. This short
            // synchronous lock never crosses an app callback, await or I/O.
            let current = self
                .run
                .authority
                .lock()
                .map_err(|_| meerkat_core::OperationAuthorizationError::Unavailable)?;
            let state = current.state();
            self.run.check_state(state)?;
            let OperationExecutionScope::RuntimeInput {
                canonical_input_id, ..
            } = &self.run.execution_scope
            else {
                return Err(malformed().into());
            };
            if !self
                .selected_input_bindings
                .contains_key(&canonical_input_id.to_string())
            {
                return Err(malformed().into());
            }
            for (key, (binding, batch_key)) in &self.selected_input_bindings {
                if state.input_run_associations.get(key) != Some(&self.run.run_id)
                    || state.input_authority_bindings.get(key) != Some(binding)
                    || state.input_authority_batch_keys.get(key) != Some(batch_key)
                {
                    return Err(malformed().into());
                }
            }
            // Presence alone is insufficient: an additional staged contributor
            // requires a newly built batch context, not reuse of this one.
            if state.input_run_associations.iter().any(|(key, run)| {
                run == &self.run.run_id && !self.selected_input_bindings.contains_key(key)
            }) {
                return Err(malformed().into());
            }
        }
        // Retained originals include every pre-stage coalesced contribution.
        // They are immutable input data, not current account/route permission.
        // Current invocation, grant and resource owners remain independent.
        self.invocation_owner
            .authorize_admitted_work(association, binding, purpose, now_ms)
    }
}

#[derive(Clone)]
struct NativeRunCustody {
    authority: SharedIngressDslAuthority,
    machine: Weak<MeerkatMachineShared>,
    session_id: dsl::SessionId,
    epoch_id: dsl::RuntimeEpochId,
    run_id: dsl::RunId,
    execution_scope: OperationExecutionScope,
    domain_run_id: meerkat_core::RunId,
    durability_health: Option<super::DurabilityHealthHandle>,
}

impl NativeRunCustody {
    fn for_batch(
        batch: &NativeWorkBatch,
        machine: Weak<MeerkatMachineShared>,
    ) -> Result<Self, NativeWorkContextError> {
        let OperationExecutionScope::RuntimeInput {
            owner_session_id,
            runtime_epoch_id,
            submitted_input_id,
            canonical_input_id,
        } = batch.execution_scope()
        else {
            return Err(NativeWorkContextError::MalformedAcceptedWork);
        };
        if submitted_input_id != canonical_input_id {
            return Err(NativeWorkContextError::MalformedAcceptedWork);
        }
        Ok(Self {
            authority: Arc::clone(&batch.authority),
            machine,
            session_id: dsl::SessionId::from_domain(owner_session_id),
            epoch_id: dsl::RuntimeEpochId::from_domain(runtime_epoch_id),
            run_id: dsl::RunId::from_domain(batch.run_id()),
            execution_scope: batch.execution_scope().clone(),
            domain_run_id: batch.run_id().clone(),
            durability_health: batch.durability_health.clone(),
        })
    }

    fn check_binding(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<(), OperationRefused> {
        if binding.facts().execution_scope != self.execution_scope
            || binding.facts().run_id.as_ref() != Some(&self.domain_run_id)
        {
            return Err(malformed());
        }
        Ok(())
    }

    fn check_state(&self, state: &dsl::MeerkatMachineState) -> Result<(), OperationRefused> {
        if state.session_id.as_ref() != Some(&self.session_id)
            || state.active_runtime_epoch_id.as_ref() != Some(&self.epoch_id)
            || state.current_run_id.as_ref() != Some(&self.run_id)
            || state.lifecycle_phase != dsl::MeerkatPhase::Running
            || state.turn_terminal_run_id.as_ref() == Some(&self.run_id)
        {
            return Err(malformed());
        }
        Ok(())
    }

    fn check_durability(&self) -> Result<(), meerkat_core::OperationAuthorizationError> {
        if let Some(health) = self.durability_health.as_ref() {
            health
                .require_ready()
                .map_err(|_| meerkat_core::OperationAuthorizationError::Unavailable)?;
        }
        Ok(())
    }

    fn check(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<(), meerkat_core::OperationAuthorizationError> {
        self.check_binding(binding)?;
        self.check_durability()?;
        let _machine = self
            .machine
            .upgrade()
            .ok_or(meerkat_core::OperationAuthorizationError::Unavailable)?;
        // Brief read contention does not change permission. Keep the existing
        // scalar warm check over the same actual generated owner. This change
        // does not add a second dynamic membership/publication protocol.
        let authority = self
            .authority
            .lock()
            .map_err(|_| meerkat_core::OperationAuthorizationError::Unavailable)?;
        self.check_state(authority.state()).map_err(Into::into)
    }
}

struct NativeRunAuthorization {
    inner: Arc<dyn WorkAuthorization>,
    run: NativeRunCustody,
}

impl WorkAuthorization for NativeRunAuthorization {
    fn controller_model_selection(&self) -> Option<meerkat_core::ControllerModelSelection> {
        self.inner.controller_model_selection()
    }

    fn prepare(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<Arc<dyn PreparedOperationAuthorization>, meerkat_core::OperationAuthorizationError>
    {
        // The inner compiler verifies the actual accepted rows and records
        // refusal. Check run custody again after that observed preparation so
        // a racing run end cannot publish a usable prepared result.
        let prepared = self.inner.prepare(binding)?;
        if let Err(error) = self.run.check(binding) {
            let observation = match error {
                meerkat_core::OperationAuthorizationError::Refused(refusal) => {
                    OperationObservation::Refused(refusal.kind())
                }
                meerkat_core::OperationAuthorizationError::Unavailable => {
                    OperationObservation::AuthorizationUnavailable
                }
                meerkat_core::OperationAuthorizationError::ObservationUnavailable(_) => {
                    return Err(error);
                }
            };
            prepared.observe(binding, observation)?;
            return Err(error);
        }
        Ok(Arc::new(NativePreparedAuthorization {
            prepared,
            run: self.run.clone(),
        }))
    }
}

struct NativePreparedAuthorization {
    prepared: Arc<dyn PreparedOperationAuthorization>,
    run: NativeRunCustody,
}

impl PreparedOperationAuthorization for NativePreparedAuthorization {
    fn check_current(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<(), meerkat_core::OperationAuthorizationError> {
        self.run.check(binding)?;
        self.prepared.check_current(binding)
    }

    fn observe(
        &self,
        binding: &PreparedAuthorizationBinding,
        observation: OperationObservation,
    ) -> Result<(), OperationObservationError> {
        // An outcome describes an actual operation even if its run or grant
        // became unavailable after entry. Never reauthorize historical outcome.
        self.prepared.observe(binding, observation)
    }
}

fn malformed() -> OperationRefused {
    OperationRefused::new(OperationRefusalKind::MalformedFacts)
}
#[cfg(test)]
fn denied() -> OperationRefused {
    OperationRefused::new(OperationRefusalKind::Denied)
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::input_authority::tests::input;
    use crate::meerkat_machine::DriverEntry;
    use crate::traits::RuntimeDriver;
    use meerkat_authorization::grants::LocalGrantConfiguration;
    use meerkat_authorization::policy::{LocalOperationValues, LocalPolicyAllowance};
    use meerkat_authorization::publication::LocalAuthorizationPublication;
    use meerkat_authorization::work::{LocalAuthorizationClock, LocalAuthorizationTime};
    use meerkat_authorization_contracts::constraints::{
        ActionRef, AudienceRef, ExecutionRestrictions, ProcessorRef, ResourceDomain,
    };
    use meerkat_authorization_contracts::evidence::EvidenceId;
    use meerkat_core::authorization::{
        AuthorizationOperation, OperationAuthorizationFacts, OwnerQualifiedTarget,
        SourceAuthorizationFacts, SourceAuthorizationTarget, SourceAuthorizationUse,
    };
    use meerkat_core::{ControllerModelClient, ControllerModelSelection, PrincipalRef, SessionId};

    struct Clock;
    impl LocalAuthorizationClock for Clock {
        fn now(
            &self,
        ) -> Result<LocalAuthorizationTime, meerkat_authorization::clock::LocalClockError> {
            Ok(LocalAuthorizationTime {
                unix_ms: 100,
                monotonic: meerkat_core::time_compat::Instant::now(),
            })
        }
    }

    struct Invocation {
        requester: PrincipalRef,
        executor: PrincipalRef,
    }
    impl AdmittedWorkPolicyOwner for Invocation {
        fn authorize_admitted_work(
            &self,
            association: &InputAuthorityAssociation,
            _: &PreparedAuthorizationBinding,
            _: LocalPolicyPurpose,
            _: u64,
        ) -> Result<WorkOwnerAllowance, meerkat_core::OperationAuthorizationError> {
            if association.candidate().requester != self.requester
                || association.candidate().logical_executor != self.executor
                || association.candidate().represented_subject.is_some()
            {
                return Err(denied().into());
            }
            Ok(WorkOwnerAllowance {
                restrictions: ExecutionRestrictions::unrestricted(),
                expires_at_ms: 10_000,
            })
        }
    }

    struct ResourceOwner {
        domain: ResourceDomain,
        processor: PrincipalRef,
    }
    impl OperationPolicyOwner for ResourceOwner {
        fn authorize_controller_admission(
            &self,
            association: &InputAuthorityAssociation,
            facts: &meerkat_core::ControllerModelFacts,
            _: u64,
        ) -> Result<
            meerkat_authorization::grant_policy::ControllerAdmissionAllowance,
            meerkat_core::OperationAuthorizationError,
        > {
            if association.candidate().logical_executor != self.processor
                || association.candidate().controller_model.as_ref() != Some(facts.selection())
                || facts.endpoint() != "https://controller.invalid/model"
                || facts.wire_model() != facts.selection().model()
            {
                return Err(denied().into());
            }
            Ok(
                meerkat_authorization::grant_policy::ControllerAdmissionAllowance {
                    operation_values: vec![LocalOperationValues {
                        action: ActionRef {
                            feature: "native-test".into(),
                            action: "infer".into(),
                        },
                        resource_domain: self.domain.clone(),
                        processor: ProcessorRef::Principal {
                            principal: self.processor.clone(),
                        },
                        audience: AudienceRef::Principal {
                            principal: self.processor.clone(),
                        },
                    }],
                    restrictions: ExecutionRestrictions::unrestricted(),
                },
            )
        }
        fn authorize_operation(
            &self,
            _: &InputAuthorityAssociation,
            binding: &PreparedAuthorizationBinding,
            purpose: LocalPolicyPurpose,
            _: u64,
        ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
            let AuthorizationOperation::Source(facts) = &binding.facts().operation else {
                return Err(denied().into());
            };
            let SourceAuthorizationTarget::External(target) = &facts.target else {
                return Err(denied().into());
            };
            if purpose != LocalPolicyPurpose::Operation
                || facts.usage != SourceAuthorizationUse::Read
                || target.authority != self.domain.authority
                || target.namespace.as_ref() != self.domain.namespace
                || target.id.as_ref() != "record"
            {
                return Err(denied().into());
            }
            Ok(LocalPolicyAllowance {
                operation_values: vec![LocalOperationValues {
                    action: ActionRef {
                        feature: "native-test".into(),
                        action: "read".into(),
                    },
                    resource_domain: self.domain.clone(),
                    processor: ProcessorRef::Principal {
                        principal: self.processor.clone(),
                    },
                    audience: AudienceRef::Principal {
                        principal: self.processor.clone(),
                    },
                }],
                restrictions: ExecutionRestrictions::unrestricted(),
                expires_at_ms: 10_000,
            })
        }
    }

    struct SelectedClient(ControllerModelSelection);
    #[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
    #[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
    impl meerkat_core::AgentLlmClient for SelectedClient {
        fn controller_model_selection(&self) -> Option<ControllerModelSelection> {
            Some(self.0.clone())
        }
        fn controller_model_facts(
            &self,
        ) -> Result<meerkat_core::ControllerModelFacts, meerkat_core::ControllerFactsUnavailable>
        {
            Ok(meerkat_core::ControllerModelFacts::new(
                self.0.clone(),
                "https://controller.invalid/model".into(),
                self.0.model().into(),
            ))
        }
        async fn stream_response(
            &self,
            _: &[meerkat_core::Message],
            _: &[Arc<meerkat_core::ToolDef>],
            _: u32,
            _: Option<f32>,
            _: Option<&meerkat_core::ProviderParamsOverride>,
        ) -> Result<meerkat_core::LlmStreamResult, meerkat_core::AgentError> {
            Err(meerkat_core::AgentError::ConfigError(
                "source-only fixture never calls a model".into(),
            ))
        }
        fn provider(&self) -> meerkat_core::Provider {
            self.0.provider()
        }
        fn model(&self) -> &str {
            self.0.model()
        }
    }

    fn configuration() -> (NativeGrantWorkConfiguration, Input, ResourceDomain) {
        let mut prompt = input("caller");
        let candidate = prompt
            .header()
            .authority_association
            .as_ref()
            .expect("claims")
            .candidate();
        let requester = candidate.requester.clone();
        let executor = candidate.logical_executor.clone();
        let root = candidate.controller_grant_lineage[0].root_authority.clone();
        let domain = ResourceDomain {
            authority: root.clone(),
            namespace: "test-source".into(),
        };
        let grants = Arc::new(
            LocalGrantAuthority::new(
                LocalGrantConfiguration {
                    root: root.clone(),
                    namespace: EvidenceId::new("controller").expect("id"),
                    generation: 1,
                },
                LocalAuthorizationPublication::new(),
                Arc::new(Clock),
            )
            .expect("real generated grant owner"),
        );
        let grant = grants
            .issue_root(
                &root,
                EvidenceId::new("controller-leaf").expect("id"),
                executor.clone(),
                None,
                ExecutionRestrictions::unrestricted(),
            )
            .expect("real issued controller");
        let mut candidate = candidate.clone();
        candidate.controller_grant_lineage = vec![grant];
        prompt.header_mut().authority_association =
            Some(InputAuthorityAssociation::new(candidate).expect("claims"));
        let expected_requester = requester.clone();
        let expected_executor = executor.clone();
        let ingress: Arc<NativeIngressCheck> = Arc::new(move |_, _, current, association| {
            if current.requester() == &expected_requester
                && association.candidate().logical_executor == expected_executor
                && association.candidate().represented_subject.is_none()
            {
                Ok(())
            } else {
                Err(denied().into())
            }
        });
        (
            NativeGrantWorkConfiguration {
                grants,
                ingress,
                invocation_owner: Arc::new(Invocation {
                    requester,
                    executor: executor.clone(),
                }),
                operation_owner: Arc::new(ResourceOwner {
                    domain: domain.clone(),
                    processor: executor,
                }),
            },
            prompt,
            domain,
        )
    }

    async fn pending_controller_input(
        configuration: NativeGrantWorkConfiguration,
        mut prompt: Input,
    ) -> (MeerkatMachine, SessionId, Input) {
        let machine = MeerkatMachine::ephemeral()
            .with_local_grant_authorization(configuration)
            .expect("exclusive setup before Weak construction");
        let session = SessionId::new();
        machine
            .register_session(session.clone())
            .await
            .expect("actual registration");
        let runtime = {
            let sessions = machine.sessions.read().await;
            let entry = sessions.get(&session).expect("actual entry");
            entry.runtime_id.clone()
        };
        let original_ingress = Arc::clone(
            prompt
                .header()
                .ingress_context
                .as_ref()
                .expect("process observation"),
        );
        let mut candidate = prompt
            .header()
            .authority_association
            .as_ref()
            .expect("claims")
            .candidate()
            .clone();
        candidate.target.logical_runtime = EvidenceId::new(runtime.to_string()).expect("runtime");
        let selection = candidate
            .controller_model
            .clone()
            .expect("selected controller");
        prompt.header_mut().authority_association =
            Some(InputAuthorityAssociation::new(candidate).expect("claims"));
        let context = NativeIngressContext::from_trusted_ingress(
            &prompt,
            original_ingress.requester().clone(),
            original_ingress.ingress_actor().clone(),
            original_ingress.realm().clone(),
            original_ingress.authentication().clone(),
        )
        .expect("actual current ingress")
        .with_controller_client(
            &prompt,
            ControllerModelClient::new(selection.clone(), Arc::new(SelectedClient(selection))),
        )
        .expect("actual selected child");
        let prompt = prompt
            .with_ingress_context(context)
            .expect("exact final input");
        (machine, session, prompt)
    }

    async fn stage_controller_input(
        machine: &MeerkatMachine,
        session: &SessionId,
        prompt: Input,
    ) -> (
        meerkat_core::InputId,
        meerkat_core::RunId,
        WorkAuthorizationContext,
    ) {
        let driver = Arc::clone(
            &machine
                .sessions
                .read()
                .await
                .get(session)
                .expect("actual entry")
                .driver,
        );
        let id = prompt.id().clone();
        let run = meerkat_core::RunId::new();
        let context = {
            let mut locked = driver.lock().await;
            let DriverEntry::Ephemeral(driver) = &mut *locked else {
                panic!("storeless")
            };
            driver.set_executor_work_authorization_support(true);
            driver
                .accept_input(prompt)
                .await
                .expect("actual native acceptance");
            let context = driver
                .batch_work_authorization(&run, std::slice::from_ref(&id))
                .expect("structural batch before staging")
                .expect("governed context");
            assert!(context.controller_client().is_some());
            driver
                .contract_begin_run_authority(run.clone())
                .expect("actual generated Prepare");
            driver
                .machine_realize_authorized_stage_batch(
                    crate::meerkat_machine::driver::test_authorized_stage_for_run(
                        vec![id.clone()],
                        run.clone(),
                    ),
                )
                .expect("actual generated StageForRun");
            context
        };
        (id, run, context)
    }

    fn install_owner_fixture_credential(machine: &MeerkatMachine, prompt: &Input) {
        // This fixture tests native row/currentness, not vault persistence or
        // transport. Initial synthetic credentials still use the real owner.
        let credential = prompt
            .header()
            .ingress_context
            .as_ref()
            .expect("actual ingress")
            .controller_client()
            .expect("selected client")
            .selection()
            .credential();
        meerkat_core::publish_token_lifecycle_acquired_for_identity(
            &machine.generated_auth_lease_handle(),
            credential,
            &meerkat_core::auth::PersistedTokens::api_key("synthetic-native-owner-fixture"),
        )
        .expect("actual initial credential owner");
    }

    async fn accepted() -> (
        MeerkatMachine,
        SessionId,
        meerkat_core::InputId,
        meerkat_core::RunId,
        WorkAuthorizationContext,
        ResourceDomain,
    ) {
        let (configuration, prompt, domain) = configuration();
        let (machine, session, prompt) = pending_controller_input(configuration, prompt).await;
        install_owner_fixture_credential(&machine, &prompt);
        let (id, run, context) = stage_controller_input(&machine, &session, prompt).await;
        (machine, session, id, run, context, domain)
    }

    fn binding(
        context: &WorkAuthorizationContext,
        run: &meerkat_core::RunId,
        domain: &ResourceDomain,
    ) -> PreparedAuthorizationBinding {
        PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
            operation_id: meerkat_core::OperationId::new(),
            execution_scope: context.execution_scope().clone(),
            run_id: Some(run.clone()),
            context_revision: None,
            operation: AuthorizationOperation::Source(SourceAuthorizationFacts {
                target: SourceAuthorizationTarget::External(OwnerQualifiedTarget {
                    authority: domain.authority.clone(),
                    namespace: domain.namespace.clone().into(),
                    id: "record".into(),
                }),
                usage: SourceAuthorizationUse::Read,
            }),
        })
    }

    #[tokio::test]
    async fn actual_native_rows_and_grants_compile_then_generated_run_end_refuses() {
        let (machine, session, id, run, context, domain) = accepted().await;
        let bound = binding(&context, &run, &domain);
        let prepared = context
            .authorization()
            .prepare(&bound)
            .expect("real accepted row and current owners");
        prepared
            .check_current(&bound)
            .expect("same running native owner");
        let driver = Arc::clone(
            &machine
                .sessions
                .read()
                .await
                .get(&session)
                .expect("entry")
                .driver,
        );
        {
            let mut locked = driver.lock().await;
            let DriverEntry::Ephemeral(driver) = &mut *locked else {
                panic!("storeless")
            };
            driver
                .abandon_all_non_terminal(crate::input_state::InputAbandonReason::Stopped)
                .expect("actual terminal input transition");
            assert!(
                driver
                    .input_is_terminal_by_authority(&id)
                    .expect("generated terminality")
            );
        }
        prepared
            .check_current(&bound)
            .expect("delivery terminality does not end the admitted run");
        context
            .authorization()
            .prepare(&bound)
            .expect("terminal original remains the exact admitted contributor");
        let mut forbidden_domain = domain.clone();
        forbidden_domain.namespace = "other-source".into();
        let refused = binding(&context, &run, &forbidden_domain);
        assert!(
            matches!(context.authorization().prepare(&refused), Err(meerkat_core::OperationAuthorizationError::Refused(error)) if error.kind() == OperationRefusalKind::Denied)
        );
        context
            .authorization()
            .prepare(&bound)
            .expect("operation-local refusal did not end controller or run custody");
        {
            let locked = driver.lock().await;
            let authority = locked.shared_dsl_authority();
            let mut owner = authority.lock().expect("generated owner");
            dsl::MeerkatMachineMutator::apply(
                &mut *owner,
                dsl::MeerkatMachineInput::RunCompleted {
                    run_id: dsl::RunId::from_domain(&run),
                },
            )
            .expect("actual generated run terminal");
            assert_eq!(owner.state().lifecycle_phase, dsl::MeerkatPhase::Running);
        }
        assert!(
            prepared.check_current(&bound).is_err(),
            "same run terminal scalar ends warm custody"
        );
        assert!(
            context.authorization().prepare(&bound).is_err(),
            "historical terminal contributor cannot reopen the run"
        );
        prepared
            .observe(
                &bound,
                OperationObservation::Outcome(
                    meerkat_core::authorization::OperationObservedOutcome::TransportError,
                ),
            )
            .expect("actual late outcome remains observable after run end");
    }

    #[tokio::test]
    async fn current_check_is_exact_without_reentering_driver_custody() {
        let (machine, session, _, run, context, domain) = accepted().await;
        let bound = binding(&context, &run, &domain);
        let prepared = context.authorization().prepare(&bound).expect("current");
        let wrong = binding(&context, &meerkat_core::RunId::new(), &domain);
        assert!(prepared.check_current(&wrong).is_err());
        let driver = Arc::clone(
            &machine
                .sessions
                .read()
                .await
                .get(&session)
                .expect("entry")
                .driver,
        );
        let locked = driver.lock().await;
        prepared
            .check_current(&bound)
            .expect("entry reads its native owner without the driver gate");
        drop(locked);
        prepared
            .check_current(&bound)
            .expect("same native facts after releasing driver custody");
    }

    #[tokio::test]
    async fn retained_preparation_cannot_outlive_the_actual_machine_owner() {
        let (machine, _, _, run, context, domain) = accepted().await;
        let bound = binding(&context, &run, &domain);
        let prepared = context
            .authorization()
            .prepare(&bound)
            .expect("current owner");
        drop(machine);
        assert!(prepared.check_current(&bound).is_err());
        assert!(context.authorization().prepare(&bound).is_err());
    }

    #[tokio::test]
    async fn persisted_record_does_not_reconstruct_actual_controller_custody() {
        let (machine, session, id, _, _, _) = accepted().await;
        let driver = Arc::clone(
            &machine
                .sessions
                .read()
                .await
                .get(&session)
                .expect("entry")
                .driver,
        );
        let locked = driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &*locked else {
            panic!("storeless")
        };
        let stored = driver.stored_input_state(&id).expect("actual retained row");
        assert!(stored.state.controller_client.is_some());
        let bytes = serde_json::to_vec(&stored).expect("store row");
        let recovered: crate::input_state::StoredInputState =
            serde_json::from_slice(&bytes).expect("historical retained row");
        assert!(recovered.state.controller_client.is_none());
        assert!(
            recovered
                .state
                .persisted_input
                .as_ref()
                .expect("payload")
                .header()
                .ingress_context
                .is_none()
        );
        assert_eq!(
            recovered.state.authority_contributors,
            stored.state.authority_contributors
        );
        let candidate =
            crate::input_state::InputStatePersistenceRecord::from_machine_snapshot(stored.clone())
                .expect("actual generated persistence seed")
                .into_stored();
        assert!(
            candidate.state.controller_client.is_none(),
            "memory-store clone must not retain a runnable client"
        );
        assert!(
            candidate
                .state
                .persisted_input
                .as_ref()
                .expect("payload")
                .header()
                .ingress_context
                .is_none()
        );
        assert!(
            stored.state.controller_client.is_some(),
            "preparing a store candidate preserves live custody"
        );
    }

    #[test]
    fn already_shared_empty_machine_cannot_install_a_new_authority_profile() {
        let machine = MeerkatMachine::ephemeral();
        let retained = machine.clone();
        assert!(
            machine
                .with_local_grant_authorization(configuration().0)
                .is_err()
        );
        drop(retained);
    }

    // Red-first integration probes. Native accepted rows, current generated
    // grants, publication and final PreparedOperationCheck are production code.
    // Only the application account/route entitlement is a fixture owner.
    struct MutableControllerAccount {
        enabled: std::sync::atomic::AtomicBool,
        domain: ResourceDomain,
        executor: PrincipalRef,
    }

    impl OperationPolicyOwner for MutableControllerAccount {
        fn authorize_controller_admission(
            &self,
            association: &InputAuthorityAssociation,
            facts: &meerkat_core::ControllerModelFacts,
            _: u64,
        ) -> Result<
            meerkat_authorization::grant_policy::ControllerAdmissionAllowance,
            meerkat_core::OperationAuthorizationError,
        > {
            if !self.enabled.load(std::sync::atomic::Ordering::SeqCst)
                || association.candidate().logical_executor != self.executor
                || association.candidate().controller_model.as_ref() != Some(facts.selection())
                || facts.endpoint() != "https://controller.invalid/model"
                || facts.wire_model() != facts.selection().model()
            {
                return Err(denied().into());
            }
            Ok(
                meerkat_authorization::grant_policy::ControllerAdmissionAllowance {
                    operation_values: vec![LocalOperationValues {
                        action: ActionRef {
                            feature: "native-test".into(),
                            action: "infer".into(),
                        },
                        resource_domain: self.domain.clone(),
                        processor: ProcessorRef::Principal {
                            principal: self.executor.clone(),
                        },
                        audience: AudienceRef::Principal {
                            principal: self.executor.clone(),
                        },
                    }],
                    restrictions: ExecutionRestrictions::unrestricted(),
                },
            )
        }
        fn authorize_operation(
            &self,
            _: &InputAuthorityAssociation,
            binding: &PreparedAuthorizationBinding,
            purpose: LocalPolicyPurpose,
            _: u64,
        ) -> Result<LocalPolicyAllowance, meerkat_core::OperationAuthorizationError> {
            if !self.enabled.load(std::sync::atomic::Ordering::SeqCst)
                || purpose != LocalPolicyPurpose::Controller
                || !matches!(&binding.facts().operation, AuthorizationOperation::Model(_))
            {
                return Err(denied().into());
            }
            Ok(LocalPolicyAllowance {
                operation_values: vec![LocalOperationValues {
                    action: ActionRef {
                        feature: "native-test".into(),
                        action: "infer".into(),
                    },
                    resource_domain: self.domain.clone(),
                    processor: ProcessorRef::Principal {
                        principal: self.executor.clone(),
                    },
                    audience: AudienceRef::Principal {
                        principal: self.executor.clone(),
                    },
                }],
                restrictions: ExecutionRestrictions::unrestricted(),
                expires_at_ms: 10_000,
            })
        }
    }

    fn mutable_controller_configuration(
        enabled: bool,
    ) -> (
        NativeGrantWorkConfiguration,
        Input,
        Arc<MutableControllerAccount>,
        LocalAuthorizationPublication,
    ) {
        let (mut configuration, mut prompt, domain) = configuration();
        let mut candidate = prompt
            .header()
            .authority_association
            .as_ref()
            .expect("claims")
            .candidate()
            .clone();
        let root = candidate.controller_grant_lineage[0].root_authority.clone();
        let publication = LocalAuthorizationPublication::new();
        let grants = Arc::new(
            LocalGrantAuthority::new(
                LocalGrantConfiguration {
                    root: root.clone(),
                    namespace: EvidenceId::new("controller").expect("id"),
                    generation: 1,
                },
                publication.clone(),
                Arc::new(Clock),
            )
            .expect("actual generated grant owner"),
        );
        candidate.controller_grant_lineage = vec![
            grants
                .issue_root(
                    &root,
                    EvidenceId::new("controller-leaf").expect("id"),
                    candidate.logical_executor.clone(),
                    None,
                    ExecutionRestrictions::unrestricted(),
                )
                .expect("nonexpiring controller"),
        ];
        let account = Arc::new(MutableControllerAccount {
            enabled: std::sync::atomic::AtomicBool::new(enabled),
            domain,
            executor: candidate.logical_executor.clone(),
        });
        prompt.header_mut().authority_association =
            Some(InputAuthorityAssociation::new(candidate).expect("claims"));
        configuration.grants = grants;
        configuration.operation_owner = account.clone();
        (configuration, prompt, account, publication)
    }

    fn plain_controller_binding(
        context: &WorkAuthorizationContext,
        run: &meerkat_core::RunId,
    ) -> PreparedAuthorizationBinding {
        let selected = context
            .controller_client()
            .expect("actual retained client")
            .selection();
        PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
            operation_id: meerkat_core::OperationId::new(),
            execution_scope: context.execution_scope().clone(),
            run_id: Some(run.clone()),
            context_revision: None,
            operation: AuthorizationOperation::Model(
                meerkat_core::authorization::ModelAuthorizationFacts {
                    identity: Arc::new(meerkat_core::SessionLlmIdentity {
                        model: selected.model().into(),
                        provider: selected.provider(),
                        self_hosted_server_id: selected.self_hosted_server_id().map(str::to_owned),
                        provider_params: None,
                        auth_binding: selected.auth_binding().cloned(),
                    }),
                    wire_model: selected.model().into(),
                    hosted_capabilities: Arc::from([]),
                    backend_profile_id: Some(selected.backend_profile_id().into()),
                    backend_kind: selected.backend_kind().into(),
                    endpoint: "https://controller.invalid/model".into(),
                    credential: Some(selected.credential().clone()),
                    usage: meerkat_core::authorization::ModelAuthorizationUse::ControllerInference,
                    live_channel: None,
                },
            ),
        })
    }

    #[tokio::test]
    async fn governed_admission_refuses_currently_denied_controller_account() {
        let (configuration, prompt, _, _) = mutable_controller_configuration(false);
        let (machine, session, prompt) = pending_controller_input(configuration, prompt).await;
        // Actual generated credential positive control; policy denial must be decisive.
        install_owner_fixture_credential(&machine, &prompt);
        let driver = Arc::clone(
            &machine
                .sessions
                .read()
                .await
                .get(&session)
                .expect("session")
                .driver,
        );
        let mut locked = driver.lock().await;
        let DriverEntry::Ephemeral(driver) = &mut *locked else {
            panic!("storeless")
        };
        driver.set_executor_work_authorization_support(true);
        assert!(
            driver.accept_input(prompt).await.is_err(),
            "a selected client plus controller grant cannot admit a currently denied account/route"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn ordinary_account_policy_mutation_cannot_remove_the_live_controller() {
        controller_policy::assert_controller_veto_preserves_policy_and_publication().await;
    }

    #[tokio::test]
    async fn short_native_owner_contention_does_not_refuse_unchanged_work() {
        let (machine, session, _, run, context, domain) = accepted().await;
        let binding = binding(&context, &run, &domain);
        let check = meerkat_core::authorization::PreparedOperationCheck::prepare(context, binding)
            .expect("current owners");
        let driver = Arc::clone(
            &machine
                .sessions
                .read()
                .await
                .get(&session)
                .expect("session")
                .driver,
        );
        let authority = { driver.lock().await.shared_dsl_authority() };
        let held = authority
            .lock()
            .expect("actual generated owner, no mutation");
        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(1);
        let (result_tx, result_rx) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            started_tx.send(()).expect("check worker started");
            result_tx
                .send(check.current().map(|_| ()))
                .expect("result receiver");
        });
        started_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("worker scheduled");
        let premature = result_rx.recv_timeout(std::time::Duration::from_millis(30));
        drop(held);
        let result = match premature {
            Ok(result) => result,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => result_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .expect("owner released"),
            Err(error) => panic!("check worker disconnected: {error}"),
        };
        worker.join().expect("check worker joined");
        assert!(
            result.is_ok(),
            "lock contention is not a change or loss of authority"
        );
    }

    // Both supported owned clear routes operate on their actual credential
    // identity. Account coverage must not fabricate a Binding from its alias.
    #[cfg(not(target_arch = "wasm32"))]
    fn controller_configuration_for_owned_clear(
        binding_route: bool,
    ) -> (
        NativeGrantWorkConfiguration,
        Input,
        meerkat_core::AuthBindingRef,
    ) {
        let (configuration, mut prompt, _, _) = mutable_controller_configuration(true);
        let mut candidate = prompt
            .header()
            .authority_association
            .as_ref()
            .unwrap()
            .candidate()
            .clone();
        let selected = candidate.controller_model.as_ref().unwrap();
        let binding = meerkat_core::AuthBindingRef {
            realm: meerkat_core::RealmId::parse("native-clear-test").unwrap(),
            binding: meerkat_core::connection::BindingId::parse(format!(
                "clear-{}",
                uuid::Uuid::new_v4()
            ))
            .unwrap(),
            profile: None,
            origin: meerkat_core::connection::BindingOrigin::Configured,
        };
        let credential = if binding_route {
            meerkat_core::AuthCredentialIdentity::from_auth_binding(&binding)
        } else {
            meerkat_core::AuthCredentialIdentity::Account(meerkat_core::CredentialAccountRef {
                realm: binding.realm.clone(),
                account: meerkat_core::CredentialAccountId::parse(format!(
                    "clear-account-{}",
                    uuid::Uuid::new_v4()
                ))
                .unwrap(),
            })
        };
        candidate.controller_model = Some(ControllerModelSelection::new(
            meerkat_core::SessionLlmIdentity {
                model: selected.model().into(),
                provider: selected.provider(),
                self_hosted_server_id: selected.self_hosted_server_id().map(str::to_owned),
                provider_params: None,
                auth_binding: Some(binding.clone()),
            },
            credential,
            selected.backend_profile_id().into(),
            selected.backend_kind().into(),
        ));
        prompt.header_mut().authority_association =
            Some(InputAuthorityAssociation::new(candidate).unwrap());
        (configuration, prompt, binding)
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn shared_token_clear_api_cannot_delete_the_live_controller_credential() {
        use meerkat_auth_core::auth_store::InMemoryCoordinator;
        use meerkat_core::auth::{
            PersistedAuthMode, PersistedTokens, ProviderAuthPersistence, TokenKey, TokenStore,
        };
        for binding_route in [true, false] {
            let (configuration, prompt, selected_binding) =
                controller_configuration_for_owned_clear(binding_route);
            let (machine, session, prompt) = pending_controller_input(configuration, prompt).await;
            let credential = prompt
                .header()
                .ingress_context
                .as_ref()
                .expect("actual ingress")
                .controller_client()
                .expect("actual selected client")
                .selection()
                .credential()
                .clone();
            let key = TokenKey::from_credential_identity(&credential);
            let tokens = PersistedTokens {
                auth_mode: PersistedAuthMode::ApiKey,
                primary_secret: Some("synthetic-controller-fixture".into()),
                refresh_token: None,
                id_token: None,
                expires_at: None,
                last_refresh: None,
                scopes: Vec::new(),
                account_id: None,
                metadata: serde_json::Value::Null,
            };
            let handle = machine.generated_auth_lease_handle();
            let transition = meerkat_core::publish_token_lifecycle_acquired_for_identity(
                &handle,
                &credential,
                &tokens,
            )
            .expect("actual AuthMachine credential acquisition");
            let tokens = meerkat_core::mark_tokens_lifecycle_published_for_transition(
                &key,
                &tokens,
                &transition,
            )
            .expect("actual lifecycle-marked credential");
            let store = Arc::new(CredentialClearGateStore::new(false));
            store.save(&key, &tokens).await.expect("real memory vault");
            let before_bytes = serde_json::to_vec(&store.load(&key).await.expect("vault read"))
                .expect("encode stored tokens");
            let lease_key = meerkat_core::handles::LeaseKey::from_credential_identity(&credential);
            let before_snapshot = handle.snapshot(&lease_key);
            let (id, _, _) = stage_controller_input(&machine, &session, prompt).await;
            let persistence =
                ProviderAuthPersistence::new(store.clone(), Arc::new(InMemoryCoordinator::new()));
            let result = if binding_route {
                assert_eq!(
                    credential,
                    meerkat_core::AuthCredentialIdentity::from_auth_binding(&selected_binding)
                );
                meerkat_core::clear_tokens_and_publish_lifecycle_released_coordinated(
                    persistence,
                    handle.clone(),
                    selected_binding,
                )
                .await
            } else {
                assert!(matches!(
                    &credential,
                    meerkat_core::AuthCredentialIdentity::Account(_)
                ));
                meerkat_core::clear_tokens_and_publish_lifecycle_released_coordinated_for_identity(
                    persistence,
                    handle.clone(),
                    credential.clone(),
                )
                .await
            };
            assert!(
                matches!(
                    &result,
                    Err(meerkat_core::auth::CredentialMutationError::AuthLifecycle(
                        _
                    ))
                ),
                "native veto must retain its coordinated lifecycle error class: {result:?}"
            );
            let refused = result.is_err();
            assert!(
                refused,
                "controller credential removal must refuse before physical vault mutation"
            );
            assert_eq!(
                store.clears.load(std::sync::atomic::Ordering::SeqCst),
                0,
                "native custody must veto before TokenStore::clear, not compensate later"
            );
            assert_eq!(
                serde_json::to_vec(&store.load(&key).await.expect("vault inspection"))
                    .expect("encode retained tokens"),
                before_bytes,
                "all token bytes and the actual lifecycle marker remain unchanged"
            );
            assert_eq!(
                handle.snapshot(&lease_key),
                before_snapshot,
                "phase, presence, generation, expiry and publication marker remain unchanged"
            );
            let driver = Arc::clone(
                &machine
                    .sessions
                    .read()
                    .await
                    .get(&session)
                    .expect("session")
                    .driver,
            );
            let driver = driver.lock().await;
            let DriverEntry::Ephemeral(driver) = &*driver else {
                panic!("storeless")
            };
            assert!(
                driver.ledger().get(&id).is_some(),
                "only the administrative operation was refused"
            );
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    use meerkat_core::auth::TokenStore as _;

    #[cfg(not(target_arch = "wasm32"))]
    struct CredentialClearGateStore {
        actual: meerkat_auth_core::auth_store::EphemeralTokenStore,
        block_clear: bool,
        entered: tokio::sync::Semaphore,
        proceed: tokio::sync::Semaphore,
        clears: std::sync::atomic::AtomicUsize,
    }
    #[cfg(not(target_arch = "wasm32"))]
    impl CredentialClearGateStore {
        fn new(block_clear: bool) -> Self {
            Self {
                actual: meerkat_auth_core::auth_store::EphemeralTokenStore::new(),
                block_clear,
                entered: tokio::sync::Semaphore::new(0),
                proceed: tokio::sync::Semaphore::new(0),
                clears: std::sync::atomic::AtomicUsize::new(0),
            }
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    #[async_trait::async_trait]
    impl meerkat_core::auth::TokenStore for CredentialClearGateStore {
        async fn load(
            &self,
            key: &meerkat_core::auth::TokenKey,
        ) -> Result<Option<meerkat_core::auth::PersistedTokens>, meerkat_core::auth::TokenStoreError>
        {
            self.actual.load(key).await
        }
        async fn save(
            &self,
            key: &meerkat_core::auth::TokenKey,
            tokens: &meerkat_core::auth::PersistedTokens,
        ) -> Result<(), meerkat_core::auth::TokenStoreError> {
            self.actual.save(key, tokens).await
        }
        async fn clear(
            &self,
            key: &meerkat_core::auth::TokenKey,
        ) -> Result<(), meerkat_core::auth::TokenStoreError> {
            self.clears
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self.block_clear {
                self.entered.add_permits(1);
                self.proceed
                    .acquire()
                    .await
                    .expect("test clear gate")
                    .forget();
            }
            self.actual.clear(key).await
        }
        async fn list(
            &self,
        ) -> Result<Vec<meerkat_core::auth::TokenKey>, meerkat_core::auth::TokenStoreError>
        {
            self.actual.list().await
        }
        fn backend_name(&self) -> &'static str {
            "native-credential-clear-gate-test"
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn clear_first_cannot_accept_stale_controller_between_release_and_vault_commit() {
        use meerkat_auth_core::auth_store::InMemoryCoordinator;
        use meerkat_core::auth::{PersistedTokens, ProviderAuthPersistence, TokenKey, TokenStore};
        use std::sync::atomic::Ordering;
        use std::task::Poll;
        use std::time::Duration;

        struct ReleaseOnDrop<'a>(&'a tokio::sync::Semaphore);
        impl Drop for ReleaseOnDrop<'_> {
            fn drop(&mut self) {
                self.0.add_permits(1);
            }
        }
        for binding_route in [true, false] {
            let (configuration, prompt, selected_binding) =
                controller_configuration_for_owned_clear(binding_route);
            let (machine, session, prompt) = pending_controller_input(configuration, prompt).await;
            let credential = prompt
                .header()
                .ingress_context
                .as_ref()
                .unwrap()
                .controller_client()
                .unwrap()
                .selection()
                .credential()
                .clone();
            let key = TokenKey::from_credential_identity(&credential);
            let lease_key = meerkat_core::handles::LeaseKey::from_credential_identity(&credential);
            let handle = machine.generated_auth_lease_handle();
            let tokens = PersistedTokens::api_key("synthetic-clear-first-controller");
            let acquired = meerkat_core::publish_token_lifecycle_acquired_for_identity(
                &handle,
                &credential,
                &tokens,
            )
            .unwrap();
            let tokens = meerkat_core::mark_tokens_lifecycle_published_for_transition(
                &key, &tokens, &acquired,
            )
            .unwrap();
            let store = Arc::new(CredentialClearGateStore::new(true));
            store.save(&key, &tokens).await.unwrap();
            let before_bytes = serde_json::to_vec(&store.load(&key).await.unwrap()).unwrap();
            let driver = Arc::clone(&machine.sessions.read().await.get(&session).unwrap().driver);
            let id = prompt.id().clone();
            {
                let mut locked = driver.lock().await;
                let DriverEntry::Ephemeral(driver) = &mut *locked else {
                    panic!("storeless fixture")
                };
                driver.set_executor_work_authorization_support(true);
            }
            let release_on_drop = ReleaseOnDrop(&store.proceed);
            let mut clear = Box::pin(async {
                let persistence = ProviderAuthPersistence::new(
                    store.clone(),
                    Arc::new(InMemoryCoordinator::new()),
                );
                if binding_route {
                    assert_eq!(
                        credential,
                        meerkat_core::AuthCredentialIdentity::from_auth_binding(&selected_binding)
                    );
                    meerkat_core::clear_tokens_and_publish_lifecycle_released_coordinated(
                        persistence,
                        handle.clone(),
                        selected_binding.clone(),
                    )
                    .await
                } else {
                    assert!(matches!(
                        &credential,
                        meerkat_core::AuthCredentialIdentity::Account(_)
                    ));
                    meerkat_core::clear_tokens_and_publish_lifecycle_released_coordinated_for_identity(
                        persistence, handle.clone(), credential.clone(),
                    ).await
                }
            });
            tokio::time::timeout(Duration::from_secs(10), async {
                tokio::select! {
                    biased;
                    result = &mut clear => panic!("clear ended before its actual TokenStore gate: {result:?}"),
                    entered = store.entered.acquire() => entered.expect("test entry gate").forget(),
                }
            }).await.expect("actual vault-clear entry must be reached");
            // This signal comes from TokenStore::clear after real Release, not
            // from task startup. No timeout is used as evidence of admission.
            let during_snapshot = handle.snapshot(&lease_key);
            let during_bytes = serde_json::to_vec(&store.load(&key).await.unwrap()).unwrap();
            let mut admission = Box::pin(async {
                let mut locked = driver.lock().await;
                let DriverEntry::Ephemeral(driver) = &mut *locked else {
                    panic!("storeless fixture")
                };
                let accepted = driver.accept_input(prompt).await;
                (accepted, driver.ledger().get(&id).is_some())
            });
            let early = std::future::poll_fn(|cx| {
                Poll::Ready(match std::future::Future::poll(admission.as_mut(), cx) {
                    Poll::Ready(result) => Some(result),
                    Poll::Pending => None,
                })
            })
            .await;
            drop(release_on_drop);
            tokio::time::timeout(Duration::from_secs(10), &mut clear)
                .await
                .expect("clear completes")
                .expect("clear succeeds");
            let (accepted, recorded) = match early {
                Some(result) => result,
                None => tokio::time::timeout(Duration::from_secs(10), &mut admission)
                    .await
                    .expect("admission settles after clear"),
            };
            assert!(
                !during_snapshot.credential_present,
                "the actual AuthMachine Release preceded the gate"
            );
            assert_eq!(
                during_bytes, before_bytes,
                "vault bytes were still present at the race point"
            );
            assert_eq!(store.clears.load(Ordering::SeqCst), 1);
            assert!(store.load(&key).await.unwrap().is_none());
            assert!(
                accepted.is_err(),
                "an old selected client cannot admit work after its real credential Release"
            );
            assert!(
                !recorded,
                "no accepted row may retain the stale controller pin"
            );
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    mod storage_races {
        use super::*;
        use meerkat_auth_core::{EphemeralTokenStore, FileTokenStore, InMemoryCoordinator};
        use meerkat_core::auth::{
            PersistedAuthMode, PersistedTokens, ProviderAuthPersistence, TokenKey, TokenStore,
            TokenStoreError,
        };
        use meerkat_core::handles::{AuthLeasePhase, LeaseKey};
        use std::future::Future;
        use std::pin::Pin;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        use std::task::Poll;
        use std::time::Duration;
        use tokio::sync::Semaphore;

        // This decorator gates actual TokenStore calls. Its flags choose injected
        // I/O failures only; neither flags nor counters participate in authority.
        struct StorageRaceStore {
            actual: Arc<dyn TokenStore>,
            directory: Option<tempfile::TempDir>,
            gate_save: bool,
            gate_clear: bool,
            fail_save: AtomicBool,
            fail_clear: bool,
            save_entered: Semaphore,
            save_proceed: Semaphore,
            clear_entered: Semaphore,
            clear_proceed: Semaphore,
            saves: AtomicUsize,
            clears: AtomicUsize,
        }

        impl StorageRaceStore {
            fn new(gate_save: bool, fail_save: bool, fail_clear: bool) -> Arc<Self> {
                Arc::new(Self {
                    actual: Arc::new(EphemeralTokenStore::new()),
                    directory: None,
                    gate_save,
                    gate_clear: !gate_save,
                    fail_save: AtomicBool::new(fail_save),
                    fail_clear,
                    save_entered: Semaphore::new(0),
                    save_proceed: Semaphore::new(0),
                    clear_entered: Semaphore::new(0),
                    clear_proceed: Semaphore::new(0),
                    saves: AtomicUsize::new(0),
                    clears: AtomicUsize::new(0),
                })
            }
        }

        impl StorageRaceStore {
            fn file_backed(fail_clear: bool) -> Arc<Self> {
                let directory = tempfile::tempdir().unwrap();
                let mut store = Self::new(false, false, fail_clear);
                let unique = Arc::get_mut(&mut store).unwrap();
                unique.actual = Arc::new(FileTokenStore::new(directory.path()));
                unique.directory = Some(directory);
                store
            }

            fn actual_file(&self, key: &TokenKey) -> std::path::PathBuf {
                self.directory
                    .as_ref()
                    .unwrap()
                    .path()
                    .join(key.realm().as_str())
                    .join(format!("{}.json", key.storage_stem()))
            }
        }

        #[async_trait::async_trait]
        impl TokenStore for StorageRaceStore {
            async fn load(
                &self,
                key: &TokenKey,
            ) -> Result<Option<PersistedTokens>, TokenStoreError> {
                self.actual.load(key).await
            }
            async fn save(
                &self,
                key: &TokenKey,
                tokens: &PersistedTokens,
            ) -> Result<(), TokenStoreError> {
                let first = self.saves.fetch_add(1, Ordering::SeqCst) == 0;
                if self.gate_save && first {
                    self.save_entered.add_permits(1);
                    self.save_proceed
                        .acquire()
                        .await
                        .expect("save gate")
                        .forget();
                }
                if self.fail_save.swap(false, Ordering::SeqCst) {
                    return Err(TokenStoreError::Io("injected first save failure".into()));
                }
                self.actual.save(key, tokens).await
            }
            async fn clear(&self, key: &TokenKey) -> Result<(), TokenStoreError> {
                self.clears.fetch_add(1, Ordering::SeqCst);
                if self.gate_clear {
                    self.clear_entered.add_permits(1);
                    self.clear_proceed
                        .acquire()
                        .await
                        .expect("clear gate")
                        .forget();
                }
                if self.fail_clear {
                    return Err(TokenStoreError::Io(
                        "injected clear failure before mutation".into(),
                    ));
                }
                self.actual.clear(key).await
            }
            async fn list(&self) -> Result<Vec<TokenKey>, TokenStoreError> {
                self.actual.list().await
            }
            fn backend_name(&self) -> &'static str {
                "controller-storage-race-test"
            }
        }

        struct ReleaseStorageGates(Arc<StorageRaceStore>);
        impl Drop for ReleaseStorageGates {
            fn drop(&mut self) {
                self.0.save_proceed.add_permits(1);
                self.0.clear_proceed.add_permits(1);
            }
        }

        async fn entered(gate: &Semaphore) {
            tokio::time::timeout(Duration::from_secs(10), gate.acquire())
                .await
                .expect("actual store call reached")
                .expect("gate open")
                .forget();
        }

        async fn poll_once<F: Future>(mut future: Pin<&mut F>) -> Option<F::Output> {
            std::future::poll_fn(|cx| {
                Poll::Ready(match future.as_mut().poll(cx) {
                    Poll::Ready(result) => Some(result),
                    Poll::Pending => None,
                })
            })
            .await
        }

        async fn finish<F: Future>(future: F) -> F::Output {
            tokio::time::timeout(Duration::from_secs(10), future)
                .await
                .expect("owner operation settles")
        }

        async fn fixture() -> (MeerkatMachine, SessionId, Input) {
            let (configuration, mut prompt, _, _) = mutable_controller_configuration(true);
            let mut candidate = prompt
                .header()
                .authority_association
                .as_ref()
                .unwrap()
                .candidate()
                .clone();
            let selected = candidate.controller_model.as_ref().unwrap();
            // The existing lifecycle guard is keyed globally by LeaseKey. Give each
            // case its own actual account, including parallel test executions.
            let credential =
                meerkat_core::AuthCredentialIdentity::Account(meerkat_core::CredentialAccountRef {
                    realm: meerkat_core::RealmId::parse("native-test").unwrap(),
                    account: meerkat_core::CredentialAccountId::parse(format!(
                        "storage-race-{}",
                        uuid::Uuid::new_v4()
                    ))
                    .unwrap(),
                });
            candidate.controller_model = Some(ControllerModelSelection::new(
                meerkat_core::SessionLlmIdentity {
                    model: selected.model().into(),
                    provider: selected.provider(),
                    self_hosted_server_id: selected.self_hosted_server_id().map(str::to_owned),
                    provider_params: None,
                    auth_binding: selected.auth_binding().cloned(),
                },
                credential,
                selected.backend_profile_id().into(),
                selected.backend_kind().into(),
            ));
            prompt.header_mut().authority_association =
                Some(InputAuthorityAssociation::new(candidate).unwrap());
            pending_controller_input(configuration, prompt).await
        }

        fn identity(prompt: &Input) -> meerkat_core::AuthCredentialIdentity {
            prompt
                .header()
                .ingress_context
                .as_ref()
                .unwrap()
                .controller_client()
                .unwrap()
                .selection()
                .credential()
                .clone()
        }

        async fn attempt(
            machine: &MeerkatMachine,
            session: &SessionId,
            prompt: Input,
        ) -> (bool, bool) {
            let driver = Arc::clone(&machine.sessions.read().await.get(session).unwrap().driver);
            let mut locked = driver.lock().await;
            let DriverEntry::Ephemeral(driver) = &mut *locked else {
                panic!("storeless fixture")
            };
            driver.set_executor_work_authorization_support(true);
            let id = prompt.id().clone();
            let accepted = driver.accept_input(prompt).await.is_ok();
            (accepted, driver.ledger().get(&id).is_some())
        }

        #[tokio::test]
        async fn status_cannot_restore_ready_during_coordinated_clear() {
            let (machine, session, prompt) = fixture().await;
            let credential = identity(&prompt);
            let key = TokenKey::from_credential_identity(&credential);
            let lease_key = LeaseKey::from_credential_identity(&credential);
            let handle = machine.generated_auth_lease_handle();
            let store = StorageRaceStore::new(false, false, false);
            let persistence =
                ProviderAuthPersistence::new(store.clone(), Arc::new(InMemoryCoordinator::new()));
            meerkat_auth_core::save_tokens_and_publish_lifecycle(
                persistence.clone(),
                handle.clone(),
                credential.clone(),
                PersistedTokens::api_key("synthetic-status-predecessor"),
            )
            .await
            .unwrap();
            // Positive control: the same status helper sees the actual marked
            // predecessor before any clear. The selected client is not the oracle.
            assert!(
                meerkat_core::rehydrate_marked_tokens_for_status_for_identity(
                    store.as_ref(),
                    &handle,
                    &credential,
                    PersistedAuthMode::ApiKey,
                    chrono::Utc::now(),
                )
                .await
                .unwrap()
                .is_some()
            );
            assert_eq!(
                handle
                    .resolve_credential_use_admission(
                        &lease_key,
                        meerkat_core::handles::CredentialUseIntent::HoldAuthority,
                    )
                    .unwrap(),
                meerkat_core::handles::CredentialUseDisposition::Authorized,
            );
            let before_bytes = serde_json::to_vec(&store.load(&key).await.unwrap()).unwrap();
            let cleanup = ReleaseStorageGates(store.clone());
            let clear = tokio::spawn(
                meerkat_core::clear_tokens_and_publish_lifecycle_released_coordinated_for_identity(
                    persistence,
                    handle.clone(),
                    credential.clone(),
                ),
            );
            entered(&store.clear_entered).await;
            let released = handle.snapshot(&lease_key);
            // The public snapshot intentionally projects Released as an absent
            // phase. Use the generated classifier for actual credential use.
            assert_eq!(released.phase, None);
            assert_eq!(
                handle
                    .resolve_credential_use_admission(
                        &lease_key,
                        meerkat_core::handles::CredentialUseIntent::HoldAuthority,
                    )
                    .unwrap(),
                meerkat_core::handles::CredentialUseDisposition::LeaseAbsent,
            );
            assert!(!released.credential_present);
            assert_eq!(
                serde_json::to_vec(&store.load(&key).await.unwrap()).unwrap(),
                before_bytes
            );

            let mut status = Box::pin(
                meerkat_core::rehydrate_marked_tokens_for_status_for_identity(
                    store.as_ref(),
                    &handle,
                    &credential,
                    PersistedAuthMode::ApiKey,
                    chrono::Utc::now(),
                ),
            );
            // Poll the real helper, not a task-start notification or a sleep. A
            // blocked helper is resumed after clear; an early completion is kept.
            let early_status = poll_once(status.as_mut()).await;
            let after_status_poll = handle.snapshot(&lease_key);
            let mut admission = Box::pin(attempt(&machine, &session, prompt));
            let early_admission = poll_once(admission.as_mut()).await;
            drop(cleanup);
            finish(clear).await.unwrap().unwrap();
            let status_result = match early_status {
                Some(result) => result,
                None => finish(status).await,
            };
            let admission_result = match early_admission {
                Some(result) => result,
                None => finish(admission).await,
            };

            assert_eq!(
                after_status_poll, released,
                "status cannot revive a released lease before physical clear"
            );
            assert!(
                !matches!(status_result, Ok(Some(_))),
                "status must not return a credential across successful clear; explicit refusal is valid"
            );
            assert!(
                meerkat_core::rehydrate_marked_tokens_for_status_for_identity(
                    store.as_ref(),
                    &handle,
                    &credential,
                    PersistedAuthMode::ApiKey,
                    chrono::Utc::now(),
                )
                .await
                .unwrap()
                .is_none(),
                "fresh status after clear observes the actual empty store"
            );
            assert_eq!(
                admission_result,
                (false, false),
                "stale selected client cannot publish a native row"
            );
            assert!(store.load(&key).await.unwrap().is_none());
            assert_eq!(handle.snapshot(&lease_key), released);
            assert_eq!(store.clears.load(Ordering::SeqCst), 1);
        }

        #[tokio::test]
        async fn acquired_but_uncommitted_credential_cannot_admit_controller() {
            for fail_save in [false, true] {
                let (machine, session, prompt) = fixture().await;
                let credential = identity(&prompt);
                let key = TokenKey::from_credential_identity(&credential);
                let lease_key = LeaseKey::from_credential_identity(&credential);
                let handle = machine.generated_auth_lease_handle();
                let store = StorageRaceStore::new(true, fail_save, false);
                let persistence = ProviderAuthPersistence::new(
                    store.clone(),
                    Arc::new(InMemoryCoordinator::new()),
                );
                let cleanup = ReleaseStorageGates(store.clone());
                let save = tokio::spawn(meerkat_auth_core::save_tokens_and_publish_lifecycle(
                    persistence.clone(),
                    handle.clone(),
                    credential.clone(),
                    PersistedTokens::api_key("synthetic-first-save"),
                ));
                entered(&store.save_entered).await;
                // The physical save has been reached. A correct implementation
                // may publish Acquire before or after committing these bytes;
                // neither staging choice may allow an unsupported controller.
                assert!(
                    store.load(&key).await.unwrap().is_none(),
                    "no candidate bytes committed"
                );
                let mut admission = Box::pin(attempt(&machine, &session, prompt.clone()));
                let early_admission = poll_once(admission.as_mut()).await;
                let admitted_before_commit = matches!(early_admission, Some((true, _)));
                drop(cleanup);
                let save_result = finish(save).await.unwrap();
                let admission_result = match early_admission {
                    Some(result) => result,
                    None => finish(admission).await,
                };
                if fail_save {
                    assert!(save_result.is_err(), "actual save failure propagates");
                    assert!(!handle.snapshot(&lease_key).credential_present);
                    assert!(store.load(&key).await.unwrap().is_none());
                    // No staged Acquire means no compensating clear is needed.
                    // The public owner and actual store must still have no credential.
                    assert_eq!(
                        admission_result,
                        (false, false),
                        "failed storage never leaves accepted work"
                    );
                    // Same exact input/account can be accepted after a real retry.
                    finish(meerkat_auth_core::save_tokens_and_publish_lifecycle(
                        persistence,
                        handle.clone(),
                        credential,
                        PersistedTokens::api_key("synthetic-committed-retry"),
                    ))
                    .await
                    .unwrap();
                    assert!(store.load(&key).await.unwrap().is_some());
                    assert_eq!(attempt(&machine, &session, prompt).await, (true, true));
                } else {
                    let committed = save_result.expect("actual successful save");
                    assert_eq!(
                        serde_json::to_vec(&store.load(&key).await.unwrap().unwrap()).unwrap(),
                        serde_json::to_vec(&committed).unwrap(),
                        "the committed marker and credential are exact"
                    );
                    assert!(handle.snapshot(&lease_key).credential_present);
                    assert_eq!(store.clears.load(Ordering::SeqCst), 0);
                    // A guarded waiter may accept after commit. A local in-progress
                    // refusal may be retried through the same real native owner.
                    let accepted_after_commit = if admission_result == (false, false) {
                        attempt(&machine, &session, prompt).await
                    } else {
                        admission_result
                    };
                    assert_eq!(accepted_after_commit, (true, true));
                }
                assert!(
                    !admitted_before_commit,
                    "Acquire alone must not publish a native row before TokenStore save"
                );
            }
        }

        #[tokio::test]
        async fn cancelling_coordinated_clear_keeps_owner_through_commit_or_rollback() {
            for fail_clear in [false, true] {
                let (machine, session, prompt) = fixture().await;
                let credential = identity(&prompt);
                let key = TokenKey::from_credential_identity(&credential);
                let lease_key = LeaseKey::from_credential_identity(&credential);
                let handle = machine.generated_auth_lease_handle();
                let store = StorageRaceStore::file_backed(fail_clear);
                let persistence = ProviderAuthPersistence::new(
                    store.clone(),
                    Arc::new(InMemoryCoordinator::new()),
                );
                meerkat_auth_core::save_tokens_and_publish_lifecycle(
                    persistence.clone(),
                    handle.clone(),
                    credential.clone(),
                    PersistedTokens::api_key("synthetic-cancel-predecessor"),
                )
                .await
                .unwrap();
                assert_eq!(
                    handle
                        .resolve_credential_use_admission(
                            &lease_key,
                            meerkat_core::handles::CredentialUseIntent::HoldAuthority,
                        )
                        .unwrap(),
                    meerkat_core::handles::CredentialUseDisposition::Authorized,
                );
                let before = handle.snapshot(&lease_key);
                let before_bytes = serde_json::to_vec(&store.load(&key).await.unwrap()).unwrap();
                let actual_file = store.actual_file(&key);
                let before_file = std::fs::read(&actual_file).unwrap();
                let cleanup = ReleaseStorageGates(store.clone());
                let caller = tokio::spawn(
                    meerkat_core::clear_tokens_and_publish_lifecycle_released_coordinated_for_identity(
                        persistence.clone(),
                        handle.clone(),
                        credential.clone(),
                    ),
                );
                entered(&store.clear_entered).await;
                let released = handle.snapshot(&lease_key);
                // The public snapshot intentionally projects Released as an absent
                // phase. Use the generated classifier for actual credential use.
                assert_eq!(released.phase, None);
                assert_eq!(
                    handle
                        .resolve_credential_use_admission(
                            &lease_key,
                            meerkat_core::handles::CredentialUseIntent::HoldAuthority,
                        )
                        .unwrap(),
                    meerkat_core::handles::CredentialUseDisposition::LeaseAbsent,
                );
                caller.abort();
                assert!(
                    finish(caller).await.unwrap_err().is_cancelled(),
                    "cancel actual coordinated caller"
                );

                let mut next_owner =
                    Box::pin(meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key));
                let premature_owner = poll_once(next_owner.as_mut()).await;
                let owner_was_held = premature_owner.is_none();
                // An unexpectedly early test guard must not itself block admission
                // and manufacture a passing negative. The final assertion retains
                // this custody failure independently of the native result.
                let mut next_owner = if let Some(owner) = premature_owner {
                    drop(owner);
                    Box::pin(meerkat_core::acquire_auth_login_lifecycle_guard(&lease_key))
                } else {
                    next_owner
                };
                let mut admission = Box::pin(attempt(&machine, &session, prompt.clone()));
                let early_admission = poll_once(admission.as_mut()).await;
                let admitted_before_settlement = matches!(early_admission, Some((true, _)));
                assert_eq!(
                    serde_json::to_vec(&store.load(&key).await.unwrap()).unwrap(),
                    before_bytes
                );
                assert_eq!(handle.snapshot(&lease_key), released);
                drop(cleanup);
                // Getting the actual existing lifecycle mutex proves its previous
                // owner ended; the store-entry signal alone cannot prove rollback.
                let owner = finish(next_owner.as_mut()).await;
                let settled = handle.snapshot(&lease_key);
                let settled_bytes = serde_json::to_vec(&store.load(&key).await.unwrap()).unwrap();
                drop(owner);
                let admission_result = match early_admission {
                    Some(result) => result,
                    None => finish(admission).await,
                };
                assert!(
                    !admitted_before_settlement,
                    "cancelled outer caller cannot expose native admission during clear or rollback"
                );
                if fail_clear {
                    let after_rollback = if admission_result == (false, false) {
                        attempt(&machine, &session, prompt).await
                    } else {
                        admission_result
                    };
                    assert_eq!(
                        after_rollback,
                        (true, true),
                        "same input is eligible only after exact rollback"
                    );
                } else {
                    assert_eq!(
                        admission_result,
                        (false, false),
                        "cleared credential cannot admit the stale pin"
                    );
                }
                assert!(
                    owner_was_held,
                    "cancelling waiter must not release live mutation custody"
                );
                if fail_clear {
                    assert_eq!(
                        settled, before,
                        "failed clear restores exact lifecycle before owner release"
                    );
                    assert_eq!(
                        settled_bytes, before_bytes,
                        "failed clear preserves exact predecessor bytes"
                    );
                } else {
                    assert_eq!(settled, released);
                    assert!(store.load(&key).await.unwrap().is_none());
                }
                if fail_clear {
                    assert_eq!(
                        std::fs::read(&actual_file).unwrap(),
                        before_file,
                        "real file predecessor remains byte-for-byte unchanged"
                    );
                } else {
                    assert!(
                        !actual_file.exists(),
                        "actual FileTokenStore deletion completed"
                    );
                }
                assert_eq!(store.clears.load(Ordering::SeqCst), 1);
                // Successful deletion has no admitted work. A later real writer
                // can use the same owner. The rollback variant already proved
                // liveness by admitting its exact retained input above; do not
                // turn this test into a policy for rotating an in-use credential.
                if !fail_clear {
                    finish(meerkat_auth_core::save_tokens_and_publish_lifecycle(
                        persistence,
                        handle.clone(),
                        credential,
                        PersistedTokens::api_key("synthetic-after-cancel"),
                    ))
                    .await
                    .unwrap();
                    assert!(handle.snapshot(&lease_key).credential_present);
                    assert_eq!(
                        store
                            .load(&key)
                            .await
                            .unwrap()
                            .unwrap()
                            .primary_secret
                            .as_deref(),
                        Some("synthetic-after-cancel")
                    );
                }
            }
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    mod direct_ingest;

    #[cfg(not(target_arch = "wasm32"))]
    mod controller_policy;

    #[cfg(not(target_arch = "wasm32"))]
    mod credential_custody;

    #[cfg(not(target_arch = "wasm32"))]
    mod preparation_contention;

    #[cfg(not(target_arch = "wasm32"))]
    mod batch_custody;

    // Uses the same native constructor as PersistenceBundle. The stock bundle's
    // missing early configuration hook is documented separately; no bundle is
    // unwrapped or replaced to simulate that public path here.
    fn memory_persistent_constructor_fixture()
    -> (MeerkatMachine, Arc<dyn crate::store::RuntimeStore>) {
        let store: Arc<dyn crate::store::RuntimeStore> =
            Arc::new(crate::store::InMemoryRuntimeStore::new());
        let blobs: Arc<dyn meerkat_core::BlobStore> =
            Arc::new(meerkat_store::MemoryBlobStore::new());
        let machine = MeerkatMachine::persistent(Arc::clone(&store), blobs);
        assert!(Arc::ptr_eq(
            machine
                .shared
                .store
                .as_ref()
                .expect("persistent store retained"),
            &store,
        ));
        (machine, store)
    }

    async fn assert_memory_persistent_registration(machine: &MeerkatMachine) {
        let session = SessionId::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            machine.register_session(session.clone()),
        )
        .await
        .expect("bounded actual registration")
        .expect("actual persistent registration");
        let driver = {
            let sessions = machine.sessions.read().await;
            let entry = sessions.get(&session).expect("registered native entry");
            assert_eq!(entry.runtime_id, LogicalRuntimeId::for_session(&session));
            Arc::clone(&entry.driver)
        };
        {
            let locked = driver.lock().await;
            assert!(
                matches!(&*locked, DriverEntry::Persistent(_)),
                "a memory RuntimeStore still uses the actual PersistentRuntimeDriver"
            );
        }
        let runtime = LogicalRuntimeId::for_session(&session);
        let store = machine
            .shared
            .store
            .as_ref()
            .expect("actual persistent store");
        let rows = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            store.load_input_states(&runtime),
        )
        .await
        .expect("bounded actual store read")
        .expect("actual registered runtime rows");
        assert!(rows.is_empty(), "registration alone admits no prompt");
    }

    #[tokio::test]
    async fn memory_persistent_constructor_uses_real_persistent_driver() {
        // Each test owns a fresh backing store. This control must not acquire
        // same-store custody that could mask the governed test's setup result.
        let (machine, _) = memory_persistent_constructor_fixture();
        assert_memory_persistent_registration(&machine).await;
    }

    #[tokio::test]
    async fn memory_persistent_constructor_accepts_governed_configuration() {
        let (mut machine, store) = memory_persistent_constructor_fixture();
        assert!(
            Arc::get_mut(&mut machine.shared).is_some(),
            "the setup attempt has exclusive actual machine ownership"
        );
        assert!(machine.sessions.read().await.is_empty());
        assert!(
            machine
                .shared
                .native_work_authorization_host
                .get()
                .is_none()
        );
        let (configuration, _, _) = configuration();
        let result = machine.with_local_grant_authorization(configuration);
        // Expected current RED: ValidationFailed from the store-presence guard.
        // A refusal is not the success oracle and no new API is required.
        let machine = result.expect("memory persistent owner must support configured governance");
        assert!(Arc::ptr_eq(machine.shared.store.as_ref().unwrap(), &store));
        assert!(
            machine
                .shared
                .native_work_authorization_host
                .get()
                .is_some()
        );
        assert_memory_persistent_registration(&machine).await;
    }

    #[cfg(not(target_arch = "wasm32"))]
    mod memory_persistence;
}
