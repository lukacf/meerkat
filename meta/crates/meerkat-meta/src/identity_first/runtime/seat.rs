//! Seating roster identities in Core's mob.
//!
//! Every identity a roster lists is a member of Core's mob, under every
//! bootstrap mode. An identity Core does not hold is SEATED, never woken:
//! Core admits it through its ordinary spawn admission and commits it
//! Dormant, bound to a session, with nothing built. Core materializes a
//! seated member on its first delivery, from the host or from a peer, and at
//! boot only when its own obligation facts say the member owes work. Meta
//! never decides which member is materialized.
//!
//! What a seat binds:
//! - an identity Core never held: a fresh empty session Core mints for it;
//! - an identity Core retired (a directory an earlier release wrote): its
//!   last session, at the next generation;
//! - a pending first-boot `Resume` row: the row's session;
//! - a pending first-boot `Fresh` row: a fresh session, or Core's rotation
//!   onto one when Core holds the identity;
//! - an identity Core holds under a role the roster no longer names, on a
//!   session that lives in memory only: Core's rotation onto a fresh session
//!   under the roster profile, since that session has nothing to carry over,
//!   once the member is idle (a busy member keeps its role and its work
//!   until a later step finds it idle);
//! - an identity whose declared role migration finds its session never
//!   created (Core holds it unborn: no wake has run): the same rotation,
//!   since there is no session to restamp. A wake that creates the session
//!   before the move runs leaves the move to the identity's next step, which
//!   restamps the member onto that session.
//!
//! The draft a seat records is the host's `customize_build` of the spec, run
//! with the runtime's real context: Core builds the member from it at its
//! first materialization, as a create of the identity would build it. That
//! run also publishes the identity's customizer tools, and the customizer is
//! told the bound session (`after_create`).
//!
//! A seat holds no lifecycle lock while the customizer runs, so a customizer
//! that calls into another identity never waits on this one. The Core step
//! runs under the lifecycle locks of the identities it seats, taken in one
//! stable order, so no lifecycle verb interleaves with it; it calls back into
//! nothing.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use futures::stream::{self, StreamExt};
use meerkat_core::types::SessionId;

use super::super::bridge::{DormantSeatRequest, DormantSeatResult};
use super::super::contracts::AgentCustomizer;
use super::super::first_boot_manifest::{FirstBootRow, FirstBootRowKind, FirstBootRowState};
use super::super::types::{
    AgentBuildContext, AgentBuildDraft, AgentIdentity, DurableAgentSpec, FirstBootWait,
    IdentityLifecycleState,
};
use super::{
    AfterCreateNotice, CANDIDATE_CREATE_MEMBER, CANDIDATE_FIRST_BOOT_FRESH,
    CANDIDATE_FRESH_SESSION, CANDIDATE_MOVE_ROLE, CANDIDATE_RESUME_MEMBER, FirstBootRows,
    IdentityRuntime, IdentityRuntimeError, WakeHold, binding_spawned_for_identity,
    classify_wake_failure, deterministic_build_rejection_reason,
    durable_spec_uses_external_binding, refuse_runtime_plane_creation, release_error,
};

/// How one listed identity becomes, or stays, a member of Core's mob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum SeatPlan {
    /// Core holds the identity and nothing is owed: no Core call.
    Seated,
    /// Seat the identity Dormant on `session`, its committed session, or on
    /// a fresh session Core mints when `session` is `None`.
    Seat {
        session: Option<SessionId>,
        /// The identity's previous role, when the roster moved it to another
        /// one and this activation declared exactly that migration.
        resume_from_role: Option<meerkat_mob::ProfileName>,
        /// The identity's first-boot row is done and Core already holds the
        /// identity on `session`: the seat only ends the identity's host
        /// preparation in Core (Core answers `AlreadySeated`). Nothing is
        /// customized and nothing is written.
        settles_row_only: bool,
    },
    /// A first-boot `Fresh` row on an identity Core holds (`from_row`), or
    /// a recorded rotation a crash interrupted: Core's rotation onto a fresh
    /// session.
    Rotate { from_row: bool },
    /// Core holds the identity under a role the roster no longer names, and
    /// this activation declared that migration: its member is restamped onto
    /// the same session, then its materialization is released.
    Migrate {
        session: SessionId,
        from_role: meerkat_mob::ProfileName,
    },
    /// Core holds the identity under a role the roster no longer names, on a
    /// session that lives in memory only (Core's session service keeps no
    /// session beyond its member): there is no session to carry over to the
    /// new role, and Core's release keeps such a member materialized. So,
    /// declared or not, the move is Core's rotation onto a fresh session
    /// under the roster profile, which retires the member under its previous
    /// role, once the member is idle on `session`. A declared migration of a
    /// member whose session no wake has created yet (Core holds it unborn: a
    /// seat built nothing) is this rotation too: there is no session to
    /// restamp, and a resume of it would find none.
    RotateRole {
        session: SessionId,
        /// The declared migration's predecessor role, when the move is a
        /// rotation because Core held `session` unborn at plan time; `None`
        /// for a session that lives in memory only. The plan takes no lock,
        /// so the carry-out reads Core's unborn fact again under the
        /// identity's lifecycle lock: a session a wake created meanwhile is
        /// never rotated away, and the declared migration restamps the
        /// member onto it at the identity's next step.
        unborn_from: Option<meerkat_mob::ProfileName>,
    },
    /// A placed or externally bound identity Core does not hold: Core's
    /// ordinary create. Its host owns its materialization, so it has no
    /// Dormant phase to be seated in.
    CreateHosted,
}

impl SeatPlan {
    /// Whether the plan builds a draft from the host's customizer.
    fn customizes(&self) -> bool {
        match self {
            Self::Seated => false,
            Self::Seat {
                settles_row_only, ..
            } => !settles_row_only,
            Self::Rotate { .. }
            | Self::RotateRole { .. }
            | Self::Migrate { .. }
            | Self::CreateHosted => true,
        }
    }
}

/// What a seat step left of one identity.
pub(crate) struct SeatOutcome {
    /// Core held the identity before the step: nothing was seated, rotated
    /// or created for it.
    pub(crate) already_seated: bool,
    /// The session Core binds the identity to after the step, when known.
    pub(crate) session_id: Option<SessionId>,
    /// The step bound the identity to a fresh session: a fresh seat, a
    /// rotation, or a create.
    pub(crate) minted: bool,
    /// The draft the step recorded: the host's build of the spec, or the
    /// spec's base draft when nothing was built.
    pub(crate) draft: AgentBuildDraft,
    /// The `after_create` owed to the customizer the caller passed, for a
    /// caller that holds the identity's lifecycle lock: it tells it once it
    /// has released the lock, so the customizer may call back into the
    /// identity.
    pub(super) override_after_create: Option<AfterCreateNotice>,
    /// Core ended the identity's host preparation in this step: a seat
    /// Core answered (seated, already seated, embodied), or a rotation's
    /// terminal step. Every other step leaves a mark Meta asked for at
    /// start in place.
    host_preparation_ended: bool,
}

