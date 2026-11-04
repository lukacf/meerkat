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
