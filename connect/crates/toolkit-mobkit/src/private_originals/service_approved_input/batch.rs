//! One reviewed public set through the existing intent, source and attachment
//! owners. A returned error reconciles each actual unpublished member; it does
//! not claim rollback of a separately accepted member-policy successor.
use super::*;
use crate::installed_host::operator_setup::{MAX_PUBLIC_SERVICE_BATCH, ServiceMcpParentBatchReview};
use crate::private_originals::service_grants::catalog::{CatalogDecisionKind, ServiceCatalogDecision};
use std::collections::BTreeSet;

pub struct PublicServiceBatchEntry {
    pub configuration: meerkat_core::McpServerConfig,
    pub operations: BTreeSet<Id>,
    pub request: ApprovalRequest,
}

pub struct PreparedPublicServiceBatch {
    task: InstalledServiceTask,
    operator: OperatorSetup,
    review: ServiceMcpParentBatchReview,
    tool_policy: crate::installed_host::tool_policy::service_policy::PreparedServiceToolPolicy,
}

impl InstalledServiceTask {
    /// The integrated public path freezes 2-8 distinct MCP targets, or 1-8
    /// when a configured warm setup capability was explicitly selected. Other
    /// singleton callers keep their existing review and record encoding.
    pub async fn prepare_public_batch(mut self, mut entries: Vec<PublicServiceBatchEntry>,
        expires_in_ms: Option<u64>, mut request: ApprovalRequest, policy: ServiceToolPolicySelection,
    ) -> Result<PreparedPublicServiceBatch, PrivateOriginalError> {
        if entries.len() < (if self.setup.is_some() { 1 } else { 2 }) || entries.len() > MAX_PUBLIC_SERVICE_BATCH || policy.catalog.is_none() {
            return Err(PrivateOriginalError::Denied);
        }
        entries.sort_by(|left, right| left.configuration.name.cmp(&right.configuration.name));
        if entries.windows(2).any(|pair| pair[0].configuration.name == pair[1].configuration.name) {
            return Err(PrivateOriginalError::Denied);
        }
        if let Some(setup) = &self.setup {
            let setup_targets = setup.binding_targets()?;
            if setup_targets.len() + entries.len()
                > meerkat_authorization_contracts::work_association::MAX_ASSOCIATION_REFERENCES {
                return Err(PrivateOriginalError::Denied);
            }
            if entries.iter().any(|entry| setup.configured.capability.helper_targets.iter().any(|target|
                matches!(&target.target, Some(meerkat_authorization_contracts::work_association::GrantTarget::McpServer { server })
                    if server == &entry.configuration.name))) {
                return Err(PrivateOriginalError::Conflict);
            }
        }
        self.current().await?;
        let tool_policy = self.prepare_tool_policy(Some(policy)).await?.ok_or(PrivateOriginalError::Denied)?;
        let mut members = Vec::with_capacity(entries.len());
        for entry in entries {
            let selected = ServiceMcpSelection::no_connector(&entry.configuration)?;
            let prepared = self.clone().prepare_mcp_review_for_member(&entry.configuration, selected,
                entry.operations, expires_in_ms, entry.request, None, None, true).await?;
            members.push(prepared.review);
        }
        let first = members.first().ok_or(PrivateOriginalError::Denied)?;
        let context = first.request().proposed_action.body.as_ref()
            .and_then(|body| body.get("access_request")).and_then(|body| body.get("service_instruction"))
            .cloned().ok_or(PrivateOriginalError::Conflict)?;
        let body = request.proposed_action.body.as_mut().and_then(serde_json::Value::as_object_mut)
            .ok_or(PrivateOriginalError::Denied)?;
        body.insert("service_instruction".into(), context);
        body.insert("member_tool_policy".into(), serde_json::json!({
            "proposal": tool_policy.proposal,
            "approval_scope": "the complete successor policy including preserved member permissions",
            "lifetime": "until superseded; task closure does not undo this separately reviewed policy",
            "catalog_visibility": "eligible tool metadata in the original session; invocation still needs its exact native target grant"
        }));
        let expiry = members.iter().filter_map(|member| member.decision().permission.lifetime.expires_at_ms)
            .min().ok_or(PrivateOriginalError::Denied)?;
        if let Some(setup) = &mut self.setup {
            setup.expires_at_ms = setup.expires_at_ms.min(expiry);
            let route = self.source.route(&self.parent.route)?;
            body.insert("configured_setup".into(), serde_json::json!({
                "selection": setup,
                "authority": "fresh children for this task from the already configured agent setup policy",
                "catalog_scope": "Both named agents may read tool metadata in both named sessions. This is the full two-agent by two-session scope, not paired permissions. It grants no service invocation.",
                "processors": [route.logical_executor, setup.helper.logical_executor],
                "sessions": [self.task.origin_session_id, setup.helper.session_id],
                "audience": setup.helper.audience,
                "expires_at_ms": setup.expires_at_ms,
                "continuation": "A later acquisition requires its own current capture, access review and fresh grants. Earlier connections must be selected again when needed."
            }));
        }
        let catalog = ServiceCatalogDecision {
            kind: match &self.setup {
                None => CatalogDecisionKind::PublicServiceCatalogV1,
                Some(setup) => CatalogDecisionKind::PublicServiceCatalogWithSetupV2 { helper: setup.helper.clone() },
            },
            route: self.parent.route.clone(),
            decision: crate::installed_host::operator_setup::service_batch_member_id(&self.fresh, None)
                .map_err(application::policy_error)?,
            evidence: PrivateSourceEvidence { resource_id: self.fresh.clone(),
                revision: EvidenceId::new(if self.setup.is_some() { "public-services-with-setup-v2" }
                    else { "public-service-batch-v1" }).map_err(|_| PrivateOriginalError::Conflict)?,
                sha256: digest(&canonical_json(&request.proposed_action.body).map_err(|_| PrivateOriginalError::Conflict)?) },
            original: self.fresh.clone(), session: self.task.origin_session_id.clone(),
            lifetime: RoleLifetime { scope: LifetimeScope::Task {
                task: Id::new(self.fresh.as_str()).map_err(|_| PrivateOriginalError::Conflict)? },
                expires_at_ms: Some(expiry) },
        };
        let lifetime = self.source.host_lifetime.get().and_then(Weak::upgrade).ok_or(PrivateOriginalError::Unavailable)?;
        let operator = self.source.service_operator_setup(lifetime).map_err(application::policy_error)?;
        let review = operator.prepare_service_mcp_parent_set_review(members, catalog, request, self.setup.clone())
            .map_err(application::policy_error)?;
        Ok(PreparedPublicServiceBatch { task: self, operator, review, tool_policy })
    }
}

