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

use std::sync::Arc;

use meerkat_authorization::clock::LocalAuthorizationClock;
use meerkat_authorization::grant_policy::GrantBackedWorkPolicy;
use meerkat_authorization::grants::LocalGrantAuthority;
use meerkat_authorization::work::{FurtherOriginals, RetainedOriginal};
use meerkat_authorization_contracts::work_association::{
    InputAuthorityAssociation, OriginalWorkRef,
};
use meerkat_core::OperationAuthorizationError;
use meerkat_core::authorization::PreparedOperationCheck;
use meerkat_runtime::meerkat_machine::{
    AuthenticatedOriginalSource, SourceOriginalBasis, original_operation_grant,
    require_active_original,
};
use meerkat_workgraph::{WorkAdmission, WorkGraphAdmissionFence, WorkGraphError, WorkNamespace};

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
/// `create_idempotent_with_fence` call. It runs its check at most once; a
/// reused fence refuses without running the store operation, so
/// [`Self::refusal`] always describes this fence's own attempt.
pub struct GovernedHelperAdmissionFence {
    check: FenceCheck,
    realm_id: String,
    namespace: WorkNamespace,
    admission: WorkAdmission,
    attempted: std::sync::atomic::AtomicBool,
    refusal: std::sync::OnceLock<HelperAdmissionRefusal>,
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
    },
    /// A governed tool dispatch's own operation, checked by that dispatch's
    /// own retained work owners; nothing of the operation leaves core.
    Dispatch {
        operation: meerkat_core::GovernedDispatchOperation,
        further: FurtherOriginals,
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
    /// refuses as unavailable and the commit never runs.
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
        Arc::new(Self {
            check,
            realm_id,
            namespace,
            admission,
            attempted: std::sync::atomic::AtomicBool::new(false),
            refusal: std::sync::OnceLock::new(),
        })
    }

    /// The typed reason this fence's one attempt refused, if it did.
    pub fn refusal(&self) -> Option<HelperAdmissionRefusal> {
        self.refusal.get().copied()
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
        if self
            .attempted
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            // Not this fence's attempt: its recorded outcome stays its own.
            return Err(WorkGraphError::Conflict(
                "a governed helper admission fence serves one attempt".into(),
            ));
        }
        if realm_id != self.realm_id || namespace != &self.namespace || admission != &self.admission
        {
            return Err(self.refuse(HelperAdmissionRefusal::Denied));
        }
        // One reservation through the store callback: owner changes stay
        // excluded through the commit or the replay disclosure, and the
        // operation's own result is returned unchanged.
        match &self.check {
            FenceCheck::Prepared {
                grants,
                originals,
                source,
                complete_contributors,
                setup_operation,
                operation_policy,
                clock,
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
                    Err(_) => Err(self.refuse(HelperAdmissionRefusal::Unavailable)),
                }
            }
            FenceCheck::Dispatch {
                operation: governed,
                further,
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
