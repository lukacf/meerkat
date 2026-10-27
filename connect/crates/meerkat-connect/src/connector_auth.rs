//! Protected host connector controls using one retained native authority.
//! No value in this module is exposed to model tools or public conversation.
use std::sync::Arc;
use meerkat::{ConnectorLoginComplete, ConnectorLoginStart, ConnectorOAuthCallback,
    ConnectorOAuthTarget, HostAuthError, HostAuthService};
use serde_json::{Value, json};

pub(crate) mod existing;

pub(crate) struct ConnectorAuth {
    pub(crate) facade: HostAuthService,
    pub(crate) context_provider: Arc<meerkat::ConnectorMcpCallContextProvider>,
    pub(crate) realm: meerkat_core::RealmId,
}

impl ConnectorAuth {
    /// Consume the already selected host facade. This shares its existing
    /// native account authority and never opens another runtime or store.
    pub(crate) fn from_facade(
        facade: HostAuthService,
        realm: meerkat_core::RealmId,
    ) -> Result<Arc<Self>, crate::ConnectError> {
        let authority = facade.connector_oauth_authority().map_err(|error|
            crate::ConnectError::provider_setup("Configure connector authority", error))?;
        Ok(Arc::new(Self {
            facade,
            context_provider: Arc::new(meerkat::ConnectorMcpCallContextProvider::live(authority)),
            realm,
        }))
    }

    pub(crate) fn router(&self) -> meerkat::McpRouter {
        meerkat::McpRouter::new().with_call_context_provider(self.context_provider.clone())
    }
}

pub(crate) struct Control {
    auth: Arc<ConnectorAuth>,
    // One child-owned host ceremony, not an account or attempt registry.
    ceremony: Option<Ceremony>,
}

#[derive(Clone)]
pub(crate) enum LoginSelection {
    Installed(Arc<crate::connector_install::InstallationLogin>),
    Existing(Arc<existing::ExistingConnector>),
}
impl LoginSelection {
    async fn check_target(&self, target: &ConnectorOAuthTarget) -> Result<(), &'static str> {
        match self {
            Self::Installed(selection) => selection.check_target(target),
            Self::Existing(selection) => selection.check_target(target).await,
        }
    }
}

enum Ceremony {
    Started(ConnectorOAuthTarget, ConnectorLoginStart, LoginSelection),
    Completed(Arc<CompletedLogin>),
}

/// Historical native login evidence for the host that owns this control.
/// This has no wire decoder and is not current credential or access authority.
/// Its target is the admitted host target, never reconstructed from a callback.
pub(crate) struct CompletedLogin {
    pub(crate) target: ConnectorOAuthTarget,
    pub(crate) native: ConnectorLoginComplete,
    selection: LoginSelection,
    installation: Result<meerkat::ConnectorMcpInstallation, &'static str>,
}

impl CompletedLogin {
    /// A separately derived host configuration, still awaiting its actual
    /// access decision, realm persistence and native attachment owners.
    pub(crate) fn prepared_installation(&self)
        -> Result<&meerkat::ConnectorMcpInstallation, &'static str>
    {
        let LoginSelection::Installed(selection) = &self.selection else {
            return Err("reconnect_has_no_installation");
        };
        selection.check_current()?;
        self.installation.as_ref().map_err(|code| *code)
    }
}

pub(crate) enum Reply {
    Started(Value),
    Completed(Arc<CompletedLogin>),
    Cancelled,
}

fn failure(error: HostAuthError) -> &'static str {
    match error {
        HostAuthError::Connector(error) if error.is_refusal() => "connector_refused",
        _ => "connector_unavailable",
    }
}

fn absent(error: &HostAuthError) -> bool {
    matches!(error, HostAuthError::Connector(meerkat::ConnectorLoginError::Flow(
        meerkat_auth_core::oauth_flow::OAuthFlowError::Missing)))
}

impl Control {
    pub(crate) fn new(auth: Arc<ConnectorAuth>) -> Self {
        Self { auth, ceremony: None }
    }

    pub(crate) fn realm(&self) -> &meerkat_core::RealmId { &self.auth.realm }

    pub(crate) fn router(&self) -> meerkat::McpRouter { self.auth.router() }

    pub(crate) fn pending_slot(&self) -> Option<&meerkat_core::CredentialAccountRef> {
        match &self.ceremony { Some(Ceremony::Started(target, _, _)) => Some(&target.slot), _ => None }
    }

    /// Borrow the exact completed ceremony. The selected host must still join
    /// its installation/operation owner and current native credential/grant
    /// state before requesting access or finalizing a connection.
    pub(crate) fn completed(&self) -> Option<&CompletedLogin> {
        match &self.ceremony {
            Some(Ceremony::Completed(completed)) => Some(completed),
            _ => None,
        }
    }

    /// Borrow native ceremony data for the protected terminal only. This is
    /// not reconstructed from the wire reply and is never given to tools.
    pub(crate) fn started(&self) -> Option<&ConnectorLoginStart> {
        match &self.ceremony {
            Some(Ceremony::Started(_, started, _)) => Some(started),
            _ => None,
        }
    }

