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
use std::time::Duration;

use futures::stream::{self, StreamExt};
use tokio::time::Instant;

use meerkat_core::types::SessionId;

use super::super::bridge::{
    BridgeError, DeclaredIdentity, DeclaredIdentityAnswer, DeclaredRosterRequest,
    MemberRuntimeBinding, UnlistedIdentityAnswer,
};
use super::super::contracts::AgentCustomizer;
use super::super::first_boot_manifest::FirstBootRowKind;
use super::super::first_boot_manifest::{FirstBootRow, FirstBootRowState};
use super::super::orchestrator::RestoreOutcome;
use super::super::types::{
    AgentBuildContext, AgentBuildDraft, AgentIdentity, DurableAgentSpec, ManagedPeerEdge,
};
use super::super::types::{FirstBootWait, IdentityLifecycleState};
use super::{
    CANDIDATE_CREATE_MEMBER, CANDIDATE_FIRST_BOOT_FRESH, CANDIDATE_FRESH_SESSION,
    CANDIDATE_MOVE_ROLE, CANDIDATE_RESUME_MEMBER, FirstBootRows, IdentityRuntime,
    IdentityRuntimeError, SessionBridge, WakeHold, binding_spawned_for_identity,
    classify_wake_failure, deterministic_build_rejection_reason, live_binding_runtime_id,
    refuse_runtime_plane_creation,
};

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

/// How a roster's declaration holds the declared identities' lifecycle locks
/// ([`IdentityRuntime::declare_roster`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeclarationLocks {
    /// The declaration takes the lifecycle lock of each identity it declares,
    /// in one stable order, for each of its apply steps, and holds none while
    /// the identities' builds are composed: the roster pass, a fleet's
    /// membership.
    Take,
    /// The caller holds the lifecycle lock of the declaration's one identity
    /// for the whole declaration: a door that seats its identity before it
    /// acts on it.
    Held,
}

/// What a roster's declaration in Core's mob answered
/// ([`IdentityRuntime::declare_roster`]).
pub(crate) struct RosterDeclaration {
    /// Each declared identity's outcome: Core's answer, or a refusal made
    /// before Core was asked (the first-boot manifest, Core's member history
    /// or the lifecycle projection's cursor could not be read, the
    /// identity's first-boot row contradicts Core's history, its build
    /// could not be composed) or when Core answered nothing for it.
    pub(crate) outcomes:
        BTreeMap<AgentIdentity, Result<DeclaredIdentityAnswer, IdentityRuntimeError>>,
    /// What a complete roster's apply did with each identity Core held
    /// declared that the roster does not list.
    pub(crate) unlisted: BTreeMap<AgentIdentity, UnlistedIdentityAnswer>,
    /// The build each identity was declared with when its verdict built a
    /// member: the host's composed build.
    pub(crate) composed: BTreeMap<AgentIdentity, AgentBuildDraft>,
}

/// What a roster pass does with an identity Core answers hosted on its first
/// use ([`DeclaredIdentityAnswer::HostedOnFirstUse`]: placed on a member
/// host, or bound to an external runtime, and held by no member).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostedIdentities {
    /// Its host creates its member now, through Core's ordinary create (the
    /// default pass).
    CreateNow,
    /// It is left for its first use: its first delivery, or the background
    /// warm, creates it (the lazy passes).
    LeaveForFirstUse,
}

/// What a roster pass made of its identities
/// ([`IdentityRuntime::declare_roster_pass`]).
pub(crate) struct RosterPassOutcomes {
    /// Each listed identity's restore outcome.
    pub(crate) outcomes: BTreeMap<AgentIdentity, RestoreOutcome>,
    /// What a complete roster did with each identity Core held declared
    /// that it does not list.
    pub(crate) unlisted: BTreeMap<AgentIdentity, UnlistedIdentityAnswer>,
}

/// What a door's declaration of its one identity left
/// ([`IdentityRuntime::declare_identity_locked`]).
pub(crate) struct SeatOutcome {
    /// Core held the identity's member before the declaration and kept it:
    /// nothing was seated, moved or created for it.
    pub(crate) already_seated: bool,
    /// The session the declaration bound the identity's member to, when it
    /// bound one.
    pub(crate) session_id: Option<SessionId>,
    /// The declaration bound the member to a session it started: a fresh
    /// seat, a fresh life, a move to a fresh session, or a hosted create.
    pub(crate) minted: bool,
    /// The build the declaration carried: the host's composed build, or the
    /// spec's base draft when nothing was built.
    pub(crate) draft: AgentBuildDraft,
}

/// One declaration's answers, and what settles once the declared
/// identities' lifecycle locks are released
/// ([`IdentityRuntime::finish_declaration`]).
pub(super) struct AppliedDeclaration {
    /// Each declared identity's outcome: Core's answer, or a refusal before
    /// Core was asked.
    pub(super) outcomes:
        BTreeMap<AgentIdentity, Result<DeclaredIdentityAnswer, IdentityRuntimeError>>,
    /// What a complete roster's apply did with each identity Core held
    /// declared that the roster does not list.
    pub(super) unlisted: BTreeMap<AgentIdentity, UnlistedIdentityAnswer>,
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
                    .apply_declaration(&bridge, specs, inputs, &composed, false)
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

