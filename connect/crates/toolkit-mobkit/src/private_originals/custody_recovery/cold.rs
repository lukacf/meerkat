//! Readback of already enrolled rows from the actual intact installed pair.
//! This cursor and completed observations live in the same retained task. They
//! neither mint input nor replace the canonical row's decision or step budget.
use super::*;
use meerkat_runtime::SessionServiceRuntimeExt;
use crate::original_ingress::helper_originals;

#[derive(Clone, Copy)]
enum Kind { Original, Helper }
struct ColdEntry {
    kind: Kind,
    baseline: StoredRecord,
    observation: Option<NativeObservation>,
    last_observation: Option<tokio::time::Instant>,
    closed_failure: bool,
}
#[derive(Default)]
pub(super) struct ColdScan {
    helper: bool,
    after: Option<String>,
    complete: bool,
    closed_census_failure: bool,
    entries: Vec<ColdEntry>,
    cursor: usize,
}

#[cfg(test)]
impl ColdScan {
    pub(super) fn failed_observations(&self) -> Vec<(StoreKey, Option<meerkat_runtime::input_state::StoredInputState>)> {
        self.entries.iter().filter(|entry| entry.closed_failure).map(|entry| {
            let observed = match &entry.observation {
                Some(NativeObservation::Present(stored)) => Some((**stored).clone()),
                _ => None,
            };
            (entry.baseline.key.clone(), observed)
        }).collect()
    }
}

impl ColdEntry {
    fn attempt(&self, source: &PrivateOriginals, row: &StoredRecord)
        -> Result<CustodyAttempt, PrivateOriginalError>
    {
        if row.key != self.baseline.key { return Err(PrivateOriginalError::Conflict); }
        let attempt = source.cold_attempt(self.kind, row)?.ok_or(PrivateOriginalError::Conflict)?;
        // Validate the immutable baseline and all monotone decision fields even
        // on a read-only pass. Do not let another row/update reset this attempt.
        self.transition(source, row, attempt.clone())?;
        let original = source.cold_attempt(self.kind, &self.baseline)?.ok_or(PrivateOriginalError::Conflict)?;
        if !original.same_attempt(&attempt) {
            return Err(PrivateOriginalError::Conflict);
        }
        Ok(attempt)
    }
    fn transition(&self, source: &PrivateOriginals, row: &StoredRecord, next: CustodyAttempt)
        -> Result<Transition, PrivateOriginalError>
    {
        match self.kind {
            Kind::Original => {
                let current = source.cold_original(row)?;
                let prior = source.cold_original(&self.baseline)?;
                if !same_source_origin(&self.baseline, row)
                    || prior.final_association != current.final_association
                { return Err(PrivateOriginalError::Conflict); }
                PrivateOriginals::custody_transition(row, current, next)
            }
            Kind::Helper => helper_originals::cold_custody_transition(
                &source.scope, &self.baseline, row, next).map_err(cold_service_admission::writer_error),
        }
    }
    fn due(&self) -> bool {
        self.last_observation.map_or(true, |last|
            tokio::time::Instant::now().saturating_duration_since(last) >= ROUND_PAUSE)
    }
}

impl PrivateOriginals {
    fn verified_cold_input(&self, kind: Kind, row: &StoredRecord,
        attempt: &CustodyAttempt, stored: &meerkat_runtime::input_state::StoredInputState,
    ) -> Result<meerkat_runtime::Input, PrivateOriginalError> {
        // Native may retire a terminal input's payload while retaining its
        // exact replay and authority witnesses. Reuse only input bytes already
        // persisted by this canonical source; never reconstruct them from IDs.
        let retained = match kind {
            Kind::Original => self.cold_original(row)?.bound_service_input,
            Kind::Helper => helper_originals::cold_bound_input(&self.scope, row)
                .map_err(cold_service_admission::writer_error)?,
        };
        let input = match retained {
            Some(input) => input.decode()?,
            None => stored.state.persisted_input.clone().ok_or(PrivateOriginalError::Unavailable)?,
        };
        if stored.state.input_id != *attempt.input_id()
            || !attempt.matches_input(&input).map_err(cold_service_admission::writer_error)?
            || verify_exact_replay(&stored.state, &input).is_err()
        { return Err(PrivateOriginalError::Conflict); }
        Ok(input)
    }

