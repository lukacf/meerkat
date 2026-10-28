//! Protected observations from the actual grant-backed work compiler.
//!
//! The sink belongs to the work's native audit owner: the native input row,
//! or for a governed service call the machine's service-call audit log. This
//! module has no audit registry, permission lookup, replay reducer,
//! durability claim, or exporter fallback.

use std::sync::Arc;

use meerkat_authorization_contracts::audit::{
    AuditDelegateAttribution, AuditDelegateRole, AuditModelTarget, AuditModelUse, AuditObservation,
    AuditPolicyRead, AuditPublicationMode, AuditRecipient, AuditRefusalBasis, AuditRefusalRead,
    AuditReviewAttribution, AuditReviewRole, AuditSourceUse, AuditTarget, AuditToolOwner,
    AuthorizationAuditObservation, AuthorizationAuditSink,
};
use meerkat_authorization_contracts::constraints::ResourceDomain;
use meerkat_authorization_contracts::evidence::EvidenceDigest;
use meerkat_authorization_contracts::resource::ResourceRef;
use meerkat_authorization_contracts::work_association::InputAuthorityAssociation;
use meerkat_core::authorization::{
    AuthorizationOperation, ModelAuthorizationFacts, ModelAuthorizationUse,
    OperationAuthorizationError, OperationObservation, OperationObservationError,
    OperationRefusalKind, OperationRefused, PreparedAuthorizationBinding,
    PreparedOperationAuthorization, PublicationMode, PublicationRecipient,
    SourceAuthorizationTarget, SourceAuthorizationUse, ToolAuthorizationTarget, WorkAuthorization,
};

use crate::publication::{LocalAuthorizationPublication, PublicationError};
use crate::work::LocalAuthorizationClock;

/// Only the grant-backed factory builds this adapter after selecting the real
/// compiler and the work's native sink (row-bound, or the service-call audit
/// log's). All authority still lives in `inner`.
pub(crate) struct AuditedWorkAuthorization {
    pub(crate) inner: Arc<dyn WorkAuthorization>,
    pub(crate) associations: Arc<[InputAuthorityAssociation]>,
    pub(crate) publication: LocalAuthorizationPublication,
    pub(crate) clock: Arc<dyn LocalAuthorizationClock>,
    pub(crate) sink: Arc<dyn AuthorizationAuditSink>,
}

impl WorkAuthorization for AuditedWorkAuthorization {
    fn owner_change_in_progress(
        &self,
    ) -> Option<meerkat_core::authorization::OwnerChangeInProgress> {
        self.inner.owner_change_in_progress()
    }

    fn run_if_current_within(
        &self,
        binding: &PreparedAuthorizationBinding,
        prepared: Option<&meerkat_core::authorization::OperationValidity>,
        further: &(dyn std::any::Any + Send + Sync),
        operation: &mut dyn FnMut(),
    ) -> Result<(), OperationAuthorizationError> {
        self.inner
            .run_if_current_within(binding, prepared, further, operation)
    }

    fn run_entered_if_current_within(
        &self,
        binding: &PreparedAuthorizationBinding,
        prepared: Option<&meerkat_core::authorization::OperationValidity>,
        further: &(dyn std::any::Any + Send + Sync),
        operation: &mut dyn FnMut(),
        entered_tier: meerkat_core::authorization::OperationReviewTier,
    ) -> Result<(), OperationAuthorizationError> {
        self.inner.run_entered_if_current_within(
            binding,
            prepared,
            further,
            operation,
            entered_tier,
        )
    }

    fn validate_child_issuance(
        &self,
        further: &(dyn std::any::Any + Send + Sync),
        intent: &(dyn std::any::Any + Send + Sync),
    ) -> Result<(), OperationAuthorizationError> {
        self.inner.validate_child_issuance(further, intent)
    }

    fn issue_child_if_current_within(
        &self,
        binding: &PreparedAuthorizationBinding,
        prepared: Option<&meerkat_core::authorization::OperationValidity>,
        further: &(dyn std::any::Any + Send + Sync),
        intent: &(dyn std::any::Any + Send + Sync),
        entered_tier: meerkat_core::authorization::OperationReviewTier,
    ) -> Result<Box<dyn std::any::Any + Send>, OperationAuthorizationError> {
        self.inner
            .issue_child_if_current_within(binding, prepared, further, intent, entered_tier)
    }

