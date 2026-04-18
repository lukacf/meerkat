#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! T2 top-down integration test for provider auth v2 (Phase 2).
//!
//! Exercises the full resolution chain per provider: `RealmConnectionSet`
//! in memory → `ProviderRuntimeRegistry::default().resolve(...)` →
//! `build_client(...)` → `.provider()`. Originally written during the
//! T1+T2 scaffolding phase with red-at-assertion reds; flipped green once
//! Phase 2 leaf slices landed the per-provider runtimes and the registry
//! dispatch.
//!
//! See /Users/luka/.claude/plans/yes-make-a-plan-shimmying-bengio.md.

use std::collections::BTreeMap;

use meerkat_client::{ProviderAuthError, ProviderRuntimeRegistry, ResolverEnvironment};
use meerkat_core::{
    AuthProfile, BackendProfile, BindingPolicy, CredentialSourceSpec, Provider, ProviderBinding,
    RealmConnectionSet,
};

/// Build an in-memory RealmConnectionSet carrying one binding per provider
/// with InlineSecret auth.
fn fixture_realm() -> RealmConnectionSet {
    let mut backends = BTreeMap::new();
    let mut auth_profiles = BTreeMap::new();
    let mut bindings = BTreeMap::new();

    for (id, provider, backend_kind) in [
        ("openai_api", Provider::OpenAI, "openai_api"),
        ("anthropic_api", Provider::Anthropic, "anthropic_api"),
        ("google_genai", Provider::Gemini, "google_genai"),
    ] {
        backends.insert(
            id.to_string(),
            BackendProfile {
                id: id.to_string(),
                provider,
                backend_kind: backend_kind.to_string(),
                base_url: None,
                options: serde_json::Value::Null,
            },
        );
    }

    for (id, provider, secret) in [
        ("openai_key", Provider::OpenAI, "sk-openai"),
        ("anthropic_key", Provider::Anthropic, "sk-anthropic"),
        ("google_key", Provider::Gemini, "sk-google"),
    ] {
        auth_profiles.insert(
            id.to_string(),
            AuthProfile {
                id: id.to_string(),
                provider,
                auth_method: "api_key".into(),
                source: CredentialSourceSpec::InlineSecret {
                    secret: secret.into(),
                },
                storage: Default::default(),
                constraints: Default::default(),
                metadata_defaults: Default::default(),
            },
        );
    }

    for (id, backend_id, auth_id) in [
        ("default_openai", "openai_api", "openai_key"),
        ("default_anthropic", "anthropic_api", "anthropic_key"),
        ("default_google", "google_genai", "google_key"),
    ] {
        bindings.insert(
            id.to_string(),
            ProviderBinding {
                id: id.to_string(),
                backend_profile: backend_id.into(),
                auth_profile: auth_id.into(),
                default_model: None,
                policy: BindingPolicy::default(),
            },
        );
    }

    RealmConnectionSet {
        realm_id: "dev".into(),
        backends,
        auth_profiles,
        bindings,
        default_binding: Some("default_openai".into()),
    }
}

#[tokio::test]
async fn openai_api_key_roundtrip_resolves_and_builds_client() {
    // T2 happy path: resolve → shim_credential::Secret → build_client → .provider().
    let registry = ProviderRuntimeRegistry::default();
    let realm = fixture_realm();
    let env = ResolverEnvironment::testing();

    let connection = registry
        .resolve(&realm, "default_openai", &env)
        .await
        .expect("Phase 2 L2.12 should make resolve() return Ok for default_openai");

    // C2 observable: credential material is a simple Secret.
    assert_eq!(
        connection.resolved_secret(),
        Some("sk-openai".to_string()),
        "InlineSecret should materialize as Some(secret)",
    );
    assert_eq!(connection.provider, Provider::OpenAI);

    // C3 observable: build_client produces a client whose .provider() string matches.
    let client = registry
        .build_client(connection)
        .expect("Phase 2 L2.9 should build an OpenAI client");
    assert_eq!(client.provider(), "openai");
}

