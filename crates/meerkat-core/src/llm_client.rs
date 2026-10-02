//! Per-request authorization forwarding for existing model clients.
//!
//! This is operation data and a retained feature handle, not a provider policy,
//! credential grant, or session default. The actual provider supplies its final
//! resolved target and retains the returned check beside that exact request.

use crate::authorization::{
    AuthorizationOperation, ModelAuthorizationFacts, ModelAuthorizationUse,
    OperationAuthorizationError, OperationAuthorizationFacts, PreparedAuthorizationBinding,
    PreparedOperationCheck, WorkAuthorizationContext,
};
use crate::live_execution::CanonicalContextRevision;
use crate::{AuthCredentialIdentity, OperationId, RunId, SessionLlmIdentity};

/// Exact nonsecret model selection retained by the native work owner.
///
/// This is a projection of the existing provider factory's resolved target,
/// not authority to use it. The native owner must compare its admitted
/// selection with final operation facts and the current controller grant.
/// A credential binding identifies the selected credential owner; it does not
/// assert the external account behind a dynamically refreshed credential.
/// Request parameters are intentionally excluded: temperature and hosted-tool
/// toggles do not select another route. Their actual effects remain operation
/// facts evaluated by the current owner at request preparation.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ControllerModelSelection {
    model: String,
    provider: crate::Provider,
    self_hosted_server_id: Option<String>,
    auth_binding: Option<crate::AuthBindingRef>,
    credential: AuthCredentialIdentity,
    backend_profile_id: String,
    backend_kind: String,
}

impl ControllerModelSelection {
    pub fn new(
        identity: SessionLlmIdentity,
        credential: AuthCredentialIdentity,
        backend_profile_id: String,
        backend_kind: String,
    ) -> Self {
        Self {
            model: identity.model,
            provider: identity.provider,
            self_hosted_server_id: identity.self_hosted_server_id,
            auth_binding: identity.auth_binding,
            credential,
            backend_profile_id,
            backend_kind,
        }
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn provider(&self) -> crate::Provider {
        self.provider
    }

    pub fn self_hosted_server_id(&self) -> Option<&str> {
        self.self_hosted_server_id.as_deref()
    }

    pub fn auth_binding(&self) -> Option<&crate::AuthBindingRef> {
        self.auth_binding.as_ref()
    }

    pub fn credential(&self) -> &AuthCredentialIdentity {
        &self.credential
    }

    pub fn backend_profile_id(&self) -> &str {
        &self.backend_profile_id
    }

    pub fn backend_kind(&self) -> &str {
        &self.backend_kind
    }

    /// Compare the admitted selection with actual resolved operation data.
    /// Endpoint, wire model and hosted capabilities still require the current
    /// owner's grant evaluation; this equality is not an authorization result.
    pub fn matches_model_facts(&self, facts: &ModelAuthorizationFacts) -> bool {
        self.model == facts.identity.model
            && self.provider == facts.identity.provider
            && self.self_hosted_server_id == facts.identity.self_hosted_server_id
            && self.auth_binding == facts.identity.auth_binding
            && facts.credential.as_ref() == Some(&self.credential)
            && facts.backend_profile_id.as_deref() == Some(self.backend_profile_id.as_str())
            && facts.backend_kind.as_ref() == self.backend_kind
    }
}

impl std::fmt::Debug for ControllerModelSelection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ControllerModelSelection([REDACTED])")
    }
}

/// Request-free facts supplied by the actual immutable selected client.
///
/// This is nonserializable data, never account permission or a request. The
/// trusted provider must use the same endpoint/model lowering as its transport.
#[derive(Clone)]
pub struct ControllerModelFacts {
    selection: ControllerModelSelection,
    endpoint: std::sync::Arc<str>,
    wire_model: std::sync::Arc<str>,
}

impl ControllerModelFacts {
    pub fn new(
        selection: ControllerModelSelection,
        endpoint: std::sync::Arc<str>,
        wire_model: std::sync::Arc<str>,
    ) -> Self {
        Self {
            selection,
            endpoint,
            wire_model,
        }
    }

    pub fn selection(&self) -> &ControllerModelSelection {
        &self.selection
    }
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
    pub fn wire_model(&self) -> &str {
        &self.wire_model
    }
}

impl std::fmt::Debug for ControllerModelFacts {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ControllerModelFacts([REDACTED])")
    }
}

