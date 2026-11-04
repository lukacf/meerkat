//! Declaring roster identities in Core's mob.
//!
//! A registration declares its identity in Core's mob through Core's
//! declared apply ([`SessionBridge::apply_declared_roster`]): Core's
//! MobMachine decides the identity's verdict from the declaration and Core's
//! member facts (a Dormant seat, a kept member, a restamp, a held role move,
//! a refusal), and Core realizes it. Meta decides nothing about the member:
//! Core's answer is the identity's answer, typed.
//!
//! A declaration carries, from the sources Meta already keeps:
//! - the identity's roster spec. The first round declares its lifecycle
//!   facts only, and Core answers `BuildRequired` for an identity whose
//!   verdict builds a member (an owed seat, a fresh life, a rotation onto a
//!   fresh session); only such an identity is declared again, with the build
//!   the host's customizer composes. An identity Core keeps calls no
//!   customizer;
//! - whether the host holds the identity's creation for its held protocol: a
//!   running held pass covers it, or it owes its host a held completion. When
//!   the record of owed completions cannot be read, the identity is declared
//!   held, so nothing is created outside its held protocol;
//! - its pending first-boot row, as a first-boot request. A row Core's
//!   history shows carried out already is declared as a resume of the
//!   session Core holds the identity on, which ends Core's first-boot
//!   preparation of it and starts nothing; a row Core's history contradicts
//!   refuses the identity before Core is asked. The manifest drops exactly
//!   the rows Core names carried out;
//! - this activation's declared role migrations, which the session bridge
//!   adds.
//!
//! The lifecycle projection's cursor is prepared before Core is asked, as
//! for every seat-like step, so a seat the declaration makes tells its
//! customizer `after_create`.
//!
//! The declaration runs under the identity's lifecycle lock, and the host's
//! customizer runs with no lifecycle lock held: it may call into other
//! identities. Core serializes its declared applies: an apply that meets
//! another one in progress is declared again after a short wait, within
//! [`DECLARED_APPLY_PENDING_WAIT`].

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::super::bridge::{
    BridgeError, DeclaredIdentity, DeclaredIdentityAnswer, DeclaredRosterRequest,
};
use super::super::contracts::AgentCustomizer;
use super::super::first_boot_manifest::{FirstBootRow, FirstBootRowState};
use super::super::types::{
    AgentBuildContext, AgentBuildDraft, AgentIdentity, DurableAgentSpec, ManagedPeerEdge,
};
use super::{IdentityRuntime, IdentityRuntimeError, SessionBridge};

/// How long a registration keeps declaring its identity again while another
/// declared apply of the mob is in progress, before it answers
/// [`IdentityRuntimeError::DeclarationNotApplied`], retryable.
pub(super) const DECLARED_APPLY_PENDING_WAIT: Duration = Duration::from_secs(15);

/// The first wait before a registration declares its identity again after
/// meeting another apply in progress; each wait doubles, up to
/// [`DECLARED_APPLY_PENDING_MAX_BACKOFF`].
const DECLARED_APPLY_PENDING_FIRST_BACKOFF: Duration = Duration::from_millis(20);

/// The longest single wait between two declarations of one registration.
const DECLARED_APPLY_PENDING_MAX_BACKOFF: Duration = Duration::from_secs(1);

/// Which registration a declaration of one identity records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Registration {
    /// A registration, new or again ([`IdentityRuntime::register`]).
    Register,
    /// An update of a registered identity's row
    /// ([`IdentityRuntime::update_spec`]).
    Update,
}

/// What a declaration of some identities carries besides their specs, read
/// before Core is asked, with no lifecycle lock held.
pub(super) struct DeclarationInputs {
    /// The first-boot request each identity's pending row carries.
    first_boot: BTreeMap<AgentIdentity, meerkat_mob::FirstBootRequest>,
    /// The identities whose creation the host holds for its held protocol.
    held: BTreeSet<AgentIdentity>,
    /// The identities refused before Core is asked: a first-boot row Core's
    /// history contradicts.
    refused: BTreeMap<AgentIdentity, IdentityRuntimeError>,
    /// Whether a first-boot row was settled in memory while reading: a row
    /// Core's history shows carried out, whose identity Core no longer holds
    /// seated.
    rows_settled: bool,
}

