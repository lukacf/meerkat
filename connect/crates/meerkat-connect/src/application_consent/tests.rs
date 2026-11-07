//! Tests the Connect selection/lease/disposition owner through actual Core
//! review admission, decision, entry and OS hosting APIs. The fixture's one
//! trusted policy selects only its local probe as R3; it is not a Console
//! authentication adapter, installed gateway or recovered native actor.
use super::*;
use futures_util::FutureExt;
use meerkat_core::{
    AgentToolDispatcher, ApprovalActionKind, ApprovalDecision, ApprovalOwnerRef,
    ApprovalPrincipalId, ApprovalProposedAction, ApprovalRequest, ApprovalResourceId,
    ApprovalResourceKind, ApprovalResourceRef, ApprovalRisk, ApprovalStatus, PrincipalKind,
    ToolCallView, ToolDispatchContext, ToolError, TrustDomainId,
    approval::{
        consent::{
            AuthenticatedApprover, AuthenticatedConsentDecision, ConsentEligibility,
            ConsentIneligible, ConsentNotice, ConsentPresentation, ConsentPresentationCandidate,
            ConsentRouteUnavailable, HumanConsentRoute,
        },
        review::{
            BoundOperationReview, OperationReviewer, ReviewCandidate, ReviewEntrySupport,
            ReviewUnsatisfiedKind, ReviewVerdict, ReviewerFailure,
        },
    },
    authorization::{
        AuthorizationOperation, OperationAuthorizationError, OperationReviewTier,
        PreparedAuthorizationBinding, PreparedOperationAuthorization,
        ReviewOperationContextResolution, WorkAuthorization, WorkAuthorizationContext,
    },
    session_hosting::{
        HostingCapability, HostingClaim, HostingOwner, HostingPaths, HostingRefused,
        OwnerLossAttachment, grant_session_hosting,
    },
};
use std::{
    panic::{AssertUnwindSafe, resume_unwind},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

const CHILD: &str = "application_consent::tests::persistent_consent_child";
const CHILD_ROOT: &str = "CONNECT_CONSENT_OWNER_TEST_ROOT";
const READY: &str = "CONNECT_CONSENT_READY=";

pub(crate) fn identity() -> ConsentApplicationIdentity {
    // The test's declared application selection. This is not a native grant
    // or hosting proof; production selection uses from_preparation.
    ConsentApplicationIdentity {
        application: PrincipalRef::in_domain(
            PrincipalKind::RuntimeHost,
            "consent-application",
            TrustDomainId::new("connect-consent-control").unwrap(),
        )
        .unwrap(),
        deployment: Id::new("consent-deployment").unwrap(),
        realm: RealmId::parse("consent-realm").unwrap(),
    }
}
fn hosting(root: &Path) -> HostingCapability {
    HostingCapability::OsLock(Arc::new(HostingPaths {
        hosting_lock_dir: root.join("hosting"),
        cold_delivery_lock: root.join("delivery.lock"),
        database: None,
    }))
}
/// A test-owned private (0700) root for a consent issuer, which refuses a
/// parent with group or other access. The mode is set at creation instead of
/// inherited from the process umask, which concurrent tests share.
pub(crate) fn private_root() -> tempfile::TempDir {
    tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap()
}
pub(crate) fn artifact(path: &Path, bytes: &[u8]) -> SelectedHostArtifact {
    create_file(path, bytes).unwrap();
    SelectedHostArtifact {
        path: path.to_owned(),
        sha256: hash(bytes),
    }
}

struct ProbePolicy;
struct ProbeDecision {
    binding: PreparedAuthorizationBinding,
}
impl WorkAuthorization for ProbePolicy {
    fn review_operation_context(
        &self,
        _: &PreparedAuthorizationBinding,
    ) -> Result<ReviewOperationContextResolution, OperationAuthorizationError> {
        Ok(ReviewOperationContextResolution::NotApplicable)
    }
    fn prepare(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<Arc<dyn PreparedOperationAuthorization>, OperationAuthorizationError> {
        let AuthorizationOperation::Tool(tool) = &binding.facts().operation else {
            return Err(OperationAuthorizationError::Unavailable);
        };
        if tool.name != "consent_probe" || tool.arguments.get() != "{}" {
            return Err(OperationAuthorizationError::Unavailable);
        }
        Ok(Arc::new(ProbeDecision {
            binding: binding.clone(),
        }))
    }
}
impl PreparedOperationAuthorization for ProbeDecision {
    fn review_tier(&self) -> OperationReviewTier {
        OperationReviewTier::R3
    }
    fn check_current(
        &self,
        binding: &PreparedAuthorizationBinding,
    ) -> Result<(), OperationAuthorizationError> {
        if !binding.same_operation(&self.binding) {
            return Err(OperationAuthorizationError::Unavailable);
        }
        Ok(())
    }
}
struct NoModelReview;
#[async_trait::async_trait]
impl OperationReviewer for NoModelReview {
    async fn review(&self, _: &ReviewCandidate<'_>) -> Result<ReviewVerdict, ReviewerFailure> {
        panic!("the fixture's exact R3 operation must never invoke model review")
    }
}
struct LocalHumanRoute;
struct LocalEligibility;
impl ConsentEligibility for LocalEligibility {
    fn check_current(&self) -> Result<(), ConsentIneligible> {
        Ok(())
    }
}
fn human() -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::Human,
        "local-fixture-person",
        TrustDomainId::new("connect-consent-control").unwrap(),
    )
    .unwrap()
}
impl HumanConsentRoute for LocalHumanRoute {
    fn present(
        &self,
        _: &ConsentPresentationCandidate<'_>,
    ) -> Result<ConsentPresentation, ConsentRouteUnavailable> {
        Ok(ConsentPresentation {
            requester: ApprovalPrincipalId::new("fixture-requester").unwrap(),
            risk: ApprovalRisk::High,
            metadata: Default::default(),
        })
    }
    fn eligibility(
        &self,
        approver: &AuthenticatedApprover,
        _: &ApprovalRecord,
    ) -> Result<Arc<dyn ConsentEligibility>, ConsentIneligible> {
        // A trusted local fixture event and a fixed policy, deliberately no
        // claim to implement current Console fence/expiry eligibility.
        if approver.principal() != &human() {
            return Err(ConsentIneligible);
        }
        Ok(Arc::new(LocalEligibility))
    }
    fn notice(&self, _: ConsentNotice) {}
}
#[derive(Default)]
pub(crate) struct Probe(pub(crate) AtomicUsize);
#[async_trait::async_trait]
impl AgentToolDispatcher for Probe {
    fn tools(&self) -> Arc<[Arc<meerkat_core::ToolDef>]> {
        vec![Arc::new(
            meerkat_core::ToolDef::new(
                "consent_probe",
                "local fixture counter",
                serde_json::json!({"type":"object"}),
            )
            .with_provenance(meerkat_core::ToolProvenance::new(
                meerkat_core::ToolSourceKind::Builtin,
                "connect-consent-control",
            )),
        )]
        .into()
    }
    fn review_entry_support(&self, _: &str) -> ReviewEntrySupport {
        ReviewEntrySupport::ConsumesAtEntry
    }
    async fn dispatch(
        &self,
        call: ToolCallView<'_>,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
        Err(ToolError::access_denied(call.name))
    }
    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<meerkat_core::ops::ToolDispatchOutcome, ToolError> {
        // The local counter is the fixture effect. Consume the actual native
        // consent immediately before entry; declaring support alone is not a
        // consumption receipt. No await or external callback separates them.
        let _entering = context.enter_reviewed_effect(call, Some(plan))?;
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(
            meerkat_core::ToolResult::new(call.id.to_owned(), "local probe entered".into(), false)
                .into(),
        )
    }
}
pub(crate) fn review(owner: &ApplicationConsentOwner) -> Arc<BoundOperationReview> {
    Arc::new(
        BoundOperationReview::new(
            Arc::new(NoModelReview),
            owner.approvals(),
            Duration::from_secs(30),
        )
        .with_human_consent(Arc::new(LocalHumanRoute), Duration::from_secs(3600)),
    )
}
pub(crate) fn identity_for_realm(realm: RealmId) -> ConsentApplicationIdentity {
    let mut selected = identity();
    selected.realm = realm;
    selected
}
pub(crate) fn context(
    owner: &ApplicationConsentOwner,
    claim: &HostingClaim,
) -> ToolDispatchContext {
    let review = review(owner);
    ToolDispatchContext::default()
        .with_work_authorization(Some(
            WorkAuthorizationContext::new(
                Arc::new(ProbePolicy),
                meerkat_core::OperationExecutionScope::Domain,
            )
            .with_consent_hosting(claim.clone()),
        ))
        .with_operation_review(Some(review))
}
pub(crate) async fn request(
    probe: &Arc<Probe>,
    context: &ToolDispatchContext,
    id: &str,
) -> ApprovalId {
    let args = serde_json::value::RawValue::from_string("{}".into()).unwrap();
    let call = ToolCallView {
        id,
        name: "consent_probe",
        args: &args,
    };
    let resolution = meerkat_core::ToolExecutionResolutionContext::new(
        meerkat_core::ToolDeadlineChain::new(vec![meerkat_core::ToolDeadlineContributor::finite(
            meerkat_core::ToolDeadlineOwner::CoreToolDispatch,
            Duration::from_secs(30),
        )])
        .unwrap(),
    );
    let plan = meerkat_core::resolve_tool_execution_plan_fenced(probe, call, context, &resolution)
        .unwrap();
    match meerkat_core::dispatch_tool_execution_plan_fenced(probe, call, context, &plan).await {
        Err(ToolError::ReviewUnsatisfied {
            kind: ReviewUnsatisfiedKind::HumanConsentRequired,
            approval_id: Some(id),
        }) => id,
        other => panic!("actual native R3 request required, got {other:?}"),
    }
}
pub(crate) fn approve(owner: &ApplicationConsentOwner, id: &ApprovalId) {
    owner
        .approvals()
        .decide_consent(
            id,
            AuthenticatedConsentDecision {
                approver: AuthenticatedApprover::new(
                    human(),
                    meerkat_core::ApprovalAssurance::PersonAuthenticated,
                )
                .unwrap(),
                decision: ApprovalDecision::Approve,
                reason: None,
            },
        )
        .unwrap();
}
fn ordinary(owner: &ApplicationConsentOwner, session: &meerkat_core::SessionId) -> ApprovalRecord {
    owner
        .approvals()
        .request(ApprovalRequest {
            requester: ApprovalPrincipalId::new("ordinary-fixture").unwrap(),
            owner: ApprovalOwnerRef::Session {
                session_id: session.clone(),
            },
            resource: ApprovalResourceRef {
                kind: ApprovalResourceKind::Other,
                id: ApprovalResourceId::new("ordinary"),
            },
            proposed_action: ApprovalProposedAction {
                kind: ApprovalActionKind::Other,
                summary: "ordinary history".into(),
                body: None,
            },
            risk: ApprovalRisk::Low,
            request_body: None,
            allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny]
                .into_iter()
                .collect(),
            expires_at: None,
            metadata: Default::default(),
            request_provenance: None,
        })
        .unwrap()
}

