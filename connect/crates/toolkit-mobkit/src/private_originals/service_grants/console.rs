//! Projection and exact-leaf mutation of this application's retained access.
//! Historical source facts attribute the record; only the installed host's
//! explicit policy and current protected login permit a fresh revoke action.
use super::*;
use crate::private_originals::source_status::same_source_human;
use meerkat_authorization::grants::ExpectedRevokeRefusal;
use meerkat_authorization::publication::LocalPublicationStamp;
use meerkat_core::MobMemberBinding;
use meerkat_mobkit::http_console::ConsoleViewerContext;

// Legacy whole-view calls remain bounded. The mounted Console supplies one
// shared canonical source page to both task and access decoders.
const MAX_RECORDS: usize = 1024;
const MAX_ACCESS: usize = 64;

pub(crate) struct ApplicationAccessRead {
    pub human: PrincipalRef,
    pub publication: LocalPublicationStamp,
    pub rows: Vec<ApplicationAccessRow>,
}
impl ApplicationAccessRead {
    /// Final disclosure check against the caller's freshly resolved native
    /// view. This predicate conveys visibility only, never mutation authority.
    pub(crate) fn check_visible(&self, permits: impl Fn(&MobMemberBinding) -> bool)
        -> Result<(), PrivateOriginalError>
    {
        self.publication.check_current().map_err(|_| PrivateOriginalError::Unavailable)?;
        if self.rows.iter().any(|row| !permits(&row.member)) { return Err(PrivateOriginalError::Denied); }
        Ok(())
    }
}
pub(crate) struct ApplicationAccessRow {
    pub id: String,
    pub projection: serde_json::Value,
    pub member: MobMemberBinding,
    source: StoredRecord,
    source_ref: PrivateOriginalRef,
    access: StoredRecord,
    reference: PrivateServiceGrantRef,
    record: AccessRecord,
}

impl PrivateOriginals {
    /// Use exactly the existing native member lookup and current viewer. This
    /// is visibility only; it does not choose the revoke policy or issuer.
    pub(crate) async fn console_access_members(
        &self, viewer: &ConsoleViewerContext,
        current: &(impl Fn() -> Option<ConsoleViewerContext> + Sync),
    ) -> Result<Vec<(MobMemberBinding, meerkat_mobkit::identity_first::AgentIdentity)>, PrivateOriginalError> {
        let human = viewer.human.as_ref().ok_or(PrivateOriginalError::Denied)?;
        let principal = human.principal().map_err(|_| PrivateOriginalError::Denied)?;
        human.current().map_err(|_| PrivateOriginalError::Denied)?;
        let lifetime = self.status_lifetime()?;
        let runtime = self.bound_identity_runtime()?;
        let mut members = Vec::new();
        for route in self.routes()? {
            if !same_source_human(&principal, &route.requester, route.represented_subject.as_ref()) { continue; }
            let member = self.status_route(&lifetime, &principal, route)?;
            let identity = runtime.owned_identity_for_member_alias(route.grant_target.as_str()).await
                .ok_or(PrivateOriginalError::Unavailable)?;
            let fresh = current().ok_or(PrivateOriginalError::Denied)?;
            let fresh_human = fresh.human.as_ref().ok_or(PrivateOriginalError::Denied)?;
            if fresh.service_identity || fresh_human.current().is_err()
                || fresh_human.principal().ok().as_ref() != Some(&principal) {
                return Err(PrivateOriginalError::Denied);
            }
            self.status_lifetime()?;
            if fresh.access_view.as_ref().is_none_or(|view| !view.enforced() || view.can_view_agent(identity.as_str())) {
                members.push((member, identity));
            }
        }
        Ok(members)
    }

    pub(crate) async fn read_application_access(
        &self, human: &PrincipalRef, members: &[MobMemberBinding],
    ) -> Result<ApplicationAccessRead, PrivateOriginalError> {
        self.read_application_access_from(self.store.as_ref(), human, members).await
    }

    pub(crate) async fn read_application_access_from(
        &self, store: &dyn ToolkitStore, human: &PrincipalRef, members: &[MobMemberBinding],
    ) -> Result<ApplicationAccessRead, PrivateOriginalError> {
        self.read_application_access_rows(store, human, members, None).await
    }

    pub(crate) async fn read_application_access_page(
        &self, human: &PrincipalRef, members: &[MobMemberBinding], rows: &[StoredRecord],
    ) -> Result<ApplicationAccessRead, PrivateOriginalError> {
        self.read_application_access_rows(self.store.as_ref(), human, members, Some(rows)).await
    }