/// One declaration's answers, and what settles once the declared
/// identities' lifecycle locks are released
/// ([`IdentityRuntime::finish_declaration`]).
pub(super) struct AppliedDeclaration {
    /// Each declared identity's outcome: Core's answer, or a refusal before
    /// Core was asked.
    pub(super) outcomes:
        BTreeMap<AgentIdentity, Result<DeclaredIdentityAnswer, IdentityRuntimeError>>,
    /// The identities Core bound to a session: their views settle as a
    /// wake's do.
    pub(super) bound: Vec<AgentIdentity>,
    /// Whether a first-boot row was settled: the manifest is rewritten.
    pub(super) rows_settled: bool,
}

impl IdentityRuntime {
    /// Declare `spec`'s identity in Core's mob ([`Self::register`],
    /// [`Self::update_spec`]) and record what the answer registers, under
    /// the identity's lifecycle lock. With no session bridge there is no
    /// Core mob: the spec is recorded and the answer is
    /// [`DeclaredIdentityAnswer::RecordedOnly`].
    pub(super) async fn declare_registration(
        &self,
        spec: DurableAgentSpec,
        registration: Registration,
    ) -> Result<DeclaredIdentityAnswer, IdentityRuntimeError> {
        let identity = spec.identity.clone();
        let lifecycle_lock = self.lifecycle_lock_for(&identity).await;
        let Some(bridge) = self.bridge.clone() else {
            let _lifecycle_guard = lifecycle_lock.lock().await;
            self.record_registration(spec, registration).await?;
            return Ok(DeclaredIdentityAnswer::RecordedOnly);
        };
        let specs = std::slice::from_ref(&spec);
        let mut composed = BTreeMap::new();
        let pending_deadline = Instant::now() + DECLARED_APPLY_PENDING_WAIT;
        let mut pending_backoff = DECLARED_APPLY_PENDING_FIRST_BACKOFF;
        loop {
            let inputs = self.declaration_inputs(&bridge, specs).await?;
            let (outcome, bound, rows_settled) = {
                let _lifecycle_guard = lifecycle_lock.lock().await;
                if registration == Registration::Update
                    && !self.entries.read().await.contains_key(&identity)
                {
                    return Err(IdentityRuntimeError::UnknownIdentity(identity));
                }
                match self
                    .apply_declaration(&bridge, specs, inputs, &composed)
                    .await
                {
                    Ok(mut applied) => {
                        let outcome = applied.outcomes.remove(&identity).unwrap_or_else(|| {
                            Err(IdentityRuntimeError::DeclarationNotApplied {
                                retryable: false,
                                detail: format!("Core answered no declaration of {identity}"),
                            })
                        });
                        if let Ok(answer) = &outcome
                            && answer.registers_spec()
                        {
                            self.record_registration(spec.clone(), registration).await?;
                        }
                        (outcome, applied.bound, applied.rows_settled)
                    }
                    // Nothing was declared, retryably: decided below, once
                    // the lifecycle lock is released.
                    Err(
                        error @ IdentityRuntimeError::DeclarationNotApplied {
                            retryable: true, ..
                        },
                    ) => (Err(error), Vec::new(), false),
                    Err(error) => return Err(error),
                }
            };
            self.finish_declaration(&bound, rows_settled).await;
            match outcome {
                // Another apply of the mob held Core's apply: nothing was
                // declared, and the identity is declared again once that
                // apply has had time to end.
                Err(IdentityRuntimeError::DeclarationNotApplied {
                    retryable: true,
                    detail,
                }) if Instant::now() + pending_backoff < pending_deadline => {
                    tracing::debug!(
                        %identity,
                        %detail,
                        "nothing was declared, retryably; the identity is declared again"
                    );
                    tokio::time::sleep(pending_backoff).await;
                    pending_backoff = (pending_backoff * 2).min(DECLARED_APPLY_PENDING_MAX_BACKOFF);
                    continue;
                }
                // The verdict builds a member and the first round declared
                // the identity's lifecycle facts only: the host's customizer
                // composes the build, with no lifecycle lock held, and the
                // identity is declared again with it, once.
                Ok(DeclaredIdentityAnswer::BuildRequired) if composed.is_empty() => {}
                outcome => return outcome,
            }
            let customizer = self.installed_agent_customizer().await;
            let active_peers: Vec<AgentIdentity> =
                self.entries.read().await.keys().cloned().collect();
            let managed_edges = self.desired_peer_edges.read().await.clone();
            let draft = self
                .compose_declared_build(&spec, customizer.as_deref(), active_peers, managed_edges)
                .await?;
            composed.insert(identity.clone(), draft);
        }
    }