    /// Declare `specs` in Core's mob as one roster, the host's whole roster
    /// when `complete` (Core then unlists each identity it holds declared
    /// that the roster does not list, and retires its member), and answer
    /// each identity ([`RosterDeclaration`]). Core's MobMachine decides each
    /// identity's verdict, and Core realizes it.
    ///
    /// The first round declares each identity's lifecycle facts only. Each
    /// identity whose verdict builds a member (Core answers
    /// [`DeclaredIdentityAnswer::BuildRequired`]) is declared again, once,
    /// in a partial roster, with the build the host's customizer composes
    /// (`customizer`, or the installed one), at most
    /// [`super::super::orchestrator::identity_restore_concurrency`] at once;
    /// a roster Core keeps calls no customizer. The builds name the roster's
    /// identities and the registered ones as their active peers.
    ///
    /// With `register`, each identity whose answer stands as its
    /// registration ([`DeclaredIdentityAnswer::registers_spec`]) is
    /// registered with its spec, under its lifecycle lock; any other answer
    /// leaves its previous registration, if any, as it was.
    ///
    /// An apply that meets another apply of the mob in progress is applied
    /// again after a short wait, within [`DECLARED_APPLY_PENDING_WAIT`]. A
    /// refusal or failure of the whole apply (no answer from Core, an
    /// unreadable first-boot manifest) is each identity's outcome: it never
    /// fails the others' answers, and a complete roster whose apply did not
    /// run unlists nothing.
    pub(crate) async fn declare_roster(
        &self,
        bridge: &Arc<dyn SessionBridge>,
        specs: &[DurableAgentSpec],
        complete: bool,
        locks: DeclarationLocks,
        register: bool,
        customizer: Option<&dyn AgentCustomizer>,
    ) -> RosterDeclaration {
        let mut declaration = RosterDeclaration {
            outcomes: BTreeMap::new(),
            unlisted: BTreeMap::new(),
            composed: BTreeMap::new(),
        };
        let mut peers = self
            .entries
            .read()
            .await
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        peers.extend(specs.iter().map(|spec| spec.identity.clone()));
        let peers = peers.into_iter().collect::<Vec<_>>();
        let mut round = specs.to_vec();
        let mut round_complete = complete;
        loop {
            let (outcomes, unlisted) = self
                .apply_roster_round(
                    bridge,
                    &round,
                    round_complete,
                    locks,
                    register,
                    &declaration.composed,
                )
                .await;
            declaration.unlisted.extend(unlisted);
            let mut build_required = Vec::new();
            for spec in round {
                match outcomes.get(&spec.identity) {
                    // The verdict builds a member and this round declared the
                    // identity's lifecycle facts only: it is declared again
                    // with its composed build.
                    Some(Ok(DeclaredIdentityAnswer::BuildRequired))
                        if !declaration.composed.contains_key(&spec.identity) =>
                    {
                        build_required.push(spec);
                    }
                    _ => {}
                }
            }
            for (identity, outcome) in outcomes {
                if !build_required.iter().any(|spec| spec.identity == identity) {
                    declaration.outcomes.insert(identity, outcome);
                }
            }
            if build_required.is_empty() {
                break;
            }
            let drafts = self
                .compose_declared_builds(&build_required, customizer, &peers)
                .await;
            round = Vec::with_capacity(build_required.len());
            for (spec, draft) in build_required.into_iter().zip(drafts) {
                match draft {
                    Ok(draft) => {
                        declaration.composed.insert(spec.identity.clone(), draft);
                        round.push(spec);
                    }
                    Err(error) => {
                        declaration
                            .outcomes
                            .insert(spec.identity.clone(), Err(error));
                    }
                }
            }
            // The roster's unlisting was the first round's: the identities
            // declared again are part of it.
            round_complete = false;
            if round.is_empty() {
                break;
            }
        }
        declaration
    }

    /// One apply of [`Self::declare_roster`]: `specs` declared as one roster
    /// (`complete` or partial) with their builds in `composed`, under their
    /// lifecycle locks when `locks` says so, applied again while another
    /// apply of the mob is in progress (within
    /// [`DECLARED_APPLY_PENDING_WAIT`]). Answers each identity, and what a
    /// complete roster unlisted.
    async fn apply_roster_round(
        &self,
        bridge: &Arc<dyn SessionBridge>,
        specs: &[DurableAgentSpec],
        complete: bool,
        locks: DeclarationLocks,
        register: bool,
        composed: &BTreeMap<AgentIdentity, AgentBuildDraft>,
    ) -> (
        BTreeMap<AgentIdentity, Result<DeclaredIdentityAnswer, IdentityRuntimeError>>,
        BTreeMap<AgentIdentity, UnlistedIdentityAnswer>,
    ) {
        let pending_deadline = Instant::now() + DECLARED_APPLY_PENDING_WAIT;
        let mut pending_backoff = DECLARED_APPLY_PENDING_FIRST_BACKOFF;
        loop {
            let inputs = match self.declaration_inputs(bridge, specs).await {
                Ok(inputs) => inputs,
                Err(error) => return (refused_alike(specs, &error), BTreeMap::new()),
            };
            let mut guards = Vec::new();
            if locks == DeclarationLocks::Take {
                let ordered = specs
                    .iter()
                    .map(|spec| spec.identity.clone())
                    .collect::<BTreeSet<_>>();
                for identity in &ordered {
                    guards.push(self.lifecycle_lock_for(identity).await.lock_owned().await);
                }
            }
            let applied = match self
                .apply_declaration(bridge, specs, inputs, composed, complete)
                .await
            {
                Ok(applied) => applied,
                // Another apply of the mob held Core's apply: nothing was
                // declared, and the roster is declared again once that apply
                // has had time to end.
                Err(IdentityRuntimeError::DeclarationNotApplied {
                    retryable: true,
                    detail,
                }) if Instant::now() + pending_backoff < pending_deadline => {
                    drop(guards);
                    tracing::debug!(
                        identities = specs.len(),
                        %detail,
                        "nothing was declared, retryably; the roster is declared again"
                    );
                    tokio::time::sleep(pending_backoff).await;
                    pending_backoff = (pending_backoff * 2).min(DECLARED_APPLY_PENDING_MAX_BACKOFF);
                    continue;
                }
                Err(error) => {
                    drop(guards);
                    return (refused_alike(specs, &error), BTreeMap::new());
                }
            };
            if register {
                for spec in specs {
                    if let Some(Ok(answer)) = applied.outcomes.get(&spec.identity)
                        && answer.registers_spec()
                    {
                        self.register_locked(spec.clone()).await;
                    }
                }
            }
            drop(guards);
            // A door settles its identity's views itself, once, with its own
            // step.
            let bound: &[AgentIdentity] = match locks {
                DeclarationLocks::Take => &applied.bound,
                DeclarationLocks::Held => &[],
            };
            self.finish_declaration(bound, applied.rows_settled).await;
            return (applied.outcomes, applied.unlisted);
        }
    }

