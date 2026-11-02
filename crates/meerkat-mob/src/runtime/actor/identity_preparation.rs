//! Host preparation on the actor (0.9.9): a settle's commit step, a held
//! construction's start, its hold kept in the mob's registry, its abort and
//! its end (see [`super::super::identity_preparation`]).
//!
//! Each step decides on one prepared authority from the facts it observes
//! there: the store's receipt of a settle's operation and the requester's
//! grant (the change's admission), the identity's row (its preparation's
//! mode, its attempt, its spec's digest), and, for a settle naming an
//! attempt, the hold the registry keeps (its deadline and the spec digest
//! its construction was built from). A step writes its store transaction
//! before MobMachine commits; a refusal writes nothing.

use super::super::identity_change::{
    IdentityChangeRefusal, committed_change_events, now_ms, refused,
};
#[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
use super::super::identity_preparation::{
    ConstructionBegun, ConstructionOutcome, HeldConstruction, IdentityConstructionRequest,
    construction_end_of, construction_refusal, settle_published_construction_row,
};
use super::super::identity_preparation::{
    ConstructionEnd, PreparationSettleCommit, PreparationSettleCommitted, PreparationSettlement,
    settle_refusal,
};
use super::identity_change::ChangeAdmission;
use super::*;
use crate::identity::{
    CreatedIdentityObservation, CreatedIdentityPresence, CreatedIdentityRecord,
    IdentityChangeEntryOutcome, IdentityChangeKind, IdentityChangeReceipt,
    IdentityChangeReceiptEntry,
};
use crate::machines::mob_machine::MobMachineMutator as _;
use crate::store::{CreatedIdentityRowExpectation, CreatedIdentityRowWrite, IdentityChangeWrite};

impl MobActor {
    /// `identity`'s readable store-owned row, if it has one.
    async fn preparation_row(
        &self,
        identity: &AgentIdentity,
    ) -> Result<Option<CreatedIdentityRecord>, MobError> {
        let store = self
            .identity_member
            .as_ref()
            .ok_or_else(|| refused(IdentityChangeRefusal::StorageUnsupported))?;
        Ok(
            match store
                .observe_created_identity(&self.definition.id, identity)
                .await?
            {
                Some(CreatedIdentityObservation::Created(row)) => Some(row),
                _ => None,
            },
        )
    }

    /// The facts of the attempt a settle names, from the hold the registry
    /// keeps: whether the settle was decided before the attempt's deadline
    /// (its activation not sent yet), and whether the construction binds
    /// the identity's current spec digest and the settle's expected one.
    #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
    fn held_attempt_facts(
        &self,
        commit: &PreparationSettleCommit,
        decided_at: super::super::held_spawn::HeldSpawnDeadline,
        current_digest: Option<&str>,
    ) -> (bool, bool) {
        let Some(attempt) = commit.attempt else {
            return (false, false);
        };
        let Some(held) = self.held_constructions.held(&commit.identity) else {
            return (false, false);
        };
        if held.attempt != attempt {
            return (false, false);
        }
        let before_deadline = decided_at < held.deadline && held.decision.is_some();
        let binding_matches = held.spec_digest.as_deref() == current_digest
            && commit
                .expected_spec_digest
                .as_deref()
                .is_none_or(|expected| held.spec_digest.as_deref() == Some(expected));
        (before_deadline, binding_matches)
    }

