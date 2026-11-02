//! Native WorkGraph admission fence for governed helper work.
//!
//! A host that admits helper work for a child first records the helper's
//! WorkGraph item with
//! [`meerkat_workgraph::WorkGraphService::create_idempotent_with_fence`]
//! under [`GovernedHelperAdmissionFence`]. Inside one non-waiting exclusion
//! of every owner change on the grant publication, held through the store's
//! commit or replay disclosure, the fence checks the exact admission it was
//! bound to, then the actual prepared setup operation: currently, under the
//! installed native policy and clock, with the operation's own grant
//! selection conjoined for the child's original and every relevant original
//! it depends on.
//!
//! The item launches nothing. The helper itself is admitted only through
//! native governed admission: its own original, recorded by the source
//! owner, bound with [`meerkat_runtime::meerkat_machine::bind_scoped_original`]
//! and naming the complete relevant set (the child's original, the originals
//! the child names, and every further retained original the host carries).
//! Native contributor checks read each named original directly (custody is
//! not recursive), so the helper must name every original it depends on.
//! Each original's grant is selected for the setup operation's own target,
//! targeted lineages included ([`original_operation_grant`]); a host that
//! retains further originals must supply them all as
//! `complete_contributors`.
//!
//! The fence runs inside the store's write boundary and never waits. A
//! refusal it meets because another writer holds the publication its check
//! excludes owner changes on (a reservation of the writer that has not
//! published) or because an owner change is in progress there is
//! contention: nothing ran and nothing was committed, and it is retryable at
//! the host level.
//! [`GovernedHelperAdmissionFence::create_idempotent_after_owner_change`]
//! makes the keyed create through a new fence for each attempt and waits for
//! the holder outside every lock between attempts. On the dispatch paths it
//! retries every refusal to rebuild, as Core's waiting step forms do.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use meerkat_authorization::clock::LocalAuthorizationClock;
use meerkat_authorization::grant_policy::GrantBackedWorkPolicy;
use meerkat_authorization::grants::LocalGrantAuthority;
use meerkat_authorization::work::{FurtherOriginals, RetainedOriginal};
use meerkat_authorization_contracts::work_association::{
    InputAuthorityAssociation, OriginalWorkRef,
};
use meerkat_core::OperationAuthorizationError;
use meerkat_core::authorization::{
    OperationRefusalKind, OwnerChangeChurn, OwnerChangeInProgress, PreparedOperationCheck,
    WorkAuthorization,
};
use meerkat_runtime::meerkat_machine::{
    AuthenticatedOriginalSource, SourceOriginalBasis, original_operation_grant,
    require_active_original,
};
use meerkat_workgraph::{
    CreateWorkItemRequest, WorkAdmission, WorkAdmissionKey, WorkAdmissionOutcome,
    WorkGraphAdmissionFence, WorkGraphError, WorkGraphService, WorkNamespace,
};

/// Why a governed helper admission was refused. WorkGraph reports every
/// refusal as `Conflict`; this keeps the native distinction for the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HelperAdmissionRefusal {
    /// Not permitted now: another admission than the one the fence was bound
    /// to, the setup operation's current decision refused it or requires a
    /// review this entry cannot carry (for an entered step, more review than
    /// the dispatch entered), the admitting dispatch of an entered step
    /// ended, or a relevant original is cancelled, no longer retained or
    /// does not allow the operation. A refusal by a dispatch's review is
    /// final for that dispatch only: a new call is reviewed anew. For an
    /// entered step it speaks only for this admission: earlier steps keep
    /// their effects.
    Denied,
    /// The owners cannot say now (contention, a refusal to rebuild after an
    /// owner change, clock or custody unavailable). The store operation did
    /// not run; nothing was committed or disclosed.
    ///
    /// Contention is retryable at the host level: another writer held the
    /// publication the check excludes owner changes on (a reservation of
    /// its writer that had not published), or an owner change was in
    /// progress there, when the fence refused. Retry it with a new fence and
    /// the same idempotent create after a bounded wait taken outside every
    /// lock and store transaction
    /// ([`OwnerChangeChurn::wait_after_stale_read`]), as
    /// [`GovernedHelperAdmissionFence::create_idempotent_after_owner_change`]
    /// does. That form also retries a refusal to rebuild on the dispatch
    /// paths, as Core's waiting step forms do, and answers the other causes
    /// at once: a refusal to rebuild inside the prepared path's own
    /// exclusion, clock or custody unavailable, a poisoned publication. A
    /// later attempt may still succeed once the owners can answer.
    Unavailable,
}

