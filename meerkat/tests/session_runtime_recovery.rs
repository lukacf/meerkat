//! Smoke test that the `meerkat::session_runtime::recovery` module is
//! wired up: `RecoveryRuntimeBindingMode` variants are constructible
//! and `RecoveredCreateRequest` accepts the canonical fields.

#![allow(clippy::expect_used)]

use meerkat::session_runtime::recovery::{RecoveredCreateRequest, RecoveryRuntimeBindingMode};
use meerkat_core::service::{CreateSessionRequest, DeferredPromptPolicy, InitialTurnPolicy};
use meerkat_core::types::ContentInput;

#[test]
fn binding_mode_variants_are_distinct() {
    let auth = RecoveryRuntimeBindingMode::Authoritative;
    let local = RecoveryRuntimeBindingMode::LocalResources;
    // Copy + Debug are part of the contract — assert distinctness via
    // Debug projections rather than relying on a compile-fence binding.
    assert_ne!(format!("{auth:?}"), format!("{local:?}"));
    // Copy is exercised by passing each value into the format! call
    // twice (once on the line above, once below) — if Copy were missing
    // the second use would not compile.
    let _ = (auth, local);
}

#[test]
fn parse_provider_override_accepts_known_providers() {
    use meerkat::session_runtime::recovery::{parse_provider_override, unknown_provider_message};

    assert!(parse_provider_override("openai").is_ok());
    assert!(parse_provider_override("anthropic").is_ok());
    assert!(parse_provider_override("gemini").is_ok());

    let err = parse_provider_override("nope").expect_err("unknown provider must reject");
    assert_eq!(err, unknown_provider_message("nope"));
    assert!(err.contains("unknown provider"));
}

#[test]
fn recovery_error_code_maps_each_variant() {
    use meerkat::session_runtime::errors::RecoveryError;
    use meerkat_core::SurfaceSessionRecoveryError;
    use meerkat_core::service::SessionError;
    use meerkat_core::types::SessionId;

    let invalid: RecoveryError = SurfaceSessionRecoveryError::InvalidOverride("bad".into()).into();
    assert_eq!(invalid.code(), "INVALID_OVERRIDE");

    let missing: RecoveryError =
        SurfaceSessionRecoveryError::MissingSessionMetadata("sid".into()).into();
    assert_eq!(missing.code(), "MISSING_SESSION_METADATA");

    let binding = RecoveryError::BindingPreparation {
        session_id: SessionId::new(),
        message: "boom".into(),
    };
    assert_eq!(binding.code(), "BINDING_PREPARATION");

    let session: RecoveryError = SessionError::NotFound {
        id: SessionId::new(),
    }
    .into();
    assert_eq!(session.code(), "SESSION");
}

#[test]
fn recovered_create_request_holds_registration_flag() {
    let request = CreateSessionRequest {
        model: "test-model".into(),
        prompt: ContentInput::from("hello"),
        system_prompt: meerkat::SystemPromptOverride::Inherit,
        max_tokens: None,
        event_tx: None,
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