    /// What a replayed settle's acceptance came to: an activation accepted
    /// under the operation published (the identity is seated), still in
    /// flight, or ended (failed, or interrupted by a restart, which holds no
    /// attempt). `None` for a settle without a construction.
    fn replayed_settlement(
        &self,
        identity: &AgentIdentity,
        attempt: Option<u64>,
        receipt: &IdentityChangeReceipt,
    ) -> Option<PreparationSettlement> {
        let accepted = receipt.identities.iter().any(|entry| {
            entry.identity == *identity && entry.outcome == IdentityChangeEntryOutcome::Activating
        });
        if !accepted {
            return None;
        }
        let attempt = attempt.unwrap_or_default();
        let state = self.dsl_authority.state();
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(identity);
        Some(
            if state.identity_to_runtime.contains_key(&dsl_identity)
                && !state.identity_entry_unseated.contains_key(&dsl_identity)
            {
                PreparationSettlement::Activated { attempt }
            } else if state
                .identity_entry_constructions_activating
                .get(&dsl_identity)
                == Some(&attempt)
            {
                PreparationSettlement::ActivationPending { attempt }
            } else {
                let end = match self.held_constructions.ended(identity) {
                    Some((ended, end)) if ended == attempt => end,
                    _ => ConstructionEnd::PublicationInterrupted,
                };
                PreparationSettlement::PublicationEnded { attempt, end }
            },
        )
    }