/// The originals helper work for `source` must name as its
/// `contributing_work`: the source's own original, every original the source
/// names, then each of `retained` held under the source's own ingress,
/// without duplicates. Native admission reads a helper's contributors under
/// the helper's own ingress namespace, so a retained original under another
/// ingress cannot be named there; the fence still conjoins it.
pub fn helper_contributing_work(
    source: &InputAuthorityAssociation,
    retained: &[RetainedOriginal],
) -> Vec<OriginalWorkRef> {
    let claims = source.candidate();
    let mut work = vec![claims.original_work.clone()];
    let same_ingress = retained
        .iter()
        .filter(|retained| {
            retained.namespace.realm == claims.ingress_namespace.realm
                && retained.namespace.ingress == claims.ingress_namespace.ingress
        })
        .map(|retained| &retained.original);
    for original in claims.contributing_work.iter().chain(same_ingress) {
        if !work.contains(original) {
            work.push(original.clone());
        }
    }
    work
}

/// The originals `source` names (its contributors, in its namespace) and
/// `retained`, each under its own ingress, without duplicates and without
/// the source's own original.
pub(crate) fn named_originals(
    source: &InputAuthorityAssociation,
    retained: &[RetainedOriginal],
) -> Vec<RetainedOriginal> {
    let claims = source.candidate();
    let own = RetainedOriginal {
        namespace: claims.ingress_namespace.clone(),
        original: claims.original_work.clone(),
    };
    let mut named: Vec<RetainedOriginal> = Vec::new();
    let contributors = claims
        .contributing_work
        .iter()
        .map(|original| RetainedOriginal {
            namespace: claims.ingress_namespace.clone(),
            original: original.clone(),
        });
    for original in contributors.chain(retained.iter().cloned()) {
        if !own.same_original(&original)
            && !named.iter().any(|known| known.same_original(&original))
        {
            named.push(original);
        }
    }
    named
}

/// Fence for one keyed WorkGraph admission attempt: one
/// `create_idempotent_with_fence` call, or one waiting admission through
/// [`Self::create_idempotent_after_owner_change`], whose every attempt is a
/// new fence with this one's check, scope and admission. It runs its check
/// at most once; a reused fence refuses without running the store
/// operation, so [`Self::refusal`] always describes this fence's own
/// attempt, or the attempt its waiting admission answered with.
pub struct GovernedHelperAdmissionFence {
    /// Shared by every fence of one waiting admission.
    check: Arc<FenceCheck>,
    realm_id: String,
    namespace: WorkNamespace,
    admission: WorkAdmission,
    attempted: AtomicBool,
    refusal: OnceLock<HelperAdmissionRefusal>,
    /// Whether this fence's attempt refused in a way its waiting admission
    /// waits out before it tries again: on the dispatch paths a refusal to
    /// rebuild (`ReprepareRequired`), as Core's waiting step forms treat it;
    /// on the prepared path an exclusion its fence could not take while the
    /// publication named an owner change in progress or another writer's
    /// hold, read as it refused.
    rebuild_refused: AtomicBool,
    /// Test seam: the fences a waiting admission through this fence made, in
    /// order.
    #[cfg(test)]
    attempts: std::sync::Mutex<Vec<Arc<Self>>>,
}

/// How a fence checks its setup operation under one exclusion held through
/// the store commit.
enum FenceCheck {
    /// A prepared check the caller holds, with the native owners that
    /// prepared it.
    Prepared {
        grants: Arc<LocalGrantAuthority>,
        originals: Arc<dyn AuthenticatedOriginalSource>,
        source: InputAuthorityAssociation,
        complete_contributors: Arc<[RetainedOriginal]>,
        setup_operation: PreparedOperationCheck,
        operation_policy: Arc<GrantBackedWorkPolicy>,
        clock: Arc<dyn LocalAuthorizationClock>,
        /// The grant owner's own work owner over its publication, built on
        /// first use, for its waiting hint only ([`FenceCheck::waiting_work`]).
        waiting: OnceLock<Option<meerkat_core::WorkAuthorizationContext>>,
    },
    /// A governed tool dispatch's own operation, checked by that dispatch's
    /// own retained work owners; nothing of the operation leaves core.
    Dispatch {
        operation: meerkat_core::GovernedDispatchOperation,
        further: FurtherOriginals,
        /// The dispatch context's own waiting state
        /// (`ToolDispatchContext::owner_change_churn`), taken by the one
        /// waiting admission of the fence, so that its waits report to the
        /// host's operation refusal observer on the call's surface.
        churn: std::sync::Mutex<Option<OwnerChangeChurn>>,
    },
    /// A further step of a governed tool dispatch's entered operation,
    /// checked by that dispatch's own retained work owners within the review
    /// it entered; it spends nothing and runs no review.
    Entered {
        operation: meerkat_core::EnteredDispatchOperation,
        further: FurtherOriginals,
    },
}