/// Whether a seat batch takes the lifecycle locks of its identities.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SeatLocks {
    /// The batch takes each identity's lifecycle lock for its Core step.
    Take,
    /// The caller already holds the lifecycle lock of the batch's one
    /// identity.
    Held,
}

/// One identity's planned step, with what it carries out.
struct PlannedSeat {
    spec: DurableAgentSpec,
    plan: SeatPlan,
    /// The identity's first-boot row this step carries out, if any.
    row: Option<FirstBootRow>,
    /// Whether this step settles the identity's recorded rotation.
    rotation: bool,
}

impl IdentityRuntime {
    /// Make every identity of `specs` a member of Core's mob, as one step:
    /// seat the ones Core does not hold, rotate a pending first-boot `Fresh`
    /// row's identity Core holds, apply a declared role migration (or, on a
    /// session that lives in memory only or one no wake has created yet,
    /// rotate the identity onto a fresh session under its roster profile),
    /// and carry out each pending first-boot row of these identities with
    /// it. One read of Core's member history plans every identity; the seats
    /// go to Core in one call, which journals them in batches.
    ///
    /// Returns each identity's outcome. A refusal is that identity's alone,
    /// typed, and changes nothing for it; the others go on. A candidate
    /// launch changes no stored member: each identity it would seat, rotate
    /// or move is refused with the candidate's deferral, and a first-boot
    /// row Core already shows done settles in memory only.
    ///
    /// The first-boot manifest is rewritten once, after every carried-out
    /// row of the step.
    pub(crate) async fn seat_identities(
        &self,
        specs: &[DurableAgentSpec],
        customizer: Option<&dyn AgentCustomizer>,
    ) -> BTreeMap<AgentIdentity, Result<SeatOutcome, IdentityRuntimeError>> {
        self.seat_identities_with(specs, customizer, SeatLocks::Take)
            .await
    }

    /// [`Self::seat_identities`] for the one identity whose lifecycle lock
    /// the caller holds: a delivery or an explicit materialization of an
    /// identity Core does not hold yet, or one whose first-boot row is still
    /// pending. Returns the identity's outcome.
    pub(super) async fn seat_identity_locked(
        &self,
        spec: &DurableAgentSpec,
        customizer: Option<&dyn AgentCustomizer>,
    ) -> Result<SeatOutcome, IdentityRuntimeError> {
        let identity = spec.identity.clone();
        self.seat_identities_with(std::slice::from_ref(spec), customizer, SeatLocks::Held)
            .await
            .remove(&identity)
            .unwrap_or_else(|| {
                Err(IdentityRuntimeError::Internal(format!(
                    "the seat step answered nothing for {identity}"
                )))
            })
    }

