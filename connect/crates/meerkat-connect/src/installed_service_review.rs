//! Protected account review for an installed application's selected helper.
//!
//! This composes the existing software, authentication and approval owners.
//! The review owns no wire protocol, grant issuer or attachment loop.
//! Explicit terminal composition adds the native account request tool below.
//! The native registration owner must install the returned checked dispatcher
//! on its exact setup router before enabling installation operations. Software
//! review runs inside that selected dispatcher and shares the same native
//! decision owner as private account review. A name alone cannot select it.

pub(crate) mod account_request;
pub use account_request::{InstalledAccountSelection, installed_account_request_definition};
pub use account_request::public::installed_public_service_request_definition;
pub(crate) mod composer;
pub use composer::configure_installed_service_review;
#[cfg(all(unix, feature = "console"))]
pub use composer::configure_installed_account_requests;

use std::{collections::BTreeMap, sync::Arc, time::SystemTime};

use meerkat::{
    ConnectorAuthStatus, ConnectorLoginComplete, ConnectorLoginStart, ConnectorMcpInstallation,
    ConnectorOAuthTarget, HostAuthService,
};
use meerkat_core::{
    AgentToolDispatcher, RealmLocator, SessionId,
    approval::{
        ApprovalActionKind, ApprovalDecision, ApprovalOwnerRef, ApprovalPrincipalId,
        ApprovalProposedAction, ApprovalRecord, ApprovalRequest, ApprovalResourceId,
        ApprovalResourceKind, ApprovalResourceRef, ApprovalRisk, ApprovalStatus,
    },
    mcp_config::McpConnectorOperation,
};
use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    ConnectError,
    approval::LocalApprovals,
    connector_auth::{ConnectorAuth, Control, Reply},
    connector_install::{InstallationLogin, authorized_account, reviewed_configuration},
    service_install::{CheckedInstalls, InstalledSelection},
};

/// The existing owners for one selected helper registration. The application
/// owns private presentation, admission and shutdown. This value adds no
/// ceremony state: `Control` retains the sole native login attempt and
/// `LocalApprovals` retains the sole human decision attempt.
pub struct InstalledServiceReview {
    realm: RealmLocator,
    session: SessionId,
    checked: Arc<CheckedInstalls>,
    auth: Arc<ConnectorAuth>,
    control: Control,
    approvals: Arc<LocalApprovals>,
    #[cfg(all(unix, feature = "console"))]
    realm_installation: Option<meerkat_mobkit::rpc_gateway::GatewayRealmMcpInstallation>,
}

/// Compose resources supplied by the actual installed native registration.
/// `approvals` must already be bound to this session by its private presenter.
/// `facade` is the application's existing native account owner, with any client
/// authentication profiles already selected. No runtime, router or account
/// store is created here. The adapter must be the actual selected setup router,
/// never a new router constructed from a retained configuration document.
///
/// The returned dispatcher includes owner-local software review. This does
/// not enable an installation profile or add a global admission gate. The
/// native registration hook must bind this exact dispatcher before the
/// application's selected installation profile can be enabled.
pub fn compose_installed_service_review(
    adapter: Arc<meerkat::McpRouterAdapter>,
    facade: HostAuthService,
    realm: RealmLocator,
    session: SessionId,
    approvals: Arc<LocalApprovals>,
) -> Result<InstalledServiceReview, ConnectError> {
    approvals.check_session(&session)?;
    let auth = ConnectorAuth::from_facade(facade, realm.realm.clone())?;
    compose_with_auth(adapter, auth, realm, session, approvals)
}


fn compose_with_auth(
    adapter: Arc<meerkat::McpRouterAdapter>,
    auth: Arc<ConnectorAuth>,
    realm: RealmLocator,
    session: SessionId,
    approvals: Arc<LocalApprovals>,
) -> Result<InstalledServiceReview, ConnectError> {
    approvals.check_session(&session)?;
    let checked = CheckedInstalls::with_software_review(adapter, approvals.clone());
    checked
        .bind_realm(&realm)
        .map_err(|_| refused("The selected setup router could not be bound"))?;
    approvals.check_session(&session)?;
    Ok(InstalledServiceReview {
        realm,
        session,
        checked,
        control: Control::new(auth.clone()),
        auth,
        approvals,
        #[cfg(all(unix, feature = "console"))]
        realm_installation: None,
    })
}