    /// The composed build of each of `specs` ([`Self::compose_declared_build`]),
    /// in order, with `customizer` or the installed one, at most
    /// [`super::super::orchestrator::identity_restore_concurrency`] at once,
    /// with no lifecycle lock taken. `peers` are the identities each build
    /// names as its active peers.
    async fn compose_declared_builds(
        &self,
        specs: &[DurableAgentSpec],
        customizer: Option<&dyn AgentCustomizer>,
        peers: &[AgentIdentity],
    ) -> Vec<Result<AgentBuildDraft, IdentityRuntimeError>> {
        let installed = self.installed_agent_customizer().await;
        let customizer = customizer.or(installed.as_deref());
        let managed_edges = self.desired_peer_edges.read().await.clone();
        // The stream's items are indices, not borrows of `specs`: a stream of
        // borrowed items makes every caller's future fail its `Send` check
        // ("not general enough").
        stream::iter(0..specs.len())
            .map(|index| {
                let spec = &specs[index];
                let peers = peers.to_vec();
                let managed_edges = managed_edges.clone();
                async move {
                    self.compose_declared_build(spec, customizer, peers, managed_edges)
                        .await
                }
            })
            .buffered(super::super::orchestrator::identity_restore_concurrency())
            .collect::<Vec<_>>()
            .await
    }

    /// A roster pass's declaration of `specs` ([`Self::declare_roster`],
    /// the host's whole roster when `complete`), and each listed identity's
    /// restore outcome:
    /// - a member Core seated, kept, restamped or moved: `Resumed` on its
    ///   own session, or `Created` on a session the declaration started (a
    ///   fresh seat, a fresh life, a move to a fresh session), from one read
    ///   of Core's member history after the declaration; `Dormant` when no
    ///   binding reads;
    /// - an identity awaiting its held creation: `Dormant` (the held pass
    ///   creates it);
    /// - an identity hosted on its first use: created through its host now
    ///   ([`HostedIdentities::CreateNow`]), or left `Dormant` for its first
    ///   use;
    /// - any other answer, and a refusal before Core was asked: `Refused`,
    ///   typed ([`Self::declared_answer_error`]).
    ///
    /// With `register`, each identity whose answer stands as its
    /// registration is registered with its spec. Each identity Core retired
    /// or removed because the complete roster does not list it leaves the
    /// registry. Without a session bridge there is no Core mob: each spec is
    /// registered (with `register`) and `Dormant`.
    pub(crate) async fn declare_roster_pass(
        &self,
        specs: &[DurableAgentSpec],
        complete: bool,
        register: bool,
        customizer: Option<&dyn AgentCustomizer>,
        hosted: HostedIdentities,
    ) -> RosterPassOutcomes {
        let Some(bridge) = self.bridge.clone() else {
            let mut outcomes = BTreeMap::new();
            for spec in specs {
                if register {
                    let lifecycle_lock = self.lifecycle_lock_for(&spec.identity).await;
                    let _lifecycle_guard = lifecycle_lock.lock().await;
                    self.register_locked(spec.clone()).await;
                }
                outcomes.insert(
                    spec.identity.clone(),
                    RestoreOutcome::Dormant {
                        draft: Self::base_build_draft(spec),
                    },
                );
            }
            return RosterPassOutcomes {
                outcomes,
                unlisted: BTreeMap::new(),
            };
        };
        // The sessions Core held the identities on before the declaration
        // (their last bindings, and the first-boot rows' sessions): a seat
        // on one of them resumes the identity, any other seat starts it.
        let before = self.sessions_held_before(specs).await;
        let mut declaration = self
            .declare_roster(
                &bridge,
                specs,
                complete,
                DeclarationLocks::Take,
                register,
                customizer,
            )
            .await;
        // The roster is registered: the runtime is composed. Every member
        // Core built held before now has its customizer tools published and
        // its hold released, and every later build of a registered identity
        // is prepared inline.
        self.compose_member_builds().await;
        let after = self.core_view_history().await.ok().flatten();
        let held_pass = self.held_create_pass();
        let mut outcomes = BTreeMap::new();
        let mut hosted_creates = Vec::new();
        for spec in specs {
            let identity = &spec.identity;
            let draft = declaration
                .composed
                .get(identity)
                .cloned()
                .unwrap_or_else(|| Self::base_build_draft(spec));
            let answer = declaration.outcomes.remove(identity).unwrap_or_else(|| {
                Err(IdentityRuntimeError::DeclarationNotApplied {
                    retryable: false,
                    detail: format!("Core answered no declaration of {identity}"),
                })
            });
            let outcome = match answer {
                Ok(
                    DeclaredIdentityAnswer::Seated { session_id, .. }
                    | DeclaredIdentityAnswer::Embodied { session_id, .. },
                ) => {
                    let minted = !before
                        .get(identity)
                        .is_some_and(|sessions| sessions.contains(&session_id));
                    member_outcome(after.as_ref(), identity, draft, minted)
                }
                Ok(
                    DeclaredIdentityAnswer::FreshLife { .. }
                    | DeclaredIdentityAnswer::RoleRotated { .. },
                ) => member_outcome(after.as_ref(), identity, draft, true),
                Ok(
                    DeclaredIdentityAnswer::Kept
                    | DeclaredIdentityAnswer::Noted { .. }
                    | DeclaredIdentityAnswer::Restamped { .. }
                    | DeclaredIdentityAnswer::RoleMigrated { .. },
                ) => member_outcome(after.as_ref(), identity, draft, false),
                Ok(
                    DeclaredIdentityAnswer::AwaitingHeldCreation
                    | DeclaredIdentityAnswer::RecordedOnly,
                ) => RestoreOutcome::Dormant { draft },
                // A candidate launch: Core declares nothing, and the identity
                // is deferred to the authoritative launch when the verdict
                // Core reports that launch would give changes its member.
                Ok(DeclaredIdentityAnswer::Refused {
                    refusal: meerkat_mob::DeclaredEntryRefusal::NonAuthoritative { would_be },
                }) => match self
                    .candidate_deferral(spec, would_be, after.as_ref())
                    .await
                {
                    None => member_outcome(after.as_ref(), identity, draft, false),
                    Some(error) => {
                        self.record_wake_refusal(identity, &error).await;
                        RestoreOutcome::Refused(Arc::new(error))
                    }
                },
                Ok(DeclaredIdentityAnswer::HostedOnFirstUse) => {
                    if hosted == HostedIdentities::CreateNow
                        && held_pass.as_ref().is_none_or(|pass| !pass.covers(identity))
                    {
                        hosted_creates.push(spec.clone());
                    }
                    RestoreOutcome::Dormant { draft }
                }
                Ok(answer) => {
                    let error = self
                        .declared_answer_error(identity, answer, after.as_ref())
                        .unwrap_or_else(|| {
                            IdentityRuntimeError::Internal(format!(
                                "Core's answer for {identity} was mapped as a refusal"
                            ))
                        });
                    self.record_wake_refusal(identity, &error).await;
                    RestoreOutcome::Refused(Arc::new(error))
                }
                Err(error) => {
                    self.record_wake_refusal(identity, &error).await;
                    RestoreOutcome::Refused(Arc::new(error))
                }
            };
            outcomes.insert(identity.clone(), outcome);
        }
        // An identity hosted on its first use is created through its host
        // now, by the default pass: at most the restore concurrency at once,
        // each under its own lifecycle lock.
        if !hosted_creates.is_empty() {
            let created = stream::iter(hosted_creates)
                .map(|spec| async move {
                    let result = self.create_hosted_member(&spec, customizer).await;
                    (spec.identity, result)
                })
                .buffer_unordered(super::super::orchestrator::identity_restore_concurrency())
                .collect::<Vec<_>>()
                .await;
            for (identity, result) in created {
                let outcome = match result {
                    Ok((binding, draft)) => RestoreOutcome::Created { binding, draft },
                    Err(error) => {
                        self.record_wake_refusal(&identity, &error).await;
                        RestoreOutcome::Refused(Arc::new(error))
                    }
                };
                outcomes.insert(identity, outcome);
            }
        }
        // An identity Core retired or removed because the complete roster
        // does not list it leaves the registry; one whose retirement failed
        // keeps its row, and the next complete roster retires it again.
        for (identity, answer) in &declaration.unlisted {
            match answer {
                UnlistedIdentityAnswer::Retired | UnlistedIdentityAnswer::Removed => {
                    self.forget_unlisted_registration(identity).await;
                }
                UnlistedIdentityAnswer::Failed { error } => tracing::warn!(
                    %identity,
                    %error,
                    "Core could not retire the member of an identity the roster no longer \
                     lists; the next complete roster retires it again"
                ),
                UnlistedIdentityAnswer::NotAdopted { reason } => tracing::info!(
                    %identity,
                    ?reason,
                    "Core left a legacy member the roster does not list as it is; the next \
                     complete roster decides it again"
                ),
            }
        }
        RosterPassOutcomes {
            outcomes,
            unlisted: declaration.unlisted,
        }
    }