    async fn seat_identities_with(
        &self,
        specs: &[DurableAgentSpec],
        customizer: Option<&dyn AgentCustomizer>,
        locks: SeatLocks,
    ) -> BTreeMap<AgentIdentity, Result<SeatOutcome, IdentityRuntimeError>> {
        let mut outcomes = BTreeMap::new();
        if specs.is_empty() {
            return outcomes;
        }
        let Some(bridge) = self.bridge.clone() else {
            // Without a session bridge there is no Core mob to seat in.
            for spec in specs {
                outcomes.insert(
                    spec.identity.clone(),
                    Ok(SeatOutcome {
                        already_seated: true,
                        session_id: None,
                        minted: false,
                        draft: Self::base_build_draft(spec),
                        override_after_create: None,
                        host_preparation_ended: false,
                    }),
                );
            }
            return outcomes;
        };
        // The manifest is read first: an unreadable one refuses every
        // identity of the step, since which of them it covers is unknown.
        if let Err(error) = self.load_first_boot_manifest().await {
            for spec in specs {
                outcomes.insert(spec.identity.clone(), Err(clone_refusal(&error)));
            }
            return outcomes;
        }
        let history = match bridge.member_binding_history().await {
            Ok(history) => history,
            Err(error) => {
                for spec in specs {
                    outcomes.insert(
                        spec.identity.clone(),
                        Err(IdentityRuntimeError::IdentityHistoryUnavailable {
                            identity: spec.identity.clone(),
                            detail: format!(
                                "reading Core's member history to seat the identity: {error}"
                            ),
                        }),
                    );
                }
                return outcomes;
            }
        };
        // A member a held pass could not publish owes its host a fresh held
        // completion: no seat outside a held pass that covers it creates it.
        let held_marks = self.load_held_completion_marks().await;
        let held_pass = self.held_create_pass();
        let rows = self.pending_first_boot_rows_snapshot();
        // Rotations a crash interrupted: each is driven to its end here. An
        // unreadable record store re-drives none; Core keeps their
        // identities' host preparation pending until a compile reads it.
        let rotations = match self.pending_rotation_intents().await {
            Ok(rotations) => rotations,
            Err(error) => {
                tracing::error!(
                    %error,
                    "the identities' rotation records could not be read; no interrupted \
                     rotation is driven to its end by this step"
                );
                BTreeMap::new()
            }
        };

        let mut planned = Vec::with_capacity(specs.len());
        for spec in specs {
            let identity = &spec.identity;
            let row = rows.get(identity).cloned();
            let row_done = row
                .as_ref()
                .is_some_and(|row| row.state_in(&history) == FirstBootRowState::Done);
            let rotation = rotations.get(identity);
            let plan = match rotation {
                Some(rotation) => Ok(Self::rotation_redrive_plan(&history, identity, rotation)),
                None => self
                    .role_move_in_memory_only(spec, &history)
                    .await
                    .and_then(|in_memory_only| {
                        self.seat_plan(spec, &history, row.as_ref(), in_memory_only)
                    }),
            }
            .and_then(|plan| {
                let creates = matches!(
                    plan,
                    SeatPlan::Seat { session: None, .. } | SeatPlan::CreateHosted
                );
                if creates {
                    if let Err(error) = &held_marks {
                        return Err(clone_refusal(error));
                    }
                    if self.held_completion_owed(identity)
                        && held_pass.as_ref().is_none_or(|pass| !pass.covers(identity))
                    {
                        return Err(IdentityRuntimeError::HeldCompletionOwed {
                            identity: identity.clone(),
                        });
                    }
                }
                Ok(plan)
            });
            match plan {
                // A candidate launch writes nothing to Core: a row Core shows
                // done settles in memory only, and the manifest is left as it
                // was found for the authoritative launch.
                Ok(plan) => match self.candidate_refusal(&bridge, identity, &plan).await {
                    Some(refusal) => {
                        if row_done {
                            self.settle_first_boot_row_in_memory(identity);
                        }
                        outcomes.insert(identity.clone(), Err(refusal));
                    }
                    None if self.is_candidate_launch() => {
                        if row_done {
                            self.settle_first_boot_row_in_memory(identity);
                        }
                        outcomes.insert(
                            identity.clone(),
                            Ok(SeatOutcome {
                                already_seated: true,
                                session_id: None,
                                minted: false,
                                draft: Self::base_build_draft(spec),
                                override_after_create: None,
                                host_preparation_ended: false,
                            }),
                        );
                    }
                    None => planned.push(PlannedSeat {
                        spec: spec.clone(),
                        plan,
                        row,
                        rotation: rotation.is_some(),
                    }),
                },
                Err(error) => {
                    // A planned hold (a role migration this activation did
                    // not declare) is the identity's recorded wake refusal:
                    // its deliveries take the seat step, which holds them
                    // typed, until the migration is declared.
                    self.record_wake_refusal(identity, &error).await;
                    outcomes.insert(identity.clone(), Err(error));
                }
            }
        }

        // Every draft is built before any Core step, with no lifecycle lock
        // held: the host's customizer may call into other identities.
        let drafts = self.seat_drafts(&planned, customizer).await;
        let mut ready = Vec::with_capacity(planned.len());
        for (planned, draft) in planned.into_iter().zip(drafts) {
            match draft {
                Ok(draft) => ready.push((planned, draft)),
                Err(error) => {
                    outcomes.insert(planned.spec.identity.clone(), Err(error));
                }
            }
        }

        let committed = self.commit_seats(&bridge, ready, locks).await;
        let installed = self.customizer.read().await.clone();
        let mut told = Vec::new();
        let mut seeded_settled = Vec::new();
        for (planned, draft, result) in committed {
            let identity = planned.spec.identity.clone();
            match result {
                Ok(mut outcome) => {
                    if self.host_pending_seeded_contains(&identity) {
                        if outcome.host_preparation_ended {
                            self.forget_seeded_host_preparation(&identity);
                        } else {
                            seeded_settled.push(planned.spec.clone());
                        }
                    }
                    if planned.row.is_some() {
                        self.settle_first_boot_row_in_memory(&identity);
                    }
                    if planned.rotation {
                        self.clear_rotation_intent(&identity).await;
                    }
                    if !outcome.already_seated {
                        // A caller that holds the identity's lifecycle lock
                        // settles the views itself, once, with its own step.
                        if locks == SeatLocks::Take {
                            self.settle_successful_wake(&identity).await;
                        }
                        // The customizer that built the draft is told the
                        // session the identity is bound to, with no
                        // lifecycle lock held: the installed one in a
                        // runtime-owned task, the caller's own by the caller
                        // once it holds none.
                        if planned.plan.customizes()
                            && let Some(session_id) = outcome.session_id.clone()
                        {
                            let notice = AfterCreateNotice {
                                session_id,
                                context: self.session_created_context(&planned.spec, &draft),
                            };
                            match (customizer, installed.as_ref(), locks) {
                                (Some(_), _, SeatLocks::Held) => {
                                    outcome.override_after_create = Some(notice);
                                }
                                (Some(_), _, SeatLocks::Take) => {
                                    told.push((identity.clone(), notice));
                                }
                                (None, Some(installed), _) => {
                                    self.spawn_after_create(
                                        Arc::clone(installed),
                                        identity.clone(),
                                        notice,
                                    )
                                    .await;
                                }
                                (None, None, _) => {}
                            }
                        }
                    }
                    outcomes.insert(identity, Ok(outcome));
                }
                Err(error) => {
                    self.record_wake_refusal(&identity, &error).await;
                    outcomes.insert(identity, Err(error));
                }
            }
        }
        #[cfg(test)]
        {
            let hook = self
                .first_boot_batch_hook
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            if let Some(hook) = hook {
                hook().await;
            }
        }
        // Every seeded identity whose step succeeded without ending Core's
        // host preparation mark has it ended now.
        self.end_seeded_host_preparation(&bridge, seeded_settled)
            .await;
        // The step's one manifest rewrite, which also retries a rewrite that
        // failed before.
        self.rewrite_first_boot_manifest().await;
        self.publish_first_boot_pending().await;
        if let Some(customizer) = customizer {
            for (identity, notice) in told {
                Self::tell_after_create(customizer, &identity, &notice).await;
            }
        }
        outcomes
    }

    /// Record the member ids Meta handed the mob as host-pending before it
    /// was built (`host_pending_members_before_build`): each keeps its mark
    /// in Core until a seat step for it ends the mark.
    pub(crate) fn seed_host_pending(
        &self,
        members: impl IntoIterator<Item = meerkat_mob::AgentIdentity>,
    ) {
        self.host_pending_seeded
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend(members);
    }

