//! Explicit setup-only selection on the existing pending original row.
use super::*;
use meerkat_authorization::grants::GrantRefusal;
use meerkat_authorization_contracts::{
    constraints::DelegationDepth,
    grant::GrantLineageRef,
    work_association::{InputAuthorityAssociation, WorkAuthorityBasis},
};
use meerkat_toolkit::host_lifetime::control::HostLifetimeError;

/// Exact native references produced by a distinct protected setup review.
/// Decoding this value does not authorize anything; the source owner reopens
/// its actual operator decision and resolves the native lineage before use.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrivateSetupRequestSelection {
    pub original: PrivateOriginalRef,
    pub session_id: meerkat_core::SessionId,
    pub parent: Vec<GrantLineageRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub helper_targets: Vec<meerkat_toolkit::host_lifetime::policy::CompiledAgentSetupTarget>,
}

impl PrivateOriginals {
    /// Called only inside the existing original mutation owner. The same
    /// canonical read refuses missing, cancelled or already-bound sources.
    pub(crate) async fn check_setup_review_source(
        &self, original: &PrivateOriginalRef,
    ) -> Result<Option<PrivateSetupRequestSelection>, PrivateOriginalError> {
        let stored = self.store.get(&self.key(original)?).await.map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let record = self.decode_record(&stored, original)?;
        if record.operation_cancelled || record.final_association.is_some()
            || record.approved_task.is_some() || record.service_admission.is_some() || record.configured_setup.is_some()
        { return Err(PrivateOriginalError::Denied); }
        Ok(record.setup_request)
    }

    /// Select only on the same captured, still-unbound A. A competing first
    /// binder or cancellation wins by the existing canonical CAS. An uncertain
    /// CAS is reconciled by this exact row, never by creating another original.
    pub(crate) async fn select_setup_request(
        self: &Arc<Self>,
        prepared: &LocalControllerSetup,
        parent: Vec<GrantLineageRef>,
    ) -> Result<PrivateSetupRequestSelection, PrivateOriginalError> {
        let captured = prepared
            .source_owner()
            .ok_or(PrivateOriginalError::Unavailable)?;
        if !Arc::ptr_eq(self, &captured) {
            return Err(PrivateOriginalError::Denied);
        }
        let selection = PrivateSetupRequestSelection {
            original: prepared.original().clone(),
            session_id: prepared.session_id().clone(),
            parent,
            helper_targets: vec![],
        };
        let route = self.route(&selection.original.route)?;
        let original = meerkat_authorization_contracts::work_association::OriginalWorkRef {
            authority: route.source_authority.clone(),
            work: selection.original.original.clone(),
        };
        self.owner
            .with_unbound_source_selection(&route.realm, &original, || {
                self.select_setup_request_locked(selection)
            })
            .await
            .map_err(|error| match error {
                OriginalWriterError::Conflict => PrivateOriginalError::Conflict,
                OriginalWriterError::OperationCancelled => PrivateOriginalError::Denied,
                _ => PrivateOriginalError::Unavailable,
            })?
    }