    pub(crate) async fn start(
        &mut self, target: ConnectorOAuthTarget, redirect_uri: String,
        selection: crate::connector_install::InstallationLogin,
    ) -> Result<Reply, &'static str> {
        self.start_selected(target, redirect_uri, LoginSelection::Installed(Arc::new(selection))).await
    }

    pub(crate) async fn start_selected(
        &mut self, target: ConnectorOAuthTarget, redirect_uri: String, selection: LoginSelection,
    ) -> Result<Reply, &'static str> {
        if matches!(&self.ceremony, Some(Ceremony::Started(..))) { return Err("busy"); }
        if target.slot.realm != self.auth.realm { return Err("invalid_binding"); }
        selection.check_target(&target).await?;
        let started = self.auth.facade.connector_login_start(&target, &redirect_uri)
            .await.map_err(failure)?;
        let mut reply = json!({"authorize_url":started.authorize_url,"state":started.state,
            "redirect_uri":started.redirect_uri});
        if let Some(handle) = &started.host_attempt_handle {
            reply["host_attempt_handle"] = json!(handle);
        }
        // A new admitted attempt replaces the completed ceremony. A refused
        // start leaves already-known completed authentication intact.
        self.ceremony = Some(Ceremony::Started(target, started, selection));
        Ok(Reply::Started(reply))
    }

    pub(crate) async fn complete(
        &mut self, state: String, code: String, host_attempt_handle: Option<String>,
        retired: &mut tokio::sync::watch::Receiver<bool>,
        cancel: tokio::sync::oneshot::Receiver<()>,
    ) -> (Result<Reply, &'static str>, Option<Result<(), &'static str>>, bool) {
        let Some(Ceremony::Started(target, started, selection)) = self.ceremony.as_ref() else {
            return (Err("connector_refused"), None, false);
        };
        // Scope chooses this child. The host chooses its retained ceremony;
        // callback bytes never select a target, realm, profile or redirect.
        if state != started.state { return (Err("connector_refused"), None, false); }
        if let Err(code) = selection.check_target(target).await { return (Err(code), None, false); }
        let admitted_target = target.clone();
        let admitted_selection = selection.clone();
        let (completed, cancellation) = {
            let completion = self.auth.facade.connector_login_complete(target, ConnectorOAuthCallback {
                redirect_uri: started.redirect_uri.clone(), state, code, host_attempt_handle,
            });
            tokio::pin!(completion);
            tokio::select! {
                biased;
                _ = async {
                    tokio::select! { biased;
                        _ = retired.changed() => {},
                        _ = cancel => {},
                    }
                } => {
                    // Retire the native attempt before it can publish, then join
                    // this same entered completion. Never cancel by dropping it.
                    let cancellation = self.auth.facade.connector_login_cancel(&target.slot, &started.state);
                    (completion.await, Some(cancellation))
                }
                completed = &mut completion => (completed, None),
            }
        };
        let cleanup_failed = cancellation.as_ref().is_some_and(|result|
            matches!(result, Err(error) if !absent(error)));
        // Ok means cancellation won. Missing means the native attempt was
        // already terminal; only the joined completion determines its result.
        if cancellation.as_ref().is_some_and(|result|
            result.is_ok() || matches!(result, Err(error) if absent(error))) {
            self.ceremony = None;
        }
        let reply = match completed {
            Ok(completed) => {
                // Take only this admitted ceremony. A lost/changed software
                // observation cannot erase a known completed native login.
                let installation = match &admitted_selection {
                    LoginSelection::Installed(selection) => selection.join(&admitted_target, &completed),
                    LoginSelection::Existing(_) => Err("reconnect_has_no_installation"),
                };
                let completed = Arc::new(CompletedLogin {
                    target: admitted_target,
                    native: completed,
                    selection: admitted_selection,
                    installation,
                });
                self.ceremony = Some(Ceremony::Completed(completed.clone()));
                Ok(Reply::Completed(completed))
            }
            Err(error) => Err(failure(error)),
        };
        (reply, cancellation.map(|result| result.map_err(failure)), cleanup_failed)
    }

    pub(crate) fn cancel(&mut self) -> Result<Reply, &'static str> {
        let Some(Ceremony::Started(target, started, _)) = self.ceremony.as_ref() else {
            // A finished login is not an active attempt to cancel. Preserve
            // its known result; disconnect requires the native logout owner.
            return Err("connector_refused");
        };
        match self.auth.facade.connector_login_cancel(&target.slot, &started.state) {
            Ok(()) => { self.ceremony = None; Ok(Reply::Cancelled) }
            Err(error) if absent(&error) => {
                self.ceremony = None;
                Err("connector_refused")
            }
            Err(error) => Err(failure(error)),
        }
    }

    pub(crate) fn retire(&mut self) -> Result<(), &'static str> {
        if !matches!(&self.ceremony, Some(Ceremony::Started(..))) { return Ok(()); }
        match self.cancel() {
            Ok(_) => Ok(()),
            Err(_) if self.ceremony.is_none() => Ok(()),
            Err(_) => Err("shutdown_unconfirmed"),
        }
    }
}