impl GovernedHelperAdmissionFence {
    /// The fence for exactly one admission.
    ///
    /// - `realm_id`, `namespace` and `admission` (key and request digest,
    ///   from `WorkGraphService::admission_identity`) are the scope and
    ///   identity it admits; any other is refused.
    /// - `source` is the child's actual accepted association;
    ///   `complete_contributors` the further originals the host retains for
    ///   it. Each is read from the source owner's current custody, never
    ///   from the association.
    /// - `setup_operation` is the actual prepared check of the setup
    ///   operation whose request is this admission; the setup owner binds
    ///   them. `operation_policy`, `grants`, `originals` and `clock` are the
    ///   same trusted native composition that prepared it.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        grants: Arc<LocalGrantAuthority>,
        originals: Arc<dyn AuthenticatedOriginalSource>,
        source: InputAuthorityAssociation,
        complete_contributors: Arc<[RetainedOriginal]>,
        setup_operation: PreparedOperationCheck,
        operation_policy: Arc<GrantBackedWorkPolicy>,
        clock: Arc<dyn LocalAuthorizationClock>,
        realm_id: impl Into<String>,
        namespace: WorkNamespace,
        admission: WorkAdmission,
    ) -> Arc<Self> {
        Self::with_check(
            FenceCheck::Prepared {
                grants,
                originals,
                source,
                complete_contributors,
                setup_operation,
                operation_policy,
                clock,
                waiting: OnceLock::new(),
            },
            realm_id.into(),
            namespace,
            admission,
        )
    }

    /// The fence for one admission of helper work started by a governed tool
    /// dispatch. The setup operation is the dispatch's own prepared
    /// operation, matched against the actual borrowed `call`; at commit it
    /// is checked by that dispatch's own work owners (its run custody, its
    /// retained grant-backed policy for every work association, its grant
    /// owner and clock), with every original of the work and every one of
    /// `complete_contributors` conjoined by its own grant for the
    /// operation's target, under those owners' exclusion held through the
    /// commit. The caller supplies only the source owner, the further
    /// retained originals (each under its own ingress) and the exact
    /// admission. The owner binding is by construction: the check runs on
    /// the dispatch context's own authorization chain, never on an owner
    /// looked up or supplied separately. A `WorkAuthorization` that does not
    /// implement `run_if_current_within` (the generic composition boundary)
    /// refuses as unavailable and the commit never runs. The fence keeps the
    /// dispatch context's own waiting state
    /// ([`meerkat_core::ToolDispatchContext::owner_change_churn`]) for its
    /// waiting admission ([`Self::create_idempotent_after_owner_change`]),
    /// whose waits then report on the call's surface, without an operation
    /// id.
    ///
    /// # Errors
    /// An ungoverned or default context, or another call, refuses.
    pub fn from_dispatch(
        call: meerkat_core::ToolCallView<'_>,
        context: &meerkat_core::ToolDispatchContext,
        originals: Arc<dyn AuthenticatedOriginalSource>,
        complete_contributors: Arc<[RetainedOriginal]>,
        realm_id: impl Into<String>,
        namespace: WorkNamespace,
        admission: WorkAdmission,
    ) -> Result<Arc<Self>, HelperAdmissionRefusal> {
        let operation = context
            .governed_operation(call)
            .map_err(|_| HelperAdmissionRefusal::Denied)?;
        Ok(Self::with_check(
            FenceCheck::Dispatch {
                operation,
                further: FurtherOriginals {
                    originals: complete_contributors,
                    source: Arc::new(SourceOriginalBasis::new(originals)),
                },
                churn: std::sync::Mutex::new(Some(context.owner_change_churn())),
            },
            realm_id.into(),
            namespace,
            admission,
        ))
    }

    /// The fence for one admission of helper work as a further step of a
    /// governed tool dispatch's entered operation
    /// ([`meerkat_core::EnteredDispatchOperation`]): for a reviewed (R2 or
    /// R3) call that spent its review at its first effect, or any call that
    /// entered through the witness. At commit the step is checked by that
    /// dispatch's own work owners as [`Self::from_dispatch`] describes, with
    /// every original of the work and every one of `complete_contributors`
    /// conjoined, under those owners' exclusion held through the commit,
    /// except that the current decision may require the review the dispatch
    /// entered, and no more. Nothing is spent, no review runs and no consent
    /// is requested under the graph lock; the review owner admits the step
    /// only while the admitting dispatch is live. One fence serves one
    /// attempt, and the witness serves every step of the call.
    pub fn from_entered(
        entered: &meerkat_core::EnteredDispatchOperation,
        originals: Arc<dyn AuthenticatedOriginalSource>,
        complete_contributors: Arc<[RetainedOriginal]>,
        realm_id: impl Into<String>,
        namespace: WorkNamespace,
        admission: WorkAdmission,
    ) -> Arc<Self> {
        Self::with_check(
            FenceCheck::Entered {
                operation: entered.clone(),
                further: FurtherOriginals {
                    originals: complete_contributors,
                    source: Arc::new(SourceOriginalBasis::new(originals)),
                },
            },
            realm_id.into(),
            namespace,
            admission,
        )
    }

    fn with_check(
        check: FenceCheck,
        realm_id: String,
        namespace: WorkNamespace,
        admission: WorkAdmission,
    ) -> Arc<Self> {
        Self::unattempted(Arc::new(check), realm_id, namespace, admission)
    }

    /// A fence that has not served its attempt yet.
    fn unattempted(
        check: Arc<FenceCheck>,
        realm_id: String,
        namespace: WorkNamespace,
        admission: WorkAdmission,
    ) -> Arc<Self> {
        Arc::new(Self {
            check,
            realm_id,
            namespace,
            admission,
            attempted: AtomicBool::new(false),
            refusal: OnceLock::new(),
            rebuild_refused: AtomicBool::new(false),
            #[cfg(test)]
            attempts: std::sync::Mutex::new(Vec::new()),
        })
    }

    /// A new fence for one more attempt of this fence's admission: the same
    /// check, scope and admission, not yet attempted.
    fn renewed(&self) -> Arc<Self> {
        Self::unattempted(
            Arc::clone(&self.check),
            self.realm_id.clone(),
            self.namespace.clone(),
            self.admission.clone(),
        )
    }

    /// The typed reason this fence's one attempt refused, if it did; after a
    /// waiting admission ([`Self::create_idempotent_after_owner_change`]),
    /// the reason the attempt it answered with refused.
    pub fn refusal(&self) -> Option<HelperAdmissionRefusal> {
        self.refusal.get().copied()
    }

    /// Make the keyed create this fence admits, waiting out contention.
    ///
    /// The fence runs inside the store's write boundary and never waits, so
    /// a refusal it meets while another writer holds the publication its
    /// check excludes owner changes on (a reservation of the writer that has
    /// not published), or while an owner change is in progress there, is no
    /// verdict: nothing ran and nothing was committed. This is the
    /// host-level retry of that contention. Each attempt is a new fence with
    /// this fence's check, scope and admission (a fence serves one attempt)
    /// and the same idempotent `create_idempotent_with_fence(admission_key,
    /// request)` on `service`, so the one attempt the owners admit creates
    /// the item, or replays it.
    ///
    /// After an attempt refused as [`HelperAdmissionRefusal::Unavailable`]
    /// for one of these, it waits outside every lock and store transaction,
    /// then tries again:
    /// - on the prepared path ([`Self::new`]), an exclusion its fence could
    ///   not take while the publication named an owner change in progress
    ///   or another writer's hold, read as it refused;
    /// - on the dispatch paths ([`Self::from_dispatch`],
    ///   [`Self::from_entered`]), a refusal to rebuild (`ReprepareRequired`),
    ///   which the native work owner also answers while another actor holds
    ///   its exclusion. As Core's waiting step forms do, it waits for a
    ///   change or hold in progress, or, with none named (the holder may
    ///   have finished in between), pauses one backoff step (1 ms).
    ///
    /// Every wait is [`OwnerChangeChurn::wait_after_stale_read`] of this
    /// call's one [`OwnerChangeChurn`]: one deadline,
    /// [`meerkat_core::authorization::OWNER_CHANGE_WAIT_BOUND`] from its
    /// first wait, clamped to `budget_remaining` (in a tool body,
    /// `ToolDispatchContext::remaining_tool_time`), and each settled wait is
    /// one of its [`meerkat_core::authorization::OWNER_CHANGE_FREE_REBUILDS`]
    /// retries. Read `budget_remaining` immediately before the call: the
    /// deadline is armed at the first wait, after the first attempt, so it
    /// can end later than the caller's own deadline by that attempt's time.
    /// For a fence built from a dispatch the churn is that dispatch
    /// context's own ([`meerkat_core::ToolDispatchContext::owner_change_churn`]),
    /// so each wait is also reported to the host's operation refusal
    /// observer on the call's surface, without an operation id (the
    /// dispatch's prepared operation is not public); the prepared and
    /// entered fences have no such context, and their waits are traced only.
    ///
    /// Await it holding no lock, guard or store transaction of your own (no
    /// owner or host lock): its waits run inside this call, and the wait's
    /// contract ([`OwnerChangeChurn::wait_after_stale_read`]) holds its
    /// caller to that. A host that must keep a lock of its own makes its
    /// single attempt under the lock and retries outside it.
    ///
    /// It answers at once with an attempt's own answer on:
    /// - success: the store's outcome for the key (`Created`, `Replayed`, or
    ///   `Conflict` for another request under it);
    /// - [`HelperAdmissionRefusal::Denied`];
    /// - any other `Unavailable`: a refusal to rebuild inside the prepared
    ///   path's own exclusion, clock or custody unavailable, owners that
    ///   cannot answer now, or a poisoned publication;
    /// - any other store error.
    ///
    /// Once a wait answers
    /// [`meerkat_core::authorization::OwnerChangeWait::NotSettled`] (the
    /// deadline passed, or every free rebuild is spent) it returns the last
    /// attempt's `Unavailable` without another attempt. Then, as after every
    /// answer, [`Self::refusal`] reads the answered attempt's typed refusal.
    ///
    /// This fence serves this one waiting admission, and its own check never
    /// runs: called on a fence already used, directly or here, it refuses at
    /// once, as a reused fence does, and runs nothing.
    ///
    /// Cancellation is safe: dropping the future, during a wait or between
    /// attempts, commits nothing beyond what an attempt that already reached
    /// the store commits itself, which the same keyed create then replays,
    /// and the fence stays used. Each wait is traced at site
    /// `helper_admission` ([`meerkat_core::authorization::OwnerChangeWait::trace`]),
    /// with the setup operation's id on the prepared path. Shell
    /// synchronization only: every attempt is decided afresh by the owners,
    /// and nothing here decides.
    ///
    /// # Errors
    /// The answered attempt's error as `create_idempotent_with_fence`
    /// returns it: `WorkGraphError::Conflict` for a refusal, typed by
    /// [`Self::refusal`].
    pub async fn create_idempotent_after_owner_change(
        &self,
        service: &WorkGraphService,
        admission_key: WorkAdmissionKey,
        request: CreateWorkItemRequest,
        budget_remaining: Option<std::time::Duration>,
    ) -> Result<WorkAdmissionOutcome, WorkGraphError> {
        if self.attempted.swap(true, Ordering::AcqRel) {
            return Err(serves_one_attempt());
        }
        // Read before the first attempt, so that a refusal reads the hint at
        // once.
        let waiting = self.check.waiting_work();
        let operation = self.check.operation_id();
        let mut churn = self.check.take_churn();
        loop {
            let attempt = self.renewed();
            #[cfg(test)]
            self.record_attempt(&attempt);
            let answer = service
                .create_idempotent_with_fence(
                    admission_key.clone(),
                    request.clone(),
                    attempt.clone(),
                )
                .await;
            // Only an attempt that refused for what is waited out waits: it
            // ran and committed nothing, and nothing is held across the wait.
            if answer.is_err()
                && attempt.rebuild_refused.load(Ordering::Acquire)
                && let Some(work) = waiting
                && churn
                    .wait_after_stale_read(work, budget_remaining, ADMISSION_WAIT_SITE, operation)
                    .await
                    .settled()
            {
                continue;
            }
            if let Some(refusal) = attempt.refusal() {
                let _ = self.refusal.set(refusal);
            }
            return answer;
        }
    }

    /// Test seam: record `attempt` as one of this waiting admission's fences.
    #[cfg(test)]
    fn record_attempt(&self, attempt: &Arc<Self>) {
        self.attempts
            .lock()
            .expect("the attempt record")
            .push(Arc::clone(attempt));
    }

    /// Test seam: the fences a waiting admission through this fence made, in
    /// order.
    #[cfg(test)]
    pub(crate) fn attempts(&self) -> Vec<Arc<Self>> {
        self.attempts.lock().expect("the attempt record").clone()
    }

    /// Test seam: whether this fence served its one attempt.
    #[cfg(test)]
    pub(crate) fn was_attempted(&self) -> bool {
        self.attempted.load(Ordering::Acquire)
    }

    /// Run the store `operation` at most once inside `run` (a governed
    /// dispatch's own check, held through it) and return its result
    /// unchanged; a refusal met before it ran is typed by `refusal`.
    fn commit_within(
        &self,
        run: impl FnOnce(&mut dyn FnMut()) -> Result<(), meerkat_core::ToolError>,
        operation: Box<dyn FnOnce() -> Result<(), WorkGraphError> + '_>,
        refusal: fn(&meerkat_core::ToolError) -> HelperAdmissionRefusal,
    ) -> Result<(), WorkGraphError> {
        let mut pending = Some(operation);
        let mut result = None;
        let checked = run(&mut || {
            if let Some(operation) = pending.take() {
                result = Some(operation());
            }
        });
        match (checked, result) {
            (_, Some(result)) => result,
            // A refusal to rebuild, which the work owner also answers while
            // another actor holds its exclusion.
            (Err(error), None) if refused_to_rebuild(&error) => Err(self.refuse_rebuild()),
            (Err(error), None) => Err(self.refuse(refusal(&error))),
            (Ok(()), None) => Err(self.refuse(HelperAdmissionRefusal::Unavailable)),
        }
    }

    fn refuse(&self, refusal: HelperAdmissionRefusal) -> WorkGraphError {
        let _ = self.refusal.set(refusal);
        WorkGraphError::Conflict(match refusal {
            HelperAdmissionRefusal::Denied => "governed helper admission is not permitted".into(),
            HelperAdmissionRefusal::Unavailable => {
                "governed helper admission authority is unavailable now".into()
            }
        })
    }

    /// [`Self::refuse`] as unavailable, for the prepared path's exclusion
    /// refused: another holder of the writer, or a publication that can no
    /// longer answer. When the publication names an owner change in progress
    /// or a hold of its writer, read at once after the refusal, the attempt
    /// met contention and its waiting admission waits for it; a poisoned
    /// publication names nothing.
    fn refuse_exclusion(&self) -> WorkGraphError {
        if self.check.owner_change_in_progress().is_some() {
            self.rebuild_refused.store(true, Ordering::Release);
        }
        self.refuse(HelperAdmissionRefusal::Unavailable)
    }

    /// [`Self::refuse`] as unavailable, for a dispatch-path step that refused
    /// to rebuild: its waiting admission waits it out as Core's waiting step
    /// forms do, for a change or hold in progress, or one backoff step when
    /// none is named.
    fn refuse_rebuild(&self) -> WorkGraphError {
        self.rebuild_refused.store(true, Ordering::Release);
        self.refuse(HelperAdmissionRefusal::Unavailable)
    }
}

