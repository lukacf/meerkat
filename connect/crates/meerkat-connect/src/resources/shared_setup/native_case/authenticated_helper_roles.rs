//! One actual authenticated R3 Find issues all five ordinary helper roles.
//! The controlled model supplies only tool calls. The installed policy, real
//! Console resolver, persistent consent issuer and native retained attempt own
//! review/entry; no local authenticated-principal or entered witness is made.
use super::*;
use crate::application_consent::{ApplicationConsentOwner, ConsentApplicationIdentity};
use crate::application_gateway::console_consent::{
    ConsoleConsentRoute, ConsoleConsentRule, ConsoleConsentSelection, PreparedConsoleConsentForTest,
};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use meerkat_core::{
    ApprovalDecision, ApprovalRecord, ApprovalStatus,
    approval::{
        ApprovalListFilter,
        review::{
            BoundOperationReview, OperationReviewObserver, OperationReviewer, ReviewAttemptStatus,
            ReviewCandidate, ReviewObservation, ReviewRetirementReason, ReviewVerdict,
            ReviewerFailure,
        },
    },
};
use meerkat_mobkit::rpc_gateway::GatewayHostComposition;
use meerkat_mobkit::{
    ConsoleSessionFences,
    access::{AccessControlConfig, AccessController, AccessRule},
};
use std::{
    sync::OnceLock,
    time::{SystemTime, UNIX_EPOCH},
};
use toolkit_mobkit::installed_host::{
    InstalledOperationReviewContext, InstalledOperationReviewPolicy,
    preparation::{PreparedHelperEvidence, PreparedHelperReadback, PreparedHelperSnapshot},
};
use tower::ServiceExt;

mod manual_recovery;

const CALL: &str = "authenticated-five-role-find";
const TEXT: &str = "sdk-child-a: prepare one ordinary helper with the selected five role parents";
const ISSUER: &str = "http://controlled-helper-console.localhost";
const SUBJECT: &str = "controlled-helper-subject";
const EMAIL: &str = "helper-console-display@example.invalid";
const ORIGIN: &str = "http://localhost:19192";
const KEY: &[u8] = b"controlled-helper-console-fixture-key-only";
fn human() -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::Human,
        SUBJECT,
        TrustDomainId::new(format!("oidc-issuer:{ISSUER}")).unwrap(),
    )
    .unwrap()
}
fn arguments() -> Value {
    json!({"request": TEXT})
}
struct NoModelReview;
#[async_trait]
impl OperationReviewer for NoModelReview {
    async fn review(&self, _: &ReviewCandidate<'_>) -> Result<ReviewVerdict, ReviewerFailure> {
        panic!("this actual installed Find is R3, not a setup-parent or R2 approval")
    }
}
#[derive(Default)]
struct Observations(Mutex<Vec<ReviewObservation>>);
impl OperationReviewObserver for Observations {
    fn observe(&self, row: ReviewObservation) {
        self.0.lock().unwrap().push(row);
    }
}

