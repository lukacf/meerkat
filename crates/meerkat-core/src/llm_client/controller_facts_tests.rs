//! Data projection and actual pinned-child identity only, not permission tests.
use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

fn selected(model: &str) -> ControllerModelSelection {
    let binding = crate::AuthBindingRef {
        realm: crate::RealmId::parse("plain-controller-test").unwrap(),
        binding: crate::BindingId::parse("account-canary").unwrap(),
        profile: None,
        origin: crate::BindingOrigin::Configured,
    };
    ControllerModelSelection::new(
        SessionLlmIdentity {
            model: model.into(),
            provider: crate::Provider::OpenAI,
            self_hosted_server_id: None,
            provider_params: None,
            auth_binding: Some(binding.clone()),
        },
        AuthCredentialIdentity::Binding(binding),
        "profile-canary".into(),
        "openai".into(),
    )
}

struct ProjectingClient {
    reported: ControllerModelSelection,
    projected: ControllerModelSelection,
    sends: Arc<AtomicUsize>,
}
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl crate::AgentLlmClient for ProjectingClient {
    fn controller_model_selection(&self) -> Option<ControllerModelSelection> {
        Some(self.reported.clone())
    }
    fn controller_model_facts(&self) -> Result<ControllerModelFacts, ControllerFactsUnavailable> {
        Ok(ControllerModelFacts::new(
            self.projected.clone(),
            "https://endpoint-canary.invalid/responses".into(),
            "wire-model-canary".into(),
        ))
    }
    async fn stream_response(
        &self,
        _: &[crate::Message],
        _: &[Arc<crate::ToolDef>],
        _: u32,
        _: Option<f32>,
        _: Option<&crate::ProviderParamsOverride>,
    ) -> Result<crate::LlmStreamResult, crate::AgentError> {
        self.sends.fetch_add(1, Ordering::SeqCst);
        Err(crate::AgentError::ConfigError(
            "unexpected test send".into(),
        ))
    }
    fn provider(&self) -> crate::Provider {
        self.reported.provider()
    }
    fn model(&self) -> &str {
        self.reported.model()
    }
}

#[test]
fn pinned_plain_facts_are_data_only_and_reject_cross_selection() {
    let selection = selected("model-canary");
    let sends = Arc::new(AtomicUsize::new(0));
    let pin = ControllerModelClient::new(
        selection.clone(),
        Arc::new(ProjectingClient {
            reported: selection.clone(),
            projected: selection.clone(),
            sends: sends.clone(),
        }),
    );
    let plain = pin.plain_facts().expect("same immutable selected child");
    assert_eq!(plain.selection(), &selection);
    assert_eq!(
        plain.endpoint(),
        "https://endpoint-canary.invalid/responses"
    );
    assert_eq!(plain.wire_model(), "wire-model-canary");
    for secret in [
        "model-canary",
        "account-canary",
        "profile-canary",
        "endpoint-canary",
        "wire-model-canary",
    ] {
        assert!(!format!("{plain:?}").contains(secret));
    }
    let mismatch = ControllerModelClient::new(
        selection.clone(),
        Arc::new(ProjectingClient {
            reported: selection,
            projected: selected("different-model"),
            sends: sends.clone(),
        }),
    );
    assert!(mismatch.plain_facts().is_err());
    assert_eq!(sends.load(Ordering::SeqCst), 0);
}

struct LegacySelectedClient(ProjectingClient);
#[cfg_attr(target_arch = "wasm32", async_trait::async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait::async_trait)]
impl crate::AgentLlmClient for LegacySelectedClient {
    fn controller_model_selection(&self) -> Option<ControllerModelSelection> {
        Some(self.0.reported.clone())
    }
    async fn stream_response(
        &self,
        _: &[crate::Message],
        _: &[Arc<crate::ToolDef>],
        _: u32,
        _: Option<f32>,
        _: Option<&crate::ProviderParamsOverride>,
    ) -> Result<crate::LlmStreamResult, crate::AgentError> {
        self.0.sends.fetch_add(1, Ordering::SeqCst);
        Err(crate::AgentError::ConfigError(
            "unexpected legacy send".into(),
        ))
    }
    fn provider(&self) -> crate::Provider {
        self.0.reported.provider()
    }
    fn model(&self) -> &str {
        self.0.reported.model()
    }
}
#[test]
fn selected_legacy_client_is_not_a_plain_controller_fact_producer() {
    let selected = selected("legacy-selected");
    let sends = Arc::new(AtomicUsize::new(0));
    let pin = ControllerModelClient::new(
        selected.clone(),
        Arc::new(LegacySelectedClient(ProjectingClient {
            reported: selected.clone(),
            projected: selected,
            sends: sends.clone(),
        })),
    );
    assert!(
        pin.plain_facts().is_err(),
        "selection alone cannot invent provider route facts"
    );
    assert_eq!(sends.load(Ordering::SeqCst), 0);
}
