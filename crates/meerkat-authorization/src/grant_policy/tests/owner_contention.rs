//! Another actor holding the work's publication writer, at the native work
//! owner's operation-time steps. Each step takes the writer without waiting:
//! a held writer is a refusal to rebuild (no verdict on the operation, and
//! nothing appended), named as an owner change in progress until it ends, so
//! the caller can wait for it outside every lock. A poisoned publication
//! stays unavailable, and the host-facing entries that never wait keep
//! answering unavailable for the same hold.
//!
//! A holds the writer on its own thread: variant 1 a held exclusion that
//! never publishes (`try_with_owner_changes_excluded`), variant 2 a published
//! owner change (`begin_owner_change`). Channels order every step.

use std::sync::mpsc;

use super::*;
use crate::grants::{ChildGrantIntent, ChildGrantOutcome, ChildGrantRefusal, GrantRefusal};
use crate::publication::PublicationError;
use crate::work::{FurtherOriginals, OriginalBasisSource};
use meerkat_authorization_contracts::grant_mutation::{
    ControllerCustodyRefusal, ControllerGrantMutationCustody,
};
use meerkat_authorization_contracts::work_association::WorkAuthorityBasis;
use meerkat_core::OperationAuthorizationError;
use meerkat_core::authorization::{OperationRefusalKind, OperationReviewTier};

/// Every original is active, and none is further retained.
struct ActiveOriginals;
impl OriginalBasisSource for ActiveOriginals {
    fn require_active(
        &self,
        _namespace: &QualifiedIngressNamespace,
        _original: &OriginalWorkRef,
    ) -> Result<(), OperationAuthorizationError> {
        Ok(())
    }

    fn active_retained_basis(
        &self,
        _namespace: &QualifiedIngressNamespace,
        _original: &OriginalWorkRef,
    ) -> Result<Option<WorkAuthorityBasis>, OperationAuthorizationError> {
        Ok(None)
    }
}

fn no_further_originals() -> FurtherOriginals {
    FurtherOriginals {
        originals: Vec::new().into(),
        source: Arc::new(ActiveOriginals),
    }
}

/// No unfinished controller work references any grant.
struct NoControllerWork;
impl ControllerGrantMutationCustody for NoControllerWork {
    fn with_unreferenced_controller_grant<T, E>(
        &mut self,
        _grant: &GrantLineageRef,
        mutate: impl FnOnce() -> Result<T, E>,
    ) -> Result<Result<T, E>, ControllerCustodyRefusal> {
        Ok(mutate())
    }
}

/// The helper's independent parent grant, issued before anything is held.
fn helper_parent(fixture: &Fixture) -> GrantLineageRef {
    fixture
        .grants
        .issue_root(
            &who("root"),
            id("helper-parent"),
            who("helper-operator"),
            None,
            ExecutionRestrictions::unrestricted(),
        )
        .expect("the helper's independent parent grant")
}

/// A child of `parent`, issued by its grantee.
fn child_intent(parent: &GrantLineageRef, name: &str) -> ChildGrantIntent {
    ChildGrantIntent {
        issuer: who("helper-operator"),
        parent_lineage: vec![parent.clone()],
        id: id(name),
        grantee: who("helper"),
        represented_subject: None,
        requested: ExecutionRestrictions::unrestricted(),
    }
}

/// How actor A holds the publication writer.
#[derive(Clone, Copy, Debug)]
enum Hold {
    /// Variant 1: a held exclusion, which never publishes.
    Excluded,
    /// Variant 2: a published owner change.
    Change,
}

/// Actor A, holding the publication writer on its own thread until it is
/// released.
struct Holder {
    release: mpsc::Sender<()>,
    thread: std::thread::JoinHandle<()>,
}

impl Holder {
    /// Begin `hold` on a new thread; returns once A holds the writer.
    fn begin(fixture: &Fixture, hold: Hold) -> Self {
        let (held, holding) = mpsc::channel();
        let (release, released) = mpsc::channel::<()>();
        let grants = Arc::clone(&fixture.grants);
        let publication = fixture.publication.clone();
        let thread = std::thread::spawn(move || {
            let hold_until_released = || {
                held.send(()).expect("A reports its hold");
                // Released, or the sender dropped: `recv` returns either way.
                let _ = released.recv();
            };
            match hold {
                Hold::Excluded => grants
                    .try_with_owner_changes_excluded(hold_until_released)
                    .expect("A takes the free writer"),
                Hold::Change => {
                    let change = publication
                        .begin_owner_change()
                        .expect("A begins an owner change");
                    hold_until_released();
                    drop(change);
                }
            }
        });
        holding.recv().expect("A holds the writer");
        Self { release, thread }
    }