    pub(crate) async fn select_setup_request_locked(
        &self,
        selection: PrivateSetupRequestSelection,
    ) -> Result<PrivateSetupRequestSelection, PrivateOriginalError> {
        let lifetime = self
            .host_lifetime
            .get()
            .and_then(Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        crate::installed_host::operator_setup::validate_setup_selection(
            &lifetime,
            &self.owner,
            self.store.as_ref(),
            &selection,
        )
        .await
        .map_err(|_| PrivateOriginalError::Denied)?;
        let key = self.key(&selection.original)?;
        let stored = self
            .store
            .get(&key)
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let mut record = self.decode_record(&stored, &selection.original)?;
        if record.operation_cancelled
            || record.final_association.is_some()
            || record.approved_task.is_some()
            || record.service_admission.is_some()
            || record.configured_setup.is_some()
        {
            return Err(PrivateOriginalError::Denied);
        }
        if let Some(current) = &record.setup_request {
            return if current == &selection {
                Ok(selection)
            } else {
                Err(PrivateOriginalError::Conflict)
            };
        }
        lifetime
            .validate_current_selection()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        record.setup_request = Some(selection.clone());
        let written = self
            .store
            .compare_and_swap(Transition {
                key: key.clone(),
                expected_revision: stored.revision,
                expected_digest: Some(stored.content_digest),
                claim: None,
                data: encode(record)?,
                events: vec![],
                effects: vec![],
            })
            .await;
        if let Err(error) = written {
            // A returned storage error is not evidence that the conditional
            // write failed. Observe this exact choice once; never issue a new
            // parent or substitute another original as recovery.
            let observed = self
                .store
                .get(&key)
                .await
                .map_err(store_error)?
                .ok_or(PrivateOriginalError::Unavailable)?;
            let current = self.decode_record(&observed, &selection.original)?;
            if current.operation_cancelled {
                return Err(PrivateOriginalError::Denied);
            }
            if current.setup_request.as_ref() != Some(&selection) {
                return Err(store_error(error));
            }
        }
        Ok(selection)
    }

    pub(super) async fn setup_request_basis(
        &self,
        record: &SourceRecord,
        reference: &PrivateOriginalRef,
        session: &meerkat_core::SessionId,
        console: Option<super::console_ingress::ConsoleEntry>,
    ) -> Result<Option<WorkAuthorityBasis>, PrivateOriginalError> {
        let Some(selection) = record.setup_request.as_ref() else {
            return Ok(None);
        };
        if &selection.original != reference
            || &selection.session_id != session
            || record.operation_cancelled
            || record.approved_task.is_some()
            || record.service_admission.is_some()
        {
            return Err(PrivateOriginalError::Denied);
        }
        if let Some(saved) = record.final_association.as_ref() {
            self.check_setup_source_binding(record, saved).map_err(|_| PrivateOriginalError::Conflict)?;
            return Ok(Some(saved.candidate().authority_basis.clone()));
        }
        let lifetime = self
            .host_lifetime
            .get()
            .and_then(Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        let mut bounds = crate::installed_host::operator_setup::validate_setup_selection(
            &lifetime,
            &self.owner,
            self.store.as_ref(),
            selection,
        )
        .await
        .map_err(|_| PrivateOriginalError::Denied)?;
        bounds.delegation_depth = DelegationDepth::remaining(0);
        let child_id = self.setup_child_id(selection)?;
        if let Some(saved) = record.final_association.as_ref() {
            self.check_setup_source_binding(record, saved)
                .map_err(|_| PrivateOriginalError::Conflict)?;
        }
        let route = self.route(&reference.route)?;
        let caller = route.grant_caller.clone();
        let executor = route.logical_executor.clone();
        let represented = route.represented_subject.clone();
        let parent = selection.parent.clone();
        let grants = self.owner.grants();
        let owner = self.owner.clone();
        let targets = selection.helper_targets.clone();
        let origin = self.setup_origin_for(reference)?;
        // The actual blocking writer owns the same login reservation through
        // native issuance even if the waiting binder is dropped.
        let lineage = tokio::task::spawn_blocking(move || {
            super::first_input::with_controller_login(console.as_ref(), None, || {
                lifetime.with_current_setup(|| {
                    let issue = || {
                        let current =
                            grants.resolve_lineage(&parent, &caller, represented.as_ref())?;
                        let expected = current
                            .restrictions()
                            .for_child(&bounds)
                            .map_err(|_| GrantRefusal::Denied)?;
                        current
                            .check_current()
                            .map_err(|_| GrantRefusal::Unavailable)?;
                        let child = match grants.issued_reference(&child_id)? {
                            Some(child) => child,
                            None => grants.issue_child(
                                &caller,
                                parent.last().ok_or(GrantRefusal::Denied)?,
                                child_id.clone(),
                                executor.clone(),
                                bounds,
                            )?,
                        };
                        let mut lineage = parent;
                        lineage.push(child);
                        let current =
                            grants.resolve_lineage(&lineage, &executor, represented.as_ref())?;
                        if current.restrictions() != &expected {
                            return Err(GrantRefusal::Denied);
                        }
                        current
                            .check_current()
                            .map_err(|_| GrantRefusal::Unavailable)?;
                        Ok(lineage)
                    };
                    let lineage = issue().map_err(|_| HostLifetimeError::Unavailable)?;
                    let targets = super::setup_targets::issue_targets(&owner, &caller, &executor, represented.as_ref(),
                        &origin, &child_id, &targets)?;
                    super::setup_targets::project(lineage, targets)
                })
            })
        })
        .await
        .map_err(|_| PrivateOriginalError::Unavailable)?
        .map_err(|_| PrivateOriginalError::Unavailable)?;
        Ok(Some(lineage))
    }

    #[cfg(test)]
    pub(super) async fn setup_request_lineage(&self, record: &SourceRecord, reference: &PrivateOriginalRef,
        session: &meerkat_core::SessionId, console: Option<super::console_ingress::ConsoleEntry>)
        -> Result<Option<Vec<GrantLineageRef>>, PrivateOriginalError>
    {
        self.setup_request_basis(record, reference, session, console).await?.map(|basis|
            super::setup_targets::find_lineage(&basis).map(<[_]>::to_vec).ok_or(PrivateOriginalError::Conflict)).transpose()
    }

    fn setup_child_id(
        &self,
        selection: &PrivateSetupRequestSelection,
    ) -> Result<EvidenceId, PrivateOriginalError> {
        EvidenceId::new(format!(
            "original-setup-{}",
            self.key(&selection.original)?.id
        ))
        .map_err(|_| PrivateOriginalError::Conflict)
    }

    /// Rechecked by the existing source writer after its awaited canonical
    /// read. A controller-only binder that raced explicit selection cannot
    /// publish an association ignoring the now-retained setup intent.
    pub(super) fn check_setup_source_binding(
        &self,
        record: &SourceRecord,
        association: &InputAuthorityAssociation,
    ) -> Result<(), OriginalWriterError> {
        let Some(selection) = record.setup_request.as_ref() else {
            return Ok(());
        };
        let claims = association.candidate();
        let child = self.setup_child_id(selection).map_err(|_| OriginalWriterError::Conflict)?;
        super::setup_targets::check_structure(&selection.parent, &child, &selection.helper_targets, &claims.authority_basis)
            .map_err(|_| OriginalWriterError::Conflict)?;
        if selection.parent.len() != 1
            || claims.target.context.as_str() != selection.session_id.to_string()
            || claims.original_work.work != selection.original.original
            || record.approved_task.is_some()
        {
            return Err(OriginalWriterError::Conflict);
        }
        Ok(())
    }
}

#[cfg(test)]
pub(in crate::private_originals) mod tests;

#[cfg(test)]
mod recovery_tests;
