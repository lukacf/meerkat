//! Fail-closed authorization for explicit realm credential binding use.
//!
//! This module deliberately runs before credential resolution or provider
//! factory construction. It evaluates only typed identity, grant, realm, and
//! binding references. Credential stores and resolved secret material are not
//! inputs and cannot escape through the witness.

use std::future::Future;

use crate::AuthBindingRef;

use super::{ActingOnBehalfOf, AuthGrant, GrantAction, GrantScope, PrincipalRef};

/// One exact request to use an explicit auth binding for a durable target.
///
/// `durable_target` must be the target resolved by the authoritative identity
/// owner, not a display label supplied by the requesting surface. The exact
/// value is included in the grant's acting-on-behalf-of relation and in the
/// resulting witness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthBindingUseRequest {
    principal: PrincipalRef,
    durable_target: PrincipalRef,
    auth_binding: AuthBindingRef,
}

impl AuthBindingUseRequest {
    #[must_use]
    pub fn new(
        principal: PrincipalRef,
        durable_target: PrincipalRef,
        auth_binding: AuthBindingRef,
    ) -> Self {
        Self {
            principal,
            durable_target,
            auth_binding,
        }
    }

    #[must_use]
    pub fn principal(&self) -> &PrincipalRef {
        &self.principal
    }

    #[must_use]
    pub fn durable_target(&self) -> &PrincipalRef {
        &self.durable_target
    }

    #[must_use]
    pub fn auth_binding(&self) -> &AuthBindingRef {
        &self.auth_binding
    }

    fn grant_scope(&self) -> GrantScope {
        GrantScope::AuthBinding {
            realm_id: self.auth_binding.realm.clone(),
            binding_id: self.auth_binding.binding.clone(),
            profile_id: self.auth_binding.profile.clone(),
        }
    }

    fn delegation(&self) -> ActingOnBehalfOf {
        ActingOnBehalfOf::new(self.principal.clone(), self.durable_target.clone())
    }
}

/// Typed denial from the pre-materialization binding-use authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AuthBindingUseDenial {
    /// The env default is an implicit process fallback, not an explicit
    /// durable realm binding and therefore cannot cross this seam.
    #[error("binding-use authorization requires an explicit configured auth binding")]
    ExplicitConfiguredBindingRequired,
    /// No grant matched the principal, durable target, realm, binding, and
    /// optional override profile exactly.
    #[error("principal is not authorized to use the requested auth binding for the durable target")]
    MissingExactGrant,
}

/// Non-serializable proof for one exact principal, target, and binding tuple.
///
/// There is intentionally no public constructor. Provider resolution and
/// channel creation can require this witness without learning anything about
/// the credential store or resolved secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthBindingUseWitness {
    principal: PrincipalRef,
    durable_target: PrincipalRef,
    auth_binding: AuthBindingRef,
}

impl AuthBindingUseWitness {
    /// Verify that the witness still fences the exact request being
    /// materialized.
    #[must_use]
    pub fn authorizes(&self, request: &AuthBindingUseRequest) -> bool {
        self.principal == request.principal
            && self.durable_target == request.durable_target
            && self.auth_binding == request.auth_binding
    }

    #[must_use]
    pub fn principal(&self) -> &PrincipalRef {
        &self.principal
    }

    #[must_use]
    pub fn durable_target(&self) -> &PrincipalRef {
        &self.durable_target
    }

    #[must_use]
    pub fn auth_binding(&self) -> &AuthBindingRef {
        &self.auth_binding
    }
}

/// Typed result of realm credential policy evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthBindingUseDecision {
    Allowed(AuthBindingUseWitness),
    Denied(AuthBindingUseDenial),
}

impl AuthBindingUseDecision {
    pub fn into_result(self) -> Result<AuthBindingUseWitness, AuthBindingUseDenial> {
        match self {
            Self::Allowed(witness) => Ok(witness),
            Self::Denied(denial) => Err(denial),
        }
    }
}

/// Evaluate an explicit auth-binding request against realm ABAC grants.
///
/// Authorization requires all of the following to match exactly:
///
/// - requesting principal;
/// - `UseAuthBinding` action;
/// - realm, binding, and optional override profile;
/// - acting-on-behalf-of subject equal to the resolved durable target.
///
/// A configured binding's mere existence is not an input to this decision.
#[must_use]
pub fn authorize_explicit_auth_binding_use(
    request: &AuthBindingUseRequest,
    grants: &[AuthGrant],
) -> AuthBindingUseDecision {
    if request.auth_binding.is_env_default() {
        return AuthBindingUseDecision::Denied(
            AuthBindingUseDenial::ExplicitConfiguredBindingRequired,
        );
    }

    let scope = request.grant_scope();
    let delegation = request.delegation();
    let allowed = grants.iter().any(|grant| {
        grant.allows(
            &request.principal,
            GrantAction::UseAuthBinding,
            &scope,
            Some(&delegation),
        )
    });

    if allowed {
        AuthBindingUseDecision::Allowed(AuthBindingUseWitness {
            principal: request.principal.clone(),
            durable_target: request.durable_target.clone(),
            auth_binding: request.auth_binding.clone(),
        })
    } else {
        AuthBindingUseDecision::Denied(AuthBindingUseDenial::MissingExactGrant)
    }
}

/// Error from the authorization-before-materialization integration seam.
#[derive(Debug, PartialEq, Eq)]
pub enum AuthBindingUseGateError<E> {
    Authorization(AuthBindingUseDenial),
    Materialization(E),
}

