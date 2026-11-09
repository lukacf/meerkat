//! The one approval issuer of a host's persistence.
//!
//! Every surface, the operation review, decision ingress, recovery and
//! delivery over one persistence share one [`ApprovalService`]: the host
//! states whether its persistence is durable, selects the issuer once,
//! before the factory is cloned, and passes that owner on. It is never
//! replaced while it issues. Two services over one approval file are two
//! in-process owners, so a host opens its selection exactly once.
//!
//! The same selection opens the persistence's durable decision owner
//! ([`ApprovalIssuerSelection::open_decisions`]): its decisions live in their
//! own file beside `approvals.json`, which they never touch.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use meerkat_core::ApprovalService;
use meerkat_runtime::decision::DurableDecisionService;

/// How a host's persistence selects its one approval issuer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalIssuerSelection {
    /// Durable approvals in `approvals.json` under the host's store path,
    /// and durable decisions beside them.
    Persistent { store_path: PathBuf },
    /// Process-local approvals, and no durable decisions, for a host that is
    /// explicitly ephemeral.
    Ephemeral,
}

/// A host's approval issuer cannot be selected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApprovalIssuerSelectionError {
    #[error("a persistent host must name its approval store path")]
    PersistentWithoutStorePath,
}

impl ApprovalIssuerSelection {
    /// The selection of a host that states whether it is `persistent`: a
    /// persistent host names its `store_path`, and an absent path never
    /// downgrades it to ephemeral approvals.
    ///
    /// # Errors
    /// [`ApprovalIssuerSelectionError::PersistentWithoutStorePath`].
    pub fn for_host(
        persistent: bool,
        store_path: Option<&Path>,
    ) -> Result<Self, ApprovalIssuerSelectionError> {
        match (persistent, store_path) {
            (true, Some(store_path)) => Ok(Self::Persistent {
                store_path: store_path.to_path_buf(),
            }),
            (true, None) => Err(ApprovalIssuerSelectionError::PersistentWithoutStorePath),
            (false, _) => Ok(Self::Ephemeral),
        }
    }