pub(super) struct Control {
    owner: OnceLock<Arc<ApplicationConsentOwner>>,
    early: Mutex<Option<PreparedConsoleConsentForTest>>,
    review: OnceLock<Arc<BoundOperationReview>>,
    route: OnceLock<Arc<ConsoleConsentRoute>>,
    router: OnceLock<Router>,
    observed: Arc<Observations>,
    readback: OnceLock<PreparedHelperReadback>,
    selections: OnceLock<Vec<InstalledSetupSelection>>,
    blocked: tokio::sync::Semaphore,
    resume: tokio::sync::Semaphore,
    helper_entered: tokio::sync::Semaphore,
    helper_release: tokio::sync::Semaphore,
    completed: tokio::sync::Semaphore,
    refusal: OnceLock<meerkat_core::ToolResult>,
    result: OnceLock<Value>,
    helper_calls: AtomicUsize,
    barrier: AtomicBool,
    promotion_diagnostic: AtomicBool,
    manual_recovery: Option<Arc<manual_recovery::Recovery>>,
    promotion_probe: Mutex<Option<toolkit_mobkit::private_originals::OriginalPromotionProbe>>,
}
impl Control {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            owner: OnceLock::new(),
            early: Mutex::new(None),
            review: OnceLock::new(),
            route: OnceLock::new(),
            router: OnceLock::new(),
            observed: Arc::new(Observations::default()),
            readback: OnceLock::new(),
            selections: OnceLock::new(),
            blocked: tokio::sync::Semaphore::new(0),
            resume: tokio::sync::Semaphore::new(0),
            helper_entered: tokio::sync::Semaphore::new(0),
            helper_release: tokio::sync::Semaphore::new(0),
            completed: tokio::sync::Semaphore::new(0),
            refusal: OnceLock::new(),
            result: OnceLock::new(),
            helper_calls: AtomicUsize::new(0),
            barrier: AtomicBool::new(false),
            promotion_diagnostic: AtomicBool::new(false),
            manual_recovery: None,
            promotion_probe: Mutex::new(None),
        })
    }
    pub(super) fn arm_promotion_diagnostic(
        &self,
        source: &toolkit_mobkit::private_originals::PrivateOriginals,
    ) {
        if self.promotion_diagnostic.load(Ordering::SeqCst) {
            let probe = source.hold_next_original_promotion_for_test();
            assert!(
                self.promotion_probe
                    .lock()
                    .unwrap()
                    .replace(probe)
                    .is_none()
            );
        }
    }
    pub(super) fn prepare(&self, prepared: &PreparedInstallationOwner, root: &std::path::Path) {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let owner = ApplicationConsentOwner::create(
            root,
            ConsentApplicationIdentity::from_preparation(prepared).unwrap(),
        )
        .unwrap();
        // Retain the exact real issuer selection before any native factory use.
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(root.join("authenticated-helper-consent-selection.json"))
            .unwrap();
        file.write_all(&serde_json::to_vec(owner.selection()).unwrap())
            .unwrap();
        file.sync_all().unwrap();
        fs::File::open(root).unwrap().sync_all().unwrap();
        let selection = ConsoleConsentSelection::new(
            owner.clone(),
            Arc::new(NoModelReview),
            Duration::from_secs(30),
            Duration::from_secs(120),
            ORIGIN.into(),
            vec![
                ConsoleConsentRule::for_target(
                    AgentIdentity::parse("original-a").unwrap(),
                    vec![human()],
                )
                .unwrap(),
            ],
        )
        .unwrap()
        .with_test_observer(self.observed.clone());
        let early = selection.prepare_for_test(prepared).unwrap();
        assert!(early.route().is_err(), "unresolved actual route refuses");
        self.review.set(early.review().unwrap()).ok().unwrap();
        self.owner.set(owner).ok().unwrap();
        assert!(self.early.lock().unwrap().replace(early).is_none());
    }
    pub(super) fn review(&self) -> Arc<BoundOperationReview> {
        self.review.get().unwrap().clone()
    }
    pub(super) fn retain_readback(&self, readback: PreparedHelperReadback) {
        self.readback.set(readback).ok().unwrap();
    }
    pub(super) fn retain_selections(&self, selections: Vec<InstalledSetupSelection>) {
        assert_eq!(selections.len(), 2);
        for selected in &selections {
            let report = selected.report_capability.as_ref().unwrap();
            assert!(!selected.controller_parent.is_empty());
            assert!(report.account_parent.is_some() && report.public_parent.is_some());
            assert!(selected.catalog_capability.is_some());
        }
        self.selections.set(selections).ok().unwrap();
    }
    pub(super) fn compose(
        &self,
        host: ToolkitGatewayHost,
    ) -> Result<ToolkitGatewayHost, meerkat_toolkit::host_lifetime::control::HostLifetimeError>
    {
        let early = self
            .early
            .lock()
            .unwrap()
            .take()
            .expect("one exact prepared consent owner");
        let (host, route, owner) = early.compose(host)?;
        assert!(Arc::ptr_eq(&owner, self.owner.get().unwrap()));
        self.route.set(route.clone()).ok().unwrap();
        host.augment_console_routes(Arc::new(move |actual| route.routes(actual)))
    }
    pub(super) fn configure_runtime(&self, runtime: &mut UnifiedRuntime) {
        let root = self
            .owner
            .get()
            .unwrap()
            .selection()
            .artifact()
            .path
            .parent()
            .unwrap();
        runtime.set_console_session_fences(
            ConsoleSessionFences::open(root.join("helper-console-fences.json")).unwrap(),
        );
        runtime.set_access_controller(
            AccessController::new(AccessControlConfig {
                enabled: true,
                admins: vec!["unrelated-admin".into()],
                rules: vec![AccessRule {
                    id: "authenticated-helper-send".into(),
                    subjects: vec![EMAIL.into()],
                    actions: vec!["agent.view".into(), "agent.send".into()],
                    agents: vec!["original-a".into()],
                    ..Default::default()
                }],
                ..Default::default()
            })
            .unwrap(),
        );
    }
    pub(super) fn bind_routes(&self, runtime: &UnifiedRuntime, host: &ToolkitGatewayHost) {
        // Actual prepared adoption must precede this source observation.
        assert!(Arc::ptr_eq(
            &host.selected_operation_review().unwrap().unwrap(),
            &self.review()
        ));
        let decisions = meerkat_mobkit::console_auth_config::parse_console_auth_config(&json!({
            "provider":"jwt", "shared_secret": std::str::from_utf8(KEY).unwrap(),
            "issuer":ISSUER, "audience":"controlled-helper-console", "email_allowlist":[EMAIL]
        }))
        .unwrap();
        let actual = runtime.console_viewer_resolver(&decisions);
        self.router
            .set(host.host_app_routes(actual).unwrap().unwrap())
            .ok()
            .unwrap();
    }
    pub(super) async fn response(&self, script: &Script, request: &LlmRequest) -> Vec<LlmEvent> {
        let text = serde_json::to_string(&request.messages).unwrap();
        assert!(
            text.contains(TEXT),
            "only the actual selected Original/helper input"
        );
        if request
            .tools
            .iter()
            .any(|tool| tool.name == meerkat_toolkit::setup::SETUP_REPORT)
        {
            assert_eq!(self.helper_calls.fetch_add(1, Ordering::SeqCst), 0);
            self.helper_entered.add_permits(1);
            self.helper_release.acquire().await.unwrap().forget();
            return done(
                "The real helper entered; no account or public business effect was requested.",
            );
        }
        if let Some(recovery) = &self.manual_recovery
            && recovery.started.load(Ordering::SeqCst)
        {
            return recovery.response(self, request).await;
        }
        let matching = request
            .messages
            .iter()
            .flat_map(|message| match message {
                Message::ToolResults { results, .. } => results.iter().collect::<Vec<_>>(),
                _ => vec![],
            })
            .filter(|row| row.tool_use_id == CALL)
            .collect::<Vec<_>>();
        if matching.is_empty() {
            assert!(
                request
                    .tools
                    .iter()
                    .any(|tool| tool.name == toolkit_mobkit::FIND_AND_INSTALL)
            );
            let mut calls = script.calls.lock().unwrap();
            assert!(
                calls.is_empty(),
                "one native retained Find, never a replacement submit"
            );
            calls.push((
                CALL.into(),
                toolkit_mobkit::FIND_AND_INSTALL.into(),
                arguments(),
            ));
            return vec![
                LlmEvent::ToolCallComplete {
                    id: CALL.into(),
                    name: toolkit_mobkit::FIND_AND_INSTALL.into(),
                    args: arguments(),
                    meta: None,
                },
                LlmEvent::Done {
                    outcome: LlmDoneOutcome::success(StopReason::ToolUse),
                },
            ];
        }
        assert_eq!(matching.len(), 1);
        let notices = request
            .messages
            .iter()
            .filter_map(|message| match message {
                Message::SystemNotice(notice) => Some(&notice.blocks),
                _ => None,
            })
            .flatten()
            .filter_map(|block| match block {
                meerkat_core::SystemNoticeBlock::RuntimeNotice {
                    category,
                    payload: Some(payload),
                    content,
                    ..
                } if category == "consent_attempt"
                    && payload.get("tool_use_id") == Some(&json!(CALL)) =>
                {
                    Some((payload, content))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(notices.len() <= 1);
        if let Some((payload, content)) = notices.first() {
            let refused: Value = serde_json::from_str(&matching[0].text_content()).unwrap();
            assert_eq!(payload["approval_id"], refused["data"]["approval_id"]);
            assert_eq!(payload["tool_name"], toolkit_mobkit::FIND_AND_INSTALL);
            assert_eq!(
                payload["is_error"], false,
                "actual retained helper entry refused: {content:?}"
            );
            assert_eq!(
                payload.as_object().unwrap().len(),
                4,
                "settled native result required"
            );
            let [meerkat_core::ContentBlock::Text { text }] = content.as_slice() else {
                panic!("typed Find result")
            };
            let value: Value = serde_json::from_str(text).unwrap();
            self.result.set(value).ok().unwrap();
            self.completed.add_permits(1);
            return done(
                "The same authenticated retained Find completed its native helper admission.",
            );
        }
        assert!(
            !self.barrier.swap(true, Ordering::SeqCst),
            "no extra refused Find cycle"
        );
        self.refusal.set((*matching[0]).clone()).ok().unwrap();
        self.blocked.add_permits(1);
        self.resume.acquire().await.unwrap().forget();
        if self.promotion_diagnostic.load(Ordering::SeqCst) {
            if let Some(recovery) = &self.manual_recovery
                && recovery.requested.load(Ordering::SeqCst)
            {
                return recovery.begin(script, request);
            }
            // Diagnostic cleanup resumes only after its oracle (or failure).
            // Do not request a second Find or spend the withdrawn candidate.
            return done("The controlled custody publication comparison is complete.");
        }
        assert!(
            request
                .tools
                .iter()
                .any(|tool| tool.name == "tool_catalog_search")
        );
        // The native loop, not the test, retries its actual retained candidate.
        vec![
            LlmEvent::ToolCallComplete {
                id: "authenticated-helper-next-boundary".into(),
                name: "tool_catalog_search".into(),
                args: json!({"query":"find","limit":1}),
                meta: None,
            },
            LlmEvent::Done {
                outcome: LlmDoneOutcome::success(StopReason::ToolUse),
            },
        ]
    }
    async fn http(&self, path: &str, token: &str, body: Value) -> (StatusCode, Value) {
        let request = Request::builder()
            .method("POST")
            .uri(path)
            .header("origin", ORIGIN)
            .header("content-type", "application/json")
            .header("authorization", token)
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let response = self
            .router
            .get()
            .unwrap()
            .clone()
            .oneshot(request)
            .await
            .unwrap();
        let status = response.status();
        let raw = to_bytes(response.into_body(), 128 * 1024).await.unwrap();
        (status, serde_json::from_slice(&raw).unwrap())
    }
    async fn snapshot(&self) -> Vec<PreparedHelperEvidence> {
        let selected = &self.selections.get().unwrap()[0];
        assert_eq!(selected.member, member("helper-a"));
        tokio::time::timeout(BOUND, async {
            loop {
                match self
                    .readback
                    .get()
                    .unwrap()
                    .snapshot(selected)
                    .await
                    .unwrap()
                {
                    PreparedHelperSnapshot::Ready(rows) => return rows,
                    // The admitted helper may run before the real recovery
                    // owner persists Promoted. Await only that typed state;
                    // every read/identity/grant error above still fails.
                    PreparedHelperSnapshot::Pending => {
                        tokio::time::sleep(Duration::from_millis(10)).await
                    }
                }
            }
        })
        .await
        .expect("actual helper custody must settle within the existing bound")
    }
    fn pre_decision_failure_diagnostic(&self, pending: &ApprovalRecord) {
        use meerkat_core::approval::ApprovalConsentWithdrawalReason;
        use std::io::Write;
        // Observe only the row already returned by the real approval owner.
        // In particular, do not read again or retry a withdrawn candidate.
        let withdrawal = pending
            .consent
            .as_ref()
            .and_then(|consent| consent.withdrawal.as_ref());
        let reason = withdrawal.map(|withdrawal| match &withdrawal.reason {
            ApprovalConsentWithdrawalReason::RunEnded => "run_ended",
            ApprovalConsentWithdrawalReason::ContextChanged => "context_changed",
            ApprovalConsentWithdrawalReason::AuthorityChanged => "authority_changed",
            ApprovalConsentWithdrawalReason::Expired => "expired",
            ApprovalConsentWithdrawalReason::Abandoned => "abandoned",
            ApprovalConsentWithdrawalReason::NoticeInputEnded { .. } => "notice_input_ended",
        });
        // Clone scalar projections under the observer lock, then format/output
        // only after releasing it. No attempt IDs or operation payloads leave.
        let observed = {
            let rows = self
                .observed
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            rows.iter()
                .map(|row| (row.status, row.retirement))
                .collect::<Vec<_>>()
        };
        let observed = observed
            .into_iter()
            .map(|(status, retirement)| {
                let status = match status {
                    ReviewAttemptStatus::Pending => "pending",
                    ReviewAttemptStatus::Allowed => "allowed",
                    ReviewAttemptStatus::Denied => "denied",
                    ReviewAttemptStatus::Escalated => "escalated",
                    ReviewAttemptStatus::Unavailable => "unavailable",
                    ReviewAttemptStatus::Retired => "retired",
                    ReviewAttemptStatus::Used => "used",
                };
                let retirement = retirement.map(|reason| match reason {
                    ReviewRetirementReason::ContextChanged => "context_changed",
                    ReviewRetirementReason::AuthorityChanged => "authority_changed",
                    ReviewRetirementReason::DeadlineExpired => "deadline_expired",
                    ReviewRetirementReason::Abandoned => "abandoned",
                    ReviewRetirementReason::RunEnded => "run_ended",
                });
                json!({"status":status,"retirement":retirement})
            })
            .collect::<Vec<_>>();
        let diagnostic = json!({"event":"authenticated_helper_pre_decision_failure",
            "stage":"before_human_decision", "status":pending.status,
            "withdrawal_reason":reason,
            "withdrawn_at":withdrawal.map(|withdrawal| withdrawal.withdrawn_at.to_rfc3339()),
            "observations":observed});
        // Diagnostic I/O failure must not replace the original assertion.
        let _ = writeln!(std::io::stderr().lock(), "{diagnostic}");
    }
    fn release_all(&self) {
        drop(
            self.promotion_probe
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take(),
        );
        self.resume.add_permits(1);
        self.helper_release.add_permits(1);
        if let Some(recovery) = &self.manual_recovery {
            recovery.resume.add_permits(1);
        }
    }
}
impl InstalledOperationReviewPolicy for Control {
    fn review_floor(
        &self,
        context: InstalledOperationReviewContext<'_>,
    ) -> Result<
        meerkat_core::authorization::OperationReviewTier,
        meerkat_core::OperationAuthorizationError,
    > {
        crate::application_consent::fixture_review_floor::find(
            self.owner.get().unwrap(),
            context,
            &["original-a"],
        )
    }
}

pub(super) fn authorities(index: usize) -> PreparationAuthorities {
    let mut selected = super::authorities(index);
    let original = IDENTITIES[index];
    let helper = IDENTITIES[index + 2];
    let report = selected.helper_report.as_mut().unwrap();
    report.account_issuance_id = Some(evidence(&format!("{helper}-account")));
    report.public_issuance_id = Some(evidence(&format!("{helper}-public")));
    for (suffix, operation) in [
        ("account", meerkat_toolkit::setup::SETUP_ACCOUNT_REQUEST),
        (
            "public",
            meerkat_toolkit::setup::SETUP_PUBLIC_SERVICE_REQUEST,
        ),
    ] {
        selected.agent_setup[0]
            .helper_targets
            .push(SetupTargetRequest {
                target: Some(GrantTarget::RegisteredToolBundleOperation {
                    bundle: meerkat_toolkit::setup::SETUP_ACCOUNTS_BUNDLE.into(),
                    operation: operation.into(),
                }),
                issuance_id: evidence(&format!("{original}-{suffix}")),
            });
    }
    selected.agent_setup[0]
        .helper_targets
        .sort_by(|a, b| a.target.cmp(&b.target));
    selected
}
// Account/public parent and child grants are selected explicitly above. This
// fixture keeps the existing real MCP/report/catalog tool surface; it does not
// name an unregistered account bundle or claim account/public method execution.
fn token(scope: &str, now: u64) -> String {
    use base64::Engine;
    use hmac::{Hmac, Mac};
    let encode = |bytes: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let header = encode(br#"{"alg":"HS256","typ":"JWT"}"#);
    let payload = encode(
        &serde_json::to_vec(&json!({"iss":ISSUER,"aud":"controlled-helper-console",
        "sub":SUBJECT,"email":EMAIL,"email_verified":true,"sid":scope,"iat":now-1,"exp":now+120}))
        .unwrap(),
    );
    let unsigned = format!("{header}.{payload}");
    let mut hmac = Hmac::<Sha256>::new_from_slice(KEY).unwrap();
    hmac.update(unsigned.as_bytes());
    format!(
        "Bearer {unsigned}.{}",
        encode(&hmac.finalize().into_bytes())
    )
}
async fn wait(semaphore: &tokio::sync::Semaphore) {
    tokio::time::timeout(BOUND, semaphore.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
}
async fn exercise(case: &mut Case, control: &Arc<Control>) {
    let launch = case.launch.as_ref().unwrap().clone();
    assert!(
        launch.selected_planning().unwrap().is_none(),
        "ordinary helper branch required"
    );
    assert!(control.snapshot().await.is_empty());
    launch.create_parent().await;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let login = case.client.lock().await.call(json!({"kind":"enroll_console_login","login":{
        "issuer":ISSUER,"subject":SUBJECT,"login_scope_id":"five-role-human","expires_at_ms":(now+120)*1000
    }})).await.unwrap();
    assert_eq!(login["kind"], "console_login_enrolled");
    let auth = token("five-role-human", now);
    let event = PrivateAuthenticatedEvent {
        route: id("original-a"),
        original: evidence("authenticated-five-role-source"),
        occurrence: evidence("authenticated-five-role-occurrence"),
        contributors: vec![],
        content_sha256: digest(TEXT.as_bytes()),
        original_authentication: PrivateSourceEvidence {
            resource_id: evidence("authenticated-five-role-transport"),
            revision: evidence("1"),
            sha256: digest(b"controlled actual private SDK transport"),
        },
        root_event: PrivateSourceEvidence {
            resource_id: evidence("authenticated-five-role-root"),
            revision: evidence("1"),
            sha256: digest(TEXT.as_bytes()),
        },
    };
    let original = launch
        .launch(
            meerkat_workgraph::WorkOwnerKey::agent("sdk-original-a").unwrap(),
            event,
            TEXT.into(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(original.route, id("original-a"));
    wait(&control.blocked).await;
    let refusal = control.refusal.get().unwrap();
    assert!(refusal.is_error);
    let refused: Value = serde_json::from_str(&refusal.text_content()).unwrap();
    assert_eq!(refused["error"], "review_unsatisfied");
    assert_eq!(refused["data"]["kind"], "human_consent_required");
    let approval_id = serde_json::from_value(refused["data"]["approval_id"].clone()).unwrap();
    let approvals = control.owner.get().unwrap().approvals();
    let rows = approvals.list(ApprovalListFilter::default()).unwrap();
    assert_eq!(rows.len(), 1);
    let pending = approvals.get(&approval_id).unwrap();
    if pending.status != ApprovalStatus::Pending {
        control.pre_decision_failure_diagnostic(&pending);
    }
    assert_eq!(pending.status, ApprovalStatus::Pending);
    assert_eq!(pending.proposed_action.body.as_ref(), Some(&arguments()));
    let consent = pending.consent.as_ref().unwrap();
    assert!(consent.consumed_at.is_none());
    assert_eq!(
        consent.scope.as_ref().unwrap().session_id,
        case.audit_sessions["original-a"]
    );
    let observations = launch.observations();
    assert_eq!(observations.len(), 1);
    assert_eq!(observations[0]["original"], json!(original));
    let input = observations[0]["input"].as_str().unwrap();
    let selected = consent
        .origin
        .as_ref()
        .unwrap()
        .retained_work
        .selected_input_bindings();
    assert!(selected.contains_key(input));
    assert!(control.snapshot().await.is_empty());
    assert_eq!(control.helper_calls.load(Ordering::SeqCst), 0);
    assert_ne!(
        control
            .http(
                "/toolkit/consent/decide",
                "Bearer invalid",
                json!({"approval_id":approval_id,"decision":"approve"})
            )
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(approvals.get(&approval_id).unwrap(), pending);
    let (status, read) = control
        .http(
            "/toolkit/consent/read",
            &auth,
            json!({"approval_id":approval_id}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(read["approval"], json!(pending));
    let (status, decided) = control
        .http(
            "/toolkit/consent/decide",
            &auth,
            json!({"approval_id":approval_id,"decision":ApprovalDecision::Approve}),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "real authenticated decision: {decided}"
    );
    let decided: ApprovalRecord = serde_json::from_value(decided["approval"].clone()).unwrap();
    assert_eq!(decided.status, ApprovalStatus::Approved);
    assert_eq!(
        decided
            .consent
            .as_ref()
            .unwrap()
            .approver
            .as_ref()
            .unwrap()
            .principal,
        human()
    );
    assert!(
        decided.consent.as_ref().unwrap().consumed_at.is_none(),
        "decision alone is not entry"
    );
    assert!(control.snapshot().await.is_empty());
    assert_eq!(control.helper_calls.load(Ordering::SeqCst), 0);
    control.resume.add_permits(1);
    wait(&control.helper_entered).await;
    wait(&control.completed).await;
    let result = control.result.get().unwrap();
    assert_eq!(result["worker"]["state"], "input_admitted");
    assert_eq!(
        result["worker"]["source_settlement"],
        "fresh_acceptance_promoted"
    );
    let consumed = approvals.get(&approval_id).unwrap();
    assert_eq!(consumed.proposed_action, pending.proposed_action);
    assert_eq!(consumed.consent.as_ref().unwrap().scope, consent.scope);
    assert_eq!(consumed.consent.as_ref().unwrap().origin, consent.origin);
    assert!(consumed.consent.as_ref().unwrap().consumed_at.is_some());
    assert_eq!(
        approvals.list(ApprovalListFilter::default()).unwrap().len(),
        1
    );
    let mut snapshots = control.snapshot().await;
    assert_eq!(snapshots.len(), 1);
    let snapshot = snapshots.pop().unwrap();
    assert_eq!(snapshot.record.data.body["state"]["phase"], "bound");
    assert_eq!(
        snapshot.record.data.body["custody_attempt"]["decision"],
        "promoted"
    );
    assert_eq!(snapshot.roles.len(), 5);
    let mut names = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for role in &snapshot.roles {
        assert!(names.insert(role.role));
        assert!(ids.insert(role.issued.grant_id.clone()));
        assert_eq!(role.lineage.len(), role.parent.len() + 1);
        assert!(role.lineage.starts_with(&role.parent));
        assert_eq!(role.lineage.last(), Some(&role.issued));
        assert_eq!(
            role.parent_restrictions.for_child(&role.requested).unwrap(),
            role.child_restrictions
        );
    }
    assert_eq!(
        names,
        BTreeSet::from(["controller", "report", "account", "public", "catalog"])
    );
    assert_eq!(ids.len(), 5);
    assert_eq!(control.helper_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        case.script.calls.lock().unwrap().as_slice(),
        &[(
            CALL.to_owned(),
            toolkit_mobkit::FIND_AND_INSTALL.to_owned(),
            arguments()
        )]
    );
    let session = &case.audit_sessions["helper-a"];
    assert_eq!(
        snapshot.association.candidate().target.context.as_str(),
        session.to_string()
    );
    control.helper_release.add_permits(1);
    let runtime_id = meerkat_runtime::identifiers::LogicalRuntimeId::for_session(session);
    tokio::time::timeout(BOUND, async {
        loop {
            let rows = case
                .audit_store
                .as_ref()
                .unwrap()
                .load_input_states_strict(&runtime_id)
                .await
                .unwrap();
            assert_eq!(rows.len(), 1);
            let row = &rows[0];
            assert_eq!(row.state.authority_contributors.len(), 1);
            assert_eq!(
                row.state.authority_contributors[0].input_id(),
                &row.state.input_id
            );
            assert_eq!(
                row.state.authority_contributors[0].association(),
                &snapshot.association
            );
            if row.seed.phase == meerkat_runtime::input_state::InputLifecycleState::Consumed {
                assert_eq!(
                    row.seed.terminal_outcome,
                    Some(meerkat_runtime::input_state::InputTerminalOutcome::Consumed)
                );
                assert!(row.seed.last_run_id.is_some());
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        control.snapshot().await[0].record,
        snapshot.record,
        "no replacement source/intent after completion"
    );
    assert_eq!(
        approvals
            .get(&approval_id)
            .unwrap()
            .consent
            .as_ref()
            .unwrap()
            .consumed_at,
        consumed.consent.as_ref().unwrap().consumed_at,
        "one native consent spend"
    );
    let other = meerkat_runtime::identifiers::LogicalRuntimeId::for_session(
        &case.audit_sessions["helper-b"],
    );
    assert!(
        case.audit_store
            .as_ref()
            .unwrap()
            .load_input_states_strict(&other)
            .await
            .unwrap()
            .is_empty()
    );
}

#[test]
fn authenticated_r3_find_issues_all_five_real_helper_roles_from_one_retained_entry() {
    let (pipes, parent) = SdkParent::pair();
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            let mut case = Case::new(parent.enter());
            let control = Control::new();
            case.script = Arc::new(Script {
                authenticated_helper: Some(control.clone()),
                ..Script::default()
            });
            let body = std::panic::AssertUnwindSafe(async {
                case.construct(pipes).await;
                exercise(&mut case, &control).await;
            })
            .catch_unwind()
            .await;
            control.release_all();
            if body.is_err() {
                case.retain_failed_audits().await;
            }
            let cleanup = case.cleanup().await;
            if body.is_err() || !cleanup.is_empty() {
                // Keep the original issuer/store/audit and actual owner artifacts on
                // body failure even when cleanup succeeds. Cleanup still runs first;
                // the diagnostic above retains the pre-cleanup row's closed facts.
                let retained = case.directory.keep();
                eprintln!(
                    "authenticated helper failure evidence: {}",
                    retained.display()
                );
            }
            assert!(
                cleanup.is_empty(),
                "actual native/private/resource owners must settle: {cleanup:?}"
            );
            if let Err(payload) = body {
                std::panic::resume_unwind(payload);
            }
            tokio::time::timeout(BOUND, async {
                loop {
                    if control
                        .observed
                        .0
                        .lock()
                        .unwrap()
                        .iter()
                        .any(|row| row.status == ReviewAttemptStatus::Used)
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("actual joined review owner must publish Used");
            let rows = control.observed.0.lock().unwrap();
            assert!(!rows.is_empty());
            assert!(rows.iter().all(|row| row.tool_call_id.as_ref() == CALL));
            let used = rows
                .iter()
                .filter(|row| row.status == ReviewAttemptStatus::Used)
                .collect::<Vec<_>>();
            assert_eq!(
                used.len(),
                1,
                "one actual final native Used, not a callback count"
            );
            assert_eq!(rows.last().unwrap().status, ReviewAttemptStatus::Used);
            for row in rows
                .iter()
                .filter(|row| row.status == ReviewAttemptStatus::Retired)
            {
                assert_eq!(row.retirement, Some(ReviewRetirementReason::ContextChanged));
                assert_ne!(
                    row.attempt, used[0].attempt,
                    "no retirement after entered effects"
                );
            }
            assert_eq!(case.script.calls.lock().unwrap().len(), 1);
        });
}

// A separate characterization, not the canonical five-role success test.
// This diagnostic releases one real custody writer while its model is held.
async fn exercise_promotion_publication(case: &mut Case, control: &Arc<Control>) {
    use meerkat_core::approval::ApprovalConsentWithdrawalReason;
    let mut probe = control
        .promotion_probe
        .lock()
        .unwrap()
        .take()
        .expect("armed actual source probe");
    let launch = case.launch.as_ref().unwrap().clone();
    assert!(launch.selected_planning().unwrap().is_none());
    assert!(control.snapshot().await.is_empty());
    launch.create_parent().await;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let login = case.client.lock().await.call(json!({"kind":"enroll_console_login","login":{
        "issuer":ISSUER,"subject":SUBJECT,"login_scope_id":"five-role-human","expires_at_ms":(now+120)*1000
    }})).await.unwrap();
    assert_eq!(login["kind"], "console_login_enrolled");
    let event = PrivateAuthenticatedEvent {
        route: id("original-a"),
        original: evidence("authenticated-five-role-source"),
        occurrence: evidence("authenticated-five-role-occurrence"),
        contributors: vec![],
        content_sha256: digest(TEXT.as_bytes()),
        original_authentication: PrivateSourceEvidence {
            resource_id: evidence("authenticated-five-role-transport"),
            revision: evidence("1"),
            sha256: digest(b"controlled actual private SDK transport"),
        },
        root_event: PrivateSourceEvidence {
            resource_id: evidence("authenticated-five-role-root"),
            revision: evidence("1"),
            sha256: digest(TEXT.as_bytes()),
        },
    };
    let original = launch
        .launch(
            meerkat_workgraph::WorkOwnerKey::agent("sdk-original-a").unwrap(),
            event,
            TEXT.into(),
        )
        .await
        .unwrap()
        .unwrap();
    let entered = tokio::time::timeout(BOUND, probe.entered())
        .await
        .unwrap()
        .unwrap();
    assert!(
        entered.original == original,
        "only the real returned Original may be paused"
    );
    assert_eq!(entered.session, case.audit_sessions["original-a"]);
    wait(&control.blocked).await;
    let refusal = control.refusal.get().unwrap();
    assert!(refusal.is_error);
    let refused: Value = serde_json::from_str(&refusal.text_content()).unwrap();
    assert_eq!(refused["error"], "review_unsatisfied");
    assert_eq!(refused["data"]["kind"], "human_consent_required");
    let approval_id = serde_json::from_value(refused["data"]["approval_id"].clone()).unwrap();
    let approvals = control.owner.get().unwrap().approvals();
    let pending = approvals.get(&approval_id).unwrap();
    if pending.status != ApprovalStatus::Pending {
        control.pre_decision_failure_diagnostic(&pending);
    }
    assert_eq!(
        pending.status,
        ApprovalStatus::Pending,
        "real Pending before releasing the custody writer"
    );
    assert!(pending.proposed_action.body.as_ref() == Some(&arguments()));
    let consent = pending.consent.as_ref().unwrap();
    assert!(
        consent.consumed_at.is_none() && consent.withdrawal.is_none() && pending.decision.is_none()
    );
    assert!(consent.deliveries.is_empty());
    assert_eq!(consent.scope.as_ref().unwrap().session_id, entered.session);
    let observations = launch.observations();
    assert_eq!(observations.len(), 1);
    assert!(observations[0]["original"] == json!(original));
    assert_eq!(
        observations[0]["input"].as_str().unwrap(),
        entered.input_id.to_string()
    );
    assert!(
        consent
            .origin
            .as_ref()
            .unwrap()
            .retained_work
            .selected_input_bindings()
            .contains_key(&entered.input_id.to_string())
    );
    assert!(
        control.snapshot().await.is_empty(),
        "readback must work while the promotion is paused"
    );
    assert_eq!(control.helper_calls.load(Ordering::SeqCst), 0);
    let runtime_id = meerkat_runtime::identifiers::LogicalRuntimeId::for_session(&entered.session);
    let before_inputs = case
        .audit_store
        .as_ref()
        .unwrap()
        .load_input_states_strict(&runtime_id)
        .await
        .unwrap();
    assert_eq!(before_inputs.len(), 1);
    assert_eq!(before_inputs[0].state.input_id, entered.input_id);
    let observed_attempt = {
        let rows = control.observed.0.lock().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, ReviewAttemptStatus::Pending);
        assert_eq!(rows[0].tool_call_id.as_ref(), CALL);
        rows[0].attempt.clone()
    };
    // No model resume, human decision, new input, host close or recovery stop.
    probe.release();
    let written = tokio::time::timeout(BOUND, probe.completed())
        .await
        .unwrap()
        .unwrap();
    assert!(written.identity.original == original);
    assert_eq!(written.identity.session, entered.session);
    assert_eq!(written.identity.input_id, entered.input_id);
    assert_eq!(written.before.key, written.after.key);
    assert_eq!(written.after.revision, written.before.revision + 1);
    assert!(written.before.data.body["custody_attempt"]["decision"] == "pending");
    assert!(written.after.data.body["custody_attempt"]["decision"] == "promoted");
    let mut expected_attempt = written.before.data.body["custody_attempt"].clone();
    expected_attempt["decision"] = json!("promoted");
    expected_attempt["resolution_entered"] = json!(false);
    assert!(
        written.after.data.body["custody_attempt"] == expected_attempt,
        "only the real Promote transition may change the bound attempt"
    );
    let mut before_data = written.before.data.clone();
    let mut after_data = written.after.data.clone();
    before_data
        .body
        .as_object_mut()
        .unwrap()
        .remove("custody_attempt");
    after_data
        .body
        .as_object_mut()
        .unwrap()
        .remove("custody_attempt");
    assert!(
        before_data == after_data,
        "canonical non-custody source and input bytes must be unchanged"
    );
    let after = approvals.get(&approval_id).unwrap();
    control.pre_decision_failure_diagnostic(&after);
    assert_eq!(
        after.status,
        ApprovalStatus::Cancelled,
        "characterize the same candidate after this exact completed custody publication"
    );
    assert_eq!(after.approval_id, pending.approval_id);
    assert!(after.proposed_action == pending.proposed_action && after.decision.is_none());
    let after_consent = after.consent.as_ref().unwrap();
    assert!(after_consent.scope == consent.scope && after_consent.origin == consent.origin);
    assert!(after_consent.consumed_at.is_none() && after_consent.approver.is_none());
    assert!(matches!(
        after_consent.withdrawal.as_ref().map(|w| &w.reason),
        Some(ApprovalConsentWithdrawalReason::ContextChanged)
    ));
    // This raw-spec fixture has no consent notice issuer on MobRuntimeDelivery.
    // Withdrawal still owes its exact native notice; no obligation is hidden.
    assert_eq!(after_consent.deliveries.len(), 1);
    assert_eq!(
        after_consent.deliveries[0].transition,
        meerkat_core::ApprovalConsentTransition::Withdrawn
    );
    assert_eq!(
        after_consent.deliveries[0].state,
        meerkat_core::ApprovalConsentDeliveryState::Owed
    );
    {
        let rows = control.observed.0.lock().unwrap();
        assert_eq!(rows.len(), 2);
        assert!(
            rows.iter()
                .all(|row| row.attempt == observed_attempt && row.tool_call_id.as_ref() == CALL)
        );
        assert_eq!(rows[1].status, ReviewAttemptStatus::Retired);
        assert_eq!(
            rows[1].retirement,
            Some(ReviewRetirementReason::ContextChanged)
        );
    }
    let after_inputs = case
        .audit_store
        .as_ref()
        .unwrap()
        .load_input_states_strict(&runtime_id)
        .await
        .unwrap();
    assert_eq!(after_inputs.len(), 1);
    assert_eq!(after_inputs[0].state.input_id, entered.input_id);
    assert!(control.snapshot().await.is_empty());
    assert_eq!(control.helper_calls.load(Ordering::SeqCst), 0);
    assert_eq!(case.script.calls.lock().unwrap().len(), 1);
    for name in ["helper-a", "helper-b"] {
        let runtime_id =
            meerkat_runtime::identifiers::LogicalRuntimeId::for_session(&case.audit_sessions[name]);
        assert!(
            case.audit_store
                .as_ref()
                .unwrap()
                .load_input_states_strict(&runtime_id)
                .await
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
#[ignore = "separate deterministic custody-publication diagnostic; not canonical case333"]
fn authenticated_r3_pending_consent_observes_one_actual_custody_promotion_publication() {
    let (pipes, parent) = SdkParent::pair();
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            let mut case = Case::new(parent.enter());
            let control = Control::new();
            control.promotion_diagnostic.store(true, Ordering::SeqCst);
            case.script = Arc::new(Script {
                authenticated_helper: Some(control.clone()),
                ..Script::default()
            });
            let body = std::panic::AssertUnwindSafe(tokio::time::timeout(BOUND * 2, async {
                case.construct(pipes).await;
                exercise_promotion_publication(&mut case, &control).await;
            }))
            .catch_unwind()
            .await;
            // A body-local probe releases on timeout/panic/drop. This also releases
            // a still-armed probe if construction failed, before any native close.
            control.release_all();
            let failed = !matches!(&body, Ok(Ok(())));
            if failed {
                case.retain_failed_audits().await;
            }
            let cleanup = case.cleanup().await;
            if failed || !cleanup.is_empty() {
                let retained = case.directory.keep();
                eprintln!(
                    "custody publication diagnostic evidence: {}",
                    retained.display()
                );
            }
            assert!(
                cleanup.is_empty(),
                "actual diagnostic owners must settle: {cleanup:?}"
            );
            match body {
                Ok(result) => result.expect("bounded real custody publication diagnostic"),
                Err(payload) => std::panic::resume_unwind(payload),
            }
        });
}
