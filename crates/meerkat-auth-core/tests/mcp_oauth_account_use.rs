//! Actual native account-use regression, using file credentials and SQLite flow persistence.
#![cfg(all(not(target_arch = "wasm32"), feature = "oauth", feature = "file-lock"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! No token store write or replacement authority is implemented in this test.

#[path = "mcp_oauth_account_use_support/provider.rs"]
mod provider;

use std::collections::BTreeSet;
use std::panic::AssertUnwindSafe;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use futures::FutureExt;
use meerkat_auth_core::auth_oauth::OAuthTokenResult;
use meerkat_auth_core::auth_store::{FileLockCoordinator, FileTokenStore, ProviderAuthPersistence};
use meerkat_auth_core::connector_oauth::{
    ConnectorAccountObservation, ConnectorOAuthDescriptor, ConnectorOAuthParameters,
    ConnectorOAuthRefusal,
};
use meerkat_auth_core::oauth_flow::{OAuthFlowAuthority, OAuthFlowRegistrySnapshot};
use meerkat_auth_core::{
    BrowserOpener, McpOAuthAccountStrategy, McpOAuthAuthority, McpOAuthCeremonyContext,
    McpOAuthError, McpServerIdentity,
};
use meerkat_core::handles::GeneratedAuthLeaseHandle;
use meerkat_runtime::handles::{RuntimeAuthLeaseHandle, RuntimeOAuthFlowHandle};
use meerkat_runtime::store::{RuntimeStore, SqliteRuntimeStore};
use provider::SelectedProfile;
use reqwest::{StatusCode, Url};
use serde::Deserialize;
use serde_json::json;

fn private<T, E>(result: Result<T, E>, message: &'static str) -> T {
    result.unwrap_or_else(|_| panic!("{message}"))
}

// Use the actual native persistence owners and public stored-use API.
// No consumer resolver, fixture token write or account-check facade is added.
struct AuthOwners {
    store: Arc<dyn RuntimeStore>,
    flows: Arc<dyn OAuthFlowAuthority>,
    lease: GeneratedAuthLeaseHandle,
    persistence: ProviderAuthPersistence,
}

impl AuthOwners {
    fn open(root: &Path) -> Self {
        let store: Arc<dyn RuntimeStore> = Arc::new(private(
            SqliteRuntimeStore::new(root.join("native.sqlite3")),
            "native SQLite open failed",
        ));
        let lifecycle = Arc::new(RuntimeAuthLeaseHandle::new());
        let flows: Arc<dyn OAuthFlowAuthority> = Arc::new(
            RuntimeOAuthFlowHandle::new_with_persistent_store_and_auth_lease(
                Duration::from_secs(300),
                lifecycle,
                &store,
            ),
        );
        let lease = flows
            .generated_credential_lifecycle()
            .expect("native generated lifecycle absent");
        let persistence = ProviderAuthPersistence::new(
            Arc::new(FileTokenStore::new(root.join("vault"))),
            Arc::new(FileLockCoordinator::new(root.join("locks"))),
        );
        Self {
            store,
            flows,
            lease,
            persistence,
        }
    }

    fn require_no_pending_attempts(&self) {
        let bytes = private(
            self.store.load_auth_oauth_flow_snapshot(),
            "native persisted flow read failed",
        )
        .expect("native persisted flow snapshot absent");
        let snapshot: OAuthFlowRegistrySnapshot =
            private(serde_json::from_slice(&bytes), "native flow decode failed");
        assert!(
            snapshot.browser.is_empty(),
            "native browser attempt remains"
        );
        assert!(snapshot.device.is_empty(), "unexpected device attempt");
    }

    async fn require_persisted_account(&self, target: &McpServerIdentity, expected: &str) {
        // This is an assertion about native commit evidence, never a pre-use
        // authorization check and never a token-store write by the fixture.
        let key = private(target.token_key(), "native token key failed");
        let tokens = private(
            self.persistence.token_store().load(&key).await,
            "native credential read failed",
        )
        .expect("native credential absent after login");
        assert!(
            tokens.account_id.as_deref() == Some(expected),
            "native committed credential has wrong account evidence"
        );
        assert!(tokens.primary_secret.is_some(), "native bearer absent");
    }
}

