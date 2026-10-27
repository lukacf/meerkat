//! Existing installed source and private-login owners for native HTTP routes.
use super::*;
use meerkat_mobkit::http_console::{ConsoleViewerContext, ConsoleViewerResolver};
use crate::private_host::{ConsoleLoginHandle, ConsoleLogins};

// Only fixed labels and unit error categories are written to local stderr.
fn status_read_refused(stage: &'static str, cause: &'static str) {
    use std::io::Write as _;
    let _ = writeln!(std::io::stderr().lock(),
        "{{\"event\":\"toolkit_console_status_refused\",\"stage\":\"{stage}\",\"cause\":\"{cause}\"}}");
}
fn status_owner_refused(stage: &'static str, error: HostLifetimeError) {
    status_read_refused(stage, match error {
        HostLifetimeError::InvalidSelection => "InvalidSelection",
        HostLifetimeError::Unavailable => "Unavailable",
        HostLifetimeError::Denied => "Denied",
        HostLifetimeError::WrongOwner => "WrongOwner",
        HostLifetimeError::NotSettled => "NotSettled",
    });
}

// An unavailable owner observation cannot establish either visibility or denial.
fn status_visibility_error(stage: &'static str, error: PrivateOriginalError) -> HostLifetimeError {
    status_read_refused(stage, match error {
        PrivateOriginalError::Unavailable => "Unavailable",
        PrivateOriginalError::Conflict => "Conflict",
        PrivateOriginalError::Denied => "Denied",
    });
    match error {
        PrivateOriginalError::Unavailable => HostLifetimeError::Unavailable,
        PrivateOriginalError::Conflict | PrivateOriginalError::Denied => HostLifetimeError::Denied,
    }
}

pub type ConsoleRouteComposition = Arc<dyn Fn(ConsoleViewerResolver)
    -> Result<axum::Router, HostLifetimeError> + Send + Sync>;

#[derive(Clone)]
pub struct ConsoleSetupBinding {
    owner: Arc<NativeHostLifetime>,
    source: Arc<PrivateOriginals>,
    logins: ConsoleLogins,
    closed: PrivateHostClosed,
    access_policy: Option<ConsoleAccessPolicy>,
    recovery_policy: Option<ConsoleRecoveryPolicy>,
}
/// A bounded current read with private canonical positions. Browser IDs may
/// select this presentation but never construct a source or an access grant.
pub struct ConsoleStatusPage {
    source: Arc<PrivateOriginals>,
    login: ConsoleLoginHandle,
    principal: meerkat_core::PrincipalRef,
    start: Option<String>,
    next: Option<String>,
    projection: serde_json::Value,
    visible_members: Vec<meerkat_mobkit::identity_first::AgentIdentity>,
    access: Option<Result<ConsoleAccessSnapshot, ()>>,
    recovery: Option<Result<ConsoleServiceRecoverySnapshot, ()>>,
}
#[derive(Clone, Copy)]
pub enum ConsolePageRead { Refresh, Next }
impl ConsoleStatusPage {
    pub fn same_login(&self, login: &ConsoleLoginHandle) -> bool { self.login.same_owner(login) }
    pub fn is_first(&self) -> bool { self.start.is_none() }
    pub fn has_next(&self) -> bool { self.next.is_some() }
    pub fn projection(&self) -> serde_json::Value { self.projection.clone() }
    pub fn access(&self) -> Option<&ConsoleAccessSnapshot> {
        self.access.as_ref().and_then(|value| value.as_ref().ok())
    }
    pub fn access_projection(&self) -> Option<serde_json::Value> {
        self.access.as_ref().map(|value| match value {
            Ok(offer) => offer.projection(),
            Err(()) => serde_json::json!({"scope":"application_issued_access","state":"unavailable"}),
        })
    }
    pub fn recovery(&self) -> Option<&ConsoleServiceRecoverySnapshot> {
        self.recovery.as_ref().and_then(|value| value.as_ref().ok())
    }
    pub fn recovery_projection(&self) -> Option<serde_json::Value> {
        self.recovery.as_ref().map(|value| match value {
            Ok(offer) => offer.projection(),
            Err(()) => serde_json::json!({"scope":"operator_service_recovery","state":"unavailable"}),
        })
    }
}

