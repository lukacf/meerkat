//! Installed canonical admission and pure work formatting. Agents choose tools.
use super::*;
use crate::{HostAdmissionContext, private_originals::PrivateOriginalRef};
use meerkat_authorization_contracts::work_association::InputAuthorityAssociation;
use meerkat_toolkit::{
    authority::{Action, Ceiling, SetupProposalConstraints, SetupRequestEvidence},
    request::{
        ContextReference, HostSetupRequest, RequestError, SetupLimits, TaskContinuation,
        ValidatedSetupRequest,
    },
    setup::{SetupWorkPreparation, canonical_setup_input_reference},
    work_admission::{HostWorkGraphScope, SetupWorkAdmissionError},
};
use toolkit_package::{ContentDigest, Revision, canonical_json};

// Mechanical retry correlation only. Both the actual invocation producer and
// historical envelope validation share this exact serialized tuple. This pure
// Connect-owned derivation grants no authority and is not a native input key.
pub(crate) fn installed_invocation_correlation(
    original: &PrivateOriginalRef,
    scope: &impl serde::Serialize,
    call_id: &str,
) -> Result<Id, ToolAdmissionError> {
    Id::new(format!(
        "call-{}",
        ContentDigest::of_bytes(
            &canonical_json(&("toolkit-installed-invocation-v1", original, scope, call_id,))
                .map_err(|_| ToolAdmissionError::Unavailable)?
        )
    ))
    .map_err(|_| ToolAdmissionError::Unavailable)
}

/// Only this installed source constructs the exact view passed to the existing
/// original invocation owner. It supplies actual native staged identity, not a task ID.
pub struct InstalledOriginalInvocation<'a> {
    reference: &'a PrivateOriginalRef,
    association: &'a InputAuthorityAssociation,
    context: &'a ToolDispatchContext,
    correlation: &'a Id,
    workflow_churn: &'a tokio::sync::Mutex<meerkat_core::authorization::OwnerChangeChurn>,
    workflow_deadline: tokio::time::Instant,
}
impl InstalledOriginalInvocation<'_> {
    pub fn original(&self) -> &PrivateOriginalRef {
        self.reference
    }
    pub fn association(&self) -> &InputAuthorityAssociation {
        self.association
    }
    pub fn context(&self) -> &ToolDispatchContext {
        self.context
    }
    /// Host-issued setup namespace correlation, never a native task or receipt.
    pub fn correlation(&self) -> &Id {
        self.correlation
    }
    // One invocation's delay budget, never permission or a reusable grant.
    pub(crate) fn workflow_observation_budget(
        &self,
    ) -> (
        &tokio::sync::Mutex<meerkat_core::authorization::OwnerChangeChurn>,
        tokio::time::Instant,
    ) {
        (self.workflow_churn, self.workflow_deadline)
    }

    /// Capture identity facts through the real native continuation owner while
    /// the fixture still retains its actual staged context and source owner.
    /// This does not admit an invocation or create work authorization.
    #[cfg(test)]
    pub(crate) async fn capture_native_continuation_for_test(
        owner: &super::continuation::InstalledNativeContinuation,
        originals: &crate::private_originals::PrivateOriginals,
        context: &ToolDispatchContext,
        call_id: &str,
    ) -> Result<(Id, TaskContinuation), ToolAdmissionError> {
        let original = originals
            .acquisition_original(context)
            .await
            .map_err(source_admission_error)?;
        let correlation = installed_invocation_correlation(
            &original.reference,
            context
                .work_authorization()
                .ok_or(ToolAdmissionError::Denied)?
                .execution_scope(),
            call_id,
        )?;
        let churn = tokio::sync::Mutex::new(meerkat_core::authorization::OwnerChangeChurn::new());
        // original_task reads identity only; it performs no workflow wait.
        let continuation = owner
            .original_task(InstalledOriginalInvocation {
                reference: &original.reference,
                association: original.original.association(),
                context,
                correlation: &correlation,
                workflow_churn: &churn,
                workflow_deadline: tokio::time::Instant::now(),
            })
            .await?;
        Ok((correlation, continuation))
    }
}

