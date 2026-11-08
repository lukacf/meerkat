//! Separate manual recovery qualification. No production retry policy or clock
//! override: one withdrawn candidate, one explicitly requested fresh Find,
//! one fresh authenticated approval after a real 61-second Pending interval.
use super::*;

const FRESH_CALL: &str = "manual-fresh-authenticated-find";
const HUMAN_WAIT: Duration = Duration::from_secs(61);
const RECOVERY_BOUND: Duration = Duration::from_secs(180);

pub(super) struct Recovery {
    pub(super) requested: AtomicBool,
    pub(super) started: AtomicBool,
    blocked: tokio::sync::Semaphore,
    pub(super) resume: tokio::sync::Semaphore,
    refusal: OnceLock<meerkat_core::ToolResult>,
}
impl Recovery {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            requested: AtomicBool::new(false),
            started: AtomicBool::new(false),
            blocked: tokio::sync::Semaphore::new(0),
            resume: tokio::sync::Semaphore::new(0),
            refusal: OnceLock::new(),
        })
    }
    pub(super) fn begin(&self, script: &Script, request: &LlmRequest) -> Vec<LlmEvent> {
        assert!(
            self.requested.load(Ordering::SeqCst),
            "explicit fixture manual action required"
        );
        assert!(!self.started.swap(true, Ordering::SeqCst));
        assert!(
            request
                .tools
                .iter()
                .any(|tool| tool.name == toolkit_mobkit::FIND_AND_INSTALL)
        );
        let mut calls = script.calls.lock().unwrap();
        assert_eq!(
            calls.as_slice(),
            &[(
                CALL.into(),
                toolkit_mobkit::FIND_AND_INSTALL.into(),
                arguments()
            )]
        );
        calls.push((
            FRESH_CALL.into(),
            toolkit_mobkit::FIND_AND_INSTALL.into(),
            arguments(),
        ));
        vec![
            LlmEvent::ToolCallComplete {
                id: FRESH_CALL.into(),
                name: toolkit_mobkit::FIND_AND_INSTALL.into(),
                args: arguments(),
                meta: None,
            },
            LlmEvent::Done {
                outcome: LlmDoneOutcome::success(StopReason::ToolUse),
            },
        ]
    }
    pub(super) async fn response(&self, control: &Control, request: &LlmRequest) -> Vec<LlmEvent> {
        let matching = request
            .messages
            .iter()
            .flat_map(|message| match message {
                Message::ToolResults { results, .. } => results.iter().collect::<Vec<_>>(),
                _ => vec![],
            })
            .filter(|row| row.tool_use_id == FRESH_CALL)
            .collect::<Vec<_>>();
        assert_eq!(
            matching.len(),
            1,
            "one real fresh Find result, no automatic retry"
        );
        let refused: Value = serde_json::from_str(&matching[0].text_content()).unwrap();
        assert!(matching[0].is_error);
        assert_eq!(refused["error"], "review_unsatisfied");
        assert_eq!(refused["data"]["kind"], "human_consent_required");
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
                    && payload.get("tool_use_id") == Some(&json!(FRESH_CALL)) =>
                {
                    Some((payload, content))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(notices.len() <= 1);
        if let Some((payload, content)) = notices.first() {
            assert_eq!(payload["approval_id"], refused["data"]["approval_id"]);
            assert_eq!(payload["tool_name"], toolkit_mobkit::FIND_AND_INSTALL);
            assert_eq!(
                payload["is_error"], false,
                "the fresh approved native entry must succeed"
            );
            assert_eq!(payload.as_object().unwrap().len(), 4);
            let [meerkat_core::ContentBlock::Text { text }] = content.as_slice() else {
                panic!("typed fresh Find result")
            };
            control
                .result
                .set(serde_json::from_str(text).unwrap())
                .ok()
                .unwrap();
            control.completed.add_permits(1);
            return done(
                "The explicitly requested fresh Find completed once after its own human decision.",
            );
        }
        self.refusal.set((*matching[0]).clone()).ok().unwrap();
        self.blocked.add_permits(1);
        self.resume.acquire().await.unwrap().forget();
        assert!(
            request
                .tools
                .iter()
                .any(|tool| tool.name == "tool_catalog_search")
        );
        vec![
            LlmEvent::ToolCallComplete {
                id: "manual-fresh-next-boundary".into(),
                name: "tool_catalog_search".into(),
                args: json!({"query":"find","limit":1}),
                meta: None,
            },
            LlmEvent::Done {
                outcome: LlmDoneOutcome::success(StopReason::ToolUse),
            },
        ]
    }
}