/// The trace site of a waiting admission's waits.
const ADMISSION_WAIT_SITE: &str = "helper_admission";

// The waiting admission's future is `Send`, so a host can spawn it, and a
// tool body (an `async_trait` future) can await it.
const _: fn(
    &GovernedHelperAdmissionFence,
    &WorkGraphService,
    WorkAdmissionKey,
    CreateWorkItemRequest,
) = |fence, service, key, request| {
    fn send<T: Send>(_: T) {}
    send(fence.create_idempotent_after_owner_change(service, key, request, None));
};

impl FenceCheck {
    /// The work owner of the publication this check excludes owner changes
    /// on, for its waiting hint only: its `owner_change_in_progress` names
    /// an owner change in progress there, or another actor's hold of its
    /// writer, published or not. Nothing here decides from it. On the
    /// prepared path it is the grant owner's own work owner over that
    /// publication (the operation policy's, which shares the grant owner's
    /// publication), built once on first use; on the dispatch paths it is
    /// the dispatch's own work.
    fn waiting_work(&self) -> Option<&dyn WorkAuthorization> {
        let work = match self {
            Self::Prepared {
                waiting,
                operation_policy,
                source,
                setup_operation,
                ..
            } => waiting
                .get_or_init(|| {
                    operation_policy
                        .work_context(
                            Arc::from([source.clone()]),
                            setup_operation.binding().facts().execution_scope.clone(),
                        )
                        .ok()
                })
                .as_ref()?,
            Self::Dispatch { operation, .. } => operation.work_authorization(),
            Self::Entered { operation, .. } => operation.work_authorization(),
        };
        Some(work.authorization().as_ref())
    }