/// The selected client cannot provide coherent request-free route facts.
/// This is a setup/data error, not a permission denial or a model response.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("controller model facts unavailable")]
pub struct ControllerFactsUnavailable;

/// Runnable client retained by its existing selection owner for one work item.
///
/// A mutable fallback selector must pin its selected child, not itself. This
/// handle keeps that actual child alive; its selection remains data that the
/// native admission owner must match against the admitted controller grant.
/// Trusted custom hosts must uphold the same immutable-selection contract.
/// The handle is deliberately not serializable.
#[derive(Clone)]
pub struct ControllerModelClient {
    selection: ControllerModelSelection,
    client: std::sync::Arc<dyn crate::AgentLlmClient>,
}

impl ControllerModelClient {
    pub fn new(
        selection: ControllerModelSelection,
        client: std::sync::Arc<dyn crate::AgentLlmClient>,
    ) -> Self {
        Self { selection, client }
    }

    pub fn selection(&self) -> &ControllerModelSelection {
        &self.selection
    }

    pub fn client(&self) -> &std::sync::Arc<dyn crate::AgentLlmClient> {
        &self.client
    }

    /// Run existing-credential maintenance on this exact retained child.
    /// Selection is checked on both sides of the await; maintenance cannot
    /// silently select a fallback or replace the admitted controller identity.
    pub async fn prepare_controller_credential(&self) -> Result<(), crate::auth::AuthError> {
        if self.client.controller_model_selection().as_ref() != Some(&self.selection) {
            return Err(crate::auth::AuthError::StaleCredential);
        }
        self.client.prepare_controller_credential().await?;
        if self.client.controller_model_selection().as_ref() != Some(&self.selection) {
            return Err(crate::auth::AuthError::StaleCredential);
        }
        Ok(())
    }

    /// Read the pinned child's route without constructing or sending a request.
    /// Custom clients must uphold the immutable selection contract.
    pub fn plain_facts(&self) -> Result<ControllerModelFacts, ControllerFactsUnavailable> {
        if self.client.controller_model_selection().as_ref() != Some(&self.selection) {
            return Err(ControllerFactsUnavailable);
        }
        let facts = self.client.controller_model_facts()?;
        if facts.selection() != &self.selection {
            return Err(ControllerFactsUnavailable);
        }
        Ok(facts)
    }
}

impl std::fmt::Debug for ControllerModelClient {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ControllerModelClient([REDACTED])")
    }
}

/// The authorization context of one model operation in admitted work.
///
/// Cloning forwards the same work association. It does not authorize another
/// payload, provider, account or operation, and has no serialized form. Actor,
/// requester, represented subject and delegation stay with the feature-owned
/// work association; provider credentials never replace those identities.
#[derive(Clone)]
pub struct LlmRequestAuthorization {
    work: WorkAuthorizationContext,
    operation_id: OperationId,
    run_id: Option<RunId>,
    context_revision: Option<CanonicalContextRevision>,
    usage: ModelAuthorizationUse,
}

impl LlmRequestAuthorization {
    pub fn new(
        work: WorkAuthorizationContext,
        operation_id: OperationId,
        usage: ModelAuthorizationUse,
    ) -> Self {
        Self {
            work,
            operation_id,
            run_id: None,
            context_revision: None,
            usage,
        }
    }

    /// Retain coordinates supplied by the existing work/turn owner. Missing
    /// coordinates remain absent rather than being inferred from a session.
    pub fn with_coordinates(
        mut self,
        run_id: Option<RunId>,
        context_revision: Option<CanonicalContextRevision>,
    ) -> Self {
        self.run_id = run_id;
        self.context_revision = context_revision;
        self
    }

    pub fn usage(&self) -> ModelAuthorizationUse {
        self.usage
    }

    /// Prepare once for the actual immutable request and final provider facts.
    /// The sink must call `current` after relevant waits and before sending.
    /// Any target or payload change requires a new call to this method.
    pub fn prepare(
        &self,
        mut target: ModelAuthorizationFacts,
    ) -> Result<PreparedOperationCheck, OperationAuthorizationError> {
        target.usage = self.usage;
        let binding = PreparedAuthorizationBinding::new(OperationAuthorizationFacts {
            operation_id: self.operation_id.clone(),
            execution_scope: self.work.execution_scope().clone(),
            run_id: self.run_id.clone(),
            context_revision: self.context_revision.clone(),
            operation: AuthorizationOperation::Model(target),
        });
        PreparedOperationCheck::prepare(self.work.clone(), binding)
    }
}

