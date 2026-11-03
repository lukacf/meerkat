//! Durable installed-host collector selection. A setup reference selects
//! historical enrollment; it never supplies an executable call context.
//!
//! Native grants still authorize every operation through Core's service-call
//! seam. No setup session or source cursor belongs in this record.
use super::*;
use crate::installed_host::operator_setup::CollectorSetupHost;
use meerkat::{ConnectorAuthPhase, ConnectorAuthStatus};
use meerkat_authorization_contracts::{
    grant::GrantLineageRef,
    work_association::{
        GrantTarget, InputAuthorityAssociation, InputAuthorityAssociationCandidate,
        TargetGrantLineage, WorkAuthorityBasis,
    },
};
use meerkat_core::{
    CredentialAccountId, CredentialAccountRef, McpServerConfig, ToolOperationTarget,
};
use std::collections::BTreeSet;

const SCHEMA: &str = "toolkit.collector-setup";
const VERSION: u32 = 1;
const PREFIX: &str = "connect-setup-";

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CollectorSetupError {
    #[error("setup_revoked")]
    SetupRevoked,
    #[error("installation_changed")]
    InstallationChanged,
    #[error("reauth_required")]
    ReauthRequired,
    #[error("principal_changed")]
    PrincipalChanged,
    #[error("collector_authority_denied")]
    Denied,
    #[error("collector_setup_unavailable")]
    Unavailable,
    #[error("collector_setup_pending_reconciliation")]
    PendingReconciliation,
}

/// Stable provider identity, copied only from the native verified account.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectorAccount {
    pub issuer: String,
    pub strategy_id: String,
    pub subject: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectorSource {
    pub mob_id: EvidenceId,
    pub source_id: EvidenceId,
    pub generation: u64,
}

/// Exact native installation facts captured for operator review. Construct this
/// from the selected own realm document and native slot status, not provider or
/// tool JSON. The full configuration stays with Core; only its digest persists.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectorInstallation {
    realm: RealmId,
    server: String,
    config_sha256: String,
    connector_approval: String,
    slot: CredentialAccountRef,
    account: CollectorAccount,
    operations: BTreeSet<String>,
}
impl CollectorInstallation {
    pub fn from_native(
        config: &McpServerConfig,
        status: &ConnectorAuthStatus,
        operations: BTreeSet<String>,
    ) -> Result<Self, CollectorSetupError> {
        let binding = config
            .connector
            .as_ref()
            .ok_or(CollectorSetupError::Denied)?;
        if operations.is_empty()
            || operations.len() > 100
            || operations.iter().any(|operation| {
                !binding
                    .operations
                    .get(operation)
                    .is_some_and(|value| value.read_only)
            })
        {
            return Err(CollectorSetupError::Denied);
        }
        meerkat::ConnectorMcpInstallation::from_config(config)
            .ok_or(CollectorSetupError::Denied)?
            .map_err(|_| CollectorSetupError::Denied)?;
        let realm = RealmId::parse(&binding.realm_id).map_err(|_| CollectorSetupError::Denied)?;
        let slot = CredentialAccountRef {
            realm: realm.clone(),
            account: CredentialAccountId::parse(&binding.slot_id)
                .map_err(|_| CollectorSetupError::Denied)?,
        };
        let account = CollectorAccount {
            issuer: binding.issuer.clone(),
            strategy_id: binding.strategy_id.clone(),
            subject: binding.account.clone(),
        };
        let value = Self {
            realm,
            server: config.name.clone(),
            config_sha256: config_digest(config)?,
            connector_approval: config.connector_approval().token().into(),
            slot,
            account,
            operations,
        };
        value.check_account(status)?;
        Ok(value)
    }
    pub fn account(&self) -> &CollectorAccount {
        &self.account
    }
    pub fn slot(&self) -> &CredentialAccountRef {
        &self.slot
    }
    pub fn operations(&self) -> &BTreeSet<String> {
        &self.operations
    }
    pub fn server(&self) -> &str {
        &self.server
    }