#[derive(Serialize, Deserialize)]
struct ChildReady {
    selection: ConsentIssuerSelection,
    association: ConsentAssociation,
    pending: ApprovalRecord,
    approved: ApprovalRecord,
    ordinary: ApprovalRecord,
    spent: Option<ApprovalRecord>,
}

/// Only the parent controls below invoke this child. process::exit deliberately
/// skips Rust disposal: it tests actual owner loss, not graceful withdrawal.
#[tokio::test]
#[ignore = "private child of application consent owner controls"]
async fn persistent_consent_child() {
    let root = PathBuf::from(std::env::var_os(CHILD_ROOT).expect("parent-selected fixture root"));
    let owner = ApplicationConsentOwner::create(&root, identity()).unwrap();
    let session = meerkat_core::SessionId::new();
    let claim = grant_session_hosting(&hosting(&root), &HostingOwner::mint(), &session).unwrap();
    let association = owner
        .bind_association(
            artifact(
                &root.join("selected-association.json"),
                b"{\"fixture\":\"selected application association\"}",
            ),
            &claim,
        )
        .unwrap();
    let context = context(&owner, &claim);
    let probe = Arc::new(Probe::default());
    let pending = request(&probe, &context, "pending-call").await;
    let approved = request(&probe, &context, "approved-call").await;
    approve(&owner, &approved);
    assert_eq!(probe.0.load(Ordering::SeqCst), 0);
    let live_pending = owner.approvals().get(&pending).unwrap();
    let live_approved = owner.approvals().get(&approved).unwrap();
    assert!(
        matches!(
            owner.settle_abandoned_scope(&association, &claim),
            Err(ConsentOwnerError::NotAbandoned(_))
        ),
        "a reservation on the live candidate's own lineage cannot certify abandonment"
    );
    assert_eq!(owner.approvals().get(&pending).unwrap(), live_pending);
    assert_eq!(owner.approvals().get(&approved).unwrap(), live_approved);
    let spent = if std::env::var_os("CONNECT_CONSENT_OWNER_TEST_SPENT").is_some() {
        let id = request(&probe, &context, "spent-call").await;
        approve(&owner, &id);
        meerkat_core::dispatch_retained_consent_attempt(&probe, &context, &id)
            .await
            .unwrap();
        assert_eq!(probe.0.load(Ordering::SeqCst), 1);
        let entered = owner.approvals().get(&id).unwrap();
        assert!(
            entered.consent.as_ref().unwrap().consumed_at.is_some(),
            "the real reviewed effect entry must durably consume consent before readiness"
        );
        Some(entered)
    } else {
        None
    };
    let ready = ChildReady {
        selection: owner.selection().clone(),
        association,
        pending: owner.approvals().get(&pending).unwrap(),
        approved: owner.approvals().get(&approved).unwrap(),
        ordinary: ordinary(&owner, &session),
        spent,
    };
    assert_eq!(ready.pending.status, ApprovalStatus::Pending);
    assert_eq!(ready.approved.status, ApprovalStatus::Approved);
    assert!(ready.pending.consent.as_ref().unwrap().withdrawal.is_none());
    assert!(
        ready
            .approved
            .consent
            .as_ref()
            .unwrap()
            .withdrawal
            .is_none()
    );
    assert!(
        ready
            .approved
            .consent
            .as_ref()
            .unwrap()
            .consumed_at
            .is_none()
    );
    println!("{READY}{}", serde_json::to_string(&ready).unwrap());
    std::io::stdout().flush().unwrap();
    let mut release = [0];
    std::io::stdin().read_exact(&mut release).unwrap();
    assert_eq!(release, [1]);
    std::process::exit(0);
}

