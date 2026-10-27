//! Historical task custody enters only from the native admitted-original callback.
//! No source publication waits while the native B/M/driver owners are held.
use super::*;
use meerkat_runtime::meerkat_machine::AdmittedOriginal;

impl ToolkitOriginalOwner {
    /// Try the existing source writer without waiting. Canonical evidence reads
    /// release it before native observation; the synchronous callback reacquires
    /// it and retains it through the nonblocking publication/originals checks.
    pub(crate) fn try_source_writer(&self)
        -> Result<tokio::sync::MutexGuard<'_, ()>, OriginalWriterError>
    {
        self.mutation.try_lock().map_err(|_| OriginalWriterError::Unavailable)
    }

    /// Ordinary async membership changes use this same source writer. Native
    /// callbacks use the nonwaiting variant above and never wait for this guard.
    pub(crate) async fn source_writer(&self) -> tokio::sync::MutexGuard<'_, ()> {
        // The actual async caller retains this wait. It checks source/grant
        // currentness after acquisition, and never enters a native callback
        // while waiting for recovery's entered canonical write.
        #[cfg(not(test))]
        { self.mutation.lock().await }
        #[cfg(test)]
        {
            use std::future::Future;
            let acquiring = self.mutation.lock();
            tokio::pin!(acquiring);
            std::future::poll_fn(|cx| {
                let polled = acquiring.as_mut().poll(cx);
                if polled.is_pending() {
                    if let Some(entered) = self.source_writer_wait.lock().unwrap().take() {
                        let _ = entered.send(());
                    }
                }
                polled
            }).await
        }
    }

    /// Observe one real queued writer in a fixture. This neither holds nor
    /// releases the writer, and it cannot change the acquisition result.
    #[cfg(test)]
    pub(crate) fn observe_source_writer_wait_for_test(&self) -> tokio::sync::oneshot::Receiver<()> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        assert!(self.source_writer_wait.lock().unwrap().replace(sender).is_none());
        receiver
    }

    /// Warm custody remains the existing native resume path, including source
    /// classes outside the narrow cold reconstruction. This is no permission
    /// check: native still checks the complete contributor closure and grants.
    pub(crate) fn has_admitted_source(&self, association: &InputAuthorityAssociation)
        -> Result<bool, OriginalWriterError>
    {
        let originals = self.originals.try_lock().map_err(|_| OriginalWriterError::Unavailable)?;
        let claims = association.candidate();
        let Some(record) = originals.iter().find(|row|
            row.names(&claims.ingress_namespace.realm, &claims.original_work)) else { return Ok(false); };
        let OriginalRecord::Bound(held) = record else { return Ok(false); };
        if held.association() != association { return Err(OriginalWriterError::Conflict); }
        match &held.operation {
            OriginalOperationDisposition::Open => (),
            OriginalOperationDisposition::CancellationPending => return Err(OriginalWriterError::Unavailable),
            OriginalOperationDisposition::Cancelled { .. } => return Err(OriginalWriterError::OperationCancelled),
        }
        Ok(held.source_custody == Some(SourceCustody::Admitted)
            && held.controller.as_ref().is_some_and(|client|
                claims.controller_model.as_ref() == Some(client.selection())))
    }

    /// Preserve restore_authenticated's task-child check, including the legacy
    /// single-lineage resolution. Called before entering native B/M/D.
    pub(crate) fn check_historical_task_child(&self, association: &InputAuthorityAssociation)
        -> Result<(), OriginalWriterError>
    {
        self.check_original_child(association)
    }

    /// Recheck the exact candidate before retaining its bound Input. The caller
    /// holds the source writer across its durable evidence write.
    pub(crate) fn check_bound_source(&self, original: &Arc<RetainedToolkitOriginal>)
        -> Result<(), OriginalWriterError>
    {
        self.require_retained(original)?;
        self.refuse_cancelled_binding(original.association())
    }

    /// Called only after the canonical approved B decision or selected setup A
    /// child binding was joined under this owner's writer. Native supplies the
    /// actual admitted association/controller, never a decoded input capability.
    /// This changes source custody only: all native phases stay historical.
    pub(crate) fn promote_historical_task(
        &self,
        association: &InputAuthorityAssociation,
        custody: AuthenticatedOriginal,
        sources: &[OriginalCancellationSource],
        admitted: &AdmittedOriginal<'_>,
    ) -> Result<Arc<RetainedToolkitOriginal>, OriginalWriterError> {
        self.promote_historical_source(association, custody, sources, admitted, OriginalGrantBinding::Task { issuer: None })
    }

    /// Only the canonical, explicitly selected planning P restoration calls
    /// this after native has verified its exact historical input. Keep its
    /// ControllerOnly class; neither a task leaf nor a revoke issuer is added.
    /// The caller resolves its controller before native B/M/D and checks that
    /// observation atomically inside the callback. This method never resolves
    /// grants or waits for their owner while native custody is held.
    pub(crate) fn promote_historical_controller_only(
        &self, association: &InputAuthorityAssociation, custody: AuthenticatedOriginal,
        sources: &[OriginalCancellationSource], admitted: &AdmittedOriginal<'_>,
    ) -> Result<Arc<RetainedToolkitOriginal>, OriginalWriterError> {
        let claims = association.candidate();
        if !controller_only_basis(&claims.authority_basis) || !claims.contributing_work.is_empty() {
            return Err(OriginalWriterError::UnsupportedMandate);
        }
        self.promote_historical_source(association, custody, sources, admitted, OriginalGrantBinding::ControllerOnly)
    }

    /// The native callback owns the current registration, attachment and actor.
    /// Facts are synchronous and request-free; route data alone is not custody.
    pub(super) fn admitted_controller_route(
        association: &InputAuthorityAssociation, admitted: &AdmittedOriginal<'_>,
    ) -> Result<meerkat_core::ControllerRouteKey, OriginalWriterError> {
        let claims = association.candidate();
        if admitted.association() != association
            || claims.controller_model.as_ref() != Some(admitted.controller().selection())
            || claims.target.context.as_str() != admitted.session_id().to_string()
            || claims.target.logical_runtime.as_str()
                != meerkat_runtime::LogicalRuntimeId::for_session(admitted.session_id()).to_string()
        { return Err(OriginalWriterError::Conflict); }
        Ok(admitted.controller().plain_facts().map_err(|_| OriginalWriterError::Unavailable)?.route_key())
    }

    fn promote_historical_source(
        &self, association: &InputAuthorityAssociation, custody: AuthenticatedOriginal,
        sources: &[OriginalCancellationSource], admitted: &AdmittedOriginal<'_>,
        class: OriginalGrantBinding,
    ) -> Result<Arc<RetainedToolkitOriginal>, OriginalWriterError> {
        for source in sources {
            if let Some(attempt) = crate::private_originals::historical_custody_attempt(&source.record)? {
                if !self.has_installed_pair_recovery()? {
                    return Err(OriginalWriterError::Unavailable);
                }
                match attempt.decision() {
                    super::custody_attempt::CustodyDecision::Promoted => (),
                    super::custody_attempt::CustodyDecision::Pending => return Err(OriginalWriterError::Unavailable),
                    super::custody_attempt::CustodyDecision::Withdrawn => return Err(OriginalWriterError::Conflict),
                }
            }
        }
        self.check_source(association, &custody,
            &association.candidate().requester, &association.candidate().ingress_actor)?;
        let route = Self::admitted_controller_route(association, admitted)?;
        let check_route = |held: &HeldOriginal| -> Result<(), OriginalWriterError> {
            if let Some(controller) = &held.controller {
                if controller.selection() != admitted.controller().selection()
                    || controller.plain_facts().map_err(|_| OriginalWriterError::Unavailable)?.route_key() != route
                { return Err(OriginalWriterError::Conflict); }
            }
            Ok(())
        };
        // The source writer and native B/M/D custody are held throughout. An
        // already-admitted exact source changes no policy, but must still adopt
        // THIS callback's runnable pin, including after an actor rebuild. The
        // workflow attachment retains only route metadata, never a second pin.
        let (prior, stamp) = self.publication.observe(|| {
            let mut originals = self.originals.try_lock().map_err(|_| OriginalWriterError::Unavailable)?;
            let _poison = self.publication.poison_on_unwind();
            let index = Self::historical_source_index(association, sources, &class, &originals)?;
            let Some(index) = index else { return Ok(None); };
            let held = originals[index].bound_mut().ok_or(OriginalWriterError::Unavailable)?;
            check_route(held)?;
            if held.source_custody != Some(SourceCustody::Admitted) || held.controller.is_none() { return Ok(None); }
            held.controller = Some(admitted.controller().clone());
            Ok::<_, OriginalWriterError>(Some(held.retained.clone()))
        }).map_err(|_| OriginalWriterError::Unavailable)?;
        if let Some(prior) = prior? {
            stamp.check_current().map_err(|_| OriginalWriterError::Unavailable)?;
            return Ok(prior);
        }
        // Required lock order inside native B/M/D: non-waiting publication,
        // then non-waiting source collection. Recheck before any mutation.
        let _publication = self.publication.try_begin_owner_change()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let mut originals = self.originals.try_lock()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let index = Self::historical_source_index(association, sources, &class, &originals)?;
        if let Some(index) = index {
            let OriginalRecord::Bound(held) = &mut originals[index] else {
                return Err(OriginalWriterError::Unavailable);
            };
            check_route(held)?;
            held.source_custody = Some(SourceCustody::Admitted);
            held.controller = Some(admitted.controller().clone());
            return Ok(held.retained.clone());
        }
        let retained = Arc::new(RetainedToolkitOriginal {
            association: association.clone(), custody,
            // Historical admission preserves the selected class and cannot
            // recover a task-revoke issuer.
            grant_binding: class,
            first_binding: None,
        });
        originals.push(OriginalRecord::Bound(HeldOriginal {
            retained: retained.clone(), source_custody: Some(SourceCustody::Admitted), custody_settlement: None,
            controller: Some(admitted.controller().clone()), service_set_admission: None, operation: OriginalOperationDisposition::Open,
        }));
        Ok(retained)
    }

    /// Re-publish this still-retained owner after a native-accepted source
    /// publication failed. The caller has durably promoted the exact attempt,
    /// rechecked that row's publication stamp and reacquired the source writer
    /// inside the real native admitted-original callback. No new binding or
    /// grant is issued, and cancellation/current identity remain authoritative.
    pub(crate) fn recover_retained_custody(
        &self, original: &Arc<RetainedToolkitOriginal>, admitted: &AdmittedOriginal<'_>,
    ) -> Result<(), OriginalWriterError> {
        let _route = Self::admitted_controller_route(original.association(), admitted)?;
        let _publication = self.publication.try_begin_owner_change()
            .map_err(|_| OriginalWriterError::Unavailable)?;
        let mut originals = self.originals.try_lock().map_err(|_| OriginalWriterError::Unavailable)?;
        let claims = original.association().candidate();
        if claims.contributing_work.iter().any(|contributor| !originals.iter()
            .filter_map(OriginalRecord::bound).any(|held|
                held.source_custody == Some(SourceCustody::Admitted)
                    && held.operation == OriginalOperationDisposition::Open
                    && held.names(&claims.ingress_namespace, contributor)
                    && held.association.candidate().requester == claims.requester)) {
            return Err(OriginalWriterError::Conflict);
        }
        let held = originals.iter_mut().filter_map(OriginalRecord::bound_mut)
            .find(|held| Arc::ptr_eq(&held.retained, original)).ok_or(OriginalWriterError::WrongOwner)?;
        if !matches!(held.operation, OriginalOperationDisposition::Open) {
            return Err(OriginalWriterError::OperationCancelled);
        }
        held.controller = Some(admitted.controller().clone());
        held.source_custody = Some(SourceCustody::Admitted);
        Ok(())
    }

    /// Shared exact validation for publication-free replay and mutation passes.
    /// Callers already hold the source collection and never wait here.
    fn historical_source_index(
        association: &InputAuthorityAssociation, sources: &[OriginalCancellationSource],
        class: &OriginalGrantBinding, originals: &[OriginalRecord],
    ) -> Result<Option<usize>, OriginalWriterError> {
        for source in sources {
            if let Some(retained) = originals.iter().find(|entry| entry.names(&source.realm, &source.original)) {
                let operation = match retained {
                    OriginalRecord::Bound(held) => {
                        if source.association.as_ref() != Some(held.association()) {
                            return Err(OriginalWriterError::Conflict);
                        }
                        &held.operation
                    }
                    OriginalRecord::UnboundCancellation { source_record, operation, .. } => {
                        if !crate::private_originals::same_source_origin(source_record, &source.record) {
                            return Err(OriginalWriterError::Conflict);
                        }
                        operation
                    }
                };
                match operation {
                    OriginalOperationDisposition::Open => (),
                    OriginalOperationDisposition::CancellationPending => return Err(OriginalWriterError::Unavailable),
                    OriginalOperationDisposition::Cancelled { .. } => return Err(OriginalWriterError::OperationCancelled),
                }
            }
        }
        if !matches!(class, OriginalGrantBinding::ControllerOnly) {
            Self::check_task_lineage_isolation(association, originals)?;
        }
        let claims = association.candidate();
        let index = originals.iter().position(|entry|
            entry.names(&claims.ingress_namespace.realm, &claims.original_work));
        if let Some(index) = index {
            let OriginalRecord::Bound(held) = &originals[index] else { return Err(OriginalWriterError::Unavailable); };
            let matching_class = match class {
                OriginalGrantBinding::ControllerOnly => matches!(held.grant_binding, OriginalGrantBinding::ControllerOnly),
                _ => matches!(held.grant_binding, OriginalGrantBinding::Task { .. } | OriginalGrantBinding::ServiceSet { .. }),
            };
            if held.association() != association || !matching_class {
                return Err(OriginalWriterError::Conflict);
            }
            match held.operation {
                OriginalOperationDisposition::Open => (),
                OriginalOperationDisposition::CancellationPending => return Err(OriginalWriterError::Unavailable),
                OriginalOperationDisposition::Cancelled { .. } => return Err(OriginalWriterError::OperationCancelled),
            }
            if held.source_custody == Some(SourceCustody::Candidate) {
                // The warm native settlement still owns this guard.
                return Err(OriginalWriterError::Unavailable);
            }
        }
        Ok(index)
    }
}

