//! One protected operator review, native grant issuance and durable enrollment.
//! The existing installed source store retains the approved issuance intent
//! before its effect. Core owns the grant and every eventual service call.
use super::*;
use crate::private_originals::{
    PrivateOriginals,
    collector_setup::{
        CollectorInstallation, CollectorRecord, CollectorSetupError, CollectorSource,
        collector_review_body, validate_record,
    },
};
use meerkat::{ConnectorAuthStatus, HostAuthService};
use meerkat_authorization_contracts::{
    constraints::{AudienceRef, LifetimeRestriction},
    evidence::{EvidenceDigest, HistoricalEvidenceRef},
    protocol::ContractRequirements,
    resource::ResourceRef,
    work_association::{
        GrantTarget, InputAuthorityAssociation, InputAuthorityAssociationCandidate,
        NativeWorkTarget, OriginalWorkRef, QualifiedIngressNamespace, TargetGrantLineage,
        WorkAuthorityBasis,
    },
};
use meerkat_core::{CredentialAccountRef, McpServerConfig, PrincipalRef};
use meerkat_mobkit::rpc_gateway::{GatewayFactoryConstruction, GatewayRealmMcpInstallation};
use std::sync::Weak;

const DECISION_SCHEMA: &str = "toolkit.operator-collector-grant";

/// Exact native factory handles retained by trusted installed-host composition.
/// Construct inside that host's real factory callback. No JSON constructor,
/// copied configuration, environment credential or secondary realm is accepted.
pub struct CollectorSetupHost {
    installation: GatewayRealmMcpInstallation,
    auth: HostAuthService,
    source: Weak<ToolkitOriginalOwner>,
    lifetime: Weak<NativeHostLifetime>,
    store: Weak<dyn ToolkitStore>,
}
impl CollectorSetupHost {
    pub(crate) fn collector_router(
        &self,
        launch: meerkat_mcp::McpStdioLaunchProfile,
        egress: meerkat_mcp::HttpEgressProfile,
    ) -> Result<meerkat_mcp::McpRouter, CollectorSetupError> {
        self.current()?;
        let resolver = self
            .auth
            .mcp_oauth_authority()
            .map_err(|_| CollectorSetupError::Unavailable)?;
        let provider = meerkat::ConnectorMcpCallContextProvider::live(
            self.auth
                .connector_oauth_authority()
                .map_err(|_| CollectorSetupError::Unavailable)?,
        );
        self.current()?;
        Ok(meerkat::mcp::standalone_router_with_stdio_profile(launch)
            .with_surface_ownership(meerkat_mcp::McpSurfaceOwnership::Owned)
            .with_http_egress_profile(egress)
            .with_mcp_auth(meerkat::McpAuthMode::Interactive, Some(Arc::new(resolver)))
            .with_call_context_provider(Arc::new(provider)))
    }

    pub(crate) fn credential_realm(&self) -> &meerkat_core::RealmId {
        self.installation.realm()
    }
    fn current(&self) -> Result<(), CollectorSetupError> {
        let lifetime = self
            .lifetime
            .upgrade()
            .ok_or(CollectorSetupError::Unavailable)?;
        lifetime
            .validate_current_selection()
            .map_err(|_| CollectorSetupError::Unavailable)
    }
    pub(crate) fn check_owner(
        &self,
        source: &Arc<ToolkitOriginalOwner>,
        store: &Arc<dyn ToolkitStore>,
    ) -> Result<(), CollectorSetupError> {
        self.current()?;
        if !self
            .source
            .upgrade()
            .is_some_and(|value| Arc::ptr_eq(&value, source))
            || !self
                .store
                .upgrade()
                .is_some_and(|value| Arc::ptr_eq(&value, store))
        {
            return Err(CollectorSetupError::Denied);
        }
        Ok(())
    }
    pub(crate) async fn servers(&self) -> Result<Vec<McpServerConfig>, CollectorSetupError> {
        self.current()?;
        let (servers, _) = self
            .installation
            .servers()
            .await
            .map_err(|_| CollectorSetupError::Unavailable)?;
        self.current()?;
        Ok(servers)
    }
    pub(crate) async fn account_status(
        &self,
        slot: &CredentialAccountRef,
    ) -> Result<ConnectorAuthStatus, CollectorSetupError> {
        self.current()?;
        if &slot.realm != self.installation.realm() {
            return Err(CollectorSetupError::Denied);
        }
        let status = self
            .auth
            .connector_status(slot)
            .await
            .map_err(|_| CollectorSetupError::Unavailable)?;
        self.current()?;
        Ok(status)
    }
    /// Capture the actual own installation and verified account for one review.
    pub async fn select_installation(
        &self,
        server: &str,
        operations: std::collections::BTreeSet<String>,
    ) -> Result<CollectorInstallation, CollectorSetupError> {
        let servers = self.servers().await?;
        let mut matches = servers.iter().filter(|value| value.name == server);
        let config = matches
            .next()
            .ok_or(CollectorSetupError::InstallationChanged)?;
        if matches.next().is_some() {
            return Err(CollectorSetupError::InstallationChanged);
        }
        let binding = config
            .connector
            .as_ref()
            .ok_or(CollectorSetupError::Denied)?;
        let slot = CredentialAccountRef {
            realm: meerkat_core::RealmId::parse(&binding.realm_id)
                .map_err(|_| CollectorSetupError::Denied)?,
            account: meerkat_core::CredentialAccountId::parse(&binding.slot_id)
                .map_err(|_| CollectorSetupError::Denied)?,
        };
        let status = self.account_status(&slot).await?;
        let selected = CollectorInstallation::from_native(config, &status, operations)?;
        selected.check_credential_realm(self.installation.realm())?;
        Ok(selected)
    }
}