    pub(crate) fn select<'a>(
        &self,
        servers: &'a [McpServerConfig],
    ) -> Result<&'a McpServerConfig, CollectorSetupError> {
        let mut selected = servers.iter().filter(|server| server.name == self.server);
        let config = selected
            .next()
            .ok_or(CollectorSetupError::InstallationChanged)?;
        if selected.next().is_some()
            || config.connector_approval().token() != self.connector_approval
            || config_digest(config)? != self.config_sha256
        {
            return Err(CollectorSetupError::InstallationChanged);
        }
        Ok(config)
    }
    pub(crate) async fn check_current(
        &self,
        host: &CollectorSetupHost,
    ) -> Result<McpServerConfig, CollectorSetupError> {
        self.check_credential_realm(host.credential_realm())?;
        let servers = host.servers().await?;
        let config = self.select(&servers)?.clone();
        let status = host.account_status(&self.slot).await?;
        if Self::from_native(&config, &status, self.operations.clone())? != *self {
            return Err(CollectorSetupError::InstallationChanged);
        }
        Ok(config)
    }
    pub(crate) fn approval(&self) -> meerkat_core::mcp_config::McpConnectorApproval {
        meerkat_core::mcp_config::McpConnectorApproval::Connector(self.connector_approval.clone())
    }
    pub(crate) fn realm(&self) -> &RealmId {
        &self.realm
    }
    pub(crate) fn check_credential_realm(
        &self,
        realm: &RealmId,
    ) -> Result<(), CollectorSetupError> {
        if &self.realm != realm || &self.slot.realm != realm {
            return Err(CollectorSetupError::Denied);
        }
        Ok(())
    }
    fn check_account(&self, status: &ConnectorAuthStatus) -> Result<(), CollectorSetupError> {
        if status.slot != self.slot {
            return Err(CollectorSetupError::PrincipalChanged);
        }
        if status.phase != ConnectorAuthPhase::Authorized {
            return Err(CollectorSetupError::ReauthRequired);
        }
        let current = status
            .verified_account
            .as_ref()
            .ok_or(CollectorSetupError::ReauthRequired)?;
        if current.issuer != self.account.issuer
            || current.strategy_id != self.account.strategy_id
            || current.subject != self.account.subject
        {
            return Err(CollectorSetupError::PrincipalChanged);
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CollectorRecord {
    pub(crate) id: EvidenceId,
    pub(crate) installation: CollectorInstallation,
    pub(crate) source: CollectorSource,
    pub(crate) association: InputAuthorityAssociationCandidate,
    pub(crate) lineage: Vec<GrantLineageRef>,
    pub(crate) operator: PrincipalRef,
    pub(crate) revoked: bool,
    pub(crate) review: Option<meerkat_core::ApprovalRecord>,
}

impl CollectorRecord {
    pub(crate) fn check_operation(
        &self,
        target: &ToolOperationTarget,
    ) -> Result<(), CollectorSetupError> {
        let ToolOperationTarget::McpServerOperation {
            server,
            raw_operation,
            connector,
            account,
            ..
        } = target
        else {
            return Err(CollectorSetupError::Denied);
        };
        if server != &self.installation.server
            || !self.installation.operations.contains(raw_operation)
            || connector.token() != self.installation.connector_approval
        {
            return Err(CollectorSetupError::Denied);
        }
        check_operation_account(account, &self.installation.account)
    }

    pub(crate) fn review_body(&self) -> serde_json::Value {
        collector_review_body(
            &self.id,
            &self.installation,
            &self.source,
            &self.operator,
            &self.association.logical_executor,
            &self.association.target.audience,
            &self.association.admitted_ceiling,
        )
    }
}
pub(crate) fn collector_review_body(
    id: &EvidenceId,
    installation: &CollectorInstallation,
    source: &CollectorSource,
    operator: &PrincipalRef,
    collector: &PrincipalRef,
    audience: &meerkat_authorization_contracts::constraints::AudienceRef,
    restrictions: &meerkat_authorization_contracts::constraints::ExecutionRestrictions,
) -> serde_json::Value {
    serde_json::json!({"collector_enrollment": {
        "id": id, "installation": installation, "source": source,
        "operator": operator, "collector": collector, "audience": audience,
        "restrictions": restrictions,
    }})
}

/// A current selection, not native authorization. Re-resolve before each
/// collection; use Core's service-call owner for each dispatched operation.
/// This handle cannot turn a stored association into admitted/retained input.
pub struct ResolvedCollectorSetup {
    record: CollectorRecord,
    config: McpServerConfig,
}
impl ResolvedCollectorSetup {
    pub(crate) fn operations(&self) -> &BTreeSet<String> {
        &self.record.installation.operations
    }
    pub fn id(&self) -> &EvidenceId {
        &self.record.id
    }
    pub fn config(&self) -> &McpServerConfig {
        &self.config
    }
    pub fn slot(&self) -> &CredentialAccountRef {
        self.record.installation.slot()
    }
    pub fn account(&self) -> &CollectorAccount {
        self.record.installation.account()
    }
    pub fn source(&self) -> &CollectorSource {
        &self.record.source
    }
    pub fn association(&self) -> Result<InputAuthorityAssociation, CollectorSetupError> {
        InputAuthorityAssociation::new(self.record.association.clone())
            .map_err(|_| CollectorSetupError::Unavailable)
    }
    /// Compare the actual owner-produced MCP target. A matching name is never
    /// enough: the native connection's approval and account must also agree.
    /// This comparison supplies no dispatch authority or native entry permit.
    pub fn check_operation(&self, target: &ToolOperationTarget) -> Result<(), CollectorSetupError> {
        self.record.check_operation(target)
    }
}

impl PrivateOriginals {
    pub(crate) fn collector_for_policy(
        &self,
        association: &InputAuthorityAssociation,
    ) -> Result<Option<CollectorRecord>, meerkat_core::authorization::ControllerReadinessFailure>
    {
        self.owner.current_collector_record(association)
    }

    fn collector_store(&self) -> Result<(), CollectorSetupError> {
        self.owner
            .check_canonical_source(self.store.as_ref(), &self.scope)
            .map_err(|_| CollectorSetupError::Unavailable)?;
        if !self
            .owner
            .has_installed_pair_recovery()
            .map_err(|_| CollectorSetupError::Unavailable)?
        {
            return Err(CollectorSetupError::Unavailable);
        }
        self.store
            .capabilities()
            .require_durable_contract()
            .map_err(|_| CollectorSetupError::Unavailable)
    }
    pub(crate) fn collector_owner_matches(
        &self,
        owner: &Arc<ToolkitOriginalOwner>,
        store: &Arc<dyn ToolkitStore>,
    ) -> bool {
        Arc::ptr_eq(&self.owner, owner) && Arc::ptr_eq(&self.store, store)
    }

    pub(crate) async fn persist_collector_setup(
        &self,
        record: CollectorRecord,
    ) -> Result<EvidenceId, CollectorSetupError> {
        let _writer = self.owner.source_writer().await;
        self.collector_store()?;
        validate_record(&record, &self.scope, self.owner.ingress_domain())?;
        validate_review(&record)?;
        check_grant(&self.owner, &record)?;
        let key = collector_key(&self.scope, &record.id)?;
        let data = encode(&record)?;
        let outcome = self
            .store
            .admit(AdmissionRequest {
                key: key.clone(),
                admission_key: key.id.clone(),
                data,
                events: vec![],
                effects: vec![],
            })
            .await
            .map_err(store_error)?;
        let retained = decode(outcome.record(), &key)?;
        if retained != record {
            return Err(CollectorSetupError::Denied);
        }
        // Admission may have committed before its reply was delayed. Observe
        // the canonical row again: cancellation can already have revoked it
        // even when native grant cleanup is still unavailable.
        let current = self
            .store
            .get(&key)
            .await
            .map_err(store_error)?
            .ok_or(CollectorSetupError::Unavailable)?;
        let current = decode(&current, &key)?;
        if current.revoked {
            return Err(CollectorSetupError::SetupRevoked);
        }
        if current != retained {
            return Err(CollectorSetupError::Denied);
        }
        // Exact retries recover the same committed enrollment and id.
        check_grant(&self.owner, &current)?;
        let id = current.id.clone();
        self.owner.publish_collector_original(key, current)?;
        Ok(id)
    }

    /// Read this installed host's durable selection directly from its existing
    /// source store and Core's own MCP/credential owners. Document generation is
    /// intentionally not compared: an unrelated config edit is not a reinstall.
    pub async fn resolve_collector_setup(
        &self,
        id: &EvidenceId,
        host: &CollectorSetupHost,
    ) -> Result<ResolvedCollectorSetup, CollectorSetupError> {
        let _writer = self.owner.source_writer().await;
        self.collector_store()?;
        host.check_owner(&self.owner, &self.store)?;
        let key = collector_key(&self.scope, id)?;
        let stored = self
            .store
            .get(&key)
            .await
            .map_err(store_error)?
            .ok_or(CollectorSetupError::SetupRevoked)?;
        let record = decode(&stored, &key)?;
        validate_record(&record, &self.scope, self.owner.ingress_domain())?;
        if record.revoked {
            return Err(CollectorSetupError::SetupRevoked);
        }
        let config = record.installation.check_current(host).await?;
        check_grant(&self.owner, &record)?;
        // Re-read after awaited owner observations. Revocation or a replacement
        // during resolution cannot return the earlier snapshot as current.
        let current = self
            .store
            .get(&key)
            .await
            .map_err(store_error)?
            .ok_or(CollectorSetupError::SetupRevoked)?;
        if current != stored {
            return Err(CollectorSetupError::SetupRevoked);
        }
        self.collector_store()?;
        host.check_owner(&self.owner, &self.store)?;
        self.owner.publish_collector_original(key, record.clone())?;
        Ok(ResolvedCollectorSetup { record, config })
    }

    /// Repeat durable/config/account selection for an actual native operation.
    /// The caller must still use Core ServiceCall to obtain governed entry.
    pub async fn resolve_collector_operation(
        &self,
        id: &EvidenceId,
        host: &CollectorSetupHost,
        target: &ToolOperationTarget,
    ) -> Result<ResolvedCollectorSetup, CollectorSetupError> {
        let selection = self.resolve_collector_setup(id, host).await?;
        selection.check_operation(target)?;
        Ok(selection)
    }

    pub(crate) async fn collector_setup_not_revoked(
        &self,
        id: &EvidenceId,
    ) -> Result<(), CollectorSetupError> {
        self.collector_store()?;
        let key = collector_key(&self.scope, id)?;
        if let Some(saved) = self.store.get(&key).await.map_err(store_error)? {
            let record = decode(&saved, &key)?;
            if record.revoked {
                return Err(CollectorSetupError::SetupRevoked);
            }
        }
        Ok(())
    }

    pub(crate) async fn revoke_collector_setup(
        &self,
        id: &EvidenceId,
    ) -> Result<(), CollectorSetupError> {
        self.collector_store()?;
        let key = collector_key(&self.scope, id)?;
        self.owner.fence_collector_original(&key)?;
        // Cancellation and first publication compete for this same canonical
        // key. A tombstone has no invented grant or association. An overlapping
        // enrollment can neither replace it nor acknowledge an active result.
        for _ in 0..4 {
            let saved = self.store.get(&key).await.map_err(store_error)?;
            let result = if let Some(saved) = saved {
                let mut record = match decode(&saved, &key) {
                    Err(CollectorSetupError::SetupRevoked) => {
                        return self.publish_collector_revocation(&saved);
                    }
                    other => other?,
                };
                validate_record(&record, &self.scope, self.owner.ingress_domain())?;
                if record.revoked {
                    return self.publish_collector_revocation(&saved);
                }
                record.revoked = true;
                self.store
                    .compare_and_swap(Transition {
                        key: key.clone(),
                        expected_revision: saved.revision,
                        expected_digest: Some(saved.content_digest),
                        claim: None,
                        data: encode(&record)?,
                        events: vec![],
                        effects: vec![],
                    })
                    .await
                    .map(|_| ())
            } else {
                self.store
                    .admit(AdmissionRequest {
                        key: key.clone(),
                        admission_key: key.id.clone(),
                        data: RecordData {
                            schema: SCHEMA.into(),
                            version: 2,
                            body: serde_json::json!({"id": id, "revoked": true}),
                        },
                        events: vec![],
                        effects: vec![],
                    })
                    .await
                    .map(|_| ())
            };
            match result {
                Ok(()) => continue, // Read the actual current result before acknowledging.
                Err(
                    toolkit_store::StoreError::Conflict
                    | toolkit_store::StoreError::AdmissionConflict,
                ) => continue,
                Err(_) => return Err(CollectorSetupError::PendingReconciliation),
            }
        }
        Err(CollectorSetupError::PendingReconciliation)
    }

    fn publish_collector_revocation(
        &self,
        stored: &StoredRecord,
    ) -> Result<(), CollectorSetupError> {
        use meerkat_authorization_contracts::{
            evidence::{EvidenceDigest, HistoricalEvidenceRef},
            resource::ResourceRef,
        };
        // The caller has decoded the exact cancelled row or exact tombstone.
        // This is evidence of source cancellation, never native grant cleanup.
        let evidence = HistoricalEvidenceRef {
            resource: ResourceRef {
                domain: self.owner.ingress_domain().clone(),
                resource_id: stored.key.id.clone(),
            },
            revision: EvidenceId::new(stored.revision.to_string())
                .map_err(|_| CollectorSetupError::Unavailable)?,
            digest: EvidenceDigest::of_bytes(
                &canonical_json(stored).map_err(|_| CollectorSetupError::Unavailable)?,
            ),
        };
        self.owner
            .publish_collector_revocation(&stored.key, evidence)
    }
}

fn config_digest(config: &McpServerConfig) -> Result<String, CollectorSetupError> {
    canonical_json(config)
        .map(|bytes| digest(&bytes))
        .map_err(|_| CollectorSetupError::Unavailable)
}
fn collector_key(scope: &StoreScope, id: &EvidenceId) -> Result<StoreKey, CollectorSetupError> {
    if !id.as_str().starts_with(PREFIX) {
        return Err(CollectorSetupError::SetupRevoked);
    }
    Ok(StoreKey {
        realm: scope.realm.clone(),
        owner: scope.owner.clone(),
        id: id.as_str().into(),
    })
}
fn encode(record: &CollectorRecord) -> Result<RecordData, CollectorSetupError> {
    Ok(RecordData {
        schema: SCHEMA.into(),
        version: VERSION,
        body: serde_json::to_value(record).map_err(|_| CollectorSetupError::Unavailable)?,
    })
}
fn decode(stored: &StoredRecord, key: &StoreKey) -> Result<CollectorRecord, CollectorSetupError> {
    if &stored.key != key
        || stored.data.schema != SCHEMA
        || stored.data.digest().ok().as_deref() != Some(stored.content_digest.as_str())
    {
        return Err(CollectorSetupError::Unavailable);
    }
    if stored.data.version == 2
        && stored.data.body == serde_json::json!({"id": key.id, "revoked": true})
    {
        return Err(CollectorSetupError::SetupRevoked);
    }
    if stored.data.version != VERSION {
        return Err(CollectorSetupError::Unavailable);
    }
    let record: CollectorRecord = serde_json::from_value(stored.data.body.clone())
        .map_err(|_| CollectorSetupError::Unavailable)?;
    if record.id.as_str() != key.id {
        return Err(CollectorSetupError::Unavailable);
    }
    validate_review(&record).map_err(|_| CollectorSetupError::Unavailable)?;
    Ok(record)
}
fn validate_review(record: &CollectorRecord) -> Result<(), CollectorSetupError> {
    let review = record.review.as_ref().ok_or(CollectorSetupError::Denied)?;
    if review.status != meerkat_core::ApprovalStatus::Approved
        || !review
            .decision
            .as_ref()
            .is_some_and(|decision| decision.decision == meerkat_core::ApprovalDecision::Approve)
    {
        return Err(CollectorSetupError::Denied);
    }
    if review.proposed_action.body != Some(record.review_body()) {
        return Err(CollectorSetupError::Denied);
    }
    Ok(())
}
fn store_error(error: toolkit_store::StoreError) -> CollectorSetupError {
    match error {
        toolkit_store::StoreError::AdmissionConflict | toolkit_store::StoreError::Conflict => {
            CollectorSetupError::Denied
        }
        _ => CollectorSetupError::Unavailable,
    }
}

pub(crate) fn validate_record(
    record: &CollectorRecord,
    scope: &StoreScope,
    ingress: &ResourceDomain,
) -> Result<(), CollectorSetupError> {
    let c = &record.association;
    if record.id.as_str() != c.original_work.work.as_str()
        || record.source.generation == 0
        || c.ingress_namespace.realm.as_str() != scope.realm
        || record.installation.realm != record.installation.slot.realm
        || &c.ingress_namespace.ingress != ingress
        || ingress.namespace.starts_with("meerkat.")
        || c.requester != record.operator
        || c.ingress_actor != record.operator
        || c.original_work.authority != ingress.authority
        || c.target.logical_owner != ingress.authority
        || c.target.logical_runtime.as_str()
            != format!("meta-source/{}", record.source.mob_id.as_str())
        || c.target.context != record.source.source_id
        || c.target.context_generation != record.source.generation
        || c.represented_subject.is_some()
        || !c.contributing_work.is_empty()
        || c.controller_model.is_some()
        || !c.controller_grant_lineage.is_empty()
        || record.lineage.is_empty()
        || record.lineage[0].root_authority != record.operator
    {
        return Err(CollectorSetupError::Denied);
    }
    let expected = WorkAuthorityBasis::TargetedGrantLineages {
        base: None,
        targets: vec![TargetGrantLineage {
            target: GrantTarget::McpServer {
                server: record.installation.server.clone(),
            },
            lineage: record.lineage.clone(),
        }],
    };
    if c.authority_basis != expected {
        return Err(CollectorSetupError::Denied);
    }
    InputAuthorityAssociation::new(c.clone()).map_err(|_| CollectorSetupError::Denied)?;
    Ok(())
}

pub(crate) fn check_grant(
    owner: &ToolkitOriginalOwner,
    record: &CollectorRecord,
) -> Result<(), CollectorSetupError> {
    // Native resolve binds the entire root-to-leaf path to this grantee. This
    // cannot turn an agent grant into authority for a collector principal.
    for end in 1..record.lineage.len() {
        owner
            .grants()
            .resolve_lineage(
                &record.lineage[..end],
                &record.association.logical_executor,
                None,
            )
            .map_err(|error| match error {
                meerkat_authorization::grants::GrantRefusal::Denied => CollectorSetupError::Denied,
                _ => CollectorSetupError::Unavailable,
            })?;
    }
    let grant = owner
        .grants()
        .resolve_lineage(&record.lineage, &record.association.logical_executor, None)
        .map_err(|error| match error {
            meerkat_authorization::grants::GrantRefusal::Denied => CollectorSetupError::Denied,
            _ => CollectorSetupError::Unavailable,
        })?;
    let approval = meerkat_core::mcp_config::McpConnectorApproval::Connector(
        record.installation.connector_approval.clone(),
    );
    let expected = crate::installed_host::application_operations::mcp_access_restrictions(
        &owner.ingress_domain().authority,
        &record.association.logical_executor,
        &record.association.target.audience,
        &record.installation.server,
        record.installation.operations.iter().map(String::as_str),
        &approval,
    );
    // A grant's real time/delegation limits are preserved. Other dimensions
    // must exactly match this read-only installation and grantee.
    let mut observed = grant.restrictions().clone();
    observed.lifetime = expected.lifetime.clone();
    observed.delegation_depth = expected.delegation_depth.clone();
    if observed != expected {
        return Err(CollectorSetupError::Denied);
    }
    let now = owner
        .clock()
        .now()
        .map_err(|_| CollectorSetupError::Unavailable)?;
    if now.unix_ms >= grant.expires_at_ms() {
        return Err(CollectorSetupError::Denied);
    }
    grant
        .check_current()
        .map_err(|_| CollectorSetupError::Unavailable)
}

fn check_operation_account(
    account: &meerkat_core::OperationAccount,
    expected: &CollectorAccount,
) -> Result<(), CollectorSetupError> {
    match account {
        meerkat_core::OperationAccount::Selected(account)
            if account.authority() == expected.issuer && account.subject() == expected.subject =>
        {
            Ok(())
        }
        _ => Err(CollectorSetupError::PrincipalChanged),
    }
}

#[cfg(test)]
pub(crate) mod tests;