/// Exact application selection retained with the review. `selection` is the
/// installed application's typed grant/destination/lifetime proposal, not a
/// Toolkit substitute for it. Serialization is only for private presentation;
/// the same typed value is returned to its native policy consumer. It must not
/// contain credential payloads or raw provider/tool logs.
pub struct InstalledAccessProposal<T> {
    pub requester: ApprovalPrincipalId,
    pub summary: String,
    pub selection: T,
    /// Human decision deadline, separate from any selected access lifetime.
    pub decision_expires_at: SystemTime,
}

/// Historical account and human-decision evidence. There is no deserializer,
/// grant token or readiness flag. Native grant and attachment owners must
/// authenticate this exact application selection and recheck their own current
/// mandate, account, destination and configuration before any effect.
pub struct ReviewedInstalledAccount<T> {
    selection: T,
    decision: ApprovalRecord,
    target: ConnectorOAuthTarget,
    account: ConnectorAuthStatus,
    installation: ConnectorMcpInstallation,
    software: InstalledSelection,
    fresh_instruction_retained: bool,
}

impl<T> ReviewedInstalledAccount<T> {
    /// The caller's original typed proposal. Its current native owner must
    /// match it to the exact recorded presentation before using this evidence.
    pub fn selection(&self) -> &T {
        &self.selection
    }
    pub fn decision(&self) -> &ApprovalRecord {
        &self.decision
    }
    pub fn target(&self) -> &ConnectorOAuthTarget {
        &self.target
    }
    pub fn account_observation(&self) -> &ConnectorAuthStatus {
        &self.account
    }
    pub fn installation(&self) -> &ConnectorMcpInstallation {
        &self.installation
    }
    /// A historical fresh-source publication, not input admission or readiness.
    pub fn fresh_instruction_retained(&self) -> bool { self.fresh_instruction_retained }
    pub fn software_job(&self) -> &str {
        &self.software.job_id
    }
}

pub enum InstalledAccountReview<T> {
    Approved(ReviewedInstalledAccount<T>),
    Denied(ApprovalRecord),
}

/// Native result of the one entered ceremony, returned to the host's private
/// settlement sink. A completed login remains known even if cancellation or
/// later software validation fails. It is not permission to publish a tool
/// result after the presenter's login retires.
pub struct ProtectedAccountCompletion {
    pub result: Result<ConnectorLoginComplete, &'static str>,
    pub cancellation: Option<Result<(), &'static str>>,
    pub cleanup_unconfirmed: bool,
}

impl InstalledServiceReview {
    #[cfg(test)]
    pub(crate) fn fixture_checked(
        realm: RealmLocator, session: SessionId, checked: Arc<CheckedInstalls>,
        auth: Arc<ConnectorAuth>, approvals: Arc<LocalApprovals>,
    ) -> Result<Self, ConnectError> {
        approvals.check_session(&session)?;
        if realm.realm != auth.realm { return Err(refused("fixture realm mismatch")); }
        Ok(Self { realm, session, checked, control: Control::new(auth.clone()), auth, approvals,
            #[cfg(all(unix, feature = "console"))]
            realm_installation: None,
        })
    }

    #[cfg(test)]
    pub(crate) fn fixture_account_started(&self) -> bool {
        self.control.started().is_some()
    }

    /// Install this exact wrapper through the native registration owner. It
    /// refuses install operations until the same checked proposal is armed by
    /// that owner's software review. This getter does not install that gate.
    pub fn checked_dispatcher(&self) -> Arc<dyn AgentToolDispatcher> {
        self.checked.clone()
    }

    /// The same native credential owner used by this protected ceremony.
    pub fn call_context_provider(&self) -> Arc<meerkat::ConnectorMcpCallContextProvider> {
        self.auth.context_provider.clone()
    }