#[cfg(test)]
impl ToolkitOriginalOwner {
    /// Simulate loss of this fixture's B process custody, not durable/native
    /// state or a full process restart. Production has no row-removal API.
    pub(crate) fn forget_task_custody_for_test(&self, association: &InputAuthorityAssociation) {
        let _publication = self.publication.begin_owner_change().unwrap();
        let mut originals = self.originals.lock().unwrap();
        let index = originals.iter().position(|entry|
            entry.bound().is_some_and(|held| held.association() == association)).unwrap();
        let held = originals[index].bound().unwrap();
        assert!(matches!(held.grant_binding, OriginalGrantBinding::Task { .. } | OriginalGrantBinding::ServiceSet { .. }));
        assert!(held.source_custody != Some(SourceCustody::Candidate));
        originals.remove(index);
    }

    pub(crate) fn historical_task_custody_for_test(&self, association: &InputAuthorityAssociation)
        -> Option<(bool, bool)>
    {
        self.originals.lock().unwrap().iter().filter_map(OriginalRecord::bound)
            .find(|held| held.association() == association).map(|held| (
                held.source_custody == Some(SourceCustody::Admitted),
                matches!(held.grant_binding, OriginalGrantBinding::Task { issuer: None }),
            ))
    }

    pub(crate) fn lock_originals_for_test(&self) -> impl Drop + '_ {
        self.originals.lock().unwrap()
    }
}

#[cfg(test)]
impl ToolkitOriginalOwner {
    pub(crate) fn forget_controller_custody_for_test(&self, association: &InputAuthorityAssociation) {
        let _publication = self.publication.begin_owner_change().unwrap();
        let mut originals = self.originals.lock().unwrap();
        let index = originals.iter().position(|entry|
            entry.bound().is_some_and(|held| held.association() == association)).unwrap();
        let held = originals[index].bound().unwrap();
        assert!(matches!(held.grant_binding, OriginalGrantBinding::ControllerOnly));
        assert!(held.source_custody != Some(SourceCustody::Candidate));
        originals.remove(index);
    }
}
