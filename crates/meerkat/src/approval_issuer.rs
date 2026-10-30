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
    use meerkat_core::approval::decision::{
        ApproverId, DecisionHoldRequest, DecisionKind, DecisionOperationId, DecisionPrincipalKey,
        DecisionRenderer, DecisionRendering, DecisionRequestBody, DecisionSubjectKey, Sha256Digest,
    };
    use meerkat_core::{PrincipalKind, PrincipalRef, TrustDomainId};
    use meerkat_runtime::decision::{DecisionError, DecisionWriteMode};
    use meerkat_store::approval_decision_store::APPROVAL_DECISIONS_FILE;

    use super::ApprovalIssuerSelection;

    fn hold_request() -> DecisionHoldRequest {
        let requester = PrincipalRef::in_domain(
            PrincipalKind::PersonalAgent,
            "builder",
            TrustDomainId::new("example.org").expect("trust domain"),
        )
        .expect("principal");
        DecisionHoldRequest {
            kind: DecisionKind::new("release").expect("kind"),
            subject_key: DecisionSubjectKey::new("artifact:build-7").expect("subject key"),
            operation_id: DecisionOperationId::new("operation-7").expect("operation id"),
            request_digest: Sha256Digest::of(br#"{"build":7}"#),
            requester: DecisionPrincipalKey::of(&requester).expect("principal key"),
            approver_id: ApproverId::new("team:owners").expect("approver id"),
            reviewer: None,
            deadline_at_ms: u64::MAX / 2,
            request: DecisionRequestBody::from_value(&serde_json::json!({"build": 7}))
                .expect("request body"),
            rendering: DecisionRendering::new("Release build 7.").expect("rendering"),
            renderer: DecisionRenderer::new("render.v1").expect("renderer"),
            expected_subject_digest: None,
        }
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
}
