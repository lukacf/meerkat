//! Read-only projection from the existing source and acquisition owners.
//! Native request authentication and member visibility are checked by the
//! installed Console handler. Neither a principal string nor a task reference
//! from HTTP input is accepted here as an authenticated viewer.

use super::*;
use meerkat_core::{MobMemberBinding, PrincipalKind};
use meerkat_runtime::meerkat_machine::{AuthenticatedOriginalSource, OriginalDisposition};
use meerkat_toolkit::{host_lifetime::NativeHostLifetime, request::NativeOriginalTask};

const MAX_STATUS_RECORDS: usize = 1024;
const MAX_STATUS_TASKS: usize = 64;

/// One captured slice of the existing canonical record order. Continuation is
/// a read position, never a source grant or a cross-request snapshot.
pub(crate) struct SourceStatusPage {
    pub(crate) rows: Vec<StoredRecord>,
    pub(crate) next_after: Option<String>,
}


pub(crate) struct ConsoleTaskStatus {
    pub(crate) projection: serde_json::Value,
    pub(crate) visible_members: Vec<meerkat_mobkit::identity_first::AgentIdentity>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SourcePublicationStatus {
    Unobserved,
    Pending,
    Published,
    PublicationFailed,
}

impl PrivateOriginals {
    pub(crate) async fn source_status_page(&self, after: Option<&str>)
        -> Result<SourceStatusPage, PrivateOriginalError>
    {
        self.source_status_page_from(self.store.as_ref(), after).await
    }

    pub(crate) async fn source_status_page_from(&self, store: &dyn ToolkitStore, after: Option<&str>)
        -> Result<SourceStatusPage, PrivateOriginalError>
    {
        self.status_page_records(store, after, false).await
    }

    pub(crate) async fn source_status_page_with_recovery(&self, after: Option<&str>)
        -> Result<SourceStatusPage, PrivateOriginalError>
    {
        self.status_page_records(self.store.as_ref(), after, true).await
    }

    pub(crate) async fn status_page_records(&self, store: &dyn ToolkitStore, after: Option<&str>, recovery: bool)
        -> Result<SourceStatusPage, PrivateOriginalError>
    {
        let lifetime = self.status_lifetime()?;
        let mut cursor = after.map(str::to_owned);
        let mut scanned = 0;
        let mut rows = Vec::new();
        while scanned < MAX_STATUS_RECORDS && rows.len() < MAX_STATUS_TASKS {
            let batch = store.list_records(&self.scope, cursor.as_deref(),
                (MAX_STATUS_RECORDS - scanned).min(64) as u32).await.map_err(store_error)?;
            lifetime.validate_current_selection().map_err(|_| PrivateOriginalError::Denied)?;
            if batch.is_empty() { return Ok(SourceStatusPage { rows, next_after: None }); }
            for row in batch {
                if row.key.scope() != self.scope || cursor.as_ref().is_some_and(|last| row.key.id.as_str() <= last.as_str()) {
                    return Err(PrivateOriginalError::Conflict);
                }
                // Advance only through the processed row. A fetched tail is
                // read again on Next, including when this row fills the page.
                cursor = Some(row.key.id.clone());
                scanned += 1;
                if row.data.schema == SOURCE_SCHEMA || recovery
                    && crate::installed_host::operator_setup::service_recovery::is_parent_record(&row) { rows.push(row); }
                if scanned == MAX_STATUS_RECORDS || rows.len() == MAX_STATUS_TASKS { break; }
            }
        }
        let lookahead = store.list_records(&self.scope, cursor.as_deref(), 1).await.map_err(store_error)?;
        lifetime.validate_current_selection().map_err(|_| PrivateOriginalError::Denied)?;
        if lookahead.len() > 1 || lookahead.iter().any(|row| row.key.scope() != self.scope
            || cursor.as_ref().is_some_and(|last| row.key.id.as_str() <= last.as_str())) {
            return Err(PrivateOriginalError::Conflict);
        }
        Ok(SourceStatusPage { rows, next_after: (!lookahead.is_empty()).then_some(cursor)
            .flatten() })
    }