    fn selected_login(
        &self,
        job_id: &str,
        operations: BTreeMap<String, McpConnectorOperation>,
    ) -> Result<(InstallationLogin, InstalledSelection), ConnectError> {
        self.approvals.check_session(&self.session)?;
        let selected = self
            .checked
            .connector_selection(&self.realm, &self.session, job_id)
            .map_err(|_| {
                refused("The checked installation or its operation metadata is unavailable")
            })?;
        let software = selected.installed.clone();
        let login = InstallationLogin::prepare(selected, operations).map_err(refused)?;
        Ok((login, software))
    }

    /// Only a private authenticated presenter may receive this native start.
    /// Its URL/state/attempt handle must never enter a model transcript or log.
    /// Retain the attempt handle in the host; only the authorization URL goes
    /// to browser navigation.
    /// The exact target and operations come from the selected application owner;
    /// model-provided callback fields cannot select a different target.
    pub async fn start_account(
        &mut self,
        job_id: &str,
        target: ConnectorOAuthTarget,
        operations: BTreeMap<String, McpConnectorOperation>,
        redirect_uri: String,
    ) -> Result<ConnectorLoginStart, ConnectError> {
        let (login, _) = self.selected_login(job_id, operations)?;
        let started = self.control.start(target, redirect_uri, login).await;
        if let Err(primary) = self.approvals.check_session(&self.session) {
            return match self.control.retire() {
                Ok(()) => Err(primary),
                Err(code) => Err(ConnectError::Cleanup {
                    primary: Box::new(primary),
                    cleanup: Box::new(refused(code)),
                }),
            };
        }
        started.map_err(refused)?;
        self.control
            .started()
            .cloned()
            .ok_or_else(|| refused("The native login attempt is unavailable"))
    }