    /// A newly reconstructed enrolled source cannot infer admitted custody from a
    /// terminal marker alone. Before a later input binds, rejoin the original
    /// exact native input and current runtime owners, without staging Candidate.
    pub(crate) async fn restore_promoted_custody(&self, key: &StoreKey,
        original: &Arc<RetainedToolkitOriginal>,
    ) -> Result<(), PrivateOriginalError> {
        if original.first_binding_for(key)
            || self.owner.has_admitted_source(original.association())
                .map_err(cold_service_admission::writer_error)?
        { return Ok(()); }
        self.owner.check_canonical_source(self.store.as_ref(), &self.scope)
            .map_err(cold_service_admission::writer_error)?;
        let (_, stamp) = self.owner.publication().observe(|| ())
            .map_err(|_| PrivateOriginalError::Unavailable)?;
        let row = self.store.get(key).await.map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let kind = if row.data.schema == SOURCE_SCHEMA { Kind::Original } else { Kind::Helper };
        let attempt = if matches!(kind, Kind::Original) {
            let record = self.cold_original(&row)?;
            if record.operation_cancelled || record.final_association.as_ref() != Some(original.association()) {
                return Err(PrivateOriginalError::Conflict);
            }
            record.custody_attempt
        } else {
            helper_originals::cold_bound_attempt(&self.scope, &row, original.association())
                .map_err(cold_service_admission::writer_error)?
        };
        let Some(attempt) = attempt else { return Ok(()); }; // Existing legacy restoration stays unchanged.
        if !self.owner.has_installed_pair_recovery().map_err(cold_service_admission::writer_error)? {
            return Err(PrivateOriginalError::Unavailable);
        }
        match attempt.decision() {
            CustodyDecision::Promoted => (),
            CustodyDecision::Pending => return Err(PrivateOriginalError::Unavailable),
            CustodyDecision::Withdrawn => return Err(PrivateOriginalError::Conflict),
        }
        let machine = self.machine()?;
        let stored = machine.input_state(attempt.session(), attempt.input_id()).await
            .map_err(|_| PrivateOriginalError::Unavailable)?.ok_or(PrivateOriginalError::Unavailable)?;
        let input = self.verified_cold_input(kind, &row, &attempt, &stored)?;
        let registration = machine.current_session_registration_witness(attempt.session()).await
            .ok_or(PrivateOriginalError::Unavailable)?;
        let attachment = machine.current_executor_attachment_witness(attempt.session()).await
            .ok_or(PrivateOriginalError::Unavailable)?;
        let pins = self.controller_pin_source.get().and_then(Weak::upgrade)
            .ok_or(PrivateOriginalError::Unavailable)?;
        let actor = pins.live_actor(attempt.session()).await.ok_or(PrivateOriginalError::Unavailable)?;
        machine.with_admitted_original(&registration, &attachment,
            &actor.executor_actor_identity(), &input, |admitted| {
                let control = self.custody_recovery.control.state.lock().map_err(|_| PrivateOriginalError::Unavailable)?;
                if !control.selected || control.stopped { return Err(PrivateOriginalError::Unavailable); }
                let _writer = self.owner.try_source_writer().map_err(cold_service_admission::writer_error)?;
                stamp.check_current().map_err(|_| PrivateOriginalError::Unavailable)?;
                self.owner.recover_retained_custody(original, admitted)
                    .map_err(cold_service_admission::writer_error)
            }).await.map_err(|_| PrivateOriginalError::Unavailable)?
    }

