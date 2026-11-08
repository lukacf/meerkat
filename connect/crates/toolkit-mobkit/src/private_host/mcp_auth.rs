//! Hosted callback custody on the existing private connection. Core owns OAuth
//! and credentials. This module retains only live ceremonies and their joins.

use super::ConsoleLoginHandle;
use meerkat::{
    HostAuthService, McpOAuthLoginDisposition, McpOAuthLoginStart, McpOAuthRegisteredCallback,
    McpRegisteredRedirectBase, McpServerIdentity,
};
use meerkat_mobkit::{MemberMcpCustody, rpc_gateway::GatewayFactoryConstruction};
use meerkat_toolkit::host_lifetime::control::HostLifetimeError;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore, mpsc, watch},
    task::JoinHandle,
};

/// A requested selection, never a configured identity. The host resolver must
/// obtain the identity from the actual runtime's trusted effective config.
pub type HostedMcpTargetSelection = meerkat::contracts::WireMcpAuthTarget;

/// Native capabilities supplied by the same factory/runtime, not wire data.
pub struct ResolvedHostedMcpTarget {
    pub service: HostAuthService,
    pub target: McpServerIdentity,
}

/// Composition boundary for the host's canonical target resolver. This is not
/// a second config loader; missing native resolution must remain unavailable.
pub type HostedMcpTargetResolver = Arc<
    dyn Fn(
            HostedMcpTargetSelection,
        ) -> futures::future::BoxFuture<
            'static,
            Result<ResolvedHostedMcpTarget, HostLifetimeError>,
        > + Send
        + Sync,
>;

#[derive(Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum HostedMcpCommand {
    Describe {
        target: HostedMcpTargetSelection,
    },
    Start {
        target: HostedMcpTargetSelection,
    },
    Complete {
        ceremony_id: u64,
        redirect_uri: String,
        state: String,
        code: String,
        iss: Option<String>,
    },
    Cancel {
        ceremony_id: u64,
    },
    Release {
        ceremony_id: u64,
    },
    Status {
        target: HostedMcpTargetSelection,
    },
}

#[derive(Clone)]
enum Outcome {
    Completed(Value),
    Cancelled(bool),
    NativeFailure {
        error: meerkat::contracts::WireAuthErrorData,
        // Only an actual successful native cancel, never inferred from an
        // error reason/detail, certifies this attempt cannot publish later.
        cancelled: bool,
    },
}

impl Outcome {
    fn cancelled(&self) -> bool {
        matches!(
            self,
            Self::Cancelled(true)
                | Self::NativeFailure {
                    cancelled: true,
                    ..
                }
        )
    }

    fn settled(&self) -> bool {
        matches!(self, Self::Completed(_)) || self.cancelled()
    }
}

// Core owns the reason/detail mapping. Do not serialize Display/source errors,
// classify settlement, or infer retry permission from these diagnostics.
fn native_failure(error: &meerkat::HostAuthError) -> meerkat::contracts::WireAuthErrorData {
    meerkat::contracts::WireAuthErrorData {
        reason: error.reason(),
        detail: error.detail(),
    }
}

fn failure_reply(
    selection: &HostedMcpTargetSelection,
    error: meerkat::contracts::WireAuthErrorData,
) -> Value {
    json!({"mcp": selection, "auth_error": error})
}

struct Callback {
    redirect_uri: String,
    state: String,
    code: String,
    iss: Option<String>,
}

struct Ceremony {
    _slot: OwnedSemaphorePermit,
    login: ConsoleLoginHandle,
    selection: HostedMcpTargetSelection,
    start: Arc<McpOAuthLoginStart>,
    callbacks: mpsc::Sender<Callback>,
    stop: watch::Sender<bool>,
    outcome: watch::Receiver<Option<Outcome>>,
    task: tokio::sync::Mutex<Option<JoinHandle<()>>>,
    join_failed: AtomicBool,
}