    /// Commit a settle of one identity's preparation in one actor step:
    /// MobMachine decides the change's admission and the settle's verdict
    /// (`SettleHostPreparation`) on one prepared authority. Without a
    /// construction, the store-owned identity's row (present again) and the
    /// receipt are written in one store transaction, and the entry is owed
    /// its seat, which the handle carries out. An accepted activation is
    /// sent to the attempt's hold in this step, decided before its deadline
    /// (the publication's membership commit waits for the step), and its
    /// receipt journals the acceptance before MobMachine commits it.
    pub(super) async fn settle_identity_preparation_step(
        &mut self,
        commit: PreparationSettleCommit,
    ) -> Result<PreparationSettleCommitted, MobError> {
        let store = self
            .identity_member
            .clone()
            .ok_or_else(|| refused(IdentityChangeRefusal::StorageUnsupported))?;
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(&commit.identity);
        let row = self.preparation_row(&commit.identity).await?;
        // The preparation's mode is the store-owned identity's row's; a
        // roster-owned identity's host settles a construction only under a
        // `constructed` rule.
        let (store_owned, constructed) = match row.as_ref().map(|row| &row.presence) {
            Some(CreatedIdentityPresence::PendingPreparation { constructed, .. }) => {
                (true, *constructed)
            }
            Some(_) => (true, false),
            None => (false, commit.attempt.is_some()),
        };
        let mut authority = self.dsl_authority.prepare_authority();
        match self
            .decide_change_admission(
                &mut authority,
                &commit.operation,
                commit.facts,
                commit.grant.as_ref(),
                commit.decided,
                None,
            )
            .await?
        {
            ChangeAdmission::Replayed(receipt) => {
                let settlement =
                    self.replayed_settlement(&commit.identity, commit.attempt, &receipt);
                return Ok(PreparationSettleCommitted::Replayed {
                    receipt,
                    settlement,
                });
            }
            ChangeAdmission::Admitted => {}
        }
        // A roster-owned identity's `required` preparation is settled by its
        // host's declaration, which names no preparation.
        if !store_owned
            && commit.attempt.is_none()
            && authority.state().identity_entry_unseated.get(&dsl_identity)
                == Some(&mob_dsl::UnseatedEntryCause::PreparationPending)
        {
            return Err(refused(IdentityChangeRefusal::OwnerReadOnly {
                owner: "roster",
            }));
        }
        let current_spec = row.as_ref().and_then(|row| {
            row.spec_digest()
                .ok()
                .flatten()
                .map(|digest| (digest, row.revision))
        });
        #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
        let decided_at = crate::tokio::time::Instant::now();
        #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
        let (before_deadline, binding_matches) = self.held_attempt_facts(
            &commit,
            decided_at,
            current_spec.as_ref().map(|(digest, _)| digest.as_str()),
        );
        // No construction is held on a build without held spawns.
        #[cfg(not(all(feature = "runtime-adapter", not(target_arch = "wasm32"))))]
        let (before_deadline, binding_matches) = (false, false);
        let transition = authority
            .apply(mob_dsl::MobMachineInput::SettleHostPreparation {
                agent_identity: dsl_identity.clone(),
                attempt: commit.attempt,
                constructed,
                before_deadline,
                binding_matches,
            })
            .map_err(|error| MobError::MobMachineRejected {
                context: "settle_host_preparation",
                reason: error.to_string(),
            })?;
        let (verdict, held_attempt, activating_attempt) = transition
            .effects()
            .iter()
            .find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::PreparationSettleResolved {
                    verdict,
                    held_attempt,
                    activating_attempt,
                    ..
                } => Some((*verdict, *held_attempt, *activating_attempt)),
                _ => None,
            })
            .ok_or_else(|| {
                MobError::Internal("MobMachine settled a preparation with no verdict".into())
            })?;
        if let Some(refusal) = settle_refusal(
            verdict,
            &commit.identity,
            commit.attempt,
            held_attempt.or(activating_attempt),
        ) {
            return Err(refused(refusal));
        }
        let at_ms = now_ms();
        let receipt = |outcome: IdentityChangeEntryOutcome| {
            let mut entry = IdentityChangeReceiptEntry::new(commit.identity.clone(), outcome);
            if let Some((digest, revision)) = &current_spec {
                entry = entry.with_spec(digest.clone(), Some(*revision));
            }
            IdentityChangeReceipt::sealed(
                self.definition.id.clone(),
                commit.operation.operation_id.clone(),
                commit.operation.request_digest.clone(),
                IdentityChangeKind::SettlePreparation,
                vec![entry],
                at_ms,
            )
            .and_then(|receipt| {
                receipt.with_attribution(commit.requester.clone(), commit.consent.clone())
            })
            .map_err(|error| MobError::Internal(format!("a settle's receipt: {error}")))
        };
        let prepared = PreparedDslInput {
            authority,
            effects: transition.into_effects(),
            phase_changed: false,
        };
        match verdict {
            mob_dsl::PreparationSettleVerdict::Seat => {
                let row = row.ok_or_else(|| {
                    MobError::Internal(format!(
                        "the store-owned identity '{}' has no row",
                        commit.identity
                    ))
                })?;
                let spec = row.spec.clone().ok_or_else(|| {
                    MobError::Internal(format!(
                        "the row of store-owned '{}' holds no spec",
                        commit.identity
                    ))
                })?;
                let held_creation = row.held_creation;
                let expected = CreatedIdentityRowExpectation::Current {
                    authority_digest: row.authority_digest.clone(),
                };
                let record = row
                    .with_presence(CreatedIdentityPresence::Present)
                    .map_err(|error| {
                        MobError::Internal(format!("a store-owned identity's row: {error}"))
                    })?;
                let receipt = receipt(IdentityChangeEntryOutcome::Committed)?;
                let written = receipt.clone();
                self.commit_prepared_dsl_input_after(prepared, move || async move {
                    let commit = store
                        .commit_identity_change(&IdentityChangeWrite {
                            receipt: written,
                            rows: vec![CreatedIdentityRowWrite { record, expected }],
                            events: Vec::new(),
                        })
                        .await
                        .map_err(MobError::from)?;
                    committed_change_events(commit).map(|_| ())
                })
                .await?;
                Ok(PreparationSettleCommitted::Seat {
                    receipt,
                    spec: Box::new(spec),
                    held_creation,
                })
            }
            #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
            mob_dsl::PreparationSettleVerdict::ActivationAccepted => {
                let attempt = commit.attempt.unwrap_or_default();
                let stale = || {
                    refused(IdentityChangeRefusal::StaleAttempt {
                        identity: commit.identity.clone(),
                        current: None,
                    })
                };
                // The receipt is sealed before anything is sent, so only the
                // store write can fail after the activation left.
                let receipt = receipt(IdentityChangeEntryOutcome::Activating)?;
                // The acceptance's linearization point: the activation,
                // decided before the deadline, goes to the hold now. A hold
                // that already ended (its deadline) accepts nothing. Should
                // the store write below fail, MobMachine still holds the
                // attempt unaccepted, so the publication's membership commit
                // (which waits for this step) is refused, and the hold's end
                // (a failed publication) ends the attempt with the identity
                // still pending.
                let held = self
                    .held_constructions
                    .held_mut(&commit.identity)
                    .ok_or_else(stale)?;
                let decision = held.decision.take().ok_or_else(stale)?;
                if decision
                    .send(super::super::held_spawn::HeldSpawnDecision::Activate { decided_at })
                    .is_err()
                {
                    return Err(stale());
                }
                held.activation_sent = true;
                let outcome = held.outcome.subscribe();
                let written = receipt.clone();
                self.commit_prepared_dsl_input_after(prepared, move || async move {
                    let commit = store
                        .commit_identity_change(&IdentityChangeWrite {
                            receipt: written,
                            rows: Vec::new(),
                            events: Vec::new(),
                        })
                        .await
                        .map_err(MobError::from)?;
                    committed_change_events(commit).map(|_| ())
                })
                .await?;
                Ok(PreparationSettleCommitted::Accepted {
                    receipt,
                    attempt,
                    outcome,
                })
            }
            other => Err(MobError::Internal(format!(
                "MobMachine settled the preparation of '{}' with {other:?}",
                commit.identity
            ))),
        }
    }

    /// Begin a held construction of one identity in one actor step: the
    /// same identity and operation's held attempt answers its observer (a
    /// lost reply asked again); otherwise MobMachine decides the start
    /// (`BeginPreparationConstruction`) of the next attempt (a store-owned
    /// identity's row's attempt plus one, written with the step; a
    /// roster-owned identity's next one in this process) and the spec its
    /// held member is built from: the row's, under created custody, or the
    /// roster's, under entry custody.
    #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
    pub(super) async fn begin_identity_construction(
        &mut self,
        request: IdentityConstructionRequest,
    ) -> Result<ConstructionBegun, MobError> {
        let store = self
            .identity_member
            .clone()
            .ok_or_else(|| refused(IdentityChangeRefusal::StorageUnsupported))?;
        let identity = request.identity.clone();
        if let Some(held) = self.held_constructions.held(&identity)
            && held.operation_id == request.operation.operation_id
            && held.decision.is_some()
        {
            return Ok(ConstructionBegun::Existing(Box::new(held.observer.clone())));
        }
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(&identity);
        let invalid = |error: crate::identity::IdentityIntentError| {
            MobError::Internal(format!("a store-owned identity's row: {error}"))
        };
        let (constructed, attempt, spec, spec_digest, row_write) =
            match self.preparation_row(&identity).await? {
                Some(row) => match row.presence {
                    CreatedIdentityPresence::PendingPreparation {
                        attempt,
                        constructed,
                    } => {
                        let next = attempt.saturating_add(1);
                        let spec = row
                            .spec
                            .as_ref()
                            .map(super::super::identity_change::lower_created_spec)
                            .transpose()
                            .map_err(refused)?;
                        let digest = row.spec_digest().ok().flatten();
                        let expected = CreatedIdentityRowExpectation::Current {
                            authority_digest: row.authority_digest.clone(),
                        };
                        let record = row
                            .with_presence(CreatedIdentityPresence::PendingPreparation {
                                attempt: next,
                                constructed,
                            })
                            .map_err(invalid)?;
                        (
                            constructed,
                            next,
                            spec,
                            digest,
                            Some(CreatedIdentityRowWrite { record, expected }),
                        )
                    }
                    // MobMachine answers an identity awaiting no
                    // preparation.
                    _ => (false, 0, None, None, None),
                },
                None => match request.roster_spec {
                    Some(spec) => (
                        true,
                        self.held_constructions.next_roster_attempt(&identity),
                        Some(spec.with_entry_custody(true)),
                        None,
                        None,
                    ),
                    None => {
                        // A roster-owned identity awaiting its preparation:
                        // its host names the spec it declares.
                        if self
                            .dsl_authority
                            .state()
                            .identity_entry_unseated
                            .get(&dsl_identity)
                            == Some(&mob_dsl::UnseatedEntryCause::PreparationPending)
                        {
                            return Err(refused(IdentityChangeRefusal::InvalidRequest {
                                problem: "a roster-owned identity's construction names its spec"
                                    .to_string(),
                            }));
                        }
                        (false, 0, None, None, None)
                    }
                },
            };
        let mut authority = self.dsl_authority.prepare_authority();
        let transition = authority
            .apply(mob_dsl::MobMachineInput::BeginPreparationConstruction {
                agent_identity: dsl_identity,
                attempt,
                constructed,
            })
            .map_err(|error| MobError::MobMachineRejected {
                context: "begin_preparation_construction",
                reason: error.to_string(),
            })?;
        let (verdict, held_attempt, activating_attempt) = transition
            .effects()
            .iter()
            .find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::PreparationConstructionResolved {
                    verdict,
                    held_attempt,
                    activating_attempt,
                    ..
                } => Some((*verdict, *held_attempt, *activating_attempt)),
                _ => None,
            })
            .ok_or_else(|| {
                MobError::Internal("MobMachine began a construction with no verdict".into())
            })?;
        if let Some(refusal) =
            construction_refusal(verdict, &identity, held_attempt.or(activating_attempt))
        {
            return Err(refused(refusal));
        }
        let spec = spec.ok_or_else(|| {
            MobError::Internal(format!("the construction of '{identity}' has no spec"))
        })?;
        let prepared = PreparedDslInput {
            authority,
            effects: transition.into_effects(),
            phase_changed: false,
        };
        let changed = identity.clone();
        self.commit_prepared_dsl_input_after(prepared, move || async move {
            if let Some(row) = row_write
                && !store
                    .settle_created_identity_row(&row)
                    .await
                    .map_err(MobError::from)?
            {
                return Err(MobError::Internal(format!(
                    "the row of '{changed}' changed while its construction began: retry"
                )));
            }
            Ok(())
        })
        .await?;
        Ok(ConstructionBegun::Held {
            attempt,
            spec: Box::new(spec),
            spec_digest,
        })
    }

    /// Keep the hold of `identity`'s attempt in the registry, while
    /// MobMachine still holds the attempt. An attempt that ended before its
    /// hold reached the registry (an abort, a removal, the mob's reset) is
    /// refused: the dropped hold aborts.
    #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
    pub(super) fn adopt_identity_construction(
        &mut self,
        identity: AgentIdentity,
        construction: HeldConstruction,
    ) -> Result<(), MobError> {
        let held = self
            .dsl_authority
            .state()
            .identity_entry_constructions_held
            .get(&mob_dsl::AgentIdentity::from_domain(&identity))
            .copied();
        if held != Some(construction.attempt) {
            return Err(refused(IdentityChangeRefusal::StaleAttempt {
                identity,
                current: held,
            }));
        }
        self.held_constructions.keep(identity, construction);
        Ok(())
    }

    /// The end of `identity`'s `attempt`, from its spawn's completion: a
    /// published construction's membership commit ended the attempt, and a
    /// store-owned identity's row is present again; otherwise MobMachine
    /// ends the attempt (`EndPreparationConstruction`) with how it ended,
    /// the identity still pending. The registry lets the hold go and
    /// reports the end to a settle or an abort that waits on it.
    #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
    pub(super) async fn end_identity_construction(
        &mut self,
        identity: AgentIdentity,
        attempt: u64,
        completion: Result<(), MobError>,
    ) -> Result<(), MobError> {
        let held = self.held_constructions.release(&identity, attempt);
        let outcome = match completion {
            Ok(()) => {
                if let Some(store) = self.identity_member.as_ref()
                    && let Err(error) = settle_published_construction_row(
                        store.as_ref(),
                        &self.definition.id,
                        &identity,
                    )
                    .await
                {
                    tracing::warn!(
                        mob_id = %self.definition.id,
                        agent_identity = %identity,
                        error = %error,
                        "a published construction's row was not settled: the next start settles it"
                    );
                }
                ConstructionOutcome::Published
            }
            Err(error) => {
                let activation_sent = held.as_ref().is_some_and(|held| held.activation_sent);
                let (end, cleanup) = construction_end_of(&error, activation_sent);
                if let Some(kind) = end.machine_kind() {
                    self.apply_dsl_input(
                        mob_dsl::MobMachineInput::EndPreparationConstruction {
                            agent_identity: mob_dsl::AgentIdentity::from_domain(&identity),
                            attempt,
                            end: kind,
                        },
                        "end_preparation_construction",
                    )?;
                    self.publish_machine_state_projection();
                }
                self.held_constructions.record_end(&identity, attempt, end);
                ConstructionOutcome::Ended { end, cleanup }
            }
        };
        if let Some(held) = held {
            let _ = held.outcome.send(Some(outcome));
        }
        Ok(())
    }

    /// Abort `identity`'s held `attempt` in one actor step: MobMachine ends
    /// it (`EndPreparationConstruction`, aborted) unless its activation was
    /// accepted, and the abort goes to its hold. Answers the report of the
    /// hold's end, which its completion sends.
    #[cfg(all(feature = "runtime-adapter", not(target_arch = "wasm32")))]
    pub(super) fn abort_identity_construction_step(
        &mut self,
        identity: AgentIdentity,
        attempt: u64,
    ) -> Result<crate::tokio::sync::watch::Receiver<Option<ConstructionOutcome>>, MobError> {
        let dsl_identity = mob_dsl::AgentIdentity::from_domain(&identity);
        let effects = self.apply_dsl_input_collect_effects(
            mob_dsl::MobMachineInput::EndPreparationConstruction {
                agent_identity: dsl_identity.clone(),
                attempt,
                end: mob_dsl::ConstructionEndKind::Aborted,
            },
            "abort_preparation_construction",
        )?;
        self.publish_machine_state_projection();
        let verdict = effects
            .iter()
            .find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::PreparationConstructionEnded { verdict, .. } => {
                    Some(*verdict)
                }
                _ => None,
            })
            .ok_or_else(|| {
                MobError::Internal("MobMachine ended a construction with no verdict".into())
            })?;
        match verdict {
            mob_dsl::ConstructionEndVerdict::Ended => {
                let report = match self.held_constructions.held_mut(&identity) {
                    Some(held) if held.attempt == attempt => {
                        if let Some(decision) = held.decision.take() {
                            let _ =
                                decision.send(super::super::held_spawn::HeldSpawnDecision::Abort);
                        }
                        held.outcome.subscribe()
                    }
                    // The attempt's hold has not reached the registry: its
                    // adoption is refused and the dropped hold aborts.
                    _ => {
                        let (_, report) =
                            crate::tokio::sync::watch::channel(Some(ConstructionOutcome::Ended {
                                end: ConstructionEnd::Aborted,
                                cleanup: None,
                            }));
                        report
                    }
                };
                self.held_constructions
                    .record_end(&identity, attempt, ConstructionEnd::Aborted);
                Ok(report)
            }
            mob_dsl::ConstructionEndVerdict::ActivationPending => {
                Err(refused(IdentityChangeRefusal::ActivationPending {
                    identity,
                    attempt,
                }))
            }
            mob_dsl::ConstructionEndVerdict::NotOpen => {
                let state = self.dsl_authority.state();
                let current = state
                    .identity_entry_constructions_held
                    .get(&dsl_identity)
                    .or_else(|| {
                        state
                            .identity_entry_constructions_activating
                            .get(&dsl_identity)
                    })
                    .copied();
                Err(refused(IdentityChangeRefusal::StaleAttempt {
                    identity,
                    current,
                }))
            }
        }
    }
}