    #[cfg(test)]
    pub(crate) async fn read_application_access_page_with_read_probe(
        &self, store: &dyn ToolkitStore, human: &PrincipalRef, members: &[MobMemberBinding], rows: &[StoredRecord],
    ) -> Result<ApplicationAccessRead, PrivateOriginalError> {
        self.read_application_access_rows(store, human, members, Some(rows)).await
    }

    async fn read_application_access_rows(
        &self, store: &dyn ToolkitStore, human: &PrincipalRef, members: &[MobMemberBinding],
        rows: Option<&[StoredRecord]>,
    ) -> Result<ApplicationAccessRead, PrivateOriginalError> {
        let page_only = rows.is_some();
        let mut selected = rows.map(<[StoredRecord]>::to_vec);
        let lifetime = self.status_lifetime()?;
        if !same_source_human(human, human, None) { return Err(PrivateOriginalError::Denied); }
        let (_, publication) = self.owner.publication().observe(|| ())
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let mut result = ApplicationAccessRead { human: human.clone(), publication, rows: vec![] };
        let mut after = None;
        let mut count = 0;
        loop {
            let page = if page_only {
                match selected.take() { Some(rows) => rows, None => break }
            } else {
                store.list_records(&self.scope, after.as_deref(), 64).await.map_err(store_error)?
            };
            lifetime.validate_current_selection().map_err(|_| PrivateOriginalError::Denied)?;
            result.publication.check_current().map_err(|_| PrivateOriginalError::Unavailable)?;
            if page.is_empty() { break; }
            count += page.len();
            if count > MAX_RECORDS { return Err(PrivateOriginalError::Unavailable); }
            after = page.last().map(|row| row.key.id.clone());
            for source in page {
                if source.data.schema != SOURCE_SCHEMA { continue; }
                let record = self.decode_stored(&source)?;
                // Route IDs survive deployment replacement. Attribute only from
                // the original canonical source, never today's route by itself.
                if !same_source_human(human, &record.source.requester, record.source.represented_subject.as_ref()) { continue; }
                let Some(link) = &record.approved_task else { continue; };
                let Some(route) = self.routes()?.iter().find(|route| route.id == record.event.route
                    && same_source_human(human, &route.requester, route.represented_subject.as_ref())) else { continue; };
                let member = self.status_route(&lifetime, human, route)?;
                if !members.contains(&member) { continue; }
                if link.original_task.member.as_ref() != Some(&member) || link.grants().iter().any(|grant| grant.route != route.id) {
                    return Err(PrivateOriginalError::Conflict);
                }
                let source_ref = PrivateOriginalRef { route: route.id.clone(), original: record.event.original.clone(),
                    event_sha256: digest(&canonical_json(&record.event).map_err(|_| PrivateOriginalError::Conflict)?) };
                self.decode_record(&source, &source_ref)?;
                // Reuse the canonical task/source correlation reader, including
                // completed rows. No task-open, admission or retry check is made.
                self.service_task_snapshot(&record).await?;
                for grant in link.grants() {
                let access = store.get(&self.access_key(&grant)?).await.map_err(store_error)?
                    .ok_or(PrivateOriginalError::Unavailable)?;
                let access_record = self.decode_access(&access, &grant)?;
                // Old rows without native correspondence remain historical.
                // Neither a missing source join nor v2 becomes a live account.
                if result.rows.iter().any(|row| row.access.key == access.key) { continue; }
                if result.rows.len() == MAX_ACCESS { return Err(PrivateOriginalError::Unavailable); }
                let state = self.access_observation(grant.clone(), &access_record)?.state;
                let id = digest(&canonical_json(&(human, &access.key, &access.content_digest))
                    .map_err(|_| PrivateOriginalError::Conflict)?);
                let mut projection = match (&access_record.decision, &access_record.mcp) {
                    (AccessDecision::Mcp(decision), Some(selection)) => serde_json::json!({"service":selection.server,
                        "selected_account":selection.connector.as_ref().map(|binding| binding.account.as_str()),
                        "target":decision.permission.target, "operations":decision.permission.operations,
                        "resource":decision.permission.resource, "lifetime":decision.permission.lifetime}),
                    (AccessDecision::Catalog(decision), None) => serde_json::json!({"service":"public_service_set_catalog",
                        "selected_account":null, "target":route.grant_target, "operations":["catalog"],
                        "resource":decision.session.to_string(), "lifetime":decision.lifetime}),
                    _ => continue,
                };
                projection["id"] = serde_json::json!(id);
                projection["state"] = serde_json::json!(state);
                projection["can_revoke"] = serde_json::json!(state == PrivateServiceGrantState::GrantIssued);
                result.rows.push(ApplicationAccessRow { id, projection, member: member.clone(), source: source.clone(), source_ref: source_ref.clone(),
                    access, reference: grant.clone(), record: access_record });
                }
            }
        }
        for row in &result.rows { self.recheck_application_access_from(store, &result, row).await?; }
        lifetime.validate_current_selection().map_err(|_| PrivateOriginalError::Denied)?;
        result.publication.check_current().map_err(|_| PrivateOriginalError::Unavailable)?;
        Ok(result)
    }

