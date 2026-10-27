//! Typed readiness evaluation over current policy and runtime-owner evidence.
//!
//! Callers inject owners through narrow ports. Untrusted JSON and setup-agent
//! reports have no path to construct `VerifiedReadiness` directly.

use crate::authority::{
    Action, FrontendPermission, LifetimeScope, RoleLifetime, ServicePermission, SetupContextError,
    SetupRequestEvidence, SetupUseContext,
};
use meerkat_authorization_contracts::work_association::OriginalWorkRef;
use meerkat_core::auth::PrincipalRef;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use toolkit_package::{ContentDigest, Id, Revision, canonical_json};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceAccess {
    pub operation: Id,
    pub resource: Id,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RoleRequirement {
    Service {
        connection: Id,
        connection_revision: Revision,
        access: BTreeSet<ServiceAccess>,
        runtime: Id,
    },
    Frontend {
        endpoint: Id,
        participants: BTreeSet<Id>,
        audience: BTreeSet<Id>,
        interactions: BTreeSet<Id>,
        agent: Id,
        conversation_policy: Revision,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingIntent {
    pub id: Id,
    pub tenant: Id,
    pub deployment: Id,
    #[serde(with = "crate::authority::principal_codec")]
    pub requester: PrincipalRef,
    pub target: Id,
    pub original_work: OriginalWorkRef,
    pub manifest_digest: ContentDigest,
    pub revision: Revision,
    pub policy_revision: Revision,
    pub request_valid_until_ms: u64,
    pub lifetime: RoleLifetime,
    pub role: RoleRequirement,
}

/// Policy owner output, not an agent-authored approval claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantReceipt {
    pub id: Id,
    pub owner: Id,
    pub intent: Id,
    pub intent_revision: Revision,
    pub tenant: Id,
    pub deployment: Id,
    pub subject: Id,
    pub policy_revision: Revision,
    pub manifest_digest: ContentDigest,
    pub original_work: OriginalWorkRef,
    pub intent_digest: ContentDigest,
    pub role: RoleRequirement,
    pub lifetime: RoleLifetime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RoleEvidence {
    Service {
        runtime: Id,
        incarnation: Id,
        catalog_revision: Revision,
        connection_revision: Revision,
    },
    Frontend {
        endpoint: Id,
        admission_revision: Revision,
        conversation_revision: Revision,
        delivery_revision: Revision,
        verified_audience: BTreeSet<Id>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingReceipt {
    pub id: Id,
    pub owner: Id,
    pub intent: Id,
    pub intent_revision: Revision,
    pub grant: Id,
    pub intent_digest: ContentDigest,
    pub grant_digest: ContentDigest,
    pub manifest_digest: ContentDigest,
    pub policy_revision: Revision,
    pub observed_at_ms: u64,
    pub expires_at_ms: u64,
    pub evidence: RoleEvidence,
}

/// Implemented by the application's actual policy owner. Revocation and current
/// policy are checked on every call; no default allow implementation exists.
pub trait GrantOwner: Send + Sync {
    fn current_grant(
        &self,
        intent: &BindingIntent,
        now_ms: u64,
    ) -> Result<Option<GrantReceipt>, BindingError>;
}

/// Implemented by the actual runtime/interaction owner. A superseded runtime,
/// changed catalog or stale audience must not return an old receipt as current.
pub trait BindingOwner: Send + Sync {
    fn current_binding(
        &self,
        intent: &BindingIntent,
        grant: &GrantReceipt,
        now_ms: u64,
    ) -> Result<Option<BindingReceipt>, BindingError>;
}

#[derive(Debug, thiserror::Error)]
pub enum BindingError {
    #[error(transparent)]
    Authority(#[from] SetupContextError),
    #[error("intent is outside the caller's authenticated context")]
    WrongContext,
    #[error("role requirements are empty or exceed the caller's authority")]
    InvalidRequirements,
    #[error("binding intent has expired")]
    Expired,
    #[error("owner returned stale or mismatched grant evidence")]
    InvalidGrant,
    #[error("owner returned stale or mismatched runtime evidence")]
    InvalidBinding,
    #[error("owner is unavailable: {0}")]
    OwnerUnavailable(String),
    #[error("evidence encoding failed: {0}")]
    Encoding(#[from] serde_json::Error),
}

impl BindingIntent {
    pub fn digest(&self) -> Result<ContentDigest, BindingError> {
        Ok(ContentDigest::of_bytes(&canonical_json(self)?))
    }
}

impl GrantReceipt {
    pub fn digest(&self) -> Result<ContentDigest, BindingError> {
        Ok(ContentDigest::of_bytes(&canonical_json(self)?))
    }
}

/// Host-only observation context. A frontend's management/read authorization
/// must be enforced by its application before exposing these observations.
pub struct BindingContext<'a> {
    pub tenant: &'a Id,
    pub deployment: &'a Id,
    pub policy_revision: &'a Revision,
    pub now_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum RequirementOutcome {
    PendingGrant,
    PendingBinding { grant: Id },
    Ready { grant: Id, receipt: Id },
}

/// Read-only result, constructed exclusively by querying canonical owners.
#[derive(Debug, Clone, Serialize)]
pub struct VerifiedReadiness {
    outcomes: BTreeMap<Id, RequirementOutcome>,
    observed_at_ms: u64,
}

impl VerifiedReadiness {
    pub fn outcomes(&self) -> &BTreeMap<Id, RequirementOutcome> {
        &self.outcomes
    }

    pub fn all_ready(&self) -> bool {
        !self.outcomes.is_empty()
            && self
                .outcomes
                .values()
                .all(|r| matches!(r, RequirementOutcome::Ready { .. }))
    }
}

pub fn verify_readiness(
    authority: &SetupRequestEvidence,
    intents: &[BindingIntent],
    grants: &dyn GrantOwner,
    bindings: &dyn BindingOwner,
    context: &SetupUseContext<'_>,
) -> Result<VerifiedReadiness, BindingError> {
    authority.check_context(context)?;
    for intent in intents {
        validate_proposal(authority, intent, context)?;
    }
    observe_readiness(
        intents,
        grants,
        bindings,
        &BindingContext {
            tenant: context.tenant,
            deployment: context.deployment,
            policy_revision: context.policy_revision,
            now_ms: context.now_ms,
        },
    )
}

/// Observes owner-approved roles after the setup request has ended. This path
/// cannot authorize installation, grants, exposure, or tool invocation. A
/// current policy owner is consulted each time; setup-token expiry does not
/// revoke a separately approved member- or deployment-scoped role.
pub fn observe_readiness(
    intents: &[BindingIntent],
    grants: &dyn GrantOwner,
    bindings: &dyn BindingOwner,
    context: &BindingContext<'_>,
) -> Result<VerifiedReadiness, BindingError> {
    let now_ms = context.now_ms;
    let mut outcomes = BTreeMap::new();
    for intent in intents {
        if intent.tenant != *context.tenant
            || intent.deployment != *context.deployment
            || intent.policy_revision != *context.policy_revision
        {
            return Err(BindingError::WrongContext);
        }
        validate_requirements(intent, now_ms)?;
        if outcomes.contains_key(&intent.id) {
            return Err(BindingError::InvalidRequirements);
        }
        let Some(grant) = grants.current_grant(intent, now_ms)? else {
            outcomes.insert(intent.id.clone(), RequirementOutcome::PendingGrant);
            continue;
        };
        validate_grant(intent, &grant, now_ms)?;
        let Some(receipt) = bindings.current_binding(intent, &grant, now_ms)? else {
            outcomes.insert(
                intent.id.clone(),
                RequirementOutcome::PendingBinding { grant: grant.id },
            );
            continue;
        };
        validate_binding(intent, &grant, &receipt, now_ms)?;
        outcomes.insert(
            intent.id.clone(),
            RequirementOutcome::Ready {
                grant: grant.id,
                receipt: receipt.id,
            },
        );
    }
    Ok(VerifiedReadiness {
        outcomes,
        observed_at_ms: now_ms,
    })
}

/// Validates a new requested effect, including the full role lifetime. The
/// policy owner still makes the actual grant decision.
pub fn validate_proposal(
    authority: &SetupRequestEvidence,
    intent: &BindingIntent,
    context: &SetupUseContext<'_>,
) -> Result<(), BindingError> {
    authority.check_context(context)?;
    let now_ms = context.now_ms;
    let claims = authority.constraints();
    if intent.tenant != claims.tenant
        || intent.deployment != claims.deployment
        || intent.requester != authority.association().candidate().requester
        || intent.policy_revision != claims.policy_revision
        || intent.original_work != *authority.original_work()
    {
        return Err(BindingError::WrongContext);
    }
    if now_ms >= intent.request_valid_until_ms
        || intent.request_valid_until_ms > claims.expires_at_ms
    {
        return Err(BindingError::Expired);
    }
    validate_requirements(intent, now_ms)?;
    let action = match &intent.role {
        RoleRequirement::Service {
            connection, access, ..
        } => {
            if !access.iter().all(|access| {
                claims.ceiling.services.iter().any(|allowed| {
                    allowed.permits(&ServicePermission {
                        target: intent.target.clone(),
                        connection: connection.clone(),
                        operation: access.operation.clone(),
                        resource: access.resource.clone(),
                        lifetime: intent.lifetime.clone(),
                    })
                })
            }) {
                return Err(BindingError::InvalidRequirements);
            }
            Action::GrantService
        }
        RoleRequirement::Frontend {
            endpoint,
            participants,
            interactions,
            audience,
            ..
        } => {
            if !audience.is_subset(&claims.ceiling.output_audiences)
                || !participants.iter().all(|participant| {
                    interactions.iter().all(|interaction| {
                        claims.ceiling.frontends.iter().any(|allowed| {
                            allowed.permits(&FrontendPermission {
                                target: intent.target.clone(),
                                endpoint: endpoint.clone(),
                                participant: participant.clone(),
                                interaction: interaction.clone(),
                                lifetime: intent.lifetime.clone(),
                            })
                        })
                    })
                })
            {
                return Err(BindingError::InvalidRequirements);
            }
            Action::ExposeFrontend
        }
    };
    authority.check_proposal(action, &intent.target, context)?;
    Ok(())
}

fn validate_requirements(intent: &BindingIntent, now_ms: u64) -> Result<(), BindingError> {
    if !intent.lifetime.active_at(now_ms) {
        return Err(BindingError::Expired);
    }
    match &intent.lifetime.scope {
        LifetimeScope::Member { member } if member != &intent.target => {
            return Err(BindingError::InvalidRequirements);
        }
        LifetimeScope::Deployment { deployment } if deployment != &intent.deployment => {
            return Err(BindingError::InvalidRequirements);
        }
        _ => (),
    }
    match &intent.role {
        RoleRequirement::Service { access, .. } if access.is_empty() => {
            Err(BindingError::InvalidRequirements)
        }
        RoleRequirement::Frontend {
            participants,
            audience,
            interactions,
            agent,
            ..
        } if participants.is_empty()
            || audience.is_empty()
            || interactions.is_empty()
            || agent != &intent.target =>
        {
            Err(BindingError::InvalidRequirements)
        }
        _ => Ok(()),
    }
}

fn validate_grant(
    intent: &BindingIntent,
    grant: &GrantReceipt,
    now_ms: u64,
) -> Result<(), BindingError> {
    if grant.intent != intent.id
        || grant.intent_revision != intent.revision
        || grant.tenant != intent.tenant
        || grant.deployment != intent.deployment
        || grant.subject != intent.target
        || grant.policy_revision != intent.policy_revision
        || grant.manifest_digest != intent.manifest_digest
        || grant.original_work != intent.original_work
        || grant.intent_digest != intent.digest()?
        || grant.role != intent.role
        || !grant.lifetime.active_at(now_ms)
        || !intent.lifetime.permits(&grant.lifetime)
    {
        return Err(BindingError::InvalidGrant);
    }
    Ok(())
}

fn validate_binding(
    intent: &BindingIntent,
    grant: &GrantReceipt,
    receipt: &BindingReceipt,
    now_ms: u64,
) -> Result<(), BindingError> {
    if receipt.intent != intent.id
        || receipt.intent_revision != intent.revision
        || receipt.grant != grant.id
        || receipt.manifest_digest != intent.manifest_digest
        || receipt.intent_digest != intent.digest()?
        || receipt.grant_digest != grant.digest()?
        || receipt.policy_revision != intent.policy_revision
        || receipt.observed_at_ms > now_ms
        || receipt.expires_at_ms <= now_ms
        || grant
            .lifetime
            .expires_at_ms
            .is_some_and(|expiry| receipt.expires_at_ms > expiry)
    {
        return Err(BindingError::InvalidBinding);
    }
    match (&intent.role, &receipt.evidence) {
        (
            RoleRequirement::Service {
                runtime,
                connection_revision,
                ..
            },
            RoleEvidence::Service {
                runtime: actual,
                connection_revision: actual_revision,
                ..
            },
        ) if runtime == actual && connection_revision == actual_revision => Ok(()),
        (
            RoleRequirement::Frontend {
                endpoint, audience, ..
            },
            RoleEvidence::Frontend {
                endpoint: actual,
                verified_audience,
                ..
            },
        ) if endpoint == actual && audience == verified_audience => Ok(()),
        _ => Err(BindingError::InvalidBinding),
    }
}
