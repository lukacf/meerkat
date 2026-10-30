//! Lifecycle side effects as projections over Core's member lifecycle
//! journal.
//!
//! Core journals each lifecycle step of an identity's member: a seat, a
//! rotation onto a fresh session (one append holds the retirement of the
//! incarnation that ends and the seat of the next one), a retirement. Some
//! of these steps owe side effects that Meta runs:
//!
//! | Journal fact | Side effects |
//! |---|---|
//! | the seat of an identity's member | the customizer's `after_create`, with the session the seat binds |
//! | a retirement the roster's unlisting asked for | the distillation of the retired session, the exit interview (`retire`), the retirement hook (`unlisted`) |
//! | a reset's retirement (online, a first-boot `Fresh` row's, a role move onto a fresh session) | the distillation of the retired session behind its reset boundary, the retirement hook (`reset`) |
//! | a delete's retirement | the drop of the retired session's semantic-memory scope, the exit interview (`delete`), the retirement hook (`delete`) |
//! | a rotation onto a fresh session no Meta verb asked for (a host's own) | as a reset's |
//! | any other retirement: a reload's, a respawn's, Core's own | none |
//!
//! A release (`retire`, a scoped retire, a reprofile) is no journaled step:
//! its retirement hook stays the verb's own, called before the verb answers.
//!
//! # At least once
//!
//! The projection reads the journal page by page from a cursor kept in
//! Meta's metadata store ([`crate::runtime::LifecycleProjectionCursor`]) and
//! advances the cursor only once the side effects of the facts it covers
//! ran. A process stopped in between, by a kill or a crash, runs them again
//! at its next start, so no side effect a step owes is lost. Each one
//! tolerates the repeat: the exit interview is keyed by the time its step
//! was asked, so a repeat queues nothing more; a repeated distillation reads
//! the session again, and the memory store keeps one copy of what it already
//! holds; the retirement hook and `after_create` may hear of the same
//! incarnation again after a restart, and a host keys what it does by the
//! incarnation (`agent_runtime_id`, generation) or the session. Within one
//! process a page that failed part way runs again without repeating the
//! effects that ran.
//!
//! # What the journal does not say
//!
//! Core's journal does not say which Meta verb asked for a retirement: a
//! reset and a delete are the same rotation to Core, and the roster's
//! unlisting the same retirement as any other. Each Meta verb records that
//! ([`crate::runtime::LifecycleEffectIntent`]) before it asks Core for the
//! step, keyed by the incarnation the step ends, and the projection reads
//! the record when it reaches the step's retirement; once the cursor past it
//! is recorded, the record goes. A step Core refused before any of it
//! started takes its record back.
//!
//! What a step knows that no durable record keeps (the exact registration
//! the retiring incarnation held, read before Core was asked; the build
//! draft an `after_create` reports) is kept in this process for the
//! projection to add when it reaches the step. After a restart the
//! retirement hook names no registration, and `after_create` reports a
//! context derived from the identity's roster spec (or from Core's binding),
//! with no system prompt.
//!
//! # Scale
//!
//! The projection reads one page at a time, at most [`PAGE_LIMIT`] journal
//! events, with at most one read of Core's member history per page that has
//! a seat or a rotation in it, and runs the side effects of different
//! identities at most [`EFFECT_CONCURRENCY`] at once (one identity's in
//! journal order). It reads nothing per member and wakes no member: an idle
//! journal costs one empty page read per sweep.
//!
//! # Start and stop
//!
//! The identity context that serves the runtime binds the projection
//! ([`IdentityRuntime::bind_lifecycle_projection`]). It starts at the end of
//! the context's first roster pass, or at the first step a Meta verb asks
//! Core for after the binding. Each such step kicks it, and a sweep every
//! [`SWEEP_INTERVAL`] reads the steps nothing kicked (Core's own, a host's).
//! The cursor is read, or started at the journal's head when there is none,
//! before the first step Meta asks Core for in this process, so every step
//! Meta asks for is projected. A unified runtime's identity bootstrap waits
//! for the projection to reach the journal's head before it answers, so the
//! side effects of the bootstrap's own steps, and those a stopped process
//! still owed, run before the bootstrap answers. A shutdown lets it reach the
//! head within the runtime's event drain window, then stops it; what it did
//! not reach runs at the next start.
//!
//! Nothing is projected on a candidate launch, whose verbs change no stored
//! member, or over a session bridge without Core's journal.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex as StdMutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use futures::stream::{self, StreamExt};
use meerkat_core::types::SessionId;
use tokio::sync::{Mutex, Notify, watch};
use tokio::task::JoinHandle;

use super::super::bridge::{
    BridgeError, MemberLifecycleFact, MemberLifecycleFactRecord, MemberLifecycleJournalPage,
};
use super::super::contracts::AgentCustomizer;
use super::super::types::{
    AgentIdentity, AgentRuntimeId, RetiredIdentityCause, RetiredIdentityMember,
};
use super::records::unix_ms_now;
use super::{IdentityRuntime, IdentityRuntimeError, binding_spawned_for_identity};
use crate::memory::distiller::DistillCause;
use crate::mob_handle_runtime::SessionCreatedContext;
use crate::runtime::{LifecycleEffectIntent, LifecycleEffectVerb, LifecycleProjectionCursor};

/// The most journal events one page reads: Core's own bound.
const PAGE_LIMIT: usize = meerkat_mob::IDENTITY_ENTRY_PAGE_MAX;

/// How many identities' side effects of one page run at once.
const EFFECT_CONCURRENCY: usize = 16;