    /// The owner change or hold in progress on that publication now.
    fn owner_change_in_progress(&self) -> Option<OwnerChangeInProgress> {
        self.waiting_work()?.owner_change_in_progress()
    }

    /// The waiting state of the one waiting admission of this check's fence:
    /// on the dispatch path the dispatch context's own, which reports each
    /// wait as the call's; on the prepared and entered paths a new one,
    /// whose waits are traced only.
    fn take_churn(&self) -> OwnerChangeChurn {
        match self {
            Self::Dispatch { churn, .. } => churn
                .lock()
                .ok()
                .and_then(|mut kept| kept.take())
                .unwrap_or_default(),
            Self::Prepared { .. } | Self::Entered { .. } => OwnerChangeChurn::new(),
        }
    }

    /// The operation a waiting admission's waits are traced for: the setup
    /// operation, on the prepared path.
    fn operation_id(&self) -> Option<&meerkat_core::OperationId> {
        match self {
            Self::Prepared {
                setup_operation, ..
            } => Some(&setup_operation.binding().facts().operation_id),
            Self::Dispatch { .. } | Self::Entered { .. } => None,
        }
    }
}

/// The prepared setup operation is currently permitted for the source and
/// every relevant original. Runs inside the exclusion; synchronous.
#[allow(clippy::too_many_arguments)]
fn check_prepared(
    grants: &LocalGrantAuthority,
    originals: &dyn AuthenticatedOriginalSource,
    source: &InputAuthorityAssociation,
    complete_contributors: &[RetainedOriginal],
    setup_operation: &PreparedOperationCheck,
    operation_policy: &GrantBackedWorkPolicy,
    clock: &dyn LocalAuthorizationClock,
) -> Result<(), HelperAdmissionRefusal> {
    let check = setup_operation.current().map_err(refusal_of)?;
    // No review can run under the graph lock: an operation whose current
    // decision requires one refuses here.
    check
        .require_unreviewed_entry()
        .map_err(|_| HelperAdmissionRefusal::Denied)?;
    let now_ms = clock
        .now()
        .map_err(|_| HelperAdmissionRefusal::Unavailable)?
        .unix_ms;
    let claims = source.candidate();
    // The source's own original: its mandate is the association's own, so
    // only its disposition is read.
    require_active_original(originals, &claims.ingress_namespace, &claims.original_work)
        .map_err(refusal_of)?;
    let mut further = Vec::new();
    for retained in named_originals(source, complete_contributors) {
        if let Some(grant) = original_operation_grant(
            grants,
            originals,
            &retained.namespace,
            &retained.original,
            check.binding(),
            &claims.logical_executor,
            claims.represented_subject.as_ref(),
        )
        .map_err(refusal_of)?
        {
            further.push(grant);
        }
    }
    operation_policy
        .evaluate_operation_within(source, check.binding(), further.iter(), now_ms)
        .map(|_| ())
        .map_err(refusal_of)
}