struct FactoryBinding {
    resolver: HostedMcpTargetResolver,
    members: Option<Arc<MemberMcpCustody>>,
}

/// One private connection's bounded live callback custody. The resolver is
/// installed once by trusted composition; no command can install or replace it.
#[derive(Clone)]
pub struct HostedMcpAuthEndpoint {
    binding: Arc<OnceLock<FactoryBinding>>,
    ceremonies: Arc<Mutex<BTreeMap<u64, Arc<Ceremony>>>>,
    unconfirmed: Arc<AtomicBool>,
    slots: Arc<Semaphore>,
}

impl Default for HostedMcpAuthEndpoint {
    fn default() -> Self {
        Self {
            binding: Arc::new(OnceLock::new()),
            ceremonies: Arc::new(Mutex::new(BTreeMap::new())),
            unconfirmed: Arc::new(AtomicBool::new(false)),
            slots: Arc::new(Semaphore::new(256)),
        }
    }
}

fn unavailable() -> HostLifetimeError {
    HostLifetimeError::Unavailable
}
fn current(login: &ConsoleLoginHandle) -> Result<(), HostLifetimeError> {
    login.check().map_err(|_| HostLifetimeError::Denied)
}
fn valid_text(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

impl HostedMcpAuthEndpoint {
    pub fn bind(&self, resolver: HostedMcpTargetResolver) -> Result<(), HostLifetimeError> {
        self.bind_parts(resolver, None)
    }

    /// Bind the real gateway factory's account service, configured target
    /// resolver and member custody together. `redirect_base` has already passed
    /// Core's registered HTTPS-base validation. No request can replace these
    /// handles or provide a server definition, credential store or callback base.
    pub fn bind_factory(
        &self,
        construction: &GatewayFactoryConstruction<'_>,
        redirect_base: McpRegisteredRedirectBase,
    ) -> Result<(), HostLifetimeError> {
        let service = construction
            .host_auth_service()
            .map_err(|_| unavailable())?
            .with_mcp_registered_redirect_base(redirect_base);
        let installation = construction
            .realm_mcp_installation()
            .map_err(|_| unavailable())?;
        let resolver: HostedMcpTargetResolver = Arc::new(move |selection| {
            let service = service.clone();
            let installation = installation.clone();
            Box::pin(async move {
                let target = installation
                    .resolve_mcp_auth_target(&selection)
                    .await
                    .map_err(|_| unavailable())?;
                Ok(ResolvedHostedMcpTarget { service, target })
            })
        });
        self.bind_parts(resolver, Some(construction.member_mcp_custody()))
    }

    fn bind_parts(
        &self,
        resolver: HostedMcpTargetResolver,
        members: Option<Arc<MemberMcpCustody>>,
    ) -> Result<(), HostLifetimeError> {
        self.binding
            .set(FactoryBinding { resolver, members })
            .map_err(|_| HostLifetimeError::WrongOwner)
    }

    async fn resolve(
        &self,
        selection: &HostedMcpTargetSelection,
    ) -> Result<ResolvedHostedMcpTarget, HostLifetimeError> {
        if !valid_text(&selection.server_name, 1024)
            || !valid_text(&selection.server_url, 8192)
            || selection
                .oauth_account
                .as_ref()
                .is_some_and(|account| !valid_text(account, 4096))
            || selection.oauth_account.is_some() == selection.oauth_account_selection.is_some()
        {
            return Err(HostLifetimeError::InvalidSelection);
        }
        let binding = self.binding.get().ok_or_else(unavailable)?;
        let resolved = (binding.resolver)(selection.clone()).await?;
        if meerkat::mcp_auth_target_to_wire(&resolved.target) != *selection {
            return Err(HostLifetimeError::WrongOwner);
        }
        Ok(resolved)
    }

    pub(super) async fn execute(
        &self,
        id: u64,
        login: ConsoleLoginHandle,
        command: HostedMcpCommand,
    ) -> Result<Value, HostLifetimeError> {
        current(&login)?;
        match command {
            HostedMcpCommand::Describe { target } => {
                let resolved = self.resolve(&target).await?;
                current(&login)?;
                let redirect = resolved
                    .service
                    .mcp_registered_redirect_uri(&resolved.target)
                    .await;
                current(&login)?;
                let redirect_uri = match redirect {
                    Ok(uri) => uri.ok_or_else(unavailable)?,
                    Err(error) => return Ok(failure_reply(&target, native_failure(&error))),
                };
                Ok(json!({"mcp": target, "redirect_uri": redirect_uri}))
            }
            HostedMcpCommand::Start { target } => self.start(id, login, target).await,
            HostedMcpCommand::Status { target } => {
                let resolved = self.resolve(&target).await?;
                current(&login)?;
                let status = resolved.service.mcp_status(&resolved.target).await;
                current(&login)?;
                match status {
                    Ok(status) => serde_json::to_value(status.to_wire()).map_err(|_| unavailable()),
                    Err(error) => Ok(failure_reply(&target, native_failure(&error))),
                }
            }
            HostedMcpCommand::Complete {
                ceremony_id,
                redirect_uri,
                state,
                code,
                iss,
            } => {
                let ceremony = self.owned(ceremony_id, &login)?;
                // Refuse substitutions before handing anything to the native
                // completion, so malformed callbacks cannot retire its owner.
                if redirect_uri != ceremony.start.redirect_uri
                    || state != ceremony.start.state
                    || !valid_text(&code, 65536)
                    || iss.as_ref().is_some_and(|iss| !valid_text(iss, 8192))
                {
                    return Err(HostLifetimeError::InvalidSelection);
                }
                ceremony
                    .callbacks
                    .try_send(Callback {
                        redirect_uri,
                        state,
                        code,
                        iss,
                    })
                    .map_err(|_| HostLifetimeError::NotSettled)?;
                // This is a credential observation, not member readiness. The same
                // ceremony task retains the later best-effort member reconnect.
                match ceremony.observed().await? {
                    Outcome::Completed(value) => {
                        current(&login)?;
                        Ok(value)
                    }
                    Outcome::NativeFailure { error, .. } => {
                        // Retirement still fences the observer. This diagnostic
                        // alone makes no claim about cleanup or retry permission.
                        current(&login).map_err(|_| HostLifetimeError::NotSettled)?;
                        Ok(failure_reply(&ceremony.selection, error))
                    }
                    _ => Err(HostLifetimeError::NotSettled),
                }
            }
            HostedMcpCommand::Cancel { ceremony_id } => {
                let ceremony = self.owned(ceremony_id, &login)?;
                ceremony.stop.send_replace(true);
                let outcome = ceremony.joined().await?;
                Ok(json!({"mcp": ceremony.selection,
                    "cancelled": outcome.cancelled(),
                    "completed": matches!(outcome, Outcome::Completed(_))}))
            }
            HostedMcpCommand::Release { ceremony_id } => {
                let ceremony = self.owned(ceremony_id, &login)?;
                if ceremony.outcome.borrow().is_none() {
                    return Err(HostLifetimeError::NotSettled);
                }
                // Explicit host disposal after its final observation. In-flight
                // completion/cancel observers retain their Arc and actual join.
                ceremony.joined().await?;
                self.ceremonies
                    .lock()
                    .map_err(|_| unavailable())?
                    .remove(&ceremony_id);
                Ok(json!({"released": true}))
            }
        }
    }

    fn owned(
        &self,
        id: u64,
        login: &ConsoleLoginHandle,
    ) -> Result<Arc<Ceremony>, HostLifetimeError> {
        let ceremony = self
            .ceremonies
            .lock()
            .map_err(|_| unavailable())?
            .get(&id)
            .cloned()
            .ok_or(HostLifetimeError::WrongOwner)?;
        if !ceremony.login.same_owner(login) {
            return Err(HostLifetimeError::WrongOwner);
        }
        Ok(ceremony)
    }

    async fn start(
        &self,
        id: u64,
        login: ConsoleLoginHandle,
        selection: HostedMcpTargetSelection,
    ) -> Result<Value, HostLifetimeError> {
        self.reap_closed().await?;
        let slot = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| unavailable())?;
        let resolved = self.resolve(&selection).await?;
        current(&login)?;
        // The reservation is lifetime accounting, not a native commit lock.
        let reservation = login.reserve().map_err(|_| HostLifetimeError::Denied)?;
        let start = match resolved
            .service
            .mcp_login_start_registered(&resolved.target, None)
            .await
        {
            Ok(start) => start,
            Err(error) => {
                self.unconfirmed.store(true, Ordering::Release);
                // A returned native error is observed, but does not certify
                // that its credential effects settled. Preserve uncertainty.
                current(&login)?;
                return Ok(failure_reply(&selection, native_failure(&error)));
            }
        };
        let started = start.disposition == McpOAuthLoginDisposition::Started;
        if current(&login).is_err() {
            if !started
                || resolved
                    .service
                    .mcp_login_cancel(&resolved.target, &start)
                    .is_ok()
            {
                reservation.settled();
            } else {
                self.unconfirmed.store(true, Ordering::Release);
                reservation.uncertain();
            }
            return Err(HostLifetimeError::Denied);
        }
        let reply = json!({"mcp": selection, "ceremony_id": if started { Some(id) } else { None },
            "state": start.state, "authorize_url": start.authorize_url, "redirect_uri": start.redirect_uri,
            "disposition": if started { "started" } else { "joined" }});
        if !started {
            // A joiner never gets host_attempt_handle or cancel custody.
            reservation.settled();
            return Ok(reply);
        }
        if start.host_attempt_handle.is_none() {
            let _ = resolved.service.mcp_login_cancel(&resolved.target, &start);
            self.unconfirmed.store(true, Ordering::Release);
            reservation.uncertain();
            return Err(unavailable());
        }
        // Hold the transport slot before spawning: a poisoned slot must not
        // leave an unretained native task behind.
        let mut ceremonies = match self.ceremonies.lock() {
            Ok(ceremonies) => ceremonies,
            Err(_) => {
                let _ = resolved.service.mcp_login_cancel(&resolved.target, &start);
                self.unconfirmed.store(true, Ordering::Release);
                reservation.uncertain();
                return Err(HostLifetimeError::NotSettled);
            }
        };
        let start = Arc::new(start);
        let (callbacks, receiver) = mpsc::channel(1);
        let (stop, stopped) = watch::channel(false);
        let (send, outcome) = watch::channel(None);
        let task = tokio::spawn(run(
            resolved,
            selection.clone(),
            login.clone(),
            start.clone(),
            receiver,
            stopped,
            send,
            reservation,
            self.unconfirmed.clone(),
            self.binding
                .get()
                .and_then(|binding| binding.members.clone()),
        ));
        let ceremony = Arc::new(Ceremony {
            _slot: slot,
            login,
            selection,
            start,
            callbacks,
            stop,
            outcome,
            task: tokio::sync::Mutex::new(Some(task)),
            join_failed: AtomicBool::new(false),
        });
        ceremonies.insert(id, ceremony);
        Ok(reply)
    }

    async fn reap_closed(&self) -> Result<(), HostLifetimeError> {
        // A retired login cannot authorize another callback/result read. Join
        // its existing native work before releasing only its transport rows.
        let closed: Vec<_> = self
            .ceremonies
            .lock()
            .map_err(|_| unavailable())?
            .iter()
            .filter(|(_, ceremony)| {
                ceremony.login.check().is_err() && ceremony.outcome.borrow().is_some()
            })
            .map(|(id, ceremony)| (*id, ceremony.clone()))
            .collect();
        for (id, ceremony) in closed {
            ceremony.joined().await?;
            self.ceremonies
                .lock()
                .map_err(|_| unavailable())?
                .remove(&id);
        }
        Ok(())
    }

    pub(super) async fn close(&self) -> Result<(), HostLifetimeError> {
        let ceremonies: Vec<_> = self
            .ceremonies
            .lock()
            .map_err(|_| unavailable())?
            .values()
            .cloned()
            .collect();
        for ceremony in &ceremonies {
            ceremony.stop.send_replace(true);
        }
        let mut settled = true;
        for ceremony in ceremonies {
            settled &= ceremony
                .joined()
                .await
                .is_ok_and(|outcome| outcome.settled());
        }
        if settled && !self.unconfirmed.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(HostLifetimeError::NotSettled)
        }
    }
}