impl ConsoleSetupBinding {
    #[cfg(test)]
    pub(crate) fn fixture(owner: Arc<NativeHostLifetime>, source: Arc<PrivateOriginals>,
        logins: ConsoleLogins, closed: PrivateHostClosed) -> Self {
        Self { owner, source, logins, closed, access_policy: None, recovery_policy: None }
    }

    /// A page position retains no authorization snapshot. Every refresh/next
    /// repeats the installed/login/native-view and canonical row checks.
    pub async fn status_page(&self,
        current: &(impl Fn() -> Option<ConsoleViewerContext> + Sync),
        previous: Option<(&ConsoleStatusPage, ConsolePageRead)>,
    ) -> Result<ConsoleStatusPage, HostLifetimeError> {
        let viewer = current().ok_or(HostLifetimeError::Denied).map_err(|error| {
            status_owner_refused("binding.status_page.current_viewer", error);
            error
        })?;
        let login = self.current_login(&viewer).map_err(|error| {
            status_owner_refused("binding.status_page.current_login", error);
            error
        })?;
        let principal = viewer.human.as_ref().ok_or(HostLifetimeError::Denied).map_err(|error| {
            status_owner_refused("binding.status_page.human", error);
            error
        })?.principal().map_err(|_| {
            status_read_refused("binding.status_page.principal", "Denied");
            HostLifetimeError::Denied
        })?;
        let start = match previous {
            None => None,
            Some((page, direction)) => {
                if !Arc::ptr_eq(&self.source, &page.source) || !page.login.same_owner(&login) {
                    status_read_refused("binding.status_page.previous_owner", "Denied");
                    return Err(HostLifetimeError::Denied);
                }
                match direction {
                    ConsolePageRead::Refresh => page.start.clone(),
                    ConsolePageRead::Next => Some(page.next.clone().ok_or(HostLifetimeError::Denied).map_err(|error| {
                        status_owner_refused("binding.status_page.next_position", error);
                        error
                    })?),
                }
            },
        };
        let page = if self.recovery_policy.is_some() {
            self.source.source_status_page_with_recovery(start.as_deref()).await
        } else { self.source.source_status_page(start.as_deref()).await }
            .map_err(|error| {
                status_read_refused("binding.status_page.source_page", match error {
                    PrivateOriginalError::Unavailable => "Unavailable",
                    PrivateOriginalError::Conflict => "Conflict",
                    PrivateOriginalError::Denied => "Denied",
                });
                match error {
                    PrivateOriginalError::Denied => HostLifetimeError::Denied,
                    PrivateOriginalError::Unavailable | PrivateOriginalError::Conflict => HostLifetimeError::Unavailable,
                }
            })?;
        // Every selected projection shares this bounded canonical slice.
        // Absent recovery selection preserves the original page membership.
        let task_read = self.source.console_status_native_page(&viewer, current, &page.rows).await
            .map_err(|error| {
                status_read_refused("binding.status_page.native_projection", match error {
                    PrivateOriginalError::Unavailable => "Unavailable",
                    PrivateOriginalError::Conflict => "Conflict",
                    PrivateOriginalError::Denied => "Denied",
                });
                match error {
                    PrivateOriginalError::Denied => HostLifetimeError::Denied,
                    PrivateOriginalError::Unavailable | PrivateOriginalError::Conflict => HostLifetimeError::Unavailable,
                }
            })?;
        let access = match self.application_access_rows(current, Some(&page.rows)).await {
            Ok(value) => value.map(Ok),
            Err(_) => Some(Err(())),
        };
        let recovery = match self.service_recovery_rows(current, &page.rows).await {
            Ok(value) => value.map(Ok),
            Err(_) => Some(Err(())),
        };
        let result = ConsoleStatusPage { source:self.source.clone(), login, principal, start,
            next:page.next_after, projection:task_read.projection,
            visible_members:task_read.visible_members, access, recovery };
        self.check_status_page(&result, current).map_err(|error| {
            status_owner_refused("binding.status_page.final_currentness", error);
            error
        })?;
        Ok(result)
    }