impl PreparedPublicServiceBatch {
    pub fn request(&self) -> &ApprovalRequest { self.review.request() }

    /// The caller must retain and join this future after retiring its currentness
    /// predicate. Dropping it does not confirm that entered native issuance
    /// settled or that compensation completed. The normal Console driver joins
    /// the review after cancellation, including any entered issuance worker.
    pub async fn approve(self, record: ApprovalRecord,
        presented: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Result<bool, PrivateOriginalError> {
        let mut cleanup = Vec::with_capacity(self.review.members().len() + 1);
        for member in self.review.members() {
            let Some(parent) = self.operator.service_mcp_parent_compensation(member, &record)
                .map_err(application::policy_error)? else { return Ok(false); };
            cleanup.push((parent, PrivateOriginals::service_mcp_reference(member.decision(), member.selection())?));
        }
        cleanup.push((self.operator.catalog_parent_compensation(self.review.catalog(), &record)
            .map_err(application::policy_error)?, PrivateOriginals::service_catalog_reference(self.review.catalog().decision())?));
        let reservation = self.task.console_login.as_ref().ok_or(PrivateOriginalError::Denied)?
            .reserve().map_err(|_| PrivateOriginalError::Denied)?;
        let retained_login = Arc::new(std::sync::Mutex::new(Some(reservation)));
        let worker_login = retained_login.clone();
        let task = self.task.clone();
        let result = self.approve_retained(record, Arc::new(move || {
            let _actual_reservation = &worker_login;
            presented()
        })).await;
        if result.is_ok() {
            if let Some(reservation) = retained_login.lock().map_err(|_| PrivateOriginalError::Unavailable)?.take() {
                reservation.settled();
            }
        } else {
            // Join every known member even if another compensation remains
            // uncertain. The retained intents remain available for cold review.
            let mut failure = None;
            for (parent, grant) in cleanup {
                if let Err(error) = task.compensate_unpublished_approval(parent, grant).await { failure = Some(error); }
            }
            if let Some(error) = failure { return Err(error); }
        }
        result
    }

    async fn approve_retained(self, record: ApprovalRecord, presented: Arc<dyn Fn() -> bool + Send + Sync>)
        -> Result<bool, PrivateOriginalError>
    {
        self.task.current().await?;
        if !presented() { return Err(PrivateOriginalError::Denied); }
        let evidence = approval_evidence(&record)?;
        let approval = ServiceToolPolicyApproval::reviewed(&self.tool_policy.proposal, record.clone())
            .map_err(application::policy_error)?;
        let transaction = self.tool_policy.begin(&approval, &|| if presented() { Ok(()) }
            else { Err(meerkat_toolkit::host_lifetime::control::HostLifetimeError::Denied) })
            .await.map_err(application::policy_error)?;
        let custody = Arc::new(tokio::sync::Mutex::new(transaction));
        let worker_custody = custody.clone();
        let current: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(move || {
            let _actual_policy_custody = &worker_custody;
            presented()
        });
        let set_sha256 = self.review.set_sha256().to_owned();
        let (reviews, catalog) = self.review.into_parts();
        let mut members = Vec::with_capacity(reviews.len());
        for review in reviews {
            self.task.current().await?;
            if !current() { return Err(PrivateOriginalError::Denied); }
            let connection = review.decision().permission.connection.clone();
            let approved = match self.operator.apply_service_mcp_parent_decision_current(review, record.clone(), current.clone())
                .await.map_err(application::policy_error)? {
                ServiceMcpParentOutcome::Approved(value) => value,
                _ => return Err(PrivateOriginalError::Unavailable),
            };
            self.task.current().await?;
            let observed = self.task.source.apply_mcp_service_access_decision_current(approved, current.clone()).await?;
            if observed.state != PrivateServiceGrantState::GrantIssued { return Err(PrivateOriginalError::Unavailable); }
            members.push(ApprovedServiceMember { connection, grant: observed.grant });
        }
        self.task.current().await?;
        let approved = self.operator.apply_catalog_parent(catalog, record, current.clone()).await.map_err(application::policy_error)?;
        let catalog = self.task.source.apply_service_catalog_access(approved, current.clone()).await?;
        if catalog.state != PrivateServiceGrantState::GrantIssued { return Err(PrivateOriginalError::Unavailable); }
        let mut transaction = custody.lock().await;
        let condition = transaction.publish(&|| if current() { Ok(()) }
            else { Err(meerkat_toolkit::host_lifetime::control::HostLifetimeError::Denied) })
            .await.map_err(application::policy_error)?;
        let link = ApprovedTaskLink::Batch(selection::ApprovedTaskBatch {
            kind: if self.task.setup.is_some() { selection::BatchVersion::PublicServicesWithSetupV2 }
                else { selection::BatchVersion::PublicServicesV1 },
            header: self.task.task_header(), set_sha256, members, catalog: catalog.grant,
            setup: self.task.setup.clone(),
        });
        self.task.retain_approved_selection(link, evidence, &|| if current() { Ok(()) }
            else { Err(PrivateOriginalError::Denied) }, Some(condition)).await?;
        Ok(true)
    }
}

/// Public setup-enabled singletons use the same reviewed-set owner. Existing
/// callers without that selected capability retain their exact singleton path.
pub enum PreparedPublicInstruction {
    Single(PreparedServiceInstruction),
    WithSetup(PreparedPublicServiceBatch),
}
impl PreparedPublicInstruction {
    pub fn request(&self) -> &ApprovalRequest {
        match self { Self::Single(value) => value.request(), Self::WithSetup(value) => value.request() }
    }
    pub async fn approve(self, record: ApprovalRecord,
        presented: impl Fn() -> bool + Send + Sync + 'static,
    ) -> Result<bool, PrivateOriginalError> {
        match self {
            Self::Single(value) => value.approve(record, presented).await,
            Self::WithSetup(value) => value.approve(record, presented).await,
        }
    }
}
impl InstalledServiceTask {
    pub async fn prepare_public_instruction(self, configuration: &meerkat_core::McpServerConfig,
        operations: BTreeSet<Id>, expires_in_ms: Option<u64>, mut request: ApprovalRequest,
        policy: Option<ServiceToolPolicySelection>,
    ) -> Result<PreparedPublicInstruction, PrivateOriginalError> {
        if self.setup.is_none() {
            return self.prepare_no_connector_review_with_tool_policy(configuration, operations,
                expires_in_ms, request, policy).await.map(PreparedPublicInstruction::Single);
        }
        let member_request = request.clone();
        let document = request.proposed_action.body.clone().ok_or(PrivateOriginalError::Denied)?;
        request.proposed_action.body = Some(serde_json::json!({
            "phase": "installed_public_service_batch_access", "services": [document],
            "notice": "One reviewed service and fresh task-specific setup capability. Approval is not readiness."
        }));
        request.request_body = request.proposed_action.body.clone();
        self.prepare_public_batch(vec![PublicServiceBatchEntry {
            configuration: configuration.clone(), operations, request: member_request,
        }], expires_in_ms, request, policy.ok_or(PrivateOriginalError::Denied)?)
            .await.map(PreparedPublicInstruction::WithSetup)
    }
}