struct Child {
    _temp: tempfile::TempDir,
    root: PathBuf,
    process: tokio::process::Child,
}
impl Child {
    fn spawn(spent: bool) -> Self {
        let temp = private_root();
        let root = temp.path().canonicalize().unwrap();
        let mut command = tokio::process::Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                CHILD,
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD_ROOT, &root)
            .env_remove("CONNECT_CONSENT_OWNER_TEST_SPENT")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true);
        if spent {
            command.env("CONNECT_CONSENT_OWNER_TEST_SPENT", "1");
        }
        Self {
            _temp: temp,
            root,
            process: command.spawn().unwrap(),
        }
    }
    async fn ready(&mut self) -> ChildReady {
        let output = self.process.stdout.take().unwrap();
        let mut lines = tokio::io::BufReader::new(output).lines();
        while let Some(line) = lines.next_line().await.unwrap() {
            if let Some(start) = line.find(READY) {
                return serde_json::from_str(&line[start + READY.len()..]).unwrap();
            }
        }
        panic!("child exited without actual issuing-owner observation");
    }
    async fn lose_owner(&mut self) {
        self.process
            .stdin
            .take()
            .unwrap()
            .write_all(&[1])
            .await
            .unwrap();
        assert!(
            self.process.wait().await.unwrap().success(),
            "actual old owner exited"
        );
    }
    async fn cleanup(&mut self) {
        if self.process.try_wait().unwrap().is_none() {
            self.process.kill().await.unwrap();
        }
        self.process.wait().await.unwrap();
    }
}