#[tokio::test]
async fn anthropic_api_key_roundtrip_resolves_and_builds_client() {
    let registry = ProviderRuntimeRegistry::default();
    let realm = fixture_realm();
    let env = ResolverEnvironment::testing();

    let connection = registry
        .resolve(&realm, "default_anthropic", &env)
        .await
        .expect("Phase 2 L2.12 should make resolve() return Ok for default_anthropic");

    assert_eq!(
        connection.resolved_secret(),
        Some("sk-anthropic".to_string()),
    );
    assert_eq!(connection.provider, Provider::Anthropic);

    let client = registry
        .build_client(connection)
        .expect("Phase 2 L2.10 should build an Anthropic client");
    assert_eq!(client.provider(), "anthropic");
}

#[tokio::test]
async fn google_api_key_roundtrip_resolves_and_builds_client() {
    let registry = ProviderRuntimeRegistry::default();
    let realm = fixture_realm();
    let env = ResolverEnvironment::testing();

    let connection = registry
        .resolve(&realm, "default_google", &env)
        .await
        .expect("Phase 2 L2.12 should make resolve() return Ok for default_google");

    assert_eq!(connection.resolved_secret(), Some("sk-google".to_string()),);
    assert_eq!(connection.provider, Provider::Gemini);

    let client = registry
        .build_client(connection)
        .expect("Phase 2 L2.11 should build a Google (Gemini) client");
    assert_eq!(client.provider(), "gemini");
}

#[tokio::test]
async fn env_lookup_missing_surfaces_missing_secret() {
    // Env source + empty env_lookup should surface AuthError::MissingSecret
    // wrapped in ProviderAuthError::Auth.
    let mut realm = fixture_realm();
    // Swap the OpenAI auth to require env lookup that won't resolve.
    realm.auth_profiles.get_mut("openai_key").unwrap().source = CredentialSourceSpec::Env {
        env: "MEERKAT_TEST_NEVER_SET".into(),
        fallback: Vec::new(),
    };

    let registry = ProviderRuntimeRegistry::default();
    let env = ResolverEnvironment::testing();

    let err = registry
        .resolve(&realm, "default_openai", &env)
        .await
        .expect_err("must fail when env lookup returns None");

    assert!(
        matches!(
            err,
            ProviderAuthError::Auth(meerkat_core::AuthError::MissingSecret)
        ),
        "expected AuthError::MissingSecret, got {err:?}",
    );
}

#[tokio::test]
async fn external_resolver_missing_surfaces_typed_error() {
    // ExternalResolver source with an unregistered handle should fail with
    // ProviderAuthError::ExternalResolverMissing.
    let mut realm = fixture_realm();
    realm.auth_profiles.get_mut("anthropic_key").unwrap().source =
        CredentialSourceSpec::ExternalResolver {
            handle: "desktop-never-registered".into(),
        };

    let registry = ProviderRuntimeRegistry::default();
    let env = ResolverEnvironment::testing(); // no external resolvers registered

    let err = registry
        .resolve(&realm, "default_anthropic", &env)
        .await
        .expect_err("must fail when external resolver handle is not registered");

    assert!(
        matches!(err, ProviderAuthError::ExternalResolverMissing(ref h) if h == "desktop-never-registered"),
        "expected ExternalResolverMissing, got {err:?}",
    );
}

#[test]
fn unsupported_combination_surfaces_typed_error() {
    // OpenAI backend + Anthropic auth must fail validation.
    let mut realm = fixture_realm();
    realm
        .bindings
        .get_mut("default_openai")
        .unwrap()
        .auth_profile = "anthropic_key".into();

    let registry = ProviderRuntimeRegistry::default();
    // Use the synchronous resolve path through build_client. Since we
    // can't easily call validate_binding from the outside without a full
    // resolve, this test depends on L2.12 surfacing validate errors from
    // resolve().
    let env = ResolverEnvironment::testing();
    let err = futures::executor::block_on(async {
        registry.resolve(&realm, "default_openai", &env).await
    })
    .expect_err("mismatched provider between backend and auth must fail");

    // Phase 2 L2.12 surfaces provider-mismatch via the binding-validation
    // path, which the registry wraps into ProviderAuthError::Binding.
    assert!(
        matches!(
            err,
            ProviderAuthError::Binding(meerkat_client::ProviderBindingError::ProviderMismatch)
        ),
        "expected ProviderAuthError::Binding(ProviderMismatch), got {err:?}",
    );
}

