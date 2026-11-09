//! The installed route and native verified human produce one actual source.
//! Forwarded login labels alone never select a requester or a route.

use super::governed_submit::{BindFailure, OriginalBinder, evidence};
use super::*;
use crate::private_host::{ConsoleLoginHandle, ConsoleLoginReservation, ConsoleLogins};
use meerkat_mobkit::{
    access::{ACTION_AGENT_SEND, AccessController},
    console_governed_ingress::{ConsoleGovernedIngress, ConsoleSendContext},
    identity_first::{
        AgentIdentity, BoundGovernedInput, GovernedBindContext, GovernedBindError,
        GovernedEntryFence, GovernedEntryPermit, GovernedEntryRefusal, GovernedInputBinder,
        GovernedSettlement,
    },
    runtime::AuthenticatedConsoleHuman,
};
use meerkat_runtime::{Input, identifiers::IdempotencyKey};
use meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceOrigin;

/// Set only while retaining the actual Console child's raw input. Historical
/// evidence never restores the verified human, login or a live entry permit.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ConsoleChildInputEvidence {
    NativeConsole,
}

pub(super) fn validate_console_child_input(
    record: &SourceRecord,
    bound: &Input,
) -> Result<(), PrivateOriginalError> {
    if record.console_child_input != Some(ConsoleChildInputEvidence::NativeConsole)
        || record.workflow_child.is_none()
        || !matches!(
            record.planning_source_evidence,
            Some(planning_source_evidence::PlanningSourceEvidence::EffectWaiter { .. })
        )
        || record.final_association.is_none()
        || bound.header().authority_association.as_ref() != record.final_association.as_ref()
    {
        return Err(PrivateOriginalError::Conflict);
    }
    let Input::Prompt(mut raw) = bound.clone() else {
        return Err(PrivateOriginalError::Conflict);
    };
    // Native added this exact checked association. Preserve every other raw
    // input field, including id, timestamp, stock key and content.
    raw.header.authority_association = None;
    let raw = Input::Prompt(raw);
    stock_console_event_key(&raw).map_err(|_| PrivateOriginalError::Conflict)?;
    let raw_digest = digest(&canonical_json(&raw).map_err(|_| PrivateOriginalError::Conflict)?);
    if record.event.original.as_str() != raw.id().to_string()
        || record.event.occurrence != record.event.original
        || record.event.root_event.resource_id != record.event.original
        || record.event.root_event.sha256 != raw_digest
        || record.event.content_sha256 != raw_digest
    {
        return Err(PrivateOriginalError::Conflict);
    }
    Ok(())
}