    /// Final synchronous disclosure check after local presentation work. It
    /// does not replace canonical reads or authorize a revoke.
    pub fn check_status_page(&self, page: &ConsoleStatusPage,
        current: &(impl Fn() -> Option<ConsoleViewerContext> + Sync),
    ) -> Result<(), HostLifetimeError> {
        let fresh = current().ok_or(HostLifetimeError::Denied).map_err(|error| {
            status_owner_refused("binding.check_status_page.viewer", error); error
        })?;
        if !Arc::ptr_eq(&self.source, &page.source) {
            status_read_refused("binding.check_status_page.source", "Denied");
            return Err(HostLifetimeError::Denied);
        }
        if !page.login.same_owner(&self.current_login(&fresh)?) {
            status_read_refused("binding.check_status_page.login_owner", "Denied");
            return Err(HostLifetimeError::Denied);
        }
        if fresh.human.as_ref().and_then(|human| human.principal().ok()).as_ref() != Some(&page.principal) {
            status_read_refused("binding.check_status_page.principal", "Denied");
            return Err(HostLifetimeError::Denied);
        }
        if fresh.access_view.as_ref().is_some_and(|view| view.enforced()
            && page.visible_members.iter().any(|identity| !view.can_view_agent(identity.as_str()))) {
            status_read_refused("binding.check_status_page.member_visibility", "Denied");
            return Err(HostLifetimeError::Denied);
        }
        if let Some(Ok(offer)) = &page.access { offer.check_visible(&fresh)?; }
        if let Some(Ok(offer)) = &page.recovery { offer.check_visible(&fresh)?; }
        Ok(())
    }

    /// Retain the actual activation close fence through the bounded native
    /// Human/admin check and same-login reservation. No enrollment is resolved
    /// and no filesystem observation is made under native writer custody.
    pub fn with_credential_entry<T>(&self, enter: impl FnOnce() -> T) -> Result<T, HostLifetimeError> {
        if self.closed.is_closed() { return Err(HostLifetimeError::Unavailable); }
        self.owner.with_native_administration_entry(enter)
    }

    pub fn current_login(&self, viewer: &ConsoleViewerContext) -> Result<ConsoleLoginHandle, HostLifetimeError> {
        if self.closed.is_closed() {
            status_read_refused("binding.current_login.closed", "Denied");
            return Err(HostLifetimeError::Denied);
        }
        if viewer.service_identity {
            status_read_refused("binding.current_login.service_identity", "Denied");
            return Err(HostLifetimeError::Denied);
        }
        self.owner.validate_current_selection().map_err(|error| {
            status_owner_refused("binding.current_login.selection", error); error
        })?;
        let human = viewer.human.as_ref().ok_or(HostLifetimeError::Denied).map_err(|error| {
            status_owner_refused("binding.current_login.human", error); error
        })?;
        human.current().map_err(|_| {
            status_read_refused("binding.current_login.current", "Denied"); HostLifetimeError::Denied
        })?;
        human.principal().map_err(|_| {
            status_read_refused("binding.current_login.principal", "Denied"); HostLifetimeError::Denied
        })?;
        let scope = human.login_scope().ok_or(HostLifetimeError::Denied).map_err(|error| {
            status_owner_refused("binding.current_login.scope", error); error
        })?;
        self.logins.resolve(human.issuer(), human.subject(), scope).map_err(|_| {
            status_read_refused("binding.current_login.resolve", "Denied"); HostLifetimeError::Denied
        })
    }
    /// Pure canonical read. Re-resolve the HTTP viewer after this await before
    /// disclosing the result. This is not a reconciliation or readiness call.
    pub async fn status(&self, current: &(impl Fn() -> Option<ConsoleViewerContext> + Sync)) -> Result<serde_json::Value, HostLifetimeError> {
        let viewer = current().ok_or(HostLifetimeError::Denied)?;
        let login = self.current_login(&viewer)?;
        let value = self.source.console_status_native(&viewer, current).await
            .map_err(|_| HostLifetimeError::Unavailable)?;
        if !login.same_owner(&self.current_login(&current().ok_or(HostLifetimeError::Denied)?)?) { return Err(HostLifetimeError::Denied); }
        Ok(value)
    }
}
impl ToolkitGatewayHost {
    /// Available only from the already selected host and its actual private pipe.
    pub fn console_setup_binding(&self) -> Result<ConsoleSetupBinding, HostLifetimeError> {
        self.owner.validate_current_selection()?;
        if self.machine.get().is_some() || self.private_closed.is_closed() {
            return Err(HostLifetimeError::InvalidSelection);
        }
        Ok(ConsoleSetupBinding { owner: self.owner.clone(),
            source: self.private_originals.clone().ok_or(HostLifetimeError::InvalidSelection)?,
            logins: self.console_logins.clone().ok_or(HostLifetimeError::InvalidSelection)?,
            closed: self.private_closed.clone(), access_policy: None, recovery_policy: None })
    }
    pub fn with_console_routes(mut self, routes: ConsoleRouteComposition) -> Result<Self, HostLifetimeError> {
        if self.console_routes.is_some() || self.machine.get().is_some() || self.setup_application.is_none()
            || self.private_closed.is_closed() { return Err(HostLifetimeError::InvalidSelection); }
        self.owner.validate_current_selection()?;
        self.console_routes = Some(routes);
        Ok(self)
    }
}

