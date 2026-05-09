//! Smoke test that the `meerkat::session_runtime::recovery` module is
//! wired up: `RecoveryRuntimeBindingMode` variants are constructible
//! and `RecoveredCreateRequest` accepts the canonical fields.

use meerkat::session_runtime::recovery::{RecoveredCreateRequest, RecoveryRuntimeBindingMode};
use meerkat_core::service::{CreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy};
use meerkat_core::types::ContentInput;

#[test]
fn binding_mode_variants_are_distinct() {
    let auth = RecoveryRuntimeBindingMode::Authoritative;
    let local = RecoveryRuntimeBindingMode::LocalResources;
    // Copy + Debug are part of the contract — the test is a compile fence
    // over those derives.
    let _auth_copy = auth;
    let _local_copy = local;
    assert_ne!(format!("{auth:?}"), format!("{local:?}"));
}

#[test]
fn recovered_create_request_holds_registration_flag() {
    let request = CreateSessionRequest {
        model: "test-model".into(),
        prompt: ContentInput::from("hello"),
        render_metadata: None,
        system_prompt: None,
        max_tokens: None,
        event_tx: None,
        skill_references: None,
        initial_turn: InitialTurnPolicy::RunImmediately,
        deferred_prompt_policy: DeferredPromptPolicy::default(),
        build: None,
        labels: None,
    };
    let recovered = RecoveredCreateRequest {
        request,
        runtime_was_registered: true,
    };
    assert!(recovered.runtime_was_registered);
    assert_eq!(recovered.request.model, "test-model");
}