/// The original invocation owner issues and retains this task binding at actual
/// request admission, before an acquisition or result exists. The selected
/// adapter projects that exact binding and also implements existing completion
/// delivery. The later RetainedJobRecord::from_owner committed-result reader
/// cannot produce this initial binding. Replays must recover the same binding,
/// never derive task/delivery IDs from source hashes, call IDs or a new session.
/// Native Host-producer confirmation is still needed after the result commits.
#[async_trait]
pub trait InstalledOriginalTaskOwner: SetupContinuationDelivery {
    async fn original_task(
        &self,
        invocation: InstalledOriginalInvocation<'_>,
    ) -> Result<TaskContinuation, ToolAdmissionError>;

    /// One producer returns continuation and optional fan-out context together.
    /// Existing owners remain ungrouped. An override must recover the actual
    /// child launched for this exact invocation, not search by labels or run ID.
    async fn original_binding(
        &self,
        invocation: InstalledOriginalInvocation<'_>,
    ) -> Result<InstalledOriginalTaskBinding, ToolAdmissionError> {
        Ok(InstalledOriginalTaskBinding {
            continuation: self.original_task(invocation).await?,
            work_context: None,
            planning: None,
            context_transfer: None,
        })
    }
}

/// Produced by the same invocation owner in one lookup. WorkGraph validation
/// below checks persisted group facts; the producer owns their association with
/// this native input. The optional group supplies no native permission.
pub struct InstalledOriginalTaskBinding {
    pub continuation: TaskContinuation,
    pub work_context: Option<meerkat_toolkit::setup::SetupWorkContext>,
    /// Optional actual host-owned planning source. Explicit None preserves the
    /// singleton path; unavailable or conflicting Some is never downgraded.
    pub planning: Option<Arc<crate::private_originals::InstalledWorkflowPlanning>>,
    /// Explicit host permission for this exact invocation's inline request to
    /// its fixed G/P recipient. Planning selection alone never permits copying.
    /// Native current Find authority remains required at the durable commit.
    pub context_transfer: Option<Arc<crate::private_originals::InstalledWorkflowContextTransfer>>,
}

/// Trusted optional application configuration, not model input. These are
/// proposal bounds only. Current native grants authorize every eventual effect.
/// Native placement, policy, source, target and canonical inputs are derived.
pub struct InstalledSetupDefaults {
    pub helper: InstalledSetupSelection,
    /// Each selected FindAndInstall target also needs Delegate for that same
    /// member in these proposal bounds. The trusted application compiler owns
    /// this pair; it does not add or replace any native operation grant.
    pub proposal_ceiling: Ceiling,
    pub continuation: Arc<dyn InstalledOriginalTaskOwner>,
    pub work_backend: SetupWorkBackend,
    /// None supports inline content. External references need their real owner.
    pub references: Option<Arc<dyn ContextReferenceOwner>>,
    pub limits: InstalledSetupLimits,
    pub work_step: WorkAdmissionStepOptions,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledSetupLimits {
    pub setup_timeout_ms: u64,
    pub max_workers: u16,
    pub max_effect_attempts: u16,
    pub admission_timeout_ms: u64,
}
impl Default for InstalledSetupLimits {
    fn default() -> Self {
        Self {
            setup_timeout_ms: 3_600_000,
            max_workers: 1,
            max_effect_attempts: 3,
            admission_timeout_ms: 10_000,
        }
    }
}
impl InstalledSetupLimits {
    pub(super) fn validate(&self) -> Result<(), HostLifetimeError> {
        if self.setup_timeout_ms == 0
            || self.max_workers == 0
            || self.max_effect_attempts == 0
            || self.admission_timeout_ms == 0
            || self.admission_timeout_ms > 60_000
        {
            return Err(HostLifetimeError::InvalidSelection);
        }
        Ok(())
    }

