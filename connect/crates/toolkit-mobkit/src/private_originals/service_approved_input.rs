//! A fresh local approval source keeps a cancellation link to the waiting task.
//! This link is business correlation; the native input carries only its actual
//! newly approved source and any independently named authority contributors.

use super::*;
use crate::original_ingress::OriginalCancellationSource;
use meerkat_runtime::meerkat_machine::{AuthenticatedOriginalSource, OriginalDisposition};
use meerkat_toolkit::request::NativeOriginalTask;

pub(super) mod attachment;
mod continuation;
pub(super) mod application;
mod compensation;
mod batch;
mod selection;
use selection::{ApprovedServiceMember, ApprovedTaskHeader, ApprovedTaskMember};
pub(super) use selection::ApprovedTaskLink;
pub use batch::{PublicServiceBatchEntry, PreparedPublicServiceBatch, PreparedPublicInstruction};

impl PrivateOriginals {
    /// Reopen the original source chain after a crash. Parent cancellation is
    /// authoritative even if writing this child's cancellation never finished.
    /// Missing or conflicting source facts refuse; a missing row is not open.
    pub(super) async fn approved_task_is_cancelled(
        &self,
        record: &SourceRecord,
    ) -> Result<bool, PrivateOriginalError> {
        Ok(self.cancelled_task_ancestor(record).await?.is_some())
    }

    /// Return the outermost durably cancelled ancestor. Replaying that exact
    /// group repairs both parent and child process publication after a partial
    /// parent-first write, including a child already admitted by native.
    async fn cancelled_task_ancestor(
        &self,
        record: &SourceRecord,
    ) -> Result<Option<PrivateOriginalRef>, PrivateOriginalError> {
        let mut cancelled = None;
        let mut next = record.approved_task.as_ref().map(|link| link.parent.clone());
        let mut visited = vec![];
        while let Some(parent) = next {
            let key = self.key(&parent)?;
            if visited.contains(&key) || visited.len() >= 64 {
                return Err(PrivateOriginalError::Conflict);
            }
            visited.push(key.clone());
            let stored = self.store.get(&key).await.map_err(store_error)?
                .ok_or(PrivateOriginalError::Unavailable)?;
            let parent_record = self.decode_record(&stored, &parent)?;
            if parent_record.operation_cancelled { cancelled = Some(parent); }
            next = parent_record.approved_task.map(|link| link.parent.clone());
        }
        Ok(cancelled)
    }

    /// Capture existing approved-task ancestry for the same result write.
    /// These conditions fence parent-first cancellation, not native permission.
    pub(in crate::private_originals) async fn extend_result_ancestor_sources(
        &self, record: &SourceRecord, sources: &mut Vec<toolkit_store::RecordCondition>,
    ) -> Result<(), PrivateOriginalError> {
        let mut next = record.approved_task.as_ref().map(|link| link.parent.clone());
        let mut visited = vec![];
        while let Some(parent) = next {
            let key = self.key(&parent)?;
            if visited.contains(&key) || visited.len() >= 64 {
                return Err(PrivateOriginalError::Conflict);
            }
            visited.push(key.clone());
            let row = self.store.get(&key).await.map_err(store_error)?
                .ok_or(PrivateOriginalError::Unavailable)?;
            let parent_record = self.decode_record(&row, &parent)?;
            if parent_record.operation_cancelled { return Err(PrivateOriginalError::Denied); }
            let condition = toolkit_store::RecordCondition {
                key: row.key, revision: row.revision, content_digest: row.content_digest,
                claim: None, dispatch_claim: None,
            };
            if let Some(prior) = sources.iter().find(|prior| prior.key == condition.key) {
                if prior != &condition { return Err(PrivateOriginalError::Conflict); }
            } else { sources.push(condition); }
            next = parent_record.approved_task.map(|link| link.parent.clone());
        }
        Ok(())
    }

    pub(super) async fn persist_source_cancellation(
        &self,
        validated: Arc<StoredRecord>,
    ) -> Result<Arc<StoredRecord>, OriginalWriterError> {
        self.persist_source_cancellation_with_failure(validated, None, None, None)
            .await
    }

    async fn persist_source_cancellation_with_failure(
        &self, validated: Arc<StoredRecord>,
        failure: Option<meerkat_toolkit::setup::ApprovedServiceAttachmentFailure>,
        observation: Option<meerkat_toolkit::setup::ApprovedServiceAttachmentObservation>,
        member: Option<&PrivateServiceGrantRef>,
    ) -> Result<Arc<StoredRecord>, OriginalWriterError> {
        let before = self.decode_stored(&validated).map_err(|_| OriginalWriterError::Conflict)?;
        let stored = self.store.get(&validated.key).await
            .map_err(|_| OriginalWriterError::Unavailable)?
            .ok_or(OriginalWriterError::Unavailable)?;
        let mut current = self.decode_stored(&stored).map_err(|_| OriginalWriterError::Conflict)?;
        if current.source != before.source || current.event != before.event
            || current.approved_task != before.approved_task
            || current.final_association != before.final_association
        { return Err(OriginalWriterError::Conflict); }
        if current.operation_cancelled { return Ok(Arc::new(stored)); }
        // Only the exact held terminal attempt supplies `failure`. Commit it
        // in the same CAS as cancellation, before native publication settles.
        // A prior cancellation or any immutable B binding never acquires it.
        if current.approved_task.is_some() && current.final_association.is_none()
            && current.service_admission.is_none() {
            current.service_attachment_failure = failure;
            current.service_attachment_observation = observation;
            if let Some(member) = member {
                let Some(ApprovedTaskLink::Batch(batch)) = &current.approved_task else { return Err(OriginalWriterError::Conflict); };
                if !batch.members.iter().any(|selected| &selected.grant == member) || failure.is_none() {
                    return Err(OriginalWriterError::Conflict);
                }
                current.service_attachment_member = Some(member.clone());
            }
        }
        current.operation_cancelled = true;
        let data = encode(current.clone()).map_err(|_| OriginalWriterError::Conflict)?;
        let saved = match self.store.compare_and_swap(Transition {
            key: stored.key.clone(), expected_revision: stored.revision,
            expected_digest: Some(stored.content_digest), claim: None,
            data, events: vec![], effects: vec![],
        }).await {
            Ok(saved) => saved,
            Err(_) => self.store.get(&stored.key).await
                .map_err(|_| OriginalWriterError::Unavailable)?
                .ok_or(OriginalWriterError::Unavailable)?,
        };
        if self.decode_stored(&saved).map_err(|_| OriginalWriterError::Conflict)? != current {
            return Err(OriginalWriterError::Unavailable);
        }
        Ok(Arc::new(saved))
    }

    /// The existing writer owns discovery, Pending publication and all writes.
    /// Source creation takes that same writer, so no child can enter behind the
    /// census. This scans the existing canonical store, not a second registry.
    pub(super) async fn record_task_group_cancelled(
        &self,
        reference: &PrivateOriginalRef,
    ) -> Result<(), PrivateOriginalError> {
        self.record_task_group_cancelled_with_failure(reference, None, None)
            .await
    }

    pub(super) async fn record_task_group_cancelled_with_failure(
        &self, reference: &PrivateOriginalRef,
        failure: Option<meerkat_toolkit::setup::ApprovedServiceAttachmentFailure>,
        observation: Option<meerkat_toolkit::setup::ApprovedServiceAttachmentObservation>,
    ) -> Result<(), PrivateOriginalError> {
        self.record_task_group_cancelled_with_member(reference, failure, observation, None).await
    }