    fn review_operation_context(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<
        meerkat_core::authorization::ReviewOperationContextResolution,
        OperationAuthorizationError,
    > {
        self.inner.review_operation_context(binding)
    }

    fn controller_model_selection(&self) -> Option<meerkat_core::ControllerModelSelection> {
        self.inner.controller_model_selection()
    }

    /// Append to the same row-bound sink as every prepared observation. No
    /// policy read, preparation or clock: the fact is historical.
    fn observe_mandatory_hook_withholding(
        &self,
        record: &meerkat_core::authorization::MandatoryHookWithholdingRecord,
    ) -> Result<(), OperationObservationError> {
        self.sink.append(AuthorizationAuditObservation {
            operation_id: record.operation_id().clone(),
            execution_scope: record.execution_scope().clone(),
            run_id: record.run_id().cloned(),
            context_revision: None,
            review_attribution: None,
            delegate_attribution: None,
            observation: AuditObservation::MandatoryHookWithheld {
                withholding: record.withholding().clone(),
            },
        })
    }

    fn prepare(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
        self.prepare_observed(binding).result
    }

    fn prepare_observed(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> meerkat_core::authorization::ObservedAuthorizationResult<
        Arc<dyn PreparedOperationAuthorization>,
    > {
        let mut policy_observation = None;
        let result =
            (|| -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
                let target = Arc::new(target(binding));
                // On a concurrent owner change the publication contract is: "The
                // caller may rebuild once; it must not relabel old facts with a
                // newer stamp" (publication.rs). So a changed attempt is rebuilt
                // once, from a fresh clock read, and only the final attempt is
                // classified and recorded below, under its own stamp.
                let mut rebuilt = false;
                let (now, known_failure, started, observed) = loop {
                    let now = match self.clock.now() {
                        Ok(now) => now,
                        Err(_) => {
                            self.sink.append(observation(
                                binding,
                                AuditObservation::AuthorizationUnavailable { target },
                            ))?;
                            return Err(OperationAuthorizationError::Unavailable);
                        }
                    };
                    // This enclosing observation binds the audit's diagnostic revision to
                    // the same successful compiler read. It cannot retag an old decision.
                    // A changed stamp invalidates policy data, but must not erase a known
                    // infrastructure failure or turn it into another refusal to observe.
                    let mut known_failure = None;
                    // The sequence read just before this attempt, for the read
                    // of a refusal if the publication changes under it. The
                    // attempt's own first read can be one change newer.
                    let started = self.publication.current_read();
                    let observed = self.publication.observe(|| {
                        let result = self.inner.prepare(binding);
                        if matches!(
                            result,
                            Err(OperationAuthorizationError::Unavailable
                                | OperationAuthorizationError::ObservationUnavailable(_))
                        ) {
                            known_failure = result.as_ref().err().copied();
                        }
                        result
                    });
                    // A known infrastructure failure is never retried, and a
                    // second change refuses below.
                    if !rebuilt
                        && known_failure.is_none()
                        && matches!(observed, Err(PublicationError::Changed))
                    {
                        rebuilt = true;
                        continue;
                    }
                    break (now, known_failure, started, observed);
                };
                let (result, read) = if let Some(error) = known_failure {
                    (Err(error), None)
                } else {
                    match observed {
                        Ok((result, stamp)) => {
                            policy_observation = Some(stamp.policy_observation());
                            (
                                result,
                                Some(AttemptRead::Coherent {
                                    instance: stamp.instance(),
                                    sequence: stamp.observation_sequence(),
                                }),
                            )
                        }
                        Err(PublicationError::Changed) => (
                            Err(
                                OperationRefused::new(OperationRefusalKind::ReprepareRequired)
                                    .into(),
                            ),
                            started.map(|(instance, sequence)| AttemptRead::Changed {
                                instance,
                                sequence,
                                live: self.publication.current_read().map(|(_, live)| live),
                            }),
                        ),
                        Err(PublicationError::Unavailable) => {
                            (Err(OperationAuthorizationError::Unavailable), None)
                        }
                    }
                };
                match result {
                    Ok(inner) => {
                        let selected = self.associations.first().and_then(|association| {
                            association.candidate().controller_model.as_ref()
                        });
                        let controller = matches!(&binding.facts().operation,
                    AuthorizationOperation::Model(facts) if crate::work::controller_route(facts, selected));
                        let operation = !matches!(&binding.facts().operation,
                    AuthorizationOperation::Model(facts) if crate::work::controller_route(facts, selected) && facts.hosted_capabilities.is_empty());
                        let Some(AttemptRead::Coherent { instance, sequence }) = read else {
                            return Err(denied().into());
                        };
                        let policy = AuditPolicyRead {
                            publication_sequence: sequence,
                            observed_at_ms: now.unix_ms,
                            operation_authorities: if operation {
                                self.associations
                                    .iter()
                                    .map(|item| item.candidate().authority_basis.clone())
                                    .collect()
                            } else {
                                Vec::new()
                            },
                            controller_lineages: if controller {
                                self.associations
                                    .iter()
                                    .map(|item| item.candidate().controller_grant_lineage.clone())
                                    .collect()
                            } else {
                                Vec::new()
                            },
                            publication_instance: Some(instance),
                        };
                        self.sink.append(observation(
                            binding,
                            AuditObservation::Prepared {
                                target: Arc::clone(&target),
                                policy,
                            },
                        ))?;
                        Ok(Arc::new(AuditedPrepared {
                            inner,
                            policy_observation,
                            binding: binding.clone(),
                            target,
                            sink: Arc::clone(&self.sink),
                            publication: self.publication.clone(),
                            clock: Arc::clone(&self.clock),
                        }))
                    }
                    Err(OperationAuthorizationError::Unavailable) => {
                        self.sink.append(observation(
                            binding,
                            AuditObservation::AuthorizationUnavailable { target },
                        ))?;
                        Err(OperationAuthorizationError::Unavailable)
                    }
                    Err(OperationAuthorizationError::Refused(refusal)) => {
                        self.sink.append(observation(
                            binding,
                            AuditObservation::Refused {
                                target,
                                reason: refusal.kind(),
                                read: read.map(|read| read.refusal_read(now.unix_ms)),
                            },
                        ))?;
                        Err(refusal.into())
                    }
                    Err(error @ OperationAuthorizationError::ObservationUnavailable(_)) => {
                        Err(error)
                    }
                }
            })();
        meerkat_core::authorization::ObservedAuthorizationResult {
            result,
            policy: policy_observation,
        }
    }
}

/// The publication read of one audited preparation attempt.
#[derive(Clone, Copy)]
enum AttemptRead {
    /// A coherent observation: the owners answered at `sequence`.
    Coherent { instance: uuid::Uuid, sequence: u64 },
    /// The publication changed under an attempt that started at `sequence`;
    /// `live` is the sequence once the attempt ended.
    Changed {
        instance: uuid::Uuid,
        sequence: u64,
        live: Option<u64>,
    },
}

impl AttemptRead {
    /// The audit read of a refusal decided at this read.
    fn refusal_read(self, observed_at_ms: u64) -> AuditRefusalRead {
        match self {
            Self::Coherent { instance, sequence } => AuditRefusalRead::new(
                observed_at_ms,
                AuditRefusalBasis::OwnerAnswer,
                sequence,
                None,
                instance,
            ),
            Self::Changed {
                instance,
                sequence,
                live,
            } => AuditRefusalRead::new(
                observed_at_ms,
                if live.is_some_and(|live| !live.is_multiple_of(2)) {
                    AuditRefusalBasis::ChangeInProgress
                } else {
                    AuditRefusalBasis::PublicationChanged
                },
                sequence,
                live,
                instance,
            ),
        }
    }
}

struct AuditedPrepared {
    inner: Arc<dyn PreparedOperationAuthorization>,
    policy_observation: Option<meerkat_core::authorization::PolicyPublicationObservation>,
    binding: PreparedAuthorizationBinding,
    target: Arc<AuditTarget>,
    sink: Arc<dyn AuthorizationAuditSink>,
    publication: LocalAuthorizationPublication,
    clock: Arc<dyn LocalAuthorizationClock>,
}

impl AuditedPrepared {
    /// The read of a refusal observed at this decision's check. A refusal to
    /// rebuild means the publication moved after the decision's own read
    /// ([`PreparedOperationAuthorization::observe`] refuses another binding
    /// before it records anything), so it reads as changed against the live
    /// sequence, as at prepare time. Every other refusal is the owners'
    /// answer at the decision's own coherent read. `None` when the clock
    /// cannot be read.
    fn refusal_read(&self, reason: OperationRefusalKind) -> Option<AuditRefusalRead> {
        let meerkat_core::authorization::PolicyPublicationObservation::LocalPublication {
            instance,
            sequence,
        } = self.policy_observation?;
        let now = self.clock.now().ok()?;
        let read = if reason == OperationRefusalKind::ReprepareRequired {
            AttemptRead::Changed {
                instance,
                sequence,
                live: self.publication.current_read().map(|(_, live)| live),
            }
        } else {
            AttemptRead::Coherent { instance, sequence }
        };
        Some(read.refusal_read(now.unix_ms))
    }
}

impl PreparedOperationAuthorization for AuditedPrepared {
    fn policy_observation(
        &self,
    ) -> Option<meerkat_core::authorization::PolicyPublicationObservation> {
        self.policy_observation
    }