    /// Record `spec` as its identity's registration: a new or replaced
    /// entry, or an update of a registered one.
    async fn record_registration(
        &self,
        spec: DurableAgentSpec,
        registration: Registration,
    ) -> Result<(), IdentityRuntimeError> {
        match registration {
            Registration::Register => {
                self.register_locked(spec).await;
                Ok(())
            }
            Registration::Update => self.update_spec_locked(spec).await,
        }
    }

    /// Read what a declaration of `specs` carries ([`DeclarationInputs`]),
    /// with no lifecycle lock held. `Err` refuses the whole declaration with
    /// nothing asked of Core: the lifecycle projection could not be prepared
    /// while no cursor is stored, the first-boot manifest cannot be read, or
    /// Core's member history cannot be read to tell a listed row's state.
    pub(super) async fn declaration_inputs(
        &self,
        bridge: &Arc<dyn SessionBridge>,
        specs: &[DurableAgentSpec],
    ) -> Result<DeclarationInputs, IdentityRuntimeError> {
        // The lifecycle projection's cursor is read before Core is asked,
        // so each seat the declaration journals is projected. While no
        // cursor is stored and none can be started, nothing is asked: a seat
        // Core committed before the cursor would lie behind it, and its
        // `after_create` would never be told.
        match self.prepare_lifecycle_projection().await {
            Ok(()) => {}
            Err(unprepared) if unprepared.cursor_stored() => {
                tracing::warn!(
                    error = %unprepared,
                    "the lifecycle projection could not be prepared; the declaration goes ahead \
                     past its stored cursor, and is projected once a preparation gets through"
                );
            }
            Err(unprepared) => {
                return Err(IdentityRuntimeError::DeclarationNotApplied {
                    retryable: true,
                    detail: format!(
                        "the lifecycle projection's cursor could not be read or started \
                         ({unprepared})"
                    ),
                });
            }
        }
        // An unreadable manifest refuses the declaration: which identities
        // it covers is unknown.
        self.load_first_boot_manifest().await?;
        let rows = self.pending_first_boot_rows_snapshot();
        let listed: Vec<(AgentIdentity, FirstBootRow)> = specs
            .iter()
            .filter_map(|spec| {
                rows.get(&spec.identity)
                    .map(|row| (spec.identity.clone(), row.clone()))
            })
            .collect();
        let mut first_boot = BTreeMap::new();
        let mut refused = BTreeMap::new();
        let mut rows_settled = false;
        if !listed.is_empty() {
            let history = bridge.member_binding_history().await.map_err(|error| {
                IdentityRuntimeError::MemberHistoryUnavailable {
                    detail: format!(
                        "reading Core's member history to carry out first-boot rows: {error}"
                    ),
                }
            })?;
            for (identity, row) in listed {
                match row.state_in(&history) {
                    FirstBootRowState::ToDo => {
                        let request = match &row {
                            FirstBootRow::Resume { session, .. } => {
                                meerkat_mob::FirstBootRequest::Resume {
                                    session_id: session.clone(),
                                }
                            }
                            FirstBootRow::Fresh { .. } => meerkat_mob::FirstBootRequest::Fresh,
                        };
                        first_boot.insert(identity, request);
                    }
                    // Carried out already. A resume of the session Core holds
                    // the identity on ends Core's first-boot preparation of it
                    // and starts nothing; an identity Core holds seated on no
                    // session has its preparation ended by the seat its
                    // declaration owes.
                    FirstBootRowState::Done => {
                        let member = crate::member_comms_id::roster_member_id_for_identity(
                            identity.as_str(),
                        );
                        match history
                            .last_binding(&member)
                            .filter(|binding| binding.is_live())
                            .and_then(|binding| binding.session_id().cloned())
                        {
                            Some(session_id) => {
                                first_boot.insert(
                                    identity,
                                    meerkat_mob::FirstBootRequest::Resume { session_id },
                                );
                            }
                            None => {
                                self.settle_first_boot_row_in_memory(&identity);
                                rows_settled = true;
                            }
                        }
                    }
                    FirstBootRowState::Contradicted {
                        core_session,
                        detail,
                    } => {
                        refused.insert(
                            identity.clone(),
                            IdentityRuntimeError::FirstBootRowRefused {
                                identity,
                                kind: row.kind(),
                                manifest_session: row.session().clone(),
                                core_session,
                                detail,
                            },
                        );
                    }
                }
            }
        }
        // A member a held pass could not publish owes its host a fresh held
        // completion, and a running held pass creates the identities it
        // covers: either way the identity's creation is held.
        let marks_read = match self.load_held_completion_marks().await {
            Ok(()) => true,
            Err(error) => {
                tracing::warn!(
                    %error,
                    "the held-completion marks could not be read; the identities are declared \
                     held, so none is created outside its held protocol"
                );
                false
            }
        };
        let held_pass = self.held_create_pass();
        let held = specs
            .iter()
            .map(|spec| &spec.identity)
            .filter(|identity| {
                !marks_read
                    || self.held_completion_owed(identity)
                    || held_pass.as_ref().is_some_and(|pass| pass.covers(identity))
            })
            .cloned()
            .collect();
        Ok(DeclarationInputs {
            first_boot,
            held,
            refused,
            rows_settled,
        })
    }