/// The installed application may let an authenticated requester revoke only
/// leaves that this application issued for that same retained requester.
/// This is a selected host policy, not authority inferred from visibility.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleAccessPolicy { RequesterIssuedLeaves }

/// Separate installed administrative selection. It permits an authenticated
/// operator to recover only that same operator's unpublished service approval.
/// RequesterIssuedLeaves and member visibility never imply this permission.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleRecoveryPolicy { OperatorUnpublishedServiceApprovals }

/// A single protected operator action over an existing canonical read. This
/// owns presentation/result custody only, not a new durable recovery record.
pub struct ConsoleServiceRecoverySnapshot {
    source: Arc<PrivateOriginals>,
    login: ConsoleLoginHandle,
    scope: String,
    read: crate::private_originals::service_recovery::ServiceRecoveryRead,
    members: Vec<(meerkat_core::MobMemberBinding, meerkat_mobkit::identity_first::AgentIdentity)>,
    used: std::sync::atomic::AtomicBool,
    result: Mutex<Option<crate::private_originals::service_recovery::ServiceRecoveryOutcome>>,
}
impl ConsoleServiceRecoverySnapshot {
    fn check_visible(&self, viewer: &ConsoleViewerContext) -> Result<(), HostLifetimeError> {
        let human = viewer.human.as_ref().ok_or(HostLifetimeError::Denied).map_err(|error| {
            status_owner_refused("recovery.check_visible.human", error); error
        })?;
        let subject = human.current().map_err(|_| {
            status_read_refused("recovery.check_visible.current", "Denied"); HostLifetimeError::Denied
        })?;
        let denied = if viewer.service_identity { Some("recovery.check_visible.service_identity") }
            else if viewer.principal.as_ref() != Some(&subject) { Some("recovery.check_visible.subject") }
            else if human.principal().ok().as_ref() != Some(&self.read.human) { Some("recovery.check_visible.principal") }
            else if human.login_scope() != Some(self.scope.as_str()) { Some("recovery.check_visible.scope") }
            else if viewer.access_view.as_ref().is_none_or(|access| !access.can_administer()) { Some("recovery.check_visible.admin") }
            else { None };
        if let Some(stage) = denied {
            status_read_refused(stage, "Denied");
            return Err(HostLifetimeError::Denied);
        }
        self.read.check_visible(|member| self.members.iter().find(|(selected, _)| selected == member)
            .is_some_and(|(_, identity)| viewer.access_view.as_ref().is_some_and(|access|
                !access.enforced() || access.can_view_agent(identity.as_str()))))
            .map_err(|error| status_visibility_error("recovery.check_visible.read", error))
    }
    pub fn projection(&self) -> serde_json::Value {
        serde_json::json!({"scope":"operator_service_recovery",
            "rows":self.read.rows.iter().map(|row| &row.projection).collect::<Vec<_>>()})
    }
}