/// How often the projection reads the journal when nothing kicked it.
const SWEEP_INTERVAL: Duration = Duration::from_secs(5);

fn lock<T>(mutex: &StdMutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The identity runtime's lifecycle projection: where it stands in Core's
/// journal, its task, and what this process knows of the steps it asked Core
/// for that no durable record keeps.
pub(super) struct LifecycleProjection {
    phase: StdMutex<Phase>,
    /// Serializes the preparations.
    preparing: Mutex<()>,
    /// The cursor, held by the one run in progress.
    run: Mutex<RunState>,
    /// The runtime the task runs for, bound by the identity context that
    /// serves it.
    owner: StdMutex<Option<Owner>>,
    /// Wakes the task.
    kick: Arc<Notify>,
    /// Set once the runtime shuts down: the task finishes the run it is in,
    /// or runs once more, and ends; no task starts after it.
    stopping: watch::Sender<bool>,
    task: StdMutex<Option<JoinHandle<()>>>,
    /// The incarnation a step this process asked Core for is about to end,
    /// as the retirement hook reads it, by identity: its registration was
    /// read before Core was asked.
    retiring: StdMutex<BTreeMap<AgentIdentity, RetiredIdentityMember>>,
    /// What the build of the latest seat-like step of an identity made, by
    /// identity, for the `after_create` of the seat it asks Core for.
    created: StdMutex<BTreeMap<AgentIdentity, SessionCreatedContext>>,
}

#[derive(Clone)]
enum Phase {
    /// Not prepared yet, or the last preparation failed.
    Unprepared,
    /// The cursor is read, over the journal of the mob incarnation
    /// `incarnation`.
    Ready { incarnation: String },
    /// Nothing is projected: a candidate launch, or a bridge without Core's
    /// journal.
    Disabled,
}

#[derive(Clone)]
struct Owner {
    runtime: Weak<IdentityRuntime>,
    /// The customizer the identity context carries: the one its roster
    /// passes build with.
    customizer: Option<Arc<dyn AgentCustomizer>>,
}

#[derive(Default)]
struct RunState {
    /// The cursor of the last journal event whose side effects ran.
    cursor: u64,
    /// The cursor the store holds.
    persisted: u64,
    /// Facts past `cursor` whose side effects ran in this process: a page
    /// that failed after them runs again without them.
    done: BTreeSet<u64>,
    /// The intents (identity, generation) of the retirements up to `cursor`,
    /// removed once the store holds a cursor past them.
    consumed: Vec<(String, u64)>,
}

impl LifecycleProjection {
    pub(super) fn new() -> Self {
        Self {
            phase: StdMutex::new(Phase::Unprepared),
            preparing: Mutex::new(()),
            run: Mutex::new(RunState::default()),
            owner: StdMutex::new(None),
            kick: Arc::new(Notify::new()),
            stopping: watch::channel(false).0,
            task: StdMutex::new(None),
            retiring: StdMutex::new(BTreeMap::new()),
            created: StdMutex::new(BTreeMap::new()),
        }
    }

    fn phase(&self) -> Phase {
        lock(&self.phase).clone()
    }

    fn set_phase(&self, phase: Phase) {
        *lock(&self.phase) = phase;
    }

    fn stash_retiring(&self, member: RetiredIdentityMember) {
        lock(&self.retiring).insert(member.identity.clone(), member);
    }

    /// Replace the kept incarnation of `member`'s identity, while the
    /// projection has not taken it yet.
    fn refine_retiring(&self, member: RetiredIdentityMember) {
        let mut retiring = lock(&self.retiring);
        if retiring
            .get(&member.identity)
            .is_some_and(|kept| kept.generation == member.generation)
        {
            retiring.insert(member.identity.clone(), member);
        }
    }

    fn take_retiring(
        &self,
        identity: &AgentIdentity,
        generation: u64,
    ) -> Option<RetiredIdentityMember> {
        let mut retiring = lock(&self.retiring);
        if retiring
            .get(identity)
            .is_some_and(|kept| kept.generation == generation)
        {
            return retiring.remove(identity);
        }
        None
    }

    fn take_created(&self, identity: &AgentIdentity) -> Option<SessionCreatedContext> {
        lock(&self.created).remove(identity)
    }
}

/// A step that ends an identity's live incarnation, prepared before Core is
/// asked for it ([`IdentityRuntime::prepare_lifecycle_retirement`]).
pub(super) struct PreparedRetirement {
    identity: AgentIdentity,
    generation: u64,
    /// The step's intent was recorded.
    recorded: bool,
    /// The incarnation as the retirement hook reads it, when a hook is
    /// installed.
    retiring: Option<RetiredIdentityMember>,
}

impl PreparedRetirement {
    /// The generation of the incarnation the step ends.
    pub(super) fn generation(&self) -> u64 {
        self.generation
    }
}

/// One side effect a journal fact owes an identity.
enum Effect {
    /// The customizer's `after_create` for the seat of the incarnation Core
    /// bound to `session`; `role` and `labels` are Core's binding's, for a
    /// context derived without the build's own.
    Told {
        session: SessionId,
        role: meerkat_mob::ProfileName,
        labels: BTreeMap<String, String>,
    },
    Retirement(Retirement),
}

/// The retirement of one incarnation, with what it owes.
struct Retirement {
    member: meerkat_mob::AgentIdentity,
    generation: u64,
    role: meerkat_mob::ProfileName,
    session: Option<SessionId>,
    /// `Unlisted`, `Reset` or `Delete`.
    cause: RetiredIdentityCause,
    /// When its step was asked: the key of the exit interview it queues.
    asked_at_ms: Option<u64>,
}

/// A fact whose side effects wait for Core's member history: a seat, or a
/// retirement no intent names that Core journaled with its successor's seat.
enum Pending {
    Seat {
        cursor: u64,
        identity: AgentIdentity,
        member: meerkat_mob::AgentIdentity,
        generation: u64,
    },
    Rotation {
        cursor: u64,
        identity: AgentIdentity,
        member: meerkat_mob::AgentIdentity,
        generation: u64,
        role: meerkat_mob::ProfileName,
        successor: u64,
    },
}

/// The facts of `page`, read after `after`, the projection runs now, and the
/// cursor it stands at once they ran. Core journals a rotation's retirement
/// and its successor's seat in one append, at consecutive cursors: a
/// retirement that ends a page which may have stopped at its limit waits for
/// the next read, which starts right before it, so it is read with the event
/// that follows it.
fn processable(
    page: &MemberLifecycleJournalPage,
    after: u64,
) -> (&[MemberLifecycleFactRecord], u64) {
    if let Some((last, rest)) = page.records.split_last()
        && matches!(last.fact, MemberLifecycleFact::Retired { .. })
        && last.cursor == page.next
        && page.next.saturating_sub(after) >= PAGE_LIMIT as u64
        && last.cursor > after.saturating_add(1)
    {
        return (rest, last.cursor - 1);
    }
    (&page.records, page.next)
}

/// The durable identity a Core member id names.
fn meta_identity(member: &meerkat_mob::AgentIdentity) -> Option<AgentIdentity> {
    AgentIdentity::parse(&crate::member_comms_id::durable_identity_for_roster_member(
        member.as_str(),
    ))
    .ok()
}

/// The seat of `member` at `generation` follows its retirement at
/// `generation` in the same append: Core rotated it.
fn rotation_successor(
    records: &[MemberLifecycleFactRecord],
    index: usize,
    member: &meerkat_mob::AgentIdentity,
    generation: u64,
) -> Option<u64> {
    let retired = records.get(index)?;
    let next = records.get(index + 1)?;
    if next.cursor != retired.cursor.saturating_add(1) {
        return None;
    }
    match &next.fact {
        MemberLifecycleFact::Seated {
            member: seated,
            generation: successor,
        } if seated == member && *successor > generation => Some(*successor),
        _ => None,
    }
}

impl IdentityRuntime {
    /// Bind the lifecycle projection to this runtime, with the customizer the
    /// identity context that serves it carries: the context's construction
    /// does. The projection runs its task for a bound runtime only.
    pub(crate) fn bind_lifecycle_projection(
        self: &Arc<Self>,
        customizer: Option<Arc<dyn AgentCustomizer>>,
    ) {
        *lock(&self.lifecycle_projection.owner) = Some(Owner {
            runtime: Arc::downgrade(self),
            customizer,
        });
    }

    /// Read the projection's cursor, or start it at the journal's head when
    /// the store holds none for the mob's current incarnation. Called before
    /// every step Meta asks Core for, so every such step is projected; cheap
    /// once prepared. A candidate launch and a bridge without Core's journal
    /// turn the projection off.
    pub(super) async fn prepare_lifecycle_projection(&self) -> Result<(), String> {
        let projection = &self.lifecycle_projection;
        if !matches!(projection.phase(), Phase::Unprepared) {
            return Ok(());
        }
        let _preparing = projection.preparing.lock().await;
        if !matches!(projection.phase(), Phase::Unprepared) {
            return Ok(());
        }
        if self.is_candidate_launch() {
            projection.set_phase(Phase::Disabled);
            return Ok(());
        }
        let Some(bridge) = self.bridge.as_ref() else {
            projection.set_phase(Phase::Disabled);
            return Ok(());
        };
        let head = match bridge.member_lifecycle_journal_head().await {
            Ok(head) => head,
            Err(BridgeError::Unsupported { .. }) => {
                tracing::debug!(
                    runtime_instance_id = %self.runtime_instance_id,
                    "the session bridge reads no member lifecycle journal: no lifecycle side \
                     effect is projected"
                );
                projection.set_phase(Phase::Disabled);
                return Ok(());
            }
            Err(error) => {
                return Err(format!(
                    "reading the head of Core's member lifecycle journal: {error}"
                ));
            }
        };
        let (store, mob_id) = self.identity_record_store();
        let stored = store
            .lifecycle_projection_cursor(&mob_id)
            .await
            .map_err(|error| format!("reading the lifecycle projection's cursor: {error}"))?;
        let cursor = match stored {
            Some(stored) if stored.incarnation == head.incarnation => stored.cursor,
            // No cursor yet (a new mob, or one an earlier release ran, which
            // owed its side effects in its own process), or one of an
            // earlier incarnation of the mob, whose journal is gone: the
            // projection starts at the journal's head.
            _ => {
                store
                    .set_lifecycle_projection_cursor(&LifecycleProjectionCursor {
                        mob_id,
                        incarnation: head.incarnation.clone(),
                        cursor: head.cursor,
                        recorded_at_ms: unix_ms_now(),
                    })
                    .await
                    .map_err(|error| {
                        format!("recording the lifecycle projection's first cursor: {error}")
                    })?;
                head.cursor
            }
        };
        *projection.run.lock().await = RunState {
            cursor,
            persisted: cursor,
            ..RunState::default()
        };
        projection.set_phase(Phase::Ready {
            incarnation: head.incarnation,
        });
        Ok(())
    }

    /// Start the projection's task: for a bound, prepared runtime that is
    /// not shutting down, once.
    pub(crate) fn start_lifecycle_projection(&self) {
        let projection = &self.lifecycle_projection;
        if *projection.stopping.borrow() || !matches!(projection.phase(), Phase::Ready { .. }) {
            return;
        }
        let Some(owner) = lock(&projection.owner).clone() else {
            return;
        };
        let mut task = lock(&projection.task);
        if task.is_some() {
            return;
        }
        let kick = Arc::clone(&projection.kick);
        let mut stopping = projection.stopping.subscribe();
        let runtime = owner.runtime;
        *task = Some(tokio::spawn(async move {
            loop {
                let Some(identities) = runtime.upgrade() else {
                    return;
                };
                if let Err(error) = identities.run_lifecycle_projection_to_head().await {
                    tracing::warn!(
                        runtime_instance_id = %identities.runtime_instance_id,
                        %error,
                        "the lifecycle projection stopped short of the journal's head; it \
                         reads from where it stopped at its next sweep"
                    );
                }
                drop(identities);
                if *stopping.borrow() {
                    return;
                }
                tokio::select! {
                    () = kick.notified() => {}
                    () = tokio::time::sleep(SWEEP_INTERVAL) => {}
                    changed = stopping.changed() => {
                        if changed.is_err() {
                            return;
                        }
                    }
                }
            }
        }));
    }

    /// A step Meta asked Core for was journaled: wake the projection, and
    /// start it when it is bound and has not started yet.
    pub(super) fn kick_lifecycle_projection(&self) {
        self.lifecycle_projection.kick.notify_one();
        self.start_lifecycle_projection();
    }

    /// Run the side effects of every lifecycle step Core journaled so far,
    /// here: the projection reaches the journal's head before this answers,
    /// or stops at a failure, which is logged and read again at its next
    /// sweep. Never call it from a retirement hook or a customizer's
    /// `after_create`: it would wait for the side effect that is running it.
    pub(crate) async fn settle_lifecycle_projection(&self) {
        if let Err(error) = self.prepare_lifecycle_projection().await {
            tracing::warn!(
                runtime_instance_id = %self.runtime_instance_id,
                %error,
                "the lifecycle projection could not be prepared; its next step tries again"
            );
            return;
        }
        if let Err(error) = self.run_lifecycle_projection_to_head().await {
            tracing::warn!(
                runtime_instance_id = %self.runtime_instance_id,
                %error,
                "the lifecycle projection stopped short of the journal's head; it reads from \
                 where it stopped at its next sweep"
            );
        }
    }

    /// Stop the projection at a shutdown: its task reaches the journal's head
    /// once more and ends, by `deadline` at the latest. What it does not
    /// reach runs at the next start, from the cursor the store holds.
    pub(crate) async fn stop_lifecycle_projection(&self, deadline: tokio::time::Instant) {
        let projection = &self.lifecycle_projection;
        projection.stopping.send_replace(true);
        projection.kick.notify_one();
        let task = lock(&projection.task).take();
        let Some(mut task) = task else {
            return;
        };
        if tokio::time::timeout_at(deadline, &mut task).await.is_err() {
            tracing::warn!(
                runtime_instance_id = %self.runtime_instance_id,
                "the lifecycle projection did not reach the journal's head within the shutdown's \
                 window; what it did not reach runs at the next start"
            );
            task.abort();
            let _ = task.await;
        }
    }

    /// Before Core is asked for a step that ends `identity`'s live
    /// incarnation (`verb`): record what the step owes, keyed by that
    /// incarnation, and keep the incarnation as the retirement hook reads it
    /// (`cause`; `None` for a step that owes no hook). `None` when Core holds
    /// no live member, so the step retires nothing. Refused typed, with
    /// nothing asked of Core, when the record cannot be written; a reload's
    /// record, which owes nothing, is best-effort.
    pub(super) async fn prepare_lifecycle_retirement(
        &self,
        identity: &AgentIdentity,
        verb: LifecycleEffectVerb,
        cause: Option<RetiredIdentityCause>,
    ) -> Result<Option<PreparedRetirement>, IdentityRuntimeError> {
        let Some(binding) = self
            .core_last_binding(identity)
            .await?
            .filter(meerkat_mob::MemberBinding::is_live)
        else {
            return Ok(None);
        };
        let generation = binding.generation().get();
        let session_id = binding.session_id().cloned();
        let retiring = match cause {
            Some(cause) => self.retiring_member(identity, &binding, cause).await,
            None => None,
        };
        let recorded = match self
            .record_lifecycle_effect_intent(identity, generation, verb, session_id.clone())
            .await
        {
            Ok(recorded) => recorded,
            // A replacement on the same session owes no side effect, and an
            // unrecorded retirement owes none either.
            Err(detail) if verb == LifecycleEffectVerb::Reload => {
                tracing::warn!(
                    %identity,
                    generation,
                    %detail,
                    "the replacement's lifecycle effect intent could not be recorded; it owes \
                     no side effect, so it goes on"
                );
                false
            }
            Err(detail) => {
                return Err(IdentityRuntimeError::IdentityWakeRetryable {
                    identity: identity.clone(),
                    session_id,
                    detail: format!(
                        "nothing was asked of Core: the side effects the step owes could not be \
                         recorded ({detail})"
                    ),
                });
            }
        };
        if let Some(member) = retiring.clone() {
            self.lifecycle_projection.stash_retiring(member);
        }
        Ok(Some(PreparedRetirement {
            identity: identity.clone(),
            generation,
            recorded,
            retiring,
        }))
    }

    /// Core answered the step `prepared` prepared: a registration still
    /// current now was not ended by it, so the hook will name none; then wake
    /// the projection.
    pub(super) async fn settle_lifecycle_retirement(&self, prepared: Option<PreparedRetirement>) {
        if let Some(PreparedRetirement {
            retiring: Some(member),
            ..
        }) = prepared
        {
            let mut retiring = Some(member);
            self.settle_retired_registration(&mut retiring).await;
            if let Some(member) = retiring {
                self.lifecycle_projection.refine_retiring(member);
            }
        }
        self.kick_lifecycle_projection();
    }

    /// Core refused the step `prepared` prepared before any of it started:
    /// its record and what this process kept of it go.
    pub(super) async fn abandon_lifecycle_retirement(&self, prepared: Option<PreparedRetirement>) {
        let Some(prepared) = prepared else {
            return;
        };
        let _ = self
            .lifecycle_projection
            .take_retiring(&prepared.identity, prepared.generation);
        if !prepared.recorded {
            return;
        }
        let (store, mob_id) = self.identity_record_store();
        if let Err(error) = store
            .clear_lifecycle_effect_intent(&mob_id, prepared.identity.as_str(), prepared.generation)
            .await
        {
            tracing::warn!(
                identity = %prepared.identity,
                generation = prepared.generation,
                %error,
                "the lifecycle effect intent of a step Core refused could not be removed; the \
                 next step that ends the same incarnation records its own in its place"
            );
        }
    }

    /// Keep what the build of a seat-like step of `identity` made, which the
    /// step is about to ask Core for, for the `after_create` the projection
    /// tells once it reaches the seat.
    pub(super) fn note_lifecycle_build(
        &self,
        identity: &AgentIdentity,
        context: SessionCreatedContext,
    ) {
        lock(&self.lifecycle_projection.created).insert(identity.clone(), context);
    }

    /// The step [`Self::note_lifecycle_build`] kept a build for seated
    /// nothing.
    pub(super) fn forget_lifecycle_build(&self, identity: &AgentIdentity) {
        lock(&self.lifecycle_projection.created).remove(identity);
    }

    /// Record what the step that ends `identity`'s incarnation `generation`
    /// owes (`verb`), durably, before Core is asked for it. `Ok(false)` when
    /// nothing is projected, so nothing is recorded.
    async fn record_lifecycle_effect_intent(
        &self,
        identity: &AgentIdentity,
        generation: u64,
        verb: LifecycleEffectVerb,
        session_id: Option<SessionId>,
    ) -> Result<bool, String> {
        self.prepare_lifecycle_projection().await?;
        let Phase::Ready { incarnation } = self.lifecycle_projection.phase() else {
            return Ok(false);
        };
        let (store, mob_id) = self.identity_record_store();
        store
            .set_lifecycle_effect_intent(&LifecycleEffectIntent {
                mob_id,
                incarnation,
                identity: identity.as_str().to_string(),
                generation,
                verb,
                session_id,
                recorded_at_ms: unix_ms_now(),
            })
            .await
            .map_err(|error| format!("recording the step's lifecycle effect intent: {error}"))?;
        Ok(true)
    }

    /// Read the journal from the cursor to its head, running each page's side
    /// effects and then recording the cursor past it. Runs are serialized:
    /// the task's and a settle's.
    async fn run_lifecycle_projection_to_head(&self) -> Result<(), String> {
        let Phase::Ready { incarnation } = self.lifecycle_projection.phase() else {
            return Ok(());
        };
        let Some(bridge) = self.bridge.clone() else {
            return Ok(());
        };
        let mut run = self.lifecycle_projection.run.lock().await;
        let result = loop {
            let after = run.cursor;
            let page = match bridge
                .member_lifecycle_journal_page(after, PAGE_LIMIT)
                .await
            {
                Ok(page) => page,
                Err(error) => {
                    break Err(format!(
                        "reading Core's member lifecycle journal after {after}: {error}"
                    ));
                }
            };
            if page.next <= after {
                break Ok(());
            }
            let (records, upto) = processable(&page, after);
            let consumed = match self
                .apply_lifecycle_page(&incarnation, records, &mut run.done)
                .await
            {
                Ok(consumed) => consumed,
                Err(error) => break Err(error),
            };
            run.cursor = upto;
            run.done.retain(|cursor| *cursor > upto);
            run.consumed.extend(consumed);
            if !records.is_empty()
                && let Err(error) = self.persist_lifecycle_cursor(&incarnation, &mut run).await
            {
                break Err(error);
            }
        };
        // A run that moved past pages with no fact keeps that too.
        if run.persisted != run.cursor
            && let Err(error) = self.persist_lifecycle_cursor(&incarnation, &mut run).await
        {
            return result.and(Err(error));
        }
        result
    }

    /// Record the run's cursor, then remove the intents of the retirements
    /// the recorded cursor covers.
    async fn persist_lifecycle_cursor(
        &self,
        incarnation: &str,
        run: &mut RunState,
    ) -> Result<(), String> {
        let (store, mob_id) = self.identity_record_store();
        store
            .set_lifecycle_projection_cursor(&LifecycleProjectionCursor {
                mob_id: mob_id.clone(),
                incarnation: incarnation.to_string(),
                cursor: run.cursor,
                recorded_at_ms: unix_ms_now(),
            })
            .await
            .map_err(|error| format!("recording the lifecycle projection's cursor: {error}"))?;
        run.persisted = run.cursor;
        for (identity, generation) in std::mem::take(&mut run.consumed) {
            if let Err(error) = store
                .clear_lifecycle_effect_intent(&mob_id, &identity, generation)
                .await
            {
                tracing::warn!(
                    %identity,
                    generation,
                    %error,
                    "the lifecycle effect intent of a step whose side effects ran could not be \
                     removed; it names an incarnation that ended, which no later step ends again"
                );
            }
        }
        Ok(())
    }

    /// Run the side effects of `records`, skipping those `done` names and
    /// adding those that run to it. Answers the intents the page's
    /// retirements consumed, for removal once the cursor past them is
    /// recorded. A failure fails the page, which runs again.
    async fn apply_lifecycle_page(
        &self,
        incarnation: &str,
        records: &[MemberLifecycleFactRecord],
        done: &mut BTreeSet<u64>,
    ) -> Result<Vec<(String, u64)>, String> {
        let customizer = self.after_create_customizer().await;
        let mut effects: Vec<(u64, AgentIdentity, Effect)> = Vec::new();
        let mut pending: Vec<Pending> = Vec::new();
        let mut consumed = Vec::new();
        for (index, record) in records.iter().enumerate() {
            match &record.fact {
                MemberLifecycleFact::Seated { member, generation } => {
                    // Nothing to tell without a customizer.
                    if customizer.is_none() {
                        continue;
                    }
                    let Some(identity) = meta_identity(member) else {
                        continue;
                    };
                    pending.push(Pending::Seat {
                        cursor: record.cursor,
                        identity,
                        member: member.clone(),
                        generation: *generation,
                    });
                }
                MemberLifecycleFact::Retired {
                    member,
                    generation,
                    role,
                } => {
                    let Some(identity) = meta_identity(member) else {
                        continue;
                    };
                    let (store, mob_id) = self.identity_record_store();
                    let intent = store
                        .lifecycle_effect_intent(&mob_id, identity.as_str(), *generation)
                        .await
                        .map_err(|error| {
                            format!(
                                "reading the lifecycle effect intent of {identity} at \
                                 generation {generation}: {error}"
                            )
                        })?;
                    if intent.is_some() {
                        consumed.push((identity.as_str().to_string(), *generation));
                    }
                    // An intent of an earlier incarnation of the mob names
                    // another journal's step.
                    match intent.filter(|intent| intent.incarnation == incarnation) {
                        Some(intent) => {
                            let cause = match intent.verb {
                                LifecycleEffectVerb::Unlisted => RetiredIdentityCause::Unlisted,
                                LifecycleEffectVerb::Reset => RetiredIdentityCause::Reset,
                                LifecycleEffectVerb::Delete => RetiredIdentityCause::Delete,
                                // A replacement on the same session owes
                                // nothing.
                                LifecycleEffectVerb::Reload => continue,
                            };
                            effects.push((
                                record.cursor,
                                identity,
                                Effect::Retirement(Retirement {
                                    member: member.clone(),
                                    generation: *generation,
                                    role: role.clone(),
                                    session: intent.session_id,
                                    cause,
                                    asked_at_ms: Some(intent.recorded_at_ms),
                                }),
                            ));
                        }
                        None => {
                            // A retirement no Meta verb asked for owes
                            // nothing, unless Core journaled it with its
                            // successor's seat: a rotation, which owes a
                            // reset's side effects when it moved the
                            // identity onto a fresh session.
                            if let Some(successor) =
                                rotation_successor(records, index, member, *generation)
                            {
                                pending.push(Pending::Rotation {
                                    cursor: record.cursor,
                                    identity,
                                    member: member.clone(),
                                    generation: *generation,
                                    role: role.clone(),
                                    successor,
                                });
                            }
                        }
                    }
                }
                MemberLifecycleFact::MobReset => {}
            }
        }
        if !pending.is_empty() {
            self.resolve_pending_lifecycle_facts(pending, &mut effects)
                .await?;
        }
        // One identity's side effects in journal order; different
        // identities' at most EFFECT_CONCURRENCY at once.
        effects.sort_by_key(|(cursor, _, _)| *cursor);
        let mut jobs: BTreeMap<AgentIdentity, Vec<(u64, Effect)>> = BTreeMap::new();
        for (cursor, identity, effect) in effects {
            if done.contains(&cursor) {
                continue;
            }
            jobs.entry(identity).or_default().push((cursor, effect));
        }
        let outcomes = stream::iter(jobs.into_iter().map(|(identity, effects)| {
            let customizer = customizer.clone();
            async move {
                let mut ran = Vec::with_capacity(effects.len());
                for (cursor, effect) in effects {
                    if let Err(error) = self
                        .run_lifecycle_effect(&identity, effect, customizer.as_ref())
                        .await
                    {
                        return (ran, Err(error));
                    }
                    ran.push(cursor);
                }
                (ran, Ok(()))
            }
        }))
        .buffer_unordered(EFFECT_CONCURRENCY)
        .collect::<Vec<_>>()
        .await;
        let mut failure = None;
        for (ran, outcome) in outcomes {
            done.extend(ran);
            if let Err(error) = outcome {
                failure.get_or_insert(error);
            }
        }
        match failure {
            Some(error) => Err(error),
            None => Ok(consumed),
        }
    }

    /// The side effects of the seats and the unrecorded rotations of a page,
    /// from one read of Core's member history.
    async fn resolve_pending_lifecycle_facts(
        &self,
        pending: Vec<Pending>,
        effects: &mut Vec<(u64, AgentIdentity, Effect)>,
    ) -> Result<(), String> {
        let Some(bridge) = self.bridge.as_ref() else {
            return Ok(());
        };
        let history = bridge
            .member_binding_history()
            .await
            .map_err(|error| format!("reading Core's member history: {error}"))?;
        let members = pending
            .iter()
            .map(|pending| match pending {
                Pending::Seat { member, .. } | Pending::Rotation { member, .. } => member.clone(),
            })
            .collect::<BTreeSet<_>>();
        // Each binding of the page's members by generation: the latest of
        // each session (a session a later incarnation resumed names that
        // one), and each member's latest.
        let mut bindings = BTreeMap::new();
        for binding in history
            .session_bindings_since_reset()
            .filter(|binding| members.contains(binding.identity()))
        {
            bindings.insert(
                (binding.identity().clone(), binding.generation().get()),
                binding.clone(),
            );
        }
        for member in &members {
            if let Some(binding) = history.last_binding(member) {
                bindings.insert(
                    (member.clone(), binding.generation().get()),
                    binding.clone(),
                );
            }
        }
        for pending in pending {
            match pending {
                Pending::Seat {
                    cursor,
                    identity,
                    member,
                    generation,
                } => {
                    // A seat no identity-first step made is a worker's, and an
                    // incarnation a later one replaced on its session is
                    // told no more: its successor's seat is.
                    let Some(binding) = bindings
                        .get(&(member, generation))
                        .filter(|binding| binding_spawned_for_identity(binding, &identity))
                    else {
                        continue;
                    };
                    // A peer-only member has no session to tell.
                    let Some(session) = binding.session_id().cloned() else {
                        continue;
                    };
                    effects.push((
                        cursor,
                        identity,
                        Effect::Told {
                            session,
                            role: binding.role().clone(),
                            labels: binding.labels().clone(),
                        },
                    ));
                }
                Pending::Rotation {
                    cursor,
                    identity,
                    member,
                    generation,
                    role,
                    successor,
                } => {
                    let retired = bindings.get(&(member.clone(), generation));
                    let seated = bindings.get(&(member.clone(), successor));
                    let (Some(retired), Some(seated)) = (retired, seated) else {
                        continue;
                    };
                    // A successor on the same session (a replacement) owes
                    // nothing, and a member no identity-first step made is a
                    // worker's.
                    let fresh = retired.session_id().is_some()
                        && seated.session_id().is_some()
                        && retired.session_id() != seated.session_id();
                    if !fresh || !binding_spawned_for_identity(retired, &identity) {
                        continue;
                    }
                    effects.push((
                        cursor,
                        identity,
                        Effect::Retirement(Retirement {
                            member,
                            generation,
                            role,
                            session: retired.session_id().cloned(),
                            cause: RetiredIdentityCause::Reset,
                            asked_at_ms: None,
                        }),
                    ));
                }
            }
        }
        Ok(())
    }

    /// The customizer `after_create` goes to: the identity context's, the
    /// one its roster passes build with, else the runtime's own.
    async fn after_create_customizer(&self) -> Option<Arc<dyn AgentCustomizer>> {
        let bound = lock(&self.lifecycle_projection.owner)
            .as_ref()
            .and_then(|owner| owner.customizer.clone());
        match bound {
            Some(customizer) => Some(customizer),
            None => self.customizer.read().await.clone(),
        }
    }

    /// Run one side effect. Only the exit interview's write fails the page:
    /// a distillation is best-effort and bounded, and an error of the
    /// retirement hook or of `after_create` is reported and stands, as an
    /// earlier call's would.
    async fn run_lifecycle_effect(
        &self,
        identity: &AgentIdentity,
        effect: Effect,
        customizer: Option<&Arc<dyn AgentCustomizer>>,
    ) -> Result<(), String> {
        match effect {
            Effect::Told {
                session,
                role,
                labels,
            } => {
                let Some(customizer) = customizer else {
                    return Ok(());
                };
                let context = match self.lifecycle_projection.take_created(identity) {
                    Some(context) => context,
                    None => self.derived_created_context(identity, &role, labels).await,
                };
                if let Err(error) = customizer.after_create(identity, &session, &context).await {
                    tracing::warn!(
                        %identity,
                        session_id = %session,
                        %error,
                        "the agent customizer's after_create failed; the build stands"
                    );
                }
                Ok(())
            }
            Effect::Retirement(retirement) => {
                self.run_lifecycle_retirement(identity, retirement).await
            }
        }
    }

    /// The memory side effects of one retirement, then its hook.
    async fn run_lifecycle_retirement(
        &self,
        identity: &AgentIdentity,
        retirement: Retirement,
    ) -> Result<(), String> {
        let memory = self.agent_memory.read().await.clone();
        let session_key = retirement.session.as_ref().map(ToString::to_string);
        let asked_at_ms = retirement.asked_at_ms.unwrap_or_else(unix_ms_now);
        if let Some(memory) = memory.as_ref() {
            match retirement.cause {
                RetiredIdentityCause::Unlisted => {
                    if let Some(session_key) = session_key.as_deref() {
                        memory
                            .distill_before_rotation(identity, session_key, DistillCause::Retire)
                            .await;
                    }
                    memory
                        .record_identity_retired_at(
                            identity,
                            session_key.as_deref(),
                            "retire",
                            asked_at_ms,
                        )
                        .await
                        .map_err(|error| {
                            format!("queueing the exit interview of {identity}: {error}")
                        })?;
                }
                RetiredIdentityCause::Reset => {
                    if let Some(session_key) = session_key.as_deref() {
                        // The marks come first: no distillate over the
                        // retired session lands before its reset boundary.
                        memory.note_reset_boundary(session_key);
                        memory.note_session_generation(
                            identity,
                            session_key,
                            retirement.generation,
                        );
                        memory
                            .distill_before_rotation(identity, session_key, DistillCause::Reset)
                            .await;
                    }
                }
                RetiredIdentityCause::Delete => {
                    // The delete distilled the session before Core was asked;
                    // the identity never resumes it, so its semantic-memory
                    // scope is dead weight.
                    if let Some(session_key) = session_key.as_deref() {
                        memory
                            .drop_orphaned_session_scope(session_key, DistillCause::Delete)
                            .await;
                    }
                    memory
                        .record_identity_retired_at(
                            identity,
                            session_key.as_deref(),
                            "delete",
                            asked_at_ms,
                        )
                        .await
                        .map_err(|error| {
                            format!("queueing the exit interview of {identity}: {error}")
                        })?;
                }
                // Releases are not journaled.
                RetiredIdentityCause::Retire | RetiredIdentityCause::Scoped => {}
            }
        }
        if self.retired_member_observer().is_none() {
            return Ok(());
        }
        let Ok(agent_runtime_id) = AgentRuntimeId::parse(
            &crate::member_comms_id::public_runtime_alias(&meerkat_mob::ids::AgentRuntimeId::new(
                retirement.member.clone(),
                meerkat_mob::Generation::new(retirement.generation),
            )),
        ) else {
            tracing::warn!(
                %identity,
                member = %retirement.member,
                generation = retirement.generation,
                "the retired incarnation has no runtime id; its retirement hook is not called"
            );
            return Ok(());
        };
        let registration = self
            .lifecycle_projection
            .take_retiring(identity, retirement.generation)
            .and_then(|kept| kept.registration);
        self.release_retired_members(vec![RetiredIdentityMember {
            identity: identity.clone(),
            member: retirement.member.as_str().to_string(),
            role: retirement.role,
            agent_runtime_id,
            generation: retirement.generation,
            session_id: retirement.session,
            registration,
            cause: retirement.cause,
        }])
        .await;
        Ok(())
    }

    /// What a seat's `after_create` reports when this process did not build
    /// it (a restart came between the seat and the call): the identity's
    /// roster spec, or Core's binding of an identity this runtime does not
    /// list, and no system prompt.
    async fn derived_created_context(
        &self,
        identity: &AgentIdentity,
        role: &meerkat_mob::ProfileName,
        labels: BTreeMap<String, String>,
    ) -> SessionCreatedContext {
        let spec = self
            .entries
            .read()
            .await
            .get(identity)
            .map(|entry| entry.spec.clone());
        match spec {
            Some(spec) => self.session_created_context(&spec, &Self::base_build_draft(&spec)),
            None => SessionCreatedContext {
                model: self.profile_model(role),
                labels,
                system_prompt: None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn retired(cursor: u64) -> MemberLifecycleFactRecord {
        MemberLifecycleFactRecord {
            cursor,
            fact: MemberLifecycleFact::Retired {
                member: meerkat_mob::AgentIdentity::from("alpha"),
                generation: 1,
                role: meerkat_mob::ProfileName::from("lead"),
            },
        }
    }

    fn seated(cursor: u64, generation: u64) -> MemberLifecycleFactRecord {
        MemberLifecycleFactRecord {
            cursor,
            fact: MemberLifecycleFact::Seated {
                member: meerkat_mob::AgentIdentity::from("alpha"),
                generation,
            },
        }
    }

    /// A retirement that ends a page which read as many events as it could
    /// waits for the next read, which starts right before it: the seat of
    /// its successor may follow in the same append.
    #[test]
    fn a_retirement_ending_a_full_page_waits_for_the_next_read() {
        let page = MemberLifecycleJournalPage {
            records: vec![seated(3, 0), retired(PAGE_LIMIT as u64)],
            next: PAGE_LIMIT as u64,
        };
        let (records, upto) = processable(&page, 0);
        assert_eq!(records, &page.records[..1]);
        assert_eq!(upto, PAGE_LIMIT as u64 - 1);
    }

    /// A page that read fewer events than its limit reached the journal's
    /// head: a retirement that ends it has no successor in the same append,
    /// and runs now.
    #[test]
    fn a_retirement_ending_the_journal_runs_now() {
        let page = MemberLifecycleJournalPage {
            records: vec![seated(3, 0), retired(9)],
            next: 9,
        };
        let (records, upto) = processable(&page, 0);
        assert_eq!(records, page.records.as_slice());
        assert_eq!(upto, 9);
    }

    /// A retirement is a rotation's only when its successor's seat is the
    /// very next event.
    #[test]
    fn only_the_next_event_s_seat_is_a_successor() {
        let member = meerkat_mob::AgentIdentity::from("alpha");
        let rotated = [retired(4), seated(5, 2)];
        assert_eq!(rotation_successor(&rotated, 0, &member, 1), Some(2));
        let later = [retired(4), seated(6, 2)];
        assert_eq!(rotation_successor(&later, 0, &member, 1), None);
        let older = [retired(4), seated(5, 1)];
        assert_eq!(rotation_successor(&older, 0, &member, 1), None);
        let last = [retired(4)];
        assert_eq!(rotation_successor(&last, 0, &member, 1), None);
    }
}