    /// Declare `specs` in Core's mob as one partial roster, with `inputs`
    /// and, for each identity in `composed`, its composed build; the others
    /// declare their lifecycle facts only. Called under the declared
    /// identities' lifecycle locks, which the caller holds until it has
    /// recorded what the answers register. `Err` when Core answered none of
    /// them ([`IdentityRuntimeError::DeclarationNotApplied`]).
    pub(super) async fn apply_declaration(
        &self,
        bridge: &Arc<dyn SessionBridge>,
        specs: &[DurableAgentSpec],
        inputs: DeclarationInputs,
        composed: &BTreeMap<AgentIdentity, AgentBuildDraft>,
    ) -> Result<AppliedDeclaration, IdentityRuntimeError> {
        let DeclarationInputs {
            first_boot,
            held,
            refused,
            rows_settled: mut settled,
        } = inputs;
        let mut roster = DeclaredRosterRequest::partial(
            specs
                .iter()
                .filter(|spec| !refused.contains_key(&spec.identity))
                .map(|spec| {
                    let declared = match composed.get(&spec.identity) {
                        Some(draft) => DeclaredIdentity::composed(spec.clone(), draft.clone()),
                        None => DeclaredIdentity::facts_only(spec.clone()),
                    };
                    declared.with_held_creation(held.contains(&spec.identity))
                }),
        );
        for (identity, request) in first_boot {
            if !refused.contains_key(&identity) {
                roster = roster.with_first_boot_request(identity, request);
            }
        }
        let mut outcomes: BTreeMap<_, _> = refused
            .into_iter()
            .map(|(identity, error)| (identity, Err(error)))
            .collect();
        if roster.identities.is_empty() {
            return Ok(AppliedDeclaration {
                outcomes,
                bound: Vec::new(),
                rows_settled: settled,
            });
        }
        let answer = bridge
            .apply_declared_roster(roster)
            .await
            .map_err(declaration_not_applied)?;
        // The rows Core carried out leave the manifest; Core ended their
        // first-boot preparation in the step that carried them out.
        for identity in &answer.first_boot_requests_carried_out {
            self.settle_first_boot_row_in_memory(identity);
            self.forget_seeded_host_preparation(identity);
            settled = true;
        }
        let mut bound = Vec::new();
        for (identity, declared) in answer.identities {
            // A seat tells the build its customizer composed for it to the
            // `after_create` the lifecycle projection reports; any other
            // answer seated nothing from it.
            match declared.bound_session() {
                Some(session) => {
                    self.bind_lifecycle_build(&identity, session);
                    bound.push(identity.clone());
                }
                None => self.forget_lifecycle_build(&identity),
            }
            outcomes.insert(identity, Ok(declared));
        }
        // An identity Core did not answer is not assumed declared.
        for spec in specs {
            outcomes.entry(spec.identity.clone()).or_insert_with(|| {
                Err(IdentityRuntimeError::DeclarationNotApplied {
                    retryable: false,
                    detail: format!("Core answered no declaration of {}", spec.identity),
                })
            });
        }
        Ok(AppliedDeclaration {
            outcomes,
            bound,
            rows_settled: settled,
        })
    }

