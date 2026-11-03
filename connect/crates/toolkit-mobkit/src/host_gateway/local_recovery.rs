//! Explicit local OS-operator management on an existing installed gateway.
//! This view never opens an owner, accepts a principal, or admits an Original.
use super::*;
use meerkat_core::{MobMemberBinding, PrincipalRef};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalOperatorRecoveryPolicy {
    UnpublishedServiceApprovals,
}

#[derive(Clone, Copy)]
pub enum LocalRecoveryPageRead {
    Refresh,
    Next,
}

pub struct LocalServiceRecovery {
    owner: Arc<NativeHostLifetime>,
    source: Arc<PrivateOriginals>,
    closed: PrivateHostClosed,
    principal: PrincipalRef,
    members: Vec<MobMemberBinding>,
}

/// A bounded owner-bound offer. Cursor and canonical identities are private;
/// the row handles only select records retained in this exact offer.
pub struct LocalServiceRecoveryPage {
    source: Arc<PrivateOriginals>,
    read: crate::private_originals::service_recovery::ServiceRecoveryRead,
    start: Option<String>,
    next: Option<String>,
    used: AtomicBool,
}

pub struct LocalServiceRecoveryResult {
    outcome: crate::private_originals::service_recovery::ServiceRecoveryOutcome,
    policy_observed_at_offer: serde_json::Value,
}
impl LocalServiceRecoveryResult {
    pub fn projection(&self) -> serde_json::Value {
        serde_json::json!({"scope":"local_operator_service_recovery",
            "parent":self.outcome.parent,"child":self.outcome.child,
            "uncertain":self.outcome.uncertain,
            "policy_observed_at_offer":self.policy_observed_at_offer,"policy_changed":false})
    }
    pub fn uncertain(&self) -> bool {
        self.outcome.uncertain
    }
}
impl LocalServiceRecoveryPage {
    pub fn projection(&self) -> serde_json::Value {
        serde_json::json!({"scope":"local_operator_service_recovery",
            "rows":self.read.rows.iter().map(|row| &row.projection).collect::<Vec<_>>(),
            "has_next":self.next.is_some()})
    }
    pub fn has_next(&self) -> bool {
        self.next.is_some()
    }
}

impl ToolkitGatewayHost {
    /// Trusted installed-artifact composition only, before native attachment.
    /// This selection is independent of every Console/admin/viewer policy.
    pub fn with_local_operator_recovery(
        mut self,
        policy: LocalOperatorRecoveryPolicy,
    ) -> Result<Self, HostLifetimeError> {
        self.owner.validate_current_selection()?;
        if self.private_closed.is_closed()
            || self.machine.get().is_some()
            || self.private_originals.is_none()
            || self.local_recovery.is_some()
        {
            return Err(HostLifetimeError::Denied);
        }
        self.local_recovery = Some(policy);
        Ok(self)
    }

    /// Configuration visibility only; it is not readiness or operator entry.
    pub fn local_operator_recovery_selected(&self) -> bool {
        self.local_recovery.is_some()
    }