    /// Drive the existing native completion under the application's retained
    /// lifecycle. The caller must join this entered future during shutdown;
    /// dropping it is not cancellation. The existing Control cancels and joins
    /// the same native completion when either supplied stop signal wins.
    pub async fn complete_account(
        &mut self,
        state: String,
        code: String,
        host_attempt_handle: Option<String>,
        retired: &mut tokio::sync::watch::Receiver<bool>,
        cancel: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<ProtectedAccountCompletion, ConnectError> {
        if let Err(primary) = self.approvals.check_session(&self.session) {
            return match self.control.retire() {
                Ok(()) => Err(primary),
                Err(code) => Err(ConnectError::Cleanup {
                    primary: Box::new(primary),
                    cleanup: Box::new(refused(code)),
                }),
            };
        }
        if *retired.borrow() || retired.has_changed().is_err() {
            self.control.retire().map_err(refused)?;
            return Err(refused("The installed account ceremony owner has retired"));
        }
        let (reply, cancellation, cleanup_unconfirmed) = self
            .control
            .complete(state, code, host_attempt_handle, retired, cancel)
            .await;
        let result = match reply {
            Ok(Reply::Completed(completed)) => Ok(completed.native.clone()),
            Err(code) => Err(code),
            Ok(_) => Err("connector_unavailable"),
        };
        Ok(ProtectedAccountCompletion {
            result,
            cancellation,
            cleanup_unconfirmed,
        })
    }

    /// Cancel only the retained active native attempt. A completed account is
    /// not logged out or erased by this operation.
    pub fn cancel_account(&mut self) -> Result<(), ConnectError> {
        self.control.cancel().map(|_| ()).map_err(refused)
    }

    /// Review either the just-authenticated account or an existing authorized
    /// account. Both paths use a fresh native status read and the same checked
    /// software owner. This performs no grant, persistence or attachment effect.
    pub async fn review_account<T: Serialize>(
        &self,
        job_id: &str,
        target: ConnectorOAuthTarget,
        operations: BTreeMap<String, McpConnectorOperation>,
        proposal: InstalledAccessProposal<T>,
    ) -> Result<InstalledAccountReview<T>, ConnectError> {
        self.review_account_with_task(job_id, target, operations, proposal, None, None, None, None, None).await
    }

    pub(super) async fn review_account_with_task<T: Serialize>(
        &self, job_id: &str, target: ConnectorOAuthTarget,
        operations: BTreeMap<String, McpConnectorOperation>,
        proposal: InstalledAccessProposal<T>,
        task: Option<toolkit_mobkit::private_originals::InstalledServiceTask>,
        current_request: Option<&account_request::CurrentRequest>,
        task_expires_in_ms: Option<u64>,
        task_tool_policy: Option<toolkit_mobkit::private_originals::ServiceToolPolicySelection>,
        cancellation: Option<account_request::ReviewCancellation>,
    ) -> Result<InstalledAccountReview<T>, ConnectError> {
        self.review_account_with_task_refreshed(job_id, target, operations, proposal,
            task, current_request, task_expires_in_ms, task_tool_policy, None, None, cancellation).await
    }

    // The Console supplies its same-login refresh here as well as at its
    // transport boundary. This check does not mint a native principal or
    // replace the retained request, account, grant or source owners.
    async fn review_account_with_task_refreshed<T: Serialize>(
        &self, job_id: &str, target: ConnectorOAuthTarget,
        operations: BTreeMap<String, McpConnectorOperation>,
        proposal: InstalledAccessProposal<T>,
        task: Option<toolkit_mobkit::private_originals::InstalledServiceTask>,
        current_request: Option<&account_request::CurrentRequest>,
        task_expires_in_ms: Option<u64>,
        task_tool_policy: Option<toolkit_mobkit::private_originals::ServiceToolPolicySelection>,
        refresh: Option<&account_request::console::Refresh<'_>>,
        capture_decision: Option<&account_request::console::CaptureDecision<'_>>,
        cancellation: Option<account_request::ReviewCancellation>,
    ) -> Result<InstalledAccountReview<T>, ConnectError> {
        let operation_ids = operations.keys().map(|name| toolkit_package::Id::new(name.clone()))
            .collect::<Result<std::collections::BTreeSet<_>, _>>()
            .map_err(|error| refused(error.to_string()))?;
        let (login, software) = self.selected_login(job_id, operations)?;
        let login = Arc::new(login);
        login.check_target(&target).map_err(refused)?;
        let observed = self.observe_account(&login, &target).await?;
        let joined = login.join_authorized(&target, &observed).map_err(refused)?;
        let config = reviewed_configuration(&joined).map_err(refused)?;
        let installation = ConnectorMcpInstallation::from_config(&config)
            .ok_or_else(|| refused("The reviewed connector configuration is missing"))?
            .map_err(|_| refused("The reviewed connector configuration is invalid"))?;
        let request = review_request(&software, &target, &observed, &installation, &proposal)?;
        self.approvals.check_session(&self.session)?;
        login.check_current().map_err(refused)?;
        let expected_selection = request
            .request_body
            .as_ref()
            .and_then(|body| body.get("application_selection"))
            .cloned()
            .ok_or_else(|| refused("The exact application review is missing"))?;
        if let Some(refresh) = refresh { refresh().await?; }
        self.approvals.check_session(&self.session)?;
        login.check_current().map_err(refused)?;
        let task = match task {
            Some(task) => Some(task.prepare_review_with_tool_policy(&installation, operation_ids, task_expires_in_ms, request.clone(), task_tool_policy)
                .await.map_err(|_| refused("The waiting task cannot be offered for service approval"))?),
            None if task_tool_policy.is_some() => return Err(refused("A tool policy amendment requires the actual waiting task")),
            None => None,
        };
        let request = task.as_ref().map_or(request, |task| task.request().clone());
        let decision = self.approvals.review_access_cancellable(request, async {
            match &cancellation {
                Some(cancellation) => cancellation.cancelled().await,
                None => std::future::pending().await,
            }
        }, |id| {
            if let Some(cancellation) = &cancellation { cancellation.approval_created(id); }
        }).await?;
        if let Some(cancellation) = &cancellation { cancellation.check()?; }
        // The transport publishes its exact accepted answer before waking the
        // retained prompt. Capture it once here, before any further await.
        let decision_current = capture_decision.map(|capture| capture()).transpose()?;
        check_selection(&proposal.selection, &expected_selection)?;
        self.approvals.check_session(&self.session)?;
        login.check_current().map_err(refused)?;
        if decision.status == ApprovalStatus::Denied {
            return Ok(InstalledAccountReview::Denied(decision));
        }
        if decision.status != ApprovalStatus::Approved {
            return Err(refused("The native access review has no matching decision"));
        }
        let current = self.observe_account(&login, &target).await?;
        if authorized_account(&target, &current).map_err(refused)?
            != authorized_account(&target, &observed).map_err(refused)?
            || reviewed_configuration(&login.join_authorized(&target, &current).map_err(refused)?)
                .map_err(refused)?
                != config
        {
            return Err(refused("The reviewed account or installation changed"));
        }
        self.approvals.check_session(&self.session)?;
        login.check_current().map_err(refused)?;
        check_selection(&proposal.selection, &expected_selection)?;
        // Refresh the actual borrowed native call after all human and account
        // waits, before any new parent/grant/source effect. The terminal owner
        // retires approvals on EOF and joins this entire future.
        if let Some(current) = current_request {
            if !current.check().await { return Err(refused("The account request is no longer current")); }
        }
        if let Some(refresh) = refresh { refresh().await?; }
        self.approvals.check_session(&self.session)?;
        login.check_current().map_err(refused)?;
        if let Some(cancellation) = &cancellation { cancellation.check()?; }
        let fresh_instruction_retained = match task {
            Some(task) => task.approve(decision.clone(), account_publication_current(
                login.clone(), self.approvals.clone(), self.session.clone(),
                cancellation.clone(), decision_current,
            )).await.map_err(|_| refused("The approved instruction could not be retained"))?,
            None => false,
        };
        self.approvals.check_session(&self.session)?;
        login.check_current().map_err(refused)?;
        if let Some(cancellation) = &cancellation { cancellation.check()?; }
        Ok(InstalledAccountReview::Approved(ReviewedInstalledAccount {
            selection: proposal.selection,
            decision,
            target,
            account: current,
            installation,
            software,
            fresh_instruction_retained,
        }))
    }

    async fn observe_account(
        &self,
        login: &InstallationLogin,
        target: &ConnectorOAuthTarget,
    ) -> Result<ConnectorAuthStatus, ConnectError> {
        let observed = self.auth.facade.connector_status(&target.slot).await;
        // Recheck private review eligibility for success and error alike.
        // This does not stand in for native attachment or operation authority.
        self.approvals.check_session(&self.session)?;
        login.check_current().map_err(refused)?;
        observed.map_err(|e| ConnectError::provider_setup("Observe selected connector account", e))
    }

    /// Stop new review/checked work before native helper shutdown, then cancel
    /// the sole unfinished account attempt. Entered completion is joined by its
    /// existing caller; this never aborts a task or rewrites a native decision.
    pub fn retire(&mut self) -> Result<(), ConnectError> {
        self.approvals.retire();
        self.checked.retire();
        self.control.retire().map_err(refused)
    }
}

impl Drop for InstalledServiceReview {
    fn drop(&mut self) {
        // The owner must still join entered futures. This synchronous fallback
        // only retires these same owners and attempts native cancellation.
        let _ = self.retire();
    }
}

// Retain the same software owner through the task's deferred publication checks.
fn account_publication_current(
    login: Arc<InstallationLogin>,
    approvals: Arc<LocalApprovals>,
    session: SessionId,
    cancellation: Option<account_request::ReviewCancellation>,
    decision_current: Option<account_request::console::DecisionCheck>,
) -> impl Fn() -> bool + Send + Sync + 'static {
    move || approvals.check_session(&session).is_ok()
        && login.check_current().is_ok()
        && cancellation.as_ref().is_none_or(|cancellation| cancellation.check().is_ok())
        && decision_current.as_ref().is_none_or(|current| current())
}