/// Opaque, one-action presentation custody. Browser values cannot create a
/// source record, select an issuer or supply a native lineage.
pub struct ConsoleAccessSnapshot {
    source: Arc<PrivateOriginals>,
    login: ConsoleLoginHandle,
    scope: String,
    read: crate::private_originals::service_grants::console::ApplicationAccessRead,
    members: Vec<(meerkat_core::MobMemberBinding, meerkat_mobkit::identity_first::AgentIdentity)>,
    used: std::sync::atomic::AtomicBool,
    // Retain the actual entered native result before HTTP serialization. This
    // is observation only, with no restore, retry or issuance API.
    result: Mutex<Option<Result<(), meerkat_authorization::grants::ExpectedRevokeRefusal>>>,
}
impl ConsoleAccessSnapshot {
    /// Correlate presentation with the actual retained login owner. This does
    /// not authenticate a carrier or authorize the protected revoke action.
    pub fn same_login(&self, login: &ConsoleLoginHandle) -> bool { self.login.same_owner(login) }
    fn check_visible(&self, viewer: &ConsoleViewerContext) -> Result<(), HostLifetimeError> {
        self.read.check_visible(|member| self.members.iter().find(|(selected, _)| selected == member)
            .is_some_and(|(_, identity)| !viewer.access_view.as_ref().is_some_and(|view|
                view.enforced() && !view.can_view_agent(identity.as_str()))))
            .map_err(|error| status_visibility_error("access.check_visible.read", error))
    }
    pub fn projection(&self) -> serde_json::Value {
        serde_json::json!({"scope":"application_issued_access",
            "rows":self.read.rows.iter().map(|row| &row.projection).collect::<Vec<_>>()})
    }
}
impl ConsoleSetupBinding {
    /// Called only by the trusted installed route composition, before mount.
    /// The absent selection keeps the legacy status/approval surface unchanged.
    pub fn with_access_policy(mut self, policy: Option<ConsoleAccessPolicy>) -> Self {
        self.access_policy = policy;
        self
    }

    /// Trusted pinned host composition only; HTTP values cannot select this.
    pub fn with_recovery_policy(mut self, policy: Option<ConsoleRecoveryPolicy>) -> Self {
        self.recovery_policy = policy;
        self
    }

    async fn service_recovery_rows(&self,
        current: &(impl Fn() -> Option<ConsoleViewerContext> + Sync), rows: &[toolkit_store::StoredRecord],
    ) -> Result<Option<ConsoleServiceRecoverySnapshot>, HostLifetimeError> {
        if self.recovery_policy != Some(ConsoleRecoveryPolicy::OperatorUnpublishedServiceApprovals) { return Ok(None); }
        let viewer = current().ok_or(HostLifetimeError::Denied)?;
        let login = self.current_login(&viewer)?;
        let human = viewer.human.as_ref().ok_or(HostLifetimeError::Denied)?;
        if viewer.access_view.as_ref().is_none_or(|access| !access.can_administer()) { return Ok(None); }
        let principal = human.principal().map_err(|_| HostLifetimeError::Denied)?;
        let scope = human.login_scope().ok_or(HostLifetimeError::Denied)?.to_owned();
        let members = self.source.console_access_members(&viewer, current).await.map_err(|_| HostLifetimeError::Unavailable)?;
        let selected = members.iter().map(|(member, _)| member.clone()).collect::<Vec<_>>();
        let read = match self.source.read_service_recovery_page(&principal, &selected, rows).await {
            Ok(read) => read,
            Err(PrivateOriginalError::Denied) => return Ok(None),
            Err(_) => return Err(HostLifetimeError::Unavailable),
        };
        let fresh = current().ok_or(HostLifetimeError::Denied)?;
        if !login.same_owner(&self.current_login(&fresh)?) { return Err(HostLifetimeError::Denied); }
        let offer = ConsoleServiceRecoverySnapshot { source:self.source.clone(), login, scope, read, members,
            used:std::sync::atomic::AtomicBool::new(false), result:Mutex::new(None) };
        offer.check_visible(&fresh)?;
        Ok(Some(offer))
    }