/// Retains the native verifier observation and live access owner for this
/// single input. Setup retains only the same login, never this expiring token.
#[derive(Clone)]
pub(super) struct ConsoleEntry {
    source: Arc<PrivateOriginals>,
    identity: AgentIdentity,
    human: AuthenticatedConsoleHuman,
    access: Option<AccessController>,
    login: ConsoleLoginHandle,
}
impl ConsoleEntry {
    pub(super) fn login(&self) -> &ConsoleLoginHandle {
        &self.login
    }
    pub(super) fn validate_custody_input(
        &self,
        record: &SourceRecord,
        raw: &Input,
        bound: &Input,
    ) -> Result<(), PrivateOriginalError> {
        self.check().map_err(|_| PrivateOriginalError::Denied)?;
        let key = stock_console_event_key(raw).map_err(|_| PrivateOriginalError::Conflict)?;
        let raw_digest = digest(&canonical_json(raw).map_err(|_| PrivateOriginalError::Conflict)?);
        let (Input::Prompt(raw_prompt), Input::Prompt(bound_prompt)) = (raw, bound) else {
            return Err(PrivateOriginalError::Conflict);
        };
        if record.event.original.as_str() != raw.id().to_string()
            || record.event.occurrence != record.event.original
            || record.event.root_event.resource_id != record.event.original
            || record.event.root_event.sha256 != raw_digest
            || record.event.content_sha256 != raw_digest
            || bound.id() != raw.id()
            || bound.header().idempotency_key.as_ref() != Some(&key)
            || canonical_json(&bound_prompt.content).map_err(|_| PrivateOriginalError::Conflict)?
                != canonical_json(&raw_prompt.content)
                    .map_err(|_| PrivateOriginalError::Conflict)?
        {
            return Err(PrivateOriginalError::Conflict);
        }
        Ok(())
    }
    pub(super) fn check(&self) -> Result<(), String> {
        self.login
            .check()
            .map_err(|_| "The Console login is unavailable")?;
        let subject = self
            .human
            .current()
            .map_err(|_| "The Console human is no longer current")?;
        if let Some(access) = self.access.as_ref().filter(|access| access.enabled()) {
            let view = access.view_for_subject(Some(subject.as_str()));
            if view.enforced() && !view.allows_agent(ACTION_AGENT_SEND, self.identity.as_str()) {
                return Err("The Console human may no longer send to this agent".into());
            }
        }
        self.source
            .host_lifetime
            .get()
            .and_then(Weak::upgrade)
            .ok_or("The installed host is unavailable")?
            .validate_current_selection()
            .map_err(|error| match error {
                meerkat_toolkit::host_lifetime::control::HostLifetimeError::Unavailable => {
                    "The installed host selection could not be verified"
                }
                _ => "The installed host selection changed",
            })?;
        Ok(())
    }
    pub(super) fn reserve(&self) -> Result<ConsoleLoginReservation, String> {
        self.check()?;
        self.login
            .reserve()
            .map_err(|_| "The Console login refused entry".into())
    }
}
impl GovernedEntryFence for ConsoleEntry {
    fn enter(&self) -> Result<Box<dyn GovernedEntryPermit>, GovernedEntryRefusal> {
        self.check().map_err(GovernedEntryRefusal::Unavailable)?;
        // The same login's mutex decides reserve versus retirement immediately
        // before native admission. No awaited Python or store read occurs here.
        GovernedEntryFence::enter(&self.login)
    }
}