impl Ceremony {
    async fn observed(&self) -> Result<Outcome, HostLifetimeError> {
        let mut observed = self.outcome.clone();
        let result = loop {
            if let Some(outcome) = observed.borrow().clone() {
                break outcome;
            }
            observed.changed().await.map_err(|_| unavailable())?;
        };
        Ok(result)
    }

    async fn joined(&self) -> Result<Outcome, HostLifetimeError> {
        let result = self.observed().await;
        // Keep the JoinHandle in its existing owner across the await. A cancelled
        // observer must not detach the native completion or member reconnection.
        join_owned_task(&self.task, &self.join_failed).await?;
        result
    }
}

async fn join_owned_task(
    task: &tokio::sync::Mutex<Option<JoinHandle<()>>>,
    failed: &AtomicBool,
) -> Result<(), HostLifetimeError> {
    let mut task = task.lock().await;
    if let Some(join) = task.as_mut() {
        let result = join.await;
        task.take();
        if result.is_err() {
            failed.store(true, Ordering::Release);
        }
    }
    if failed.load(Ordering::Acquire) {
        Err(unavailable())
    } else {
        Ok(())
    }
}

async fn run(
    resolved: ResolvedHostedMcpTarget,
    selection: HostedMcpTargetSelection,
    login: ConsoleLoginHandle,
    start: Arc<McpOAuthLoginStart>,
    mut callbacks: mpsc::Receiver<Callback>,
    mut stop: watch::Receiver<bool>,
    send: watch::Sender<Option<Outcome>>,
    reservation: super::ConsoleLoginReservation,
    unconfirmed: Arc<AtomicBool>,
    members: Option<Arc<MemberMcpCustody>>,
) {
    let mut landed = None;
    let expiry = tokio::time::sleep(start.remaining());
    tokio::pin!(expiry);
    let callback = tokio::select! { biased;
        _ = login.closed() => None,
        _ = stop.changed() => None,
        _ = &mut expiry => None,
        callback = callbacks.recv() => callback,
    };
    let outcome = if let Some(callback) = callback {
        callbacks.close();
        let completion = resolved.service.mcp_login_complete_registered(
            &resolved.target,
            McpOAuthRegisteredCallback {
                redirect_uri: callback.redirect_uri,
                state: callback.state,
                code: callback.code,
                iss: callback.iss,
                host_attempt_handle: start.host_attempt_handle.clone().expect("started custody"),
            },
        );
        tokio::pin!(completion);
        let mut cancellation = None;
        let result = tokio::select! { biased;
            _ = login.closed() => {
                // Native cancellation and commit race in Core. An already
                // entered completion stays joined and may legitimately win.
                cancellation = Some(resolved.service.mcp_login_cancel(&resolved.target, &start));
                completion.await
            },
            _ = stop.changed() => {
                cancellation = Some(resolved.service.mcp_login_cancel(&resolved.target, &start));
                completion.await
            },
            _ = &mut expiry => {
                cancellation = Some(resolved.service.mcp_login_cancel(&resolved.target, &start));
                completion.await
            },
            result = &mut completion => result,
        };
        match result {
            Ok(completed) => {
                // Core's completion identity includes the actual credential
                // realm. The requested selection is not a member retry target.
                landed = Some(completed.target);
                Outcome::Completed(json!({"mcp": selection,
                    "account_id": completed.account_id, "expires_at": completed.expires_at.map(|at| at.to_rfc3339()),
                    "account_verification": meerkat::mcp_account_verification_to_wire(Some(completed.account_verification)),
                    "has_refresh_token": completed.has_refresh_token, "scopes": completed.scopes}))
            }
            Err(error) => Outcome::NativeFailure {
                error: native_failure(&error),
                cancelled: cancellation.is_some_and(|result| result.is_ok()),
            },
        }
    } else {
        Outcome::Cancelled(
            resolved
                .service
                .mcp_login_cancel(&resolved.target, &start)
                .is_ok(),
        )
    };
    let settled = outcome.settled();
    // Publish only the credential outcome. A member may have an MCP call in
    // flight, and its retry must not delay this observation. Release, Cancel and
    // endpoint close still join this exact task, including the member work.
    send.send_replace(Some(outcome));
    if let (Some(members), Some(target)) = (members, landed) {
        // These are best-effort router staging/boundary outcomes, not proof that
        // a member is ready. A refusal never rolls back a successful credential.
        // Ceremony disposal is not logout: never call credential_released here.
        let _ = members.credential_landed(&target).await;
    }
    // The retained login's lifetime also covers the entered member work, even
    // though a credential observer was allowed to return before it.
    if settled {
        reservation.settled();
    } else {
        unconfirmed.store(true, Ordering::Release);
        reservation.uncertain();
    }
}

