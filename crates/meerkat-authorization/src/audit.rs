//! Protected observations from the actual grant-backed work compiler.
//!
//! The sink belongs to the native input row. This module has no audit registry,
//! permission lookup, replay reducer, durability claim, or exporter fallback.

use std::sync::Arc;

use meerkat_authorization_contracts::audit::{
    AuditModelTarget, AuditModelUse, AuditObservation, AuditPolicyRead, AuditPublicationMode,
    AuditRecipient, AuditReviewAttribution, AuditReviewRole, AuditSourceUse, AuditTarget,
    AuditToolOwner, AuthorizationAuditObservation, AuthorizationAuditSink,
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
/// compiler and the native row-bound sink. All authority still lives in `inner`.
pub(crate) struct AuditedWorkAuthorization {
    pub(crate) inner: Arc<dyn WorkAuthorization>,
    pub(crate) associations: Arc<[InputAuthorityAssociation]>,
    pub(crate) publication: LocalAuthorizationPublication,
    pub(crate) clock: Arc<dyn LocalAuthorizationClock>,
    pub(crate) sink: Arc<dyn AuthorizationAuditSink>,
}

impl WorkAuthorization for AuditedWorkAuthorization {
    fn controller_model_selection(&self) -> Option<meerkat_core::ControllerModelSelection> {
        self.inner.controller_model_selection()
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
                let (result, revision) = if let Some(error) = known_failure {
                    (Err(error), None)
                } else {
                    match observed {
                        Ok((result, stamp)) => {
                            policy_observation = Some(stamp.policy_observation());
                            (result, Some(stamp.observation_sequence()))
                        }
                        Err(PublicationError::Changed) => (
                            Err(
                                OperationRefused::new(OperationRefusalKind::ReprepareRequired)
                                    .into(),
                            ),
                            None,
                        ),
                        Err(PublicationError::Unavailable) => {
                            (Err(OperationAuthorizationError::Unavailable), None)
                        }
                    }
                };
                match result {
                    Ok(inner) => {
                        let controller = matches!(&binding.facts().operation,
                    AuthorizationOperation::Model(facts) if facts.usage == ModelAuthorizationUse::ControllerInference);
                        let operation = !matches!(&binding.facts().operation,
                    AuthorizationOperation::Model(facts) if facts.usage == ModelAuthorizationUse::ControllerInference && facts.hosted_capabilities.is_empty());
                        let policy = AuditPolicyRead {
                            publication_sequence: revision.ok_or_else(denied)?,
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

struct AuditedPrepared {
    inner: Arc<dyn PreparedOperationAuthorization>,
    policy_observation: Option<meerkat_core::authorization::PolicyPublicationObservation>,
    binding: PreparedAuthorizationBinding,
    target: Arc<AuditTarget>,
    sink: Arc<dyn AuthorizationAuditSink>,
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
            OperationObservation::Entry => AuditObservation::Entry,
            OperationObservation::AuthorizationUnavailable => {
                AuditObservation::AuthorizationUnavailable {
                    target: Arc::clone(&self.target),
                }
            }
            OperationObservation::Outcome(outcome) => AuditObservation::Outcome { outcome },
            OperationObservation::Refused(reason) => AuditObservation::Refused {
                target: Arc::clone(&self.target),
                reason,
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
