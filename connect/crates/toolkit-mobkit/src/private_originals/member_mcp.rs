//! Resolve opaque member-access locators through the existing installed source
//! and policy owners. A locator or application roster row is never permission.
use super::*;
use crate::installed_host::{
    dynamic_members::{AdmittedMemberClass, MemberClassAdmissionPhase},
    operator_setup::member_mcp::{
        self, MemberMcpBindingRef, MemberMcpError, MemberMcpSetupHost, MemberMcpTarget,
    },
    tool_policy::service_policy::ServicePolicyOwner,
};
use meerkat_authorization_contracts::grant::GrantLineageRef;
use meerkat_mob::MemberRuntimeBuildIdentity;
use meerkat_runtime::meerkat_machine::SessionAgentMandate;
use meerkat_toolkit::host_lifetime::control::HostLifetimeError;
use std::collections::BTreeMap;

impl PrivateOriginals {
    /// Reconcile existing observed requirement records before registration.
    /// A void application row cannot hide an issued grant. Failed revocation
    /// remains cancelled with cleanup=None for the next startup attempt.
    pub(crate) async fn reconcile_observed_member_mcp(
        &self,
        lifetime: Arc<meerkat_toolkit::host_lifetime::NativeHostLifetime>,
    ) -> Result<(), MemberMcpError> {
        let mut after = None;
        loop {
            let rows = self
                .store
                .list_records(&self.scope, after.as_deref(), 256)
                .await
                .map_err(|_| MemberMcpError::Unavailable)?;
            if rows.is_empty() {
                return Ok(());
            }
            for saved in &rows {
                if saved.data.schema != member_mcp::SCHEMA {
                    continue;
                }
                let record = member_mcp::decode(saved)?;
                if let Some(binding) = record.observed_cleanup_binding() {
                    record.validate_retained_binding(binding, &self.owner)?;
                    // Construct only the already selected issuer when cleanup
                    // exists. Empty/ordinary startup gains no new prerequisite.
                    self.service_operator_setup(lifetime.clone())?
                        .revoke_member_mcp_binding(self, binding)
                        .await?;
                }
            }
            let last = rows.last().ok_or(MemberMcpError::Unavailable)?;
            if after
                .as_ref()
                .is_some_and(|prior: &String| prior >= &last.key.id)
            {
                return Err(MemberMcpError::Unavailable);
            }
            after = Some(last.key.id.clone());
        }
    }
    pub(crate) fn member_mcp_owner(
        &self,
        owner: &Arc<ToolkitOriginalOwner>,
        store: &Arc<dyn ToolkitStore>,
    ) -> Result<(), MemberMcpError> {
        if !Arc::ptr_eq(&self.owner, owner) || !Arc::ptr_eq(&self.store, store) {
            return Err(MemberMcpError::Denied);
        }
        if !self
            .owner
            .has_installed_pair_recovery()
            .map_err(|_| MemberMcpError::Unavailable)?
        {
            return Err(MemberMcpError::Unavailable);
        }
        self.store
            .capabilities()
            .require_durable_contract()
            .map_err(|_| MemberMcpError::Unavailable)
    }
    pub(crate) fn member_mcp_policy(&self) -> Result<&Arc<ServicePolicyOwner>, MemberMcpError> {
        self.service_tool_policy
            .get()
            .ok_or(MemberMcpError::Unavailable)
    }
    pub(crate) async fn member_mcp_target(
        &self,
        admitted: &AdmittedMemberClass,
    ) -> Result<MemberMcpTarget, MemberMcpError> {
        let policy = self.member_mcp_policy()?;
        if !Arc::ptr_eq(policy, &admitted.owner) {
            return Err(MemberMcpError::Denied);
        }
        let observed = policy
            .observe_member(&admitted.instance().member.member)
            .await?;
        observed.authorization?;
        if observed.admission.id != *admitted.admission_id()
            || observed.admission.class != *admitted.class()
            || observed.admission.instance != *admitted.instance()
            || observed.admission.phase != MemberClassAdmissionPhase::Reserved
        {
            return Err(MemberMcpError::Denied);
        }
        Ok(MemberMcpTarget {
            id: observed.admission.id,
            class: observed.admission.class,
            instance: observed.admission.instance,
        })
    }
    pub(crate) async fn member_mcp_check_target(
        &self,
        target: &MemberMcpTarget,
    ) -> Result<(), MemberMcpError> {
        self.member_mcp_policy()?
            .with_current_member(&target.id, &target.class, &target.instance, || Ok(()))
            .await
            .map_err(Into::into)
    }
    /// Resolve the complete explicitly retained set for one real registration.
    /// This issues nothing and does not bind Core. The registration hook uses
    /// this same read for selection and expected-mandate recovery.
    ///
    /// An empty set explicitly selects no configured-server mandate. Duplicate
    /// servers, different admissions or mixed represented subjects refuse; a
    /// later binding cannot overwrite an earlier server or widen its lineage.
    pub async fn resolve_member_mcp_bindings(
        &self,
        host: &MemberMcpSetupHost,
        bindings: &[MemberMcpBindingRef],
        identity: &MemberRuntimeBuildIdentity,
    ) -> Result<Option<SessionAgentMandate>, MemberMcpError> {
        host.check_owner(&self.owner, &self.store)?;
        self.member_mcp_owner(&self.owner, &self.store)?;
        if bindings.len() > member_mcp::MAX_BINDINGS {
            return Err(MemberMcpError::Denied);
        }
        let policy = self.member_mcp_policy()?;
        let destination = policy
            .member_destination(identity)?
            .ok_or(MemberMcpError::Denied)?;
        if bindings.is_empty() {
            return Ok(None);
        }
        let mut records = Vec::with_capacity(bindings.len());
        for binding in bindings {
            let key = member_mcp::key(&self.scope, binding)?;
            let saved = self
                .store
                .get(&key)
                .await
                .map_err(|_| MemberMcpError::Unavailable)?
                .ok_or(MemberMcpError::Denied)?;
            let record = member_mcp::decode(&saved)?;
            record.validate_binding(binding, &self.owner)?;
            if &record.target().instance.member.member != identity.member() {
                return Err(MemberMcpError::Denied);
            }
            self.member_mcp_check_target(record.target()).await?;
            record.recheck(host).await?;
            records.push((saved, record));
        }
        combine_member_mcp_records(&self.owner, &records)?;
        // Re-read all selected decisions after awaited account/config I/O.
        // Cancellation or a successor cannot return an earlier snapshot as live.
        for (saved, record) in &records {
            if self
                .store
                .get(&saved.key)
                .await
                .map_err(|_| MemberMcpError::Unavailable)?
                .as_ref()
                != Some(saved)
            {
                return Err(MemberMcpError::Cancelled);
            }
            self.member_mcp_check_target(record.target()).await?;
        }
        if policy.member_destination(identity)?.as_ref() != Some(&destination) {
            return Err(MemberMcpError::Denied);
        }
        host.check_owner(&self.owner, &self.store)?;
        // Native resolves every lineage again at operation entry. These reads
        // select registration facts; they are not a cached execution permit.
        combine_member_mcp_records(&self.owner, &records).map(Some)
    }
}