    pub(super) async fn record_task_group_cancelled_with_member(
        &self, reference: &PrivateOriginalRef,
        failure: Option<meerkat_toolkit::setup::ApprovedServiceAttachmentFailure>,
        observation: Option<meerkat_toolkit::setup::ApprovedServiceAttachmentObservation>,
        member: Option<&PrivateServiceGrantRef>,
    ) -> Result<(), PrivateOriginalError> {
        self.record_cancelled_source_set(reference, failure, observation, member)
            .await.map(|_| ())
    }

    /// The existing cancellation owner returns its actual completed census.
    /// Only the outer private-channel cancellation uses it for workflow leave;
    /// internal admission/compensation callers keep their existing behavior.
    pub(super) async fn record_cancelled_source_set(
        &self, reference: &PrivateOriginalRef,
        failure: Option<meerkat_toolkit::setup::ApprovedServiceAttachmentFailure>,
        observation: Option<meerkat_toolkit::setup::ApprovedServiceAttachmentObservation>,
        member: Option<&PrivateServiceGrantRef>,
    ) -> Result<Vec<Arc<toolkit_store::StoredRecord>>, PrivateOriginalError> {
        if observation.is_some() && failure.is_none() {
            return Err(PrivateOriginalError::Conflict);
        }
        let failed_key = failure.map(|_| self.key(reference)).transpose()?;
        let cancelled = self.owner.record_linked_operations_cancelled(
            || async {
                let root = self.validate_cancellation_source(reference, None, None).await?;
                let mut selected = vec![root];
                let mut records = vec![];
                let mut after = None;
                loop {
                    let page = self.store.list_records(&self.scope, after.as_deref(), 256).await
                        .map_err(|_| OriginalWriterError::Unavailable)?;
                    if page.is_empty() { break; }
                    after = page.last().map(|record| record.key.id.clone());
                    for stored in page {
                        if stored.data.schema == SOURCE_SCHEMA {
                            let record = self.decode_stored(&stored)
                                .map_err(|_| OriginalWriterError::Conflict)?;
                            records.push((Arc::new(stored), record));
                        }
                    }
                }
                loop {
                    let mut added = false;
                    for (stored, record) in &records {
                        let Some(link) = &record.approved_task else { continue; };
                        if selected.iter().any(|current| current.key == stored.key) { continue; }
                        // A follow-up retains the parent's qualified source.
                        // Determine group membership from those immutable facts
                        // before requiring any currently installed route.
                        let parent_key = StoreKey {
                            realm: stored.key.realm.clone(), owner: stored.key.owner.clone(),
                            id: source_key_id(&record.source.realm, &record.source.ingress,
                                &record.source.source_authority, &link.parent.original)
                                .map_err(|_| OriginalWriterError::Conflict)?,
                        };
                        if let Some(parent) = selected.iter().find(|current| current.key == parent_key) {
                            self.decode_record(parent, &link.parent).map_err(|_| OriginalWriterError::Conflict)?;
                            selected.push(stored.clone());
                            added = true;
                        }
                    }
                    if !added { break; }
                }
                selected.into_iter().map(|stored| {
                    let record = self.decode_stored(&stored).map_err(|_| OriginalWriterError::Conflict)?;
                    Ok(OriginalCancellationSource {
                        realm: record.source.realm,
                        original: meerkat_authorization_contracts::work_association::OriginalWorkRef {
                            authority: record.source.source_authority, work: record.event.original,
                        },
                        association: record.final_association,
                        record: stored,
                    })
                }).collect()
            },
            |stored| {
                let selected = failed_key.as_ref().is_some_and(|key| key == &stored.key);
                self.persist_source_cancellation_with_failure(stored,
                    if selected { failure } else { None }, if selected { observation } else { None },
                    if selected { member } else { None })
            },
        ).await.map_err(|error| match error {
            OriginalWriterError::WrongOwner => PrivateOriginalError::Denied,
            OriginalWriterError::Conflict => PrivateOriginalError::Conflict,
            _ => PrivateOriginalError::Unavailable,
        })?;
        Ok(cancelled)
    }
}

use crate::installed_host::operator_setup::{
    OperatorSetup, ServiceMcpParentReview, ServiceMcpParentOutcome,
};
use meerkat_core::{ApprovalRecord, ApprovalRequest};
use meerkat_toolkit::{
    authority::{LifetimeScope, RoleLifetime},
    host_lifetime::policy::CompiledNativeSourceOrigin,
    setup::{SetupScope, SetupSnapshot, SetupStore},
};
use toolkit_package::Revision;
use toolkit_store::{ConditionalAdmission, ConditionalAdmissionStore, RecordCondition, SqliteStore};

/// Selected from an actual helper dispatch and the canonical acquisition, never
/// decoded from a tool argument. It is a proposal until the operator approves.
#[derive(Clone)]
pub struct InstalledServiceTask {
    source: Arc<PrivateOriginals>,
    store: Arc<SqliteStore>,
    parent: PrivateOriginalRef,
    parent_association: meerkat_authorization_contracts::work_association::InputAuthorityAssociation,
    task: NativeOriginalTask,
    acquisition: StoreKey,
    input_digest: String,
    fresh: EvidenceId,
    instruction: String,
    before_ms: u64,
    console_login: Option<crate::private_host::ConsoleLoginHandle>,
    installation_readback: Option<RetainedApplicationInstallation>,
    pub(super) setup: Option<super::configured_setup::service::ServiceSetupSelection>,
}

/// Nonsecret requested effects for the defining realm's native owner. The
/// trusted host supplies its own-document snapshot, never the member realm.
/// This is proposal data, not an admin permit, approval or effect receipt.
/// The exact configuration and declared secret names are bound during review;
/// submitted secret values never belong in this type or the review request.
pub struct ServiceMcpPersistenceProposal {
    defining_realm: RealmId,
    expected_configuration_generation: u64,
}
impl ServiceMcpPersistenceProposal {
    pub fn new(defining_realm: RealmId, expected_configuration_generation: u64) -> Self {
        Self {
            defining_realm,
            expected_configuration_generation,
        }
    }
}

