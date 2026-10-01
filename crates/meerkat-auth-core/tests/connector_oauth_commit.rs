#![cfg(not(target_arch = "wasm32"))]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use chrono::Duration;
use meerkat_auth_core::auth_store::{
    PersistedAuthMode, PersistedTokens, ProviderAuthPersistence, TokenKey, TokenStore,
};
use meerkat_auth_core::{BrowserOAuthFlowCommit, save_oauth_tokens_and_consume_browser_flow};
use meerkat_core::{AuthBindingRef, AuthCredentialIdentity, BindingId, BindingOrigin, RealmId};
use std::sync::Arc;

fn binding() -> AuthBindingRef {
    AuthBindingRef {
        realm: RealmId::parse("global").expect("valid realm"),
        binding: BindingId::parse("openai").expect("valid binding"),
        profile: None,
        origin: BindingOrigin::Configured,
    }
}

fn tokens() -> PersistedTokens {
    let now = chrono::Utc::now();
    PersistedTokens {
        auth_mode: PersistedAuthMode::ChatgptOauth,
        primary_secret: Some("access-token".to_string()),
        refresh_token: Some("refresh-token".to_string()),
        id_token: None,
        expires_at: Some(now + Duration::hours(1)),
        last_refresh: Some(now),
        scopes: vec!["openid".to_string()],
        account_id: None,
        metadata: serde_json::Value::Null,
    }
}

#[tokio::test]
async fn connector_commit_refuses_unbound_tokens_then_consumes_and_publishes_exact_evidence() {
    use meerkat_auth_core::connector_oauth::{
        ConnectorAccountObservation, ConnectorOAuthDescriptor, ConnectorOAuthParameters,
    };
    let runtime = meerkat_runtime::MeerkatMachine::ephemeral();
    let authority = runtime.oauth_flow_authority();
    let auth_lease = authority
        .generated_credential_lifecycle()
        .expect("actual runtime supplies its matched lease");
    let store = Arc::new(meerkat_auth_core::auth_store::EphemeralTokenStore::new());
    let persistence = ProviderAuthPersistence::new(
        store.clone(),
        Arc::new(meerkat_auth_core::auth_store::InMemoryCoordinator::new()),
    );
    let target = AuthCredentialIdentity::from_auth_binding(&binding());
    let redirect = "http://127.0.0.1:12345/callback";
    let descriptor: ConnectorOAuthDescriptor = ConnectorOAuthParameters {
        issuer: "https://issuer.example".into(),
        client: "client".into(),
        resource: "https://service.example".into(),
        scopes: ["service.read".into()].into(),
        redirect_uri: redirect.into(),
        expected_account: "verified-account".into(),
        strategy_id: "verified-test-profile".into(),
    }
    .try_into()
    .expect("valid descriptor");
    let evidence = descriptor
        .verify_account(
            ConnectorAccountObservation {
                account: "verified-account".into(),
                granted_scopes: ["service.read".into()].into(),
            },
            &meerkat_auth_core::auth_oauth::OAuthTokenResult {
                access_token: "access-token".into(),
                refresh_token: Some("refresh-token".into()),
                id_token: None,
                expires_in_secs: Some(3600),
                scope: Some("service.read".into()),
            },
        )
        .expect("correct observation");
    let state = authority
        .start(
            target.clone(),
            descriptor.clone().into(),
            redirect.into(),
            "secret-pkce".into(),
        )
        .expect("admit");
    let flow = BrowserOAuthFlowCommit {
        authority: authority.clone(),
        state: state.clone(),
        completion: evidence.into(),
        redirect_uri: redirect.into(),
    };
    let mut token = tokens();
    token.auth_mode = meerkat_core::auth::token_store::PersistedAuthMode::McpOauth;
    token.scopes = vec!["service.read".into()];
    token.account_id = Some("wrong-account".into());
    assert!(
        save_oauth_tokens_and_consume_browser_flow(
            persistence.clone(),
            auth_lease.clone(),
            target.clone(),
            token.clone(),
            flow.clone()
        )
        .await
        .is_err()
    );
    assert!(
        store
            .load(&TokenKey::from_credential_identity(&target))
            .await
            .expect("load")
            .is_none()
    );
    assert!(
        authority
            .verify(&state, &target, descriptor.clone().into(), redirect)
            .is_ok(),
        "invalid token binding must not consume the valid attempt"
    );
    token.account_id = Some("verified-account".into());
    let mut swapped = token.clone();
    swapped.primary_secret = Some("different-account-access-token".into());
    assert!(
        save_oauth_tokens_and_consume_browser_flow(
            persistence.clone(),
            auth_lease.clone(),
            target.clone(),
            swapped,
            flow.clone()
        )
        .await
        .is_err(),
        "matching account/scope labels must not bless different token material"
    );
    assert!(
        authority
            .verify(&state, &target, descriptor.clone().into(), redirect)
            .is_ok()
    );
    let committed = save_oauth_tokens_and_consume_browser_flow(
        persistence,
        auth_lease,
        target.clone(),
        token,
        flow,
    )
    .await
    .expect("commit verified credential");
    assert_eq!(committed.account_id.as_deref(), Some("verified-account"));
    assert_eq!(
        store
            .load(&TokenKey::from_credential_identity(&target))
            .await
            .expect("load"),
        Some(committed)
    );
    assert!(
        authority
            .verify(&state, &target, descriptor.into(), redirect)
            .is_err()
    );
}
