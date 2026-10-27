//! The installed route and native verified human produce one actual source.
//! Forwarded login labels alone never select a requester or a route.

use super::*;
use super::governed_submit::{OriginalBinder, evidence};
use crate::private_host::{ConsoleLoginHandle, ConsoleLoginReservation, ConsoleLogins};
use meerkat_mobkit::{
    access::{AccessController, ACTION_AGENT_SEND},
    console_governed_ingress::ConsoleGovernedIngress,
    identity_first::{AgentIdentity, BoundGovernedInput, GovernedBindContext,
        GovernedInputBinder, GovernedSettlement, GovernedEntryFence,
        GovernedEntryPermit, GovernedEntryRefusal},
    runtime::AuthenticatedConsoleHuman,
};
use meerkat_runtime::{Input, identifiers::IdempotencyKey};
use meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceOrigin;

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
    pub(super) fn login(&self) -> &ConsoleLoginHandle { &self.login }
    pub(super) fn validate_custody_input(
        &self, record: &SourceRecord, raw: &Input, bound: &Input,
    ) -> Result<(), PrivateOriginalError> {
        self.check().map_err(|_| PrivateOriginalError::Denied)?;
        let key = stock_console_event_key(raw).map_err(|_| PrivateOriginalError::Conflict)?;
        let raw_digest = digest(&canonical_json(raw).map_err(|_| PrivateOriginalError::Conflict)?);
        let (Input::Prompt(raw_prompt), Input::Prompt(bound_prompt)) = (raw, bound)
            else { return Err(PrivateOriginalError::Conflict); };
        if record.event.original.as_str() != raw.id().to_string()
            || record.event.occurrence != record.event.original
            || record.event.root_event.resource_id != record.event.original
            || record.event.root_event.sha256 != raw_digest
            || record.event.content_sha256 != raw_digest
            || bound.id() != raw.id()
            || bound.header().idempotency_key.as_ref() != Some(&key)
            || canonical_json(&bound_prompt.content).map_err(|_| PrivateOriginalError::Conflict)?
                != canonical_json(&raw_prompt.content).map_err(|_| PrivateOriginalError::Conflict)?
        { return Err(PrivateOriginalError::Conflict); }
        Ok(())
    }
    pub(super) fn check(&self) -> Result<(), String> {
        self.login.check().map_err(|_| "The Console login is unavailable")?;
        let subject = self.human.current().map_err(|_| "The Console human is no longer current")?;
        if let Some(access) = self.access.as_ref().filter(|access| access.enabled()) {
            let view = access.view_for_subject(Some(subject.as_str()));
            if view.enforced() && !view.allows_agent(ACTION_AGENT_SEND, self.identity.as_str()) {
                return Err("The Console human may no longer send to this agent".into());
            }
        }
        self.source.host_lifetime.get().and_then(Weak::upgrade)
            .ok_or("The installed host is unavailable")?.validate_current_selection()
            .map_err(|error| match error {
                meerkat_toolkit::host_lifetime::control::HostLifetimeError::Unavailable =>
                    "The installed host selection could not be verified",
                _ => "The installed host selection changed",
            })?;
        Ok(())
    }
    pub(super) fn reserve(&self) -> Result<ConsoleLoginReservation, String> {
        self.check()?;
        self.login.reserve().map_err(|_| "The Console login refused entry".into())
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
    routes: Vec<PrivateOriginalRoute>,
    access: Option<AccessController>,
}
impl PrivateOriginals {
    /// Retain installed route facts before roster bootstrap. Native supplies
    /// the registered identity at actual bind, where its read-only alias owner
    /// can validate the target without taking another lifecycle lease.
    pub(crate) async fn console_ingress(
        self: &Arc<Self>, logins: ConsoleLogins, access: Option<AccessController>,
    ) -> Result<Arc<dyn ConsoleGovernedIngress>, PrivateOriginalError> {
        let lifetime = self.host_lifetime.get().and_then(Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        lifetime.validate_current_selection().map_err(|_| PrivateOriginalError::Unavailable)?;
        let policy = lifetime.selected_policy();
        let native = policy.native_host.as_ref().ok_or(PrivateOriginalError::Unavailable)?;
        let mut routes = vec![];
        for route in self.routes()? {
            let Some(compiled) = native.routes.iter().find(|selected| selected.id == route.id) else {
                return Err(PrivateOriginalError::Denied);
            };
            if !matches!(&compiled.origin, CompiledNativeSourceOrigin::LocalConsole { .. }) {
                continue;
            }
            // These are installed principal facts. The verified native human
            // must exactly match at request selection; no alias is introduced.
            if route.requester.kind != meerkat_core::PrincipalKind::Human
                || route.represented_subject.as_ref() != Some(&route.requester)
            { continue; }
            routes.push(route.clone());
        }
        Ok(Arc::new(InstalledConsoleIngress { source: self.clone(), logins, routes, access }))
    }
}
impl ConsoleGovernedIngress for InstalledConsoleIngress {
    fn governs(&self, _identity: &AgentIdentity) -> bool { true }
    fn binder_for(&self, identity: &AgentIdentity, human: &AuthenticatedConsoleHuman)
        -> Result<Arc<dyn GovernedInputBinder>, String>
    {
        let principal = human.principal().map_err(|_| "The Console human is unqualified")?;
        let routes: Vec<_> = self.routes.iter().filter(|route| route.requester == principal
            && route.represented_subject.as_ref() == Some(&principal)).cloned().collect();
        if routes.is_empty() { return Err("No installed Console route matches this human".into()); }
        let scope = human.login_scope().ok_or("The Console login association is missing")?;
        let login = self.logins.resolve(human.issuer(), human.subject(), scope)
            .map_err(|_| "The exact Console login is unavailable")?;
        let entry = ConsoleEntry { source: self.source.clone(), identity: identity.clone(),
            human: human.clone(), access: self.access.clone(), login };
        entry.check()?;
        Ok(Arc::new(ConsoleBinder { source: self.source.clone(), routes,
            entry, delegate: OnceLock::new() }))
    }
}

struct ConsoleBinder {
    source: Arc<PrivateOriginals>,
    routes: Vec<PrivateOriginalRoute>,
    entry: ConsoleEntry,
    delegate: OnceLock<Arc<OriginalBinder>>,
}
#[async_trait::async_trait]
impl GovernedInputBinder for ConsoleBinder {
    async fn bind(&self, context: &GovernedBindContext, input: Input)
        -> Result<BoundGovernedInput, String>
    {
        if self.delegate.get().is_some() { return Err("The Console binder was already used".into()); }
        // The Console itself created this raw Input. Its exact native id and
        // canonical bytes are source evidence, never a browser-selected grant.
        let event_key = stock_console_event_key(&input)?;
        let reservation = self.entry.reserve()?;
        if context.spec.identity != self.entry.identity {
            return Err("The native registered Console target changed".into());
        }
        let runtime = self.source.bound_identity_runtime()
            .map_err(|_| "The native identity owner is unavailable")?;
        let mut selected = None;
        for route in &self.routes {
            // This exact native method only reads the registered entries; it
            // does not acquire the lifecycle lease already held by this bind.
            let identity = runtime.owned_identity_for_member_alias(route.grant_target.as_str()).await;
            self.entry.check()?;
            if identity.as_ref() == Some(&context.spec.identity) {
                if selected.is_some() { return Err("The installed Console route is ambiguous".into()); }
                selected = Some(route);
            }
        }
        let route = selected.ok_or("No installed Console route matches the native target")?;
        let bytes = canonical_json(&input).map_err(|_| "The Console input cannot be retained")?;
        let input_id = EvidenceId::new(input.id().to_string()).map_err(|_| "The Console input id is invalid")?;
        let revision = EvidenceId::new("console-input-v1").map_err(|_| "Invalid evidence revision")?;
        let authentication = PrivateSourceEvidence {
            resource_id: input_id.clone(), revision: revision.clone(),
            sha256: self.entry.human.token_digest().iter().map(|b| format!("{b:02x}")).collect(),
        };
        let event = PrivateAuthenticatedEvent {
            route: route.id.clone(), original: input_id.clone(), occurrence: input_id.clone(),
            original_authentication: authentication.clone(),
            root_event: PrivateSourceEvidence { resource_id: input_id, revision, sha256: digest(&bytes) },
            contributors: vec![], content_sha256: digest(&bytes),
        };
        let original = self.source.retain_event(event).await
            .map_err(|_| "The Console original could not be retained")?;
        self.entry.check()?;
        reservation.settled();
        let binder = Arc::new(OriginalBinder { source: self.source.clone(), grant: None, batch: None, original,
            held: OnceLock::new(), pending: OnceLock::new(),
            authentication: evidence(&route.authentication_domain, &authentication)
                .map_err(|_| "The Console authentication evidence is invalid")?,
            event_key, console: Some(self.entry.clone()), approved_login: None });
        self.delegate.set(binder.clone()).map_err(|_| "The Console binder was already used")?;
        binder.bind(context, input).await
    }
    fn retain_unresolved(&self, settlement: Box<dyn GovernedSettlement>, detail: &str) {
        if let Some(binder) = self.delegate.get() { binder.retain_unresolved(settlement, detail); }
        else { drop(settlement); }
    }
}

/// Native binding writes its event key back into the input header. Console
/// already owns this key through its accepted interaction and frame; preserve
/// it exactly so native delivery still projects to the same transcript frame.
fn stock_console_event_key(input: &Input) -> Result<IdempotencyKey, String> {
    input.header().idempotency_key.clone()
        .ok_or_else(|| "The stock Console input has no idempotency key".to_owned())
}

#[cfg(test)]
pub(crate) mod auth_fixture;
#[cfg(test)]
mod tests;