    /// All awaited store checks precede native entry. The same installed/login
    /// entry then owns the whole finite native cleanup, synchronously.
    pub async fn recover_service_approval(&self, offer: &ConsoleServiceRecoverySnapshot, row_id: &str,
        current: &(impl Fn() -> Option<ConsoleViewerContext> + Sync),
    ) -> Result<serde_json::Value, HostLifetimeError> {
        if self.recovery_policy != Some(ConsoleRecoveryPolicy::OperatorUnpublishedServiceApprovals)
            || !Arc::ptr_eq(&self.source, &offer.source) { return Err(HostLifetimeError::Denied); }
        let viewer = current().ok_or(HostLifetimeError::Denied)?;
        if !offer.login.same_owner(&self.current_login(&viewer)?) { return Err(HostLifetimeError::Denied); }
        offer.check_visible(&viewer)?;
        let row = offer.read.rows.iter().find(|row| row.id == row_id && row.can_recover)
            .ok_or(HostLifetimeError::Denied)?;
        if offer.used.swap(true, std::sync::atomic::Ordering::AcqRel) { return Err(HostLifetimeError::Denied); }
        let prepared = self.source.prepare_service_recovery(&offer.read, row).await
            .map_err(|_| HostLifetimeError::Unavailable)?;
        let fresh = current().ok_or(HostLifetimeError::Denied)?;
        if !offer.login.same_owner(&self.current_login(&fresh)?) { return Err(HostLifetimeError::Denied); }
        offer.check_visible(&fresh)?;
        let reservation = offer.login.reserve().map_err(|_| HostLifetimeError::Denied)?;
        // No await or worker follows reserve. A returned pre-entry error cannot
        // conceal a parent mutation; native partial outcomes are retained below.
        let entered = self.owner.with_current_setup(|| {
            offer.login.with_current_revoke(|| {
                let viewer = current().ok_or(HostLifetimeError::Denied)?;
                // Resolve only the accepted carrier. Do not reenter login or
                // installed-owner lookup under their mutation exclusion.
                offer.check_visible(&viewer)?;
                let mut result = offer.result.lock().map_err(|_| HostLifetimeError::Unavailable)?;
                let outcome = prepared.revoke().map_err(|_| HostLifetimeError::Unavailable)?;
                let value = serde_json::json!({"scope":"operator_service_recovery", "parent":outcome.parent,
                    "child":outcome.child, "uncertain":outcome.uncertain,
                    "policy_observed_at_offer":row.projection["policy"], "policy_changed":false});
                let uncertain = outcome.uncertain;
                *result = Some(outcome);
                Ok((value, uncertain))
            }).map_err(|_| HostLifetimeError::Denied)?
        });
        match entered {
            Ok((value, true)) => { reservation.uncertain(); Ok(value) },
            Ok((value, false)) => { reservation.settled(); Ok(value) },
            Err(error) => { reservation.settled(); Err(error) },
        }
    }

    pub async fn application_access(&self,
        current: &(impl Fn() -> Option<ConsoleViewerContext> + Sync),
    ) -> Result<Option<ConsoleAccessSnapshot>, HostLifetimeError> {
        self.application_access_rows(current, None).await
    }

