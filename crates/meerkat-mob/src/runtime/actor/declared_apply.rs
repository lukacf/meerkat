//! The declared apply on the actor
//! ([`super::super::MobHandle::apply_declared_roster`]): one MobMachine step
//! decides a chunk of declared identities (`DeclareIdentityEntry`), and one
//! seals a complete roster (`SealDeclaredRoster`).
//!
//! The verdicts write no journal row: what they record (an unseated entry and
//! its cause, a role hold) is declared again by the next apply, and the seal's
//! mark is recorded on the start row of the retirement it causes. The handle
//! realizes each verdict with the mob's existing steps. See
//! `runtime::declared_apply` for the apply and its answers.

use super::identity_change::ChangeAdmission;
use super::*;
use crate::runtime::declared_apply::{
    DecidedDeclaration, DeclarationStep, DeclarationVerdict, IdentityDeclaration, SealAnswer,
};
use crate::runtime::identity_change::{
    DeclaredChangeAnswer, DeclaredChangeKind, DeclaredChangeWrite, IdentityChangeRefusal,
    IdentityGrantRequest, declared_entry_receipt_outcome, grant_covers_the_commit, now_ms, refused,
    store_owned_update_commit, store_owned_update_row,
};

impl MobActor {
    /// Decide one chunk of declarations in one MobMachine step: each
    /// identity's verdict (`DeclareIdentityEntry`), all applied on one
    /// prepared authority and committed once. A declaration no arm takes,
    /// or whose role cannot be resolved, is answered for that identity
    /// alone. A mob that is not running answers an invalid transition.
    ///
    /// A step that carries a change ([`DeclaredChangeWrite`]) decides the
    /// change's admission first, on the same prepared authority (the
    /// store's receipt of its operation and its requester's grant, read and
    /// checked at this final sink), and writes the change with its verdicts
    /// in one store transaction before the machine commits: a roster-owned
    /// identity's receipt, recording each declared identity the step
    /// answered; or a store-owned update's row at the next revision (refused
    /// `stale_spec` when the row's spec moved since the change was decided)
    /// and its receipt. An agent requester's update commits only what its
    /// grant's admission covered: the new spec and the identity's current
    /// profile (its row's). MobMachine routes a store-owned update from this
    /// step's state: an archived identity, or one awaiting its host's
    /// preparation, is answered record only (its row is the whole change).
    /// A replayed store-owned update decides and writes nothing again; a
    /// replayed roster change decides its declarations and writes nothing. A
    /// store-owned update whose identity's member step is in flight (a
    /// spawn, wake, retirement, removal, archive or held construction) is
    /// refused retryable, with nothing written.
    pub(super) async fn declare_identity_entries(
        &mut self,
        declarations: Vec<IdentityDeclaration>,
        change: Option<Box<DeclaredChangeWrite>>,
    ) -> Result<DeclarationStep, MobError> {
        // A spec that names no runtime mode declares its role's: each role
        // is resolved once per chunk.
        let mut role_modes: BTreeMap<ProfileName, crate::MobRuntimeMode> = BTreeMap::new();
        let mut resolved = Vec::with_capacity(declarations.len());
        for declaration in declarations {
            let runtime_mode = match declaration.runtime_mode {
                Some(runtime_mode) => Ok(runtime_mode),
                None => match role_modes.get(&declaration.profile_name) {
                    Some(runtime_mode) => Ok(*runtime_mode),
                    None => self
                        .definition
                        .resolve_profile(
                            &declaration.profile_name,
                            self.realm_profile_store.as_ref(),
                        )
                        .await
                        .map(|profile| {
                            role_modes
                                .insert(declaration.profile_name.clone(), profile.runtime_mode);
                            profile.runtime_mode
                        }),
                },
            };
            resolved.push((declaration, runtime_mode));
        }

        let mut authority = self.dsl_authority.prepare_authority();
        // A store-owned update's row, read at this final sink: the migration
        // it declares is from the role its member holds now (the row's
        // profile for an identity no member holds), kept in the row until
        // the restamp commits; a row whose spec moved since the change was
        // decided makes the change stale, which MobMachine's admission
        // answers after a replay; and what the update asks of an agent's
        // grant is the new spec and the row's profile.
        let (update_row, stale, grant_request) = match change.as_deref() {
            Some(DeclaredChangeWrite {
                kind:
                    DeclaredChangeKind::StoreOwnedUpdate {
                        spec,
                        expected_spec_digest,
                    },
                ..
            }) => {
                let store = self
                    .identity_member
                    .as_ref()
                    .ok_or_else(|| refused(IdentityChangeRefusal::StorageUnsupported))?;
                let current = store
                    .observe_created_identity(&self.definition.id, &spec.identity)
                    .await?;
                let dsl_identity = mob_dsl::AgentIdentity::from_domain(&spec.identity);
                let state = authority.state();
                let seated_role = state
                    .identity_to_runtime
                    .contains_key(&dsl_identity)
                    .then(|| state.member_profile_names.get(&dsl_identity))
                    .flatten()
                    .map(|profile| ProfileName::from(profile.as_str()));
                let row_profile = match &current {
                    Some(crate::identity::CreatedIdentityObservation::Created(row)) => {
                        row.spec.as_ref().map(|spec| spec.profile.clone())
                    }
                    _ => None,
                };
                let roster_owned = state.identity_entry_origins.get(&dsl_identity)
                    == Some(&mob_dsl::IdentityEntryOrigin::Declared);
                let grant_request = row_profile
                    .as_ref()
                    .map(|current| IdentityGrantRequest::for_update(current, spec));
                let from = seated_role.or(row_profile);
                let pending = from
                    .clone()
                    .filter(|from| *from != spec.profile)
                    .map(crate::identity::PendingRoleMigration::new);
                // An identity whose row is gone is answered after the
                // admission: a retry of an update that committed replays.
                match store_owned_update_row(
                    spec,
                    expected_spec_digest,
                    pending,
                    current,
                    roster_owned,
                ) {
                    Ok((row, stale)) => (
                        Some(Ok((spec.identity.clone(), from, row))),
                        stale,
                        grant_request,
                    ),
                    Err(error) => (Some(Err(error)), None, None),
                }
            }
            _ => (None, None, None),
        };
        let mut replayed = None;
        if let Some(change) = change.as_deref() {
            match self
                .decide_change_admission(
                    &mut authority,
                    &change.operation,
                    change.facts,
                    change.grant.as_ref(),
                    change.decided,
                    stale,
                )
                .await?
            {
                // A committed update replays: nothing is decided or written
                // again.
                ChangeAdmission::Replayed(receipt) if change.is_store_owned_update() => {
                    return Ok(DeclarationStep {
                        verdicts: Vec::new(),
                        change: Some(DeclaredChangeAnswer::Replayed(receipt)),
                    });
                }
                ChangeAdmission::Replayed(receipt) => replayed = Some(receipt),
                ChangeAdmission::Admitted => {}
            }
        }
        let update_row = update_row.transpose()?;
        if let Some(change) = change.as_deref()
            && change.is_store_owned_update()
        {
            grant_covers_the_commit(change.grant.as_ref(), grant_request.as_ref())?;
        }
        let mut effects = Vec::new();
        let mut verdicts = Vec::with_capacity(resolved.len());
        for (mut declaration, runtime_mode) in resolved {
            // A store-owned update declares the migration from the role its
            // member holds, read on this authority.
            if let Some((identity, from, _)) = &update_row
                && *identity == declaration.agent_identity
            {
                declaration.declared_migration_from = from.clone();
            }
            let agent_identity = declaration.agent_identity.clone();
            let runtime_mode = match runtime_mode {
                Ok(runtime_mode) => runtime_mode,
                Err(error) => {
                    verdicts.push(DeclarationVerdict {
                        agent_identity,
                        verdict: Err(error),
                    });
                    continue;
                }
            };
            let dsl_identity = mob_dsl::AgentIdentity::from_domain(&agent_identity);
            // A rejected input leaves the prepared authority as it was.
            let transition = match mob_dsl::MobMachineMutator::apply(
                &mut authority,
                declaration.machine_input(runtime_mode),
            ) {
                Ok(transition) => transition,
                Err(error) => {
                    verdicts.push(DeclarationVerdict {
                        verdict: Err(MobError::MobMachineRejected {
                            context: "declared_apply",
                            reason: format!("'{agent_identity}': {error}"),
                        }),
                        agent_identity,
                    });
                    continue;
                }
            };
            let decided = transition.effects().iter().find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::IdentityEntryDeclared {
                    agent_identity,
                    verdict,
                    archived_session,
                    would_be,
                } if agent_identity == &dsl_identity => {
                    Some((*verdict, archived_session.clone(), *would_be))
                }
                _ => None,
            });
            let Some((verdict, archived_session, would_be)) = decided else {
                return Err(MobError::Internal(format!(
                    "MobMachine answered no declared apply verdict for '{agent_identity}'"
                )));
            };
            if verdict == mob_dsl::IdentityEntryVerdict::MobNotRunning {
                return Err(self.invalid_transition_to(MobState::Running));
            }
            let archived_session = archived_session
                .map(|session| SessionId::parse(&session.0))
                .transpose()
                .map_err(|error| {
                    MobError::Internal(format!(
                        "'{agent_identity}' holds an invalid archived session record: {error}"
                    ))
                })?;
            effects.extend(transition.into_effects());
            verdicts.push(DeclarationVerdict {
                agent_identity,
                verdict: Ok(DecidedDeclaration {
                    verdict,
                    runtime_mode,
                    archived_session,
                    would_be,
                }),
            });
        }
        let prepared = PreparedDslInput {
            authority,
            effects,
            phase_changed: false,
        };
        let Some(change) = change else {
            self.commit_prepared_dsl_input(prepared)?;
            return Ok(DeclarationStep {
                verdicts,
                change: None,
            });
        };
        // A store-owned update's identity whose member step is in flight is
        // left to the update's retry; a declaration no arm took refuses it.
        // Either way nothing is written.
        if change.is_store_owned_update()
            && let Some(position) = verdicts.iter().position(|verdict| {
                !matches!(
                    &verdict.verdict,
                    Ok(decided) if declared_entry_receipt_outcome(decided.verdict).is_some()
                )
            })
        {
            let verdict = verdicts.swap_remove(position);
            return Err(match verdict.verdict {
                Err(error) => error,
                Ok(_) => MobError::LifecycleOperationPending {
                    intent: format!(
                        "update '{}', whose member's spawn, wake, retirement, removal, archive \
                         or held construction is in flight",
                        verdict.agent_identity
                    ),
                },
            });
        }
        if let Some(receipt) = replayed {
            self.commit_prepared_dsl_input(prepared)?;
            return Ok(DeclarationStep {
                verdicts,
                change: Some(DeclaredChangeAnswer::Replayed(receipt)),
            });
        }
        // The change's receipt: each identity the step answered, with its
        // outcome (a store-owned update's with its spec digest and revision
        // after the change).
        let at_ms = now_ms();
        let mut entries = Vec::new();
        let (kind, row) = match update_row {
            Some((identity, _, row)) => {
                let outcome = verdicts
                    .iter()
                    .find_map(|verdict| match &verdict.verdict {
                        Ok(decided) => declared_entry_receipt_outcome(decided.verdict),
                        Err(_) => None,
                    })
                    // Every update declares its identity, and the step
                    // refused one no arm took: an archived or pending
                    // identity's record-only verdict commits its row.
                    .unwrap_or(crate::identity::IdentityChangeEntryOutcome::Committed);
                let digest = row
                    .record
                    .spec_digest()
                    .map_err(|error| MobError::Internal(format!("an update's spec: {error}")))?
                    .ok_or_else(|| MobError::Internal("an update's row holds no spec".into()))?;
                entries.push(
                    crate::identity::IdentityChangeReceiptEntry::new(identity, outcome)
                        .with_spec(digest, Some(row.record.revision)),
                );
                (crate::identity::IdentityChangeKind::Update, Some(row))
            }
            None => {
                for verdict in &verdicts {
                    if let Ok(decided) = &verdict.verdict
                        && let Some(outcome) = declared_entry_receipt_outcome(decided.verdict)
                    {
                        entries.push(crate::identity::IdentityChangeReceiptEntry::new(
                            verdict.agent_identity.clone(),
                            outcome,
                        ));
                    }
                }
                (crate::identity::IdentityChangeKind::RosterApply, None)
            }
        };
        // A roster change none of whose identities the step answered records
        // nothing: its retry decides it again.
        if entries.is_empty() {
            self.commit_prepared_dsl_input(prepared)?;
            return Ok(DeclarationStep {
                verdicts,
                change: None,
            });
        }
        let store = self
            .identity_member
            .clone()
            .ok_or_else(|| refused(IdentityChangeRefusal::StorageUnsupported))?;
        let receipt = crate::identity::IdentityChangeReceipt::sealed(
            self.definition.id.clone(),
            change.operation.operation_id.clone(),
            change.operation.request_digest.clone(),
            kind,
            entries,
            at_ms,
        )
        .and_then(|receipt| {
            receipt.with_attribution(change.requester.clone(), change.consent.clone())
        })
        .map_err(|error| MobError::Internal(format!("an identity change's receipt: {error}")))?;
        let written = receipt.clone();
        let update = row.is_some();
        // A store-owned update's descriptive inputs, which its member's seat
        // carries to the first build of a session no build created yet.
        let seat_inputs = match &change.kind {
            DeclaredChangeKind::StoreOwnedUpdate { spec, .. } => Some((
                spec.identity.clone(),
                spec.context.clone(),
                spec.additional_instructions.clone(),
            )),
            DeclaredChangeKind::Roster => None,
        };
        self.commit_prepared_dsl_input_after(prepared, move || async move {
            let commit = store
                .commit_identity_change(&crate::store::IdentityChangeWrite {
                    receipt: written,
                    rows: row.into_iter().collect(),
                    events: Vec::new(),
                })
                .await
                .map_err(MobError::from)?;
            if update {
                store_owned_update_commit(commit)
            } else {
                crate::runtime::identity_change::committed_change_events(commit).map(|_| ())
            }
        })
        .await?;
        if let Some((identity, context, instructions)) = seat_inputs
            && super::super::boot_plan::domain_member_seat_session_unborn(
                self.dsl_authority.state(),
                &identity,
            )
        {
            self.roster
                .write()
                .await
                .set_store_owned_seat_inputs(&identity, context, instructions);
        }
        Ok(DeclarationStep {
            verdicts,
            change: Some(DeclaredChangeAnswer::Committed(receipt)),
        })
    }

    /// Open a declared apply (`OpenDeclaredApply`): MobMachine's verdict,
    /// `Opened` or `Busy` (another apply of the mob is open). A mob that is
    /// not running answers an invalid transition.
    pub(super) fn open_declared_apply(
        &mut self,
    ) -> Result<mob_dsl::DeclaredApplyOpenVerdict, MobError> {
        let effects = self.apply_dsl_input_collect_effects(
            mob_dsl::MobMachineInput::OpenDeclaredApply,
            "open_declared_apply",
        )?;
        let verdict = effects
            .iter()
            .find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::DeclaredApplyOpenResolved { verdict } => Some(*verdict),
                _ => None,
            })
            .ok_or_else(|| {
                MobError::Internal("MobMachine answered no declared apply opening".to_string())
            })?;
        if verdict == mob_dsl::DeclaredApplyOpenVerdict::MobNotRunning {
            return Err(self.invalid_transition_to(MobState::Running));
        }
        Ok(verdict)
    }

    /// Close the open declared apply (`CloseDeclaredApply`), in every
    /// lifecycle phase.
    pub(super) fn close_declared_apply(&mut self) -> Result<(), MobError> {
        self.apply_dsl_input(
            mob_dsl::MobMachineInput::CloseDeclaredApply,
            "close_declared_apply",
        )
    }

    /// Unlist one roster-owned identity (`UnlistIdentityEntry`) in one
    /// MobMachine step, decided on that identity alone: a seated Declared
    /// entry is marked (the handle retires its member with the cause
    /// Unlisted), an unseated one ends at once, one with a member step in
    /// flight is deferred, and any other name is left as it is. No other
    /// entry is read or changed. A mob that is not running answers an
    /// invalid transition.
    pub(super) fn unlist_identity_entry(
        &mut self,
        identity: &AgentIdentity,
    ) -> Result<mob_dsl::UnlistEntryVerdict, MobError> {
        let effects = self.apply_dsl_input_collect_effects(
            mob_dsl::MobMachineInput::UnlistIdentityEntry {
                agent_identity: mob_dsl::AgentIdentity::from_domain(identity),
            },
            "unlist_identity_entry",
        )?;
        let verdict = effects
            .iter()
            .find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::IdentityEntryUnlisted { verdict, .. } => Some(*verdict),
                _ => None,
            })
            .ok_or_else(|| {
                MobError::Internal("MobMachine answered no unlist verdict".to_string())
            })?;
        if verdict == mob_dsl::UnlistEntryVerdict::MobNotRunning {
            return Err(self.invalid_transition_to(MobState::Running));
        }
        Ok(verdict)
    }

    /// Seal a complete declared roster (`SealDeclaredRoster`) in one
    /// MobMachine step, and answer the Declared entries it marked (whose
    /// members the handle retires) and the ones it removed at once. A mob
    /// that is not running answers an invalid transition.
    pub(super) fn seal_declared_roster(
        &mut self,
        desired: BTreeSet<AgentIdentity>,
    ) -> Result<SealAnswer, MobError> {
        let before = self.dsl_authority.state().identity_entry_origins.clone();
        let prepared = self.prepare_dsl_input_transition(
            mob_dsl::MobMachineInput::SealDeclaredRoster {
                desired: desired
                    .iter()
                    .map(mob_dsl::AgentIdentity::from_domain)
                    .collect(),
            },
            "seal_declared_roster",
        )?;
        let verdict = prepared
            .transition
            .effects()
            .iter()
            .find_map(|effect| match effect {
                mob_dsl::MobMachineEffect::DeclaredRosterSealed { verdict } => Some(*verdict),
                _ => None,
            })
            .ok_or_else(|| MobError::Internal("MobMachine answered no seal verdict".to_string()))?;
        if verdict == mob_dsl::DeclaredRosterSealVerdict::MobNotRunning {
            return Err(self.invalid_transition_to(MobState::Running));
        }
        let state = prepared.authority.state();
        let identity =
            |dsl_identity: &mob_dsl::AgentIdentity| AgentIdentity::from(dsl_identity.0.as_str());
        // The marked members whose retirement has not started: the handle
        // retires each, and its start row records the cause.
        let marked = state
            .identity_entry_retiring
            .iter()
            .filter(|dsl_identity| {
                state
                    .identity_to_runtime
                    .get(*dsl_identity)
                    .is_some_and(|runtime| {
                        state.member_state_markers.get(runtime)
                            != Some(&mob_dsl::MobMemberState::Retiring)
                    })
            })
            .map(identity)
            .collect();
        let removed = before
            .keys()
            .filter(|dsl_identity| !state.identity_entry_origins.contains_key(*dsl_identity))
            .map(identity)
            .collect();
        let phase_changed = prepared.transition.from_phase != prepared.transition.to_phase;
        self.commit_prepared_dsl_input(PreparedDslInput {
            authority: prepared.authority,
            effects: prepared.transition.into_effects(),
            phase_changed,
        })?;
        Ok(SealAnswer {
            verdict,
            marked,
            removed,
        })
    }
}