#[cfg(test)]
mod native_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn dropping_a_join_observer_keeps_owned_work_joinable() {
        let (release, held) = tokio::sync::oneshot::channel();
        let work = tokio::spawn(async { held.await.unwrap() });
        let task = tokio::sync::Mutex::new(Some(work));
        let failed = AtomicBool::new(false);
        // Timeout cancels only this observation. The retained task must remain
        // owned, and a second close must still wait for its actual settlement.
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(20),
                join_owned_task(&task, &failed)
            )
            .await
            .is_err()
        );
        assert!(task.lock().await.is_some());
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(20),
                join_owned_task(&task, &failed)
            )
            .await
            .is_err()
        );
        release.send(()).unwrap();
        join_owned_task(&task, &failed).await.unwrap();
        assert!(task.lock().await.is_none());
        join_owned_task(&task, &failed).await.unwrap();
    }

    #[tokio::test]
    async fn a_failed_owned_task_never_becomes_a_later_successful_join() {
        let task = tokio::sync::Mutex::new(Some(tokio::spawn(async { panic!("fixture failure") })));
        let failed = AtomicBool::new(false);
        for _ in 0..2 {
            assert!(matches!(
                join_owned_task(&task, &failed).await,
                Err(HostLifetimeError::Unavailable)
            ));
        }
        assert!(task.lock().await.is_none());
    }

    #[test]
    fn factory_binding_cannot_be_replaced_by_another_resolver_or_custody() {
        let endpoint = HostedMcpAuthEndpoint::default();
        let members = MemberMcpCustody::new();
        let resolver: HostedMcpTargetResolver =
            Arc::new(|_| Box::pin(async { Err(HostLifetimeError::Unavailable) }));
        endpoint
            .bind_parts(resolver.clone(), Some(members.clone()))
            .unwrap();
        assert!(matches!(
            endpoint.bind_parts(resolver, Some(MemberMcpCustody::new())),
            Err(HostLifetimeError::WrongOwner)
        ));
        assert!(Arc::ptr_eq(
            endpoint.binding.get().unwrap().members.as_ref().unwrap(),
            &members
        ));
    }

    #[test]
    fn native_failure_preserves_only_core_reason_and_detail() {
        let selection: HostedMcpTargetSelection = serde_json::from_value(json!({
            "server_name": "configured", "server_url": "https://mcp.invalid",
            "oauth_account_selection": "discover"
        }))
        .unwrap();
        for (error, expected) in [
            (
                meerkat::McpOAuthError::TokenExchangeFailed {
                    server_name: "private-server".into(),
                    reason: "private-provider-detail".into(),
                },
                json!({"reason": "upstream_failure"}),
            ),
            (
                meerkat::McpOAuthError::RemoteCustodyRequired {
                    server_name: "private-server".into(),
                },
                json!({"reason": "invalid_target", "detail": "remote_custody_required"}),
            ),
            (
                meerkat::McpOAuthError::NetworkTimedOut {
                    server_name: "private-server".into(),
                    stage: meerkat::McpOAuthNetworkStage::ResourceMetadata,
                },
                json!({"reason": "upstream_failure", "detail": "upstream_timeout"}),
            ),
            (
                meerkat::McpOAuthError::NetworkTimedOut {
                    server_name: "private-server".into(),
                    stage: meerkat::McpOAuthNetworkStage::TokenExchange,
                },
                json!({"reason": "upstream_failure", "detail": "upstream_timeout_after_code"}),
            ),
            (
                meerkat::McpOAuthError::CommitUnsettled {
                    server_name: "private-server".into(),
                },
                json!({"reason": "commit_unsettled"}),
            ),
        ] {
            let reply = failure_reply(&selection, native_failure(&error.into()));
            assert_eq!(reply, json!({"mcp": selection, "auth_error": expected}));
            assert!(!reply.to_string().contains("private-"));
            assert!(reply.get("account_id").is_none());
        }
    }

    #[tokio::test]
    async fn canonical_wire_modes_are_explicit_and_do_not_downgrade() {
        let endpoint = HostedMcpAuthEndpoint::default();
        for selection in [
            json!({"oauth_account":"subject"}),
            json!({"oauth_account_selection":"discover"}),
            json!({"oauth_account_selection":"unverified"}),
        ] {
            let mut target = selection;
            target["server_name"] = json!("configured");
            target["server_url"] = json!("https://mcp.invalid");
            let wire: HostedMcpTargetSelection = serde_json::from_value(target.clone()).unwrap();
            assert_eq!(serde_json::to_value(&wire).unwrap(), target);
            assert!(matches!(
                endpoint.resolve(&wire).await,
                Err(HostLifetimeError::Unavailable)
            ));
        }
        for selection in [
            json!({}),
            json!({"oauth_account":"subject", "oauth_account_selection":"discover"}),
        ] {
            let mut target = selection;
            target["server_name"] = json!("configured");
            target["server_url"] = json!("https://mcp.invalid");
            let wire: HostedMcpTargetSelection = serde_json::from_value(target).unwrap();
            assert!(matches!(
                endpoint.resolve(&wire).await,
                Err(HostLifetimeError::InvalidSelection)
            ));
        }
    }
    #[test]
    fn callback_wire_cannot_select_target_handle_or_login() {
        let command = json!({"action":"complete", "ceremony_id":7, "redirect_uri":"https://host.invalid/callback",
            "state":"state", "code":"code", "iss":"https://issuer.invalid/exact%2Fvalue"});
        let parsed: HostedMcpCommand = serde_json::from_value(command.clone()).unwrap();
        assert!(
            matches!(parsed, HostedMcpCommand::Complete { iss: Some(iss), .. }
            if iss == "https://issuer.invalid/exact%2Fvalue")
        );
        for key in [
            "host_attempt_handle",
            "target",
            "enrollment_id",
            "attempt_ref",
            "oauth_account",
        ] {
            let mut substituted = command.clone();
            substituted[key] = json!("substitution");
            assert!(serde_json::from_value::<HostedMcpCommand>(substituted).is_err());
        }
    }
}