    fn review_tier(&self) -> meerkat_core::authorization::OperationReviewTier {
        self.inner.review_tier()
    }

    fn validity(&self) -> Option<meerkat_core::authorization::OperationValidity> {
        self.inner.validity()
    }

    fn check_current(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<(), meerkat_core::OperationAuthorizationError> {
        // No observation allocation, serialization, hash, or mutex here.
        self.inner.check_current(binding)
    }

    fn observe(
        &self,
        binding: &PreparedAuthorizationBinding,
        event: OperationObservation,
    ) -> Result<(), OperationObservationError> {
        if !self.binding.same_operation(binding) {
            return Err(OperationObservationError);
        }
        let event = match event {
            OperationObservation::ReviewAttemptStarted { attempt_ref } => {
                AuditObservation::ReviewAttemptStarted {
                    attempt_ref: attempt_ref.to_string(),
                }
            }
            OperationObservation::Entry(boundary) => AuditObservation::Entry {
                boundary: Some(boundary),
            },
            OperationObservation::AuthorizationUnavailable => {
                AuditObservation::AuthorizationUnavailable {
                    target: Arc::clone(&self.target),
                }
            }
            OperationObservation::Outcome(outcome) => AuditObservation::Outcome { outcome },
            OperationObservation::Refused(reason) => AuditObservation::Refused {
                target: Arc::clone(&self.target),
                reason,
                read: self.refusal_read(reason),
            },
        };
        self.sink.append(observation(binding, event))
    }
}

fn observation(
    binding: &PreparedAuthorizationBinding,
    event: AuditObservation,
) -> AuthorizationAuditObservation {
    let facts = binding.facts();
    AuthorizationAuditObservation {
        operation_id: facts.operation_id.clone(),
        execution_scope: facts.execution_scope.clone(),
        run_id: facts.run_id.clone(),
        context_revision: facts
            .context_revision
            .as_ref()
            .map(|revision| revision.as_str().to_owned()),
        review_attribution: binding
            .review_attribution()
            .map(|link| AuditReviewAttribution {
                candidate_operation_id: link
                    .origin()
                    .candidate_binding()
                    .facts()
                    .operation_id
                    .clone(),
                attempt_ref: link.origin().attempt_ref().to_string(),
                role: match link.role() {
                    meerkat_core::approval::review::ReviewOperationRole::ContextRead => {
                        AuditReviewRole::ContextRead
                    }
                    meerkat_core::approval::review::ReviewOperationRole::ReviewerInference => {
                        AuditReviewRole::ReviewerInference
                    }
                },
            }),
        delegate_attribution: binding
            .delegate_attribution()
            .map(|link| AuditDelegateAttribution {
                parent_operation_id: link.parent_binding().facts().operation_id.clone(),
                role: match link.role() {
                    meerkat_core::authorization::ToolDelegateRole::HostedSearchInference => {
                        AuditDelegateRole::HostedSearchInference
                    }
                },
            }),
        observation: event,
    }
}

fn model_target(facts: &ModelAuthorizationFacts) -> AuditModelTarget {
    AuditModelTarget {
        model: facts.identity.model.clone(),
        provider: facts.identity.provider,
        self_hosted_server_id: facts.identity.self_hosted_server_id.clone(),
        auth_binding: facts.identity.auth_binding.clone(),
        credential: facts.credential.clone(),
        wire_model: facts.wire_model.to_string(),
        backend_profile_id: facts.backend_profile_id.as_deref().map(str::to_owned),
        backend_kind: facts.backend_kind.to_string(),
        endpoint: facts.endpoint.to_string(),
        hosted_capabilities: facts.hosted_capabilities.to_vec(),
        usage: match facts.usage {
            ModelAuthorizationUse::Inference => AuditModelUse::Inference,
            ModelAuthorizationUse::ControllerInference => AuditModelUse::ControllerInference,
            ModelAuthorizationUse::Compaction => AuditModelUse::Compaction,
            ModelAuthorizationUse::Live => AuditModelUse::Live,
        },
    }
}

fn source_use(usage: SourceAuthorizationUse) -> AuditSourceUse {
    match usage {
        SourceAuthorizationUse::Read => AuditSourceUse::Read,
        SourceAuthorizationUse::Hydrate => AuditSourceUse::Hydrate,
        SourceAuthorizationUse::Retain => AuditSourceUse::Retain,
    }
}

fn target(binding: &PreparedAuthorizationBinding) -> AuditTarget {
    match &binding.facts().operation {
        AuthorizationOperation::Model(facts) => AuditTarget::Model(model_target(facts)),
        AuthorizationOperation::Tool(facts) => match &facts.target {
            ToolAuthorizationTarget::Dispatcher(plan) => AuditTarget::Tool {
                call_id: facts.call_id.to_string(),
                tool_name: facts.name.to_string(),
                arguments_digest: EvidenceDigest::of_bytes(facts.arguments.get().as_bytes()),
                owners: plan
                    .owner_witnesses()
                    .iter()
                    .map(|owner| AuditToolOwner {
                        authority_key: owner.authority_key().to_owned(),
                        owner_key: owner.owner_key().to_owned(),
                    })
                    .collect(),
            },
            ToolAuthorizationTarget::ProviderHosted(model) => AuditTarget::ProviderHostedTool {
                call_id: facts.call_id.to_string(),
                tool_name: facts.name.to_string(),
                target: model_target(model),
            },
        },
        AuthorizationOperation::Source(facts) => {
            let usage = source_use(facts.usage);
            match &facts.target {
                SourceAuthorizationTarget::Blob(reference) => AuditTarget::Blob {
                    reference: reference.clone(),
                    usage,
                },
                SourceAuthorizationTarget::Memory(scope) => AuditTarget::Memory {
                    scope: scope.clone(),
                    usage,
                },
                SourceAuthorizationTarget::Transcript { session_id, range } => {
                    AuditTarget::Transcript {
                        session_id: session_id.clone(),
                        range: *range,
                        usage,
                    }
                }
                SourceAuthorizationTarget::RuntimeInput {
                    owner_session_id,
                    runtime_epoch_id,
                    input_id,
                } => AuditTarget::RuntimeInput {
                    owner_session_id: owner_session_id.clone(),
                    runtime_epoch_id: runtime_epoch_id.clone(),
                    input_id: input_id.clone(),
                    usage,
                },
                SourceAuthorizationTarget::External(resource) => AuditTarget::ExternalSource {
                    resource: ResourceRef {
                        domain: ResourceDomain {
                            authority: resource.authority.clone(),
                            namespace: resource.namespace.to_string(),
                        },
                        resource_id: resource.id.to_string(),
                    },
                    usage,
                },
            }
        }
        AuthorizationOperation::Publication(facts) => AuditTarget::Publication {
            recipient: match &facts.recipient {
                PublicationRecipient::Principal(principal) => AuditRecipient::Principal {
                    principal: principal.clone(),
                },
                PublicationRecipient::Destination(target) => AuditRecipient::Destination {
                    resource: ResourceRef {
                        domain: ResourceDomain {
                            authority: target.authority.clone(),
                            namespace: target.namespace.to_string(),
                        },
                        resource_id: target.id.to_string(),
                    },
                },
            },
            mode: match facts.mode {
                PublicationMode::Buffered => AuditPublicationMode::Buffered,
                PublicationMode::Stream => AuditPublicationMode::Stream,
                PublicationMode::Replay => AuditPublicationMode::Replay,
                PublicationMode::Live => AuditPublicationMode::Live,
            },
        },
    }
}

fn denied() -> OperationRefused {
    OperationRefused::new(OperationRefusalKind::Denied)
}

#[cfg(test)]
mod tests;