/// The answer of a fence asked to serve another attempt than its one: it
/// runs nothing, and its recorded outcome stays its own.
fn serves_one_attempt() -> WorkGraphError {
    WorkGraphError::Conflict("a governed helper admission fence serves one attempt".into())
}

/// Whether a dispatch-path step refused to rebuild (`ReprepareRequired`):
/// the native work owner's answer while another actor holds its exclusion,
/// and the one step refusal a waiting admission retries, as Core's waiting
/// step forms do (`refuses_to_rebuild` there). Both dispatch paths answer it
/// as unavailable.
fn refused_to_rebuild(error: &meerkat_core::ToolError) -> bool {
    matches!(
        error,
        meerkat_core::ToolError::AuthorizationRefused { refusal }
            if refusal.kind() == OperationRefusalKind::ReprepareRequired
    )
}

fn refusal_of(error: OperationAuthorizationError) -> HelperAdmissionRefusal {
    match error {
        OperationAuthorizationError::Refused(refusal) if super::effect_leaf_denies(refusal) => {
            HelperAdmissionRefusal::Denied
        }
        OperationAuthorizationError::Refused(_)
        | OperationAuthorizationError::Unavailable
        | OperationAuthorizationError::ObservationUnavailable(_) => {
            HelperAdmissionRefusal::Unavailable
        }
    }
}