pub struct PreparedServiceInstruction {
    task: InstalledServiceTask,
    operator: OperatorSetup,
    review: ServiceMcpParentReview,
    tool_policy: Option<crate::installed_host::tool_policy::service_policy::PreparedServiceToolPolicy>,
}
impl PreparedServiceInstruction {
    pub fn request(&self) -> &ApprovalRequest { self.review.request() }
    /// Native operator issuance and the task leaf must settle before source
    /// publication. The caller joins this future on cancellation or close.
    pub async fn approve(self, record: ApprovalRecord, presented: impl Fn() -> bool + Send + Sync + 'static) -> Result<bool, PrivateOriginalError> {
        let reservation = self.task.console_login.as_ref().map(|login| login.reserve())
            .transpose().map_err(|_| PrivateOriginalError::Denied)?;
        // Keep the login reservation with an entered blocking issuance worker,
        // even if an exceptional caller drops this future instead of joining it.
        let retained_login = Arc::new(std::sync::Mutex::new(reservation));
        let worker_login = retained_login.clone();
        let cleanup = self.operator.service_mcp_parent_compensation(&self.review, &record)
            .map_err(application::policy_error)?;
        let task = self.task.clone();
        let grant = PrivateOriginals::service_mcp_reference(self.review.decision(), self.review.selection())?;
        let result = self.approve_retained(record, Arc::new(move || {
            let _retained_login = &worker_login;
            presented()
        })).await;
        if result.is_ok() {
            if let Some(reservation) = retained_login.lock()
                .map_err(|_| PrivateOriginalError::Unavailable)?.take()
            { reservation.settled(); }
        } else if let Some(cleanup) = cleanup {
            // This future has joined every issuance worker. Reconcile only
            // this approval's unpublished grants under the source writer.
            // A policy successor is separately approved and is not rolled back.
            task.compensate_unpublished_approval(cleanup, grant).await?;
            // An error can include another owner's unresolved policy effect.
            // Native revoke success alone does not settle that login aggregate.
        }
        result
    }
    async fn approve_retained(self, record: ApprovalRecord, presented: Arc<dyn Fn() -> bool + Send + Sync>)
        -> Result<bool, PrivateOriginalError>
    {
        if !presented() { return Err(PrivateOriginalError::Denied); }
        self.task.current().await?;
        if !presented() { return Err(PrivateOriginalError::Denied); }
        let evidence = approval_evidence(&record)?;
        if !self.operator.validate_service_mcp_parent_approval(&self.review, &record)
            .map_err(application::policy_error)? { return Ok(false); }
        let transaction = match self.tool_policy {
            Some(prepared) => {
                let approval = ServiceToolPolicyApproval::reviewed(&prepared.proposal, record.clone())
                    .map_err(application::policy_error)?;
                Some(prepared.begin(&approval, &|| if presented() { Ok(()) }
                    else { Err(meerkat_toolkit::host_lifetime::control::HostLifetimeError::Denied) })
                    .await.map_err(application::policy_error)?)
            }
            None => None,
        };
        let policy_custody = Arc::new(tokio::sync::Mutex::new(transaction));
        let worker_policy = policy_custody.clone();
        let presented: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(move || {
            let _actual_policy_custody = &worker_policy;
            presented()
        });
        let approved = match self.operator.apply_service_mcp_parent_decision_current(self.review, record, presented.clone()).await
            .map_err(|_| PrivateOriginalError::Unavailable)?
        {
            ServiceMcpParentOutcome::Approved(approved) => approved,
            ServiceMcpParentOutcome::Denied => return Ok(false),
            ServiceMcpParentOutcome::Unavailable | ServiceMcpParentOutcome::PendingReconciliation =>
                return Err(PrivateOriginalError::Unavailable),
        };
        self.task.current().await?;
        if !presented() { return Err(PrivateOriginalError::Denied); }
        let observation = self.task.source.apply_mcp_service_access_decision_current(approved, presented.clone()).await?;
        if observation.state != PrivateServiceGrantState::GrantIssued {
            return Err(PrivateOriginalError::Unavailable);
        }
        let mut held_policy = policy_custody.lock().await;
        let condition = match held_policy.as_mut() {
            Some(policy) => Some(policy.publish(&|| if presented() { Ok(()) }
                else { Err(meerkat_toolkit::host_lifetime::control::HostLifetimeError::Denied) })
                .await.map_err(application::policy_error)?),
            None => None,
        };
        self.task.retain_approved_with_policy(observation.grant, evidence, &|| {
            if presented() { Ok(()) } else { Err(PrivateOriginalError::Denied) }
        }, condition).await?;
        Ok(true)
    }
}

impl PrivateOriginals {
    pub(crate) async fn prepare_service_task(
        self: &Arc<Self>, store: Arc<SqliteStore>, snapshot: SetupSnapshot,
        call_id: &str, agent: &str,
    ) -> Result<Option<InstalledServiceTask>, PrivateOriginalError> {
        let source_store: Arc<dyn ToolkitStore> = store.clone();
        if !Arc::ptr_eq(&self.store, &source_store) || call_id.is_empty() {
            return Err(PrivateOriginalError::Denied);
        }
        let task = snapshot.request().host().continuation.native_original.clone()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let association = snapshot.request().evidence().association().clone();
        let held = self.retained.lock().map_err(|_| PrivateOriginalError::Unavailable)?
            .iter().find(|held| held.original.association() == &association)
            .cloned().ok_or(PrivateOriginalError::Unavailable)?;
        let parent = held.reference.clone();
        let console_login = self.service_source_console_login(&parent, &task)?;
        let lifetime = self.host_lifetime.get().and_then(Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        lifetime.validate_current_selection().map_err(|_| PrivateOriginalError::Unavailable)?;
        let selected = lifetime.selected_policy();
        let native = selected.native_host.as_ref().ok_or(PrivateOriginalError::Unavailable)?;
        let compiled = native.routes.iter().find(|entry| entry.id == parent.route)
            .ok_or(PrivateOriginalError::Denied)?;
        // The live caller is the helper; the proposal's agent must be the
        // waiting task's selected target. Source authentication was checked
        // above without requiring either installation or service permission.
        let (member, _, _) = compiled.source_binding(&selected).map_err(|_| PrivateOriginalError::Denied)?;
        if agent != member.member || task.member.as_ref() != Some(member)
            || association.candidate().target.context.as_str() != task.origin_session_id.to_string()
            || association.candidate().original_work != *snapshot.request().original_work()
        { return Err(PrivateOriginalError::Denied); }
        let fresh = EvidenceId::new(format!("service-input-{}", digest(&canonical_json(&(
            snapshot.key(), &parent, call_id,
        )).map_err(|_| PrivateOriginalError::Conflict)?))).map_err(|_| PrivateOriginalError::Conflict)?;
        let proposed = InstalledServiceTask {
            source: self.clone(), store, parent, parent_association: association,
            task, acquisition: snapshot.key().clone(),
            input_digest: snapshot.request().input_digest().to_string(), fresh,
            instruction: snapshot.request().request().request.clone(),
            before_ms: snapshot.request().host().limits.deadline_ms,
            console_login, installation_readback: None, setup: None,
        };
        proposed.current().await?;
        Ok(Some(proposed))
    }
}

impl InstalledServiceTask {
    pub(crate) fn select_configured_setup(mut self,
        helper: &crate::installed_host::acquisition::InstalledSetupSelection,
        member: &meerkat_toolkit::host_lifetime::policy::CompiledNativeMember,
        session: &meerkat_core::SessionId,
    ) -> Result<Self, PrivateOriginalError> {
        if self.setup.is_some() { return Err(PrivateOriginalError::Conflict); }
        self.setup = self.source.select_service_setup(&self.parent, &self.task.origin_session_id,
            self.before_ms, helper, member, session)?;
        Ok(self)
    }

    /// This is a selected proposal, not a native permit or a bound capability.
    pub fn has_configured_setup(&self) -> bool { self.setup.is_some() }

    /// Bind the installed product owner's exact checked software document to
    /// this proposal. This supplies no policy decision or native permission.
    /// The document must be nonsecret; the package owner verifies it again at
    /// readback. Existing application records without it remain unsupported.
    pub fn with_installation_readback_source(mut self, configuration: &meerkat_core::McpServerConfig,
        document: serde_json::Value) -> Result<Self, PrivateOriginalError> {
        if self.installation_readback.is_some() { return Err(PrivateOriginalError::Conflict); }
        let selected = RetainedApplicationInstallation {
            configuration: serde_json::to_value(configuration).map_err(|_| PrivateOriginalError::Conflict)?, document,
        };
        if selected.configuration()? != *configuration { return Err(PrivateOriginalError::Denied); }
        self.installation_readback = Some(selected);
        Ok(self)
    }

    /// Immutable canonical setup deadline. This is a presentation bound only;
    /// current task and effect admission still require their existing checks.
    pub fn deadline_ms(&self) -> u64 { self.before_ms }