/// A principal selected by trusted Rust host composition, not an agent request.
/// A collector can be a service principal with no native member or session.
/// This bound selection grants nothing until the protected review is approved.
pub struct InstalledCollectorPrincipal {
    principal: PrincipalRef,
    audience: AudienceRef,
    source: Weak<ToolkitOriginalOwner>,
    lifetime: Weak<NativeHostLifetime>,
}

/// Caller-retainable review and exact recovery locator. Cloning does not issue
/// permission. The reviewed request is never decoded from a presenter response.
#[derive(Clone)]
pub struct CollectorEnrollmentReview {
    intent: CollectorIntent,
    request: ApprovalRequest,
    source: Weak<ToolkitOriginalOwner>,
    lifetime: Weak<NativeHostLifetime>,
}
impl CollectorEnrollmentReview {
    /// Persist this locator before submitting approval. It is not a credential;
    /// after a lost acknowledgement the same operator can resume this exact id.
    pub fn id(&self) -> &EvidenceId {
        &self.intent.id
    }
    pub fn request(&self) -> &ApprovalRequest {
        &self.request
    }
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CollectorIntent {
    id: EvidenceId,
    installation: CollectorInstallation,
    source: CollectorSource,
    operator: PrincipalRef,
    collector: PrincipalRef,
    audience: AudienceRef,
    restrictions: ExecutionRestrictions,
}
impl CollectorIntent {
    fn review_body(&self) -> serde_json::Value {
        collector_review_body(
            &self.id,
            &self.installation,
            &self.source,
            &self.operator,
            &self.collector,
            &self.audience,
            &self.restrictions,
        )
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CollectorDecision {
    cancelled: bool,
    cleanup: Option<CollectorGrantCleanup>,
    intent: CollectorIntent,
    approval: ApprovalRecord,
    issued: ParentIssuance,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
enum CollectorGrantCleanup {
    NotIssued,
    Revoked { lineage: Vec<GrantLineageRef> },
}

impl OperatorSetup {
    /// Called on the same installed host's actual native factory offer. The
    /// offer supplies both account and configuration handles from that factory.
    pub fn collector_setup_host(
        &self,
        construction: &GatewayFactoryConstruction<'_>,
    ) -> Result<CollectorSetupHost, CollectorSetupError> {
        self.collector_current()?;
        // Installation credentials and canonical source/grant authority have
        // separate realms. Retain the actual factory handles; account reads
        // below remain fenced to that factory's installation realm.
        Ok(CollectorSetupHost {
            installation: construction
                .realm_mcp_installation()
                .map_err(|_| CollectorSetupError::Unavailable)?,
            auth: construction
                .host_auth_service()
                .map_err(|_| CollectorSetupError::Unavailable)?,
            source: Arc::downgrade(&self.source),
            lifetime: Arc::downgrade(&self.lifetime),
            store: Arc::downgrade(&self.originals),
        })
    }
    /// Trusted application composition selects the exact collector and audience.
    /// Do not expose this constructor as a model/private-pipe JSON operation.
    pub fn bind_collector_principal(
        &self,
        principal: PrincipalRef,
        audience: AudienceRef,
    ) -> Result<InstalledCollectorPrincipal, CollectorSetupError> {
        self.collector_current()?;
        principal
            .validate_qualified()
            .map_err(|_| CollectorSetupError::Denied)?;
        Ok(InstalledCollectorPrincipal {
            principal,
            audience,
            source: Arc::downgrade(&self.source),
            lifetime: Arc::downgrade(&self.lifetime),
        })
    }
    pub fn prepare_collector_enrollment(
        &self,
        collector: &InstalledCollectorPrincipal,
        installation: CollectorInstallation,
        source: CollectorSource,
        lifetime: LifetimeRestriction,
        mut request: ApprovalRequest,
    ) -> Result<CollectorEnrollmentReview, CollectorSetupError> {
        self.collector_current()?;
        self.check_binding(&collector.source, &collector.lifetime)?;
        if installation.realm() != &installation.slot().realm || source.generation == 0 {
            return Err(CollectorSetupError::Denied);
        }
        let mut restrictions =
            crate::installed_host::application_operations::mcp_access_restrictions(
                &self.source.ingress_domain().authority,
                &collector.principal,
                &collector.audience,
                installation.server(),
                installation.operations().iter().map(String::as_str),
                &installation.approval(),
            );
        restrictions.lifetime = lifetime;
        let intent = CollectorIntent {
            id: new_setup_id()?,
            installation,
            source,
            operator: self.operator.clone(),
            collector: collector.principal.clone(),
            audience: collector.audience.clone(),
            restrictions,
        };
        request.proposed_action.summary =
            "Authorize this collector's read-only connection and enroll its source".into();
        request.proposed_action.body = Some(intent.review_body());
        Ok(CollectorEnrollmentReview {
            intent,
            request,
            source: Arc::downgrade(&self.source),
            lifetime: Arc::downgrade(&self.lifetime),
        })
    }
    /// One protected approval authorizes this exact root issuance and enrollment.
    /// Keep `review.id()` before calling. Cancellation or a lost store response
    /// is reconciled by that id on this owner, never by issuing another grant.
    pub async fn finalize_collector_enrollment<F: Fn() -> bool + Sync>(
        &self,
        originals: &PrivateOriginals,
        host: &CollectorSetupHost,
        review: &CollectorEnrollmentReview,
        approval: ApprovalRecord,
        presented: &F,
    ) -> Result<EvidenceId, CollectorSetupError> {
        self.collector_current()?;
        self.check_binding(&review.source, &review.lifetime)?;
        self.check_collector_inputs(originals, host)?;
        if !same_request(&review.request, &approval) || !approved(&approval) || !presented() {
            return Err(CollectorSetupError::Denied);
        }
        let decision = CollectorDecision {
            cancelled: false,
            cleanup: None,
            intent: review.intent.clone(),
            approval,
            issued: ParentIssuance::Pending,
        };
        let saved = self.retain_collector_decision(&decision).await?;
        self.finish_collector_decision(originals, host, saved, presented)
            .await
    }
    /// Resume only an already retained approval under current operator custody.
    /// No setup session, new approval, new grant id or synthesized lineage is used.
    pub async fn resume_collector_enrollment<F: Fn() -> bool + Sync>(
        &self,
        originals: &PrivateOriginals,
        host: &CollectorSetupHost,
        id: &EvidenceId,
        presented: &F,
    ) -> Result<EvidenceId, CollectorSetupError> {
        self.collector_current()?;
        self.check_collector_inputs(originals, host)?;
        if !presented() {
            return Err(CollectorSetupError::Denied);
        }
        let key = self.collector_decision_key(id)?;
        let saved = self
            .originals
            .get(&key)
            .await
            .map_err(|_| CollectorSetupError::Unavailable)?
            .ok_or(CollectorSetupError::Unavailable)?;
        self.finish_collector_decision(originals, host, saved, presented)
            .await
    }
    async fn retain_collector_decision(
        &self,
        proposed: &CollectorDecision,
    ) -> Result<StoredRecord, CollectorSetupError> {
        let key = self.collector_decision_key(&proposed.intent.id)?;
        if let Some(saved) = self
            .originals
            .get(&key)
            .await
            .map_err(|_| CollectorSetupError::Unavailable)?
        {
            let existing = self.decode_collector_decision(&saved)?;
            if existing.intent != proposed.intent || existing.approval != proposed.approval {
                return Err(CollectorSetupError::Denied);
            }
            return Ok(saved);
        }
        if approval_expired(&proposed.approval) {
            return Err(CollectorSetupError::Denied);
        }
        self.collector_current()?;
        let saved = self
            .originals
            .admit(AdmissionRequest {
                key: key.clone(),
                admission_key: key.id,
                data: encode_decision(proposed)?,
                events: vec![],
                effects: vec![],
            })
            .await
            .map_err(|_| CollectorSetupError::PendingReconciliation)?;
        let actual = self.decode_collector_decision(saved.record())?;
        if actual.intent != proposed.intent || actual.approval != proposed.approval {
            return Err(CollectorSetupError::Denied);
        }
        Ok(saved.record().clone())
    }
    async fn finish_collector_decision<F: Fn() -> bool + Sync>(
        &self,
        originals: &PrivateOriginals,
        host: &CollectorSetupHost,
        saved: StoredRecord,
        presented: &F,
    ) -> Result<EvidenceId, CollectorSetupError> {
        let decision = self.decode_collector_decision(&saved)?;
        if decision.cancelled {
            return Err(CollectorSetupError::SetupRevoked);
        }
        self.check_collector_inputs(originals, host)?;
        originals
            .collector_setup_not_revoked(&decision.intent.id)
            .await?;
        decision.intent.installation.check_current(host).await?;
        let saved = self
            .issue_collector_decision(originals, saved, presented)
            .await?;
        let decision = self.decode_collector_decision(&saved)?;
        // Native grant issuance and config/account reads are separate owners.
        // Recheck after issuance before publishing the durable setup selection.
        decision.intent.installation.check_current(host).await?;
        self.check_collector_inputs(originals, host)?;
        if !presented() {
            return Err(CollectorSetupError::Denied);
        }
        let record = self.collector_record(&saved, &decision)?;
        originals.persist_collector_setup(record).await
    }
    // Reuse the existing operator issuance states and the native owner's exact
    // issued-reference lookup. An unavailable lookup never becomes absence.
    async fn issue_collector_decision<F: Fn() -> bool + Sync>(
        &self,
        originals: &PrivateOriginals,
        saved: StoredRecord,
        presented: &F,
    ) -> Result<StoredRecord, CollectorSetupError> {
        // Share the existing source writer with cancellation. Re-read after
        // acquiring it so a retained stale Pending value cannot issue late.
        let _writer = self.source.source_writer().await;
        if !originals.collector_owner_matches(&self.source, &self.originals) {
            return Err(CollectorSetupError::Denied);
        }
        let prior = self.decode_collector_decision(&saved)?;
        let saved = self
            .originals
            .get(&saved.key)
            .await
            .map_err(|_| CollectorSetupError::Unavailable)?
            .ok_or(CollectorSetupError::Unavailable)?;
        let mut decision = self.decode_collector_decision(&saved)?;
        if decision.intent != prior.intent || decision.approval != prior.approval {
            return Err(CollectorSetupError::Denied);
        }
        if decision.cancelled {
            return Err(CollectorSetupError::SetupRevoked);
        }
        // A caller can cancel using the prepared review id before its approval
        // decision is persisted. The canonical setup tombstone fences that case.
        originals
            .collector_setup_not_revoked(&decision.intent.id)
            .await?;
        if matches!(
            decision.issued,
            ParentIssuance::Denied | ParentIssuance::NativeRefused
        ) {
            return Err(CollectorSetupError::Denied);
        }
        let id =
            EvidenceId::new(saved.key.id.clone()).map_err(|_| CollectorSetupError::Unavailable)?;
        let issued = self
            .lifetime
            .with_current_setup(|| {
                if !presented() {
                    return Err(HostLifetimeError::Denied);
                }
                let grants = self.source.grants();
                match grants.issued_reference(&id) {
                    Ok(Some(reference)) => Ok(Ok(reference)),
                    Ok(None) if matches!(decision.issued, ParentIssuance::Pending) => {
                        if approval_expired(&decision.approval) {
                            return Err(HostLifetimeError::Denied);
                        }
                        Ok(grants.issue_root(
                            &self.operator,
                            id,
                            decision.intent.collector.clone(),
                            None,
                            decision.intent.restrictions.clone(),
                        ))
                    }
                    Ok(None) => Err(HostLifetimeError::Unavailable),
                    Err(error) => Ok(Err(error)),
                }
            })
            .map_err(|error| match error {
                HostLifetimeError::Denied => CollectorSetupError::Denied,
                _ => CollectorSetupError::Unavailable,
            })?;
        let reference = match issued {
            Ok(value) => value,
            Err(meerkat_authorization::grants::GrantRefusal::Denied) => {
                return Err(CollectorSetupError::Denied);
            }
            Err(_) => return Err(CollectorSetupError::PendingReconciliation),
        };
        if reference.root_authority != self.operator {
            return Err(CollectorSetupError::Denied);
        }
        let lineage = vec![reference];
        let grant = self
            .source
            .grants()
            .resolve_lineage(&lineage, &decision.intent.collector, None)
            .map_err(|error| match error {
                meerkat_authorization::grants::GrantRefusal::Denied => CollectorSetupError::Denied,
                _ => CollectorSetupError::Unavailable,
            })?;
        if grant.restrictions() != &decision.intent.restrictions {
            return Err(CollectorSetupError::Denied);
        }
        grant
            .check_current()
            .map_err(|_| CollectorSetupError::Unavailable)?;
        if let ParentIssuance::Issued { lineage: existing } = &decision.issued {
            return if existing == &lineage {
                Ok(saved)
            } else {
                Err(CollectorSetupError::Denied)
            };
        }
        decision.issued = ParentIssuance::Issued { lineage };
        match self
            .originals
            .compare_and_swap(Transition {
                key: saved.key.clone(),
                expected_revision: saved.revision,
                expected_digest: Some(saved.content_digest.clone()),
                claim: None,
                data: encode_decision(&decision)?,
                events: vec![],
                effects: vec![],
            })
            .await
        {
            Ok(value) => Ok(value),
            Err(_) => Err(CollectorSetupError::PendingReconciliation),
        }
    }
    fn collector_record(
        &self,
        saved: &StoredRecord,
        decision: &CollectorDecision,
    ) -> Result<CollectorRecord, CollectorSetupError> {
        let ParentIssuance::Issued { lineage } = &decision.issued else {
            return Err(CollectorSetupError::PendingReconciliation);
        };
        let i = &decision.intent;
        let bytes =
            toolkit_package::canonical_json(saved).map_err(|_| CollectorSetupError::Unavailable)?;
        let evidence = HistoricalEvidenceRef {
            resource: ResourceRef {
                domain: self.source.ingress_domain().clone(),
                resource_id: saved.key.id.clone(),
            },
            revision: EvidenceId::new(saved.revision.to_string())
                .map_err(|_| CollectorSetupError::Unavailable)?,
            digest: EvidenceDigest::of_bytes(&bytes),
        };
        let association = InputAuthorityAssociationCandidate {
            requester: i.operator.clone(),
            ingress_actor: i.operator.clone(),
            represented_subject: None,
            original_authentication: evidence.clone(),
            logical_executor: i.collector.clone(),
            target: NativeWorkTarget {
                logical_owner: self.source.ingress_domain().authority.clone(),
                logical_runtime: EvidenceId::new(format!(
                    "meta-source/{}",
                    i.source.mob_id.as_str()
                ))
                .map_err(|_| CollectorSetupError::Denied)?,
                context: i.source.source_id.clone(),
                context_generation: i.source.generation,
                audience: i.audience.clone(),
            },
            original_work: OriginalWorkRef {
                authority: self.source.ingress_domain().authority.clone(),
                work: i.id.clone(),
            },
            root_event: evidence,
            contributing_work: vec![],
            authority_basis: WorkAuthorityBasis::TargetedGrantLineages {
                base: None,
                targets: vec![TargetGrantLineage {
                    target: GrantTarget::McpServer {
                        server: i.installation.server().into(),
                    },
                    lineage: lineage.clone(),
                }],
            },
            controller_grant_lineage: vec![],
            controller_model: None,
            controller_ceiling: i.restrictions.clone(),
            admitted_ceiling: i.restrictions.clone(),
            source_observations: vec![],
            ingress_namespace: QualifiedIngressNamespace {
                realm: self.native.realm.clone(),
                ingress: self.source.ingress_domain().clone(),
                occurrence_scope: i.id.clone(),
            },
            contract: ContractRequirements::local_governed_v1(),
        };
        InputAuthorityAssociation::new(association.clone())
            .map_err(|_| CollectorSetupError::Denied)?;
        let record = CollectorRecord {
            id: i.id.clone(),
            installation: i.installation.clone(),
            source: i.source.clone(),
            association,
            lineage: lineage.clone(),
            operator: i.operator.clone(),
            revoked: false,
            review: Some(decision.approval.clone()),
        };
        validate_record(&record, &saved.key.scope(), self.source.ingress_domain())?;
        Ok(record)
    }
    fn collector_current(&self) -> Result<(), CollectorSetupError> {
        self.lifetime
            .validate_current_selection()
            .map_err(|_| CollectorSetupError::Unavailable)?;
        if !self
            .source
            .has_installed_pair_recovery()
            .map_err(|_| CollectorSetupError::Unavailable)?
        {
            return Err(CollectorSetupError::Unavailable);
        }
        self.originals
            .capabilities()
            .require_durable_contract()
            .map_err(|_| CollectorSetupError::Unavailable)
    }
    fn check_binding(
        &self,
        source: &Weak<ToolkitOriginalOwner>,
        lifetime: &Weak<NativeHostLifetime>,
    ) -> Result<(), CollectorSetupError> {
        if !source
            .upgrade()
            .is_some_and(|value| Arc::ptr_eq(&value, &self.source))
            || !lifetime
                .upgrade()
                .is_some_and(|value| Arc::ptr_eq(&value, &self.lifetime))
        {
            return Err(CollectorSetupError::Denied);
        }
        Ok(())
    }
    fn check_collector_inputs(
        &self,
        originals: &PrivateOriginals,
        host: &CollectorSetupHost,
    ) -> Result<(), CollectorSetupError> {
        self.collector_current()?;
        self.check_binding(&host.source, &host.lifetime)?;
        host.check_owner(&self.source, &self.originals)?;
        if !originals.collector_owner_matches(&self.source, &self.originals) {
            return Err(CollectorSetupError::Denied);
        }
        Ok(())
    }
    fn collector_decision_key(&self, id: &EvidenceId) -> Result<StoreKey, CollectorSetupError> {
        let suffix = id
            .as_str()
            .strip_prefix("connect-setup-")
            .ok_or(CollectorSetupError::Denied)?;
        if suffix.is_empty() {
            return Err(CollectorSetupError::Denied);
        }
        Ok(StoreKey {
            realm: self.native.realm.to_string(),
            owner: self.lifetime.selected_policy().deployment.to_string(),
            id: format!("collector-grant-{suffix}"),
        })
    }
    fn decode_collector_decision(
        &self,
        saved: &StoredRecord,
    ) -> Result<CollectorDecision, CollectorSetupError> {
        if saved.data.schema != DECISION_SCHEMA
            || saved.data.version != 1
            || saved.data.digest().ok().as_deref() != Some(saved.content_digest.as_str())
        {
            return Err(CollectorSetupError::Unavailable);
        }
        let decision: CollectorDecision = serde_json::from_value(saved.data.body.clone())
            .map_err(|_| CollectorSetupError::Unavailable)?;
        if self.collector_decision_key(&decision.intent.id)? != saved.key
            || decision.intent.operator != self.operator
            || decision.intent.installation.realm() != &decision.intent.installation.slot().realm
            || decision.intent.source.generation == 0
            || !approved(&decision.approval)
            || decision.approval.proposed_action.body != Some(decision.intent.review_body())
        {
            return Err(CollectorSetupError::Denied);
        }
        Ok(decision)
    }
    /// Revoke only this enrollment, never rotate a shared provider credential.
    pub async fn revoke_collector_enrollment(
        &self,
        originals: &PrivateOriginals,
        id: &EvidenceId,
    ) -> Result<(), CollectorSetupError> {
        self.collector_current()?;
        if !originals.collector_owner_matches(&self.source, &self.originals) {
            return Err(CollectorSetupError::Denied);
        }
        let _writer = self.source.source_writer().await;
        let key = self.collector_decision_key(id)?;
        let saved = self
            .originals
            .get(&key)
            .await
            .map_err(|_| CollectorSetupError::Unavailable)?;
        // The prepared id is already a cancellation locator, even before the
        // protected approval has been persisted. Claim the existing setup key
        // without fabricating an approval, grant or original association.
        originals.revoke_collector_setup(id).await?;
        let Some(mut saved) = saved else {
            let native_id =
                EvidenceId::new(&key.id).map_err(|_| CollectorSetupError::Unavailable)?;
            return match self.source.grants().issued_reference(&native_id) {
                Ok(None) => Ok(()),
                // No approved intent exists to establish ownership of a present
                // native grant. Keep cancellation sticky but do not guess its
                // lineage or claim that native cleanup has completed.
                Ok(Some(_)) | Err(_) => Err(CollectorSetupError::PendingReconciliation),
            };
        };
        let mut decision = self.decode_collector_decision(&saved)?;
        if !decision.cancelled {
            decision.cancelled = true;
            saved = self.save_collector_decision(&saved, &decision).await?;
        }
        if decision.cleanup.is_some() {
            return Ok(());
        }
        // The same writer excludes late issuance. Native remains the authority
        // for whether this exact id was issued and whether revocation settled.
        let grants = self.source.grants();
        let native_id = EvidenceId::new(&key.id).map_err(|_| CollectorSetupError::Unavailable)?;
        let issued = grants
            .issued_reference(&native_id)
            .map_err(|_| CollectorSetupError::PendingReconciliation)?;
        decision.cleanup = Some(match issued {
            None => CollectorGrantCleanup::NotIssued,
            Some(reference) => {
                let lineage = vec![reference];
                if let ParentIssuance::Issued { lineage: prior } = &decision.issued {
                    if prior != &lineage {
                        return Err(CollectorSetupError::Denied);
                    }
                } else {
                    // Preserve the exact native issuance before attempting its
                    // cleanup, including a native-commit/Toolkit-ack gap.
                    let grant = grants
                        .resolve_lineage(&lineage, &decision.intent.collector, None)
                        .map_err(|_| CollectorSetupError::PendingReconciliation)?;
                    if grant.restrictions() != &decision.intent.restrictions {
                        return Err(CollectorSetupError::Denied);
                    }
                    decision.issued = ParentIssuance::Issued {
                        lineage: lineage.clone(),
                    };
                    saved = self.save_collector_decision(&saved, &decision).await?;
                }
                let machine = originals
                    .machine()
                    .map_err(|_| CollectorSetupError::PendingReconciliation)?;
                let mut custody = machine
                    .try_controller_grant_mutation()
                    .map_err(|_| CollectorSetupError::PendingReconciliation)?;
                self.lifetime
                    .with_current_setup(|| {
                        grants
                            .revoke(&self.operator, &lineage[0], &mut custody)
                            .map_err(|_| HostLifetimeError::Unavailable)
                    })
                    .map_err(|_| CollectorSetupError::PendingReconciliation)?;
                CollectorGrantCleanup::Revoked { lineage }
            }
        });
        self.save_collector_decision(&saved, &decision).await?;
        Ok(())
    }
    async fn save_collector_decision(
        &self,
        saved: &StoredRecord,
        decision: &CollectorDecision,
    ) -> Result<StoredRecord, CollectorSetupError> {
        self.originals
            .compare_and_swap(Transition {
                key: saved.key.clone(),
                expected_revision: saved.revision,
                expected_digest: Some(saved.content_digest.clone()),
                claim: None,
                data: encode_decision(decision)?,
                events: vec![],
                effects: vec![],
            })
            .await
            .map_err(|_| CollectorSetupError::PendingReconciliation)
    }
}
fn approved(value: &ApprovalRecord) -> bool {
    value.status == ApprovalStatus::Approved
        && value
            .decision
            .as_ref()
            .is_some_and(|decision| decision.decision == ApprovalDecision::Approve)
}
fn approval_expired(value: &ApprovalRecord) -> bool {
    value
        .expires_at
        .is_some_and(|expiry| std::time::SystemTime::from(expiry) <= std::time::SystemTime::now())
}
fn encode_decision(decision: &CollectorDecision) -> Result<RecordData, CollectorSetupError> {
    Ok(RecordData {
        schema: DECISION_SCHEMA.into(),
        version: 1,
        body: serde_json::to_value(decision).map_err(|_| CollectorSetupError::Unavailable)?,
    })
}
fn new_setup_id() -> Result<EvidenceId, CollectorSetupError> {
    use std::io::Read;
    let mut entropy = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(&mut entropy))
        .map_err(|_| CollectorSetupError::Unavailable)?;
    EvidenceId::new(format!("connect-setup-{}", digest(&entropy)))
        .map_err(|_| CollectorSetupError::Unavailable)
}

#[cfg(test)]
mod tests;