#[cfg(test)]
pub(crate) fn account_publication_current_for_test(
    login: Arc<InstallationLogin>, approvals: Arc<LocalApprovals>, session: SessionId,
    decision_current: Option<account_request::console::DecisionCheck>,
) -> impl Fn() -> bool + Send + Sync + 'static {
    account_publication_current(login, approvals, session, None, decision_current)
}

fn check_selection<T: Serialize>(selection: &T, expected: &Value) -> Result<(), ConnectError> {
    if serde_json::to_value(selection).ok().as_ref() != Some(expected) {
        return Err(refused("The application's reviewed selection changed"));
    }
    Ok(())
}

fn review_request<T: Serialize>(
    software: &InstalledSelection,
    target: &ConnectorOAuthTarget,
    observed: &ConnectorAuthStatus,
    installation: &ConnectorMcpInstallation,
    proposal: &InstalledAccessProposal<T>,
) -> Result<ApprovalRequest, ConnectError> {
    let body = account_review_body(software, target, observed, installation, &proposal.selection)?;
    account_review_request(software, proposal, body)
}

// One checked software/account projection serves either retained authority.
// This body proposes no approval or grant and contains no credential value.
fn account_review_body<T: Serialize>(
    software: &InstalledSelection,
    target: &ConnectorOAuthTarget,
    observed: &ConnectorAuthStatus,
    installation: &ConnectorMcpInstallation,
    selection: &T,
) -> Result<Value, ConnectError> {
    let account = authorized_account(target, observed).map_err(refused)?;
    let selection = serde_json::to_value(selection)
        .map_err(|_| refused("The application access selection cannot be presented"))?;
    let body: Value = json!({"phase":"installed_account_access",
        "realm":software.realm.realm.as_str(), "realm_state_root":software.realm.state_root,
        "session":software.session,
        "software":{"job_id":software.job_id, "validation_id":software.validation_id,
            "receipt_sha256":software.receipt_sha256, "configuration_sha256":software.configuration_sha256,
            "document_sha256":software.mcp_document_sha256,
            "mcp_path":software.mcp_path, "receipt_path":software.receipt_path,
            "family":software.family, "profile":software.profile, "server_name":software.server_name,
            "recovery_request_sha256":software.recovery_request_sha256},
        "account":{"slot":target.slot, "issuer":account.issuer,
            "strategy":account.strategy_id, "subject":account.subject},
        "connector":{"client":target.client, "resource":target.resource,
            "requested_scopes":target.scopes, "client_auth_profile":target.client_auth_profile},
        "installation":installation.config(), "application_selection":selection,
        "notice":"This decision records the exact review. The installed application's native owners must still authorize access, persist or attach the server, and observe readiness."});
    Ok(body)
}