async fn exercise_recovery(case: &mut Case, control: &Arc<Control>, recovery: &Recovery) {
    // Reuse the complete genuine Pending -> Promoted publication comparison.
    // Its exact old-row, original/input, issuer, zero-helper-entry and owner
    // assertions run unchanged before asking for a fresh candidate.
    exercise_promotion_publication(case, control).await;
    let original_launches = case.launch.as_ref().unwrap().observations();
    assert_eq!(original_launches.len(), 1);
    let approvals = control.owner.get().unwrap().approvals();
    let rows = approvals.list(ApprovalListFilter::default()).unwrap();
    assert_eq!(rows.len(), 1);
    let old = rows[0].clone();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let auth = token("five-role-human", now);
    let (status, denied_read) = control
        .http(
            "/toolkit/consent/read",
            "Bearer invalid",
            json!({"approval_id":old.approval_id}),
        )
        .await;
    assert_ne!(status, StatusCode::OK);
    assert!(denied_read.get("recovery").is_none());
    let (status, read) = control
        .http(
            "/toolkit/consent/read",
            &auth,
            json!({"approval_id":old.approval_id}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(read["approval"], json!(old));
    assert_eq!(read["recovery"]["withdrawal"], "context_changed");
    assert_eq!(read["recovery"]["automatic_retry"], false);
    let (status, refused) = control
        .http(
            "/toolkit/consent/decide",
            &auth,
            json!({"approval_id":old.approval_id,"decision":"approve"}),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(refused.get("recovery").is_none());
    assert_eq!(approvals.get(&old.approval_id).unwrap(), old);
    assert_eq!(case.script.calls.lock().unwrap().len(), 1);
    assert_eq!(control.helper_calls.load(Ordering::SeqCst), 0);

    // This test-only explicit action supplies no approval or authority. The
    // same live original can request a new candidate here; no restart or
    // ended-notice restoration is inferred from this fixture.
    recovery.requested.store(true, Ordering::SeqCst);
    control.resume.add_permits(1);
    wait(&recovery.blocked).await;
    let refused: Value =
        serde_json::from_str(&recovery.refusal.get().unwrap().text_content()).unwrap();
    let fresh_id = serde_json::from_value(refused["data"]["approval_id"].clone()).unwrap();
    assert_ne!(fresh_id, old.approval_id);
    let fresh = approvals.get(&fresh_id).unwrap();
    assert_eq!(fresh.status, ApprovalStatus::Pending);
    assert_eq!(fresh.proposed_action, old.proposed_action);
    assert_eq!(
        old.owner,
        meerkat_core::approval::ApprovalOwnerRef::ToolCall {
            tool_call_id: meerkat_core::ToolCallId::new(CALL),
        }
    );
    assert_eq!(
        fresh.owner,
        meerkat_core::approval::ApprovalOwnerRef::ToolCall {
            tool_call_id: meerkat_core::ToolCallId::new(FRESH_CALL),
        }
    );
    assert_ne!(fresh.owner, old.owner);
    assert_eq!(fresh.requester, old.requester);
    assert_eq!(fresh.resource, old.resource);
    let consent = fresh.consent.as_ref().unwrap();
    assert!(
        consent.withdrawal.is_none() && consent.consumed_at.is_none() && fresh.decision.is_none()
    );
    assert_eq!(consent.scope, old.consent.as_ref().unwrap().scope);
    assert_eq!(consent.origin, old.consent.as_ref().unwrap().origin);
    assert_eq!(
        approvals.list(ApprovalListFilter::default()).unwrap().len(),
        2
    );
    assert_eq!(case.script.calls.lock().unwrap().len(), 2);
    assert_eq!(control.helper_calls.load(Ordering::SeqCst), 0);
    assert!(control.snapshot().await.is_empty());
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let expiry = u128::try_from(fresh.expires_at.as_ref().unwrap().timestamp_millis()).unwrap();
    assert!(
        expiry > now_ms + HUMAN_WAIT.as_millis(),
        "do not extend an existing consent deadline"
    );
    let started = std::time::Instant::now();
    tokio::time::sleep(HUMAN_WAIT).await;
    let elapsed = started.elapsed();
    assert!(
        elapsed >= HUMAN_WAIT,
        "real wall-clock human interval, no synthetic ReviewClock"
    );
    assert_eq!(
        approvals.get(&fresh_id).unwrap(),
        fresh,
        "exact pending row must survive the real wait"
    );
    assert_eq!(control.helper_calls.load(Ordering::SeqCst), 0);
    assert!(control.snapshot().await.is_empty());
    assert_eq!(approvals.get(&old.approval_id).unwrap(), old);
    let (status, read) = control
        .http(
            "/toolkit/consent/read",
            &auth,
            json!({"approval_id":fresh_id}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(read["approval"], json!(fresh));
    assert!(
        read.get("recovery").is_none(),
        "a live pending candidate is not a withdrawal"
    );
    let (status, decided) = control
        .http(
            "/toolkit/consent/decide",
            &auth,
            json!({"approval_id":fresh_id,"decision":ApprovalDecision::Approve}),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "new real human event must be accepted"
    );
    assert!(
        decided.get("recovery").is_none(),
        "a decision is not a recovery instruction"
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
    assert!(decided.consent.as_ref().unwrap().consumed_at.is_none());
    assert_eq!(control.helper_calls.load(Ordering::SeqCst), 0);
    recovery.resume.add_permits(1);
    wait(&control.helper_entered).await;
    wait(&control.completed).await;
    assert_eq!(
        control.result.get().unwrap()["worker"]["state"],
        "input_admitted"
    );
    assert_eq!(
        control.result.get().unwrap()["worker"]["source_settlement"],
        "fresh_acceptance_promoted"
    );
    let consumed = approvals.get(&fresh_id).unwrap();
    assert_eq!(consumed.proposed_action, fresh.proposed_action);
    assert_eq!(consumed.consent.as_ref().unwrap().scope, consent.scope);
    assert_eq!(consumed.consent.as_ref().unwrap().origin, consent.origin);
    assert!(consumed.consent.as_ref().unwrap().consumed_at.is_some());
    assert_eq!(approvals.get(&old.approval_id).unwrap(), old);
    assert_eq!(
        approvals.list(ApprovalListFilter::default()).unwrap().len(),
        2
    );
    assert_eq!(
        case.launch.as_ref().unwrap().observations(),
        original_launches,
        "this bounded live-original control never resubmits an input"
    );
    let snapshots = control.snapshot().await;
    assert_eq!(snapshots.len(), 1);
    assert_eq!(
        snapshots[0].record.data.body["custody_attempt"]["decision"],
        "promoted"
    );
    assert_eq!(
        snapshots[0]
            .roles
            .iter()
            .map(|role| role.role)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["controller", "report", "account", "public", "catalog"])
    );
    assert_eq!(control.helper_calls.load(Ordering::SeqCst), 1);
    let runtime_id = meerkat_runtime::identifiers::LogicalRuntimeId::for_session(
        &case.audit_sessions["helper-a"],
    );
    control.helper_release.add_permits(1);
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
            if rows[0].seed.phase == meerkat_runtime::input_state::InputLifecycleState::Consumed {
                assert_eq!(
                    rows[0].seed.terminal_outcome,
                    Some(meerkat_runtime::input_state::InputTerminalOutcome::Consumed)
                );
                assert!(rows[0].seed.last_run_id.is_some());
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        approvals
            .get(&fresh_id)
            .unwrap()
            .consent
            .unwrap()
            .consumed_at,
        consumed.consent.unwrap().consumed_at,
        "one actual fresh consent spend"
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
    eprintln!(
        "manual consent recovery observed_pending_ms={} fresh_entries=1",
        elapsed.as_millis()
    );

    // Replacing route selection intentionally fences even this legitimate
    // historical record. Recovery never widens old-approval access.
    control
        .route
        .get()
        .unwrap()
        .replace_rules(vec![
            ConsoleConsentRule::for_target(
                AgentIdentity::parse("original-a").unwrap(),
                vec![human()],
            )
            .unwrap(),
        ])
        .unwrap();
    let (status, read) = control
        .http(
            "/toolkit/consent/read",
            &auth,
            json!({"approval_id":old.approval_id}),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(read.get("recovery").is_none());
}

#[test]
#[ignore = "separate manual fresh-consent and real 61-second ordinary-window qualification"]
fn withdrawn_r3_requires_fresh_human_approval_after_real_sixty_seconds() {
    let (pipes, parent) = SdkParent::pair();
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            let mut case = Case::new(parent.enter());
            let recovery = Recovery::new();
            let mut owned = Arc::try_unwrap(Control::new()).ok().unwrap();
            owned.manual_recovery = Some(recovery.clone());
            owned.promotion_diagnostic.store(true, Ordering::SeqCst);
            let control = Arc::new(owned);
            case.script = Arc::new(Script {
                authenticated_helper: Some(control.clone()),
                ..Script::default()
            });
            let body = std::panic::AssertUnwindSafe(tokio::time::timeout(RECOVERY_BOUND, async {
                case.construct(pipes).await;
                exercise_recovery(&mut case, &control, &recovery).await;
            }))
            .catch_unwind()
            .await;
            control.release_all();
            let failed = !matches!(&body, Ok(Ok(())));
            if failed {
                case.retain_failed_audits().await;
            }
            let cleanup = case.cleanup().await;
            if failed || !cleanup.is_empty() {
                let retained = case.directory.keep();
                eprintln!("manual consent recovery evidence: {}", retained.display());
            }
            assert!(
                cleanup.is_empty(),
                "actual diagnostic owners must settle: {cleanup:?}"
            );
            match body {
                Ok(result) => result.expect("bounded manual fresh-consent qualification"),
                Err(payload) => std::panic::resume_unwind(payload),
            }
            tokio::time::timeout(BOUND, async {
                loop {
                    if control.observed.0.lock().unwrap().iter().any(|row| {
                        row.tool_call_id.as_ref() == FRESH_CALL
                            && row.status == ReviewAttemptStatus::Used
                    }) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("actual joined fresh review owner must publish Used");
            let rows = control.observed.0.lock().unwrap();
            assert_eq!(
                rows.iter()
                    .filter(|row| row.tool_call_id.as_ref() == CALL
                        && row.status == ReviewAttemptStatus::Used)
                    .count(),
                0
            );
            assert_eq!(
                rows.iter()
                    .filter(|row| row.tool_call_id.as_ref() == FRESH_CALL
                        && row.status == ReviewAttemptStatus::Used)
                    .count(),
                1
            );
            assert_eq!(case.script.calls.lock().unwrap().len(), 2);
        });
}