/// A dispatch-path refusal: a denial, as on the prepared path, for a refused
/// operation and for a review settlement final for the dispatch (a review
/// this entry cannot carry: an R2 or R3 decision refuses here, since no
/// review runs under the graph lock); otherwise unavailable.
fn tool_refusal_of(error: &meerkat_core::ToolError) -> HelperAdmissionRefusal {
    match error {
        meerkat_core::ToolError::AuthorizationRefused { refusal, .. }
            if super::effect_leaf_denies(*refusal) =>
        {
            HelperAdmissionRefusal::Denied
        }
        error if super::review_settlement_denies(error) => HelperAdmissionRefusal::Denied,
        _ => HelperAdmissionRefusal::Unavailable,
    }
}

/// An entered step's refusal: a denial for a refused step (an ended
/// dispatch, a source cancelled, revoked or no longer retained, a decision
/// needing more review than was entered, an expired window, malformed
/// facts); otherwise unavailable (contention, a rebuild request, owners that
/// cannot answer now). It speaks only for this step.
fn entered_refusal_of(error: &meerkat_core::ToolError) -> HelperAdmissionRefusal {
    match error {
        meerkat_core::ToolError::AuthorizationRefused { refusal, .. }
            if super::effect_leaf_denies(*refusal) =>
        {
            HelperAdmissionRefusal::Denied
        }
        _ => HelperAdmissionRefusal::Unavailable,
    }
}