    /// Recheck this existing task for a native verified viewer and exact login.
    /// The HTTP owner must resolve native access again after this awaited read.
    pub async fn check_console_viewer(&self,
        current: &(impl Fn() -> Option<meerkat_mobkit::http_console::ConsoleViewerContext> + Sync),
        login: &crate::private_host::ConsoleLoginHandle,
    ) -> Result<meerkat_mobkit::identity_first::AgentIdentity, PrivateOriginalError> {
        if !self.console_login.as_ref().is_some_and(|held| held.same_owner(login)) {
            return Err(PrivateOriginalError::Denied);
        }
        let viewer = current().ok_or(PrivateOriginalError::Denied)?;
        let human = viewer.human.as_ref().ok_or(PrivateOriginalError::Denied)?;
        let principal = human.principal().map_err(|_| PrivateOriginalError::Denied)?;
        human.current().map_err(|_| PrivateOriginalError::Denied)?;
        let (parent, _) = self.current().await?;
        let record = self.source.decode_record(&parent, &self.parent)?;
        if record.source.requester != principal
            || record.source.represented_subject.as_ref() != Some(&principal)
        { return Err(PrivateOriginalError::Denied); }
        let route = self.source.route(&self.parent.route)?;
        let runtime = self.source.bound_identity_runtime()?;
        let identity = runtime.owned_identity_for_member_alias(route.grant_target.as_str()).await
            .ok_or(PrivateOriginalError::Unavailable)?;
        self.current().await?;
        human.current().map_err(|_| PrivateOriginalError::Denied)?;
        let fresh = current().ok_or(PrivateOriginalError::Denied)?;
        if fresh.human.as_ref().and_then(|human| human.principal().ok()).as_ref() != Some(&principal) { return Err(PrivateOriginalError::Denied); }
        if fresh.access_view.as_ref().is_some_and(|view| view.enforced()
            && (!view.can_view_agent(identity.as_str())
                || !view.allows_agent(meerkat_mobkit::access::ACTION_AGENT_SEND, identity.as_str())))
        { return Err(PrivateOriginalError::Denied); }
        Ok(identity)
    }
    /// Reopen the exact existing task/source before a reviewed software dispatch.
    /// This returns no authority or reusable permit.
    pub async fn check_setup_current(&self) -> Result<(), PrivateOriginalError> {
        self.current().await.map(|_| ())
    }
    /// Definite canonical terminal observation for retiring an unentered
    /// presentation. Errors, missing rows and denied viewers are not terminal.
    /// This changes no task, grant, input, cancellation or native account state.
    pub async fn console_terminal(&self) -> Result<bool, PrivateOriginalError> {
        let parent = self.source.store.get(&self.source.key(&self.parent)?).await.map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let record = self.source.decode_record(&parent, &self.parent)?;
        if record.final_association.as_ref() != Some(&self.parent_association) {
            return Err(PrivateOriginalError::Conflict);
        }
        if record.operation_cancelled || self.source.approved_task_is_cancelled(&record).await? {
            return Ok(true);
        }
        let operation = Id::new(self.acquisition.id.clone()).map_err(|_| PrivateOriginalError::Conflict)?;
        let store = SetupStore::new_conditional(self.store.as_ref(), self.scope()?)
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let snapshot = store.service_task_snapshot(&operation, &self.task).await
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        if snapshot.key() != &self.acquisition || snapshot.request().input_digest().as_str() != self.input_digest {
            return Err(PrivateOriginalError::Conflict);
        }
        let now = u64::try_from(self.store.now_ms().await.map_err(store_error)?)
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        Ok(now >= self.before_ms || matches!(snapshot.state().lifecycle_phase,
            toolkit_catalog::AcquisitionPhase::Cancelled | toolkit_catalog::AcquisitionPhase::Expired
                | toolkit_catalog::AcquisitionPhase::Failed))
    }
    /// Exact native member captured by this already validated waiting task.
    /// This is a proposal selector, not authority or a display-name resolver.
    pub fn consumer_member(&self) -> Option<&str> {
        self.task.member.as_ref().map(|member| member.member.as_str())
    }
    /// Process custody from the actual parent original, never an HTTP label.
    pub fn console_login(&self) -> Option<&crate::private_host::ConsoleLoginHandle> {
        self.console_login.as_ref()
    }
    fn check_console_login(&self) -> Result<(), PrivateOriginalError> {
        let selected = self.source.service_source_console_login(&self.parent, &self.task)?;
        match (&self.console_login, selected) {
            (None, None) => Ok(()),
            (Some(held), Some(selected)) if held.same_owner(&selected) =>
                held.check().map_err(|_| PrivateOriginalError::Denied),
            _ => Err(PrivateOriginalError::Denied),
        }
    }
    fn scope(&self) -> Result<SetupScope, PrivateOriginalError> {
        Ok(SetupScope::new(Id::new(self.source.scope.realm.clone()).map_err(|_| PrivateOriginalError::Conflict)?,
            Id::new(self.source.scope.owner.clone()).map_err(|_| PrivateOriginalError::Conflict)?))
    }
    async fn current(&self) -> Result<(StoredRecord, SetupSnapshot), PrivateOriginalError> {
        self.check_console_login()?;
        let lifetime = self.source.host_lifetime.get().and_then(Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        // This read retains no effect entry across the following awaits.
        lifetime.validate_current_selection().map_err(|_| PrivateOriginalError::Denied)?;
        let parent = self.source.store.get(&self.source.key(&self.parent)?).await.map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let record = self.source.decode_record(&parent, &self.parent)?;
        let route = self.source.route(&self.parent.route)?;
        if record.source != SourceFacts::from_route(route, &record.source.ingress) {
            return Err(PrivateOriginalError::Conflict);
        }
        if record.operation_cancelled || self.source.approved_task_is_cancelled(&record).await?
            || record.final_association.as_ref() != Some(&self.parent_association)
        { return Err(PrivateOriginalError::Denied); }
        if !matches!(self.source.owner.original_disposition(
            &self.parent_association.candidate().ingress_namespace,
            &self.parent_association.candidate().original_work,
        ), Ok(OriginalDisposition::Active)) { return Err(PrivateOriginalError::Unavailable); }
        let operation = Id::new(self.acquisition.id.clone()).map_err(|_| PrivateOriginalError::Conflict)?;
        let store = SetupStore::new_conditional(self.store.as_ref(), self.scope()?)
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let snapshot = store.service_task_snapshot(&operation, &self.task).await
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let now = u64::try_from(self.store.now_ms().await.map_err(store_error)?)
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        if snapshot.key() != &self.acquisition
            || snapshot.request().input_digest().as_str() != self.input_digest
            || now >= self.before_ms
            || matches!(snapshot.state().lifecycle_phase, toolkit_catalog::AcquisitionPhase::Cancelled
                | toolkit_catalog::AcquisitionPhase::Expired | toolkit_catalog::AcquisitionPhase::Failed)
        { return Err(PrivateOriginalError::Denied); }
        lifetime.validate_current_selection().map_err(|_| PrivateOriginalError::Denied)?;
        self.check_console_login()?;
        Ok((parent, snapshot))
    }

    async fn prepare_tool_policy(&self, selection: Option<ServiceToolPolicySelection>)
        -> Result<Option<crate::installed_host::tool_policy::service_policy::PreparedServiceToolPolicy>, PrivateOriginalError> {
        let Some(selection) = selection else { return Ok(None); };
        let route = self.source.route(&self.parent.route)?;
        let owner = self.source.service_tool_policy.get().ok_or(PrivateOriginalError::Unavailable)?;
        let session = selection.catalog.as_ref().map(|_| self.parent_association.candidate().target.context.as_str().to_owned());
        owner.prepare(route.grant_target.as_str(), selection, session, self.parent.clone(), self.acquisition.clone())
            .await.map(Some).map_err(application::policy_error)
    }

    /// The existing access approval includes this exact proposed instruction.
    /// This is setup prose, not a claim to reproduce the user's original words.
    pub async fn prepare_review(
        self, installation: &meerkat::ConnectorMcpInstallation,
        operations: std::collections::BTreeSet<Id>, expires_in_ms: Option<u64>,
        request: ApprovalRequest,
    ) -> Result<PreparedServiceInstruction, PrivateOriginalError> {
        self.prepare_review_with_tool_policy(installation, operations, expires_in_ms, request, None).await
    }

    /// Review a package that explicitly needs no native connector. This grants
    /// only the same waiting task; it never persists realm configuration.
    pub async fn prepare_no_connector_review(
        self, config: &meerkat_core::McpServerConfig,
        operations: std::collections::BTreeSet<Id>, expires_in_ms: Option<u64>,
        request: ApprovalRequest,
    ) -> Result<PreparedServiceInstruction, PrivateOriginalError> {
        self.prepare_no_connector_review_with_tool_policy(config, operations, expires_in_ms, request, None).await
    }

    pub async fn prepare_review_with_tool_policy(self, installation: &meerkat::ConnectorMcpInstallation,
        operations: std::collections::BTreeSet<Id>, expires_in_ms: Option<u64>, request: ApprovalRequest,
        tool_policy: Option<ServiceToolPolicySelection>,
    ) -> Result<PreparedServiceInstruction, PrivateOriginalError> {
        self.prepare_mcp_review(installation.config(), ServiceMcpSelection::connector(installation),
            operations, expires_in_ms, request, tool_policy, None).await
    }
    pub async fn prepare_no_connector_review_with_tool_policy(self, config: &meerkat_core::McpServerConfig,
        operations: std::collections::BTreeSet<Id>, expires_in_ms: Option<u64>, request: ApprovalRequest,
        tool_policy: Option<ServiceToolPolicySelection>,
    ) -> Result<PreparedServiceInstruction, PrivateOriginalError> {
        let selection = ServiceMcpSelection::no_connector(config)?;
        self.prepare_mcp_review(config, selection, operations, expires_in_ms, request, tool_policy, None).await
    }

    /// Prepare one combined credential-persistence and task-access review.
    /// The caller must supply a truthful nonsecret request, not the public
    /// no-auth/no-persistence notice. This method performs no persistence and
    /// supplies no administration authority. Record the exact human decision
    /// before native effects; call approve only after required effects settle
    /// successfully, preserving the same decision and current entry checks.
    pub async fn prepare_no_connector_persistence_review(
        self,
        config: &meerkat_core::McpServerConfig,
        operations: std::collections::BTreeSet<Id>,
        expires_in_ms: Option<u64>,
        request: ApprovalRequest,
        tool_policy: Option<ServiceToolPolicySelection>,
        persistence: ServiceMcpPersistenceProposal,
    ) -> Result<PreparedServiceInstruction, PrivateOriginalError> {
        let selection = ServiceMcpSelection::no_connector(config)?;
        self.prepare_mcp_review(config, selection, operations, expires_in_ms,
            request, tool_policy, Some(persistence)).await
    }

    async fn prepare_mcp_review(
        self, config: &meerkat_core::McpServerConfig, selection: ServiceMcpSelection,
        operations: std::collections::BTreeSet<Id>, expires_in_ms: Option<u64>,
        request: ApprovalRequest, tool_policy: Option<ServiceToolPolicySelection>,
        persistence: Option<ServiceMcpPersistenceProposal>,
    ) -> Result<PreparedServiceInstruction, PrivateOriginalError> {
        self.prepare_mcp_review_for_member(config, selection, operations, expires_in_ms,
            request, tool_policy, persistence, false).await
    }

    async fn prepare_mcp_review_for_member(
        self, config: &meerkat_core::McpServerConfig, selection: ServiceMcpSelection,
        operations: std::collections::BTreeSet<Id>, expires_in_ms: Option<u64>,
        mut request: ApprovalRequest, tool_policy: Option<ServiceToolPolicySelection>,
        persistence: Option<ServiceMcpPersistenceProposal>, batch: bool,
    ) -> Result<PreparedServiceInstruction, PrivateOriginalError> {
        self.current().await?;
        let configuration_persistence = match persistence {
            None => serde_json::Value::String("not authorized by this review".into()),
            Some(persistence) => {
                let meerkat_core::mcp_config::McpTransportConfig::Stdio(launch) = &config.transport else {
                    return Err(PrivateOriginalError::Denied);
                };
                if launch.secret_env.is_empty() || !launch.env.is_empty() {
                    return Err(PrivateOriginalError::Denied);
                }
                config.validate_secret_env().map_err(|_| PrivateOriginalError::Denied)?;
                serde_json::json!({
                    "kind": "requested_defining_realm_persistence",
                    "defining_realm": persistence.defining_realm.as_str(),
                    "expected_configuration_generation": persistence.expected_configuration_generation,
                    "configuration": config,
                    "credential_slots": &launch.secret_env,
                    "requested_effects": "persist this exact configuration, then privately store the declared credentials",
                    "authority": "requires current authenticated native realm administration at each mutation entry",
                    "partial_effects": "configuration or credentials can remain after cancellation or later failure",
                })
            }
        };
        let tool_policy = self.prepare_tool_policy(tool_policy).await?;
        let route = self.source.route(&self.parent.route)?;
        let config_bytes = canonical_json(config).map_err(|_| PrivateOriginalError::Conflict)?;
        let now = u64::try_from(self.store.now_ms().await.map_err(store_error)?)
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let expires_at_ms = expires_in_ms.map(|ttl| now.checked_add(ttl)
            .ok_or(PrivateOriginalError::Denied)).transpose()?.unwrap_or(self.before_ms).min(self.before_ms);
        let task_id = Id::new(self.fresh.as_str()).map_err(|_| PrivateOriginalError::Conflict)?;
        let decision_id = if batch {
            crate::installed_host::operator_setup::service_batch_member_id(&self.fresh,
                Some(&Id::new(config.name.clone()).map_err(|_| PrivateOriginalError::Denied)?))
                .map_err(application::policy_error)?
        } else { task_id.clone() };
        let live_attachment = PrivateServiceMcpLiveAttachment {
            destination: self.parent_association.candidate().target.clone(),
        };
        let context = serde_json::json!({
            "live_attachment": &live_attachment,
            "live_attachment_effect": "hydrate this exact server and publish its tools to the original task session",
            "configuration_persistence": configuration_persistence,
            "proposed_fresh_instruction": self.instruction,
            "instruction_origin": "setup proposal; approval authorizes this new instruction",
            "fresh_original": self.fresh, "business_task": self.acquisition,
            "prior_original": self.parent_association.candidate().original_work,
            "operator_and_requester": route.requester, "ingress_actor": route.ingress_actor,
            "represented_subject": route.represented_subject, "audience": route.grant_audience,
            "original_task_fulfillment": "not established by this proposal",
        });
        request.proposed_action.body.as_mut().and_then(serde_json::Value::as_object_mut)
            .ok_or(PrivateOriginalError::Denied)?.insert("service_instruction".into(), context);
        if let Some(policy) = &tool_policy {
            let body = request.proposed_action.body.as_mut().and_then(serde_json::Value::as_object_mut)
                .ok_or(PrivateOriginalError::Denied)?;
            body.insert("member_tool_policy".into(), serde_json::json!({
                "proposal": policy.proposal,
                "approval_scope": "the complete successor policy, including preserved permissions for all listed members",
                "lifetime": "until superseded; this member policy remains after the service task ends",
                "catalog_visibility": "when selected, eligible tool metadata in the entire original conversation, not only this integration",
                "native_service_scope": "the separate native service and catalog grants still expire with this task"
            }));
        }
        let decision = PrivateServiceMcpAccessDecision {
            catalog: tool_policy.as_ref().and_then(|p| p.proposal.selection.catalog.as_ref())
                .map(|_| PrivateServiceMcpCatalog::OriginalSession),
            live_attachment: Some(live_attachment),
            route: self.parent.route.clone(), decision: decision_id.clone(),
            evidence: PrivateSourceEvidence { resource_id: self.fresh.clone(),
                revision: EvidenceId::new("proposal-v1").map_err(|_| PrivateOriginalError::Conflict)?,
                sha256: digest(&canonical_json(&request.proposed_action.body).map_err(|_| PrivateOriginalError::Conflict)?), },
            choice: PrivateAccessChoice::Approve,
            permission: PrivateServiceMcpPermission {
                target: route.grant_target.clone(),
                connection: Id::new(config.name.clone()).map_err(|_| PrivateOriginalError::Denied)?,
                operations,
                resource: Id::new(config.name.clone()).map_err(|_| PrivateOriginalError::Denied)?,
                lifetime: RoleLifetime { scope: LifetimeScope::Task { task: task_id },
                    expires_at_ms: Some(expires_at_ms) },
            },
            // Exact immutable native configuration content, never a physical
            // credential generation masquerading as a Toolkit revision.
            connection_revision: Revision::new(digest(&config_bytes)).map_err(|_| PrivateOriginalError::Conflict)?,
        };
        let lifetime = self.source.host_lifetime.get().and_then(Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        let operator = self.source.service_operator_setup(lifetime).map_err(|_| PrivateOriginalError::Denied)?;
        let review = operator.prepare_service_mcp_parent_review(decision, selection, request)
            .map_err(|_| PrivateOriginalError::Denied)?;
        Ok(PreparedServiceInstruction { task: self, operator, review, tool_policy })
    }

    async fn retain_approved(self, grant: PrivateServiceGrantRef, evidence: PrivateSourceEvidence,
        presented: &(impl Fn() -> Result<(), PrivateOriginalError> + Sync))
        -> Result<PrivateOriginalRef, PrivateOriginalError>
    {
        self.retain_approved_typed(grant, evidence, presented).await
    }

    async fn retain_approved_typed<E: From<PrivateOriginalError>>(self,
        grant: PrivateServiceGrantRef, evidence: PrivateSourceEvidence,
        presented: &(impl Fn() -> Result<(), E> + Sync)) -> Result<PrivateOriginalRef, E>
    {
        self.retain_approved_with_policy(grant, evidence, presented, None).await
    }
    async fn retain_approved_with_policy<E: From<PrivateOriginalError>>(self,
        grant: PrivateServiceGrantRef, evidence: PrivateSourceEvidence,
        presented: &(impl Fn() -> Result<(), E> + Sync), policy: Option<RecordCondition>,
    ) -> Result<PrivateOriginalRef, E> {
        let link = ApprovedTaskLink::single(self.task_header(), grant);
        self.retain_approved_selection(link, evidence, presented, policy).await
    }

    fn task_header(&self) -> ApprovedTaskHeader {
        ApprovedTaskHeader { parent: self.parent.clone(), acquisition: self.acquisition.clone(),
            original_task: self.task.clone(), before_ms: self.before_ms }
    }

    async fn retain_approved_selection<E: From<PrivateOriginalError>>(self,
        link: ApprovedTaskLink, evidence: PrivateSourceEvidence,
        presented: &(impl Fn() -> Result<(), E> + Sync), policy: Option<RecordCondition>,
    ) -> Result<PrivateOriginalRef, E> {
        link.validate().map_err(E::from)?;
        if *link != self.task_header() { return Err(E::from(PrivateOriginalError::Conflict)); }
        let source = self.source.clone();
        source.owner.retain_followup_source(&self.parent_association, || async {
            // Preserve the caller's typed currentness through the owner's
            // generic retained callback. No mutable error side channel exists.
            Ok(async {
            let (parent, snapshot) = self.current().await.map_err(E::from)?;
            presented()?;
            for grant in link.grants() {
                source.require_service_grant_issued(grant).await.map_err(E::from)?;
            }
            presented()?;
            let parent_record = source.decode_record(&parent, &self.parent).map_err(|_| E::from(PrivateOriginalError::Conflict))?;
            let event = PrivateAuthenticatedEvent {
                route: self.parent.route.clone(), original: self.fresh.clone(), occurrence: self.fresh.clone(),
                original_authentication: evidence.clone(), root_event: evidence,
                // Independent original contributors remain conjoined. A itself
                // is business correlation, not silently added to the new batch.
                contributors: parent_record.event.contributors,
                content_sha256: digest(self.instruction.as_bytes()),
            };
            let reference = PrivateOriginalRef { route: event.route.clone(), original: event.original.clone(),
                event_sha256: digest(&canonical_json(&event).map_err(|_| E::from(PrivateOriginalError::Conflict))?) };
            let key = source.key(&reference).map_err(|_| E::from(PrivateOriginalError::Denied))?;
            let data = encode(SourceRecord { custody_attempt: None,
                setup_request: None, configured_setup: None, service_setup_children: vec![],
                source: parent_record.source,
                event,
                operation_cancelled: false,
                service_attachment_failure: None,
                service_attachment_observation: None,
            service_attachment_member: None,
                final_association: None,
                approved_task: Some(link.clone()),
                service_admission: None,
            bound_service_input: None,
            }).map_err(|_| E::from(PrivateOriginalError::Conflict))?;
            let mut conditions = vec![RecordCondition { key: parent.key, revision: parent.revision,
                content_digest: parent.content_digest, claim: None, dispatch_claim: None }, snapshot.condition()];
            conditions.extend(policy);
            let saved = self.store.admit_conditionally(ConditionalAdmission {
                admission: AdmissionRequest { key: key.clone(), admission_key: key.id,
                    data, events: vec![], effects: vec![] },
                conditions,
                before_ms: i64::try_from(self.before_ms).map_err(|_| E::from(PrivateOriginalError::Conflict))?,
            }).await.map_err(|_| E::from(PrivateOriginalError::Unavailable))?;
            let saved = source.decode_record(saved.record(), &reference).map_err(|_| E::from(PrivateOriginalError::Conflict))?;
            if saved.approved_task.is_none() || saved.operation_cancelled {
                return Err(E::from(PrivateOriginalError::Denied));
            }
            Ok(reference)
            }.await)
        }).await.map_err(|error| E::from(match error {
            OriginalWriterError::Conflict => PrivateOriginalError::Conflict,
            OriginalWriterError::OperationCancelled | OriginalWriterError::WrongOwner => PrivateOriginalError::Denied,
            _ => PrivateOriginalError::Unavailable,
        }))?
    }
}

fn approval_evidence(record: &ApprovalRecord) -> Result<PrivateSourceEvidence, PrivateOriginalError> {
    Ok(PrivateSourceEvidence {
        resource_id: EvidenceId::new(record.approval_id.to_string()).map_err(|_| PrivateOriginalError::Conflict)?,
        revision: EvidenceId::new("settled-review-v1").map_err(|_| PrivateOriginalError::Conflict)?,
        sha256: digest(&canonical_json(record).map_err(|_| PrivateOriginalError::Conflict)?),
    })
}

/// Actual native acceptance retained with this source occurrence. It only stops
/// re-submission of that occurrence; it is not a permit for another invocation.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ServiceInputAdmission {
    pub(super) session_id: String,
    pub(super) input_id: String,
}
impl PrivateOriginals {
    /// Rejoin the canonical approved source to the exact retained parent A.
    /// A cold or replaced Console login cannot be reconstructed from its labels.
    pub(super) async fn console_login_for_approved_source(
        &self, reference: &PrivateOriginalRef,
    ) -> Result<Option<crate::private_host::ConsoleLoginHandle>, PrivateOriginalError> {
        let stored = self.store.get(&self.key(reference)?).await.map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let record = self.decode_record(&stored, reference)?;
        self.service_console_login(&record)
    }

