//! Private acquisition-owner enumeration of its existing continuation effects.
//! This page is historical work selection, never native retry authority.
use super::*;
use toolkit_store::{EffectKey, StoreScope};

/// Content-free projection from this exact SetupStore scope. The host must
/// guard every read outcome before using it and use step_continuation for work.
/// A cursor is only pagination; it is neither a checkpoint nor a new journal.
pub struct ContinuationRecoveryPage {
    pub operations: Vec<Id>,
    /// Due nonterminal requests, not expiry permission. The selected host must
    /// authorize each exact snapshot through SetupDeadlineOwner before expire.
    pub expiry_operations: Vec<Id>,
    pub next_after_id: Option<String>,
    pub had_unavailable_records: bool,
}

impl SetupStore<'_> {
    /// Read one bounded page from the selected acquisition owner. Each record
    /// is recovered with the existing schema/identity checks. Corrupt records
    /// stay unresolved and do not starve other acquisitions in this scope.
    /// Neither content nor native admission receipts escape this projection.
    pub async fn continuation_recovery_page(
        &self,
        after_id: Option<&str>,
        limit: u32,
    ) -> Result<ContinuationRecoveryPage, SetupError> {
        if !(1..=64).contains(&limit) {
            return Err(
                StoreError::Invalid("invalid acquisition recovery page size".into()).into(),
            );
        }
        let rows = self
            .store
            .list_records(
                &StoreScope {
                    realm: self.scope.tenant.to_string(),
                    owner: format!("toolkit/acquisition/{}", self.scope.deployment),
                },
                after_id,
                limit,
            )
            .await?;
        let next_after_id = if rows.len() == limit as usize {
            rows.last().map(|row| row.key.id.clone())
        } else {
            None
        };
        let mut page = ContinuationRecoveryPage {
            operations: Vec::new(),
            expiry_operations: Vec::new(),
            next_after_id,
            had_unavailable_records: false,
        };
        let now = u64::try_from(self.store.now_ms().await?)
            .map_err(|_| SetupError::InvalidRecord)?;
        for row in rows {
            let snapshot = match self.recover(row) {
                Ok(snapshot) => snapshot,
                Err(_) => { page.had_unavailable_records = true; continue; }
            };
            if snapshot.state().terminal_revision == 0 {
                if snapshot.state().deadline <= now {
                    page.expiry_operations.push(operation_id(snapshot.request())?);
                }
                continue;
            }
            match self.recovery_operation(snapshot).await {
                Ok(Some(operation)) => page.operations.push(operation),
                Ok(None) => (),
                Err(_) => page.had_unavailable_records = true,
            }
        }
        Ok(page)
    }

    async fn recovery_operation(&self, snapshot: SetupSnapshot) -> Result<Option<Id>, SetupError> {
        let state = snapshot.state();
        // The generated terminal state must have its exact existing outbox
        // intent. Absence or contradiction is unavailable, not completion.
        let (_, intents) = handoffs(
            &snapshot.record,
            vec![AcquisitionEffect::ContinuationRequested {
                operation_id: state.operation_id.clone(),
                continuation_id: state.continuation_id.clone(),
                terminal_revision: state.terminal_revision,
                outcome: state.lifecycle_phase,
            }],
        )?;
        let intent = intents
            .into_iter()
            .next()
            .ok_or(SetupError::InvalidRecord)?;
        let key = EffectKey {
            record: snapshot.key().clone(),
            id: intent.id.clone(),
        };
        let effect = self
            .store
            .get_effect(&key)
            .await?
            .ok_or(SetupError::InvalidRecord)?;
        let obligation = continuation_obligation(&snapshot.record, &snapshot.request)?;
        super::continuation_executor::check_effect(&effect, &key, &intent, &obligation)?;
        // Budget exhaustion alone is not terminal: uncertain/pending prior
        // attempts remain candidates for the existing observation path.
        if super::continuation_executor::recovery_effect_settled(
            &effect, &obligation, snapshot.record.continuation_receipt.as_ref(),
        )? {
            return Ok(None);
        }
        Ok(Some(operation_id(snapshot.request())?))
    }
}
