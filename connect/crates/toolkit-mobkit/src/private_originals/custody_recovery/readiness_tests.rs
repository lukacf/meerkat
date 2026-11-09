//! Readiness of one enrolled pending source custody, as Core's native ingress
//! check and operation ceilings read it through retained_custody while that
//! reader holds the source collection. Enrollment and acceptance hold the
//! source-wide recovery control (and acceptance the invocation lock) and then
//! take the source collection, so this read must neither wait for those locks
//! nor answer their contention as unavailable policy.
use super::*;
use crate::original_ingress::tests::{claims, initialized, principal, private_originals, source};
use meerkat_authorization_contracts::constraints::ExecutionRestrictions;
use meerkat_core::authorization::ControllerReadinessFailure;
use meerkat_runtime::meerkat_machine::{AuthenticatedOriginalSource, RetainedCustody};

struct Pending {
    owner: Arc<crate::original_ingress::ToolkitOriginalOwner>,
    source: Arc<PrivateOriginals>,
    original: Arc<crate::original_ingress::RetainedToolkitOriginal>,
    handoff: Arc<CustodyHandoff>,
    // Retained for its drop: the candidate stays pending while a row reads it.
    _candidate: crate::original_ingress::PendingToolkitOriginal,
    _dir: tempfile::TempDir,
}

impl Pending {
    async fn new() -> Self {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap();
        let owner = initialized(&dir.path().join("grants"));
        let base = owner
            .grants()
            .issue_root(
                &meerkat_core::AdmissionGuard::none(),
                &principal("admin"),
                EvidenceId::new("base").unwrap(),
                principal("admin"),
                None,
                ExecutionRestrictions::unrestricted(),
            )
            .unwrap();
        let association = claims("a", &base);
        let original = owner
            .enroll_new_authenticated(
                association.clone(),
                source(&association),
                &principal("user"),
                &principal("frontend"),
                &principal("admin"),
                EvidenceId::new("a-child").unwrap(),
                ExecutionRestrictions::unrestricted(),
                |_| Ok(None),
            )
            .unwrap();
        let source = private_originals(owner.clone(), &dir.path().join("originals.sqlite3")).await;
        source.select_custody_recovery_for_test();
        let candidate = owner.record_candidate(&original).unwrap();
        let handoff = CustodyHandoff::new(
            true,
            &source,
            &crate::review_custody::ReviewCustody::default(),
        );
        candidate.enroll_settlement(&handoff).unwrap();
        Self {
            owner,
            source,
            original,
            handoff,
            _candidate: candidate,
            _dir: dir,
        }
    }

    fn read(&self) -> Result<RetainedCustody, ControllerReadinessFailure> {
        let claims = self.original.association().candidate();
        self.owner
            .retained_custody(&claims.ingress_namespace, &claims.original_work)
    }
}

#[tokio::test]
async fn pending_readiness_answers_while_enrollment_holds_the_recovery_control() {
    let pending = Pending::new().await;
    assert!(matches!(
        pending.read(),
        Ok(RetainedCustody::AdmissionPending)
    ));
    // Enrollment and acceptance hold this source-wide control, then take the
    // source collection that the native reader below already holds.
    let control = pending
        .source
        .custody_recovery
        .control
        .state
        .lock()
        .unwrap();
    let answer = pending.read();
    drop(control);
    assert!(
        matches!(answer, Ok(RetainedCustody::AdmissionPending)),
        "a held recovery control is contention, not unavailable policy: {answer:?}"
    );
}

#[tokio::test]
async fn pending_readiness_answers_while_acceptance_holds_the_invocation() {
    let pending = Pending::new().await;
    assert!(matches!(
        pending.read(),
        Ok(RetainedCustody::AdmissionPending)
    ));
    let invocation = pending.handoff.state.lock().unwrap();
    let answer = pending.read();
    drop(invocation);
    assert!(
        matches!(answer, Ok(RetainedCustody::AdmissionPending)),
        "a held invocation lock is contention, not unavailable policy: {answer:?}"
    );
}

impl Pending {
    /// Run `transition` exactly between the two control reads of the next
    /// readiness read, so it races that read at its most exposed point.
    fn inside_next_read(&self, transition: impl FnOnce() + Send + 'static) {
        let mut hook = self
            .source
            .custody_recovery
            .control
            .between_readiness_reads
            .lock()
            .unwrap();
        assert!(hook.replace(Box::new(transition)).is_none());
    }
}

fn refused(answer: Result<RetainedCustody, ControllerReadinessFailure>) -> bool {
    matches!(answer, Err(ControllerReadinessFailure::PolicyUnavailable))
}

