//! Actual installed policy, private SDK enrollment, native factory input and
//! Meta resolver. The model barrier coordinates the test only; it is never an
//! authentication or eligibility input. No Domain work or authored receipts.
use super::*;
use crate::application_consent::{ApplicationConsentOwner, ConsentApplicationIdentity};
use crate::application_gateway::console_consent::{
    ConsoleConsentRoute, ConsoleConsentRule, ConsoleConsentSelection, tests as route_tests,
};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{HeaderMap, Request, StatusCode},
};
use meerkat_core::{
    ApprovalDecision, ApprovalRecord, ApprovalStatus,
    approval::{
        ApprovalListFilter,
        review::{OperationReviewer, ReviewCandidate, ReviewVerdict, ReviewerFailure},
    },
};
use meerkat_mobkit::{
    ConsoleSessionFences,
    access::{AccessControlConfig, AccessController, AccessRule},
};
use std::{
    path::PathBuf,
    sync::OnceLock,
    time::{SystemTime, UNIX_EPOCH},
};
use tower::ServiceExt;
mod entered_operation;
const ISSUER: &str = "http://controlled-console.localhost";
const SUBJECT: &str = "controlled-subject";
const EMAIL: &str = "different-display@example.invalid";
const ORIGIN: &str = "http://localhost:19191";
const KEY: &[u8] = b"controlled-console-fixture-key-only";
fn human() -> PrincipalRef {
    PrincipalRef::in_domain(
        PrincipalKind::Human,
        SUBJECT,
        TrustDomainId::new(format!("oidc-issuer:{ISSUER}")).unwrap(),
    )
    .unwrap()
}
fn rules() -> Vec<ConsoleConsentRule> {
    vec![
        ConsoleConsentRule::for_target(AgentIdentity::parse("original-a").unwrap(), vec![human()])
            .unwrap(),
    ]
}
struct NoModelReview;
#[async_trait]
impl OperationReviewer for NoModelReview {
    async fn review(&self, _: &ReviewCandidate<'_>) -> Result<ReviewVerdict, ReviewerFailure> {
        panic!("the exact selected R3 Find must not invoke R2 model review")
    }
}
pub(super) struct ConsentControl {
    root: PathBuf,
    r2: Option<Arc<entered_operation::ExactFindReview>>,
    owner: Mutex<Option<Arc<ApplicationConsentOwner>>>,
    route: OnceLock<Arc<ConsoleConsentRoute>>,
    router: OnceLock<Router>,
    fences: OnceLock<ConsoleSessionFences>,
    access: OnceLock<AccessController>,
    blocked: tokio::sync::Semaphore,
    release: tokio::sync::Semaphore,
    barrier_entered: AtomicBool,
    first_results: OnceLock<Vec<meerkat_core::ToolResult>>,
}
impl toolkit_mobkit::installed_host::InstalledOperationReviewPolicy for ConsentControl {
    fn review_floor(
        &self,
        context: toolkit_mobkit::installed_host::InstalledOperationReviewContext<'_>,
    ) -> Result<
        meerkat_core::authorization::OperationReviewTier,
        meerkat_core::OperationAuthorizationError,
    > {
        let owner = self
            .owner
            .lock()
            .map_err(|_| meerkat_core::OperationAuthorizationError::Unavailable)?;
        let owner = owner
            .as_ref()
            .ok_or(meerkat_core::OperationAuthorizationError::Unavailable)?;
        let selected = crate::application_consent::fixture_review_floor::find(
            owner,
            context,
            &["original-a", "original-b"],
        )?;
        // Only the new explicit R2 fixture changes this trusted selection.
        // The exact installed application/member checks above still run.
        Ok(
            if self.r2.is_some() && selected == meerkat_core::authorization::OperationReviewTier::R3
            {
                meerkat_core::authorization::OperationReviewTier::R2
            } else {
                selected
            },
        )
    }
}
impl ConsentControl {
    fn new(root: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            root,
            r2: None,
            owner: Mutex::new(None),
            route: OnceLock::new(),
            router: OnceLock::new(),
            fences: OnceLock::new(),
            access: OnceLock::new(),
            blocked: tokio::sync::Semaphore::new(0),
            release: tokio::sync::Semaphore::new(0),
            barrier_entered: AtomicBool::new(false),
            first_results: OnceLock::new(),
        })
    }
    pub(super) fn compose(
        &self,
        host: ToolkitGatewayHost,
    ) -> Result<ToolkitGatewayHost, meerkat_toolkit::host_lifetime::control::HostLifetimeError>
    {
        use meerkat_toolkit::host_lifetime::control::HostLifetimeError;
        let identity = host.console_setup_binding()?.with_consent_host(|owner| {
            ConsentApplicationIdentity::from_installed(owner)
                .map_err(|_| HostLifetimeError::Unavailable)
        })?;
        let owner = ApplicationConsentOwner::create(&self.root, identity)
            .map_err(|_| HostLifetimeError::Unavailable)?;
        let reviewer: Arc<dyn OperationReviewer> = match &self.r2 {
            Some(reviewer) => reviewer.clone(),
            None => Arc::new(NoModelReview),
        };
        let selection = ConsoleConsentSelection::new(
            owner.clone(),
            reviewer,
            Duration::from_secs(30),
            Duration::from_secs(60),
            ORIGIN.into(),
            rules(),
        )?;
        let selection = match &self.r2 {
            Some(reviewer) => selection.with_test_observer(reviewer.clone()),
            None => selection,
        };
        let selected_control = selection.control();
        let (host, route, composed_owner) = selection.compose(host)?;
        assert!(Arc::ptr_eq(&owner, &composed_owner));
        // This fixture retains the very route returned by actual composition.
        // It never constructs the private route or supplies an authenticated human.
        self.route.set(route.clone()).ok().unwrap();
        assert!(Arc::ptr_eq(&selected_control.route()?, &route));
        *self.owner.lock().unwrap() = Some(composed_owner);
        host.augment_console_routes(Arc::new(move |actual| route.routes(actual)))
    }
    pub(super) fn configure_runtime(&self, runtime: &mut UnifiedRuntime) {
        let fences = ConsoleSessionFences::open(self.root.join("console-fences.json")).unwrap();
        runtime.set_console_session_fences(fences.clone());
        self.fences.set(fences).ok().unwrap();
        let access = AccessController::new(AccessControlConfig {
            enabled: true,
            admins: vec!["unrelated-admin".into()],
            rules: vec![AccessRule {
                id: "consent-send".into(),
                subjects: vec![EMAIL.into()],
                actions: vec!["agent.view".into(), "agent.send".into()],
                agents: vec!["original-a".into()],
                ..Default::default()
            }],
            ..Default::default()
        })
        .unwrap();
        runtime.set_access_controller(access.clone());
        self.access.set(access).ok().unwrap();
    }
    pub(super) fn bind_routes(&self, runtime: &UnifiedRuntime, host: &ToolkitGatewayHost) {
        let decisions = meerkat_mobkit::console_auth_config::parse_console_auth_config(&json!({
            "provider":"jwt", "shared_secret":std::str::from_utf8(KEY).unwrap(),
            "issuer":ISSUER, "audience":"controlled-console", "email_allowlist":[EMAIL]
        }))
        .unwrap();
        let actual = runtime.console_viewer_resolver(&decisions);
        self.router
            .set(
                host.host_app_routes(actual)
                    .unwrap()
                    .expect("selected actual host route"),
            )
            .ok()
            .unwrap();
    }
    pub(super) async fn response(
        &self,
        script: &PlanningScript,
        request: &LlmRequest,
    ) -> Vec<LlmEvent> {
        if let Some(r2) = &self.r2 {
            return r2.response(script, request).await;
        }
        let matching = request
            .messages
            .iter()
            .flat_map(|message| match message {
                Message::ToolResults { results, .. } => results.iter().collect::<Vec<_>>(),
                _ => vec![],
            })
            .filter(|row| row.tool_use_id == "a-planning-find")
            .collect::<Vec<_>>();
        if matching.is_empty() {
            if serde_json::to_string(&request.messages)
                .unwrap()
                .contains("sdk-planning-b")
            {
                let arguments = script.calls.lock().unwrap()["a-planning-find"].clone();
                assert!(
                    script
                        .calls
                        .lock()
                        .unwrap()
                        .insert("b-planning-find".into(), arguments.clone())
                        .is_none()
                );
                return vec![
                    LlmEvent::ToolCallComplete {
                        id: "a-planning-find".into(),
                        name: toolkit_mobkit::FIND_AND_INSTALL.into(),
                        args: arguments,
                        meta: None,
                    },
                    LlmEvent::Done {
                        outcome: LlmDoneOutcome::success(StopReason::ToolUse),
                    },
                ];
            }
            return script.response(request);
        }
        // Core publishes a retained attempt as typed consent_attempt notice
        // content. The earlier call keeps its local refusal; it never receives
        // a second ToolResults row (agent/state.rs::consent_attempt_notice).
        let attempts = request
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
                    && payload.get("tool_use_id") == Some(&json!("a-planning-find")) =>
                {
                    Some((payload, content))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(attempts.len() <= 1, "one exact retained-attempt notice");
        if let Some((payload, content)) = attempts.first() {
            let approval_id: meerkat_core::approval::ApprovalId =
                serde_json::from_value(payload["approval_id"].clone()).unwrap();
            // Correlate to this native history's actual R3 refusal, not just a
            // same-named tool with equal arguments in another input.
            let refused = matching
                .iter()
                .filter_map(|result| {
                    if !result.is_error {
                        return None;
                    }
                    let value: Value = serde_json::from_str(&result.text_content()).unwrap();
                    (value["error"] == "review_unsatisfied"
                        && value["data"]["kind"] == "human_consent_required")
                        .then(|| {
                            serde_json::from_value::<meerkat_core::approval::ApprovalId>(
                                value["data"]["approval_id"].clone(),
                            )
                            .unwrap()
                        })
                })
                .collect::<Vec<_>>();
            assert_eq!(
                refused,
                vec![approval_id.clone()],
                "notice must match this exact native refusal"
            );
            let actual = self.issuer().approvals().get(&approval_id).unwrap();
            assert_eq!(
                actual.proposed_action.body.as_ref(),
                script.calls.lock().unwrap().get("a-planning-find")
            );
            assert_eq!(payload["tool_name"], "find_and_install");
            assert!(
                payload["is_error"].is_boolean(),
                "typed attempt outcome required"
            );
            if payload["is_error"] == false {
                assert_eq!(actual.status, ApprovalStatus::Approved);
                assert!(actual.consent.as_ref().unwrap().consumed_at.is_some());
                assert_eq!(payload.as_object().unwrap().len(), 4, "no unsettled result");
                let [meerkat_core::ContentBlock::Text { text }] = content.as_slice() else {
                    panic!("actual Find must return one typed text result");
                };
                let value = serde_json::from_str(text).unwrap();
                script
                    .results
                    .lock()
                    .unwrap()
                    .insert("a-planning-find".into(), value);
                return done("The exact consented native Find returned.");
            }
        }
        if !self.barrier_entered.swap(true, Ordering::AcqRel) {
            // Report the actual observation even when it is unexpected. The
            // outer test retains and checks it before any consent action or
            // release, so a provider panic cannot hide behind a barrier timeout.
            self.first_results
                .set(matching.into_iter().cloned().collect())
                .ok()
                .unwrap();
            self.blocked.add_permits(1);
            self.release.acquire().await.unwrap().forget();
            assert!(
                request
                    .tools
                    .iter()
                    .any(|tool| tool.name == "tool_catalog_search")
            );
            // A real harmless native catalog turn advances Core's loop. Core
            // then spends any approved exact retained candidate itself.
            return vec![
                LlmEvent::ToolCallComplete {
                    id: "consent-next-boundary".into(),
                    name: "tool_catalog_search".into(),
                    args: json!({"query":"find","limit":1}),
                    meta: None,
                },
                LlmEvent::Done {
                    outcome: LlmDoneOutcome::success(StopReason::ToolUse),
                },
            ];
        }
        done("No consented Find entry occurred.")
    }
    pub(super) async fn before_waiter_turns(
        &self,
        planning: &PlanningCase,
        case: &Case,
        observations: &[Value],
    ) {
        if let Some(r2) = &self.r2 {
            r2.before_waiter_turns(planning, case, observations).await;
        }
    }
    fn issuer(&self) -> Arc<ApplicationConsentOwner> {
        self.owner.lock().unwrap().as_ref().unwrap().clone()
    }
    async fn http_raw(&self, path: &str, token: &str, body: Value) -> (StatusCode, Vec<u8>) {
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
        (status, raw.to_vec())
    }
    async fn http(&self, path: &str, token: &str, body: Value) -> (StatusCode, Value) {
        let (status, raw) = self.http_raw(path, token, body).await;
        // Successful consent reads/decisions must still return actual JSON.
        (status, serde_json::from_slice(&raw).unwrap())
    }
    async fn http_status(&self, path: &str, token: &str, body: Value) -> StatusCode {
        // Middleware and extractor rejections may have empty or plain-text
        // bodies. These callers assert status and unchanged issuer state.
        self.http_raw(path, token, body).await.0
    }
}
fn b64(raw: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw)
}
fn token(scope: &str, now: u64) -> String {
    use hmac::{Hmac, Mac};
    let header = b64(br#"{"alg":"HS256","typ":"JWT"}"#);
    let payload = b64(
        &serde_json::to_vec(&json!({"iss":ISSUER,"aud":"controlled-console",
        "sub":SUBJECT,"email":EMAIL,"email_verified":true,"sid":scope,
        "iat":now-1,"exp":now+120}))
        .unwrap(),
    );
    let unsigned = format!("{header}.{payload}");
    let mut hmac = Hmac::<Sha256>::new_from_slice(KEY).unwrap();
    hmac.update(unsigned.as_bytes());
    format!("Bearer {unsigned}.{}", b64(&hmac.finalize().into_bytes()))
}
fn headers(token: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("authorization", token.parse().unwrap());
    headers
}
async fn enroll(case: &Case, scope: &str, now: u64) -> Value {
    let reply = case
        .client
        .lock()
        .await
        .call(json!({"kind":"enroll_console_login","login":{
            "issuer":ISSUER,"subject":SUBJECT,"login_scope_id":scope,"expires_at_ms":(now+120)*1000
        }}))
        .await
        .unwrap();
    assert_eq!(reply["kind"], "console_login_enrolled");
    reply["enrollment_id"].clone()
}
#[derive(Clone, Copy, Debug)]
enum Change {
    Approve,
    Deny,
    Login,
    MetaAfterRenewal,
    Rules,
    RetireIssuer,
    Access,
    Scope,
    EventPoison,
    HistoricalReset,
    ParkedWrongInput,
    PendingDrop,
}
async fn exercise(
    case: &mut Case,
    planning: &PlanningCase,
    control: &Arc<ConsentControl>,
    change: Change,
) {
    let launch = case.launch.as_ref().unwrap().clone();
    let reply = case
        .client
        .lock()
        .await
        .call(json!({"kind":"submit_authenticated_original","submission":planning.parent}))
        .await
        .unwrap();
    let admitted: PrivateOriginalSubmission =
        serde_json::from_value(reply["outcome"].clone()).unwrap();
    let PrivateOriginalSubmission::Admitted {
        turn_ticket: Some(ticket),
        ..
    } = admitted
    else {
        panic!("P admission")
    };
    await_turn(case, "planning-parent", &ticket).await;
    let selected = planning
        .source
        .prepare_workflow_planning(
            &planning.parent.original,
            planning.lead.clone(),
            planning.terms.clone(),
            planning.lineage.clone(),
        )
        .await
        .unwrap();
    launch.select_planning(selected).unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let login = enroll(case, "login-a", now).await;
    let a = token("login-a", now);
    let content = "sdk-planning-a: prepare this independently authenticated request".to_string();
    let submitted = launch
        .launch(
            meerkat_workgraph::WorkOwnerKey::agent("sdk-original-a").unwrap(),
            source_event("original-a", "sdk-planning-a", &content, vec![]),
            content,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(submitted.route, id("original-a"));
    tokio::time::timeout(BOUND, control.blocked.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    let first_results = control.first_results.get().unwrap();
    // Keep private diagnostic bytes outside the Case TempDir so failure cleanup
    // cannot erase the exact native refusal. Only its path reaches the log.
    use std::os::unix::fs::OpenOptionsExt;
    let evidence = tempfile::Builder::new()
        .prefix("connect-consent-refusal-")
        .tempdir()
        .unwrap();
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(evidence.path().join("first-tool-results.json"))
        .unwrap();
    serde_json::to_writer_pretty(&file, first_results).unwrap();
    file.sync_all().unwrap();
    let evidence = evidence.keep();
    eprintln!("initial consent evidence: {}", evidence.display());
    assert_eq!(
        first_results.len(),
        1,
        "one exact initial Find result required"
    );
    let result = &first_results[0];
    assert!(result.is_error, "initial Find must require actual consent");
    let refused: Value = serde_json::from_str(&result.text_content()).unwrap();
    assert_eq!(refused["error"], "review_unsatisfied");
    assert_eq!(refused["data"]["kind"], "human_consent_required");
    let approval_id: meerkat_core::approval::ApprovalId =
        serde_json::from_value(refused["data"]["approval_id"].clone()).unwrap();
    let owner = control.issuer();
    let approvals = owner.approvals();
    let rows = approvals.list(ApprovalListFilter::default()).unwrap();
    assert_eq!(rows.len(), 1);
    let row = rows[0].clone();
    assert_eq!(row.status, ApprovalStatus::Pending);
    assert_eq!(
        row.approval_id, approval_id,
        "the pending row must be the native refusal's exact record"
    );
    assert_eq!(
        row.proposed_action.body.as_ref(),
        planning.script.calls.lock().unwrap().get("a-planning-find")
    );
    let scoped = row.consent.as_ref().unwrap();
    assert!(scoped.origin.is_some());
    assert_eq!(
        scoped.scope.as_ref().unwrap().session_id,
        case.audit_sessions["original-a"]
    );
    assert!(scoped.consumed_at.is_none());
    assert!(planning.script.results.lock().unwrap().is_empty());
    let route = control.route.get().unwrap().clone();
    let proof = route_tests::evidence(&route, &headers(&a), &row)
        .await
        .unwrap();
    proof.check_current().unwrap();
    let (status, read) = control
        .http(
            "/toolkit/consent/read",
            &a,
            json!({"approval_id":row.approval_id}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(read["approval"], json!(row));
    let mut decision = Some(ApprovalDecision::Deny);
    match change {
        Change::Approve => decision = Some(ApprovalDecision::Approve),
        Change::Deny | Change::HistoricalReset => (),
        Change::Login => {
            let reply = case
                .client
                .lock()
                .await
                .call(json!({"kind":"retire_console_login","enrollment_id":login}))
                .await
                .unwrap();
            assert_eq!(reply["kind"], "console_login_retired");
            decision = None;
        }
        Change::MetaAfterRenewal => {
            enroll(case, "login-b", now).await;
            let b = token("login-b", now);
            let b_proof = route_tests::evidence(&route, &headers(&b), &row)
                .await
                .unwrap();
            let a_after_b = route_tests::evidence(&route, &headers(&a), &row)
                .await
                .unwrap();
            a_after_b.check_current().unwrap();
            b_proof.check_current().unwrap();
            control
                .fences
                .get()
                .unwrap()
                .revoke_sessions(EMAIL, SystemTime::now())
                .unwrap();
            assert!(b_proof.check_current().is_err());
            assert!(a_after_b.check_current().is_err());
            decision = None;
        }
        Change::Rules => {
            route.replace_rules(rules()).unwrap();
            decision = None;
        }
        Change::RetireIssuer => {
            route.retire().unwrap();
            decision = None;
        }
        Change::Access => {
            control
                .access
                .get()
                .unwrap()
                .delete_rule("consent-send")
                .unwrap();
            decision = None;
        }
        Change::Scope => {
            let mut foreign = row.clone();
            foreign
                .consent
                .as_mut()
                .unwrap()
                .scope
                .as_mut()
                .unwrap()
                .session_id = case.audit_sessions["original-b"].clone();
            assert!(
                route_tests::evidence(&route, &headers(&a), &foreign)
                    .await
                    .is_err()
            );
            let mut args = row.clone();
            args.proposed_action.body = Some(json!({"request":"changed"}));
            // The real issuer row, not caller data, reaches the decision route.
            assert_eq!(
                control
                    .http_status(
                        "/toolkit/consent/decide",
                        &a,
                        json!({"approval_id":row.approval_id,
                "decision":"approve","actor":"caller","approval":args})
                    )
                    .await,
                StatusCode::UNPROCESSABLE_ENTITY
            );
        }
        Change::EventPoison => {
            route_tests::poisoned_event_cleanup(&route, &headers(&a), &row).await;
            decision = None;
        }
        Change::PendingDrop | Change::ParkedWrongInput => decision = None,
    }
    if !matches!(
        change,
        Change::Approve
            | Change::Deny
            | Change::Scope
            | Change::HistoricalReset
            | Change::ParkedWrongInput
            | Change::PendingDrop
    ) {
        assert!(proof.check_current().is_err());
        assert_ne!(
            control
                .http_status(
                    "/toolkit/consent/decide",
                    &a,
                    json!({"approval_id":row.approval_id,"decision":"approve"})
                )
                .await,
            StatusCode::OK
        );
        assert!(
            approvals
                .get(&row.approval_id)
                .unwrap()
                .consent
                .unwrap()
                .consumed_at
                .is_none()
        );
    }
    if let Some(decision) = decision {
        let (status, decided) = control
            .http(
                "/toolkit/consent/decide",
                &a,
                json!({"approval_id":row.approval_id,"decision":decision}),
            )
            .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "actual authenticated event: {decided}"
        );
        let decided: ApprovalRecord = serde_json::from_value(decided["approval"].clone()).unwrap();
        assert_eq!(
            decided.status,
            if decision == ApprovalDecision::Approve {
                ApprovalStatus::Approved
            } else {
                ApprovalStatus::Denied
            }
        );
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
            decided.consent.unwrap().consumed_at.is_none(),
            "decision alone cannot enter"
        );
        assert!(planning.script.results.lock().unwrap().is_empty());
    }
    assert!(route_tests::no_retained_events(&route));
    if matches!(change, Change::PendingDrop) {
        let pending = approvals.get(&row.approval_id).unwrap();
        assert_eq!(pending.status, ApprovalStatus::Pending);
        assert!(pending.consent.as_ref().unwrap().withdrawal.is_none());
        assert!(pending.consent.as_ref().unwrap().consumed_at.is_none());
        // Leave the actual model/input and consent candidate held. The runner
        // releases only after the real private owner publishes closure.
        return;
    }
    control.release.add_permits(1);
    let observation = launch
        .observations()
        .into_iter()
        .find(|row| row["original"]["route"] == "original-a")
        .unwrap();
    await_turn(case, "original-a", observation["ticket"].as_str().unwrap()).await;
    let final_row = approvals.get(&row.approval_id).unwrap();
    assert_eq!(final_row.proposed_action, row.proposed_action);
    assert_eq!(
        final_row.consent.as_ref().unwrap().consumed_at.is_some(),
        matches!(change, Change::Approve)
    );
    assert_eq!(
        planning.script.results.lock().unwrap().len(),
        usize::from(matches!(change, Change::Approve))
    );
    if matches!(change, Change::Approve) {
        assert_eq!(
            planning.script.results.lock().unwrap()["a-planning-find"]["worker"]["state"],
            "workflow_lead"
        );
    }
    if matches!(change, Change::ParkedWrongInput) {
        assert_eq!(final_row.status, ApprovalStatus::Pending);
        let parked = final_row.consent.as_ref().unwrap();
        assert!(parked.withdrawal.is_none() && parked.consumed_at.is_none());
        assert!(matches!(
            parked.custody.as_ref(),
            Some(meerkat_core::approval::ApprovalConsentCustody::Parked)
        ));
        let (status, approved) = control
            .http(
                "/toolkit/consent/decide",
                &a,
                json!({"approval_id":row.approval_id,"decision":"approve"}),
            )
            .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "actual parked human decision: {approved}"
        );
        // Approval of A's parked call cannot serve as B's consent, even when
        // the provider repeats the identical tool ID and exact arguments.
        let content = "sdk-planning-b: independent native work".to_string();
        launch
            .launch(
                meerkat_workgraph::WorkOwnerKey::agent("sdk-original-b").unwrap(),
                source_event("original-b", "sdk-planning-b", &content, vec![]),
                content,
            )
            .await
            .unwrap()
            .unwrap();
        let observed = launch
            .observations()
            .into_iter()
            .find(|row| row["original"]["route"] == "original-b")
            .unwrap();
        await_turn(case, "original-b", observed["ticket"].as_str().unwrap()).await;
        let all = approvals.list(ApprovalListFilter::default()).unwrap();
        let b = all
            .iter()
            .filter(|record| {
                record
                    .consent
                    .as_ref()
                    .and_then(|c| c.scope.as_ref())
                    .is_some_and(|scope| scope.session_id == case.audit_sessions["original-b"])
            })
            .collect::<Vec<_>>();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].status, ApprovalStatus::Pending);
        assert_eq!(b[0].proposed_action, row.proposed_action);
        assert_eq!(b[0].owner, row.owner);
        assert_ne!(b[0].approval_id, row.approval_id);
        assert!(b[0].consent.as_ref().unwrap().consumed_at.is_none());
        assert_eq!(input_count(case, "original-b").await, 1);
        // Any delivery/transfer of A's own approved notice belongs to A and
        // is outside this wrong-input control; it cannot become B's entry.
        assert!(
            !history(case, "original-b")
                .await
                .to_string()
                .contains("workflow_lead")
        );
    }
    if matches!(change, Change::HistoricalReset) {
        let runtime = case.native.as_ref().unwrap().identity_runtime().unwrap();
        let a = AgentIdentity::parse("original-a").unwrap();
        let b = AgentIdentity::parse("original-b").unwrap();
        let old = case.audit_sessions["original-a"].clone();
        let b_status = runtime.status(&b).await.unwrap();
        assert!(!b_status.intent_managed);
        runtime.retire_tracked(&a).await.unwrap();
        runtime.retire_tracked(&b).await.unwrap();
        assert_eq!(
            runtime.core_session_owner(&old).await.unwrap(),
            Some(a.clone())
        );
        assert_eq!(
            route_tests::historical_target(&route, &row).await.unwrap(),
            a
        );
        let handle = case.native.as_ref().unwrap().mob_runtime().handle();
        let member_b = MemberIdentity::from(b_status.member_id.unwrap());
        // Meta retirement releases materialization but keeps the member seated.
        // End B's actual Core incarnation before testing a fresh resume of A's
        // retained session; A's retained owner and session are not rewritten.
        assert!(
            handle
                .list_all_members()
                .await
                .iter()
                .any(|entry| entry.agent_identity == member_b),
            "Meta release must retain B's actual roster membership"
        );
        handle.retire(member_b.clone()).await.unwrap();
        assert!(
            !handle
                .list_all_members()
                .await
                .iter()
                .any(|entry| entry.agent_identity == member_b),
            "Core retirement must finish before the durable resume probe"
        );
        let attempted = handle
            .spawn_spec(
                SpawnMemberSpec::new(b_status.profile.unwrap(), member_b)
                    .with_runtime_mode(MobRuntimeMode::TurnDriven)
                    .with_launch_mode(meerkat_mob::MemberLaunchMode::Resume {
                        bridge_session_id: old.clone(),
                        resume_from_role: Some(ProfileName::from("original-a")),
                    }),
            )
            .await;
        let refusal = attempted.expect_err("B cannot resume A's actual persisted session");
        assert!(
            refusal
                .to_string()
                .contains("requested mob/member identity"),
            "the actual durable identity check must refuse, not an earlier gate: {refusal}"
        );
        assert_eq!(
            runtime.core_session_owner(&old).await.unwrap(),
            Some(a.clone())
        );
        assert_ne!(
            runtime.status(&b).await.unwrap().state,
            IdentityLifecycleState::Active
        );
        let reset = runtime.reset_tracked(&a).await.unwrap();
        let fresh = reset.session_id().expect("actual reset native session");
        assert_ne!(fresh, &old);
        assert_eq!(
            runtime.core_session_owner(&old).await.unwrap(),
            Some(a.clone())
        );
        assert_eq!(
            runtime.core_session_owner(fresh).await.unwrap(),
            Some(a.clone())
        );
        assert_eq!(
            route_tests::historical_target(&route, &row).await.unwrap(),
            a
        );
        assert_eq!(
            approvals.get(&row.approval_id).unwrap(),
            final_row,
            "reset does not relabel or spend the retained exact old decision"
        );
    }
    planning.cancel_parent(case).await;
}
fn run(changes: &[Change]) {
    for &change in changes {
        let (pipes, parent) = SdkParent::pair();
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .unwrap()
            .block_on(async move {
                let mut case = Case::new(parent.enter());
                let control = ConsentControl::new(case.directory.path().canonicalize().unwrap());
                let checked = std::panic::AssertUnwindSafe(async {
                    let planning =
                        construction::construct_with_consent(&mut case, pipes, control.clone())
                            .await;
                    exercise(&mut case, &planning, &control, change).await;
                })
                .catch_unwind()
                .await;
                if checked.is_err() {
                    control.release.add_permits(8);
                    case.retain_failed_audits().await;
                }
                let cleanup = if checked.is_ok() && matches!(change, Change::PendingDrop) {
                    let closed = case.private.as_ref().unwrap().closed_observer();
                    let was_closed = closed.is_closed_for_presenter();
                    let (cleanup, observed) = tokio::join!(case.cleanup(), async {
                        let observed = tokio::time::timeout(BOUND, closed.wait_closed()).await;
                        // On failure also release the controlled provider so the
                        // existing cleanup future is joined before asserting.
                        control.release.add_permits(8);
                        observed
                    });
                    assert!(
                        !was_closed,
                        "pending candidate must still have its real private owner"
                    );
                    assert!(
                        observed.is_ok(),
                        "real private closure must precede provider release"
                    );
                    cleanup
                } else {
                    control.release.add_permits(8);
                    case.cleanup().await
                };
                assert!(
                    cleanup.is_empty(),
                    "actual SDK/native owner joins: {cleanup:?}"
                );
                if let Err(panic) = checked {
                    std::panic::resume_unwind(panic);
                }
                // Keep route/router alive while the actual host/factory and issuer
                // are gone. A pending parked candidate must not create a lease cycle.
                let owner = control.owner.lock().unwrap().take().unwrap();
                let selection = owner.selection().clone();
                let identity = owner.identity().clone();
                drop(owner);
                let reopened = ApplicationConsentOwner::reopen(selection, &identity).unwrap();
                assert!(route_tests::issuer_gone(control.route.get().unwrap()));
                drop(reopened);
            });
    }
}
#[test]
fn installed_console_consent_uses_actual_runtime_input_and_exact_entry() {
    run(&[Change::Approve, Change::Deny]);
}
#[test]
fn installed_console_consent_rechecks_human_login_and_selected_policy() {
    run(&[
        Change::Login,
        Change::MetaAfterRenewal,
        Change::Rules,
        Change::RetireIssuer,
        Change::Access,
        Change::Scope,
        Change::EventPoison,
    ]);
}
#[test]
fn pending_console_consent_releases_issuer_after_native_shutdown() {
    run(&[Change::PendingDrop]);
}

#[test]
fn installed_console_consent_preserves_historical_owner_across_reset() {
    run(&[Change::HistoricalReset]);
}

#[test]
fn parked_console_consent_cannot_authorize_another_native_input() {
    run(&[Change::ParkedWrongInput]);
}
