//! Reacquire selected setup A's source custody for the existing continuation.
//! This never submits A, reconstructs a login, or validates native run currency.
use super::*;
use super::cold_service_admission::writer_error;
use crate::original_ingress::OriginalCancellationSource;
use meerkat_authorization_contracts::work_association::InputAuthorityAssociation;
use meerkat_runtime::meerkat_machine::{AdmittedOriginalRefusal, AuthenticatedOriginal};
use meerkat_toolkit::{continuation::{ContinuationError, ContinuationObligation},
    host_lifetime::control::HostLifetimeError};

fn continuation_error(error: PrivateOriginalError) -> ContinuationError {
    match error {
        PrivateOriginalError::Unavailable => ContinuationError::OwnerUnavailable,
        PrivateOriginalError::Conflict => ContinuationError::InvalidReceipt,
        PrivateOriginalError::Denied => ContinuationError::Denied,
    }
}
fn selection_error(error: HostLifetimeError) -> PrivateOriginalError {
    match error {
        HostLifetimeError::Unavailable => PrivateOriginalError::Unavailable,
        _ => PrivateOriginalError::Conflict,
    }
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

impl PrivateOriginals {
    /// Eligibility comes from the immutable installed route and recorded setup
    /// selection. Labels cannot replace a lost Console login or select Standing.
    pub(super) fn stock_setup_source(&self, record: &SourceRecord, reference: &PrivateOriginalRef)
        -> Result<bool, PrivateOriginalError>
    {
        if record.approved_task.is_some() || record.service_admission.is_some()
            || (record.setup_request.is_none() && record.configured_setup.is_none())
        { return Ok(false); }
        let Some(selected_lifetime) = self.host_lifetime.get() else { return Ok(false); };
        let lifetime = selected_lifetime.upgrade().ok_or(PrivateOriginalError::Unavailable)?;
        lifetime.validate_current_selection().map_err(|_| PrivateOriginalError::Unavailable)?;
        let native = lifetime.selected_policy().native_host.as_ref().ok_or(PrivateOriginalError::Unavailable)?;
        let selected = native.routes.iter().find(|route| route.id == reference.route)
            .ok_or(PrivateOriginalError::Conflict)?;
        if !matches!(selected.origin, meerkat_toolkit::host_lifetime::policy::CompiledNativeSourceOrigin::LocalConsole { .. }) {
            return Ok(false);
        }
        let route = self.route(&reference.route)?;
        let operator = meerkat::governed_realm::stock::StockPrincipals::for_local_operator(&native.realm)
            .map_err(|_| PrivateOriginalError::Unavailable)?.operator;
        Ok(route.requester == operator && record.source.requester == operator)
    }

    /// Compare the complete historical selection using its existing owner.
    /// No parent/child issuance or present-day selection substitution occurs.
    pub(super) async fn check_historical_setup_selection(&self, record: &SourceRecord,
        reference: &PrivateOriginalRef) -> Result<(), PrivateOriginalError>
    {
        let association = record.final_association.as_ref().ok_or(PrivateOriginalError::Unavailable)?;
        if record.setup_request.is_some() == record.configured_setup.is_some() {
            return Err(PrivateOriginalError::Conflict);
        }
        self.check_source_binding(record, association).map_err(writer_error)?;
        self.check_configured_setup_binding(reference, record, association).map_err(writer_error)?;
        if let Some(selection) = &record.setup_request {
            if &selection.original != reference { return Err(PrivateOriginalError::Conflict); }
            let lifetime = self.host_lifetime.get().and_then(Weak::upgrade)
                .ok_or(PrivateOriginalError::Unavailable)?;
            crate::installed_host::operator_setup::validate_retained_setup_selection(
                &lifetime, &self.owner, self.store.as_ref(), selection).await.map_err(selection_error)?;
        }
        // Match the existing restore_authenticated class. Targeted lineages
        // are structural; the legacy single lineage retains its old resolution.
        self.owner.check_historical_task_child(association).map_err(writer_error)
    }

    /// Called for an exact committed A result. A selected B requires its
    /// durable native-failure cause and authenticated permanent non-entry.
    /// Existing warm
    /// custody keeps its prior native path. Cold reconstruction is deliberately
    /// limited to a single selected input/contributor with no named ancestors;
    /// there is no authoritative source-reference inventory for wider batches.
    /// True records only a new cold custody hold; warm reconciliation returns
    /// false so observing it does not keep waking the native delivery owner.
    pub(crate) async fn restore_setup_continuation_source(&self, obligation: &ContinuationObligation)
        -> Result<bool, ContinuationError>
    {
        if obligation.approved_service().is_some()
            && !self.failed_service_allows_original_result(obligation).await?
        { return Err(ContinuationError::Denied); }
        self.restore_setup_source(obligation).await.map_err(continuation_error)
    }

    async fn restore_setup_source(&self, obligation: &ContinuationObligation)
        -> Result<bool, PrivateOriginalError>
    {
        #[cfg(test)]
        Self::synthetic_admission_diagnostic("cold.restore.entered", ());
        if obligation.tenant().as_str() != self.scope.realm || obligation.deployment().as_str() != self.scope.owner {
            return Err(PrivateOriginalError::Denied);
        }
        let binding = obligation.continuation().native_original.as_ref().ok_or(PrivateOriginalError::Unavailable)?;
        let original = obligation.original_work();
        let key = StoreKey { realm: self.scope.realm.clone(), owner: self.scope.owner.clone(),
            id: source_key_id(&RealmId::parse(self.scope.realm.clone()).map_err(|_| PrivateOriginalError::Conflict)?,
                self.owner.ingress_domain(), &original.authority, &original.work)? };
        let writer = self.owner.try_source_writer().map_err(writer_error)?;
        let (_, stamp) = self.owner.publication().observe(|| ()).map_err(|_| PrivateOriginalError::Unavailable)?;
        #[cfg(test)]
        Self::synthetic_admission_diagnostic("cold.source_owner.observed", ());
        let stored = self.store.get(&key).await.map_err(store_error)?.ok_or(PrivateOriginalError::Unavailable)?;
        #[cfg(test)]
        Self::synthetic_admission_diagnostic("cold.source_record.returned", ());
        let record = self.decode_stored(&stored)?;
        let reference = PrivateOriginalRef { route: record.event.route.clone(), original: record.event.original.clone(),
            event_sha256: digest(&canonical_json(&record.event).map_err(|_| PrivateOriginalError::Conflict)?) };
        let record = self.decode_record(&stored, &reference)?;
        let association = record.final_association.as_ref().ok_or(PrivateOriginalError::Unavailable)?;
        let claims = association.candidate();
        if &claims.original_work != original || &claims.requester != obligation.requester()
            || &claims.root_event != obligation.origin() || record.approved_task.is_some()
            || record.service_admission.is_some() || record.operation_cancelled
        { return Err(PrivateOriginalError::Denied); }
        if self.owner.has_admitted_source(association).map_err(writer_error)? {
            // No source promotion occurs for this branch. Native still checks
            // every contributor, run, submission digest and current authority.
            return Ok(false);
        }
        if !self.stock_setup_source(&record, &reference)? { return Err(PrivateOriginalError::Unavailable); }
        self.check_historical_setup_selection(&record, &reference).await?;
        let input = record.bound_service_input.as_ref().ok_or(PrivateOriginalError::Unavailable)?.decode()?;
        check_single_source(binding, association)?;
        let sources = [OriginalCancellationSource { realm: record.source.realm.clone(),
            original: original.clone(), record: Arc::new(stored), association: Some(association.clone()) }];
        stamp.check_current().map_err(|_| PrivateOriginalError::Unavailable)?;
        drop(writer);
        #[cfg(test)]
        Self::synthetic_admission_diagnostic("cold.source_record.checked", ());
        #[cfg(test)]
        Self::synthetic_admission_diagnostic("cold.controller_source.entered", ());
        let pins = self.controller_pin_source.get().and_then(Weak::upgrade).ok_or(PrivateOriginalError::Unavailable)?;
        #[cfg(test)]
        Self::synthetic_admission_diagnostic("cold.controller_source.returned", ());
        let machine = self.machine()?;
        #[cfg(test)]
        Self::synthetic_admission_diagnostic("cold.machine.returned", ());
        let session = &binding.origin_session_id;
        #[cfg(test)]
        Self::synthetic_admission_diagnostic("cold.registration.entered", ());
        let registration = machine.current_session_registration_witness(session).await.ok_or(PrivateOriginalError::Unavailable)?;
        #[cfg(test)]
        Self::synthetic_admission_diagnostic("cold.registration.returned", ());
        #[cfg(test)]
        Self::synthetic_admission_diagnostic("cold.attachment.entered", ());
        let attachment = machine.current_executor_attachment_witness(session).await.ok_or(PrivateOriginalError::Unavailable)?;
        #[cfg(test)]
        Self::synthetic_admission_diagnostic("cold.attachment.returned", ());
        #[cfg(test)]
        Self::synthetic_admission_diagnostic("cold.actor.entered", ());
        let actor = pins.live_actor(session).await.ok_or(PrivateOriginalError::Unavailable)?;
        #[cfg(test)]
        Self::synthetic_admission_diagnostic("cold.actor.returned", ());
        let custody = AuthenticatedOriginal {
            original_work: claims.original_work.clone(), occurrence_scope: claims.ingress_namespace.occurrence_scope.clone(),
            original_authentication: claims.original_authentication.clone(), root_event: claims.root_event.clone(),
            contributing_work: claims.contributing_work.clone(), target: claims.target.clone(), authority_basis: claims.authority_basis.clone(),
        };
        let contributor = &binding.retained_work.contributors()[0];
        #[cfg(test)]
        Self::synthetic_admission_diagnostic("cold.native_observation.started", ());
        let observed = machine.with_admitted_original(&registration, &attachment, &actor.executor_actor_identity(), &input,
            |admitted| {
                #[cfg(test)]
                Self::synthetic_admission_diagnostic("cold.native_callback.entered", ());
                let _writer = self.owner.try_source_writer().map_err(writer_error)?;
                stamp.check_current().map_err(|_| PrivateOriginalError::Unavailable)?;
                if admitted.input_id() != &contributor.input_id { return Err(PrivateOriginalError::Conflict); }
                self.owner.promote_historical_task(association, custody, &sources, admitted).map_err(writer_error)
            }).await;
        let retained = match observed {
            Ok(result) => {
                #[cfg(test)]
                Self::synthetic_admission_diagnostic("cold.native_callback.returned",
                    result.as_ref().map(|_| ()));
                result?
            },
            // Absence never becomes a fresh A submission or another input ID.
            Err(_refusal @ (AdmittedOriginalRefusal::NotAdmitted | AdmittedOriginalRefusal::Unavailable)) => {
                #[cfg(test)]
                Self::synthetic_admission_diagnostic("cold.native_observation.refused", _refusal);
                return Err(PrivateOriginalError::Unavailable);
            },
            Err(_refusal @ AdmittedOriginalRefusal::Conflict) => {
                #[cfg(test)]
                Self::synthetic_admission_diagnostic("cold.native_observation.refused", _refusal);
                return Err(PrivateOriginalError::Conflict);
            },
        };
        #[cfg(test)]
        Self::synthetic_admission_diagnostic("cold.hold_binding.started", ());
        self.hold_binding(reference, retained).await?;
        #[cfg(test)]
        Self::synthetic_admission_diagnostic("cold.hold_binding.returned", ());
        Ok(true)
    }
}

fn check_single_source(binding: &meerkat_toolkit::request::NativeOriginalTask,
    association: &InputAuthorityAssociation) -> Result<(), PrivateOriginalError>
{
    let work = &binding.retained_work;
    let claims = association.candidate();
    if work.contributors().len() != 1 || work.selected_input_bindings().len() != 1
        || !claims.contributing_work.is_empty()
    { return Err(PrivateOriginalError::Unavailable); }
    let contributor = &work.contributors()[0];
    let selected = work.selected_input_bindings().get(&contributor.input_id.to_string())
        .ok_or(PrivateOriginalError::Conflict)?;
    if claims.target.context.as_str() != binding.origin_session_id.to_string()
        || work.runtime_id() != claims.target.logical_runtime.as_str()
        || work.controller() != claims.controller_model.as_ref()
        || contributor.association_digest != meerkat_runtime::retained_work::retained_association_digest(association)
            .map_err(|_| PrivateOriginalError::Unavailable)?
        || selected.binding != hex(&association.canonical_bytes().map_err(|_| PrivateOriginalError::Conflict)?)
        || selected.batch_key != hex(&association.batch_identity_bytes().map_err(|_| PrivateOriginalError::Conflict)?)
    { return Err(PrivateOriginalError::Conflict); }
    // Submission digest and run identity stay with native retained completion.
    // #35 verifies the saved Input and reports the actual input ID in callback.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::FutureExt;
    use std::{os::unix::fs::PermissionsExt, panic::AssertUnwindSafe};

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn no_installed_host_declines_optional_capture_and_ancestor_inventory_is_not_inferred() {
        let fixture = ContinuationFixture::for_cold_continuation(false, false).await;
        let result = AssertUnwindSafe(Box::pin(async {
            let admission = fixture.submit().await;
            fixture.finish(&admission).await.unwrap();
            let row = fixture.cold_source_record().await;
            let record = fixture.source.decode_record(&row, fixture.original_reference()).unwrap();
            // A separate custom owner supplies only the absent-host predicate.
            // The real record/reference remain inert here; no native custody
            // is transferred or restored into this adapter.
            let custom_directory = tempfile::Builder::new()
                .permissions(std::fs::Permissions::from_mode(0o700)).tempdir().unwrap();
            let custom_owner = crate::original_ingress::tests::initialized(
                &custom_directory.path().join("grants"));
            let custom = crate::original_ingress::tests::private_originals(
                custom_owner, &custom_directory.path().join("source.sqlite")).await;
            assert!(custom.host_lifetime.get().is_none());
            assert!(!custom.stock_setup_source(&record, fixture.original_reference()).unwrap(),
                "optional capture cannot turn a missing installed host into fresh-input failure");
            let context = fixture.probe.as_ref().unwrap().captured();
            let binding = meerkat_toolkit::request::NativeOriginalTask { origin_session_id: fixture.session.clone(),
                retained_work: context.retained_work().unwrap().clone(), member: None };
            let association = record.final_association.unwrap();
            assert!(check_single_source(&binding, &association).is_ok());
            let mut claims = association.candidate().clone();
            let mut ancestor = claims.original_work.clone();
            ancestor.work = EvidenceId::new("unretained-ancestor").unwrap();
            claims.contributing_work.push(ancestor);
            let changed = InputAuthorityAssociation::new(claims).unwrap();
            assert!(matches!(check_single_source(&binding, &changed), Err(PrivateOriginalError::Unavailable)),
                "the anchor's saved input is not an inventory of ancestor sources");
        })).catch_unwind().await;
        let cleanup = fixture.shutdown().await;
        assert!(cleanup.is_ok(), "{cleanup:?}");
        if let Err(panic) = result { std::panic::resume_unwind(panic); }
    }
}