    /// A releases the writer, and its thread ends.
    fn release(self) {
        drop(self.release);
        self.thread.join().expect("A's thread");
    }
}

fn refused_to_rebuild<T>(result: &Result<T, OperationAuthorizationError>) -> bool {
    matches!(
        result,
        Err(OperationAuthorizationError::Refused(refusal))
            if refusal.kind() == OperationRefusalKind::ReprepareRequired
    )
}

/// While A holds the writer, every operation-time step of the native work
/// owner (the R1 run, an entered R2 run, a child issuance) refuses to
/// rebuild at once: its operation does not run and nothing is appended. The
/// work names the hold as an owner change in progress, published or not,
/// until A releases it (`change_in_progress` still names only a published
/// change). Once A released, the same steps run and issue.
///
/// RED at the base: each step answered `Unavailable`, as if the owners
/// could not decide, and a held exclusion was named nowhere, so no waiter
/// could tell it from a final answer.
#[test]
fn a_held_writer_refuses_each_operation_time_step_to_rebuild_and_is_named_while_held() {
    for hold in [Hold::Excluded, Hold::Change] {
        let fixture = Fixture::new();
        let intent = child_intent(&helper_parent(&fixture), "contended-child");
        let association = fixture.association();
        let (policy, _, _) = fixture.policy(association.clone());
        let context = fixture.context(&policy, &association);
        let binding = source(SourceAuthorizationUse::Read, "public");
        let validity = context
            .authorization()
            .prepare(&binding)
            .expect("prepared before the hold")
            .validity();
        let further = no_further_originals();
        assert_eq!(
            context.authorization().owner_change_in_progress(),
            None,
            "{hold:?}: nothing is held yet"
        );

        let holder = Holder::begin(&fixture, hold);
        let named = context.authorization().owner_change_in_progress();
        assert!(
            named.is_some(),
            "{hold:?}: the hold is named while it lasts"
        );
        match hold {
            Hold::Excluded => assert_eq!(
                fixture.publication.change_in_progress(),
                None,
                "an exclusion publishes nothing: the audit's change stays unnamed"
            ),
            Hold::Change => assert_eq!(
                fixture.publication.change_in_progress(),
                named,
                "a published change keeps one token"
            ),
        }
        let mut ran = false;
        let run = context.authorization().run_if_current_within(
            &binding,
            validity.as_ref(),
            &further,
            &mut || ran = true,
        );
        assert!(
            refused_to_rebuild(&run) && !ran,
            "{hold:?}: the R1 run refuses to rebuild: {run:?}"
        );
        let entered = context.authorization().run_entered_if_current_within(
            &binding,
            validity.as_ref(),
            &further,
            &mut || ran = true,
            OperationReviewTier::R2,
        );
        assert!(
            refused_to_rebuild(&entered) && !ran,
            "{hold:?}: the entered run refuses to rebuild: {entered:?}"
        );
        let issued = context
            .authorization()
            .issue_child_if_current_within(
                &binding,
                validity.as_ref(),
                &further,
                &intent,
                OperationReviewTier::R1,
            )
            .map(drop);
        assert!(
            refused_to_rebuild(&issued),
            "{hold:?}: the issuance refuses to rebuild: {issued:?}"
        );
        assert_eq!(
            fixture.grants.issued_reference(&intent.id).expect("lookup"),
            None,
            "{hold:?}: nothing was appended"
        );
        holder.release();

        assert_eq!(
            context.authorization().owner_change_in_progress(),
            None,
            "{hold:?}: named until released, and no longer"
        );
        // A fresh decision's window: the refusal above was contention only.
        let validity = context
            .authorization()
            .prepare(&binding)
            .expect("prepared after the hold")
            .validity();
        assert_eq!(
            context.authorization().run_if_current_within(
                &binding,
                validity.as_ref(),
                &further,
                &mut || ran = true,
            ),
            Ok(()),
            "{hold:?}: the run takes the free writer"
        );
        assert!(ran, "{hold:?}: and its operation runs");
        let issued = context
            .authorization()
            .issue_child_if_current_within(
                &binding,
                validity.as_ref(),
                &further,
                &intent,
                OperationReviewTier::R1,
            )
            .expect("the issuance takes the free writer");
        assert!(
            matches!(
                *issued
                    .downcast::<Result<ChildGrantOutcome, ChildGrantRefusal>>()
                    .expect("the grant owner's typed outcome"),
                Ok(ChildGrantOutcome::Issued { .. })
            ),
            "{hold:?}: the child is issued"
        );
    }
}