    async fn application_access_rows(&self,
        current: &(impl Fn() -> Option<ConsoleViewerContext> + Sync),
        rows: Option<&[toolkit_store::StoredRecord]>,
    ) -> Result<Option<ConsoleAccessSnapshot>, HostLifetimeError> {
        if self.access_policy != Some(ConsoleAccessPolicy::RequesterIssuedLeaves) { return Ok(None); }
        let viewer = current().ok_or(HostLifetimeError::Denied)?;
        let login = self.current_login(&viewer)?;
        let human = viewer.human.as_ref().ok_or(HostLifetimeError::Denied)?;
        let principal = human.principal().map_err(|_| HostLifetimeError::Denied)?;
        let scope = human.login_scope().ok_or(HostLifetimeError::Denied)?.to_owned();
        let members = self.source.console_access_members(&viewer, current).await
            .map_err(|_| HostLifetimeError::Unavailable)?;
        let selected = members.iter().map(|(member, _)| member.clone()).collect::<Vec<_>>();
        let read = match rows {
            Some(rows) => self.source.read_application_access_page(&principal, &selected, rows).await,
            None => self.source.read_application_access(&principal, &selected).await,
        }.map_err(|_| HostLifetimeError::Unavailable)?;
        let fresh = current().ok_or(HostLifetimeError::Denied)?;
        if !login.same_owner(&self.current_login(&fresh)?)
            || fresh.human.as_ref().and_then(|human| human.principal().ok()).as_ref() != Some(&principal) {
            return Err(HostLifetimeError::Denied);
        }
        read.check_visible(|member| members.iter().find(|(selected, _)| selected == member)
            .is_some_and(|(_, identity)| !fresh.access_view.as_ref().is_some_and(|view|
                view.enforced() && !view.can_view_agent(identity.as_str()))))
            .map_err(|error| status_visibility_error("access.read.check_visible", error))?;
        Ok(Some(ConsoleAccessSnapshot { source:self.source.clone(), login, scope, read, members,
            used:std::sync::atomic::AtomicBool::new(false), result:Mutex::new(None) }))
    }

    /// A fresh protected action on an exact retained offer. It does not cancel
    /// work, disconnect an account or delete credentials. A lost result never
    /// permits this offer to enter native mutation again.
    pub async fn revoke_application_access(&self, offer: &ConsoleAccessSnapshot, row_id: &str,
        current: &(impl Fn() -> Option<ConsoleViewerContext> + Sync),
    ) -> Result<serde_json::Value, HostLifetimeError> {
        if self.access_policy != Some(ConsoleAccessPolicy::RequesterIssuedLeaves)
            || !Arc::ptr_eq(&self.source, &offer.source) {
            return Err(HostLifetimeError::Denied);
        }
        let viewer = current().ok_or(HostLifetimeError::Denied)?;
        if !offer.login.same_owner(&self.current_login(&viewer)?) { return Err(HostLifetimeError::Denied); }
        let row = offer.read.rows.iter().find(|row| row.id == row_id).ok_or(HostLifetimeError::Denied)?;
        let identity = &offer.members.iter().find(|(member, _)| member == &row.member)
            .ok_or(HostLifetimeError::Denied)?.1;
        if offer.used.swap(true, std::sync::atomic::Ordering::AcqRel) { return Err(HostLifetimeError::Denied); }
        self.source.recheck_application_access(&offer.read, row).await
            .map_err(|_| HostLifetimeError::Unavailable)?;
        let fresh = current().ok_or(HostLifetimeError::Denied)?;
        if !offer.login.same_owner(&self.current_login(&fresh)?) { return Err(HostLifetimeError::Denied); }
        // No await follows reserve. Synchronous entry and its actual native
        // outcome settle in this same poll before a caller can cancel it.
        let reservation = offer.login.reserve().map_err(|_| HostLifetimeError::Denied)?;
        let entered = self.owner.with_current_setup(|| {
            offer.login.with_current_revoke(|| {
                // Resolve only the accepted native carrier here. Never reenter
                // current_login, login.check/reserve, or the installed owner.
                let viewer = current().ok_or(HostLifetimeError::Denied)?;
                let human = viewer.human.as_ref().ok_or(HostLifetimeError::Denied)?;
                if viewer.service_identity || human.current().is_err()
                    || human.principal().ok().as_ref() != Some(&offer.read.human)
                    || human.login_scope() != Some(offer.scope.as_str())
                    || viewer.access_view.as_ref().is_some_and(|view|
                        view.enforced() && !view.can_view_agent(identity.as_str())) {
                    return Err(HostLifetimeError::Denied);
                }
                let mut retained = offer.result.lock().map_err(|_| HostLifetimeError::Unavailable)?;
                let result = self.source.revoke_application_access_now(&offer.read, row)
                    .map_err(|_| HostLifetimeError::Unavailable)?;
                *retained = Some(result);
                Ok(result)
            }).map_err(|_| HostLifetimeError::Denied)?
        });
        Self::finish_access_revoke(reservation, entered)
    }