    /// A door's declaration of `spec`'s identity, under the identity's
    /// lifecycle lock, which the caller holds: a one-identity partial
    /// roster ([`Self::declare_roster`]), whose build `customizer` (or the
    /// installed one) composes when Core's verdict builds a member. An
    /// identity Core answers hosted on its first use is created through its
    /// host here. Any answer that leaves no member to act on (a held role
    /// move, a held creation, a deferral, a refusal, a failure) is the
    /// door's typed refusal ([`Self::declared_answer_error`]), recorded for
    /// the views when it is final or held.
    pub(crate) async fn declare_identity_locked(
        &self,
        spec: &DurableAgentSpec,
        customizer: Option<&dyn AgentCustomizer>,
    ) -> Result<SeatOutcome, IdentityRuntimeError> {
        let identity = &spec.identity;
        let bridge = Arc::clone(self.core_bridge(identity)?);
        let before = self.sessions_held_before(std::slice::from_ref(spec)).await;
        let mut declaration = self
            .declare_roster(
                &bridge,
                std::slice::from_ref(spec),
                false,
                DeclarationLocks::Held,
                false,
                customizer,
            )
            .await;
        let draft = declaration
            .composed
            .remove(identity)
            .unwrap_or_else(|| Self::base_build_draft(spec));
        let answer = declaration.outcomes.remove(identity).unwrap_or_else(|| {
            Err(IdentityRuntimeError::DeclarationNotApplied {
                retryable: false,
                detail: format!("Core answered no declaration of {identity}"),
            })
        });
        self.door_seat(
            &bridge,
            spec,
            answer,
            before.get(identity),
            draft,
            customizer,
        )
        .await
    }