    pub(super) fn service_console_login(&self, record: &SourceRecord)
        -> Result<Option<crate::private_host::ConsoleLoginHandle>, PrivateOriginalError>
    {
        let task = record.approved_task.as_ref().ok_or(PrivateOriginalError::Denied)?;
        let route = self.route(&task.parent.route)?;
        if record.source != SourceFacts::from_route(route, &record.source.ingress) {
            return Err(PrivateOriginalError::Conflict);
        }
        self.service_source_console_login(&task.parent, &task.original_task)
    }

    pub(super) async fn service_task_snapshot(&self, record: &SourceRecord) -> Result<SetupSnapshot, PrivateOriginalError> {
        let task = record.approved_task.as_ref().ok_or(PrivateOriginalError::Denied)?;
        let scope = SetupScope::new(Id::new(self.scope.realm.clone()).map_err(|_| PrivateOriginalError::Conflict)?,
            Id::new(self.scope.owner.clone()).map_err(|_| PrivateOriginalError::Conflict)?);
        let store = SetupStore::new(self.store.as_ref(), scope).map_err(|_| PrivateOriginalError::Unavailable)?;
        let snapshot = store.service_task_snapshot(&Id::new(task.acquisition.id.clone())
            .map_err(|_| PrivateOriginalError::Conflict)?, &task.original_task).await
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        if snapshot.key() != &task.acquisition || snapshot.request().original_work().work != task.parent.original
            || digest(snapshot.request().request().request.as_bytes()) != record.event.content_sha256
        { return Err(PrivateOriginalError::Conflict); }
        Ok(snapshot)
    }