    pub(crate) async fn recheck_application_access(
        &self, read: &ApplicationAccessRead, row: &ApplicationAccessRow,
    ) -> Result<(), PrivateOriginalError> {
        self.recheck_application_access_from(self.store.as_ref(), read, row).await?;
        if self.access_observation(row.reference.clone(), &row.record)?.state != PrivateServiceGrantState::GrantIssued {
            return Err(PrivateOriginalError::Denied);
        }
        read.publication.check_current().map_err(|_| PrivateOriginalError::Unavailable)
    }
    async fn recheck_application_access_from(
        &self, store: &dyn ToolkitStore, read: &ApplicationAccessRead, row: &ApplicationAccessRow,
    ) -> Result<(), PrivateOriginalError> {
        let source = store.get(&row.source.key).await.map_err(store_error)?.ok_or(PrivateOriginalError::Unavailable)?;
        let access = store.get(&row.access.key).await.map_err(store_error)?.ok_or(PrivateOriginalError::Unavailable)?;
        let original = self.decode_record(&source, &row.source_ref)?;
        self.service_task_snapshot(&original).await?;
        if source.revision != row.source.revision || source.content_digest != row.source.content_digest
            || access.revision != row.access.revision || access.content_digest != row.access.content_digest
            || !same_source_human(&read.human, &original.source.requester, original.source.represented_subject.as_ref())
            || original.approved_task.as_ref().is_none_or(|link| !link.contains(&row.reference))
            || self.decode_access(&access, &row.reference)? != row.record {
            return Err(PrivateOriginalError::Conflict);
        }
        read.publication.check_current().map_err(|_| PrivateOriginalError::Unavailable)
    }

    /// Called synchronously inside the actual installed phase and login/pipe
    /// exclusion. No store await, detached worker, supplied principal or fresh
    /// issuance occurs. Native owns the exact issuer decision and mutation.
    pub(crate) fn revoke_application_access_now(
        &self, read: &ApplicationAccessRead, row: &ApplicationAccessRow,
    ) -> Result<Result<(), ExpectedRevokeRefusal>, PrivateOriginalError> {
        let leaf = self.application_access_revoke_leaf(read, row)?;
        let machine = self.machine()?;
        let mut custody = machine.try_controller_grant_mutation().map_err(|_| PrivateOriginalError::Unavailable)?;
        // Only the inner result is an actual native revoke observation. A
        // failed pre-entry check never fabricates uncertain native issuance.
        Ok(self.revoke_application_access_in_custody(read, row, leaf, &mut custody))
    }

    fn application_access_revoke_leaf<'a>(
        &self, read: &ApplicationAccessRead, row: &'a ApplicationAccessRow,
    ) -> Result<&'a GrantLineageRef, PrivateOriginalError> {
        read.publication.check_current().map_err(|_| PrivateOriginalError::Unavailable)?;
        let route = self.route(&row.reference.route)?;
        if !same_source_human(&read.human, &route.requester, route.represented_subject.as_ref())
            || route.grant_caller != row.record.caller {
            return Err(PrivateOriginalError::Denied);
        }
        let IssuanceObservation::Issued { lineage } = &row.record.issuance else { return Err(PrivateOriginalError::Denied); };
        lineage.last().ok_or(PrivateOriginalError::Denied)
    }

    fn revoke_application_access_in_custody(
        &self, read: &ApplicationAccessRead, row: &ApplicationAccessRow, leaf: &GrantLineageRef,
        custody: &mut impl meerkat_authorization_contracts::grant_mutation::ControllerGrantMutationCustody,
    ) -> Result<(), ExpectedRevokeRefusal> {
        self.owner.grants().revoke_expecting(&row.record.caller, leaf, &read.publication, custody)
    }

    /// Component control only. The wrapper delegates to actual machine
    /// custody and schedules a completed owner change at its callback boundary.
    /// It supplies neither mutation permission nor a replacement veto.
    #[cfg(test)]
    pub(in crate::private_originals) fn revoke_application_access_with_test_custody(
        &self, read: &ApplicationAccessRead, row: &ApplicationAccessRow,
        custody: &mut impl meerkat_authorization_contracts::grant_mutation::ControllerGrantMutationCustody,
    ) -> Result<Result<(), ExpectedRevokeRefusal>, PrivateOriginalError> {
        let leaf = self.application_access_revoke_leaf(read, row)?;
        Ok(self.revoke_application_access_in_custody(read, row, leaf, custody))
    }
}