    /// The eventual native route supplies a verified qualified human and a
    /// borrowed check of that request's member access. This component check
    /// does not refresh a captured native AccessView. The actual HTTP handler
    /// must resolve the same authenticated human and fresh native access after
    /// this future returns, before serializing any of the projection.
    /// This crate-private port neither authenticates a human nor issues access.
    pub(crate) async fn service_task_status_projection(
        &self,
        human: &PrincipalRef,
        current_member_access: &(
             impl Fn(&MobMemberBinding) -> Result<bool, PrivateOriginalError> + Sync
         ),
    ) -> Result<serde_json::Value, PrivateOriginalError> {
        self.service_task_status_projection_from(self.store.as_ref(), human, current_member_access)
            .await
    }

    async fn service_task_status_projection_from(
        &self,
        store: &dyn ToolkitStore,
        human: &PrincipalRef,
        current_member_access: &(
             impl Fn(&MobMemberBinding) -> Result<bool, PrivateOriginalError> + Sync
         ),
    ) -> Result<serde_json::Value, PrivateOriginalError> {
        self.service_task_status_projection_rows(store, human, current_member_access, None).await
    }

    async fn service_task_status_projection_rows(
        &self, store: &dyn ToolkitStore, human: &PrincipalRef,
        current_member_access: &(impl Fn(&MobMemberBinding) -> Result<bool, PrivateOriginalError> + Sync),
        rows: Option<&[StoredRecord]>,
    ) -> Result<serde_json::Value, PrivateOriginalError> {
        let page_only = rows.is_some();
        let mut selected = rows.map(<[StoredRecord]>::to_vec);
        if human.kind != PrincipalKind::Human || human.validate_qualified().is_err() {
            return Err(PrivateOriginalError::Denied);
        }
        let lifetime = self.status_lifetime()?;
        let mut visible_routes = vec![];
        for route in self.routes()? {
            if !same_source_human(human, &route.requester, route.represented_subject.as_ref()) {
                continue;
            }
            let member = self.status_route(&lifetime, human, route)?;
            if current_member_access(&member)? {
                visible_routes.push((route, member));
            }
        }
        if visible_routes.is_empty() {
            return Err(PrivateOriginalError::Denied);
        }
        let mut tasks = vec![];
        let mut final_checks = vec![];
        let mut records_read = 0;
        let mut after = None;
        loop {
            self.status_current(&lifetime, human, &visible_routes, current_member_access)?;
            let limit = (MAX_STATUS_RECORDS - records_read + 1).min(64) as u32;
            let page = if page_only {
                match selected.take() { Some(rows) => rows, None => break }
            } else {
                store.list_records(&self.scope, after.as_deref(), limit).await.map_err(store_error)?
            };
            self.status_current(&lifetime, human, &visible_routes, current_member_access)?;
            if page.is_empty() {
                break;
            }
            records_read += page.len();
            // Refuse an incomplete view rather than silently describe a
            // truncated inventory as complete. No cursor is caller authority.
            if records_read > MAX_STATUS_RECORDS {
                return Err(PrivateOriginalError::Unavailable);
            }
            after = page.last().map(|row| row.key.id.clone());
            for stored in page {
                if stored.data.schema != SOURCE_SCHEMA {
                    continue;
                }
                let record = self.decode_stored(&stored)?;
                // Source subjects are retained by the authenticated source
                // owner. Agent visibility alone never discloses another task.
                if !same_source_human(
                    human,
                    &record.source.requester,
                    record.source.represented_subject.as_ref(),
                ) {
                    continue;
                }
                let Some((route, member)) = visible_routes
                    .iter()
                    .find(|(route, _)| route.id == record.event.route)
                else {
                    continue;
                };
                let Some(link) = &record.approved_task else {
                    continue;
                };
                if link.original_task.member.as_ref() != Some(member) {
                    return Err(PrivateOriginalError::Conflict);
                }
                let reference = PrivateOriginalRef {
                    route: route.id.clone(),
                    original: record.event.original.clone(),
                    event_sha256: digest(
                        &canonical_json(&record.event)
                            .map_err(|_| PrivateOriginalError::Conflict)?,
                    ),
                };
                self.decode_record(&stored, &reference)?;
                let parent = store
                    .get(&self.key(&link.parent)?)
                    .await
                    .map_err(store_error)?
                    .ok_or(PrivateOriginalError::Unavailable)?;
                self.status_current(&lifetime, human, &visible_routes, current_member_access)?;
                let parent_cancelled = self.status_parent(human, member, &record, &parent, &link.original_task)?;
                // Reuse the acquisition owner's canonical decoder and exact
                // task match. Do not reinterpret arbitrary stored JSON here.
                let snapshot = self.service_task_snapshot(&record).await?;
                self.status_current(&lifetime, human, &visible_routes, current_member_access)?;
                if record.operation_cancelled || parent_cancelled || Self::status_task_terminal(&snapshot) {
                    self.status_terminal_records_settled(store, &stored, &parent, &record, &snapshot).await?;
                    continue;
                }
                if tasks.len() >= MAX_STATUS_TASKS {
                    return Err(PrivateOriginalError::Unavailable);
                }
                self.status_task_open(&snapshot)?;
                // A source reference is correlation, not durable read
                // permission. Reopen the same source/parent/task owners after
                // the awaited acquisition read before retaining its prose.
                self.status_records_current(store, &stored, &parent, &record, &snapshot)
                    .await?;
                self.status_current(&lifetime, human, &visible_routes, current_member_access)?;
                let status = self.source_publication_status(&reference, &record)?;
                let (tone, text, next) = if status == SourcePublicationStatus::Unobserved
                    && snapshot.state().lifecycle_phase == toolkit_catalog::AcquisitionPhase::Preparing
                    && !snapshot.approved_service_was_admitted()
                    && snapshot.approved_service() == Some(&self.service_selection(&stored, &record)?)
                {
                    // Project the exact canonical reservation already read above.
                    // It does not confirm connection readiness or delivery of B.
                    (
                        "pending",
                        "Access approved; setup handoff pending",
                        "Setup retained the approved instruction. Connection readiness and delivery to your agent are not yet confirmed.",
                    )
                } else {
                    status.presentation()
                };
                tasks.push(serde_json::json!({
                    "id": digest(&canonical_json(&(link.acquisition.clone(), &link.original_task, &reference))
                        .map_err(|_| PrivateOriginalError::Conflict)?),
                    "title": "Service setup for your task",
                    "application": self.scope.owner,
                    "requester": "You",
                    "reason": format!("Approved follow-up instruction: {}", snapshot.request().request().request),
                    "phase": { "tone": tone, "text": text },
                    "nextAction": next,
                    "roles": [], "userActions": [], "controls": [],
                }));
                final_checks.push((stored, parent, record, snapshot));
            }
        }
        // Entries read early in the bounded page also undergo current source
        // and task checks after all initial reads. This is not an atomic
        // revocation transaction across response delivery.
        for (stored, parent, record, snapshot) in &final_checks {
            self.status_records_current(store, stored, parent, record, snapshot)
                .await?;
            self.status_current(&lifetime, human, &visible_routes, current_member_access)?;
        }
        self.status_current(&lifetime, human, &visible_routes, current_member_access)?;
        let failed = tasks
            .iter()
            .any(|task| task["phase"]["tone"] == "attention");
        Ok(serde_json::json!({
            "scope": digest(&canonical_json(&(self.scope.clone(), human)).map_err(|_| PrivateOriginalError::Conflict)?),
            // The panel assigns request ordering within its own mount. This
            // value is not a source revision or a business completion version.
            "revision": 0, "observedAt": "this request",
            "announcement": if failed { "Setup handoff needs attention. The follow-up was accepted, but setup could not confirm the handoff." }
                else { "Setup handoff status updated. Your agent reports the task result separately." },
            "tasks": tasks, "connections": [], "frontends": [], "packages": [],
        }))
    }