    fn new_acquisition_deadline(
        &self,
        now_ms: u64,
        workflow_deadline: Option<u64>,
    ) -> Result<u64, ToolAdmissionError> {
        let configured = now_ms
            .checked_add(self.setup_timeout_ms)
            .ok_or(ToolAdmissionError::Unavailable)?;
        match workflow_deadline {
            Some(deadline) if deadline <= now_ms => Err(ToolAdmissionError::Denied),
            Some(deadline) => Ok(configured.min(deadline)),
            None => Ok(configured),
        }
    }
}
struct InstalledSetupAdmission {
    originals: Arc<PrivateOriginals>,
    store: Arc<SqliteStore>,
    scope: SetupScope,
    helpers: InstalledHelpers,
    ceiling: Ceiling,
    continuation: Arc<dyn InstalledOriginalTaskOwner>,
    limits: InstalledSetupLimits,
    backend: SetupWorkBackend,
}
struct InlineContext;
impl ContextReferenceOwner for InlineContext {
    fn verify_access(
        &self,
        _: &ContextReference,
        _: &SetupRequestEvidence,
        _: &meerkat_toolkit::authority::SetupUseContext<'_>,
    ) -> Result<(), RequestError> {
        Err(RequestError::ContextUnavailable)
    }
}

/// Reject an incoherent helper-publication bound before installed work starts.
/// InstalledSetupFacts derives the eventual request target from this exact
/// member string. Empty and non-acquisition ceilings keep their existing scope.
pub(super) fn validate_proposal_ceiling(
    native: &CompiledNativeHost,
    ceiling: &Ceiling,
) -> Result<(), HostLifetimeError> {
    for permission in &ceiling.actions {
        if permission.action != Action::FindAndInstall {
            continue;
        }
        if native
            .members
            .iter()
            .filter(|member| member.member.member.as_str() == permission.resource.as_str())
            .count()
            != 1
            || !ceiling.permits(Action::Delegate, &permission.resource)
        {
            return Err(HostLifetimeError::InvalidSelection);
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "admission/ceiling_tests.rs"]
mod ceiling_tests;

/// Pure metadata over the full canonical input already persisted by SetupStore.
/// No metadata field resolves intent, supplies permission or creates a helper.
pub struct InstalledSetupPreparer {
    scope: HostWorkGraphScope,
}
impl InstalledSetupPreparer {
    pub fn from_backend(backend: &SetupWorkBackend) -> Result<Self, SetupWorkAdmissionError> {
        Ok(Self {
            scope: HostWorkGraphScope::new(
                backend.pin().realm().clone(),
                backend.pin().namespace().clone(),
            )?,
        })
    }
}
impl SetupWorkPreparer for InstalledSetupPreparer {
    fn prepare(
        &self,
        request: &ValidatedSetupRequest,
    ) -> Result<SetupWorkPreparation, SetupWorkAdmissionError> {
        if &request.constraints().tenant != self.scope.realm() {
            return Err(SetupWorkAdmissionError::ScopeMismatch);
        }
        Ok(SetupWorkPreparation {
            scope: self.scope.clone(),
            input_reference: canonical_setup_input_reference(request).map_err(|_| SetupWorkAdmissionError::NativeExecutionUnavailable)?,
            work_request: meerkat_workgraph::CreateWorkItemRequest {
                realm_id: Some(self.scope.realm().to_string()), namespace: Some(self.scope.namespace().clone()),
                title: "Connect tools for the original request".into(),
                description: Some("Read the complete canonical request supplied with this work. Identify the integrations it needs using the available catalog and discovery tools, then follow the protected setup and approval owners. Report pending authentication and unavailable capabilities truthfully.".into()),
                ..Default::default()
            }, delegation: None,
        })
    }
}
impl InstalledAcquisition {
    pub(in crate::installed_host) fn from_defaults(
        store: Arc<SqliteStore>,
        source_scope: StoreScope,
        originals: Arc<PrivateOriginals>,
        native: &CompiledNativeHost,
        defaults: InstalledSetupDefaults,
        additional_helpers: Vec<InstalledSetupSelection>,
    ) -> Result<Self, HostLifetimeError> {
        let tools = InstalledAcquisitionTools::new(Duration::from_millis(
            defaults.limits.admission_timeout_ms,
        ))?;
        Self::from_defaults_with_tools_and_helpers(
            store,
            source_scope,
            originals,
            native,
            defaults,
            tools,
            additional_helpers,
        )
    }

    #[cfg(test)]
    pub(in crate::installed_host) fn from_defaults_with_tools(
        store: Arc<SqliteStore>,
        source_scope: StoreScope,
        originals: Arc<PrivateOriginals>,
        native: &CompiledNativeHost,
        defaults: InstalledSetupDefaults,
        tools: InstalledAcquisitionTools,
    ) -> Result<Self, HostLifetimeError> {
        Self::from_defaults_with_tools_and_helpers(
            store,
            source_scope,
            originals,
            native,
            defaults,
            tools,
            vec![],
        )
    }

    pub(in crate::installed_host) fn from_defaults_with_tools_and_helpers(
        store: Arc<SqliteStore>,
        source_scope: StoreScope,
        originals: Arc<PrivateOriginals>,
        native: &CompiledNativeHost,
        defaults: InstalledSetupDefaults,
        tools: InstalledAcquisitionTools,
        additional_helpers: Vec<InstalledSetupSelection>,
    ) -> Result<Self, HostLifetimeError> {
        defaults.limits.validate()?;
        validate_proposal_ceiling(native, &defaults.proposal_ceiling)?;
        let tenant =
            Id::new(source_scope.realm.clone()).map_err(|_| HostLifetimeError::InvalidSelection)?;
        let deployment =
            Id::new(source_scope.owner.clone()).map_err(|_| HostLifetimeError::InvalidSelection)?;
        let preparer = Arc::new(
            InstalledSetupPreparer::from_backend(&defaults.work_backend)
                .map_err(|_| HostLifetimeError::InvalidSelection)?,
        );
        let helpers =
            InstalledHelpers::new(defaults.helper.clone(), additional_helpers.clone(), native)?;
        let admission = Arc::new(InstalledSetupAdmission {
            originals: originals.clone(),
            store: store.clone(),
            scope: SetupScope::new(tenant.clone(), deployment),
            helpers,
            ceiling: defaults.proposal_ceiling,
            continuation: defaults.continuation.clone(),
            limits: defaults.limits.clone(),
            backend: defaults.work_backend.clone(),
        });
        Self::with_tools(
            store,
            source_scope,
            originals,
            native,
            InstalledAcquisitionOwners {
                tenant,
                helper: defaults.helper,
                admission,
                references: defaults
                    .references
                    .unwrap_or_else(|| Arc::new(InlineContext)),
                preparer,
                work_backend: defaults.work_backend,
                continuation: defaults.continuation,
                admission_timeout: Duration::from_millis(defaults.limits.admission_timeout_ms),
                work_step: defaults.work_step,
            },
            tools,
            additional_helpers,
        )
    }
}
#[async_trait]
impl SetupToolHost for InstalledSetupAdmission {
    async fn admit_invocation(
        &self,
        call_id: &str,
        context: &ToolDispatchContext,
    ) -> Result<HostToolAdmission, ToolAdmissionError> {
        if call_id.is_empty() || call_id.len() > 512 {
            return Err(ToolAdmissionError::Denied);
        }
        // One budget and churn owner for all pure workflow reads in this host
        // admission. The enclosing Find timeout remains the outer bound.
        let deadline =
            tokio::time::Instant::now() + Duration::from_millis(self.limits.admission_timeout_ms);
        let churn = tokio::sync::Mutex::new(meerkat_core::authorization::OwnerChangeChurn::new());
        // The original's basis reads wait for a held source collection, and a
        // conflict in the source's facts is waited out through Core's waiter
        // with this admission's churn, within the same admission's deadline
        // and never past Core's owner-change bound.
        let original = self
            .originals
            .acquisition_original_through_waiter(
                context,
                &churn,
                std::cmp::min(
                    deadline,
                    tokio::time::Instant::now()
                        + meerkat_core::authorization::OWNER_CHANGE_WAIT_BOUND,
                ),
            )
            .await
            .map_err(source_admission_error)
            .inspect_err(|error| crate::aa1_admission_failure("admission.original", error))?;
        let facts = self
            .originals
            .installed_setup_facts(&original.reference)
            .await
            .inspect_err(|error| crate::aa1_admission_failure("admission.setup_facts", error))?;
        let claims = original.original.association().candidate();
        if facts.realm != claims.ingress_namespace.realm
            || facts.target_member != claims.target.logical_owner
        {
            return Err(ToolAdmissionError::Denied);
        }
        let policy_revision = revision(&facts.policy)?;
        // Mechanical retry correlation, scoped by the actual retained native
        // original/input. It grants nothing and is not a continuation identity.
        let correlation = installed_invocation_correlation(
            &original.reference,
            context
                .work_authorization()
                .ok_or(ToolAdmissionError::Denied)?
                .execution_scope(),
            call_id,
        )
        .inspect_err(|error| crate::aa1_admission_failure("admission.correlation", error))?;
        let current = HostAdmissionContext {
            tenant: facts.tenant,
            deployment: facts.deployment,
            realm: facts.realm,
            target: claims.target.clone(),
            policy_revision: policy_revision.clone(),
            now_ms: facts.now_ms,
        };
        let authorization = context
            .work_authorization()
            .ok_or(ToolAdmissionError::Denied)?;
        let binding = self
            .continuation
            .original_binding(InstalledOriginalInvocation {
                reference: &original.reference,
                association: original.original.association(),
                context,
                correlation: &correlation,
                workflow_churn: &churn,
                workflow_deadline: deadline,
            })
            .await
            .inspect_err(|error| {
                crate::aa1_admission_failure("admission.original_binding", error)
            })?;
        let InstalledOriginalTaskBinding {
            continuation,
            work_context,
            planning,
            context_transfer,
        } = binding;
        // The actual host binding selects a late workflow receipt. Static
        // helpers keep their original installed artifact validation unchanged.
        let late = match &planning {
            Some(planning) => self
                .originals
                .observe_workflow_for_admission(
                    authorization.authorization().as_ref(),
                    &churn,
                    deadline,
                    || planning.late_helper_digest_observed(&self.originals, facts.helper.as_ref()),
                )
                .await
                .map_err(workflow_admission_error)
                .inspect_err(|error| {
                    crate::aa1_admission_failure("admission.late_helper", error)
                })?,
            None => None,
        };
        let environment_revision = if let Some(late) = late {
            revision(&(
                "toolkit-installed-late-workflow-v1",
                &facts.policy,
                late,
                &self.ceiling,
                &self.limits,
                self.backend.pin(),
            ))?
        } else {
            let (helper, selected_member) = self
                .helpers
                .get(facts.helper.as_ref())
                .map_err(|_| ToolAdmissionError::Denied)?;
            let native = facts
                .policy
                .native_host
                .as_ref()
                .ok_or(ToolAdmissionError::Denied)?;
            if helper
                .selected_member(native)
                .map_err(|_| ToolAdmissionError::Denied)?
                != *selected_member
            {
                return Err(ToolAdmissionError::Denied);
            }
            revision(&(
                "toolkit-installed-setup-v1",
                &facts.policy,
                helper,
                &self.ceiling,
                &self.limits,
                self.backend.pin(),
            ))?
        };
        if let Some(transfer) = &context_transfer {
            let selected = planning.as_ref().ok_or(ToolAdmissionError::Denied)?;
            self.originals
                .observe_workflow_for_admission(
                    authorization.authorization().as_ref(),
                    &churn,
                    deadline,
                    || {
                        transfer.check_invocation_observed(
                            &self.originals,
                            &original,
                            &correlation,
                            selected,
                            &self.backend,
                            &self.ceiling,
                        )
                    },
                )
                .await
                .map_err(workflow_admission_error)
                .inspect_err(|error| {
                    crate::aa1_admission_failure("admission.context_transfer", error)
                })?;
        }
        let environment_revision = match &planning {
            Some(planning) => revision(&(
                environment_revision,
                planning
                    .binding_digest()
                    .map_err(source_admission_error)
                    .inspect_err(|error| {
                        crate::aa1_admission_failure("admission.planning_digest", error)
                    })?,
            ))?,
            None => environment_revision,
        };
        let environment_revision = match &context_transfer {
            Some(transfer) => revision(&(
                environment_revision,
                transfer
                    .binding_digest()
                    .map_err(source_admission_error)
                    .inspect_err(|error| {
                        crate::aa1_admission_failure("admission.transfer_digest", error)
                    })?,
            ))?,
            None => environment_revision,
        };
        if continuation.native_original.is_none() {
            return Err(ToolAdmissionError::Denied);
        }
        if let Some(work) = &work_context {
            let native = continuation
                .native_original
                .as_ref()
                .ok_or(ToolAdmissionError::Denied)?;
            if Some(&native.retained_work) != context.retained_work()
                || Some(&native.origin_session_id) != context.origin_session_id()
            {
                return Err(ToolAdmissionError::Denied);
            }
            work.check_current(&self.backend)
                .await
                .map_err(admission_setup_error)
                .inspect_err(|error| {
                    crate::aa1_admission_failure("admission.work_current", error)
                })?;
        }
        let store = SetupStore::new_conditional(self.store.as_ref(), self.scope.clone())
            .inspect_err(|error| crate::aa1_setup_failure("admission.store", error))
            .map_err(|_| ToolAdmissionError::Unavailable)?;
        let outcome = store
            .invocation_terms(
                original.original.association(),
                &correlation,
                &current.borrowed(authorization),
            )
            .await;
        // Keep every private result behind the same installed owner, including
        // absence and redacted storage errors. The historical digest above
        // does not replace this current selected-host/source policy check.
        let after = self
            .originals
            .installed_setup_facts(&original.reference)
            .await
            .inspect_err(|error| {
                crate::aa1_admission_failure("admission.setup_facts_after", error)
            })?;
        if after.policy != facts.policy || after.now_ms < current.now_ms {
            return Err(ToolAdmissionError::Denied);
        }
        let existing = outcome
            .inspect_err(|error| crate::aa1_setup_failure("admission.invocation_terms", error))
            .map_err(admission_setup_error)?;
        let (host, evidence) = match existing {
            Some(terms) => {
                if terms.host.continuation != continuation
                    || terms.host.environment_revision != environment_revision
                    || terms.host.setup_helper != facts.helper
                    || terms.host.work_context != work_context
                    || terms.host.target != facts.target
                    || terms.evidence.constraints().ceiling != self.ceiling
                {
                    return Err(ToolAdmissionError::Denied);
                }
                (terms.host, terms.evidence)
            }
            None => {
                // A later waiter gets only the selected workflow's remaining
                // lifetime. Replays above keep their original persisted terms.
                let expires_at_ms = self.limits.new_acquisition_deadline(
                    current.now_ms,
                    planning
                        .as_ref()
                        .map(|selected| selected.terms().deadline_ms),
                )?;
                let evidence = SetupRequestEvidence::new(
                    original.original.association().clone(),
                    SetupProposalConstraints {
                        tenant: current.tenant.clone(),
                        deployment: current.deployment.clone(),
                        policy_revision,
                        issued_at_ms: current.now_ms,
                        expires_at_ms,
                        ceiling: self.ceiling.clone(),
                    },
                )
                .map_err(|_| ToolAdmissionError::Unavailable)
                .inspect_err(|error| {
                    crate::aa1_admission_failure("admission.request_evidence", error)
                })?;
                let host = HostSetupRequest {
                    target: facts.target,
                    correlation,
                    parent_operation: None,
                    setup_helper: facts.helper,
                    work_context: work_context.clone(),
                    continuation,
                    environment_revision,
                    limits: SetupLimits {
                        deadline_ms: expires_at_ms,
                        max_workers: self.limits.max_workers,
                        max_effect_attempts: self.limits.max_effect_attempts,
                    },
                };
                (host, evidence)
            }
        };
        let final_reference = &original.reference;
        let expected_policy = &facts.policy;
        let not_before = after.now_ms;
        let final_facts = self
            .originals
            .retain_workflow_binding_waiting(
                &original,
                &host.correlation,
                planning.as_ref(),
                context_transfer.as_ref(),
                work_context.as_ref(),
                authorization.authorization().as_ref(),
                &churn,
                deadline,
                || async move {
                    // Read the actual policy/source/clock again after a wait and
                    // before the final synchronous observation and retention.
                    let final_facts = self
                        .originals
                        .installed_setup_facts(final_reference)
                        .await
                        .inspect_err(|error| {
                            crate::aa1_admission_failure("admission.setup_facts_final", error)
                        })
                        .map_err(|error| {
                            crate::private_originals::WorkflowChildPreparationError::Refused(
                                match error {
                                    ToolAdmissionError::Denied => {
                                        crate::private_originals::PrivateOriginalError::Denied
                                    }
                                    ToolAdmissionError::Unavailable => {
                                        crate::private_originals::PrivateOriginalError::Unavailable
                                    }
                                },
                            )
                        })?;
                    if &final_facts.policy != expected_policy || final_facts.now_ms < not_before {
                        return Err(
                            crate::private_originals::WorkflowChildPreparationError::Refused(
                                crate::private_originals::PrivateOriginalError::Denied,
                            ),
                        );
                    }
                    Ok(final_facts)
                },
            )
            .await
            .map_err(workflow_admission_error)
            .inspect_err(|error| {
                crate::aa1_admission_failure("admission.retain_workflow", error)
            })?;
        Ok(HostToolAdmission {
            host,
            evidence,
            context: HostAdmissionContext {
                now_ms: final_facts.now_ms,
                ..current
            },
        })
    }
}
fn workflow_admission_error(
    error: crate::private_originals::WorkflowChildPreparationError,
) -> ToolAdmissionError {
    match error {
        crate::private_originals::WorkflowChildPreparationError::ObservationChanged => {
            ToolAdmissionError::Unavailable
        }
        crate::private_originals::WorkflowChildPreparationError::Refused(error) => {
            source_admission_error(error)
        }
    }
}
fn revision(value: &impl serde::Serialize) -> Result<Revision, ToolAdmissionError> {
    Revision::new(
        ContentDigest::of_bytes(
            &canonical_json(value)
                .map_err(|_| ToolAdmissionError::Unavailable)
                .inspect_err(|error| {
                    crate::aa1_admission_failure("admission.revision_encoding", error)
                })?,
        )
        .to_string(),
    )
    .map_err(|_| ToolAdmissionError::Unavailable)
    .inspect_err(|error| crate::aa1_admission_failure("admission.revision", error))
}
fn admission_setup_error(error: SetupError) -> ToolAdmissionError {
    match error {
        SetupError::Request(
            RequestError::Encoding(_)
            | RequestError::Authority(meerkat_toolkit::authority::SetupContextError::InvalidEncoding),
        ) => ToolAdmissionError::Unavailable,
        SetupError::Denied | SetupError::WrongScope | SetupError::Request(_) => {
            ToolAdmissionError::Denied
        }
        _ => ToolAdmissionError::Unavailable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_toolkit::authority::SetupContextError;

    #[test]
    fn new_acquisition_deadline_respects_workflow_and_configured_bounds() {
        let limits = InstalledSetupLimits {
            setup_timeout_ms: 540_000,
            ..Default::default()
        };
        // Actual cold-P failure: fixed bootstrap A called Find after G's
        // commissioning, so a fresh full timeout exceeded G by 40,698 ms.
        let now = 1_792_817_729_101;
        let workflow_deadline = 1_792_818_228_403;
        assert_eq!(
            limits
                .new_acquisition_deadline(now, Some(workflow_deadline))
                .unwrap(),
            workflow_deadline
        );
        assert_eq!(
            limits
                .new_acquisition_deadline(now + 120_000, Some(workflow_deadline))
                .unwrap(),
            workflow_deadline,
            "a later waiter cannot extend the same workflow"
        );
        assert_eq!(
            limits
                .new_acquisition_deadline(now, Some(now + 540_000))
                .unwrap(),
            now + 540_000
        );
        assert_eq!(
            limits
                .new_acquisition_deadline(now, Some(now + 600_000))
                .unwrap(),
            now + 540_000,
            "workflow selection cannot extend the configured maximum"
        );
        assert_eq!(
            limits.new_acquisition_deadline(now, None).unwrap(),
            now + 540_000,
            "ordinary singleton retains its configured timeout"
        );
    }

    #[test]
    fn new_acquisition_deadline_refuses_expired_workflow_and_preserves_overflow() {
        let limits = InstalledSetupLimits::default();
        for deadline in [0, 99, 100] {
            assert!(matches!(
                limits.new_acquisition_deadline(100, Some(deadline)),
                Err(ToolAdmissionError::Denied)
            ));
        }
        assert_eq!(
            limits.new_acquisition_deadline(100, Some(101)).unwrap(),
            101
        );
        for selection in [None, Some(u64::MAX)] {
            assert!(matches!(
                limits.new_acquisition_deadline(u64::MAX - 1, selection),
                Err(ToolAdmissionError::Unavailable)
            ));
        }
    }

    #[test]
    fn canonical_request_encoding_is_unavailable_and_actual_refusals_stay_denied() {
        for error in [
            RequestError::Encoding(<serde_json::Error as serde::ser::Error>::custom(
                "private canonical encoding detail",
            )),
            RequestError::Authority(SetupContextError::InvalidEncoding),
        ] {
            assert!(matches!(
                admission_setup_error(SetupError::Request(error)),
                ToolAdmissionError::Unavailable,
            ));
        }
        for error in [
            RequestError::EmptyRequest,
            RequestError::InputLimit,
            RequestError::InvalidLimits,
            RequestError::ContextUnavailable,
            RequestError::ChangedAuthority,
            RequestError::InvalidAudience,
            RequestError::Authority(SetupContextError::MigrationRequired),
            RequestError::Authority(SetupContextError::InvalidEvidence),
            RequestError::Authority(SetupContextError::WrongContext),
            RequestError::Authority(SetupContextError::Expired),
            RequestError::Authority(SetupContextError::StalePolicy),
            RequestError::Authority(SetupContextError::Denied),
        ] {
            assert!(matches!(
                admission_setup_error(SetupError::Request(error)),
                ToolAdmissionError::Denied,
            ));
        }
        for error in [SetupError::Denied, SetupError::WrongScope] {
            assert!(matches!(
                admission_setup_error(error),
                ToolAdmissionError::Denied
            ));
        }
    }
}