    /// What a declaration settles once the declared identities' lifecycle
    /// locks are released: the lifecycle projection reads Core's journal,
    /// the manifest drops the settled rows, and each identity Core bound to
    /// a session settles its views as a wake does.
    pub(super) async fn finish_declaration(&self, bound: &[AgentIdentity], rows_settled: bool) {
        self.kick_lifecycle_projection();
        if rows_settled {
            self.rewrite_first_boot_manifest().await;
            self.publish_first_boot_pending().await;
        }
        for identity in bound {
            self.settle_successful_wake(identity).await;
        }
    }

    /// The member build a declaration of `spec` carries when Core needs it
    /// ([`super::super::bridge::DeclaredIdentityBuild::Composed`]): the
    /// host's `customize_build` of the spec's base draft with the runtime's
    /// real context, whose customizer tools are published into the
    /// identity's stable dispatcher and whose `after_create` the lifecycle
    /// projection tells once it reaches the seat. With no customizer it is
    /// the base draft. Built with no lifecycle lock held: the customizer may
    /// call into other identities.
    pub(super) async fn compose_declared_build(
        &self,
        spec: &DurableAgentSpec,
        customizer: Option<&dyn AgentCustomizer>,
        active_peers: Vec<AgentIdentity>,
        managed_edges: Vec<ManagedPeerEdge>,
    ) -> Result<AgentBuildDraft, IdentityRuntimeError> {
        let mut draft = Self::base_build_draft(spec);
        let Some(customizer) = customizer else {
            return Ok(draft);
        };
        let identity = &spec.identity;
        let cancelled = || {
            IdentityRuntimeError::Internal(format!(
                "the member build of {identity} was cancelled during shutdown before it ran"
            ))
        };
        let context = AgentBuildContext {
            identity: identity.clone(),
            active_peers,
            managed_edges,
            runtime_services: self.runtime_services(),
        };
        let mut shutdown = self.foreground_cancel.subscribe();
        if *shutdown.borrow() {
            return Err(cancelled());
        }
        let result = {
            let customize = customizer.customize_build(&context, spec, &mut draft);
            tokio::pin!(customize);
            tokio::select! {
                result = &mut customize => result,
                changed = shutdown.changed() => {
                    if changed.is_ok() && *shutdown.borrow() {
                        return Err(cancelled());
                    }
                    customize.await
                }
            }
        };
        match result {
            Ok(()) => {
                self.publish_customizer_tools(identity, &mut draft).await;
                self.note_lifecycle_build(identity, self.session_created_context(spec, &draft));
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
}

/// The error of a declaration Core answered no identity of: retryable when
/// another apply of the mob was in progress.
fn declaration_not_applied(error: BridgeError) -> IdentityRuntimeError {
    IdentityRuntimeError::DeclarationNotApplied {
        retryable: matches!(error, BridgeError::DeclaredApplyPending { .. }),
        detail: error.to_string(),
    }
}