    fn status_task_terminal(snapshot: &meerkat_toolkit::setup::SetupSnapshot) -> bool {
        matches!(snapshot.state().lifecycle_phase,
            toolkit_catalog::AcquisitionPhase::Cancelled | toolkit_catalog::AcquisitionPhase::Expired)
    }

    fn status_task_open(
        &self,
        snapshot: &meerkat_toolkit::setup::SetupSnapshot,
    ) -> Result<(), PrivateOriginalError> {
        if Self::status_task_terminal(snapshot) {
            return Err(PrivateOriginalError::Denied);
        }
        Ok(())
    }

    async fn status_terminal_records_settled(
        &self,
        store: &dyn ToolkitStore,
        source: &StoredRecord,
        parent: &StoredRecord,
        record: &SourceRecord,
        task: &meerkat_toolkit::setup::SetupSnapshot,
    ) -> Result<(), PrivateOriginalError> {
        // Follow the same bounded canonical parent chain as cancellation
        // recovery. A partially persisted outer cancellation is not settled
        // merely because this acquisition was already terminal.
        let mut records = vec![source.clone(), parent.clone()];
        let mut next = self.decode_stored(parent)?.approved_task.map(|link| link.parent.clone());
        while let Some(reference) = next {
            let key = self.key(&reference)?;
            if records.iter().any(|before| before.key == key) || records.len() > MAX_STATUS_TASKS {
                return Err(PrivateOriginalError::Conflict);
            }
            let stored = store.get(&key).await.map_err(store_error)?
                .ok_or(PrivateOriginalError::Unavailable)?;
            let ancestor = self.decode_record(&stored, &reference)?;
            if ancestor.source != record.source { return Err(PrivateOriginalError::Conflict); }
            next = ancestor.approved_task.map(|link| link.parent.clone());
            records.push(stored);
        }
        if self.service_task_snapshot(record).await?.condition() != task.condition() {
            return Err(PrivateOriginalError::Unavailable);
        }
        let mut child_cancelled = true;
        let mut partial_cancellation = false;
        for before in &records {
            let current = store.get(&before.key).await.map_err(store_error)?
                .ok_or(PrivateOriginalError::Unavailable)?;
            let prior = self.decode_stored(before)?;
            let row = self.decode_stored(&current)?;
            // Preserve refusal when this read already observed an open source
            // or parent and then sees its canonical cancellation.
            if row.operation_cancelled && !prior.operation_cancelled {
                return Err(PrivateOriginalError::Denied);
            }
            if &current != before { return Err(PrivateOriginalError::Unavailable); }
            partial_cancellation |= row.operation_cancelled && !child_cancelled;
            child_cancelled = row.operation_cancelled;
            let namespace = meerkat_authorization_contracts::work_association::QualifiedIngressNamespace {
                realm: row.source.realm.clone(), ingress: row.source.ingress.clone(),
                occurrence_scope: row.event.occurrence.clone(),
            };
            let original = meerkat_authorization_contracts::work_association::OriginalWorkRef {
                authority: row.source.source_authority.clone(), work: row.event.original.clone(),
            };
            self.owner.check_status_cancellation(&namespace, &original, row.operation_cancelled,
                row.final_association.as_ref(), &current)
                .map_err(|error| match error {
                    OriginalWriterError::Conflict => PrivateOriginalError::Conflict,
                    OriginalWriterError::WrongOwner => PrivateOriginalError::Denied,
                    _ => PrivateOriginalError::Unavailable,
                })?;
        }
        if partial_cancellation { return Err(PrivateOriginalError::Unavailable); }
        Ok(())
    }