    /// Whether Core still keeps the host preparation Meta asked for at
    /// start for `identity` ([`Self::seed_host_pending`]).
    pub(super) fn host_pending_seeded_contains(&self, identity: &AgentIdentity) -> bool {
        self.host_pending_seeded
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&crate::member_comms_id::roster_member_id_for_identity(
                identity.as_str(),
            ))
    }

    /// Core ended `identity`'s host preparation.
    fn forget_seeded_host_preparation(&self, identity: &AgentIdentity) {
        self.host_pending_seeded
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&crate::member_comms_id::roster_member_id_for_identity(
                identity.as_str(),
            ));
    }

    /// End Core's host-pending mark of every identity in `settled`: each
    /// one Meta seeded before the mob build whose seat step succeeded
    /// without ending it. Core ends the mark in its seat (a seat,
    /// `AlreadySeated`, `Embodied`) and in its rotation's terminal step; the
    /// steps that take neither (an archived first-boot session revived and
    /// released, a declared role migration, an identity Core already held
    /// with nothing owed) would leave it, and every wake of the identity
    /// would be refused for the rest of the process. So each one gets a
    /// settles-only seat on the session Core holds it on, which Core
    /// answers `AlreadySeated`, writing nothing and ending the mark. A
    /// candidate launch writes nothing to Core and ends nothing.
    async fn end_seeded_host_preparation(
        &self,
        bridge: &Arc<dyn super::SessionBridge>,
        settled: Vec<DurableAgentSpec>,
    ) {
        if settled.is_empty() || self.is_candidate_launch() {
            return;
        }
        let history = match bridge.member_binding_history().await {
            Ok(history) => history,
            Err(error) => {
                tracing::warn!(
                    %error,
                    "Core's member history could not be read to end the host preparation of \
                     the seeded identities; their next seat step ends it"
                );
                return;
            }
        };
        let mut requests = Vec::with_capacity(settled.len());
        for spec in settled {
            let member =
                crate::member_comms_id::roster_member_id_for_identity(spec.identity.as_str());
            let Some(session) = history
                .last_binding(&member)
                .filter(|binding| binding.is_live())
                .and_then(|binding| binding.session_id().cloned())
            else {
                continue;
            };
            requests.push(DormantSeatRequest {
                identity: spec.identity.clone(),
                draft: Self::base_build_draft(&spec),
                spec,
                session: Some(session),
                resume_from_role: None,
            });
        }
        if requests.is_empty() {
            return;
        }
        match bridge.seat_dormant_members(requests).await {
            Ok(results) => {
                for (identity, result) in results {
                    match result {
                        DormantSeatResult::AlreadySeated { .. }
                        | DormantSeatResult::Seated { .. }
                        | DormantSeatResult::Embodied { .. } => {
                            self.forget_seeded_host_preparation(&identity);
                        }
                        other => tracing::warn!(
                            %identity,
                            answer = ?other,
                            "Core did not end the seeded identity's host preparation; its next \
                             seat step tries again"
                        ),
                    }
                }
            }
            Err(error) => tracing::warn!(
                %error,
                "the settles-only seats of the seeded identities failed; their next seat \
                 step tries again"
            ),
        }
    }

    /// The rows of the first-boot manifest still to carry out, by identity.
    fn pending_first_boot_rows_snapshot(&self) -> BTreeMap<AgentIdentity, FirstBootRow> {
        match &*self
            .first_boot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            FirstBootRows::Pending { rows, .. } => rows.clone(),
            _ => BTreeMap::new(),
        }
    }

    /// How `spec`'s identity becomes, or stays, a member of Core's mob,
    /// from one read of Core's member history and the identity's pending
    /// first-boot row. `in_memory_only` says Core holds the identity live
    /// under another role on a session that lives in memory only (the
    /// seat step's `role_move_in_memory_only`). A refusal is typed and
    /// changes nothing.
    pub(super) fn seat_plan(
        &self,
        spec: &DurableAgentSpec,
        history: &meerkat_mob::MemberBindingHistory,
        row: Option<&FirstBootRow>,
        in_memory_only: bool,
    ) -> Result<SeatPlan, IdentityRuntimeError> {
        let identity = &spec.identity;
        let member = crate::member_comms_id::roster_member_id_for_identity(identity.as_str());
        // A host-adopted identity is read-only to every Meta verb.
        Self::host_adoption_refusal(history.identity_management(&member), identity)?;
        let last = history.last_binding(&member);
        // A pending first-boot row is carried out with the identity's seat.
        let mut row_done = false;
        if let Some(row) = row {
            match row.state_in(history) {
                FirstBootRowState::Done => row_done = true,
                FirstBootRowState::Contradicted {
                    core_session,
                    detail,
                } => {
                    return Err(IdentityRuntimeError::FirstBootRowRefused {
                        identity: identity.clone(),
                        kind: row.kind(),
                        manifest_session: row.session().clone(),
                        core_session,
                        detail,
                    });
                }
                FirstBootRowState::ToDo => {
                    return Ok(match row {
                        FirstBootRow::Resume { session, .. } => SeatPlan::Seat {
                            session: Some(session.clone()),
                            resume_from_role: None,
                            settles_row_only: false,
                        },
                        FirstBootRow::Fresh { .. }
                            if last.is_some_and(meerkat_mob::MemberBinding::is_live) =>
                        {
                            SeatPlan::Rotate { from_row: true }
                        }
                        FirstBootRow::Fresh { .. } => {
                            refuse_runtime_plane_creation(identity)?;
                            SeatPlan::Seat {
                                session: None,
                                resume_from_role: None,
                                settles_row_only: false,
                            }
                        }
                    });
                }
            }
        }
        // A placed or externally bound identity has no Dormant phase: its
        // host owns its materialization.
        if spec.placement.is_some() || durable_spec_uses_external_binding(spec) {
            return Ok(match last {
                Some(binding) if binding.is_live() => SeatPlan::Seated,
                _ => {
                    refuse_runtime_plane_creation(identity)?;
                    SeatPlan::CreateHosted
                }
            });
        }
        match last {
            Some(binding) if binding.is_live() => {
                if *binding.role() != spec.profile {
                    let Some(session) = binding.session_id().cloned() else {
                        return Err(IdentityRuntimeError::IdentityHistoryUnavailable {
                            identity: identity.clone(),
                            detail: "Core holds the identity's member under another role with \
                                     no session binding"
                                .to_string(),
                        });
                    };
                    // A session that lives in memory only has nothing to
                    // carry over: the identity moves to a fresh session under
                    // the roster profile, with or without a declaration.
                    if in_memory_only {
                        return Ok(SeatPlan::RotateRole {
                            session,
                            unborn_from: None,
                        });
                    }
                    let from_role =
                        self.declared_migration_from(identity, spec, binding.role(), &session)?;
                    // A declared migration of a session no wake has created
                    // (Core's unborn fact) has nothing to restamp: the move
                    // is the rotation onto the roster profile. A resume of
                    // that session would find none.
                    if history.session_unborn(&session) {
                        return Ok(SeatPlan::RotateRole {
                            session,
                            unborn_from: Some(from_role),
                        });
                    }
                    return Ok(SeatPlan::Migrate { session, from_role });
                }
                if row_done {
                    // The row is done: the identity's host preparation in
                    // Core ends in a seat step that writes nothing.
                    return Ok(SeatPlan::Seat {
                        session: binding.session_id().cloned(),
                        resume_from_role: None,
                        settles_row_only: true,
                    });
                }
                Ok(SeatPlan::Seated)
            }
            // Core retired the identity: it is seated again on its last
            // session, at the next generation. A last binding no
            // identity-first spawn made (a member the host made directly on
            // the mob under the identity's member id) is not the identity's
            // history: the identity starts fresh rather than take that
            // member's transcript.
            Some(binding) if binding_spawned_for_identity(binding, identity) => {
                let Some(session) = binding.session_id().cloned() else {
                    refuse_runtime_plane_creation(identity)?;
                    return Ok(SeatPlan::Seat {
                        session: None,
                        resume_from_role: None,
                        settles_row_only: false,
                    });
                };
                let resume_from_role = if *binding.role() == spec.profile {
                    None
                } else {
                    Some(self.declared_migration_from(identity, spec, binding.role(), &session)?)
                };
                Ok(SeatPlan::Seat {
                    session: Some(session),
                    resume_from_role,
                    settles_row_only: false,
                })
            }
            _ => {
                refuse_runtime_plane_creation(identity)?;
                Ok(SeatPlan::Seat {
                    session: None,
                    resume_from_role: None,
                    settles_row_only: false,
                })
            }
        }
    }

    /// The step that ends `identity`'s recorded rotation: settled when Core
    /// already holds the identity on a binding newer than the one the
    /// rotation replaced (the seat ends the identity's host preparation and
    /// writes nothing), otherwise Core's rotation again, which Core finishes
    /// from wherever the crash left it.
    fn rotation_redrive_plan(
        history: &meerkat_mob::MemberBindingHistory,
        identity: &AgentIdentity,
        rotation: &crate::runtime::IdentityRotationIntent,
    ) -> SeatPlan {
        let member = crate::member_comms_id::roster_member_id_for_identity(identity.as_str());
        match history.last_binding(&member) {
            Some(binding)
                if binding.is_live()
                    && rotation
                        .from_generation
                        .is_none_or(|from| binding.generation().get() > from) =>
            {
                SeatPlan::Seat {
                    session: binding.session_id().cloned(),
                    resume_from_role: None,
                    settles_row_only: true,
                }
            }
            _ => SeatPlan::Rotate { from_row: false },
        }
    }

    /// Whether Core holds `spec`'s identity live under a role the roster no
    /// longer names, on a session that lives in memory only
    /// (`session_lives_in_memory_only`). Asks Core nothing for an identity
    /// Core does not hold under another role.
    async fn role_move_in_memory_only(
        &self,
        spec: &DurableAgentSpec,
        history: &meerkat_mob::MemberBindingHistory,
    ) -> Result<bool, IdentityRuntimeError> {
        let member = crate::member_comms_id::roster_member_id_for_identity(spec.identity.as_str());
        let Some(binding) = history
            .last_binding(&member)
            .filter(|binding| binding.is_live() && *binding.role() != spec.profile)
        else {
            return Ok(false);
        };
        self.session_lives_in_memory_only(&spec.identity, binding.session_id())
            .await
    }

    /// The role `identity` migrates from when the roster profile moved off
    /// `last_role`, the role Core last ran `session` under: only a migration
    /// this activation's host declared for exactly that role. Meta never
    /// infers one; any other profile change is held typed.
    fn declared_migration_from(
        &self,
        identity: &AgentIdentity,
        spec: &DurableAgentSpec,
        last_role: &meerkat_mob::ProfileName,
        session: &SessionId,
    ) -> Result<meerkat_mob::ProfileName, IdentityRuntimeError> {
        let declared = self
            .bridge
            .as_ref()
            .and_then(|bridge| bridge.declared_role_migration(identity));
        if declared.as_ref() == Some(last_role) {
            return Ok(last_role.clone());
        }
        let detail = match declared {
            Some(from_role) => format!(
                "declared role migration of {identity} from '{from_role}' to '{}' was not \
                 applied: the role '{last_role}' Core last ran session {session} under is not \
                 the declared predecessor role",
                spec.profile
            ),
            None => format!(
                "the roster profile '{}' differs from the role '{last_role}' Core last ran \
                 session {session} under, and this activation declared no migration from it",
                spec.profile
            ),
        };
        Err(IdentityRuntimeError::WakeHeld {
            identity: identity.clone(),
            hold: WakeHold::RoleMigrationNotApplied,
            detail,
            session_id: session.clone(),
        })
    }

    /// A candidate launch's refusal of `plan`: it changes no stored member.
    /// Each refusal defers the identity to the authoritative launch
    /// (`candidate_deferral_of`) with the reason the change names. `None`
    /// when the plan changes nothing, or on a launch that speaks for its
    /// composition.
    async fn candidate_refusal(
        &self,
        bridge: &Arc<dyn super::SessionBridge>,
        identity: &AgentIdentity,
        plan: &SeatPlan,
    ) -> Option<IdentityRuntimeError> {
        if !self.is_candidate_launch() {
            return None;
        }
        let deferred = |operation: &'static str| IdentityRuntimeError::InvalidState {
            identity: identity.clone(),
            state: IdentityLifecycleState::Dormant,
            operation,
        };
        let (change, refusal) = match plan {
            SeatPlan::Seated
            | SeatPlan::Seat {
                settles_row_only: true,
                ..
            } => return None,
            SeatPlan::Seat { session: None, .. } | SeatPlan::CreateHosted => {
                ("create the member", deferred(CANDIDATE_CREATE_MEMBER))
            }
            SeatPlan::Seat {
                resume_from_role: Some(_),
                ..
            }
            | SeatPlan::Migrate { .. }
            | SeatPlan::RotateRole { .. } => (
                "move the member to another role",
                deferred(CANDIDATE_MOVE_ROLE),
            ),
            SeatPlan::Seat {
                session: Some(session),
                ..
            } => match bridge.durable_session_held(session).await {
                Ok(true) => (
                    "seat a member on its stored session",
                    deferred(CANDIDATE_RESUME_MEMBER),
                ),
                Ok(false) => (
                    "start a fresh session in place of the stored one",
                    deferred(CANDIDATE_FRESH_SESSION),
                ),
                Err(error) => {
                    return Some(IdentityRuntimeError::Internal(format!(
                        "candidate launch: whether Core holds the stored session {session} of \
                         {identity} could not be read: {error}"
                    )));
                }
            },
            SeatPlan::Rotate { from_row: true } => (
                "start the member's fresh life from its first-boot row",
                IdentityRuntimeError::FirstBootRowPending {
                    identity: identity.clone(),
                    kind: FirstBootRowKind::Fresh,
                    waiting_for: FirstBootWait::CarryOut,
                    operation: CANDIDATE_FIRST_BOOT_FRESH,
                },
            ),
            SeatPlan::Rotate { from_row: false } => (
                "finish the member's interrupted fresh life",
                deferred(CANDIDATE_FRESH_SESSION),
            ),
        };
        tracing::warn!(
            %identity,
            change,
            "candidate launch: seating the identity would change the stored roster, so it is \
             refused and the identity is deferred to the authoritative launch, which makes the \
             change"
        );
        Some(refusal)
    }

    /// The draft each planned step records, in plan order: the host's
    /// `customize_build` of the spec with the runtime's real context, whose
    /// customizer tools are published into the identity's stable dispatcher.
    /// Built with no lifecycle lock held, at most
    /// [`super::super::orchestrator::identity_restore_concurrency`] at once.
    async fn seat_drafts(
        &self,
        planned: &[PlannedSeat],
        customizer: Option<&dyn AgentCustomizer>,
    ) -> Vec<Result<AgentBuildDraft, IdentityRuntimeError>> {
        let installed = self.customizer.read().await.clone();
        let customizer = customizer.or(installed.as_deref());
        let active_peers: Vec<AgentIdentity> = self.entries.read().await.keys().cloned().collect();
        let managed_edges = self.desired_peer_edges.read().await.clone();
        // The stream's items are indices, not borrows of `planned`: a stream
        // of borrowed items makes every caller's future fail its `Send`
        // check ("not general enough").
        stream::iter(0..planned.len())
            .map(|index| {
                let planned = &planned[index];
                let active_peers = active_peers.clone();
                let managed_edges = managed_edges.clone();
                async move {
                    let mut draft = Self::base_build_draft(&planned.spec);
                    let Some(customizer) = customizer.filter(|_| planned.plan.customizes()) else {
                        return Ok(draft);
                    };
                    let identity = &planned.spec.identity;
                    let context = AgentBuildContext {
                        identity: identity.clone(),
                        active_peers,
                        managed_edges,
                        runtime_services: self.runtime_services(),
                    };
                    let mut shutdown = self.foreground_cancel.subscribe();
                    if *shutdown.borrow() {
                        return Err(IdentityRuntimeError::Internal(format!(
                            "the seat of {identity} was cancelled during shutdown before its \
                             build"
                        )));
                    }
                    let result = {
                        let customize =
                            customizer.customize_build(&context, &planned.spec, &mut draft);
                        tokio::pin!(customize);
                        tokio::select! {
                            result = &mut customize => result,
                            changed = shutdown.changed() => {
                                if changed.is_ok() && *shutdown.borrow() {
                                    return Err(IdentityRuntimeError::Internal(format!(
                                        "the seat of {identity} was cancelled during shutdown \
                                         before its build"
                                    )));
                                }
                                customize.await
                            }
                        }
                    };
                    match result {
                        Ok(()) => {
                            self.publish_customizer_tools(identity, &mut draft).await;
                            Ok(draft)
                        }
                        Err(super::super::types::CustomizerError::ToolArtifactRefused(refusal)) => {
                            Err(IdentityRuntimeError::ToolArtifactRefused(refusal))
                        }
                        Err(error) => Err(IdentityRuntimeError::Internal(format!(
                            "customizer: {error}"
                        ))),
                    }
                }
            })
            .buffered(super::super::orchestrator::identity_restore_concurrency())
            .collect::<Vec<_>>()
            .await
    }

    /// The Core step of each ready seat, under the lifecycle locks of the
    /// step's identities when `locks` says so: the seats in one Core call,
    /// then each rotation, migration and hosted create. Answers each ready
    /// seat with its draft and outcome.
    async fn commit_seats(
        &self,
        bridge: &Arc<dyn super::SessionBridge>,
        ready: Vec<(PlannedSeat, AgentBuildDraft)>,
        locks: SeatLocks,
    ) -> Vec<(
        PlannedSeat,
        AgentBuildDraft,
        Result<SeatOutcome, IdentityRuntimeError>,
    )> {
        // The lifecycle locks of every identity with a Core step, in one
        // stable order, held until the step answers.
        let mut guards = Vec::new();
        if locks == SeatLocks::Take {
            let ordered = ready
                .iter()
                .filter(|(planned, _)| planned.plan != SeatPlan::Seated)
                .map(|(planned, _)| planned.spec.identity.clone())
                .collect::<BTreeSet<_>>();
            for identity in &ordered {
                guards.push(self.lifecycle_lock_for(identity).await.lock_owned().await);
            }
        }
        let mut answered = Vec::with_capacity(ready.len());
        let mut seats = Vec::new();
        let mut others = Vec::new();
        for (planned, draft) in ready {
            match &planned.plan {
                SeatPlan::Seated => {
                    let outcome = SeatOutcome {
                        already_seated: true,
                        session_id: None,
                        minted: false,
                        draft: draft.clone(),
                        override_after_create: None,
                        host_preparation_ended: false,
                    };
                    answered.push((planned, draft, Ok(outcome)));
                }
                SeatPlan::Seat { .. } => seats.push((planned, draft)),
                SeatPlan::Rotate { .. }
                | SeatPlan::RotateRole { .. }
                | SeatPlan::Migrate { .. }
                | SeatPlan::CreateHosted => {
                    others.push((planned, draft));
                }
            }
        }
        if !seats.is_empty() {
            let requests = seats
                .iter()
                .map(|(planned, draft)| {
                    let SeatPlan::Seat {
                        session,
                        resume_from_role,
                        ..
                    } = &planned.plan
                    else {
                        unreachable!("only seats are requested")
                    };
                    DormantSeatRequest {
                        identity: planned.spec.identity.clone(),
                        spec: planned.spec.clone(),
                        draft: draft.clone(),
                        session: session.clone(),
                        resume_from_role: resume_from_role.clone(),
                    }
                })
                .collect::<Vec<_>>();
            match bridge.seat_dormant_members(requests).await {
                Ok(results) => {
                    let mut results = results.into_iter();
                    for (planned, draft) in seats {
                        let result = match results.next() {
                            Some((_, result)) => {
                                self.seat_result(bridge, &planned, &draft, result).await
                            }
                            None => Err(IdentityRuntimeError::Internal(format!(
                                "Core answered no seat outcome for {}",
                                planned.spec.identity
                            ))),
                        };
                        answered.push((planned, draft, result));
                    }
                }
                Err(error) => {
                    // The call as a whole failed: Core keeps the seats it
                    // committed, and the identities' next compile or
                    // delivery seats them again.
                    for (planned, draft) in seats {
                        let result = Err(IdentityRuntimeError::IdentityWakeRetryable {
                            identity: planned.spec.identity.clone(),
                            session_id: None,
                            detail: format!("seating the identity in Core's mob failed: {error}"),
                        });
                        answered.push((planned, draft, result));
                    }
                }
            }
        }
        for (planned, draft) in others {
            let result = self.commit_other(bridge, &planned, &draft).await;
            answered.push((planned, draft, result));
        }
        drop(guards);
        answered
    }

    /// One identity's seat outcome from Core's answer. An archived session
    /// is revived once through Core's revival path and released, so the
    /// identity is seated on it with its liveness unchanged.
    async fn seat_result(
        &self,
        bridge: &Arc<dyn super::SessionBridge>,
        planned: &PlannedSeat,
        draft: &AgentBuildDraft,
        result: DormantSeatResult,
    ) -> Result<SeatOutcome, IdentityRuntimeError> {
        let identity = &planned.spec.identity;
        let requested_fresh = matches!(planned.plan, SeatPlan::Seat { session: None, .. });
        match result {
            DormantSeatResult::Seated { session_id } => Ok(SeatOutcome {
                already_seated: false,
                session_id: Some(session_id),
                minted: requested_fresh,
                draft: draft.clone(),
                override_after_create: None,
                host_preparation_ended: true,
            }),
            // A fresh spec that owes a kickoff: Core spawned it live, and its
            // kickoff runs once.
            DormantSeatResult::Embodied { session_id } => Ok(SeatOutcome {
                already_seated: false,
                session_id: Some(session_id),
                minted: requested_fresh,
                draft: draft.clone(),
                override_after_create: None,
                host_preparation_ended: true,
            }),
            DormantSeatResult::AlreadySeated { session_id } => Ok(SeatOutcome {
                already_seated: true,
                session_id: Some(session_id),
                minted: false,
                draft: draft.clone(),
                override_after_create: None,
                host_preparation_ended: true,
            }),
            DormantSeatResult::SessionArchived { session_id } => {
                self.revive_archived_once(bridge, planned, draft, &session_id)
                    .await
            }
            DormantSeatResult::Refused(error) => {
                // A first-boot `Resume` row whose session Core holds no
                // durable copy of is the row's own typed refusal, as before
                // seats: the identity is not seated on another session.
                if let Some(row @ FirstBootRow::Resume { .. }) = planned.row.as_ref()
                    && matches!(
                        &error,
                        super::super::bridge::BridgeError::ResumeRejected {
                            kind: super::super::bridge::ResumeRejectionKind::Unavailable {
                                reason: meerkat_mob::error::SessionResumeUnavailableReason::Absent,
                            },
                            ..
                        }
                    )
                {
                    return Err(IdentityRuntimeError::FirstBootRowRefused {
                        identity: identity.clone(),
                        kind: row.kind(),
                        manifest_session: row.session().clone(),
                        core_session: None,
                        detail: format!("Core holds no durable copy of session {}", row.session()),
                    });
                }
                Err(match deterministic_build_rejection_reason(&error) {
                    Some(reason) => IdentityRuntimeError::HostRejectedBuild {
                        identity: identity.clone(),
                        reason,
                    },
                    None => classify_wake_failure(
                        identity,
                        match &planned.plan {
                            SeatPlan::Seat { session, .. } => session.as_ref(),
                            _ => None,
                        },
                        error,
                    ),
                })
            }
        }
    }

    /// A seat whose session Core keeps archived (a directory an earlier
    /// release wrote, whose bindings it retired): the identity is revived
    /// once on that session through Core's revival path, which runs no
    /// kickoff, and its materialization is released at once. The identity
    /// is then a Dormant member on its own session, as live as before.
    async fn revive_archived_once(
        &self,
        bridge: &Arc<dyn super::SessionBridge>,
        planned: &PlannedSeat,
        draft: &AgentBuildDraft,
        session_id: &SessionId,
    ) -> Result<SeatOutcome, IdentityRuntimeError> {
        let identity = &planned.spec.identity;
        let resume_from_role = match &planned.plan {
            SeatPlan::Seat {
                resume_from_role, ..
            } => resume_from_role.clone(),
            _ => None,
        };
        tracing::info!(
            %identity,
            %session_id,
            "the identity's session is archived: it is revived once on it and released, so it \
             stays a Dormant member on its own session"
        );
        let resumed = bridge
            .resume_session(
                identity,
                &planned.spec,
                draft,
                session_id,
                resume_from_role.as_ref(),
            )
            .await
            .map_err(|error| classify_wake_failure(identity, Some(session_id), error))?;
        self.release_after_carry_out(bridge, identity).await;
        Ok(SeatOutcome {
            already_seated: false,
            session_id: Some(resumed.session_id().clone()),
            minted: false,
            draft: draft.clone(),
            override_after_create: None,
            host_preparation_ended: false,
        })
    }

    /// One identity's rotation (a first-boot `Fresh` row, an interrupted
    /// rotation, or a role move on a session that lives in memory only or
    /// that no wake has created yet), role migration or hosted create.
    async fn commit_other(
        &self,
        bridge: &Arc<dyn super::SessionBridge>,
        planned: &PlannedSeat,
        draft: &AgentBuildDraft,
    ) -> Result<SeatOutcome, IdentityRuntimeError> {
        let identity = &planned.spec.identity;
        match &planned.plan {
            SeatPlan::Rotate { .. } | SeatPlan::RotateRole { .. } => {
                // A role move rotates only an idle member. The rotation
                // retires the member under its previous role, and a
                // retirement cancels the work its session holds: a running
                // turn and every queued input. Core's release is the idle
                // check: on a session that lives in memory only it closes
                // nothing (the member stays materialized), and Core refuses
                // it, retryably, while the session holds accepted work that
                // has not reached its terminal. A refused release leaves the
                // member, its role and its work as they are: that work
                // completes under the previous role, and a later roster pass,
                // or an explicit materialization, tries the move again.
                //
                // Inputs that arrive after the idle answer are not lost.
                // Meta's own sends wait on the identity's lifecycle lock,
                // which this step holds until the rotation answers. An input
                // Core routes to the member while its retirement runs is
                // refused typed (the member is retiring), and its sender may
                // send it again. One narrow gap remains: an input Core admits
                // outside Meta's identity runtime (a peer message, a send on
                // the mob handle, Core's own work such as a flow step) after
                // the idle answer and before the retirement starts ends,
                // typed, with the retired incarnation.
                //
                // The plan was made before the drafts were built, with no
                // lifecycle lock held, so a reset or a `delete_identity` may
                // have moved the identity in between. The move rotates only
                // the member it planned for: when Core no longer holds the
                // member on the planned session, nothing is done here, and
                // the identity's next seat step plans again.
                if let SeatPlan::RotateRole {
                    session,
                    unborn_from,
                } = &planned.plan
                {
                    let planned_member_live = self
                        .core_live_member(identity)
                        .await?
                        .and_then(|live| live.session_id)
                        .is_some_and(|live| live == *session);
                    if !planned_member_live {
                        return Ok(SeatOutcome {
                            already_seated: true,
                            session_id: None,
                            minted: false,
                            draft: draft.clone(),
                            override_after_create: None,
                            host_preparation_ended: false,
                        });
                    }
                    // A wake keeps the member's session, so the check above
                    // does not see one that ran in between: a peer message,
                    // a flow step or a send on the mob handle (outside Meta's
                    // lock) may have woken the member, created the session
                    // the plan read unborn, and run turns on it. A rotation
                    // would archive that session, so Core's unborn fact is
                    // read again here, under the lock. A session that exists
                    // now is left as it is, and the identity's next seat step
                    // plans the declared migration, which restamps the member
                    // onto it.
                    if let Some(from_role) = unborn_from {
                        let history = bridge.member_binding_history().await.map_err(|error| {
                            IdentityRuntimeError::IdentityHistoryUnavailable {
                                identity: identity.clone(),
                                detail: format!(
                                    "reading Core's member history before the declared role \
                                     migration's rotation: {error}"
                                ),
                            }
                        })?;
                        if !history.session_unborn(session) {
                            return Err(IdentityRuntimeError::IdentityWakeRetryable {
                                identity: identity.clone(),
                                session_id: Some(session.clone()),
                                detail: format!(
                                    "a wake created session {session} after the declared role \
                                     migration from '{from_role}' was planned as a rotation of \
                                     the never-created session; nothing was changed, and the \
                                     identity's next roster pass or explicit materialization \
                                     moves the member onto the session"
                                ),
                            });
                        }
                    }
                    bridge
                        .release_member(identity)
                        .await
                        .map_err(|error| release_error(identity, Some(session), error))?;
                }
                let session_id = bridge
                    .rotate_member(identity, &planned.spec, draft)
                    .await
                    .map_err(|error| match deterministic_build_rejection_reason(&error) {
                        // A fresh life refused for good (its member build off
                        // the launch's member auth binding, say) is answered
                        // typed and final, as a seat's is: asking again
                        // changes nothing.
                        Some(reason) => IdentityRuntimeError::HostRejectedBuild {
                            identity: identity.clone(),
                            reason,
                        },
                        None => IdentityRuntimeError::IdentityWakeRetryable {
                            identity: identity.clone(),
                            session_id: None,
                            detail: format!("Core's rotation of the identity failed: {error}"),
                        },
                    })?;
                Ok(SeatOutcome {
                    already_seated: false,
                    session_id: Some(session_id),
                    minted: true,
                    draft: draft.clone(),
                    override_after_create: None,
                    host_preparation_ended: true,
                })
            }
            SeatPlan::Migrate { session, from_role } => {
                // The move retires the member under its previous role, and
                // Core refuses a plain retirement while the host preparation
                // Meta asked for at start is pending: that mark is ended
                // first, with a settles-only seat on the member's session.
                if self.host_pending_seeded_contains(identity) {
                    self.end_seeded_host_preparation(bridge, vec![planned.spec.clone()])
                        .await;
                    if self.host_pending_seeded_contains(identity) {
                        return Err(IdentityRuntimeError::IdentityWakeRetryable {
                            identity: identity.clone(),
                            session_id: Some(session.clone()),
                            detail: "Core still keeps the host preparation of the identity, so \
                                     its declared role migration does not retire its member \
                                     now; its next compile or delivery tries again"
                                .to_string(),
                        });
                    }
                }
                let resumed = bridge
                    .resume_session(identity, &planned.spec, draft, session, Some(from_role))
                    .await
                    .map_err(|error| classify_wake_failure(identity, Some(session), error))?;
                self.release_after_carry_out(bridge, identity).await;
                Ok(SeatOutcome {
                    already_seated: false,
                    session_id: Some(resumed.session_id().clone()),
                    minted: false,
                    draft: draft.clone(),
                    override_after_create: None,
                    host_preparation_ended: false,
                })
            }
            SeatPlan::CreateHosted => {
                let binding = bridge
                    .create_session(identity, &planned.spec, draft)
                    .await
                    .map_err(|error| match deterministic_build_rejection_reason(&error) {
                        Some(reason) => IdentityRuntimeError::HostRejectedBuild {
                            identity: identity.clone(),
                            reason,
                        },
                        None => IdentityRuntimeError::Internal(format!(
                            "bridge create_session: {error}"
                        )),
                    })?;
                Ok(SeatOutcome {
                    already_seated: false,
                    session_id: binding.session_id,
                    minted: true,
                    draft: draft.clone(),
                    override_after_create: None,
                    host_preparation_ended: false,
                })
            }
            SeatPlan::Seated | SeatPlan::Seat { .. } => Err(IdentityRuntimeError::Internal(
                format!("the seat step routed a seat of {identity} as another step"),
            )),
        }
    }

    /// Release the materialization a one-time carry-out made (a revival of
    /// an archived session, a declared role migration): the identity's
    /// liveness after the carry-out equals its liveness before it. A failed
    /// release is logged: the member stays live until Core releases it.
    async fn release_after_carry_out(
        &self,
        bridge: &Arc<dyn super::SessionBridge>,
        identity: &AgentIdentity,
    ) {
        if let Err(error) = bridge.release_member(identity).await {
            tracing::warn!(
                %identity,
                %error,
                "the member a one-time carry-out materialized could not be released; it stays \
                 live until Core releases it"
            );
        }
    }

    /// The refusal Core's identity management of an identity answers: an
    /// identity a host adopted (Core holds its identity intent row) is
    /// read-only to every Meta verb, and a row this build cannot read, or a
    /// kind it does not know, refuses too.
    pub(super) fn host_adoption_refusal(
        management: meerkat_mob::MemberIdentityManagement,
        identity: &AgentIdentity,
    ) -> Result<(), IdentityRuntimeError> {
        match management {
            meerkat_mob::MemberIdentityManagement::CommandDriven => Ok(()),
            meerkat_mob::MemberIdentityManagement::IntentManaged => {
                Err(IdentityRuntimeError::IdentityHostAdopted {
                    identity: identity.clone(),
                })
            }
            // An intent row this build cannot read may declare anything:
            // never act on it until it reads.
            meerkat_mob::MemberIdentityManagement::IntentUnreadable => {
                Err(IdentityRuntimeError::IdentityHistoryUnavailable {
                    identity: identity.clone(),
                    detail: "Core holds an identity intent row for the identity that this \
                             build cannot read"
                        .to_string(),
                })
            }
            // A management kind this build does not know: never act on it.
            _ => Err(IdentityRuntimeError::IdentityHistoryUnavailable {
                identity: identity.clone(),
                detail: "Core reports an identity management kind this build does not know"
                    .to_string(),
            }),
        }
    }
}

/// The same refusal for each identity of a step that one shared failure
/// refuses (an unreadable first-boot manifest, unreadable held marks).
fn clone_refusal(error: &IdentityRuntimeError) -> IdentityRuntimeError {
    match error {
        IdentityRuntimeError::FirstBootManifestUnreadable { path, detail } => {
            IdentityRuntimeError::FirstBootManifestUnreadable {
                path: path.clone(),
                detail: detail.clone(),
            }
        }
        other => IdentityRuntimeError::Internal(other.to_string()),
    }
}

#[cfg(test)]
#[path = "seat_tests.rs"]
mod tests;