    pub(super) async fn service_task_submission_current(&self, record: &SourceRecord)
        -> Result<bool, PrivateOriginalError>
    {
        use meerkat_core::turn_execution_authority::TurnTerminalOutcome;
        let task = record.approved_task.as_ref().ok_or(PrivateOriginalError::Denied)?;
        let _console_login = self.service_console_login(record)?;
        if record.operation_cancelled || self.approved_task_is_cancelled(record).await? { return Ok(false); }
        let snapshot = self.service_task_snapshot(record).await?;
        if let Some(selected) = snapshot.approved_service() {
            let reference = PrivateOriginalRef { route: record.event.route.clone(), original: record.event.original.clone(),
                event_sha256: digest(&canonical_json(&record.event).map_err(|_| PrivateOriginalError::Conflict)?) };
            if selected.source != self.key(&reference)? { return Ok(false); }
        }
        let now = u64::try_from(self.store.now_ms().await.map_err(store_error)?)
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        if now >= task.before_ms || matches!(snapshot.state().lifecycle_phase,
            toolkit_catalog::AcquisitionPhase::Cancelled | toolkit_catalog::AcquisitionPhase::Expired
            | toolkit_catalog::AcquisitionPhase::Failed | toolkit_catalog::AcquisitionPhase::Unavailable
            | toolkit_catalog::AcquisitionPhase::Ready)
        { return Ok(false); }
        let machine = self.machine()?;
        let prior = &task.original_task;
        if machine.run_turn_terminal(&prior.origin_session_id, prior.retained_work.run_id()).await
            != Some(TurnTerminalOutcome::Completed)
            || machine.current_run(&prior.origin_session_id).await.as_ref() == Some(prior.retained_work.run_id())
        { return Ok(false); }
        let lifetime = self.host_lifetime.get().and_then(Weak::upgrade).ok_or(PrivateOriginalError::Unavailable)?;
        lifetime.validate_current_selection().map_err(|_| PrivateOriginalError::Denied)?;
        Ok(true)
    }