    async fn status_records_current(
        &self,
        store: &dyn ToolkitStore,
        source: &StoredRecord,
        parent: &StoredRecord,
        record: &SourceRecord,
        task: &meerkat_toolkit::setup::SetupSnapshot,
    ) -> Result<(), PrivateOriginalError> {
        let current_task = self.service_task_snapshot(record).await?;
        self.status_task_open(&current_task)?;
        // Acquisition revisions also advance for normal setup progress. Read
        // disclosure depends on the same immutable request and current open
        // lifecycle, not an unchanged mutation/CAS precondition.
        if current_task.key() != task.key()
            || current_task.request().request() != task.request().request()
            || current_task.request().host() != task.request().host()
            || current_task.request().evidence() != task.request().evidence()
        {
            return Err(PrivateOriginalError::Unavailable);
        }
        if self.approved_task_is_cancelled(record).await? {
            return Err(PrivateOriginalError::Denied);
        }
        // Reopen the same canonical source after the awaited task read. First
        // binding and admission append owner facts during normal progress;
        // they do not change the approved instruction or its source identity.
        // Previously observed bindings and receipts must remain exact.
        let source_reference = PrivateOriginalRef {
            route: record.event.route.clone(),
            original: record.event.original.clone(),
            event_sha256: digest(&canonical_json(&record.event)
                .map_err(|_| PrivateOriginalError::Conflict)?),
        };
        // Preserve the parent's already-validated reference, including an
        // equivalent configured route alias for its historical source row.
        let parent_reference = &record.approved_task.as_ref()
            .ok_or(PrivateOriginalError::Conflict)?.parent;
        let mut current_records = Vec::with_capacity(2);
        for (before, reference) in [(source, &source_reference), (parent, parent_reference)] {
            let prior = self.decode_stored(before)?;
            let current = store
                .get(&before.key)
                .await
                .map_err(store_error)?
                .ok_or(PrivateOriginalError::Unavailable)?;
            // This validates any newly retained association and bound Input
            // against the exact configured source, not merely its JSON shape.
            let current = self.decode_record(&current, reference)?;
            if current.operation_cancelled {
                return Err(PrivateOriginalError::Denied);
            }
            if current.source != prior.source || current.event != prior.event
                || current.setup_request != prior.setup_request
                || current.configured_setup != prior.configured_setup
                || current.approved_task != prior.approved_task
                || (prior.final_association.is_some() && current.final_association != prior.final_association)
                || (prior.bound_service_input.is_some() && current.bound_service_input != prior.bound_service_input)
                || (prior.service_admission.is_some() && current.service_admission != prior.service_admission)
            {
                return Err(PrivateOriginalError::Unavailable);
            }
            current_records.push(current);
        }
        // A currently retained source can already be cancellation-pending
        // before its canonical write finishes. Check the reopened association
        // against that existing native owner, without creating new custody.
        // This does not promise atomic revocation through network delivery.
        for current in current_records {
            let held = self
                .retained
                .lock()
                .map_err(|_| PrivateOriginalError::Unavailable)?
                .iter()
                .find(|held| {
                    held.reference.route == current.event.route
                        && held.reference.original == current.event.original
                })
                .cloned();
            if let Some(held) = held {
                if held.reference.event_sha256
                    != digest(
                        &canonical_json(&current.event)
                            .map_err(|_| PrivateOriginalError::Conflict)?,
                    )
                    || current.final_association.as_ref() != Some(held.original.association())
                {
                    return Err(PrivateOriginalError::Unavailable);
                }
                let candidate = held.original.association().candidate();
                if !matches!(
                    self.owner
                        .original_disposition(
                            &candidate.ingress_namespace,
                            &candidate.original_work
                        )
                        .map_err(|_| PrivateOriginalError::Unavailable)?,
                    OriginalDisposition::Active
                ) {
                    return Err(PrivateOriginalError::Denied);
                }
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) async fn service_task_status_with_read_probe(
        &self,
        store: &dyn ToolkitStore,
        human: &PrincipalRef,
        current_member_access: &(
             impl Fn(&MobMemberBinding) -> Result<bool, PrivateOriginalError> + Sync
         ),
    ) -> Result<serde_json::Value, PrivateOriginalError> {
        // Test-only I/O fault injection around the same canonical store and
        // same source owner. Production always reads self.store above.
        self.service_task_status_projection_from(store, human, current_member_access)
            .await
    }

    #[cfg(test)]
    pub(super) async fn service_task_status_page_with_read_probe(
        &self, store: &dyn ToolkitStore, human: &PrincipalRef,
        current_member_access: &(impl Fn(&MobMemberBinding) -> Result<bool, PrivateOriginalError> + Sync),
        rows: &[StoredRecord],
    ) -> Result<serde_json::Value, PrivateOriginalError> {
        self.service_task_status_projection_rows(store, human, current_member_access, Some(rows)).await
    }

    pub(crate) fn status_lifetime(&self) -> Result<Arc<NativeHostLifetime>, PrivateOriginalError> {
        let lifetime = self
            .host_lifetime
            .get()
            .and_then(Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        lifetime
            .validate_current_selection()
            .map_err(|_| PrivateOriginalError::Denied)?;
        Ok(lifetime)
    }

    pub(crate) fn status_route(
        &self,
        lifetime: &NativeHostLifetime,
        human: &PrincipalRef,
        route: &PrivateOriginalRoute,
    ) -> Result<MobMemberBinding, PrivateOriginalError> {
        let selected = lifetime.selected_policy();
        let native = selected
            .native_host
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?;
        let compiled = native
            .routes
            .iter()
            .find(|entry| entry.id == route.id)
            .ok_or(PrivateOriginalError::Denied)?;
        let (member, requester, actor) = compiled
            .source_binding(selected)
            .map_err(|_| PrivateOriginalError::Denied)?;
        let target = native
            .members
            .iter()
            .find(|entry| &entry.member == member)
            .ok_or(PrivateOriginalError::Denied)?;
        if route.realm != native.realm
            || !same_source_human(human, requester, compiled.represented_subject.as_ref())
            || route.requester != *requester
            || route.ingress_actor != *actor
            || route.logical_executor != compiled.logical_executor
            || route.source_authority != compiled.source_authority
            || route.authentication_domain != compiled.authentication_domain
            || route.event_domain != compiled.event_domain
            || route.grant_audience != target.audience
        {
            return Err(PrivateOriginalError::Denied);
        }
        Ok(member.clone())
    }

    fn status_current(
        &self,
        lifetime: &NativeHostLifetime,
        human: &PrincipalRef,
        routes: &[(&PrivateOriginalRoute, MobMemberBinding)],
        current_member_access: &(
             impl Fn(&MobMemberBinding) -> Result<bool, PrivateOriginalError> + Sync
         ),
    ) -> Result<(), PrivateOriginalError> {
        lifetime
            .validate_current_selection()
            .map_err(|_| PrivateOriginalError::Denied)?;
        for (route, member) in routes {
            if self.status_route(lifetime, human, route)? != *member
                || !current_member_access(member)?
            {
                return Err(PrivateOriginalError::Denied);
            }
        }
        Ok(())
    }

    fn status_parent(
        &self,
        human: &PrincipalRef,
        member: &MobMemberBinding,
        child: &SourceRecord,
        stored: &StoredRecord,
        task: &NativeOriginalTask,
    ) -> Result<bool, PrivateOriginalError> {
        let link = child
            .approved_task
            .as_ref()
            .ok_or(PrivateOriginalError::Conflict)?;
        let parent = self.decode_record(stored, &link.parent)?;
        let association = parent
            .final_association
            .as_ref()
            .ok_or(PrivateOriginalError::Unavailable)?;
        // The retained native identity uses the owner's JSON encoding, not
        // InputAuthorityAssociation's distinct binary canonical encoding.
        let association_digest =
            digest(&serde_json::to_vec(association).map_err(|_| PrivateOriginalError::Conflict)?);
        if parent.source != child.source
            || !same_source_human(
                human,
                &parent.source.requester,
                parent.source.represented_subject.as_ref(),
            )
            || task.member.as_ref() != Some(member)
            || association.candidate().original_work.work != link.parent.original
            || association.candidate().target.context.as_str() != task.origin_session_id.to_string()
            || task.retained_work.runtime_id()
                != meerkat_runtime::LogicalRuntimeId::for_session(&task.origin_session_id)
                    .to_string()
            || !task
                .retained_work
                .contributors()
                .iter()
                .any(|current| current.association_digest == association_digest)
        {
            return Err(PrivateOriginalError::Conflict);
        }
        Ok(parent.operation_cancelled)
    }

    fn source_publication_status(
        &self,
        reference: &PrivateOriginalRef,
        record: &SourceRecord,
    ) -> Result<SourcePublicationStatus, PrivateOriginalError> {
        let task = record
            .approved_task
            .as_ref()
            .ok_or(PrivateOriginalError::Conflict)?;
        let held = self
            .retained
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?
            .iter()
            .find(|held| &held.reference == reference)
            .cloned();
        let Some(held) = held else {
            return Ok(SourcePublicationStatus::Unobserved);
        };
        let pending = held
            .unresolved
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        if pending.len() > 1 {
            return Err(PrivateOriginalError::Conflict);
        }
        if let Some(pending) = pending.first() {
            return pending.publication_status(&task.original_task.origin_session_id, task.grants().first().copied().ok_or(PrivateOriginalError::Conflict)?);
        }
        let candidate = held.original.association().candidate();
        Ok(
            if self
                .owner
                .admitted_original_basis(&candidate.ingress_namespace, &candidate.original_work)
                .map_err(|_| PrivateOriginalError::Unavailable)?
                .is_some()
            {
                SourcePublicationStatus::Published
            } else {
                SourcePublicationStatus::Unobserved
            },
        )
    }
}

/// Equality with an already authenticated qualified human is necessary source
/// disclosure evidence, not authentication or grant-issuance authority. The
/// native verified carrier is the only producer of the HTTP viewer value.
pub(crate) fn same_source_human(
    human: &PrincipalRef,
    requester: &PrincipalRef,
    represented: Option<&PrincipalRef>,
) -> bool {
    human.kind == PrincipalKind::Human
        && human.validate_qualified().is_ok()
        && requester == human
        && represented.is_none_or(|subject| subject == human)
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerkat_core::auth::principal::TrustDomainId;

    #[test]
    fn qualified_direct_human_allows_absent_or_same_subject_only() {
        // These are component identity values. Constructing them does not
        // authenticate a Console request or stand in for its native carrier.
        let human = PrincipalRef::in_domain(
            PrincipalKind::Human,
            "subject-7",
            TrustDomainId::new("oidc-issuer:https://issuer.example").unwrap(),
        )
        .unwrap();
        let other_issuer = PrincipalRef::in_domain(
            PrincipalKind::Human,
            "subject-7",
            TrustDomainId::new("oidc-issuer:https://other.example").unwrap(),
        )
        .unwrap();
        let service = PrincipalRef::in_domain(
            PrincipalKind::ServiceAccount,
            "subject-7",
            TrustDomainId::new("oidc-issuer:https://issuer.example").unwrap(),
        )
        .unwrap();
        let unqualified = PrincipalRef::new(PrincipalKind::Human, "subject-7").unwrap();
        assert!(same_source_human(&human, &human, None));
        assert!(same_source_human(&human, &human, Some(&human)));
        assert!(!same_source_human(&human, &human, Some(&other_issuer)));
        assert!(!same_source_human(&human, &other_issuer, Some(&human)));
        assert!(!same_source_human(&service, &service, None));
        assert!(!same_source_human(&unqualified, &unqualified, None));
    }
}

impl SourcePublicationStatus {
    fn presentation(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Self::Unobserved => (
                "pending",
                "Setup handoff status has not been reported",
                "There is no current handoff observation. Refreshing this view does not send the follow-up again.",
            ),
            Self::Pending => (
                "pending",
                "Waiting for the setup handoff outcome",
                "Setup is retaining the follow-up while its outcome is pending. This view will not send it again.",
            ),
            Self::Published => (
                "pending",
                "Setup handoff confirmed",
                "Your agent reports the task result separately. This status does not mean the task is complete.",
            ),
            Self::PublicationFailed => (
                "attention",
                "Setup handoff needs attention",
                "The follow-up was accepted, but setup could not confirm the handoff. It will not send the same input again. Your agent reports the task result separately.",
            ),
        }
    }
}

impl PrivateOriginals {
    pub(crate) async fn console_status_native(&self,
        viewer: &meerkat_mobkit::http_console::ConsoleViewerContext,
        current: &(impl Fn() -> Option<meerkat_mobkit::http_console::ConsoleViewerContext> + Sync),
    ) -> Result<serde_json::Value, PrivateOriginalError> {
        self.console_status_native_rows(viewer, current, None).await.map(|read| read.projection)
    }

    pub(crate) async fn console_status_native_page(&self,
        viewer: &meerkat_mobkit::http_console::ConsoleViewerContext,
        current: &(impl Fn() -> Option<meerkat_mobkit::http_console::ConsoleViewerContext> + Sync),
        rows: &[StoredRecord],
    ) -> Result<ConsoleTaskStatus, PrivateOriginalError> {
        self.console_status_native_rows(viewer, current, Some(rows)).await
    }

    async fn console_status_native_rows(&self,
        viewer: &meerkat_mobkit::http_console::ConsoleViewerContext,
        current: &(impl Fn() -> Option<meerkat_mobkit::http_console::ConsoleViewerContext> + Sync),
        rows: Option<&[StoredRecord]>,
    ) -> Result<ConsoleTaskStatus, PrivateOriginalError> {
        let human = viewer.human.as_ref().ok_or(PrivateOriginalError::Denied)?;
        let principal = human.principal().map_err(|_| PrivateOriginalError::Denied)?;
        human.current().map_err(|_| PrivateOriginalError::Denied)?;
        let lifetime = self.status_lifetime()?;
        let runtime = self.bound_identity_runtime()?;
        let mut identities = Vec::new();
        for route in self.routes()? {
            if !same_source_human(&principal, &route.requester, route.represented_subject.as_ref()) { continue; }
            let member = self.status_route(&lifetime, &principal, route)?;
            let identity = runtime.owned_identity_for_member_alias(route.grant_target.as_str()).await
                .ok_or(PrivateOriginalError::Unavailable)?;
            human.current().map_err(|_| PrivateOriginalError::Denied)?;
            self.status_lifetime()?;
            identities.push((member, identity));
        }
        let access = |member: &MobMemberBinding| -> Result<bool, PrivateOriginalError> {
            human.current().map_err(|_| PrivateOriginalError::Denied)?;
            let identity = identities.iter().find(|(selected, _)| selected == member)
                .map(|(_, identity)| identity).ok_or(PrivateOriginalError::Unavailable)?;
            let fresh = current().ok_or(PrivateOriginalError::Denied)?;
            if fresh.human.as_ref().and_then(|human| human.principal().ok()).as_ref() != Some(&principal) { return Err(PrivateOriginalError::Denied); }
            Ok(fresh.access_view.as_ref().is_none_or(|view| !view.enforced() || view.can_view_agent(identity.as_str())))
        };
        // Retain the exact visibility set used by this read. A later grant of
        // visibility waits for another read; losing it refuses this projection
        // rather than dropping the identity from its final disclosure check.
        let mut selected = Vec::new();
        for (member, identity) in &identities {
            if access(member)? { selected.push((member.clone(), identity.clone())); }
        }
        let selected_access = |member: &MobMemberBinding| {
            if selected.iter().any(|(held, _)| held == member) { access(member) } else { Ok(false) }
        };
        let projection = self.service_task_status_projection_rows(self.store.as_ref(), &principal, &selected_access, rows).await?;
        for (member, _) in &selected {
            if !access(member)? { return Err(PrivateOriginalError::Denied); }
        }
        Ok(ConsoleTaskStatus { projection, visible_members:selected.into_iter().map(|(_, identity)| identity).collect() })
    }
}