    /// Open the one issuer of this selection; a host calls it once. A
    /// persistent selection whose store cannot be opened or loaded yields an
    /// unavailable service that refuses every approval operation, never an
    /// ephemeral one.
    #[must_use]
    pub fn open(&self) -> ApprovalService {
        let Self::Persistent { store_path } = self else {
            return ApprovalService::new();
        };
        let path = store_path.join("approvals.json");
        match meerkat_store::FileApprovalStore::open(&path) {
            Ok(store) => match ApprovalService::with_store(Arc::new(store)) {
                Ok(service) => service,
                Err(err) => {
                    tracing::warn!(
                        error = %err,
                        path = %path.display(),
                        "failed to load approval records; approval service unavailable and failing closed"
                    );
                    ApprovalService::unavailable(format!(
                        "failed to load approval records from {}: {err}",
                        path.display()
                    ))
                }
            },
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    path = %path.display(),
                    "failed to open approval store; approval service unavailable and failing closed"
                );
                ApprovalService::unavailable(format!(
                    "failed to open approval store {}: {err}",
                    path.display()
                ))
            }
        }
    }

    /// Open the durable decision owner of this selection, beside its
    /// approval issuer; a host opens it once and shares it. A persistent
    /// selection keeps decisions in `approval_decisions.sqlite3` under its
    /// store path: opening reads and creates nothing, and the first hold
    /// creates the file. An ephemeral selection keeps none: its owner refuses
    /// every call as unavailable.
    #[must_use]
    pub fn open_decisions(&self) -> DurableDecisionService {
        match self {
            Self::Persistent { store_path } => DurableDecisionService::new(Arc::new(
                meerkat_store::SqliteDecisionStore::in_store_path(store_path),
            )),
            Self::Ephemeral => {
                DurableDecisionService::unavailable("an ephemeral host keeps no durable decisions")
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    use meerkat_core::approval::consent::{
        AuthenticatedApprover, ConsentEligibility, ConsentIneligible,
    };
    use meerkat_core::approval::decision::{
        ApproverId, AttestedDecision, DecisionHoldRequest, DecisionId, DecisionIngress,
        DecisionKind, DecisionNotice, DecisionOperationId, DecisionPhase, DecisionPrincipalKey,
        DecisionReference, DecisionRenderer, DecisionRendering, DecisionRequestBody,
        DecisionSubjectKey, PresentationAudience, PresentationRecord, Sha256Digest,
    };
    use meerkat_core::{
        ApprovalAssurance, ApprovalListFilter, PrincipalKind, PrincipalRef, TrustDomainId,
    };
    use meerkat_runtime::decision::{
        DecisionClock, DecisionClockError, DecisionError, DecisionWriteMode,
        DurableDecisionService, SystemDecisionClock,
    };
    use meerkat_store::SqliteDecisionStore;
    use meerkat_store::approval_decision_store::APPROVAL_DECISIONS_FILE;

    use super::ApprovalIssuerSelection;

    fn principal(kind: PrincipalKind, id: &str) -> PrincipalRef {
        PrincipalRef::in_domain(
            kind,
            id,
            TrustDomainId::new("example.org").expect("trust domain"),
        )
        .expect("principal")
    }

    fn approver_id() -> ApproverId {
        ApproverId::new("team:owners").expect("approver id")
    }

    fn reference(value: &str) -> DecisionReference {
        DecisionReference::new(value).expect("reference")
    }

    fn rendering() -> DecisionRendering {
        DecisionRendering::new("Release build 7.").expect("rendering")
    }

    /// The hold of `subject` until `deadline_at_ms`.
    fn hold_of(subject: &str, deadline_at_ms: u64) -> DecisionHoldRequest {
        DecisionHoldRequest::new(
            DecisionKind::new("release").expect("kind"),
            DecisionSubjectKey::new(subject).expect("subject key"),
            DecisionOperationId::new("operation-7").expect("operation id"),
            Sha256Digest::of(br#"{"build":7}"#),
            DecisionRequestBody::from_value(&serde_json::json!({"build": 7}))
                .expect("request body"),
            DecisionPrincipalKey::of(&principal(PrincipalKind::PersonalAgent, "builder"))
                .expect("principal key"),
            approver_id(),
            deadline_at_ms,
            rendering(),
            DecisionRenderer::new("render.v1").expect("renderer"),
        )
    }

    fn hold_request() -> DecisionHoldRequest {
        hold_of("artifact:build-7", u64::MAX / 2)
    }

    /// Eligibility that is always current.
    struct Eligible;

    impl ConsentEligibility for Eligible {
        fn check_current(&self) -> Result<(), ConsentIneligible> {
            Ok(())
        }
    }

    /// Present the rendering to the approver id and approve the decision,
    /// observed at the presentation's own time.
    fn approve(decisions: &DurableDecisionService, decision_id: &DecisionId, at_ms: u64) {
        let rendering = rendering();
        decisions
            .present(
                decision_id,
                &PresentationRecord::complete(
                    PresentationAudience::Approver {
                        approver_id: approver_id(),
                    },
                    &rendering,
                    None,
                    reference("console"),
                    reference("message-1"),
                    at_ms,
                ),
            )
            .expect("present");
        let decision = AttestedDecision::new(
            AuthenticatedApprover::new(
                principal(PrincipalKind::Human, "alice"),
                ApprovalAssurance::PersonAuthenticated,
            )
            .expect("authenticated human"),
            DecisionIngress::new("console").expect("ingress"),
            reference("event-1"),
            at_ms,
            approver_id(),
            rendering.subject_digest(),
            true,
        )
        .expect("attested decision");
        let approved = decisions
            .decide(decision_id, &decision, &Eligible)
            .expect("approve");
        assert_eq!(approved.state.phase, DecisionPhase::Approved);
    }

    fn approval_request() -> meerkat_core::ApprovalRequest {
        meerkat_core::ApprovalRequest {
            requester: meerkat_core::ApprovalPrincipalId::new("human:alice").expect("principal"),
            owner: meerkat_core::ApprovalOwnerRef::Runtime,
            resource: meerkat_core::ApprovalResourceRef {
                kind: meerkat_core::ApprovalResourceKind::Runtime,
                id: meerkat_core::ApprovalResourceId::new("local"),
            },
            proposed_action: meerkat_core::ApprovalProposedAction {
                kind: meerkat_core::ApprovalActionKind::Other,
                summary: "manual gate".to_string(),
                body: None,
            },
            risk: meerkat_core::ApprovalRisk::Medium,
            request_body: None,
            allowed_decisions: std::collections::BTreeSet::from([
                meerkat_core::ApprovalDecision::Approve,
                meerkat_core::ApprovalDecision::Deny,
            ]),
            expires_at: None,
            metadata: meerkat_core::SurfaceMetadata::default(),
            request_provenance: None,
        }
    }

    struct ManualClock(AtomicU64);

    impl DecisionClock for ManualClock {
        fn now_ms(&self) -> Result<u64, DecisionClockError> {
            Ok(self.0.load(Ordering::SeqCst))
        }
    }

    fn file_digest(path: &Path) -> Sha256Digest {
        Sha256Digest::of(&std::fs::read(path).expect("read the decisions file"))
    }

    fn sorted(mut ids: Vec<DecisionId>) -> Vec<DecisionId> {
        ids.sort();
        ids
    }

    #[test]
    fn a_persistent_issuer_opens_the_decision_owner_lazily_beside_approvals_json() {
        let temp = tempfile::tempdir().expect("tempdir");
        let selection = ApprovalIssuerSelection::Persistent {
            store_path: temp.path().to_path_buf(),
        };
        let file = temp.path().join(APPROVAL_DECISIONS_FILE);
        let decisions = selection.open_decisions();
        assert!(decisions.is_persistent());
        assert_eq!(decisions.write_mode(), DecisionWriteMode::Admitted);
        assert!(!file.exists(), "opening creates nothing");
        assert!(decisions.owed(None, 10).expect("owed").decisions.is_empty());
        assert!(!file.exists(), "a read creates nothing");

        let held = decisions.hold(hold_request(), true).expect("hold");
        assert!(file.is_file(), "the first hold creates the file");
        assert!(
            !temp.path().join("approvals.json").exists(),
            "decisions never touch the approval records"
        );
        let reopened = selection.open_decisions();
        assert_eq!(
            reopened.get(&held.decision_id).expect("durable"),
            held,
            "another owner of the selection reads the decision"
        );
    }

    #[test]
    fn an_ephemeral_issuer_refuses_durable_decisions() {
        let decisions = ApprovalIssuerSelection::Ephemeral.open_decisions();
        assert!(!decisions.is_persistent());
        assert!(matches!(
            decisions.hold(hold_request(), true),
            Err(DecisionError::Unavailable(_))
        ));
        assert!(matches!(
            decisions.owed(None, 10),
            Err(DecisionError::Unavailable(_))
        ));
    }

    /// Rollback and roll-forward in process. Under the rollback the approval
    /// issuer (whose `approvals.json` format is unchanged) opens the same
    /// store path and never touches the decisions file; rolled forward, an
    /// open decision past its deadline expires at its first touch and owes
    /// its notice, and an approved one before its deadline stays owed.
    #[test]
    fn roll_forward_expires_open_and_keeps_approved_owed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let selection = ApprovalIssuerSelection::Persistent {
            store_path: temp.path().to_path_buf(),
        };
        let clock = Arc::new(ManualClock(AtomicU64::new(1_000)));
        let owner = || {
            let clock: Arc<dyn DecisionClock> = clock.clone();
            DurableDecisionService::with_clock(
                Arc::new(SqliteDecisionStore::in_store_path(temp.path())),
                clock,
            )
        };
        let before = owner();
        let open = before
            .hold(hold_of("subject:open", 61_000), true)
            .expect("hold")
            .decision_id;
        let approved = before
            .hold(hold_of("subject:approved", 10_000_000), true)
            .expect("hold")
            .decision_id;
        approve(&before, &approved, 1_000);
        drop(before);
        let issued = selection
            .open()
            .request(approval_request())
            .expect("an approval beside the decisions");
        let file = temp.path().join(APPROVAL_DECISIONS_FILE);
        let written = file_digest(&file);

        // Rolled back: the approval issuer serves the same store path.
        let approvals = selection.open();
        let listed = approvals
            .list(ApprovalListFilter::default())
            .expect("approvals stay available");
        assert_eq!(
            listed
                .iter()
                .map(|record| record.approval_id.clone())
                .collect::<Vec<_>>(),
            vec![issued.approval_id]
        );
        assert_eq!(
            file_digest(&file),
            written,
            "the decisions file is untouched"
        );

        // Rolled forward past the open decision's deadline.
        clock.0.store(100_000, Ordering::SeqCst);
        let after = owner();
        assert_eq!(
            after
                .due(None, 10)
                .expect("due")
                .decisions
                .into_iter()
                .map(|row| row.decision_id)
                .collect::<Vec<_>>(),
            vec![open.clone()]
        );
        let expired = after
            .get(&open)
            .expect("the first touch records the expiry");
        assert_eq!(expired.state.phase, DecisionPhase::Expired);
        assert_eq!(expired.state.notice, DecisionNotice::Owed);
        assert_eq!(
            after.get(&approved).expect("read").state.phase,
            DecisionPhase::Approved
        );
        assert_eq!(
            sorted(
                after
                    .owed(None, 10)
                    .expect("owed")
                    .decisions
                    .into_iter()
                    .map(|row| row.decision_id)
                    .collect()
            ),
            sorted(vec![open, approved])
        );
    }

    /// The rollback fixture directory the lane names, with `leg`.
    fn rollback_fixture(leg: &str) -> PathBuf {
        PathBuf::from(
            std::env::var_os("MEERKAT_ROLLBACK_FIXTURE_DIR")
                .expect("MEERKAT_ROLLBACK_FIXTURE_DIR names the fixture directory"),
        )
        .join(leg)
    }

    /// Rollback, lane-run leg (R-CC-RB1): needs the published Core 0.9.7.
    ///
    /// This build writes the fixture: a store path at
    /// `$MEERKAT_ROLLBACK_FIXTURE_DIR/decisions-under-097` with one approval
    /// in `approvals.json`, an open decision whose deadline passes a second
    /// after the write and an approved one in `approval_decisions.sqlite3`,
    /// and `expected.json` naming the approval, the two decisions and the
    /// decisions file's sha256. The lane then opens the store path with the
    /// published Core 0.9.7 (its approval issuer and `approval/list`) and
    /// checks: the approval is listed exactly as before, approvals stay
    /// available, and the decisions file's digest is unchanged. The harness
    /// that runs the published crates is a release gate.
    #[test]
    #[ignore = "lane:rollback - writes the fixture the published Core 0.9.7 opens"]
    fn decision_file_written_by_098_is_ignored_by_published_core_097() {
        let dir = rollback_fixture("decisions-under-097");
        std::fs::create_dir_all(&dir).expect("create the fixture directory");
        let selection = ApprovalIssuerSelection::Persistent {
            store_path: dir.clone(),
        };
        let issued = selection
            .open()
            .request(approval_request())
            .expect("an approval");
        let now_ms = SystemDecisionClock.now_ms().expect("the host clock");
        let decisions = selection.open_decisions();
        let open = decisions
            .hold(hold_of("subject:open", now_ms + 1_000), true)
            .expect("hold")
            .decision_id;
        let approved = decisions
            .hold(
                hold_of("subject:approved", now_ms + 30 * 24 * 60 * 60 * 1_000),
                true,
            )
            .expect("hold")
            .decision_id;
        approve(&decisions, &approved, now_ms);
        drop(decisions);
        let expected = serde_json::json!({
            "approval_ids": [issued.approval_id.to_string()],
            "open_decision": open.as_str(),
            "approved_decision": approved.as_str(),
            "decisions_file_sha256": file_digest(&dir.join(APPROVAL_DECISIONS_FILE)).as_str(),
        });
        std::fs::write(
            dir.join("expected.json"),
            serde_json::to_vec_pretty(&expected).expect("encode"),
        )
        .expect("write the expectations");
    }

    /// Roll-forward, lane-run leg (R-CC-RB2): runs after the published Core
    /// 0.9.7 served the fixture of
    /// [`decision_file_written_by_098_is_ignored_by_published_core_097`].
    /// The decisions file is byte-identical to what this build wrote; the
    /// open decision, past its deadline, expires at its first touch and owes
    /// its notice; the approved one stays approved and owed.
    #[test]
    #[ignore = "lane:rollback - reads the fixture back after the published Core 0.9.7 ran"]
    fn roll_forward_after_097_expires_open_and_keeps_approved_owed() {
        let dir = rollback_fixture("decisions-under-097");
        let expected: serde_json::Value = serde_json::from_slice(
            &std::fs::read(dir.join("expected.json")).expect("read the expectations"),
        )
        .expect("decode the expectations");
        let field = |name: &str| -> String {
            expected[name]
                .as_str()
                .unwrap_or_else(|| panic!("expected.json names {name}"))
                .to_owned()
        };
        assert_eq!(
            file_digest(&dir.join(APPROVAL_DECISIONS_FILE)).as_str(),
            field("decisions_file_sha256"),
            "Core 0.9.7 left the decisions file untouched"
        );
        let open = DecisionId::new(field("open_decision")).expect("decision id");
        let approved = DecisionId::new(field("approved_decision")).expect("decision id");
        let decisions = ApprovalIssuerSelection::Persistent { store_path: dir }.open_decisions();
        let expired = decisions.get(&open).expect("read");
        assert_eq!(expired.state.phase, DecisionPhase::Expired);
        assert_eq!(expired.state.notice, DecisionNotice::Owed);
        assert_eq!(
            decisions.get(&approved).expect("read").state.phase,
            DecisionPhase::Approved
        );
        assert_eq!(
            sorted(
                decisions
                    .owed(None, 10)
                    .expect("owed")
                    .decisions
                    .into_iter()
                    .map(|row| row.decision_id)
                    .collect()
            ),
            sorted(vec![open, approved])
        );
    }
}