    /// Preparation observation only. Native entry still needs current source
    /// and setup custody, the exact live actor boundary and its own authority.
    /// No server starts, configuration is written or input is submitted here.
    pub(crate) async fn observe_service_mcp_attachment(
        &self,
        reference: &PrivateOriginalRef,
        control: &meerkat_core::mcp_attachment_control::McpAttachmentControlRequest,
        binding: &meerkat_core::authorization::PreparedAuthorizationBinding,
    ) -> Result<(), PrivateOriginalError> {
        let stored = self.store.get(&self.key(reference)?).await.map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let record = self.decode_record(&stored, reference)?;
        let link = record.approved_task.as_ref().ok_or(PrivateOriginalError::Denied)?;
        let grant = self.service_control_grant(control)?;
        link.attachment_member(grant)?;
        if link.parent.route != reference.route || grant.route != reference.route
            || record.final_association.is_some() || record.service_admission.is_some()
            || control.session_id() != &link.original_task.origin_session_id
            || !self.service_task_submission_current(&record).await?
        { return Err(PrivateOriginalError::Denied); }
        let parent = self.store.get(&self.key(&link.parent)?).await.map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let parent = self.decode_record(&parent, &link.parent)?;
        let association = parent.final_association.as_ref().ok_or(PrivateOriginalError::Unavailable)?;
        self.observe_reviewed_mcp_attachment(&grant,
            &meerkat_authorization_contracts::work_association::OriginalWorkRef {
                authority: record.source.source_authority.clone(), work: record.event.original.clone(),
            }, &association.candidate().target, control, binding).await?;
        // Reopen canonical cancellation and setup state after the grant read.
        // This deliberately does not claim an atomic effect-entry observation.
        let current = self.store.get(&stored.key).await.map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let current = self.decode_record(&current, reference)?;
        if current.approved_task != record.approved_task || current.final_association.is_some()
            || current.service_admission.is_some() || !self.service_task_submission_current(&current).await?
        { return Err(PrivateOriginalError::Denied); }
        Ok(())
    }