/// Control: the host-facing entries that never wait keep answering
/// `Unavailable` for the same hold, before any mutation, as they always did.
#[test]
fn host_facing_entries_that_never_wait_stay_unavailable_under_the_hold() {
    for hold in [Hold::Excluded, Hold::Change] {
        let fixture = Fixture::new();
        let intent = child_intent(&helper_parent(&fixture), "host-child");
        let holder = Holder::begin(&fixture, hold);
        assert_eq!(
            fixture.grants.try_with_owner_changes_excluded(|| ()),
            Err(PublicationError::Unavailable),
            "{hold:?}: another exclusion"
        );
        assert!(
            matches!(
                fixture.publication.try_begin_owner_change(),
                Err(PublicationError::Unavailable)
            ),
            "{hold:?}: an owner change begun without waiting"
        );
        assert_eq!(
            fixture
                .grants
                .issue_or_recover_child_under_exclusion(&intent, || Err(GrantRefusal::Denied)),
            Err(ChildGrantRefusal::Unavailable),
            "{hold:?}: a host's child issuance"
        );
        assert_eq!(
            fixture
                .grants
                .revoke(&who("root"), &fixture.controller, &mut NoControllerWork),
            Err(GrantRefusal::Unavailable),
            "{hold:?}: a revocation"
        );
        holder.release();
        assert_eq!(
            fixture.grants.issued_reference(&intent.id).expect("lookup"),
            None,
            "{hold:?}: nothing was appended"
        );
    }
}

/// Control: a poisoned publication stays `Unavailable` at every step, never
/// a refusal to rebuild, and names no change in progress, even while A holds
/// its writer and after A released it.
#[test]
fn a_poisoned_publication_stays_unavailable_at_each_step_even_while_its_writer_is_held() {
    for hold in [Hold::Excluded, Hold::Change] {
        let fixture = Fixture::new();
        let intent = child_intent(&helper_parent(&fixture), "poisoned-child");
        let association = fixture.association();
        let (policy, _, _) = fixture.policy(association.clone());
        let context = fixture.context(&policy, &association);
        let binding = source(SourceAuthorizationUse::Read, "public");
        let validity = context
            .authorization()
            .prepare(&binding)
            .expect("prepared before the poison")
            .validity();
        let further = no_further_originals();
        let steps = || {
            let mut ran = false;
            let run = context.authorization().run_if_current_within(
                &binding,
                validity.as_ref(),
                &further,
                &mut || ran = true,
            );
            let entered = context.authorization().run_entered_if_current_within(
                &binding,
                validity.as_ref(),
                &further,
                &mut || ran = true,
                OperationReviewTier::R2,
            );
            let issued = context
                .authorization()
                .issue_child_if_current_within(
                    &binding,
                    validity.as_ref(),
                    &further,
                    &intent,
                    OperationReviewTier::R1,
                )
                .map(drop);
            (run, entered, issued, ran)
        };
        let unavailable = (
            Err::<(), _>(OperationAuthorizationError::Unavailable),
            Err::<(), _>(OperationAuthorizationError::Unavailable),
            Err::<(), _>(OperationAuthorizationError::Unavailable),
            false,
        );

        let holder = Holder::begin(&fixture, hold);
        fixture.publication.poison();
        assert_eq!(
            context.authorization().owner_change_in_progress(),
            None,
            "{hold:?}: a poisoned publication has no change to wait for"
        );
        assert_eq!(steps(), unavailable, "{hold:?}: while the writer is held");
        holder.release();
        assert_eq!(steps(), unavailable, "{hold:?}: after its release");
        assert_eq!(
            fixture.grants.issued_reference(&intent.id).expect("lookup"),
            None,
            "{hold:?}: nothing was appended"
        );
    }
}