    fn cold_original(&self, row: &StoredRecord) -> Result<SourceRecord, PrivateOriginalError> {
        let record = self.decode_stored(row)?;
        let key = StoreKey { realm: self.scope.realm.clone(), owner: self.scope.owner.clone(),
            id: source_key_id(&record.source.realm, &record.source.ingress,
                &record.source.source_authority, &record.event.original)? };
        if row.key != key { return Err(PrivateOriginalError::Conflict); }
        let association = record.final_association.as_ref().ok_or(PrivateOriginalError::Unavailable)?;
        self.check_source_binding(&record, association).map_err(cold_service_admission::writer_error)?;
        Ok(record)
    }
    fn cold_attempt(&self, kind: Kind, row: &StoredRecord)
        -> Result<Option<CustodyAttempt>, PrivateOriginalError>
    {
        self.owner.check_canonical_source(self.store.as_ref(), &self.scope)
            .map_err(cold_service_admission::writer_error)?;
        match kind {
            Kind::Original => {
                if row.data.schema != SOURCE_SCHEMA { return Ok(None); }
                // Explicitly validate legacy rows before omitting enrollment.
                // Unknown/malformed selected versions remain unavailable.
                let decoded = self.decode_stored(row)?;
                if row.data.version == SOURCE_VERSION { return Ok(None); }
                if decoded.custody_attempt.is_none() { return Err(PrivateOriginalError::Conflict); }
                Ok(self.cold_original(row)?.custody_attempt)
            }
            Kind::Helper => helper_originals::cold_custody_attempt(&self.scope, row)
                .map_err(cold_service_admission::writer_error),
        }
    }
    async fn cold_row(&self, entry: &ColdEntry) -> Result<(StoredRecord, CustodyAttempt), PrivateOriginalError> {
        let row = self.store.get(&entry.baseline.key).await.map_err(store_error)?
            .ok_or(PrivateOriginalError::Unavailable)?;
        let attempt = entry.attempt(self, &row)?;
        Ok((row, attempt))
    }
    async fn cold_change(&self, entry: &ColdEntry, change: CustodyChange)
        -> Result<CustodyAttempt, PrivateOriginalError>
    {
        let _writer = self.owner.source_writer().await;
        // A foreground write cannot be mistaken for cold custody, including
        // before its first Candidate exists. Recheck under its actual writer.
        if self.owner.has_live_first_binding(&entry.baseline.key).map_err(cold_service_admission::writer_error)? {
            return Err(PrivateOriginalError::Unavailable);
        }
        let (row, current) = self.cold_row(entry).await?;
        if matches!((current.decision(), change),
            (CustodyDecision::Promoted, CustodyChange::Promote) |
            (CustodyDecision::Withdrawn, CustodyChange::Withdraw)) { return Ok(current); }
        let next = current.changed(change).map_err(cold_service_admission::writer_error)?;
        let saved = self.store.compare_and_swap(entry.transition(self, &row, next.clone())?).await.map_err(store_error)?;
        if entry.attempt(self, &saved)? != next { return Err(PrivateOriginalError::Conflict); }
        Ok(next)
    }
    async fn publish_cold_terminal(&self, entry: &ColdEntry) -> Result<(), PrivateOriginalError> {
        let _writer = self.owner.source_writer().await;
        if self.owner.has_live_first_binding(&entry.baseline.key).map_err(cold_service_admission::writer_error)? {
            return Err(PrivateOriginalError::Unavailable);
        }
        let (row, current) = self.cold_row(entry).await?;
        match current.decision() {
            CustodyDecision::Pending => return Err(PrivateOriginalError::Unavailable),
            CustodyDecision::Promoted => (), // Admission still needs native restoration.
            CustodyDecision::Withdrawn => {
                let (association, cancelled) = match entry.kind {
                    Kind::Original => {
                        let source = self.cold_original(&row)?;
                        (source.final_association.ok_or(PrivateOriginalError::Conflict)?, source.operation_cancelled)
                    }
                    Kind::Helper => (helper_originals::cold_association(&self.scope, &row)
                        .map_err(cold_service_admission::writer_error)?, false),
                };
                self.owner.retain_cold_withdrawal(association, Arc::new(row), cancelled)
                    .map_err(cold_service_admission::writer_error)?;
            }
        }
        // Announce only after exact durable readback is synchronously readable.
        self.custody_recovery.announce();
        Ok(())
    }

    async fn recover_cold_entry(&self, entry: &mut ColdEntry) -> Result<bool, PrivateOriginalError> {
        if self.owner.has_live_first_binding(&entry.baseline.key).map_err(cold_service_admission::writer_error)? {
            return Ok(false);
        }
        let (row, current) = self.cold_row(entry).await?;
        if current.decision() != CustodyDecision::Pending {
            self.publish_cold_terminal(entry).await?;
            return Ok(false);
        }
        if !current.observation_entered() {
            entry.observation = None;
            if self.custody_recovery.stopped() || !current.can_observe() {
                self.cold_change(entry, CustodyChange::Withdraw).await?;
                self.publish_cold_terminal(entry).await?;
                return Ok(false);
            }
            if !entry.due() { return Ok(true); }
            self.cold_change(entry, CustodyChange::ChargeResolution).await?;
        }
        // PLAN-R3 bounds source decision steps, not physical readbacks across
        // arbitrary process crashes. A cold entered step completes that SAME
        // debit, including the last one, without reset or new native submission.
        // Within this process the exact completed observation stays retained
        // until canonical settlement, even when a CAS reply is lost.
        let observation = match &entry.observation {
            Some(observed) => observed.clone(),
            None => {
                if !entry.due() { return Ok(true); }
                let machine = self.machine()?;
                entry.last_observation = Some(tokio::time::Instant::now());
                let observed = match machine.input_state(current.session(), current.input_id()).await {
                    Ok(Some(stored)) => NativeObservation::Present(Box::new(stored)),
                    Ok(None) => NativeObservation::Absent,
                    Err(_) => NativeObservation::Unavailable,
                };
                entry.observation = Some(observed.clone());
                observed
            }
        };
        if let NativeObservation::Present(stored) = observation {
            self.verified_cold_input(entry.kind, &row, &current, &stored)?;
            self.cold_change(entry, CustodyChange::Promote).await?;
            self.publish_cold_terminal(entry).await?;
            return Ok(false);
        }
        self.cold_change(entry, CustodyChange::ResolutionSettled).await?;
        Ok(true)
    }