    /// A door's seat of `spec`'s identity from Core's `answer` for its
    /// declaration, under the identity's lifecycle lock, which the caller
    /// holds ([`Self::declare_identity_locked`]): `held_before` are the
    /// sessions Core held the identity on before the declaration, so a seat
    /// on another one started the identity, and `draft` is the build the
    /// declaration carried. An identity Core answers hosted on its first use
    /// is created through its host here; any answer that leaves no member to
    /// act on is the door's typed refusal ([`Self::declared_answer_error`]),
    /// recorded for the views when it is final or held.
    pub(super) async fn door_seat(
        &self,
        bridge: &Arc<dyn SessionBridge>,
        spec: &DurableAgentSpec,
        answer: Result<DeclaredIdentityAnswer, IdentityRuntimeError>,
        held_before: Option<&BTreeSet<SessionId>>,
        draft: AgentBuildDraft,
        customizer: Option<&dyn AgentCustomizer>,
    ) -> Result<SeatOutcome, IdentityRuntimeError> {
        let identity = &spec.identity;
        let answer = match answer {
            Ok(answer) => answer,
            Err(error) => {
                self.record_wake_refusal(identity, &error).await;
                return Err(error);
            }
        };
        match answer {
            DeclaredIdentityAnswer::Seated { session_id, .. }
            | DeclaredIdentityAnswer::Embodied { session_id, .. } => {
                let minted = !held_before.is_some_and(|sessions| sessions.contains(&session_id));
                Ok(SeatOutcome {
                    already_seated: false,
                    session_id: Some(session_id),
                    minted,
                    draft,
                })
            }
            DeclaredIdentityAnswer::FreshLife { session_id }
            | DeclaredIdentityAnswer::RoleRotated { session_id } => Ok(SeatOutcome {
                already_seated: false,
                session_id: Some(session_id),
                minted: true,
                draft,
            }),
            DeclaredIdentityAnswer::Restamped { .. }
            | DeclaredIdentityAnswer::RoleMigrated { .. } => Ok(SeatOutcome {
                already_seated: false,
                session_id: None,
                minted: false,
                draft,
            }),
            DeclaredIdentityAnswer::Kept
            | DeclaredIdentityAnswer::Noted { .. }
            | DeclaredIdentityAnswer::RecordedOnly => Ok(SeatOutcome {
                already_seated: true,
                session_id: None,
                minted: false,
                draft,
            }),
            DeclaredIdentityAnswer::HostedOnFirstUse => {
                let installed = self.installed_agent_customizer().await;
                let customizer = customizer.or(installed.as_deref());
                let peers = self.entries.read().await.keys().cloned().collect();
                let managed_edges = self.desired_peer_edges.read().await.clone();
                let draft = self
                    .compose_declared_build(spec, customizer, peers, managed_edges)
                    .await?;
                let binding = self
                    .create_hosted_member_locked(bridge, spec, &draft)
                    .await?;
                self.kick_lifecycle_projection();
                Ok(SeatOutcome {
                    already_seated: false,
                    session_id: binding.session_id,
                    minted: true,
                    draft,
                })
            }
            DeclaredIdentityAnswer::Refused {
                refusal: meerkat_mob::DeclaredEntryRefusal::NonAuthoritative { would_be },
            } => {
                let history = self.core_view_history().await.ok().flatten();
                match self
                    .candidate_deferral(spec, would_be, history.as_ref())
                    .await
                {
                    None => Ok(SeatOutcome {
                        already_seated: true,
                        session_id: None,
                        minted: false,
                        draft,
                    }),
                    Some(error) => {
                        self.record_wake_refusal(identity, &error).await;
                        Err(error)
                    }
                }
            }
            DeclaredIdentityAnswer::AwaitingHeldCreation
                if self
                    .held_create_pass()
                    .is_some_and(|pass| pass.covers(identity)) =>
            {
                Err(IdentityRuntimeError::IdentityWakeRetryable {
                    identity: identity.clone(),
                    session_id: None,
                    detail: "the identity awaits its held creation by the held bootstrap pass \
                             in progress"
                        .to_string(),
                })
            }
            answer => {
                let history = match self.core_view_history().await {
                    Ok(history) => history,
                    Err(_) => None,
                };
                let error = self
                    .declared_answer_error(identity, answer, history.as_ref())
                    .unwrap_or_else(|| {
                        IdentityRuntimeError::Internal(format!(
                            "Core's answer for {identity} was mapped as a refusal"
                        ))
                    });
                self.record_wake_refusal(identity, &error).await;
                Err(error)
            }
        }
    }

    /// The typed refusal of an answer that leaves no member to act on, read
    /// with Core's member history `history` where the refusal names a
    /// session; `None` for an answer that leaves a member (a seat, a kept or
    /// moved member) or nothing to refuse (an identity hosted on its first
    /// use, nothing declared).
    pub(super) fn declared_answer_error(
        &self,
        identity: &AgentIdentity,
        answer: DeclaredIdentityAnswer,
        history: Option<&meerkat_mob::MemberBindingHistory>,
    ) -> Option<IdentityRuntimeError> {
        let last_session = || {
            history.and_then(|history| {
                history
                    .last_binding(&crate::member_comms_id::roster_member_id_for_identity(
                        identity.as_str(),
                    ))
                    .and_then(|binding| binding.session_id().cloned())
            })
        };
        Some(match answer {
            DeclaredIdentityAnswer::Seated { .. }
            | DeclaredIdentityAnswer::Embodied { .. }
            | DeclaredIdentityAnswer::Kept
            | DeclaredIdentityAnswer::Restamped { .. }
            | DeclaredIdentityAnswer::RoleMigrated { .. }
            | DeclaredIdentityAnswer::RoleRotated { .. }
            | DeclaredIdentityAnswer::FreshLife { .. }
            | DeclaredIdentityAnswer::Noted { .. }
            | DeclaredIdentityAnswer::HostedOnFirstUse
            | DeclaredIdentityAnswer::RecordedOnly => return None,
            DeclaredIdentityAnswer::RoleMigrationHeld { from, to } => {
                let detail = format!(
                    "the roster moves {identity} from role '{from}' to '{to}', and this \
                     activation declared no migration from '{from}': Core holds the member \
                     under its previous role and refuses its wakes until a roster declares \
                     the migration or restores the role"
                );
                match last_session() {
                    Some(session_id) => IdentityRuntimeError::WakeHeld {
                        identity: identity.clone(),
                        session_id,
                        hold: WakeHold::RoleMigrationNotApplied,
                        detail,
                    },
                    None => IdentityRuntimeError::DeclarationNotApplied {
                        retryable: false,
                        detail,
                    },
                }
            }
            DeclaredIdentityAnswer::AwaitingHeldCreation => {
                IdentityRuntimeError::HeldCompletionOwed {
                    identity: identity.clone(),
                }
            }
            DeclaredIdentityAnswer::PreparationPending => {
                IdentityRuntimeError::IdentityWakeRetryable {
                    identity: identity.clone(),
                    session_id: None,
                    detail: "the identity awaits its host's preparation".to_string(),
                }
            }
            DeclaredIdentityAnswer::BuildRequired => IdentityRuntimeError::DeclarationNotApplied {
                retryable: true,
                detail: format!(
                    "Core still asks for the composed build of {identity} after it was \
                     declared with one"
                ),
            },
            DeclaredIdentityAnswer::Deferred { reason } => {
                IdentityRuntimeError::IdentityWakeRetryable {
                    identity: identity.clone(),
                    session_id: None,
                    detail: format!(
                        "Core left the identity's declaration to its next apply: {reason}"
                    ),
                }
            }
            DeclaredIdentityAnswer::Refused { refusal } => match refusal {
                meerkat_mob::DeclaredEntryRefusal::HostAdopted => {
                    IdentityRuntimeError::IdentityHostAdopted {
                        identity: identity.clone(),
                    }
                }
                meerkat_mob::DeclaredEntryRefusal::NameHeldByWorker => {
                    IdentityRuntimeError::IdentityNameHeldByRawMember {
                        identity: identity.clone(),
                        member: crate::member_comms_id::roster_member_id_for_identity(
                            identity.as_str(),
                        )
                        .to_string(),
                        session_id: last_session(),
                    }
                }
                refusal => IdentityRuntimeError::DeclarationNotApplied {
                    retryable: false,
                    detail: format!("Core refused the declaration of {identity}: {refusal:?}"),
                },
            },
            DeclaredIdentityAnswer::Failed { error } => {
                classify_wake_failure(identity, None, error)
            }
        })
    }