struct InstalledConsoleIngress {
    source: Arc<PrivateOriginals>,
    logins: ConsoleLogins,
    access: Option<AccessController>,
}
impl PrivateOriginals {
    /// Retain installed route facts before roster bootstrap. Native supplies
    /// the registered identity at actual bind, where its read-only alias owner
    /// can validate the target without taking another lifecycle lease.
    pub(crate) async fn console_ingress(
        self: &Arc<Self>,
        logins: ConsoleLogins,
        access: Option<AccessController>,
    ) -> Result<Arc<dyn ConsoleGovernedIngress>, PrivateOriginalError> {
        let lifetime = self
            .host_lifetime
            .get()
            .and_then(Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        lifetime
            .validate_current_selection()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        self.current_console_routes()?;
        Ok(Arc::new(InstalledConsoleIngress {
            source: self.clone(),
            logins,
            access,
        }))
    }

    pub(super) fn current_console_routes(
        &self,
    ) -> Result<Vec<PrivateOriginalRoute>, PrivateOriginalError> {
        let mut routes = vec![];
        for route in self.current_routes()? {
            if self
                .service_tool_policy
                .get()
                .map(|owner| owner.member_route(&route.id))
                .transpose()
                .map_err(|_| PrivateOriginalError::Unavailable)?
                .flatten()
                .is_some_and(|(admission, _)| !admission.class.purpose.is_ordinary())
            {
                continue;
            }
            let (compiled, _, _) = self.selected_source_route(&route.id)?;
            if !matches!(
                &compiled.origin,
                CompiledNativeSourceOrigin::LocalConsole { .. }
            ) {
                continue;
            }
            // These are installed principal facts. The verified native human
            // must exactly match at request selection; no alias is introduced.
            if route.requester.kind != meerkat_core::PrincipalKind::Human
                || route.represented_subject.as_ref() != Some(&route.requester)
            {
                continue;
            }
            routes.push(route);
        }
        Ok(routes)
    }
}
impl ConsoleGovernedIngress for InstalledConsoleIngress {
    fn governs(&self, _identity: &AgentIdentity) -> bool {
        true
    }
    fn binder_for(
        &self,
        identity: &AgentIdentity,
        human: &AuthenticatedConsoleHuman,
    ) -> Result<Arc<dyn GovernedInputBinder>, String> {
        self.select_binder(identity, human, None)
    }
    fn binder_for_send(
        &self,
        identity: &AgentIdentity,
        human: &AuthenticatedConsoleHuman,
        send: &ConsoleSendContext,
    ) -> Result<Arc<dyn GovernedInputBinder>, String> {
        let child = self
            .source
            .console_child_for_send(&send.idempotency_key)
            .map_err(|_| "The Console child selection is unavailable")?;
        self.select_binder(identity, human, child)
    }
}
impl InstalledConsoleIngress {
    fn select_binder(
        &self,
        identity: &AgentIdentity,
        human: &AuthenticatedConsoleHuman,
        child: Option<Arc<setup_workflow::PreparedConsoleChild>>,
    ) -> Result<Arc<dyn GovernedInputBinder>, String> {
        let principal = human
            .principal()
            .map_err(|_| "The Console human is unqualified")?;
        let routes: Vec<_> = self
            .source
            .current_console_routes()
            .map_err(|_| "The current Console routes are unavailable")?
            .into_iter()
            .filter(|route| {
                route.requester == principal
                    && route.represented_subject.as_ref() == Some(&principal)
            })
            .collect();
        if routes.is_empty() {
            return Err("No installed Console route matches this human".into());
        }
        let scope = human
            .login_scope()
            .ok_or("The Console login association is missing")?;
        let login = self
            .logins
            .resolve(human.issuer(), human.subject(), scope)
            .map_err(|_| "The exact Console login is unavailable")?;
        if let Some(child) = &child {
            child
                .check(&self.source, identity, &principal, &login)
                .map_err(|_| "The prepared Console child does not match this send")?;
        }
        let entry = ConsoleEntry {
            source: self.source.clone(),
            identity: identity.clone(),
            human: human.clone(),
            access: self.access.clone(),
            login,
        };
        entry.check()?;
        Ok(Arc::new(ConsoleBinder {
            source: self.source.clone(),
            routes,
            entry,
            child,
            delegate: OnceLock::new(),
        }))
    }
}

struct ConsoleBinder {
    source: Arc<PrivateOriginals>,
    routes: Vec<PrivateOriginalRoute>,
    entry: ConsoleEntry,
    child: Option<Arc<setup_workflow::PreparedConsoleChild>>,
    delegate: OnceLock<Arc<OriginalBinder>>,
}
#[async_trait::async_trait]
impl GovernedInputBinder for ConsoleBinder {
    async fn try_bind(
        &self,
        context: &GovernedBindContext,
        input: Input,
    ) -> Result<BoundGovernedInput, GovernedBindError> {
        let binder = self
            .prepare_delegate(context, &input)
            .await
            .map_err(BindFailure::into_error)?;
        // The delegate's own answer, as typed.
        binder.try_bind(context, input).await
    }
    fn retain_unresolved(&self, settlement: Box<dyn GovernedSettlement>, detail: &str) {
        if let Some(binder) = self.delegate.get() {
            binder.retain_unresolved(settlement, detail);
        } else {
            drop(settlement);
        }
    }
}

impl ConsoleBinder {
    /// Retain the Console's source, prepare its child and settle its login
    /// reservation, then hand the send to the shared binder (once).
    async fn prepare_delegate(
        &self,
        context: &GovernedBindContext,
        input: &Input,
    ) -> Result<Arc<OriginalBinder>, BindFailure> {
        if self.delegate.get().is_some() {
            return Err(BindFailure::denied("The Console binder was already used"));
        }
        // The Console itself created this raw Input. Its exact native id and
        // canonical bytes are source evidence, never a browser-selected grant.
        let event_key = stock_console_event_key(input)?;
        let reservation = self.entry.reserve().map_err(BindFailure::denied)?;
        if context.spec.identity != self.entry.identity {
            return Err(BindFailure::denied(
                "The native registered Console target changed",
            ));
        }
        if let Some(child) = &self.child {
            let principal = self
                .entry
                .human
                .principal()
                .map_err(|_| "The Console human is unqualified")?;
            child
                .check(
                    &self.source,
                    &context.spec.identity,
                    &principal,
                    self.entry.login(),
                )
                .map_err(|_| "The prepared Console child is no longer current")?;
            child
                .claim(input)
                .map_err(|_| "The prepared Console child was reused or its input changed")?;
        }
        let runtime = self
            .source
            .bound_identity_runtime()
            .map_err(|_| "The native identity owner is unavailable")?;
        let mut selected = None;
        for route in &self.routes {
            // This exact native method only reads the registered entries; it
            // does not acquire the lifecycle lease already held by this bind.
            let identity = runtime
                .owned_identity_for_member_alias(route.grant_target.as_str())
                .await;
            self.entry.check()?;
            if identity.as_ref() == Some(&context.spec.identity) {
                if selected.is_some() {
                    return Err("The installed Console route is ambiguous".into());
                }
                selected = Some(route);
            }
        }
        let route = selected.ok_or("No installed Console route matches the native target")?;
        if self
            .child
            .as_ref()
            .is_some_and(|child| child.route() != &route.id)
        {
            return Err("The Console child selected another source route".into());
        }
        if self
            .source
            .route(&route.id)
            .map_err(|_| "The selected Console route is unavailable")?
            .as_ref()
            != route
        {
            return Err("The selected Console route changed".into());
        }
        let bytes = canonical_json(input).map_err(|_| "The Console input cannot be retained")?;
        let input_id = EvidenceId::new(input.id().to_string())
            .map_err(|_| "The Console input id is invalid")?;
        let revision =
            EvidenceId::new("console-input-v1").map_err(|_| "Invalid evidence revision")?;
        let authentication = PrivateSourceEvidence {
            resource_id: input_id.clone(),
            revision: revision.clone(),
            sha256: self
                .entry
                .human
                .token_digest()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
        };
        let event = PrivateAuthenticatedEvent {
            route: route.id.clone(),
            original: input_id.clone(),
            occurrence: input_id.clone(),
            original_authentication: authentication.clone(),
            root_event: PrivateSourceEvidence {
                resource_id: input_id,
                revision,
                sha256: digest(&bytes),
            },
            contributors: vec![],
            content_sha256: digest(&bytes),
        };
        if let Some(child) = &self.child {
            child
                .retain_source_locator(PrivateOriginalRef {
                    route: event.route.clone(),
                    original: event.original.clone(),
                    event_sha256: digest(
                        &canonical_json(&event)
                            .map_err(|_| "The Console event cannot be retained")?,
                    ),
                })
                .map_err(|_| "The Console child already retained a source")?;
        }
        // A bind attempt's durable effect: no "not now" may follow it.
        meerkat_core::protected_section::note_effect("console.source_record");
        let original = self
            .source
            .retain_event_for_route_with_input(
                event,
                route,
                None,
                self.child
                    .as_ref()
                    .map(|_| ConsoleChildInputEvidence::NativeConsole),
            )
            .await
            .map_err(|_| "The Console original could not be retained")?;
        self.entry.check()?;
        if let Some(child) = &self.child {
            // A bind attempt's durable effect: no "not now" may follow it.
            meerkat_core::protected_section::note_effect("console.child_prepared");
            child
                .prepare(
                    &self.source,
                    original.clone(),
                    self.entry.clone(),
                    context,
                    input,
                    authentication.clone(),
                )
                .await
                .map_err(|_| "The selected Console child could not be prepared")?;
            self.entry.check()?;
        }
        reservation.settled();
        let binder = Arc::new(OriginalBinder {
            review_custody: crate::review_custody::ReviewCustody::default(),
            source: self.source.clone(),
            grant: None,
            batch: None,
            original,
            held: OnceLock::new(),
            pending: OnceLock::new(),
            authentication: evidence(&route.authentication_domain, &authentication)
                .map_err(|_| "The Console authentication evidence is invalid")?,
            event_key,
            console: Some(self.entry.clone()),
            approved_login: None,
        });
        self.delegate
            .set(binder.clone())
            .map_err(|_| BindFailure::denied("The Console binder was already used"))?;
        Ok(binder)
    }
}

/// Native binding writes its event key back into the input header. Console
/// already owns this key through its accepted interaction and frame; preserve
/// it exactly so native delivery still projects to the same transcript frame.
fn stock_console_event_key(input: &Input) -> Result<IdempotencyKey, String> {
    input
        .header()
        .idempotency_key
        .clone()
        .ok_or_else(|| "The stock Console input has no idempotency key".to_owned())
}

#[cfg(test)]
pub(crate) mod auth_fixture;
#[cfg(test)]
mod tests;