impl std::fmt::Debug for LlmRequestAuthorization {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("LlmRequestAuthorization([REDACTED])")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn identity() -> SessionLlmIdentity {
        SessionLlmIdentity {
            model: "controller-model-canary".to_owned(),
            provider: crate::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: Some(crate::AuthBindingRef {
                realm: crate::RealmId::parse("controller-realm").unwrap(),
                binding: crate::BindingId::parse("controller-binding").unwrap(),
                profile: None,
                origin: crate::BindingOrigin::Configured,
            }),
        }
    }

    fn selection() -> ControllerModelSelection {
        let identity = identity();
        let credential = AuthCredentialIdentity::Binding(identity.auth_binding.clone().unwrap());
        ControllerModelSelection::new(
            identity,
            credential,
            "controller-profile-canary".to_owned(),
            "openai".to_owned(),
        )
    }

    fn facts() -> ModelAuthorizationFacts {
        let identity = identity();
        ModelAuthorizationFacts {
            credential: Some(AuthCredentialIdentity::Binding(
                identity.auth_binding.clone().unwrap(),
            )),
            identity: Arc::new(identity),
            wire_model: Arc::from("actual-wire-model"),
            hosted_capabilities: Arc::from([]),
            backend_profile_id: Some(Arc::from("controller-profile-canary")),
            backend_kind: Arc::from("openai"),
            endpoint: Arc::from("https://example.invalid/responses"),
            usage: ModelAuthorizationUse::ControllerInference,
            live_channel: None,
        }
    }

    #[test]
    fn controller_selection_matches_every_route_coordinate() {
        let selected = selection();
        assert!(selected.matches_model_facts(&facts()));
        let mutations: [fn(&mut ModelAuthorizationFacts); 7] = [
            |f| Arc::make_mut(&mut f.identity).model = "other".to_owned(),
            |f| Arc::make_mut(&mut f.identity).provider = crate::Provider::Anthropic,
            |f| Arc::make_mut(&mut f.identity).self_hosted_server_id = Some("other".to_owned()),
            |f| Arc::make_mut(&mut f.identity).auth_binding = None,
            |f| f.credential = None,
            |f| f.backend_profile_id = None,
            |f| f.backend_kind = Arc::from("other"),
        ];
        for mutate in mutations {
            let mut changed = facts();
            mutate(&mut changed);
            assert!(!selected.matches_model_facts(&changed));
        }
    }

    #[test]
    fn controller_route_excludes_request_parameters_but_does_not_authorize_them() {
        let selected = selection();
        let mut changed = facts();
        Arc::make_mut(&mut changed.identity).provider_params =
            Some(crate::lifecycle::run_primitive::ProviderParamsOverride {
                temperature: Some(0.75),
                ..Default::default()
            });
        changed.wire_model = Arc::from("another-lowered-model");
        changed.endpoint = Arc::from("https://another.invalid/messages");
        changed.hosted_capabilities = Arc::from([crate::ServerToolKind::WebSearch]);
        assert!(selected.matches_model_facts(&changed));
        // These remain independently evaluated actual facts. Matching a route
        // is intentionally not the operation owner's permission decision.
        assert_eq!(
            selected,
            ControllerModelSelection::new(
                (*changed.identity).clone(),
                changed.credential.unwrap(),
                selected.backend_profile_id().to_owned(),
                selected.backend_kind().to_owned(),
            )
        );
    }

    #[test]
    fn controller_selection_is_strict_data_with_protected_debug() {
        fn requires_eq<T: Eq>() {}
        requires_eq::<ControllerModelSelection>();
        let selected = selection();
        let value = serde_json::to_value(&selected).unwrap();
        assert_eq!(
            serde_json::from_value::<ControllerModelSelection>(value.clone()).unwrap(),
            selected
        );
        assert_eq!(
            format!("{selected:?}"),
            "ControllerModelSelection([REDACTED])"
        );
        assert!(!value.to_string().contains("provider_params"));
        let mut unknown = value.clone();
        unknown["external_account_inferred_from_headers"] = serde_json::json!("forged");
        assert!(serde_json::from_value::<ControllerModelSelection>(unknown).is_err());
        let mut missing = value;
        missing.as_object_mut().unwrap().remove("credential");
        assert!(serde_json::from_value::<ControllerModelSelection>(missing).is_err());
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
#[path = "llm_client/controller_facts_tests.rs"]
mod controller_facts_tests;