    /// Why a candidate launch defers `spec`'s identity to the authoritative
    /// launch, when Core refused its declaration because this composition is
    /// not authoritative: Core's `would_be`, the verdict an authoritative
    /// composition gives the same declaration over the same state, projected
    /// onto the change that launch would make of the identity's member
    /// ([`super::candidate_deferral_of`]). `None` when it would change
    /// nothing: Core keeps the member (and no pending first-boot `Fresh` row
    /// starts its fresh life), or leaves the identity as it is. An owed seat
    /// takes the session its pending first-boot row names, a fresh one, or
    /// the identity's stored session, which Core holds or lost (read from
    /// Core's member history `history`); a verdict the authoritative launch
    /// refuses is the same typed refusal here, a role move this activation
    /// declared no migration for held (`WakeHeld`). Core decides the verdict
    /// of a member a host seated before identity entries existed on the
    /// basis that launch's adoption of it would give.
    /// A first-boot row Core's history shows done settles in memory only: a
    /// candidate leaves the manifest as it found it.
    async fn candidate_deferral(
        &self,
        spec: &DurableAgentSpec,
        would_be: meerkat_mob::DeclaredEntryVerdict,
        history: Option<&meerkat_mob::MemberBindingHistory>,
    ) -> Option<IdentityRuntimeError> {
        use meerkat_mob::DeclaredEntryVerdict as Verdict;
        let identity = &spec.identity;
        let deferred = |operation: &'static str| IdentityRuntimeError::InvalidState {
            identity: identity.clone(),
            state: IdentityLifecycleState::Dormant,
            operation,
        };
        let member = crate::member_comms_id::roster_member_id_for_identity(identity.as_str());
        let last = history.and_then(|history| history.last_binding(&member));
        let own = last.filter(|binding| binding_spawned_for_identity(binding, identity));
        let row = self
            .pending_first_boot_rows_snapshot()
            .remove(identity)
            .filter(|row| match history.map(|history| row.state_in(history)) {
                Some(FirstBootRowState::Done) => {
                    self.settle_first_boot_row_in_memory(identity);
                    false
                }
                // Refused before Core was asked; never reaches here.
                Some(FirstBootRowState::Contradicted { .. }) => false,
                Some(FirstBootRowState::ToDo) | None => true,
            });
        match would_be {
            Verdict::Kept => matches!(row, Some(FirstBootRow::Fresh { .. }))
                .then(|| first_boot_fresh_pending(identity)),
            Verdict::SeatOwed => Some(match row {
                Some(FirstBootRow::Resume { .. }) => deferred(CANDIDATE_RESUME_MEMBER),
                Some(FirstBootRow::Fresh { .. }) => deferred(CANDIDATE_CREATE_MEMBER),
                None => {
                    let Some(binding) = own else {
                        return Some(deferred(CANDIDATE_CREATE_MEMBER));
                    };
                    if *binding.role() != spec.profile {
                        return Some(deferred(CANDIDATE_MOVE_ROLE));
                    }
                    let Some(session) = binding.session_id() else {
                        return Some(deferred(CANDIDATE_CREATE_MEMBER));
                    };
                    let bridge = self.bridge.as_ref()?;
                    match bridge.durable_session_held(session).await {
                        Ok(true) => deferred(CANDIDATE_RESUME_MEMBER),
                        Ok(false) => deferred(CANDIDATE_FRESH_SESSION),
                        Err(error) => IdentityRuntimeError::Internal(format!(
                            "candidate launch: whether Core holds the stored session {session} \
                             of {identity} could not be read: {error}"
                        )),
                    }
                }
            }),
            Verdict::HostedOnFirstUse
            | Verdict::AwaitHeldCreation
            | Verdict::PreparationPending => Some(deferred(CANDIDATE_CREATE_MEMBER)),
            Verdict::RoleMigrate | Verdict::RoleRotate => Some(deferred(CANDIDATE_MOVE_ROLE)),
            Verdict::RuntimeModeRestamp => Some(deferred(CANDIDATE_RESUME_MEMBER)),
            Verdict::RoleMigrationHeld => Some(
                role_migration_held(identity, &spec.profile, last)
                    .unwrap_or_else(|| deferred(CANDIDATE_MOVE_ROLE)),
            ),
            Verdict::LifecycleInFlight => Some(IdentityRuntimeError::IdentityWakeRetryable {
                identity: identity.clone(),
                session_id: None,
                detail: "a lifecycle operation of the identity's member is in flight: the \
                         authoritative launch leaves the identity's declaration to its next \
                         apply"
                    .to_string(),
            }),
            Verdict::RecordOnly => None,
            Verdict::NameHeldByWorker => Some(IdentityRuntimeError::IdentityNameHeldByRawMember {
                identity: identity.clone(),
                member: member.to_string(),
                session_id: last.and_then(|binding| binding.session_id().cloned()),
            }),
            Verdict::HostAdopted => Some(IdentityRuntimeError::IdentityHostAdopted {
                identity: identity.clone(),
            }),
        }
    }

    /// The sessions Core held each of `specs`' identities on before a
    /// declaration: the identity's last binding's session, and its pending
    /// first-boot `Resume` row's session. Empty for an identity whose
    /// history does not read.
    pub(super) async fn sessions_held_before(
        &self,
        specs: &[DurableAgentSpec],
    ) -> BTreeMap<AgentIdentity, BTreeSet<SessionId>> {
        let history = self.core_view_history().await.ok().flatten();
        let rows = if self.load_first_boot_manifest().await.is_ok() {
            self.pending_first_boot_rows_snapshot()
        } else {
            BTreeMap::new()
        };
        specs
            .iter()
            .map(|spec| {
                let identity = &spec.identity;
                let mut sessions = BTreeSet::new();
                if let Some(session) = history.as_ref().and_then(|history| {
                    history
                        .last_binding(&crate::member_comms_id::roster_member_id_for_identity(
                            identity.as_str(),
                        ))
                        .and_then(|binding| binding.session_id().cloned())
                }) {
                    sessions.insert(session);
                }
                if let Some(FirstBootRow::Resume { session, .. }) = rows.get(identity) {
                    sessions.insert(session.clone());
                }
                (identity.clone(), sessions)
            })
            .collect()
    }

    /// Create `spec`'s member through its host (Core's ordinary create, with
    /// the entry's custody), for an identity Core answered hosted on its
    /// first use: its build is composed with no lifecycle lock held, and the
    /// create runs under the identity's lifecycle lock. Answers the binding
    /// Core created and the build.
    async fn create_hosted_member(
        &self,
        spec: &DurableAgentSpec,
        customizer: Option<&dyn AgentCustomizer>,
    ) -> Result<(MemberRuntimeBinding, AgentBuildDraft), IdentityRuntimeError> {
        let identity = &spec.identity;
        let bridge = Arc::clone(self.core_bridge(identity)?);
        let installed = self.installed_agent_customizer().await;
        let customizer = customizer.or(installed.as_deref());
        let peers = self.entries.read().await.keys().cloned().collect();
        let managed_edges = self.desired_peer_edges.read().await.clone();
        let draft = self
            .compose_declared_build(spec, customizer, peers, managed_edges)
            .await?;
        let binding = {
            let lifecycle_lock = self.lifecycle_lock_for(identity).await;
            let _lifecycle_guard = lifecycle_lock.lock().await;
            self.create_hosted_member_locked(&bridge, spec, &draft)
                .await?
        };
        self.kick_lifecycle_projection();
        self.settle_successful_wake(identity).await;
        Ok((binding, draft))
    }

    /// The create of [`Self::create_hosted_member`] under the identity's
    /// lifecycle lock, which the caller holds: the build `draft` is the
    /// one its seat's `after_create` reports.
    async fn create_hosted_member_locked(
        &self,
        bridge: &Arc<dyn SessionBridge>,
        spec: &DurableAgentSpec,
        draft: &AgentBuildDraft,
    ) -> Result<MemberRuntimeBinding, IdentityRuntimeError> {
        let identity = &spec.identity;
        refuse_runtime_plane_creation(identity)?;
        let binding = bridge
            .create_session(identity, spec, draft)
            .await
            .map_err(|error| match deterministic_build_rejection_reason(&error) {
                Some(reason) => IdentityRuntimeError::HostRejectedBuild {
                    identity: identity.clone(),
                    reason,
                },
                None => IdentityRuntimeError::Internal(format!("bridge create_session: {error}")),
            });
        match binding
            .as_ref()
            .ok()
            .and_then(|binding| binding.session_id.as_ref())
        {
            Some(session) => self.bind_lifecycle_build(identity, session),
            None => self.forget_lifecycle_build(identity),
        }
        binding
    }

    /// Drop the registration of an identity Core retired or removed because
    /// a complete roster no longer lists it, under its lifecycle lock.
    async fn forget_unlisted_registration(&self, identity: &AgentIdentity) {
        let lifecycle_lock = self.lifecycle_lock_for(identity).await;
        let _lifecycle_guard = lifecycle_lock.lock().await;
        self.event_channels.write().await.remove(identity);
        self.entries.write().await.remove(identity);
    }

    /// Record the member ids Meta handed the mob as host-pending before it
    /// was built (`host_pending_members_before_build`): each keeps its mark
    /// in Core until the declaration that carries its first-boot row out
    /// ends it.
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
    pub(super) fn forget_seeded_host_preparation(&self, identity: &AgentIdentity) {
        self.host_pending_seeded
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&crate::member_comms_id::roster_member_id_for_identity(
                identity.as_str(),
            ));
    }

    /// The rows of the first-boot manifest still to carry out, by identity.
    pub(super) fn pending_first_boot_rows_snapshot(&self) -> BTreeMap<AgentIdentity, FirstBootRow> {
        match &*self
            .first_boot
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            FirstBootRows::Pending { rows, .. } => rows.clone(),
            _ => BTreeMap::new(),
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

    /// Declare `specs` in Core's mob as one roster, the host's whole roster
    /// when `complete` (Core then unlists each identity it holds declared
    /// that the roster does not list), with `inputs` and, for each identity
    /// in `composed`, its composed build; the others declare their
    /// lifecycle facts only. Called under the declared identities' lifecycle
    /// locks, which the caller holds until it has recorded what the answers
    /// register. `Err` when Core answered none of them
    /// ([`IdentityRuntimeError::DeclarationNotApplied`]).
    pub(super) async fn apply_declaration(
        &self,
        bridge: &Arc<dyn SessionBridge>,
        specs: &[DurableAgentSpec],
        inputs: DeclarationInputs,
        composed: &BTreeMap<AgentIdentity, AgentBuildDraft>,
        complete: bool,
    ) -> Result<AppliedDeclaration, IdentityRuntimeError> {
        let DeclarationInputs {
            first_boot,
            held,
            refused,
            rows_settled: mut settled,
        } = inputs;
        // A complete roster lists every identity it declares, and Core
        // unlists the rest: an identity refused here is declared to no one,
        // so the roster is applied as a partial one instead, and nothing is
        // unlisted until no listed identity is refused.
        let complete = if complete && !refused.is_empty() {
            tracing::warn!(
                refused = refused.len(),
                "identities of the complete roster were refused before Core was asked; the \
                 roster is applied as a partial one and unlists nothing this time"
            );
            false
        } else {
            complete
        };
        let declared = specs
            .iter()
            .filter(|spec| !refused.contains_key(&spec.identity))
            .map(|spec| {
                let declared = match composed.get(&spec.identity) {
                    Some(draft) => DeclaredIdentity::composed(spec.clone(), draft.clone()),
                    None => DeclaredIdentity::facts_only(spec.clone()),
                };
                declared.with_held_creation(held.contains(&spec.identity))
            });
        let mut roster = if complete {
            DeclaredRosterRequest::complete(declared)
        } else {
            DeclaredRosterRequest::partial(declared)
        };
        for (identity, request) in first_boot {
            if !refused.contains_key(&identity) {
                roster = roster.with_first_boot_request(identity, request);
            }
        }
        let mut outcomes: BTreeMap<_, _> = refused
            .into_iter()
            .map(|(identity, error)| (identity, Err(error)))
            .collect();
        // A partial roster with nothing left to declare asks Core nothing; a
        // complete one is applied even then, so Core unlists what it no
        // longer lists.
        if roster.identities.is_empty() && !complete {
            return Ok(AppliedDeclaration {
                outcomes,
                unlisted: BTreeMap::new(),
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
            unlisted: answer.unlisted,
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

/// The restore outcome of a member Core holds after a declaration: its
/// binding from Core's member history `history`, `Created` on a session the
/// declaration started (`minted`) and `Resumed` otherwise; `Dormant` when no
/// live binding reads.
fn member_outcome(
    history: Option<&meerkat_mob::MemberBindingHistory>,
    identity: &AgentIdentity,
    draft: AgentBuildDraft,
    minted: bool,
) -> RestoreOutcome {
    let binding = history.and_then(|history| {
        let binding = history.last_binding(
            &crate::member_comms_id::roster_member_id_for_identity(identity.as_str()),
        )?;
        Some(MemberRuntimeBinding {
            agent_runtime_id: live_binding_runtime_id(binding)?,
            session_id: binding.session_id().cloned(),
        })
    });
    match binding {
        Some(binding) if minted => RestoreOutcome::Created { binding, draft },
        Some(binding) => RestoreOutcome::Resumed { binding, draft },
        None => RestoreOutcome::Dormant { draft },
    }
}

/// Each of `specs` refused alike by one refusal of the whole declaration: a
/// declaration Core answered no identity of, or one that could not be made.
fn refused_alike(
    specs: &[DurableAgentSpec],
    error: &IdentityRuntimeError,
) -> BTreeMap<AgentIdentity, Result<DeclaredIdentityAnswer, IdentityRuntimeError>> {
    specs
        .iter()
        .map(|spec| {
            (
                spec.identity.clone(),
                Err(shared_refusal(&spec.identity, error)),
            )
        })
        .collect()
}

/// `identity`'s copy of a refusal its whole declaration shares, kept typed
/// where it is a refusal of every identity alike.
fn shared_refusal(identity: &AgentIdentity, error: &IdentityRuntimeError) -> IdentityRuntimeError {
    match error {
        IdentityRuntimeError::FirstBootManifestUnreadable { path, detail } => {
            IdentityRuntimeError::FirstBootManifestUnreadable {
                path: path.clone(),
                detail: detail.clone(),
            }
        }
        IdentityRuntimeError::DeclarationNotApplied { retryable, detail } => {
            IdentityRuntimeError::DeclarationNotApplied {
                retryable: *retryable,
                detail: detail.clone(),
            }
        }
        IdentityRuntimeError::MemberHistoryUnavailable { detail } => {
            IdentityRuntimeError::IdentityHistoryUnavailable {
                identity: identity.clone(),
                detail: detail.clone(),
            }
        }
        other => IdentityRuntimeError::Internal(other.to_string()),
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

/// A candidate launch's refusal of a pending first-boot `Fresh` row of a
/// member Core keeps: the authoritative launch starts its fresh life,
/// replacing the member.
fn first_boot_fresh_pending(identity: &AgentIdentity) -> IdentityRuntimeError {
    IdentityRuntimeError::FirstBootRowPending {
        identity: identity.clone(),
        kind: FirstBootRowKind::Fresh,
        waiting_for: FirstBootWait::CarryOut,
        operation: CANDIDATE_FIRST_BOOT_FRESH,
    }
}

/// Every launch's hold of a role move this activation declared no migration
/// for: the identity's wakes are held on the session Core last ran it on
/// (`last`, Core's last binding of the identity); `None` when that binding
/// names no session.
fn role_migration_held(
    identity: &AgentIdentity,
    profile: &meerkat_mob::ProfileName,
    last: Option<&meerkat_mob::MemberBinding>,
) -> Option<IdentityRuntimeError> {
    let binding = last?;
    let session_id = binding.session_id()?.clone();
    Some(IdentityRuntimeError::WakeHeld {
        identity: identity.clone(),
        session_id,
        hold: WakeHold::RoleMigrationNotApplied,
        detail: format!(
            "the roster profile '{profile}' differs from the role '{}' Core last ran the \
             identity under, and this activation declared no migration from it",
            binding.role()
        ),
    })
}

#[cfg(test)]
#[path = "declare_tests.rs"]
mod tests;
