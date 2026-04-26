//! Principal, grant, and visibility contracts.
//!
//! These types are policy inputs, not policy execution engines. Runtime and
//! machine-owned flows can consume them without treating labels or app context
//! as authorization truth.

use crate::SurfaceMetadata;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;

/// Validation errors for principal/grant/visibility contracts.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PrincipalContractError {
    #[error("principal id must not be empty")]
    EmptyPrincipalId,
    #[error("principal id must not contain control characters")]
    InvalidPrincipalId,
    #[error("application scope namespace must not be empty")]
    EmptyApplicationScopeNamespace,
    #[error("application scope id must not be empty")]
    EmptyApplicationScopeId,
}

/// Stable, caller-visible principal id.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(transparent)]
pub struct PrincipalId(String);

impl PrincipalId {
    pub fn new(value: impl Into<String>) -> Result<Self, PrincipalContractError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(PrincipalContractError::EmptyPrincipalId);
        }
        if value.chars().any(char::is_control) {
            return Err(PrincipalContractError::InvalidPrincipalId);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PrincipalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Generic principal categories. Product-specific role names belong outside
/// core and may be mapped to these typed categories by clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum PrincipalKind {
    Human,
    PersonalAgent,
    SharedAgent,
    RuntimeHost,
    ServiceAccount,
}

/// Typed principal reference.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct PrincipalRef {
    pub kind: PrincipalKind,
    pub id: PrincipalId,
}

impl PrincipalRef {
    pub fn new(kind: PrincipalKind, id: impl Into<String>) -> Result<Self, PrincipalContractError> {
        Ok(Self {
            kind,
            id: PrincipalId::new(id)?,
        })
    }
}

/// Explicit acting-on-behalf-of relationship for audit and policy checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct ActingOnBehalfOf {
    pub actor: PrincipalRef,
    pub subject: PrincipalRef,
}

impl ActingOnBehalfOf {
    #[must_use]
    pub fn new(actor: PrincipalRef, subject: PrincipalRef) -> Self {
        Self { actor, subject }
    }
}

/// Generic scope for grants and shared visibility.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "scope_type", rename_all = "snake_case")]
pub enum GrantScope {
    Realm {
        realm_id: String,
    },
    Session {
        session_id: String,
    },
    Mob {
        mob_id: String,
    },
    /// Product-neutral extension point. The typed namespace/id pair is policy
    /// input; labels/app context are still not authority.
    Application {
        namespace: String,
        id: String,
    },
}

impl GrantScope {
    pub fn application(
        namespace: impl Into<String>,
        id: impl Into<String>,
    ) -> Result<Self, PrincipalContractError> {
        let namespace = namespace.into();
        let id = id.into();
        if namespace.trim().is_empty() {
            return Err(PrincipalContractError::EmptyApplicationScopeNamespace);
        }
        if id.trim().is_empty() {
            return Err(PrincipalContractError::EmptyApplicationScopeId);
        }
        Ok(Self::Application { namespace, id })
    }

    #[must_use]
    pub fn matches(&self, requested: &Self) -> bool {
        self == requested
    }
}

/// Typed visibility class for events, artifacts, approvals, or future records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(tag = "visibility", rename_all = "snake_case")]
pub enum VisibilityClass {
    Private { principal: PrincipalRef },
    Scoped { scope: GrantScope },
}

/// Actions that grants may allow. Enforcement sites decide which action is
/// needed for a specific operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum GrantAction {
    Observe,
    ReplayEvents,
    RequestApproval,
    DecideApproval,
    UseTool,
    ManageRuntime,
}

/// A typed grant issued to a principal for a single scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
pub struct AuthGrant {
    pub principal: PrincipalRef,
    pub scope: GrantScope,
    pub actions: BTreeSet<GrantAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acting_on_behalf_of: Option<ActingOnBehalfOf>,
}

impl AuthGrant {
    #[must_use]
    pub fn allows(
        &self,
        principal: &PrincipalRef,
        action: GrantAction,
        scope: &GrantScope,
        acting_on_behalf_of: Option<&ActingOnBehalfOf>,
    ) -> bool {
        if &self.principal != principal {
            return false;
        }
        if !self.actions.contains(&action) {
            return false;
        }
        if !self.scope.matches(scope) {
            return false;
        }
        self.acting_on_behalf_of.as_ref() == acting_on_behalf_of
    }
}

/// Evaluate observation of a typed visibility class.
///
/// Private visibility is intentionally not broadened by scoped grants. A
/// private record is visible only to the exact principal named by the record.
#[must_use]
pub fn can_observe_visibility(
    principal: &PrincipalRef,
    grants: &[AuthGrant],
    visibility: &VisibilityClass,
) -> bool {
    match visibility {
        VisibilityClass::Private { principal: owner } => owner == principal,
        VisibilityClass::Scoped { scope } => grants.iter().any(|grant| {
            grant.allows(principal, GrantAction::Observe, scope, None)
                || grant.allows(principal, GrantAction::ReplayEvents, scope, None)
        }),
    }
}