struct AccountStrategy {
    target: McpServerIdentity,
    profile: SelectedProfile,
    http: reqwest::Client,
    observed: AtomicUsize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AccountReply {
    account: String,
    scopes: BTreeSet<String>,
}

async fn provider_account(
    http: &reqwest::Client,
    base: &str,
    bearer: &str,
) -> Result<ConnectorAccountObservation, ConnectorOAuthRefusal> {
    let failed = || ConnectorOAuthRefusal::VerificationUnavailable;
    let mut response = http
        .get(format!("{base}/account"))
        .bearer_auth(bearer)
        .send()
        .await
        .map_err(|_| failed())?;
    if response.status() != StatusCode::OK {
        return Err(failed());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| failed())? {
        if bytes.len() + chunk.len() > 8192 {
            return Err(failed());
        }
        bytes.extend_from_slice(&chunk);
    }
    let reply: AccountReply = serde_json::from_slice(&bytes).map_err(|_| failed())?;
    Ok(ConnectorAccountObservation {
        account: reply.account,
        granted_scopes: reply.scopes,
    })
}

#[async_trait]
impl McpOAuthAccountStrategy for AccountStrategy {
    fn descriptor(
        &self,
        target: &McpServerIdentity,
        context: &McpOAuthCeremonyContext<'_>,
    ) -> Result<ConnectorOAuthDescriptor, ConnectorOAuthRefusal> {
        if target != &self.target
            || context.issuer != self.profile.issuer
            || context.client != self.profile.client
            || context.resource != self.profile.resource
        {
            return Err(ConnectorOAuthRefusal::DescriptorMismatch);
        }
        ConnectorOAuthParameters {
            issuer: context.issuer.to_owned(),
            client: context.client.to_owned(),
            resource: context.resource.to_owned(),
            redirect_uri: context.redirect_uri.to_owned(),
            scopes: self.profile.scopes.clone(),
            expected_account: self.profile.expected_account.clone(),
            strategy_id: self.profile.strategy_id.clone(),
        }
        .try_into()
    }