    pub(super) fn service_input_host_current(&self) -> Result<(), PrivateOriginalError> {
        let lifetime = self.host_lifetime.get().and_then(Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        lifetime.validate_current_selection().map_err(|_| PrivateOriginalError::Denied)
    }

    /// One bounded page in the already joined acquisition recovery task.
    /// Historical or unavailable individual rows cannot stop other work.
    pub(crate) async fn step_service_inputs(self: &Arc<Self>, store: &Arc<SqliteStore>, after: Option<&str>, limit: u32)
        -> Result<ServiceInputRecoveryPage, PrivateOriginalError>
    {
        self.service_input_host_current()?;
        self.prune_service_attachments().await?;
        let page = self.store.list_records(&self.scope, after, limit).await.map_err(store_error)?;
        let next_after_id = if page.len() == limit as usize { page.last().map(|row| row.key.id.clone()) } else { None };
        let mut had_unavailable_records = false;
        for stored in page {
            self.service_input_host_current()?;
            if stored.data.schema != SOURCE_SCHEMA { continue; }
            had_unavailable_records |= self.step_service_input_with_attachment(store, &stored).await.is_err();
            // Only actual host closure ends the worker. An unavailable row is
            // revisited on a later sweep while this page's cursor advances.
            self.service_input_host_current()?;
        }
        Ok(ServiceInputRecoveryPage { next_after_id, had_unavailable_records })
    }

    async fn prepare_service_input(self: &Arc<Self>, stored: &StoredRecord)
        -> Result<ServiceInputStep, PrivateOriginalError>
    {
        let record = self.decode_stored(stored)?;
        let Some(task) = &record.approved_task else { return Ok(ServiceInputStep::Done); };
        let reference = PrivateOriginalRef { route: record.event.route.clone(), original: record.event.original.clone(),
            event_sha256: digest(&canonical_json(&record.event).map_err(|_| PrivateOriginalError::Conflict)?) };
        if let Some(ancestor) = self.cancelled_task_ancestor(&record).await? {
            self.record_task_group_cancelled(&ancestor).await?;
            self.retire_service_attachment(&reference, task).await;
            self.compensate_rejected_batch(&reference).await?;
            self.reject_service_continuation(&reference).await?;
            return Ok(ServiceInputStep::Done);
        }
        if record.operation_cancelled {
            self.record_task_group_cancelled(&reference).await?;
            self.retire_service_attachment(&reference, task).await;
            self.compensate_rejected_batch(&reference).await?;
            self.reject_service_continuation(&reference).await?;
            return Ok(ServiceInputStep::Done);
        }
        // Native admission stops resubmission, not inherited cancellation.
        // Missing historical routes remain isolated by the page owner.
        if record.service_admission.is_some() {
            // A prior CAS may have committed while its answer was lost. Only
            // this exact canonical read releases retained service admission.
            self.acknowledge_service_submission(&reference, stored)?;
            self.retire_service_attachment(&reference, task).await;
            self.settle_service_continuation(&record).await?;
            return Ok(ServiceInputStep::Done);
        }
        // Already admitted or uncertain native B keeps its exact pending
        // source guard. Never start attachment again during that reconciliation.
        let observed = self.reconcile_original_submission(&reference).await?;
        if observed.is_none() && record.final_association.is_some() {
            // The existing source row supplies the exact pre-submission Input;
            // native proves historical admission under its actual actor owners.
            // NotAdmitted and unavailable observations never fall through to submit.
            let Some(restored) = self.restore_service_admission(&reference).await? else {
                return Err(PrivateOriginalError::Unavailable);
            };
            // Every native phase remains historical admission only. Neither
            // this projection nor source promotion is task completion or resume.
            let _native_phase = restored.phase;
            self.record_service_admission(stored, &record, &reference, restored.admission).await?;
            return Ok(ServiceInputStep::Done);
        }
        if observed.is_none() && !self.service_task_submission_current(&record).await? {
            let snapshot = self.service_task_snapshot(&record).await?;
            if snapshot.approved_service().is_some_and(|selected| selected.source != stored.key) {
                return Err(PrivateOriginalError::Conflict);
            }
            if snapshot.approved_service().is_some() {
                self.reject_service_continuation(&reference).await?;
            }
            return Ok(ServiceInputStep::Done);
        }
        self.reserve_service_continuation(stored).await?;
        Ok(ServiceInputStep::Continue { record, reference, observed })
    }

    async fn service_instruction(
        &self, record: &SourceRecord, reference: &PrivateOriginalRef,
    ) -> Result<PrivateOriginalSubmit, PrivateOriginalError> {
        let snapshot = self.service_task_snapshot(record).await?;
        if !self.service_task_submission_current(record).await? {
            return Err(PrivateOriginalError::Denied);
        }
        Ok(PrivateOriginalSubmit { original: reference.clone(),
            content: snapshot.request().request().request.clone(),
            transport_authentication: record.event.original_authentication.clone() })
    }

    async fn step_service_input_with_attachment(
        self: &Arc<Self>, store: &Arc<SqliteStore>, stored: &StoredRecord,
    ) -> Result<(), PrivateOriginalError> {
        let result = self.enter_service_input_with_attachment(store, stored).await;
        self.settle_service_step_refusal(stored, result).await
    }

    async fn enter_service_input_with_attachment(
        self: &Arc<Self>, store: &Arc<SqliteStore>, stored: &StoredRecord,
    ) -> Result<(), PrivateOriginalError> {
        let ServiceInputStep::Continue { record, reference, observed } =
            self.prepare_service_input(stored).await? else { return Ok(()); };
        let task = record.approved_task.as_ref().ok_or(PrivateOriginalError::Denied)?;
        let observed = match observed {
            Some(observed) => observed,
            None => {
                if !self.ensure_service_attachment(store, &reference, task).await? { return Ok(()); }
                let submission = self.service_instruction(&record, &reference).await?;
                let Some(context) = self.recheck_service_attachments(&reference, task).await? else {
                    return Ok(());
                };
                // Existing native submission/source custody remains the only
                // B admission path. No attachment observation promotes B.
                self.submit_ready_service_set(submission, task.clone(), context).await?
            }
        };
        self.publish_service_input(stored, &record, &reference, observed).await
    }

    // These existing controls isolate native admission/source publication from
    // member attachment. Their ephemeral native fixture owns no live MCP
    // surface. Production always calls step_service_input_with_attachment.
    #[cfg(test)]
    async fn step_service_input(self: &Arc<Self>, stored: &StoredRecord)
        -> Result<(), PrivateOriginalError>
    {
        let result = async {
            let ServiceInputStep::Continue { record, reference, observed } =
                self.prepare_service_input(stored).await? else { return Ok(()); };
            let task = record.approved_task.as_ref().ok_or(PrivateOriginalError::Denied)?;
            let observed = match observed {
                Some(observed) => observed,
                None => self.submit_service_original(
                    self.service_instruction(&record, &reference).await?, task.singleton_grant()?.clone()).await?,
            };
            self.publish_service_input(stored, &record, &reference, observed).await
        }.await;
        self.settle_service_step_refusal(stored, result).await
    }

    async fn publish_service_input(
        &self, stored: &StoredRecord, record: &SourceRecord,
        reference: &PrivateOriginalRef, observed: PrivateOriginalSubmission,
    ) -> Result<(), PrivateOriginalError> {
        let PrivateOriginalSubmission::Admitted { session_id, input_id, source_settlement, .. } = observed else {
            return Err(PrivateOriginalError::Unavailable);
        };
        if matches!(source_settlement, PrivateSourceSettlement::PostAdmissionPublicationFailed) {
            return Err(PrivateOriginalError::Unavailable);
        }
        self.record_service_admission(stored, record, reference, ServiceInputAdmission { session_id, input_id }).await
    }

    async fn record_service_admission(
        &self, stored: &StoredRecord, record: &SourceRecord,
        reference: &PrivateOriginalRef, accepted: ServiceInputAdmission,
    ) -> Result<(), PrivateOriginalError> {
        let task = record.approved_task.as_ref().ok_or(PrivateOriginalError::Denied)?;
        if accepted.session_id != task.original_task.origin_session_id.to_string() { return Err(PrivateOriginalError::Conflict); }
        let current = self.store.get(&stored.key).await.map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let mut current_record = self.decode_record(&current, &reference)?;
        if current_record.approved_task != record.approved_task
            || (record.final_association.is_some() && current_record.final_association != record.final_association)
            || (record.bound_service_input.is_some() && current_record.bound_service_input != record.bound_service_input)
        { return Err(PrivateOriginalError::Conflict); }
        if let Some(prior) = &current_record.service_admission {
            if prior != &accepted { return Err(PrivateOriginalError::Conflict); }
            self.acknowledge_service_submission(reference, &current)?;
            self.retire_service_attachment(reference, task).await;
            self.settle_service_continuation(&current_record).await?;
            return Ok(());
        }
        current_record.service_admission = Some(accepted);
        let saved = self.store.compare_and_swap(Transition { key: current.key, expected_revision: current.revision,
            expected_digest: Some(current.content_digest), claim: None, data: encode(current_record)?,
            events: vec![], effects: vec![] }).await.map_err(store_error)?;
        self.acknowledge_service_submission(reference, &saved)?;
        self.retire_service_attachment(reference, task).await;
        self.settle_service_continuation(&self.decode_record(&saved, reference)?).await?;
        Ok(())
    }
}


enum ServiceInputStep {
    Done,
    Continue { record: SourceRecord, reference: PrivateOriginalRef,
        observed: Option<PrivateOriginalSubmission> },
}

pub(crate) struct ServiceInputRecoveryPage {
    pub next_after_id: Option<String>,
    pub had_unavailable_records: bool,
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "service_approved_input/selection_tests.rs"]
mod selection_tests;

#[cfg(test)]
#[path = "service_approved_input/admission_tests.rs"]
mod admission_tests;