/// Run credential resolution or factory construction only after an exact
/// binding-use witness has been minted.
///
/// Surfaces should place their resolver, token mint, adapter construction, and
/// channel open inside `materialize`. A denied request never calls it.
pub async fn authorize_then_materialize_auth_binding<T, E, F, Fut>(
    request: &AuthBindingUseRequest,
    grants: &[AuthGrant],
    materialize: F,
) -> Result<T, AuthBindingUseGateError<E>>
where
    F: FnOnce(AuthBindingUseWitness) -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    let witness = authorize_explicit_auth_binding_use(request, grants)
        .into_result()
        .map_err(AuthBindingUseGateError::Authorization)?;
    materialize(witness)
        .await
        .map_err(AuthBindingUseGateError::Materialization)
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::{BindingId, BindingOrigin, ProfileId, RealmId};

    use super::*;
    use crate::auth::PrincipalKind;

    fn principal(kind: PrincipalKind, id: &str) -> PrincipalRef {
        PrincipalRef::new(kind, id).expect("valid principal")
    }

    fn configured_binding(realm: &str, binding: &str) -> AuthBindingRef {
        AuthBindingRef {
            realm: RealmId::parse(realm).expect("valid realm"),
            binding: BindingId::parse(binding).expect("valid binding"),
            profile: None,
            origin: BindingOrigin::Configured,
        }
    }

    fn exact_grant(request: &AuthBindingUseRequest) -> AuthGrant {
        AuthGrant {
            principal: request.principal.clone(),
            scope: request.grant_scope(),
            actions: BTreeSet::from([GrantAction::UseAuthBinding]),
            acting_on_behalf_of: Some(request.delegation()),
        }
    }

    #[test]
    fn binding_existence_is_insufficient_without_an_exact_grant() {
        let request = AuthBindingUseRequest::new(
            principal(PrincipalKind::Human, "alice"),
            principal(PrincipalKind::PersonalAgent, "personal-agent"),
            configured_binding("home", "chatgpt"),
        );

        assert_eq!(
            authorize_explicit_auth_binding_use(&request, &[]),
            AuthBindingUseDecision::Denied(AuthBindingUseDenial::MissingExactGrant)
        );
    }

    #[test]
    fn wrong_durable_identity_and_wrong_realm_are_denied() {
        let allowed = AuthBindingUseRequest::new(
            principal(PrincipalKind::Human, "alice"),
            principal(PrincipalKind::PersonalAgent, "personal-agent"),
            configured_binding("home", "chatgpt"),
        );
        let grants = [exact_grant(&allowed)];

        let wrong_identity = AuthBindingUseRequest::new(
            allowed.principal.clone(),
            principal(PrincipalKind::PersonalAgent, "another-agent"),
            allowed.auth_binding.clone(),
        );
        assert_eq!(
            authorize_explicit_auth_binding_use(&wrong_identity, &grants),
            AuthBindingUseDecision::Denied(AuthBindingUseDenial::MissingExactGrant)
        );

        let wrong_realm = AuthBindingUseRequest::new(
            allowed.principal.clone(),
            allowed.durable_target.clone(),
            configured_binding("work", "chatgpt"),
        );
        assert_eq!(
            authorize_explicit_auth_binding_use(&wrong_realm, &grants),
            AuthBindingUseDecision::Denied(AuthBindingUseDenial::MissingExactGrant)
        );
    }

    #[test]
    fn binding_override_profile_requires_an_exact_profile_grant() {
        let base = AuthBindingUseRequest::new(
            principal(PrincipalKind::Human, "alice"),
            principal(PrincipalKind::PersonalAgent, "personal-agent"),
            configured_binding("home", "chatgpt"),
        );
        let grants = [exact_grant(&base)];
        let mut profile_binding = base.auth_binding.clone();
        profile_binding.profile = Some(ProfileId::parse("secondary").expect("valid profile"));
        let profile_request = AuthBindingUseRequest::new(
            base.principal.clone(),
            base.durable_target.clone(),
            profile_binding,
        );

        assert_eq!(
            authorize_explicit_auth_binding_use(&profile_request, &grants),
            AuthBindingUseDecision::Denied(AuthBindingUseDenial::MissingExactGrant)
        );
    }

    #[tokio::test]
    async fn denied_request_has_zero_resolver_and_factory_side_effects() {
        let resolver_calls = AtomicUsize::new(0);
        let factory_calls = AtomicUsize::new(0);
        let request = AuthBindingUseRequest::new(
            principal(PrincipalKind::Human, "mallory"),
            principal(PrincipalKind::PersonalAgent, "personal-agent"),
            configured_binding("home", "chatgpt"),
        );

        let result = authorize_then_materialize_auth_binding(&request, &[], |_witness| async {
            resolver_calls.fetch_add(1, Ordering::SeqCst);
            factory_calls.fetch_add(1, Ordering::SeqCst);
            Ok::<_, ()>(())
        })
        .await;

        assert_eq!(
            result,
            Err(AuthBindingUseGateError::Authorization(
                AuthBindingUseDenial::MissingExactGrant
            ))
        );
        assert_eq!(resolver_calls.load(Ordering::SeqCst), 0);
        assert_eq!(factory_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn allowed_request_materializes_once_with_an_exact_witness() {
        let calls = AtomicUsize::new(0);
        let request = AuthBindingUseRequest::new(
            principal(PrincipalKind::Human, "alice"),
            principal(PrincipalKind::PersonalAgent, "personal-agent"),
            configured_binding("home", "chatgpt"),
        );
        let grants = [exact_grant(&request)];
        let expected_request = request.clone();
        let calls_for_materialize = &calls;

        let value =
            authorize_then_materialize_auth_binding(&request, &grants, |witness| async move {
                assert!(witness.authorizes(&expected_request));
                calls_for_materialize.fetch_add(1, Ordering::SeqCst);
                Ok::<_, ()>(42)
            })
            .await
            .expect("authorized materialization");

        assert_eq!(value, 42);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