/// Defensive helper that proves caller metadata is not interpreted as
/// authorization material.
#[must_use]
pub fn metadata_grants_no_visibility(
    principal: &PrincipalRef,
    grants: &[AuthGrant],
    visibility: &VisibilityClass,
    _metadata: &SurfaceMetadata,
) -> bool {
    can_observe_visibility(principal, grants, visibility)
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::collections::{BTreeMap, BTreeSet};

    fn human(id: &str) -> PrincipalRef {
        PrincipalRef::new(PrincipalKind::Human, id).expect("valid human principal")
    }

    fn personal_agent(id: &str) -> PrincipalRef {
        PrincipalRef::new(PrincipalKind::PersonalAgent, id).expect("valid agent principal")
    }

    fn grant(
        principal: PrincipalRef,
        scope: GrantScope,
        actions: impl IntoIterator<Item = GrantAction>,
    ) -> AuthGrant {
        AuthGrant {
            principal,
            scope,
            actions: actions.into_iter().collect(),
            acting_on_behalf_of: None,
        }
    }

    #[test]
    fn invalid_empty_principal_ids_are_rejected() {
        assert!(matches!(
            PrincipalId::new(""),
            Err(PrincipalContractError::EmptyPrincipalId)
        ));
        assert!(matches!(
            PrincipalId::new(" \t "),
            Err(PrincipalContractError::EmptyPrincipalId)
        ));
        assert!(matches!(
            PrincipalId::new("human:\nmallory"),
            Err(PrincipalContractError::InvalidPrincipalId)
        ));
    }

    #[test]
    fn private_and_application_visibility_do_not_overlap() {
        let alice = human("human:alice");
        let bob = human("human:bob");
        let project_scope =
            GrantScope::application("client.project", "repo-a").expect("valid scope");
        let bob_project_grant = grant(
            bob.clone(),
            project_scope,
            [GrantAction::Observe, GrantAction::ReplayEvents],
        );

        let alice_private = VisibilityClass::Private {
            principal: alice.clone(),
        };

        assert!(can_observe_visibility(&alice, &[], &alice_private));
        assert!(!can_observe_visibility(
            &bob,
            &[bob_project_grant],
            &alice_private
        ));
    }

    #[test]
    fn grant_scope_mismatch_denies_visibility() {
        let bob = human("human:bob");
        let session_scope = GrantScope::Session {
            session_id: "session-1".into(),
        };
        let mob_scope = GrantScope::Mob {
            mob_id: "mob-1".into(),
        };
        let bob_session_grant = grant(bob.clone(), session_scope, [GrantAction::Observe]);
        let mob_visibility = VisibilityClass::Scoped { scope: mob_scope };

        assert!(!can_observe_visibility(
            &bob,
            &[bob_session_grant],
            &mob_visibility
        ));
    }

    #[test]
    fn acting_on_behalf_of_is_typed_and_exact() {
        let bob = human("human:bob");
        let bob_agent = personal_agent("agent:bob-personal");
        let alice = human("human:alice");
        let scope = GrantScope::Session {
            session_id: "session-1".into(),
        };
        let bob_relationship = ActingOnBehalfOf::new(bob_agent.clone(), bob);
        let alice_relationship = ActingOnBehalfOf::new(bob_agent.clone(), alice);
        let grant = AuthGrant {
            principal: bob_agent.clone(),
            scope: scope.clone(),
            actions: BTreeSet::from([GrantAction::RequestApproval]),
            acting_on_behalf_of: Some(bob_relationship.clone()),
        };

        assert!(grant.allows(
            &bob_agent,
            GrantAction::RequestApproval,
            &scope,
            Some(&bob_relationship)
        ));
        assert!(!grant.allows(&bob_agent, GrantAction::RequestApproval, &scope, None));
        assert!(!grant.allows(
            &bob_agent,
            GrantAction::RequestApproval,
            &scope,
            Some(&alice_relationship)
        ));
    }

    #[test]
    fn metadata_is_not_visibility_authority() {
        let bob = human("human:bob");
        let visibility = VisibilityClass::Scoped {
            scope: GrantScope::application("client.project", "repo-a").expect("valid scope"),
        };
        let metadata = SurfaceMetadata::from_optional_parts(
            Some(BTreeMap::from([
                (
                    "visibility".to_string(),
                    "client.project:repo-a".to_string(),
                ),
                ("principal".to_string(), "human:bob".to_string()),
            ])),
            Some(json!({
                "grant": {
                    "action": "observe",
                    "scope": "client.project:repo-a"
                }
            })),
        );

        assert!(!metadata_grants_no_visibility(
            &bob,
            &[],
            &visibility,
            &metadata
        ));
    }
}