impl WorkGraphAdmissionFence for GovernedHelperAdmissionFence {
    fn execute_if_current(
        &self,
        realm_id: &str,
        namespace: &WorkNamespace,
        admission: &WorkAdmission,
        operation: Box<dyn FnOnce() -> Result<(), WorkGraphError> + '_>,
    ) -> Result<(), WorkGraphError> {
        if self.attempted.swap(true, Ordering::AcqRel) {
            // Not this fence's attempt: its recorded outcome stays its own.
            return Err(serves_one_attempt());
        }
        if realm_id != self.realm_id || namespace != &self.namespace || admission != &self.admission
        {
            return Err(self.refuse(HelperAdmissionRefusal::Denied));
        }
        // One reservation through the store callback: owner changes stay
        // excluded through the commit or the replay disclosure, and the
        // operation's own result is returned unchanged.
        match self.check.as_ref() {
            FenceCheck::Prepared {
                grants,
                originals,
                source,
                complete_contributors,
                setup_operation,
                operation_policy,
                clock,
                waiting: _,
            } => {
                let checked = grants.try_with_owner_changes_excluded(|| {
                    check_prepared(
                        grants,
                        originals.as_ref(),
                        source,
                        complete_contributors,
                        setup_operation,
                        operation_policy,
                        clock.as_ref(),
                    )
                    .map(|()| operation())
                });
                match checked {
                    Ok(Ok(result)) => result,
                    Ok(Err(refusal)) => Err(self.refuse(refusal)),
                    // Another reservation holds the writer, or the
                    // publication can no longer answer: contention only
                    // while it names a change or a hold in progress.
                    Err(_) => Err(self.refuse_exclusion()),
                }
            }
            FenceCheck::Dispatch {
                operation: governed,
                further,
                churn: _,
            } => self.commit_within(
                |commit| governed.run_if_current_within(further, commit),
                operation,
                tool_refusal_of,
            ),
            FenceCheck::Entered {
                operation: entered,
                further,
            } => self.commit_within(
                |commit| entered.run_if_current_within(further, commit),
                operation,
                entered_refusal_of,
            ),
        }
    }
}
