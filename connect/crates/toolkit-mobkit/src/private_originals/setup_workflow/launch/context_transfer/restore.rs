//! Restore only a previously committed private disclosure. This never creates
//! a Find operation, a native input, or physical-join/absence evidence.
use super::*;

#[cfg(test)]
tokio::task_local! {
    // Caller-owned read pause only; no hidden store task or fabricated join.
    pub(super) static AFTER_COMMITTED_READ: Arc<(tokio::sync::Notify, tokio::sync::Notify)>;
}

impl InstalledWorkflowPlanning {
    pub(in crate::private_originals::setup_workflow::launch) fn retain_context_transfer(
        self: &Arc<Self>,
        candidate: Arc<InstalledWorkflowContextTransfer>,
    ) -> Result<Arc<InstalledWorkflowContextTransfer>, PrivateOriginalError> {
        if !Weak::ptr_eq(&candidate.planning, &Arc::downgrade(self)) {
            return Err(PrivateOriginalError::Denied);
        }
        let mut selected = self
            .context_transfers
            .lock()
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        if let Some(prior) = selected.iter().find(|row| {
            row.selected.original == candidate.selected.original
                && row.selected.correlation == candidate.selected.correlation
        }) {
            return if same_transfer(Some(prior), Some(&candidate))? {
                Ok(prior.clone())
            } else {
                Err(PrivateOriginalError::Conflict)
            };
        }
        // Warm entries remain bounded by their existing retained donor
        // invocation owner (128 per source). Cold entries require an exact
        // canonical membership, whose total includes departed donors and is
        // bounded by terms.max_waiters. Dormant warm choices cannot consume
        // that canonical membership budget or block an unrelated valid donor.
        selected.push(candidate.clone());
        Ok(candidate)
    }