pub(crate) fn combine_member_mcp_records(
    source: &ToolkitOriginalOwner,
    records: &[(StoredRecord, member_mcp::Record)],
) -> Result<SessionAgentMandate, MemberMcpError> {
    if records.is_empty() || records.len() > member_mcp::MAX_BINDINGS {
        return Err(MemberMcpError::Denied);
    }
    let first = &records[0].1;
    let mut servers = BTreeMap::new();
    for (_, record) in records {
        if record.target() != first.target()
            || record.executor() != first.executor()
            || record.represented_subject() != first.represented_subject()
            || servers
                .insert(record.server().to_owned(), record.lineage(source)?)
                .is_some()
        {
            return Err(MemberMcpError::Denied);
        }
    }
    SessionAgentMandate::new(
        first.executor().clone(),
        servers,
        first.represented_subject().cloned(),
    )
    .map_err(|_| MemberMcpError::Denied)
}

/// Immutable source facts retained in the existing original input record.
/// This is not a live permission projection or a second grant owner.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MemberMcpSourceSelection {
    admission: crate::installed_host::member_mcp_consumer::MemberMcpAdmissionSelection,
    entries: Vec<MemberMcpSourceEntry>,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemberMcpSourceEntry {
    binding: MemberMcpBindingRef,
    server: String,
    lineage: Vec<GrantLineageRef>,
}
impl MemberMcpSourceSelection {
    pub(crate) fn targets(
        &self,
    ) -> Vec<meerkat_authorization_contracts::work_association::TargetGrantLineage> {
        use meerkat_authorization_contracts::work_association::{GrantTarget, TargetGrantLineage};
        self.entries
            .iter()
            .map(|entry| TargetGrantLineage {
                target: GrantTarget::McpServer {
                    server: entry.server.clone(),
                },
                lineage: entry.lineage.clone(),
            })
            .collect()
    }
}
impl PrivateOriginals {
    /// Must run before taking source_writer: lookup can enter the application's
    /// existing async row store/private pipe. Replays compare the original
    /// retained selection, never replace or upgrade its target authority.
    pub(crate) async fn select_member_mcp_source(
        &self,
        admission: &crate::installed_host::dynamic_members::MemberClassAdmission,
        saved: Option<&MemberMcpSourceSelection>,
    ) -> Result<Option<MemberMcpSourceSelection>, HostLifetimeError> {
        use crate::installed_host::member_mcp_consumer::MemberMcpAdmissionSelection;
        let application = self
            .member_mcp_application
            .get()
            .ok_or(HostLifetimeError::Unavailable)?;
        let selection = MemberMcpAdmissionSelection::from_admission(admission)?;
        let retained = application.lookup(selection.clone()).await?;
        retained.check(&selection)?;
        retained.reobserve_requirement(application.as_ref()).await?;
        let mut entries = Vec::with_capacity(retained.bindings.len());
        for binding in retained.bindings {
            let key =
                member_mcp::key(&self.scope, &binding).map_err(|_| HostLifetimeError::Denied)?;
            let stored = self
                .store
                .get(&key)
                .await
                .map_err(|_| HostLifetimeError::Unavailable)?
                .ok_or(HostLifetimeError::Denied)?;
            let record = member_mcp::decode(&stored).map_err(|_| HostLifetimeError::Unavailable)?;
            record
                .validate_retained_binding(&binding, &self.owner)
                .map_err(|_| HostLifetimeError::Denied)?;
            if record.target().id != admission.id
                || record.target().class != admission.class
                || record.target().instance != admission.instance
                || record.accepted_requirement() != retained.accepted_requirement.as_ref()
            {
                return Err(HostLifetimeError::Denied);
            }
            entries.push(MemberMcpSourceEntry {
                binding,
                server: record.server().to_owned(),
                lineage: record
                    .retained_lineage(&self.owner)
                    .map_err(|_| HostLifetimeError::Unavailable)?,
            });
        }
        entries.sort_by(|a, b| a.server.cmp(&b.server));
        if entries
            .windows(2)
            .any(|pair| pair[0].server >= pair[1].server)
        {
            return Err(HostLifetimeError::Denied);
        }
        let result = MemberMcpSourceSelection {
            admission: selection,
            entries,
        };
        if saved.is_some_and(|saved| saved != &result) {
            return Err(HostLifetimeError::Denied);
        }
        self.check_member_mcp_source(admission, &result)?;
        if let Some(expected) = &retained.accepted_requirement {
            expected.check_owner(application.as_ref())?;
        }
        Ok(Some(result))
    }
    /// Synchronous recheck under the caller's existing source writer. The
    /// async selector already authenticated decision/config/account facts from
    /// the existing immutable issued record. This verifies their native ids,
    /// not the permissions of all targets at once.
    pub(crate) fn check_member_mcp_source(
        &self,
        admission: &crate::installed_host::dynamic_members::MemberClassAdmission,
        selected: &MemberMcpSourceSelection,
    ) -> Result<(), HostLifetimeError> {
        use crate::installed_host::member_mcp_consumer::MemberMcpAdmissionSelection;
        let policy = self
            .service_tool_policy
            .get()
            .ok_or(HostLifetimeError::Unavailable)?;
        let current = policy
            .observed_admission(&admission.instance.member.member)?
            .ok_or(HostLifetimeError::Denied)?;
        if current.id != admission.id
            || current.class != admission.class
            || current.instance != admission.instance
            || selected.admission != MemberMcpAdmissionSelection::from_admission(&current)?
            || selected.entries.len() > member_mcp::MAX_BINDINGS
            || selected
                .entries
                .windows(2)
                .any(|pair| pair[0].server >= pair[1].server)
        {
            return Err(HostLifetimeError::Denied);
        }
        for entry in &selected.entries {
            let key = member_mcp::key(&self.scope, &entry.binding)
                .map_err(|_| HostLifetimeError::Denied)?;
            let native_id = EvidenceId::new(key.id).map_err(|_| HostLifetimeError::Denied)?;
            if entry.lineage.len() != 1
                || self
                    .owner
                    .grants()
                    .issued_reference(&native_id)
                    .map_err(|_| HostLifetimeError::Unavailable)?
                    .as_ref()
                    != entry.lineage.first()
            {
                return Err(HostLifetimeError::Denied);
            }
        }
        Ok(())
    }
}