    /// Same-process local OS operator only. A configured remote issuer is not
    /// an alias for the local principal and cannot be selected by this API.
    pub fn local_service_recovery(
        &self,
    ) -> Result<Option<LocalServiceRecovery>, HostLifetimeError> {
        if self.local_recovery != Some(LocalOperatorRecoveryPolicy::UnpublishedServiceApprovals) {
            return Ok(None);
        }
        let local = self.local_operator_setup()?;
        let source = self
            .private_originals
            .clone()
            .ok_or(HostLifetimeError::Unavailable)?;
        let service = source.service_operator_setup(self.owner.clone())?;
        if local.recovery_operator() != service.recovery_operator() {
            return Err(HostLifetimeError::Denied);
        }
        let members = self
            .owner
            .selected_policy()
            .native_host
            .as_ref()
            .ok_or(HostLifetimeError::Unavailable)?
            .members
            .iter()
            .map(|row| row.member.clone())
            .collect();
        let view = LocalServiceRecovery {
            owner: self.owner.clone(),
            source,
            closed: self.private_closed.clone(),
            principal: local.recovery_operator().clone(),
            members,
        };
        view.check_current()?;
        Ok(Some(view))
    }
}
impl LocalServiceRecovery {
    fn check_current(&self) -> Result<(), HostLifetimeError> {
        if self.closed.is_closed() {
            return Err(HostLifetimeError::Denied);
        }
        self.owner.validate_current_selection()?;
        self.owner.validate_native_entry(None)?;
        let local = self.source.local_operator_setup(self.owner.clone())?;
        let service = self.source.service_operator_setup(self.owner.clone())?;
        let native = self
            .owner
            .selected_policy()
            .native_host
            .as_ref()
            .ok_or(HostLifetimeError::Unavailable)?;
        if local.recovery_operator() != &self.principal
            || service.recovery_operator() != &self.principal
            || !native
                .members
                .iter()
                .map(|row| &row.member)
                .eq(self.members.iter())
        {
            return Err(HostLifetimeError::Denied);
        }
        Ok(())
    }
    fn check_page(&self, page: &LocalServiceRecoveryPage) -> Result<(), HostLifetimeError> {
        self.check_current()?;
        if !Arc::ptr_eq(&self.source, &page.source) || page.read.human != self.principal {
            return Err(HostLifetimeError::WrongOwner);
        }
        page.read
            .check_visible(|member| self.members.contains(member))
            .map_err(|_| HostLifetimeError::Unavailable)
    }
    /// None starts at the beginning; Refresh/Next reuse only this owner's
    /// opaque position and obtain fresh canonical and host observations.
    pub async fn page(
        &self,
        previous: Option<(&LocalServiceRecoveryPage, LocalRecoveryPageRead)>,
    ) -> Result<LocalServiceRecoveryPage, HostLifetimeError> {
        self.check_current()?;
        let start = match previous {
            None => None,
            Some((previous, direction)) => {
                if !Arc::ptr_eq(&self.source, &previous.source)
                    || previous.read.human != self.principal
                {
                    return Err(HostLifetimeError::WrongOwner);
                }
                match direction {
                    LocalRecoveryPageRead::Refresh => previous.start.clone(),
                    LocalRecoveryPageRead::Next => {
                        Some(previous.next.clone().ok_or(HostLifetimeError::Denied)?)
                    }
                }
            }
        };
        let page = self
            .source
            .source_status_page_with_recovery(start.as_deref())
            .await
            .map_err(|_| HostLifetimeError::Unavailable)?;
        let read = self
            .source
            .read_service_recovery_page(&self.principal, &self.members, &page.rows)
            .await
            .map_err(|_| HostLifetimeError::Unavailable)?;
        let result = LocalServiceRecoveryPage {
            source: self.source.clone(),
            read,
            start,
            next: page.next_after,
            used: AtomicBool::new(false),
        };
        self.check_page(&result)?;
        Ok(result)
    }
    /// Explicit trusted operator decision on one retained row. The caller must
    /// stop a cancelled preparation before entry; no native work is spawned or
    /// awaits after mutation starts. A lost result never permits offer replay.
    pub async fn revoke(
        &self,
        page: &LocalServiceRecoveryPage,
        row_id: &str,
    ) -> Result<LocalServiceRecoveryResult, HostLifetimeError> {
        self.check_page(page)?;
        let row = page
            .read
            .rows
            .iter()
            .find(|row| row.id == row_id)
            .ok_or(HostLifetimeError::Denied)?;
        if !row.can_recover || page.used.swap(true, Ordering::AcqRel) {
            return Err(HostLifetimeError::Denied);
        }
        let prepared = self
            .source
            .prepare_service_recovery(&page.read, row)
            .await
            .map_err(|_| HostLifetimeError::Unavailable)?;
        self.check_page(page)?;
        // Do not reenter the installed lifetime from inside its reservation.
        let outcome = self.owner.with_current_setup(|| {
            if self.closed.is_closed() {
                return Err(HostLifetimeError::Denied);
            }
            page.read
                .check_visible(|member| self.members.contains(member))
                .map_err(|_| HostLifetimeError::Unavailable)?;
            prepared
                .revoke()
                .map_err(|_| HostLifetimeError::Unavailable)
        })?;
        Ok(LocalServiceRecoveryResult {
            outcome,
            policy_observed_at_offer: row.projection["policy"].clone(),
        })
    }
}

#[cfg(test)]
impl ToolkitGatewayHost {
    // Attach a facade to the fixture's already-running owners. This does not
    // qualify production gateway bootstrap or manufacture runtime custody.
    pub(crate) fn fixture_local_recovery(
        owner: Arc<NativeHostLifetime>,
        source: Arc<PrivateOriginals>,
        machine: Arc<MeerkatMachine>,
        closed: PrivateHostClosed,
        selected: bool,
    ) -> Self {
        let mut host = Self::new(owner, closed);
        host.private_originals = Some(source);
        if selected {
            host = host
                .with_local_operator_recovery(
                    LocalOperatorRecoveryPolicy::UnpublishedServiceApprovals,
                )
                .unwrap();
        }
        assert!(host.machine.set(machine).is_ok());
        host
    }
}