    pub(crate) async fn restore_committed_context(
        self: &Arc<Self>,
        source: &Arc<PrivateOriginals>,
        membership: &SetupWorkflowWaiter,
        recipient: WorkGraphService,
        backend: &SetupWorkBackend,
        output_audiences: BTreeSet<Id>,
        control: &Arc<RecoveryControl>,
    ) -> Result<WorkflowContextObservation, PrivateOriginalError> {
        let actual = self
            .source
            .upgrade()
            .ok_or(PrivateOriginalError::Unavailable)?;
        if !Arc::ptr_eq(&actual, source) || output_audiences.is_empty() {
            return Err(PrivateOriginalError::Denied);
        }
        let workflow = source
            .workflow_store()?
            .workflow_ref(&self.terms)
            .map_err(|_| PrivateOriginalError::Conflict)?;
        if membership.workflow() != &workflow {
            return Err(PrivateOriginalError::Conflict);
        }
        // Validate the host's real recipient owner before reading any private
        // bytes. The namespace is a locator, never a grant minted here.
        let grant = recipient.namespace_grant();
        if backend.pin() != &self.terms.backend
            || !Arc::ptr_eq(backend.service().store(), recipient.store())
            || recipient.store().kind() != WorkGraphStoreKind::Sqlite
            || grant.realm_id != self.terms.realm.to_string()
            || grant.realm_id != recipient.default_realm_id()
            || grant.namespace != recipient.default_namespace().as_str()
            || recipient.default_namespace() != &self.context_namespace()?
            || recipient.default_namespace() == backend.service().default_namespace()
        {
            return Err(PrivateOriginalError::Denied);
        }
        self.with_context_recipient(source, |_| Ok(())).await?;
        let canonical = source
            .workflow_store()?
            .load_setup_workflow(&workflow)
            .await
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        if canonical.terms() != &self.terms {
            return Err(PrivateOriginalError::Conflict);
        }
        // Membership validation is historical: a committed donor can be gone.
        let _ = canonical
            .departed(membership)
            .map_err(|_| PrivateOriginalError::Conflict)?;
        if control.enter_transfer(|| ()).is_none() {
            return Err(PrivateOriginalError::Unavailable);
        }
        let key = admission_key(membership)?;
        let observed = recipient
            .store()
            .observe_item_admission(
                recipient.default_realm_id(),
                recipient.default_namespace(),
                &key,
            )
            .await;
        self.with_context_recipient(source, |_| Ok(())).await?;
        let observed = observed.map_err(|_| PrivateOriginalError::Unavailable)?;
        let Some((item, _)) = observed else {
            // No owned physical attempt was joined. Missing data cannot prove
            // Absent or permit a new disclosure/delivery attempt.
            return Ok(WorkflowContextObservation::Pending);
        };
        let envelope: TransferEnvelope = serde_json::from_str(
            item.description
                .as_deref()
                .ok_or(PrivateOriginalError::Conflict)?,
        )
        .map_err(|_| PrivateOriginalError::Conflict)?;
        let selected = &envelope.recipient;
        let audience = source
            .route(&self.submission.original.route)?
            .grant_audience
            .clone();
        if envelope.purpose != PURPOSE
            || &envelope.membership != membership
            || selected.workflow != workflow
            || selected.planning_source != self.submission.original
            || &selected.parent != self.parent.association()
            || selected.lead != self.terms.lead
            || selected.backend != *backend.pin()
            || selected.recipient_realm != recipient.default_realm_id()
            || &selected.recipient_namespace != recipient.default_namespace()
            || selected.native_audience != audience
            || selected.output_audiences != output_audiences
            || selected.association.candidate().target.audience != audience
            || membership.source() != &source.key(&selected.original)?
            || envelope.find_call.is_empty()
            || envelope
                .request
                .user_context
                .iter()
                .any(|item| matches!(item, RequestContext::Reference(_)))
        {
            return Err(PrivateOriginalError::Conflict);
        }
        // Recompute this Connect-owned correlation from the actual persisted
        // call/scope, using the same producer representation. It is not a new
        // Find authorization or a private native idempotency-key codec.
        let correlation = crate::installed_host::acquisition::installed_invocation_correlation(
            &selected.original,
            &envelope.find_scope,
            &envelope.find_call,
        )
        .map_err(|_| PrivateOriginalError::Conflict)?;
        if correlation != selected.correlation {
            return Err(PrivateOriginalError::Conflict);
        }
        let stored = source
            .store
            .get(membership.source())
            .await
            .map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let record = source.decode_record(&stored, &selected.original)?;
        if record.final_association.as_ref() != Some(&selected.association) {
            return Err(PrivateOriginalError::Conflict);
        }
        // No A currentness, live Task custody, or unretired operation is needed
        // to validate completed disclosure. Canonical data still must be exact.
        let checked = source
            .workflow_store()?
            .validate_committed_workflow_context(
                membership,
                &selected.association,
                &selected.correlation,
                &envelope.input_digest,
                &envelope.request,
                envelope.deadline_ms,
                &selected.output_audiences,
            )
            .await
            .map_err(|error| match error {
                meerkat_toolkit::setup::SetupError::Denied => PrivateOriginalError::Denied,
                meerkat_toolkit::setup::SetupError::Store(error) => store_error(error),
                meerkat_toolkit::setup::SetupError::NativeExecutionUnavailable
                | meerkat_toolkit::setup::SetupError::WorkGraph(_) => {
                    PrivateOriginalError::Unavailable
                }
                _ => PrivateOriginalError::Conflict,
            })?;
        if checked.terms() != &self.terms {
            return Err(PrivateOriginalError::Conflict);
        }
        let request = creation_request(&envelope)?;
        let admission = recipient
            .admission_identity(key.clone(), request.clone())
            .map_err(|_| PrivateOriginalError::Conflict)?;
        validate_item(&recipient, &admission, &request, &item)?;
        // The public service owns admission-state/digest classification. Do not
        // interpret a raw index hit as an admitted envelope by ourselves.
        self.with_context_recipient(source, |_| Ok(())).await?;
        let replay = recipient.observe_admission(key, request.clone()).await;
        self.with_context_recipient(source, |_| Ok(())).await?;
        match replay.map_err(|_| PrivateOriginalError::Unavailable)? {
            Some(WorkAdmissionOutcome::Replayed(exact)) => {
                validate_item(&recipient, &admission, &request, &exact)?
            }
            None => return Ok(WorkflowContextObservation::Pending),
            Some(_) => return Err(PrivateOriginalError::Conflict),
        }
        #[cfg(test)]
        if let Ok(pause) = AFTER_COMMITTED_READ.try_with(Clone::clone) {
            pause.0.notify_one();
            pause.1.notified().await;
        }
        let candidate = Arc::new(InstalledWorkflowContextTransfer {
            source: Arc::downgrade(source),
            original: Weak::new(),
            planning: Arc::downgrade(self),
            service: recipient,
            selected: envelope.recipient.clone(),
            attempt: Mutex::new(None),
            #[cfg(test)]
            checkpoint: std::sync::OnceLock::new(),
        });
        // Current G/P and a live sibling interest gate final registration. A
        // stopped host cannot add a slot after its existing final drain.
        let retained = candidate
            .with_recipient(source, |_| {
                control
                    .enter_transfer(|| {
                        let retained = self.retain_context_transfer(candidate.clone())?;
                        let mut slot = retained
                            .attempt
                            .lock()
                            .map_err(|_| PrivateOriginalError::Unavailable)?;
                        if let Some(prior) = slot.as_ref() {
                            if prior.envelope != envelope || prior.admission != admission {
                                return Err(PrivateOriginalError::Conflict);
                            }
                            // Leave an actual running warm task with its original
                            // owner. The normal observer joins that exact task below.
                        } else {
                            *slot = Some(Arc::new(TransferAttempt {
                                envelope,
                                request,
                                admission,
                                origin: TransferOrigin::ObservedCommitted,
                                recovery: Arc::downgrade(control),
                                delivery: Mutex::new(None),
                                advance: Mutex::new(None),
                            }));
                        }
                        drop(slot);
                        Ok(retained)
                    })
                    .ok_or(PrivateOriginalError::Unavailable)?
            })
            .await?;
        retained.observe_retained(source).await
    }
}