    async fn observe_account(
        &self,
        _descriptor: &ConnectorOAuthDescriptor,
        tokens: &OAuthTokenResult,
    ) -> Result<ConnectorAccountObservation, ConnectorOAuthRefusal> {
        let observed =
            provider_account(&self.http, &self.profile.issuer, &tokens.access_token).await?;
        self.observed.fetch_add(1, Ordering::SeqCst);
        Ok(observed)
    }
}

// Fixture-only automated user. It follows exactly the selected provider's
// authorize -> native loopback callback route; redirects remain disabled.
// It enters no external browser, provider, cloud or user account.
struct AccountBrowser {
    base: String,
    account: usize,
    http: reqwest::Client,
    opened: AtomicUsize,
}

#[async_trait]
impl BrowserOpener for AccountBrowser {
    async fn open(&self, authorization_url: &str) -> Result<(), McpOAuthError> {
        let failed = || McpOAuthError::Browser("local fixture browser refused".into());
        let mut authorization_url = Url::parse(authorization_url).map_err(|_| failed())?;
        if authorization_url.origin().ascii_serialization() != self.base
            || authorization_url.path() != "/authorize"
        {
            return Err(failed());
        }
        authorization_url
            .query_pairs_mut()
            .append_pair("fixture_account", provider::ACCOUNTS[self.account]);
        let response = self
            .http
            .get(authorization_url)
            .send()
            .await
            .map_err(|_| failed())?;
        if response.status() != StatusCode::FOUND {
            return Err(failed());
        }
        let callback = response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| Url::parse(value).ok())
            .ok_or_else(failed)?;
        if callback.scheme() != "http"
            || callback.host_str() != Some("127.0.0.1")
            || callback.port().is_none()
            || callback.path() != "/mcp/oauth/callback"
            || !callback.username().is_empty()
            || callback.password().is_some()
            || callback.fragment().is_some()
        {
            return Err(failed());
        }
        let response = self
            .http
            .get(callback)
            .header("connection", "close")
            .send()
            .await
            .map_err(|_| failed())?;
        if !response.status().is_success() {
            return Err(failed());
        }
        self.opened.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn native_authority(
    owners: &AuthOwners,
    browser: Arc<AccountBrowser>,
    strategy: Arc<AccountStrategy>,
) -> McpOAuthAuthority {
    private(
        McpOAuthAuthority::with_http(
            owners.persistence.clone(),
            browser,
            provider::http(),
            owners.lease.clone(),
        )
        .with_interactive_strategy(owners.flows.clone(), strategy),
        "actual native OAuth authority refused matching owners",
    )
}

async fn require_selected_use(
    authority: &McpOAuthAuthority,
    target: &McpServerIdentity,
    base: &str,
    expected: &str,
) {
    let bearer = private(
        authority.stored_bearer_token(target).await,
        "selected native resolver control refused",
    )
    .expect("selected native resolver control returned no credential");
    let observation = private(
        provider_account(&provider::http(), base, &bearer).await,
        "selected resolver bearer was not accepted by local provider",
    );
    assert!(
        observation.account == expected,
        "selected resolver control reached wrong provider account"
    );
}

struct Observation {
    refused: bool,
    returned_account: Option<String>,
    verifier_called_on_stale_use: bool,
    reopened_refused: bool,
    reopened_returned_account: Option<String>,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn same_server_account_replacement_never_returns_other_account() {
    let directory = tempfile::tempdir().expect("fixture directory failed");
    let owners = AuthOwners::open(directory.path());
    let mut provider = provider::Fixture::start().await;
    let body = AssertUnwindSafe(async {
        let target_a = McpServerIdentity::from_server_config(
            "same-selected-server",
            format!("{}/mcp", provider.base),
        )
        .with_expected_account(provider::ACCOUNTS[0])
        .unwrap();
        let target_b = McpServerIdentity::from_server_config(
            "same-selected-server",
            format!("{}/mcp", provider.base),
        )
        .with_expected_account(provider::ACCOUNTS[1])
        .unwrap();
        assert_eq!(target_a.server_name(), target_b.server_name());
        assert_eq!(target_a.server_url(), target_b.server_url());
        assert_ne!(target_a.token_key().unwrap(), target_b.token_key().unwrap());
        assert_ne!(target_a.lease_key().unwrap(), target_b.lease_key().unwrap());
        let strategy_a = Arc::new(AccountStrategy {
            target: target_a.clone(),
            profile: provider.profile(0),
            http: provider::http(),
            observed: AtomicUsize::new(0),
        });
        let strategy_b = Arc::new(AccountStrategy {
            target: target_b.clone(),
            profile: provider.profile(1),
            http: provider::http(),
            observed: AtomicUsize::new(0),
        });
        let browser_a = Arc::new(AccountBrowser {
            base: provider.base.clone(),
            account: 0,
            http: provider::http(),
            opened: AtomicUsize::new(0),
        });
        let browser_b = Arc::new(AccountBrowser {
            base: provider.base.clone(),
            account: 1,
            http: provider::http(),
            opened: AtomicUsize::new(0),
        });
        let a = native_authority(&owners, browser_a.clone(), strategy_a.clone());
        let b = native_authority(&owners, browser_b.clone(), strategy_b.clone());

        drop(private(
            a.interactive_login(&target_a, None).await,
            "real account A native login failed",
        ));
        owners.require_no_pending_attempts();
        owners
            .require_persisted_account(&target_a, provider::ACCOUNTS[0])
            .await;
        assert!(strategy_a.observed.load(Ordering::SeqCst) == 1);
        require_selected_use(&a, &target_a, &provider.base, provider::ACCOUNTS[0]).await;

        // Authorized B login uses the same server name/URL, file vault,
        // lifecycle owner, flow owner and coordinator, with explicit native
        // account selection. Only native login writes these credentials.
        drop(private(
            b.interactive_login(&target_b, None).await,
            "real account B replacement login failed",
        ));
        owners.require_no_pending_attempts();
        owners
            .require_persisted_account(&target_b, provider::ACCOUNTS[1])
            .await;
        assert!(strategy_b.observed.load(Ordering::SeqCst) == 1);
        require_selected_use(&b, &target_b, &provider.base, provider::ACCOUNTS[1]).await;

        let a_observations = strategy_a.observed.load(Ordering::SeqCst);
        let stale = a.stored_bearer_token(&target_a).await;
        let refused = matches!(&stale, Err(_) | Ok(None));
        let returned_account = match stale {
            Ok(Some(bearer)) => Some(
                private(
                    provider_account(&provider::http(), &provider.base, &bearer).await,
                    "stale resolver returned an unrecognized fixture credential",
                )
                .account,
            ),
            _ => None,
        };
        owners.require_no_pending_attempts();
        // Reopen the same actual durable stores with a cold native lifecycle.
        // This verifies persisted selection use without any fixture credential
        // writes or carrying the old process-local lease into the new owner.
        let reopened_owners = AuthOwners::open(directory.path());
        let reopened_a = native_authority(&reopened_owners, browser_a.clone(), strategy_a.clone());
        let reopened = reopened_a.stored_bearer_token(&target_a).await;
        let reopened_refused = matches!(&reopened, Err(_) | Ok(None));
        let reopened_returned_account = match reopened {
            Ok(Some(bearer)) => Some(
                private(
                    provider_account(&provider::http(), &provider.base, &bearer).await,
                    "reopened owner returned an unrecognized fixture credential",
                )
                .account,
            ),
            _ => None,
        };
        reopened_owners.require_no_pending_attempts();
        drop(reopened_a);
        drop(reopened_owners);
        // Observation cannot silently invoke another interactive login.
        assert!(browser_a.opened.load(Ordering::SeqCst) == 1);
        assert!(browser_b.opened.load(Ordering::SeqCst) == 1);
        Observation {
            refused,
            returned_account,
            verifier_called_on_stale_use: strategy_a.observed.load(Ordering::SeqCst)
                != a_observations,
            reopened_refused,
            reopened_returned_account,
        }
    })
    .catch_unwind()
    .await;

    // Join the fixture-owned HTTP task before the decisive assertion.
    // Successful native login already joined its own actual
    // callback server. No helper login task or subprocess is created here.
    let cleanup_errors = provider.close().await;
    drop(provider);
    drop(owners);
    let observation = match body {
        Ok(observation) => {
            assert!(
                cleanup_errors.is_empty(),
                "fixture cleanup did not join every owner"
            );
            observation
        }
        Err(panic) => {
            if !cleanup_errors.is_empty() {
                eprintln!("fixture cleanup also failed; preserving original body failure");
            }
            std::panic::resume_unwind(panic)
        }
    };
    eprintln!(
        "ACCOUNT_USE_OBSERVATION {}",
        json!({
            "a_login_verified": true,
            "b_replacement_verified": true,
            "same_server_name_and_url": true,
            "distinct_selected_account_keys": true,
            "a_initial_use_verified": true,
            "b_current_use_verified": true,
            "stale_a_refused": observation.refused,
            "stale_a_returned_account": observation.returned_account,
            "a_verifier_called_on_stale_use": observation.verifier_called_on_stale_use,
            "reopened_a_refused": observation.reopened_refused,
            "reopened_a_returned_account": observation.reopened_returned_account,
            "fixture_owners_joined": true,
        })
    );
    let safe = (observation.refused
        || observation.returned_account.as_deref() == Some(provider::ACCOUNTS[0]))
        && (observation.reopened_refused
            || observation.reopened_returned_account.as_deref() == Some(provider::ACCOUNTS[0]));
    assert!(
        safe,
        "account A selection must retain account A or refuse; it must never receive account B"
    );
}