#[test]
fn default_registry_populated_by_features() {
    // Coexistence proof: the default registry has all feature-gated runtimes.
    let registry = ProviderRuntimeRegistry::default();
    assert!(registry.get(Provider::OpenAI).is_some());
    assert!(registry.get(Provider::Anthropic).is_some());
    assert!(registry.get(Provider::Gemini).is_some());
    assert!(registry.get(Provider::SelfHosted).is_none());
}

/// Plan §5.6 — distributed-mob member binding orthogonality.
///
/// Two mob members configured with two different `connection_ref`s
/// must resolve to two different credentials. The real contamination
/// risk is at the resolver layer (shared caches, shared token stores,
/// shared auth profile state) — if it was going to happen, it would
/// happen there. This test drives concurrent resolves from the same
/// ResolverEnvironment against two bindings with different InlineSecret
/// sources and asserts the resolved secrets match the bindings, not
/// each other. Concurrent ordering is deliberate (tokio::join!).
///
/// Full mob-spawn orthogonality belongs in meerkat-mob integration
/// tests; this test is the resolver-layer contract proof.
#[tokio::test]
async fn concurrent_resolves_preserve_per_binding_isolation() {
    let registry = ProviderRuntimeRegistry::default();
    let realm = fixture_realm();
    let env = ResolverEnvironment::testing();

    let (a, b) = tokio::join!(
        registry.resolve(&realm, "default_openai", &env),
        registry.resolve(&realm, "default_anthropic", &env),
    );
    let a = a.expect("openai binding resolves");
    let b = b.expect("anthropic binding resolves");
    assert_eq!(a.resolved_secret().as_deref(), Some("sk-openai"));
    assert_eq!(b.resolved_secret().as_deref(), Some("sk-anthropic"));
    assert_ne!(
        a.resolved_secret(),
        b.resolved_secret(),
        "two bindings with two different InlineSecrets must resolve to two \
         different credentials; shared-state contamination would surface here",
    );
    assert_eq!(a.provider, Provider::OpenAI);
    assert_eq!(b.provider, Provider::Anthropic);
}

/// Plan §4a.14 + §4b.13 closure: a realm configured with
/// `CredentialSourceSpec::Command` must actually execute the subprocess
/// via `CommandCredentialRunner` and surface its stdout as the
/// resolved secret. Prior to the wire-up the resolver returned a
/// hard-coded "not reachable from the simple-secret resolver" error,
/// which meant Codex-parity external-bearer config in a manifest
/// never worked end-to-end.
#[tokio::test]
async fn command_source_resolves_via_subprocess_runner() {
    let registry = ProviderRuntimeRegistry::default();
    let mut realm = fixture_realm();
    realm.auth_profiles.insert(
        "command_key".into(),
        AuthProfile {
            id: "command_key".into(),
            provider: Provider::OpenAI,
            auth_method: "api_key".into(),
            source: CredentialSourceSpec::Command {
                program: std::path::PathBuf::from("/bin/sh"),
                args: vec!["-c".into(), "printf '%s' 'sk-from-cmd'".into()],
                cwd: None,
                env: Default::default(),
                timeout_ms: 5_000,
                refresh_interval_ms: None,
            },
            storage: Default::default(),
            constraints: Default::default(),
            metadata_defaults: Default::default(),
        },
    );
    realm.bindings.insert(
        "command_openai".into(),
        ProviderBinding {
            id: "command_openai".into(),
            backend_profile: "openai_api".into(),
            auth_profile: "command_key".into(),
            default_model: None,
            policy: BindingPolicy::default(),
        },
    );

    let env = ResolverEnvironment::testing();
    let connection = registry
        .resolve(&realm, "command_openai", &env)
        .await
        .expect("Command source must resolve via CommandCredentialRunner");
    assert_eq!(
        connection.resolved_secret().as_deref(),
        Some("sk-from-cmd"),
        "resolver should dispatch Command source to CommandCredentialRunner \
         and materialize stdout as the InlineSecret",
    );
}

// C3 negative observable (empty-lease -> NoCredentialMaterial) is
// anchored in per-provider mod.rs unit tests since a ResolvedConnection
// cannot be constructed externally without resolve_binding producing one.