struct FaultStore {
    inner: Arc<dyn ApprovalStore>,
    unknown_write: AtomicBool,
    fail_read: AtomicBool,
}
impl ApprovalStore for FaultStore {
    fn load_all(&self) -> Result<Vec<ApprovalRecord>, ApprovalStoreError> {
        if self.fail_read.load(Ordering::SeqCst) {
            return Err(ApprovalStoreError::NotCommitted(
                "fixture read unavailable".into(),
            ));
        }
        self.inner.load_all()
    }
    fn get(&self, id: &ApprovalId) -> Result<Option<ApprovalRecord>, ApprovalStoreError> {
        self.inner.get(id)
    }
    fn put_if_current(
        &self,
        prior: Option<&ApprovalRecord>,
        row: &ApprovalRecord,
    ) -> Result<(), ApprovalStoreError> {
        if self.unknown_write.swap(false, Ordering::SeqCst) {
            return Err(ApprovalStoreError::CommitUnknown(
                "fixture unknown commit".into(),
            ));
        }
        self.inner.put_if_current(prior, row)
    }
    fn is_persistent(&self) -> bool {
        self.inner.is_persistent()
    }
}

async fn scenario(spent: bool, fault: bool) {
    let mut child = Child::spawn(spent);
    let caught = AssertUnwindSafe(async {
        tokio::time::timeout(Duration::from_secs(60), async {
            let ready = child.ready().await;
            let mut wrong = identity();
            wrong.deployment = Id::new("other-deployment").unwrap();
            assert!(matches!(ApplicationConsentOwner::reopen(ready.selection.clone(), &wrong), Err(ConsentOwnerError::WrongApplication)));
            wrong = identity();
            wrong.realm = RealmId::parse("other-realm").unwrap();
            assert!(matches!(ApplicationConsentOwner::reopen(ready.selection.clone(), &wrong), Err(ConsentOwnerError::WrongApplication)));
            wrong = identity();
            wrong.application = PrincipalRef::in_domain(PrincipalKind::RuntimeHost, "other-application",
                TrustDomainId::new("connect-consent-control").unwrap()).unwrap();
            assert!(matches!(ApplicationConsentOwner::reopen(ready.selection.clone(), &wrong), Err(ConsentOwnerError::WrongApplication)));
            assert!(matches!(ApplicationConsentOwner::reopen(ready.selection.clone(), &identity()), Err(ConsentOwnerError::OwnerHeld)));
            assert!(matches!(grant_session_hosting(&hosting(&child.root), &HostingOwner::mint(), &ready.association.scope().session_id),
                Err(HostingRefused::ServedElsewhere(_))), "real old claim remains held");
            child.lose_owner().await;
            // Read the actual persisted rows after process loss, before the
            // new owner can settle anything. Ordinary Rust drop cannot make
            // the lost pending/approved-unspent oracle vacuously pass.
            let path = ready.selection.artifact().path.parent().unwrap().join("decisions.json");
            let stored = FileApprovalStore::open(path).unwrap();
            assert_eq!(stored.get(&ready.pending.approval_id).unwrap(), Some(ready.pending.clone()));
            assert_eq!(stored.get(&ready.approved.approval_id).unwrap(), Some(ready.approved.clone()));
            drop(stored);
            let selected_fault = Mutex::new(None);
            let owner = ApplicationConsentOwner::reopen_with_store(ready.selection.clone(), &identity(), |inner| {
                let store = Arc::new(FaultStore { inner, unknown_write: AtomicBool::new(false), fail_read: AtomicBool::new(false) });
                *selected_fault.lock().unwrap() = Some(store.clone());
                store
            }).unwrap();
            let retained_service = owner.approvals();
            let claim = grant_session_hosting(&hosting(&child.root), &HostingOwner::mint(), &ready.association.scope().session_id).unwrap();
            let other_session = grant_session_hosting(&hosting(&child.root), &HostingOwner::mint(), &meerkat_core::SessionId::new()).unwrap();
            assert!(matches!(owner.settle_abandoned_scope(&ready.association, &other_session), Err(ConsentOwnerError::WrongClaim)));
            let other = ApplicationConsentOwner::create(&child.root, identity()).unwrap();
            assert!(matches!(other.settle_abandoned_scope(&ready.association, &claim), Err(ConsentOwnerError::InvalidSelection)));
            drop(other);
            if spent {
                assert!(matches!(owner.settle_abandoned_scope(&ready.association, &claim), Err(ConsentOwnerError::Spent(id))
                    if id == ready.spent.as_ref().unwrap().approval_id));
                assert_eq!(owner.approvals().get(&ready.pending.approval_id).unwrap(), ready.pending);
                assert_eq!(owner.approvals().get(&ready.approved.approval_id).unwrap(), ready.approved);
            } else if fault {
                let selected = selected_fault.lock().unwrap().clone().unwrap();
                selected.fail_read.store(true, Ordering::SeqCst);
                assert!(matches!(owner.settle_abandoned_scope(&ready.association, &claim),
                    Err(ConsentOwnerError::Store(ApprovalStoreError::NotCommitted(_)))));
                selected.fail_read.store(false, Ordering::SeqCst);
                selected.unknown_write.store(true, Ordering::SeqCst);
                assert!(matches!(owner.settle_abandoned_scope(&ready.association, &claim),
                    Err(ConsentOwnerError::Approval(meerkat_core::ApprovalError::StoreCommitUnknown { .. }))));
                assert!(!fs::read_dir(&owner.directory).unwrap().any(|entry| entry.unwrap().file_name().to_string_lossy().starts_with("disposition-")),
                    "partial/unknown native settlement emits no successful disposition");
            } else {
                let receipt = owner.settle_abandoned_scope(&ready.association, &claim).unwrap();
                let (status, rows) = owner.read_disposition(&ready.association, &receipt).unwrap();
                assert_eq!(status, RecoveryJoinStatus::AwaitingNativeFinalizationAndPhysicalSettlement);
                assert_eq!(rows.len(), 2);
                let pending = rows.iter().find(|row| row.approval_id == ready.pending.approval_id).unwrap();
                let approved = rows.iter().find(|row| row.approval_id == ready.approved.approval_id).unwrap();
                assert_eq!(pending.status, ApprovalStatus::Cancelled);
                assert_eq!(approved.status, ApprovalStatus::Approved);
                assert_eq!(approved.decision, ready.approved.decision);
                assert_eq!(pending.expires_at, ready.pending.expires_at);
                assert!(rows.iter().all(|row| row.consent.as_ref().unwrap().consumed_at.is_none()));
                assert!(matches!(claim.reserve_owner_loss(), OwnerLossAttachment::Reserved(_)),
                    "row-only receipt did not complete native registration's reservation");
                let repeat = owner.settle_abandoned_scope(&ready.association, &claim).unwrap();
                assert_eq!(receipt, repeat, "empty newly-settled IDs retain complete same historical evidence");
                ordinary(&owner, &meerkat_core::SessionId::new());
                assert_eq!(owner.read_disposition(&ready.association, &receipt).unwrap().1, rows,
                    "unrelated issuer writes do not rewrite immutable association history");
                let probe = Arc::new(Probe::default());
                let current = context(&owner, &claim);
                assert!(meerkat_core::dispatch_retained_consent_attempt(&probe, &current, &ready.approved.approval_id).await.is_err());
                assert_eq!(probe.0.load(Ordering::SeqCst), 0, "lost approval never restored executable custody");
            }
            assert_eq!(retained_service.get(&ready.ordinary.approval_id).unwrap(), ready.ordinary);
            assert!(ready.ordinary.consent.is_none(), "historical ordinary request remains ordinary");
            drop(owner);
            assert!(matches!(ApplicationConsentOwner::reopen(ready.selection.clone(), &identity()), Err(ConsentOwnerError::OwnerHeld)),
                "a retained service clone still owns the issuer lease");
            drop(retained_service);
            drop(selected_fault);
            let reopened = ApplicationConsentOwner::reopen(ready.selection, &identity()).unwrap();
            assert_eq!(reopened.identity(), &identity());
        }).await.expect("bounded consent owner control");
    }).catch_unwind().await;
    // Join the actual child even when setup, an assertion or the timeout
    // failed. An observer timing out is not a child completion receipt.
    child.cleanup().await;
    if let Err(panic) = caught {
        resume_unwind(panic);
    }
}