// Each terminal transition lands between the reader's two control reads. The
// reader never answers from the facts before it, and no later read answers
// Pending again: stop, control poison and invocation poison are permanent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stop_racing_the_read_is_never_answered_pending() {
    let pending = Pending::new().await;
    let source = pending.source.clone();
    pending.inside_next_read(move || source.stop_custody_recovery());
    assert!(refused(pending.read()));
    assert!(
        pending
            .source
            .custody_recovery
            .control
            .readiness()
            .stopped()
    );
    assert!(refused(pending.read()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn control_poison_racing_the_read_is_never_answered_pending() {
    let pending = Pending::new().await;
    let source = pending.source.clone();
    pending.inside_next_read(move || source.poison_custody_control_for_test());
    assert!(refused(pending.read()));
    assert!(refused(pending.read()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invocation_poison_racing_the_read_is_never_answered_pending() {
    let pending = Pending::new().await;
    let handoff = pending.handoff.clone();
    pending.inside_next_read(move || {
        let poisoned = std::thread::spawn(move || {
            let _state = handoff.state.lock().unwrap();
            panic!("controlled invocation poison");
        })
        .join();
        assert!(poisoned.is_err());
    });
    assert!(refused(pending.read()));
    assert!(pending.handoff.state.is_poisoned());
    assert!(refused(pending.read()));
}

// An unreadable canonical row refuses at once, including inside a read, and
// only the owner's own readable observation restores Pending. A failed
// publication can never restore it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unreadable_and_failed_publication_refuse_and_only_a_live_owner_restores() {
    let pending = Pending::new().await;
    let handoff = pending.handoff.clone();
    pending.inside_next_read(move || handoff.canonical_readable(false));
    assert!(refused(pending.read()));
    assert!(refused(pending.read()));
    pending.handoff.canonical_readable(true);
    assert!(matches!(
        pending.read(),
        Ok(RetainedCustody::AdmissionPending)
    ));
    let publication = pending.owner.publication();
    let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _change = publication.begin_owner_change().unwrap();
        panic!("controlled publication failure");
    }));
    assert!(failed.is_err());
    pending.handoff.canonical_readable(false);
    assert!(refused(pending.read()));
    pending.handoff.canonical_readable(true);
    assert!(
        refused(pending.read()),
        "a failed publication can never restore pending readiness"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn source_loss_and_withdrawal_are_terminal_for_the_read() {
    let pending = Pending::new().await;
    let Pending {
        owner,
        source,
        original,
        handoff,
        _candidate: candidate,
        _dir: dir,
    } = pending;
    let claims = original.association().candidate();
    let read = || owner.retained_custody(&claims.ingress_namespace, &claims.original_work);
    assert!(matches!(read(), Ok(RetainedCustody::AdmissionPending)));
    let retained = Arc::downgrade(&source);
    drop(source);
    assert!(retained.upgrade().is_none(), "the source owner is gone");
    assert!(refused(read()));
    // Withdrawal of the pending source custody ends Pending for good, whatever
    // the invocation's own readiness facts still say.
    drop(candidate);
    assert!(matches!(read(), Ok(RetainedCustody::NotRetained)));
    assert!(matches!(read(), Ok(RetainedCustody::NotRetained)));
    drop(handoff);
    drop(dir);
}

// The order that made a waiting read unsafe: enrollment and acceptance hold
// the recovery control and then take the source collection, which the native
// reader already holds while it reads readiness. The reader completes without
// the control, so the control holder then gets the collection: no refusal and
// no deadlock.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_read_inside_the_source_collection_never_waits_for_the_control_holder() {
    let pending = Pending::new().await;
    let claims = pending.original.association().candidate().clone();
    let (inside, reached) = std::sync::mpsc::channel();
    let (proceed, resume) = std::sync::mpsc::channel::<()>();
    pending.inside_next_read(move || {
        inside.send(()).unwrap();
        resume.recv().unwrap();
    });
    let control = pending
        .source
        .custody_recovery
        .control
        .state
        .lock()
        .unwrap();
    let answer = std::thread::scope(|scope| {
        let reader = scope.spawn(|| pending.read());
        reached.recv().unwrap();
        // The reader is inside its readiness read and holds the source
        // collection, while this thread holds the recovery control.
        assert!(matches!(
            pending
                .owner
                .admitted_original_basis(&claims.ingress_namespace, &claims.original_work),
            Err(ControllerReadinessFailure::PolicyUnavailable)
        ));
        proceed.send(()).unwrap();
        let answer = reader.join().unwrap();
        // The control holder now takes the collection, as enrollment does.
        assert!(
            pending
                .owner
                .original_disposition(&claims.ingress_namespace, &claims.original_work)
                .is_ok()
        );
        answer
    });
    drop(control);
    assert!(
        matches!(answer, Ok(RetainedCustody::AdmissionPending)),
        "the read answers without waiting for the control holder: {answer:?}"
    );
}