    async fn finish_closed_cold_error(&self, entry: &ColdEntry) -> bool {
        // Resolve a lost terminal write reply through exact readback only.
        // Pending/unavailable readback retains the completed observation.
        let Ok((_, current)) = self.cold_row(entry).await else { return false; };
        if current.decision() == CustodyDecision::Pending { return false; }
        self.publish_cold_terminal(entry).await.is_ok()
    }

    pub(super) async fn recover_cold_round(&self, scan: &mut ColdScan)
        -> RoundProgress
    {
        let mut progress = RoundProgress::default();
        let census = async {
            if !self.owner.has_installed_pair_recovery().map_err(cold_service_admission::writer_error)? {
                scan.complete = true;
                return Ok(());
            }
            let mut unavailable = false;
            // At most one page per selected scope per round. All page reads belong
            // to this retained future; cancelled join cannot abandon an entered read.
            for _ in 0..2 {
                if scan.complete || scan.closed_census_failure { break; }
                let scope = if scan.helper { StoreScope { realm: self.scope.realm.clone(),
                    owner: format!("toolkit/helper-originals/{}", self.scope.owner) } } else { self.scope.clone() };
                let kind = if scan.helper { Kind::Helper } else { Kind::Original };
                let page = self.store.list_records(&scope, scan.after.as_deref(), ROUND_LIMIT as u32)
                    .await.map_err(store_error)?;
                for row in &page {
                    if self.owner.has_live_first_binding(&row.key).map_err(cold_service_admission::writer_error)? { continue; }
                    match self.cold_attempt(kind, row) {
                        Ok(Some(attempt)) if attempt.decision() == CustodyDecision::Pending => {
                            if !scan.entries.iter().any(|entry| entry.baseline.key == row.key) {
                                scan.entries.push(ColdEntry { kind, baseline: row.clone(), observation: None, last_observation: None, closed_failure: false });
                            }
                        }
                        Ok(Some(_)) => {
                            // Close the commit-before-publication crash window
                            // using this same canonical row and source owner.
                            let entry = ColdEntry { kind, baseline: row.clone(), observation: None,
                                last_observation: None, closed_failure: false };
                            self.publish_cold_terminal(&entry).await?;
                        }
                        Ok(None) => (),
                        Err(_) => unavailable = true,
                    }
                }
                // Malformed selected data cannot silently become absent custody.
                // While open, retry this page; after stop the task may join with
                // Unavailable after all selected calls return, retaining their evidence.
                if unavailable { break; }
                if page.len() < ROUND_LIMIT {
                    if scan.helper { scan.complete = true; } else { scan.helper = true; scan.after = None; }
                } else { scan.after = page.last().map(|row| row.key.id.clone()); break; }
            }
            if unavailable { Err(PrivateOriginalError::Unavailable) } else { Ok(()) }
        }.await;
        if census.is_err() {
            progress.unavailable = true;
            if self.custody_recovery.stopped() { scan.closed_census_failure = true; }
        }
        progress.unavailable |= scan.closed_census_failure;
        let count = scan.entries.iter().filter(|entry| !entry.closed_failure).count();
        if count > 0 {
            let offset = scan.cursor % scan.entries.len();
            scan.entries.rotate_left(offset);
            scan.cursor = ROUND_LIMIT.min(count) % count;
            let mut remaining = Vec::new();
            let mut visited = 0;
            // Keep all actual entered work, including unavailable writes. The
            // cursor bounds per-round work and cannot discard another entry's
            // completed native observation to make room for a later page.
            for mut entry in std::mem::take(&mut scan.entries) {
                if entry.closed_failure { progress.unavailable = true; remaining.push(entry); continue; }
                if visited >= ROUND_LIMIT { remaining.push(entry); continue; }
                visited += 1;
                match self.recover_cold_entry(&mut entry).await {
                    Ok(false) => (),
                    Ok(true) => remaining.push(entry),
                    Err(_) => {
                        if self.custody_recovery.stopped() {
                            if self.finish_closed_cold_error(&entry).await { continue; }
                            entry.closed_failure = true;
                        }
                        progress.unavailable = true;
                        remaining.push(entry);
                    }
                }
            }
            scan.entries = remaining;
        }
        progress.unavailable |= scan.entries.iter().any(|entry| entry.closed_failure);
        progress.pending = (!scan.complete && !scan.closed_census_failure)
            || scan.entries.iter().any(|entry| !entry.closed_failure);
        progress
    }
}