fn account_review_request<T: Serialize>(software: &InstalledSelection,
    proposal: &InstalledAccessProposal<T>, body: Value) -> Result<ApprovalRequest, ConnectError> {
    Ok(ApprovalRequest {
        requester: proposal.requester.clone(),
        owner: ApprovalOwnerRef::Session {
            session_id: software.session.clone(),
        },
        resource: ApprovalResourceRef {
            kind: ApprovalResourceKind::Runtime,
            id: ApprovalResourceId::new("selected-installed-connector"),
        },
        proposed_action: ApprovalProposedAction {
            kind: ApprovalActionKind::Other,
            summary: proposal.summary.clone(),
            body: Some(body.clone()),
        },
        risk: ApprovalRisk::Medium,
        request_body: Some(body),
        allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny]
            .into_iter()
            .collect(),
        expires_at: Some(proposal.decision_expires_at.into()),
        metadata: Default::default(),
        request_provenance: None,
    })
}

fn refused(message: impl Into<String>) -> ConnectError {
    ConnectError::InvalidInput(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_auth_core::connector_oauth::AccountSelection;
    use meerkat_core::{
        McpServerConfig,
        connection::{CredentialAccountId, CredentialAccountRef, RealmId},
        mcp_config::McpConnectorBinding,
    };
    use std::{path::PathBuf, time::Duration};

    // These typed fixtures exercise review composition only. They are not
    // provider observations, authenticated installation receipts or readiness.
    fn fixture() -> (
        InstalledSelection,
        ConnectorOAuthTarget,
        ConnectorAuthStatus,
        ConnectorMcpInstallation,
        InstalledAccessProposal<Value>,
    ) {
        let realm = RealmLocator {
            state_root: PathBuf::from("/fixture/state"),
            realm: RealmId::parse("work").unwrap(),
        };
        let session = SessionId::new();
        let software = InstalledSelection {
            realm: realm.clone(),
            session,
            job_id: "job-one".into(),
            validation_id: "validation-one".into(),
            family: "services".into(),
            profile: "mail".into(),
            mcp_path: PathBuf::from("/fixture/mcp.json"),
            receipt_path: PathBuf::from("/fixture/receipt.json"),
            receipt_sha256: Some("1".repeat(64)),
            configuration_sha256: Some("2".repeat(64)),
            server_name: "selected-mail".into(),
            mcp_document_sha256: "3".repeat(64),
            recovery_request_sha256: None,
        };
        let target = ConnectorOAuthTarget {
            slot: CredentialAccountRef {
                realm: realm.realm,
                account: CredentialAccountId::parse("mail").unwrap(),
            },
            issuer: "https://issuer.invalid".into(),
            client: "client".into(),
            resource: "https://api.invalid".into(),
            scopes: ["read".into()].into(),
            strategy_id: "userinfo".into(),
            account: AccountSelection::Known("account-one".into()),
            client_auth_profile: Some("private-host-profile".into()),
        };
        let account = ConnectorAuthStatus {
            slot: target.slot.clone(),
            phase: meerkat::ConnectorAuthPhase::Authorized,
            verified_account: Some(meerkat::ConnectorVerifiedAccount {
                issuer: target.issuer.clone(),
                strategy_id: target.strategy_id.clone(),
                subject: "account-one".into(),
            }),
            scopes: vec!["read".into()],
            scope_evidence: None,
            expires_at: None,
            has_refresh_token: false,
        };
        let mut config = McpServerConfig::stdio(
            "selected-mail",
            "/fixture/python",
            vec!["serve-native".into()],
            Default::default(),
        );
        config.connector = Some(McpConnectorBinding {
            realm_id: "work".into(),
            slot_id: "mail".into(),
            issuer: target.issuer.clone(),
            client: target.client.clone(),
            resource: target.resource.clone(),
            strategy_id: target.strategy_id.clone(),
            account: "account-one".into(),
            operations: [(
                "messages.list".into(),
                McpConnectorOperation {
                    required_scopes: ["read".into()].into(),
                    destination_origin: "https://api.invalid".into(),
                    read_only: true,
                },
            )]
            .into(),
        });
        let installation = ConnectorMcpInstallation::from_config(&config)
            .unwrap()
            .unwrap();
        let config = reviewed_configuration(&installation).unwrap();
        let installation = ConnectorMcpInstallation::from_config(&config)
            .unwrap()
            .unwrap();
        let proposal = InstalledAccessProposal {
            requester: ApprovalPrincipalId::new("fixture-application").unwrap(),
            summary: "Review selected mail access".into(),
            selection: json!({"original":"retained-original-one", "destination":"member-one",
                "operation_basis":"actual-selection-one", "expires_in_ms":60000}),
            decision_expires_at: SystemTime::now() + Duration::from_secs(60),
        };
        (software, target, account, installation, proposal)
    }

    #[test]
    fn private_review_owner_requires_exact_live_bound_session() {
        let directory = tempfile::tempdir().unwrap();
        let (approvals, _presenter) = LocalApprovals::create(directory.path()).unwrap();
        let session = SessionId::new();
        assert!(approvals.check_session(&session).is_err());
        approvals.bind_session(&session).unwrap();
        assert!(approvals.check_session(&session).is_ok());
        assert!(approvals.check_session(&SessionId::new()).is_err());
        approvals.retire();
        assert!(approvals.check_session(&session).is_err());
    }

    #[test]
    fn private_review_owner_refuses_missing_session_with_expired_presenter() {
        let directory = tempfile::tempdir().unwrap();
        let (approvals, _presenter) = LocalApprovals::create_browser(
            directory.path(),
            ApprovalPrincipalId::new("fixture-expired").unwrap(),
            SystemTime::now() - Duration::from_secs(1),
        )
        .unwrap();
        assert!(approvals.check_session(&SessionId::new()).is_err());
    }

    #[test]
    fn review_binds_exact_software_native_account_operations_and_application_selection() {
        let (software, target, account, installation, proposal) = fixture();
        let request =
            review_request(&software, &target, &account, &installation, &proposal).unwrap();
        let body = request.request_body.as_ref().unwrap();
        assert_eq!(request.proposed_action.body.as_ref(), Some(body));
        assert_eq!(&account_review_body(&software, &target, &account, &installation,
            &proposal.selection).unwrap(), body);
        assert_eq!(body["software"]["receipt_sha256"], "1".repeat(64));
        assert_eq!(body["account"]["subject"], "account-one");
        assert_eq!(
            body["installation"],
            serde_json::to_value(installation.config()).unwrap()
        );
        assert_eq!(body["application_selection"], proposal.selection);
        assert_eq!(body["realm_state_root"], "/fixture/state");
        assert_eq!(
            request.owner,
            ApprovalOwnerRef::Session {
                session_id: software.session
            }
        );
        assert_eq!(
            request.expires_at.map(SystemTime::from),
            Some(proposal.decision_expires_at)
        );
    }

    #[test]
    fn application_account_body_preserves_checked_software_and_native_account() {
        use crate::resources::installation_readback::checked_application_body_for_test as read;
        let (mut software, target, account, installation, proposal) = fixture();
        software.job_id = "1".repeat(32);
        software.validation_id = "2".repeat(32);
        software.recovery_request_sha256 = Some("4".repeat(64));
        let body = account_review_body(&software, &target, &account, &installation,
            &proposal.selection).unwrap();
        let human = review_request(&software, &target, &account, &installation, &proposal).unwrap();
        assert_eq!(human.request_body.as_ref(), Some(&body));
        assert_eq!(read(&body, installation.config(), &software.realm).unwrap(), *installation.config());
        for field in ["receipt_sha256", "document_sha256", "recovery_request_sha256"] {
            let mut missing = body.clone();
            missing["software"].as_object_mut().unwrap().remove(field);
            assert!(read(&missing, installation.config(), &software.realm).is_err());
        }
        let mut changed = body.clone();
        changed["installation"]["connector"]["account"] = json!("another-account");
        assert!(read(&changed, installation.config(), &software.realm).is_err());
        let mut unauthorized = account;
        unauthorized.phase = meerkat::ConnectorAuthPhase::AuthorizationRequired;
        assert!(account_review_body(&software, &target, &unauthorized, &installation,
            &proposal.selection).is_err());
    }

    #[test]
    fn review_refuses_unverified_or_changed_native_account() {
        let (software, target, mut account, installation, proposal) = fixture();
        account.verified_account.as_mut().unwrap().subject = "other-account".into();
        assert!(review_request(&software, &target, &account, &installation, &proposal).is_err());
        account.verified_account = None;
        assert!(review_request(&software, &target, &account, &installation, &proposal).is_err());
    }

    #[test]
    fn changed_application_projection_cannot_match_returned_review() {
        let expected = json!({"destination":"member-one", "expires_in_ms":60000});
        assert!(check_selection(&expected, &expected).is_ok());
        assert!(
            check_selection(
                &json!({"destination":"member-two", "expires_in_ms":60000}),
                &expected
            )
            .is_err()
        );
        assert!(
            check_selection(
                &json!({"destination":"member-one", "expires_in_ms":null}),
                &expected
            )
            .is_err()
        );
    }

    #[tokio::test]
    async fn existing_native_presenter_preserves_the_whole_exact_account_decision() {
        for answer in [ApprovalDecision::Approve, ApprovalDecision::Deny] {
            let (software, target, account, installation, proposal) = fixture();
            let request =
                review_request(&software, &target, &account, &installation, &proposal).unwrap();
            let directory = tempfile::tempdir().unwrap();
            let (approvals, mut presenter) = LocalApprovals::create(directory.path()).unwrap();
            approvals.bind_session(&software.session).unwrap();
            let expected = request.request_body.clone();
            let (record, ()) = tokio::join!(approvals.review_access(request), async {
                let mut prompt = presenter.next().await.unwrap();
                assert_eq!(prompt.record().request_body, expected);
                prompt.decide(answer).unwrap();
            });
            let record = record.unwrap();
            assert_eq!(record.request_body, expected);
            assert_eq!(
                record.status,
                match answer {
                    ApprovalDecision::Approve => ApprovalStatus::Approved,
                    ApprovalDecision::Deny => ApprovalStatus::Denied,
                }
            );
            assert!(!approvals.has_uncertain_dispatch());
        }
    }
}