#[tokio::test]
async fn persistent_consent_owner_recovers_lost_unspent_history_without_restoring_custody() {
    scenario(false, false).await;
}
#[tokio::test]
async fn persistent_consent_owner_refuses_spent_association_without_withdrawing_unspent_rows() {
    scenario(true, false).await;
}
#[tokio::test]
async fn persistent_consent_owner_preserves_store_failure_and_unknown_settlement() {
    scenario(false, true).await;
}

#[test]
fn persistent_consent_selection_never_replaces_missing_or_changed_issuer_history() {
    let temp = private_root();
    let root = temp.path().canonicalize().unwrap();
    let owner = ApplicationConsentOwner::create(&root, identity()).unwrap();
    let selection = owner.selection().clone();
    let session = meerkat_core::SessionId::new();
    let claim = grant_session_hosting(&hosting(&root), &HostingOwner::mint(), &session).unwrap();
    let association = owner
        .bind_association(
            artifact(&root.join("ordinary-association.json"), b"{}"),
            &claim,
        )
        .unwrap();
    let historical = ordinary(&owner, &session);
    assert!(matches!(
        owner.settle_abandoned_scope(&association, &claim),
        Err(ConsentOwnerError::MissingConsent)
    ));
    assert_eq!(
        owner.approvals().get(&historical.approval_id).unwrap(),
        historical,
        "ordinary consent=None history is never upgraded into scoped consent"
    );
    let path = owner.directory.join("decisions.json");
    drop(owner);
    let retained = fs::read(&path).unwrap();
    fs::rename(&path, root.join("retained-decisions.json")).unwrap();
    assert!(ApplicationConsentOwner::reopen(selection.clone(), &identity()).is_err());
    assert!(
        !path.exists(),
        "reopen must not make an empty replacement store"
    );
    create_file(&path, &retained).unwrap();
    let mut changed = selection.clone();
    changed.manifest.sha256 = "0".repeat(64);
    assert!(ApplicationConsentOwner::reopen(changed, &identity()).is_err());
    ApplicationConsentOwner::reopen(selection, &identity()).unwrap();
}

#[path = "provenance_tests.rs"]
mod provenance_tests;

#[path = "operation_review_tests.rs"]
mod operation_review_tests;

#[path = "io_custody_tests.rs"]
mod io_custody_tests;

#[path = "installed_review_tests.rs"]
mod installed_review_tests;