    // The same terminal classifier is exercised with actual login reservations.
    // Outer refusal proves nonentry; only the entered native result can mark
    // this local reservation uncertain. It creates no grant or retry authority.
    pub(crate) fn finish_access_revoke(reservation: crate::private_host::ConsoleLoginReservation,
        entered: Result<Result<(), meerkat_authorization::grants::ExpectedRevokeRefusal>, HostLifetimeError>)
        -> Result<serde_json::Value, HostLifetimeError>
    {
        use meerkat_authorization::grants::{ExpectedRevokeRefusal, GrantRefusal};
        if matches!(&entered, Ok(Err(ExpectedRevokeRefusal::Grant(GrantRefusal::Unavailable)))) {
            reservation.uncertain();
        } else { reservation.settled(); }
        let state = match entered? {
            Ok(()) => "revoked",
            Err(ExpectedRevokeRefusal::NotCurrent) => "not_current",
            Err(ExpectedRevokeRefusal::Grant(GrantRefusal::ControllerInUse)) => "controller_in_use",
            Err(ExpectedRevokeRefusal::Grant(GrantRefusal::Denied | GrantRefusal::Duplicate)) => "refused",
            Err(ExpectedRevokeRefusal::Grant(GrantRefusal::Unavailable)) => "unavailable",
        };
        Ok(serde_json::json!({"scope":"application_issued_access","state":state}))
    }
}

#[cfg(test)]
mod status_visibility_tests {
    use super::*;
    use meerkat_authorization::publication::{LocalAuthorizationPublication, PublicationError};

    #[test]
    fn changed_and_poisoned_publication_remain_unavailable_at_visibility_boundary() {
        // Real publication transitions and actual read checks. This conveys no
        // synthetic viewer, grant or protected-action authority.
        for poison in [false, true] {
            let owner = LocalAuthorizationPublication::new();
            let (_, stamp) = owner.observe(|| ()).unwrap();
            let human = meerkat_core::PrincipalRef::in_domain(meerkat_core::PrincipalKind::Human,
                "visibility-fixture", meerkat_core::TrustDomainId::new("visibility-test").unwrap()).unwrap();
            let access = crate::private_originals::service_grants::console::ApplicationAccessRead {
                human: human.clone(), publication: stamp.clone(), rows: vec![],
            };
            let recovery = crate::private_originals::service_recovery::ServiceRecoveryRead {
                human, publication: stamp.clone(), rows: vec![],
            };
            assert!(access.check_visible(|_| true).is_ok());
            assert!(recovery.check_visible(|_| true).is_ok());
            if poison { owner.poison(); } else { drop(owner.begin_owner_change().unwrap()); }
            assert!(matches!((poison, stamp.check_current()),
                (false, Err(PublicationError::Changed)) | (true, Err(PublicationError::Unavailable))));
            for (stage, result) in [("access.check_visible.read", access.check_visible(|_| true)),
                ("recovery.check_visible.read", recovery.check_visible(|_| true))] {
                assert!(matches!(result.map_err(|error| status_visibility_error(stage, error)),
                    Err(HostLifetimeError::Unavailable)));
            }
        }
        for error in [PrivateOriginalError::Denied, PrivateOriginalError::Conflict] {
            assert!(matches!(status_visibility_error("visibility.fixture", error), HostLifetimeError::Denied));
        }
    }
}
