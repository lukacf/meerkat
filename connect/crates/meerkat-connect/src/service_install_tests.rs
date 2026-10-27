//! Written controls for the exact package contracts and native approval seam.
//! The delegate is synthetic; these are not package installation acceptance.

use super::*;
use crate::service_install::CheckedInstalls;
use meerkat_core::runtime_bootstrap::RealmLocator;
use meerkat_core::{
    AgentToolDispatcher, ExecutionPolicyGatedDispatcher, ToolDeadlineChain,
    ToolDeadlineContributor, ToolDeadlineOwner, ToolDef, ToolExecutionPolicy,
    ToolExecutionResolutionContext, ToolMutationClass, ToolResult,
    ops::{ToolAccessPolicy, ToolDispatchOutcome},
};
use serde_json::{Value, json, value::RawValue};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

const NAMES: &[&str] = &[
    "setup_state",
    "setup_help",
    "setup_check",
    "setup_selected_check",
    "setup_install",
    "setup_selected_install",
    "setup_job",
    "setup_reconcile",
    "setup_stop",
    "setup_prepare",
    "research_fixture",
];

fn selection(selected: bool) -> Value {
    if selected {
        json!({"profile":"python", "staged_id":"fixture", "component":"mcp", "package_sha256":"2".repeat(64), "selection_sha256":"e".repeat(64)})
    } else {
        json!({"family":"services", "profile":"slack", "operations":["auth_test"], "values":{"token_env":"FIXTURE_SLACK_TOKEN", "timeout_seconds":2, "max_response_bytes":65536}})
    }
}

fn proposal(selected: bool) -> Value {
    if selected {
        json!({"scope":"selected_software_inputs_only", "validation_id":"b".repeat(32),
            "effect_sha256":"d".repeat(64), "profile":"python", "package":{"publisher":"fixture", "name":"fixture", "version":"1.0.0"},
            "component":"mcp", "selection_sha256":"e".repeat(64), "manifest_digest":"1".repeat(64),
            "files":{"package.json":"2".repeat(64), "activation.json":"e".repeat(64), "artifact.whl":"4".repeat(64), "closure/dependency.whl":"5".repeat(64)},
            "destination":"/fixture/installations/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", "entrypoint":{"executable":"/fixture/python", "arguments":["-m","fixture"]}, "mcp_name":"selected-fixture"})
    } else {
        json!({"scope":"configuration_only", "validation_id":"a".repeat(32),
            "configuration_sha256":"c".repeat(64), "family":"services", "profile":"slack", "contents_sha256":"2".repeat(64)})
    }
}

fn install(selected: bool) -> Value {
    if selected {
        json!({"validation_id":"b".repeat(32), "effect_sha256":"d".repeat(64)})
    } else {
        let mut value = selection(false);
        value["validation_id"] = "a".repeat(32).into();
        value["configuration_sha256"] = "c".repeat(64).into();
        value
    }
}

fn installed_result(selected: bool, reconciliation: bool) -> Value {
    let mut result = if selected {
        json!({"selection_kind":"python","component":"mcp","selection_sha256":"e".repeat(64),
            "manifest_digest":"1".repeat(64),"effect_sha256":"d".repeat(64),"receipt_sha256":"6".repeat(64)})
    } else {
        json!({"family":"services","profile":"slack","configuration_sha256":"c".repeat(64),"contents_sha256":"2".repeat(64)})
    };
    result["scope"] = if reconciliation {
        "retained_payload_and_launch_binding_only"
    } else {
        "observed_software_installation_only"
    }
    .into();
    let destination = if selected {
        proposal(true)["destination"].as_str().unwrap().to_owned()
    } else {
        "/fixture/installations/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into()
    };
    result["mcp_path"] = format!("{destination}/ready/mcp.json").into();
    result["receipt_path"] = format!("{destination}/ready/installed.json").into();
    let server = if selected {
        "selected-fixture"
    } else {
        "toolkit-services-aaaaaaaaaaaa"
    };
    // Actual owner envelopes can contain environment configuration. This test
    // marker must never be copied into the host's retained receipt projection.
    result["mcp_configuration"] = json!({"mcpServers":{server:{"command":"/fixture/python","args":["-m","fixture"],"env":{"FIXTURE_SECRET":"do-not-retain-this-value"}}}});
    if reconciliation {
        result["receipt_sha256"] = "6".repeat(64).into();
    }
    result
}

fn job_readback(selected: bool) -> Value {
    json!({"job":{"id":"3".repeat(32),"kind":"install","family":if selected {"selected-python"} else {"services"},
        "status":"succeeded","result":installed_result(selected,false)},"reconciliation":{"available":true,"observation":null}})
}

fn reconciled(selected: bool, revision: u64) -> Value {
    json!({"observation":{"job_id":"3".repeat(32),"revision":revision,"status":"matched",
        "scope":"retained_payload_and_launch_binding_only","result":installed_result(selected,true)}})
}

struct Package {
    selected: bool,
    structured_reply: AtomicBool,
    check_blocks: Mutex<Option<Vec<meerkat_core::ContentBlock>>>,
    checks: AtomicUsize,
    installs: AtomicUsize,
    calls: Mutex<Vec<(String, String, SessionId, bool)>>,
    reply: Mutex<Value>,
    readback_reply: Mutex<Option<Value>>,
    readback_is_error: AtomicBool,
    lose_readback: AtomicBool,
    epoch: AtomicU64,
    behavior: u8,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    complete_together: tokio::sync::Barrier,
    resolved: AtomicUsize,
    validated: AtomicUsize,
}

impl Package {
    async fn invoke(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        resolved: bool,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        self.calls.lock().unwrap().push((
            call.name.into(),
            call.args.get().into(),
            context.origin_session_id().unwrap().clone(),
            resolved,
        ));
        let value = match call.name {
            "setup_state" => {
                if self.behavior == 6 {
                    self.entered.notify_one();
                    self.release.notified().await;
                }
                self.readback_reply.lock().unwrap().clone()
                    .unwrap_or_else(|| json!({"scope":"consumer_setup_state"}))
            },
            "setup_check" | "setup_selected_check" => {
                self.checks.fetch_add(1, Ordering::SeqCst);
                if let Some(content) = self.check_blocks.lock().unwrap().clone() {
                    return Ok(ToolResult::with_blocks(call.id.into(), content, false).into());
                }
                self.reply.lock().unwrap().clone()
            }
            "setup_install" | "setup_selected_install" => {
                self.installs.fetch_add(1, Ordering::SeqCst);
                let mut expected = install(self.selected);
                expected["validation_id"] = self.reply.lock().unwrap()["validation_id"].clone();
                assert_eq!(call.parse_args::<Value>().unwrap(), expected);
                self.entered.notify_one();
                if matches!(self.behavior, 2 | 5) {
                    self.release.notified().await;
                }
                if self.behavior == 4 {
                    self.complete_together.wait().await;
                }
                if matches!(self.behavior, 1 | 5) {
                    return Err(ToolError::execution_failed("fixture lost reply"));
                }
                if self.behavior == 3 {
                    return Ok(ToolResult::new(call.id.into(), "{}".into(), false).into());
                }
                let job = if self.behavior == 7
                    && self.reply.lock().unwrap()["validation_id"] == "9".repeat(32)
                { "4".repeat(32) } else { "3".repeat(32) };
                json!({"job":{"id":job, "kind":"install", "family":if self.selected {"selected-python"} else {"services"}, "status":"running"}})
            }
            "setup_help" => self.readback_reply.lock().unwrap().clone().unwrap_or_else(|| json!({})),
            "setup_job" | "setup_reconcile" => {
                if self.lose_readback.swap(false, Ordering::SeqCst) {
                    return Err(ToolError::execution_failed("fixture lost readback reply"));
                }
                self.readback_reply.lock().unwrap().clone().unwrap_or_else(|| {
                    if call.name == "setup_reconcile" {
                        json!({"observation":{"job_id":"3".repeat(32),"revision":1,"status":"unconfirmed","scope":"retained_payload_and_launch_binding_only"}})
                    } else {
                        json!({"job":{"id":"3".repeat(32), "kind":"install", "family":if self.selected {"selected-python"} else {"services"}, "status":"running"}})
                    }
                })
            }
            _ => json!({"job":{"id":"3".repeat(32), "status":"running"}}),
        };
        let is_error = ["setup_job", "setup_reconcile"].contains(&call.name)
            && self.readback_is_error.load(Ordering::SeqCst);
        let result = if self.structured_reply.load(Ordering::SeqCst) {
            ToolResult::with_blocks(
                call.id.into(), vec![meerkat_core::ContentBlock::structured(&value).unwrap()], is_error,
            )
        } else {
            ToolResult::new(call.id.into(), value.to_string(), is_error)
        };
        Ok(result.into())
    }
}

#[async_trait]
impl AgentToolDispatcher for Package {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        NAMES
            .iter()
            .map(|name| {
                Arc::new(ToolDef::new(
                    *name,
                    "Synthetic package contract",
                    json!({"type":"object"}),
                ))
            })
            .collect()
    }
    fn tool_catalog_capabilities(&self) -> meerkat_core::ToolCatalogCapabilities {
        meerkat_core::ToolCatalogCapabilities {
            exact_catalog: true,
            may_require_catalog_control_plane: false,
        }
    }
    fn tool_mutation_class(&self, _: &str) -> ToolMutationClass {
        ToolMutationClass::Mutating
    }
    fn live_bridge_effect_kind(&self, _: &str) -> LiveBridgeEffectKind {
        LiveBridgeEffectKind::ExternalIo
    }
    fn execution_binding_epoch(&self, _: &str) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }
    fn pending_catalog_sources(&self) -> Arc<[String]> {
        vec!["fixture-owner".into()].into()
    }
    fn resolve_execution_plan(
        &self,
        _: ToolCallView<'_>,
        _: &ToolDispatchContext,
        resolution: &ToolExecutionResolutionContext,
    ) -> Result<meerkat_core::ResolvedToolExecutionPlan, meerkat_core::ToolExecutionResolutionError>
    {
        self.resolved.fetch_add(1, Ordering::SeqCst);
        meerkat_core::ToolExecutionContract::default()
            .resolve_default(resolution.deadlines().clone())
            .map_err(Into::into)
    }
    fn validate_resolved_execution_plan(
        &self,
        _: ToolCallView<'_>,
        resolution: &ToolExecutionResolutionContext,
        plan: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<(), meerkat_core::ToolExecutionResolutionError> {
        self.validated.fetch_add(1, Ordering::SeqCst);
        resolution.validate_resolved_plan(plan)
    }
    async fn dispatch(&self, _: ToolCallView<'_>) -> Result<ToolDispatchOutcome, ToolError> {
        panic!("native context/resolved route was lost")
    }
    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        self.invoke(call, context, false).await
    }
    async fn dispatch_resolved_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
        _: &meerkat_core::ResolvedToolExecutionPlan,
    ) -> Result<ToolDispatchOutcome, ToolError> {
        self.invoke(call, context, true).await
    }
}

struct Fixture {
    _root: tempfile::TempDir,
    approvals: LocalApprovals,
    presenter: Option<ApprovalPresenter>,
    package: Arc<Package>,
    owner: Arc<CheckedInstalls>,
    gate: Arc<ExecutionPolicyGatedDispatcher<CheckedInstalls>>,
    session: SessionId,
    realm: RealmLocator,
}

impl Fixture {
    fn new(selected: bool, behavior: u8, lifetime: Duration) -> Self {
        Self::build(selected, behavior, "receipt-fixture", |root, _| {
            let (approvals, presenter) = LocalApprovals::create_with(root, lifetime, &[]).unwrap();
            (approvals, Some(presenter))
        })
    }

    fn build(selected: bool, behavior: u8, realm_id: &str,
        approvals: impl FnOnce(&std::path::Path, &RealmLocator) -> (LocalApprovals, Option<ApprovalPresenter>),
    ) -> Self {
        let root = tempfile::tempdir().unwrap();
        let realm = crate::RealmOptions {
            context_root: root.path().to_owned(),
            state_root: Some(root.path().join("native-state")),
            user_config_root: Some(root.path().join("user-config")),
        }.resolve(realm_id).unwrap().locator;
        let (approvals, presenter) = approvals(root.path(), &realm);
        let session = SessionId::new();
        approvals.bind_session(&session).unwrap();
        let package = Arc::new(Package {
            selected,
            structured_reply: false.into(),
            check_blocks: Mutex::new(None),
            checks: 0.into(),
            installs: 0.into(),
            calls: Mutex::new(Vec::new()),
            reply: Mutex::new(proposal(selected)),
            readback_reply: Mutex::new(None),
            readback_is_error: false.into(),
            lose_readback: false.into(),
            epoch: 7.into(),
            behavior,
            entered: Default::default(),
            release: Default::default(),
            complete_together: tokio::sync::Barrier::new(2),
            resolved: 0.into(),
            validated: 0.into(),
        });
        let owner = CheckedInstalls::fixture(package.clone());
        owner.bind_realm(&realm).unwrap();
        let admission = Arc::new(Admission(
            approvals.shared.clone(),
            false,
            None,
            None,
            None,
            Some(owner.clone()),
        ));
        let gate = Arc::new(
            ExecutionPolicyGatedDispatcher::new(
                owner.clone(),
                ToolExecutionPolicy::resolve(ToolAccessPolicy::AllowList(
                    NAMES.iter().copied().collect(),
                ))
                .unwrap(),
            )
            .with_dispatch_admission(admission),
        );
        Self {
            _root: root,
            approvals,
            presenter,
            package,
            owner,
            gate,
            session,
            realm,
        }
    }
    // Same synthetic package and actual native decision presenter, with the
    // selected owner-local factor and no standalone global admission gate.
    fn local(selected: bool, behavior: u8) -> Self {
        let mut fixture = Self::new(selected, behavior, DECISION_LIFETIME);
        fixture.owner = CheckedInstalls::fixture_with_review(
            fixture.package.clone(),
            Arc::new(LocalApprovals { shared: fixture.approvals.shared.clone(), service_tasks: None }),
        );
        fixture.owner.bind_realm(&fixture.realm).unwrap();
        fixture.gate = Arc::new(ExecutionPolicyGatedDispatcher::new(
            fixture.owner.clone(),
            ToolExecutionPolicy::resolve(ToolAccessPolicy::AllowList(
                NAMES.iter().copied().collect(),
            )).unwrap(),
        ));
        fixture
    }

    fn start(
        &self,
        id: &str,
        name: &str,
        args: Value,
        resolved: bool,
    ) -> tokio::task::JoinHandle<Result<ToolDispatchOutcome, ToolError>> {
        let gate = self.gate.clone();
        let id = id.to_owned();
        let name = name.to_owned();
        let context =
            ToolDispatchContext::default().with_runtime_identity(self.session.clone(), None);
        tokio::spawn(async move {
            let args = serde_json::value::to_raw_value(&args).unwrap();
            let call = ToolCallView {
                id: &id,
                name: &name,
                args: &args,
            };
            if resolved {
                let resolution = ToolExecutionResolutionContext::new(
                    ToolDeadlineChain::new(vec![ToolDeadlineContributor::finite(
                        ToolDeadlineOwner::CoreToolDispatch,
                        Duration::from_secs(30),
                    )])
                    .unwrap(),
                );
                let plan = gate
                    .resolve_execution_plan(call, &context, &resolution)
                    .unwrap();
                gate.validate_resolved_execution_plan(call, &resolution, &plan)
                    .unwrap();
                gate.dispatch_resolved_with_context(call, &context, &plan)
                    .await
            } else {
                gate.dispatch_with_context(call, &context).await
            }
        })
    }
    fn check_name(&self) -> &'static str {
        if self.package.selected {
            "setup_selected_check"
        } else {
            "setup_check"
        }
    }
    fn install_name(&self) -> &'static str {
        if self.package.selected {
            "setup_selected_install"
        } else {
            "setup_install"
        }
    }
    async fn check(&self) {
        self.start(
            "check",
            self.check_name(),
            selection(self.package.selected),
            true,
        )
        .await
        .unwrap()
        .unwrap();
    }
    fn install(&self) -> tokio::task::JoinHandle<Result<ToolDispatchOutcome, ToolError>> {
        self.start(
            "install",
            self.install_name(),
            install(self.package.selected),
            true,
        )
    }
    async fn prompt(&mut self) -> ApprovalPrompt {
        tokio::time::timeout(Duration::from_secs(2), self.presenter.as_mut().unwrap().next())
            .await
            .unwrap()
            .unwrap()
    }
    fn count(&self) -> usize {
        self.package.installs.load(Ordering::SeqCst)
    }
}

#[tokio::test]
async fn checked_install_both_contracts_bind_actual_proposal_and_delegate_once() {
    for selected in [false, true] {
        let mut f = Fixture::new(selected, 0, DECISION_LIFETIME);
        f.check().await;
        assert!(f.presenter.as_mut().unwrap().prompts.try_recv().is_err());
        let run = f.install();
        let mut prompt = f.prompt().await;
        let body = prompt.record.proposed_action.body.as_ref().unwrap();
        assert_eq!(body["selection"], selection(selected));
        assert_eq!(body["owner_proposal"], proposal(selected));
        assert_eq!(body["scope"], "software_installation_only");
        let exact_id = prompt.record.approval_id.clone();
        prompt.record.approval_id = ApprovalId::new();
        assert!(prompt.decide(ApprovalDecision::Approve).is_err());
        assert_eq!(f.count(), 0);
        prompt.record.approval_id = exact_id.clone();
        prompt.decide(ApprovalDecision::Approve).unwrap();
        assert!(prompt.decide(ApprovalDecision::Approve).is_err());
        let result = run.await.unwrap().unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&result.result.text_content()).unwrap()["job"]["id"],
            "3".repeat(32)
        );
        assert_eq!(
            f.approvals.shared.service.get(&exact_id).unwrap().status,
            ApprovalStatus::Approved
        );
        assert!(!f.approvals.has_uncertain_dispatch());
        assert!(f.install().await.unwrap().is_err());
        assert_eq!(f.count(), 1);
        assert!(f.owner.tool_catalog_capabilities().exact_catalog);
        assert_eq!(
            f.owner.pending_catalog_sources().as_ref(),
            &["fixture-owner".to_owned()]
        );
        assert_eq!(f.owner.execution_binding_epoch(f.install_name()), 7);
        assert_eq!(
            f.owner
                .execution_binding_fingerprint(f.install_name())
                .unwrap(),
            f.package
                .execution_binding_fingerprint(f.install_name())
                .unwrap()
        );
        let calls = f.package.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert!(
            calls
                .iter()
                .all(|(_, _, session, resolved)| session == &f.session && *resolved)
        );
        assert!(f.package.resolved.load(Ordering::SeqCst) >= 2);
        assert!(f.package.validated.load(Ordering::SeqCst) >= 2);
    }
}

#[tokio::test]
async fn checked_install_receipts_bind_actual_job_session_and_selected_realm() {
    for selected in [false, true] {
        let mut f = Fixture::new(selected, 0, DECISION_LIFETIME);
        f.check().await;
        let run = f.install();
        f.prompt().await.decide(ApprovalDecision::Approve).unwrap();
        run.await.unwrap().unwrap();
        assert!(
            f.owner
                .installed_selection(&f.realm, &f.session, &"3".repeat(32))
                .unwrap()
                .installed
                .is_none()
        );
        *f.package.readback_reply.lock().unwrap() = Some(job_readback(selected));
        f.start("job", "setup_job", json!({"job_id":"3".repeat(32)}), true)
            .await
            .unwrap()
            .unwrap();
        let capture = f
            .owner
            .installed_selection(&f.realm, &f.session, &"3".repeat(32))
            .unwrap();
        assert!(capture.matched);
        assert_eq!(capture.reconciliation_revision, None);
        let installed = capture.installed.unwrap();
        assert_eq!(installed.realm, f.realm);
        assert_eq!(installed.session, f.session);
        assert_eq!(
            installed.validation_id,
            if selected {
                "b".repeat(32)
            } else {
                "a".repeat(32)
            }
        );
        assert_eq!(installed.receipt_sha256.is_some(), selected);
        assert!(!format!("{installed:?}").contains("do-not-retain-this-value"));
        assert!(!format!("{installed:?}").contains("FIXTURE_SECRET"));
        *f.package.readback_reply.lock().unwrap() = Some(reconciled(selected, 2));
        f.start(
            "reconcile",
            "setup_reconcile",
            json!({"job_id":"3".repeat(32)}),
            false,
        )
        .await
        .unwrap()
        .unwrap();
        let capture = f
            .owner
            .installed_selection(&f.realm, &f.session, &"3".repeat(32))
            .unwrap();
        assert!(capture.matched);
        assert_eq!(capture.reconciliation_revision, Some(2));
        assert_eq!(
            capture.installed.unwrap().receipt_sha256.as_deref(),
            Some("6".repeat(64).as_str())
        );
        let mut other_root = f.realm.clone();
        other_root.state_root = other_root.state_root.join("different-root");
        assert!(
            f.owner
                .installed_selection(&other_root, &f.session, &"3".repeat(32))
                .is_err()
        );
        assert!(f.owner.bind_realm(&other_root).is_err());
        assert!(
            f.owner
                .installed_selection(&f.realm, &SessionId::new(), &"3".repeat(32))
                .is_err()
        );
        assert!(
            f.owner
                .installed_selection(&f.realm, &f.session, &"4".repeat(32))
                .is_err()
        );
        f.package.epoch.store(8, Ordering::SeqCst);
        assert!(
            !f.owner
                .installed_selection(&f.realm, &f.session, &"3".repeat(32))
                .unwrap()
                .matched
        );
        assert_eq!(f.count(), 1);
    }
}

#[tokio::test]
async fn checked_install_receipts_refuse_mismatched_job_and_selected_inputs() {
    for selected in [false, true] {
        let mut f = Fixture::new(selected, 0, DECISION_LIFETIME);
        f.check().await;
        let run = f.install();
        f.prompt().await.decide(ApprovalDecision::Approve).unwrap();
        run.await.unwrap().unwrap();
        for field in ["id", "kind", "family"] {
            let mut value = job_readback(selected);
            value["job"][field] = "wrong".into();
            *f.package.readback_reply.lock().unwrap() = Some(value);
            assert!(
                f.start(
                    "wrong-job",
                    "setup_job",
                    json!({"job_id":"3".repeat(32)}),
                    false
                )
                .await
                .unwrap()
                .is_err()
            );
            assert!(
                f.owner
                    .installed_selection(&f.realm, &f.session, &"3".repeat(32))
                    .unwrap()
                    .installed
                    .is_none()
            );
        }
        for field in if selected {
            vec![
                "effect_sha256",
                "selection_sha256",
                "manifest_digest",
                "component",
            ]
        } else {
            vec!["configuration_sha256", "profile", "family"]
        } {
            let mut value = reconciled(selected, 1);
            value["observation"]["result"][field] = "wrong".into();
            *f.package.readback_reply.lock().unwrap() = Some(value);
            assert!(
                f.start(
                    "wrong-selection",
                    "setup_reconcile",
                    json!({"job_id":"3".repeat(32)}),
                    false
                )
                .await
                .unwrap()
                .is_err()
            );
            assert!(
                f.owner
                    .installed_selection(&f.realm, &f.session, &"3".repeat(32))
                    .unwrap()
                    .installed
                    .is_none()
            );
        }
        for field in ["receipt_sha256", "mcp_path", "receipt_path"] {
            let mut value = reconciled(selected, 1);
            value["observation"]["result"][field] = "wrong".into();
            *f.package.readback_reply.lock().unwrap() = Some(value);
            assert!(
                f.start(
                    "wrong-receipt",
                    "setup_reconcile",
                    json!({"job_id":"3".repeat(32)}),
                    false
                )
                .await
                .unwrap()
                .is_err()
            );
        }
        // A real owner result for another selected job cannot be imported by
        // issuing a read for an unknown job ID, even if its payload looks valid.
        *f.package.readback_reply.lock().unwrap() = Some(job_readback(selected));
        f.start(
            "unknown-job",
            "setup_job",
            json!({"job_id":"4".repeat(32)}),
            false,
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            f.owner
                .installed_selection(&f.realm, &f.session, &"4".repeat(32))
                .is_err()
        );
        assert!(
            f.owner
                .installed_selection(&f.realm, &f.session, &"3".repeat(32))
                .unwrap()
                .installed
                .is_none()
        );
        assert_eq!(f.count(), 1);
    }
}

#[tokio::test]
async fn checked_install_owner_busy_reply_preserves_history_until_exact_readback() {
    for selected in [false, true] {
        for structured in [false, true] {
            let mut f = Fixture::new(selected, 0, DECISION_LIFETIME);
            f.package.structured_reply.store(structured, Ordering::SeqCst);
            f.check().await;
            let run = f.install();
            f.prompt().await.decide(ApprovalDecision::Approve).unwrap();
            run.await.unwrap().unwrap();
            *f.package.readback_reply.lock().unwrap() = Some(job_readback(selected));
            f.start("completed", "setup_job", json!({"job_id":"3".repeat(32)}), false)
                .await.unwrap().unwrap();
            let historical = f.owner.installed_selection(&f.realm, &f.session, &"3".repeat(32))
                .unwrap().installed;
            assert!(historical.is_some());

            let busy = json!({"error":"operation_in_progress"});
            *f.package.readback_reply.lock().unwrap() = Some(busy.clone());
            f.package.readback_is_error.store(true, Ordering::SeqCst);
            for tool in ["setup_job", "setup_reconcile"] {
                let result = f.start("busy", tool, json!({"job_id":"3".repeat(32)}), true)
                    .await.unwrap().unwrap();
                assert!(result.result.is_error);
                assert_eq!(result.result.tool_use_id, "busy");
                assert!(result.async_ops.is_empty() && result.session_effects.is_empty());
                match result.result.content.as_slice() {
                    [meerkat_core::ContentBlock::Text { text }] if !structured => {
                        assert_eq!(text, &busy.to_string());
                    }
                    [meerkat_core::ContentBlock::Structured { data }] if structured => {
                        assert_eq!(data.get(), busy.to_string());
                    }
                    _ => panic!("the owner's error response must be returned unchanged"),
                }
                let observed = f.owner.installed_selection(&f.realm, &f.session, &"3".repeat(32))
                    .unwrap();
                assert!(!observed.matched);
                assert_eq!(observed.installed, historical);
                assert_eq!(observed.reconciliation_revision, None);
                assert_eq!(f.count(), 1);
            }

            f.package.readback_is_error.store(false, Ordering::SeqCst);
            *f.package.readback_reply.lock().unwrap() = Some(reconciled(selected, 1));
            f.start("matched", "setup_reconcile", json!({"job_id":"3".repeat(32)}), false)
                .await.unwrap().unwrap();
            let observed = f.owner.installed_selection(&f.realm, &f.session, &"3".repeat(32))
                .unwrap();
            assert!(observed.matched);
            assert_eq!(observed.reconciliation_revision, Some(1));
            assert_eq!(observed.installed.unwrap().receipt_sha256.as_deref(), Some("6".repeat(64).as_str()));
            assert_eq!(f.count(), 1);
        }
    }
}

#[tokio::test]
async fn checked_install_lost_reconcile_reply_recovers_only_from_exact_owner_readback() {
    let mut f = Fixture::new(false, 0, DECISION_LIFETIME);
    f.check().await;
    let run = f.install();
    f.prompt().await.decide(ApprovalDecision::Approve).unwrap();
    run.await.unwrap().unwrap();
    *f.package.readback_reply.lock().unwrap() = Some(job_readback(false));
    f.start(
        "completed",
        "setup_job",
        json!({"job_id":"3".repeat(32)}),
        false,
    )
    .await
    .unwrap()
    .unwrap();
    let historical = f
        .owner
        .installed_selection(&f.realm, &f.session, &"3".repeat(32))
        .unwrap()
        .installed;
    f.package.lose_readback.store(true, Ordering::SeqCst);
    assert!(
        f.start(
            "lost",
            "setup_reconcile",
            json!({"job_id":"3".repeat(32)}),
            true
        )
        .await
        .unwrap()
        .is_err()
    );
    let lost = f
        .owner
        .installed_selection(&f.realm, &f.session, &"3".repeat(32))
        .unwrap();
    assert!(!lost.matched);
    assert_eq!(lost.installed, historical);
    let mut wrong = reconciled(false, 3);
    wrong["observation"]["job_id"] = "4".repeat(32).into();
    *f.package.readback_reply.lock().unwrap() = Some(wrong);
    assert!(
        f.start(
            "wrong",
            "setup_reconcile",
            json!({"job_id":"3".repeat(32)}),
            true
        )
        .await
        .unwrap()
        .is_err()
    );
    assert!(
        !f.owner
            .installed_selection(&f.realm, &f.session, &"3".repeat(32))
            .unwrap()
            .matched
    );
    let mut recovered = job_readback(false);
    recovered["reconciliation"]["observation"] = reconciled(false, 3)["observation"].clone();
    *f.package.readback_reply.lock().unwrap() = Some(recovered);
    f.start(
        "recovered",
        "setup_job",
        json!({"job_id":"3".repeat(32)}),
        true,
    )
    .await
    .unwrap()
    .unwrap();
    let receipt = f
        .owner
        .installed_selection(&f.realm, &f.session, &"3".repeat(32))
        .unwrap();
    assert!(receipt.matched);
    assert_eq!(receipt.reconciliation_revision, Some(3));
    let mut changed_receipt = reconciled(false, 4);
    changed_receipt["observation"]["result"]["receipt_sha256"] = "7".repeat(64).into();
    *f.package.readback_reply.lock().unwrap() = Some(changed_receipt);
    assert!(
        f.start(
            "changed-receipt",
            "setup_reconcile",
            json!({"job_id":"3".repeat(32)}),
            true
        )
        .await
        .unwrap()
        .is_err()
    );
    let rejected = f
        .owner
        .installed_selection(&f.realm, &f.session, &"3".repeat(32))
        .unwrap();
    assert!(!rejected.matched);
    assert_eq!(rejected.installed, receipt.installed);
    assert_eq!(rejected.reconciliation_revision, Some(4));
    // The exact older matched response is still stale after a newer bound
    // owner observation was rejected for changed receipt identity.
    *f.package.readback_reply.lock().unwrap() = Some(reconciled(false, 3));
    assert!(
        f.start(
            "stale",
            "setup_reconcile",
            json!({"job_id":"3".repeat(32)}),
            true
        )
        .await
        .unwrap()
        .is_err()
    );
    assert!(
        !f.owner
            .installed_selection(&f.realm, &f.session, &"3".repeat(32))
            .unwrap()
            .matched
    );
    let stale = f
        .owner
        .installed_selection(&f.realm, &f.session, &"3".repeat(32))
        .unwrap();
    assert_eq!(stale.reconciliation_revision, Some(4));
    assert_eq!(stale.installed, receipt.installed);
    *f.package.readback_reply.lock().unwrap() = Some(reconciled(false, 5));
    f.start(
        "later-current",
        "setup_reconcile",
        json!({"job_id":"3".repeat(32)}),
        true,
    )
    .await
    .unwrap()
    .unwrap();
    let recovered = f
        .owner
        .installed_selection(&f.realm, &f.session, &"3".repeat(32))
        .unwrap();
    assert!(recovered.matched);
    assert_eq!(recovered.reconciliation_revision, Some(5));
    assert_eq!(recovered.installed, receipt.installed);
    assert_eq!(f.count(), 1);
    assert!(f.presenter.as_mut().unwrap().prompts.try_recv().is_err());
    assert!(f.install().await.unwrap().is_err());
}

#[tokio::test]
async fn checked_install_job_snapshot_never_publishes_a_partial_receipt() {
    for selected in [false, true] {
        for status in [
            "checking",
            "unconfirmed",
            "wrong-job",
            "wrong-receipt",
            "malformed-envelope",
        ] {
            let mut f = Fixture::new(selected, 0, DECISION_LIFETIME);
            f.check().await;
            let run = f.install();
            f.prompt().await.decide(ApprovalDecision::Approve).unwrap();
            run.await.unwrap().unwrap();
            let mut snapshot = job_readback(selected);
            let mut observation = reconciled(selected, 4)["observation"].clone();
            match status {
                "checking" | "unconfirmed" => observation["status"] = status.into(),
                "wrong-job" => observation["job_id"] = "4".repeat(32).into(),
                "wrong-receipt" => observation["result"]["receipt_sha256"] = "invalid".into(),
                "malformed-envelope" => {}
                _ => unreachable!(),
            }
            snapshot["reconciliation"]["observation"] = observation;
            if status == "malformed-envelope" {
                snapshot["reconciliation"] = "invalid".into();
            }
            *f.package.readback_reply.lock().unwrap() = Some(snapshot);
            let result = f
                .start(
                    "snapshot",
                    "setup_job",
                    json!({"job_id":"3".repeat(32)}),
                    true,
                )
                .await
                .unwrap();
            let capture = f
                .owner
                .installed_selection(&f.realm, &f.session, &"3".repeat(32))
                .unwrap();
            assert!(!capture.matched);
            if ["checking", "unconfirmed"].contains(&status) {
                result.unwrap();
                assert!(capture.installed.is_some());
                assert_eq!(capture.reconciliation_revision, Some(4));
            } else {
                assert!(result.is_err());
                // A valid job projection in a rejected combined envelope was
                // never committed, even as an installed historical identity.
                assert!(capture.installed.is_none());
                assert_eq!(
                    capture.reconciliation_revision,
                    if ["wrong-job", "malformed-envelope"].contains(&status) {
                        None
                    } else {
                        Some(4)
                    }
                );
            }
            assert_eq!(f.count(), 1);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn checked_install_owner_concurrent_replies_cannot_share_a_job_binding() {
    let mut f = Fixture::new(false, 4, DECISION_LIFETIME);
    f.check().await;
    // Exercise only the existing CheckedInstalls consumption/binding owner.
    // LocalApprovals serializes normal outstanding approvals separately;
    // this component test makes no claim of concurrent native authorization.
    let dispatch_install = |id: &str, args: Value| {
        let id = id.to_owned();
        let args = serde_json::value::to_raw_value(&args).unwrap();
        let call = ToolCallView {
            id: &id,
            name: "setup_install",
            args: &args,
        };
        let held = f.owner.retain_action(call, &f.session).unwrap();
        f.owner.arm_action(held).unwrap();
        let owner = f.owner.clone();
        let context = ToolDispatchContext::default().with_runtime_identity(f.session.clone(), None);
        tokio::spawn(async move {
            owner
                .dispatch_with_context(
                    ToolCallView {
                        id: &id,
                        name: "setup_install",
                        args: &args,
                    },
                    &context,
                )
                .await
        })
    };
    let first = dispatch_install("first-install", install(false));
    f.package.entered.notified().await;
    let mut second_proposal = proposal(false);
    second_proposal["validation_id"] = "9".repeat(32).into();
    *f.package.reply.lock().unwrap() = second_proposal;
    f.check().await;
    let mut second_install = install(false);
    second_install["validation_id"] = "9".repeat(32).into();
    let second = dispatch_install("second-install", second_install);
    // Both dispatch futures return the same job ID together. The single
    // CheckedInstalls state owner must bind exactly one consumed proposal.
    let (first_result, second_result) = tokio::join!(first, second);
    let first_result = first_result.unwrap();
    let second_result = second_result.unwrap();
    assert_ne!(first_result.is_ok(), second_result.is_ok());
    *f.package.readback_reply.lock().unwrap() = Some(job_readback(false));
    f.start(
        "winner-readback",
        "setup_job",
        json!({"job_id":"3".repeat(32)}),
        true,
    )
    .await
    .unwrap()
    .unwrap();
    let capture = f
        .owner
        .installed_selection(&f.realm, &f.session, &"3".repeat(32))
        .unwrap();
    assert!(capture.matched);
    assert_eq!(
        capture.installed.unwrap().validation_id,
        if first_result.is_ok() {
            "a".repeat(32)
        } else {
            "9".repeat(32)
        }
    );
    assert_eq!(f.count(), 2);
    assert!(f.presenter.as_mut().unwrap().prompts.try_recv().is_err());
}

#[tokio::test]
async fn checked_install_rejects_uncaptured_substituted_session_and_epoch() {
    for selected in [false, true] {
        let f = Fixture::new(selected, 0, DECISION_LIFETIME);
        assert!(f.install().await.unwrap().is_err());
        f.check().await;
        for field in if selected {
            vec!["effect_sha256", "validation_id", "unexpected"]
        } else {
            vec![
                "configuration_sha256",
                "validation_id",
                "family",
                "profile",
                "operations",
                "values",
                "unexpected",
            ]
        } {
            let mut wrong = install(selected);
            wrong[field] = "substituted".into();
            assert!(
                f.start("substitute", f.install_name(), wrong, false)
                    .await
                    .unwrap()
                    .is_err()
            );
        }
        let args = serde_json::value::to_raw_value(&install(selected)).unwrap();
        let call = ToolCallView {
            id: "install",
            name: f.install_name(),
            args: &args,
        };
        assert!(f.owner.retain_action(call, &SessionId::new()).is_err());
        f.package.epoch.store(8, Ordering::SeqCst);
        assert!(f.install().await.unwrap().is_err());
        assert_eq!(f.count(), 0);
    }
}

#[tokio::test]
async fn checked_install_failed_mismatched_or_duplicate_check_never_arms() {
    for selected in [false, true] {
        for case in 0..4 {
            let f = Fixture::new(selected, 0, DECISION_LIFETIME);
            let mut reply = proposal(selected);
            match case {
                0 => reply = json!({"error":"owner-correction"}),
                1 => reply["profile"] = "other".into(),
                2 => reply["scope"] = "installed".into(),
                _ => reply["validation_id"] = "not-an-id".into(),
            };
            *f.package.reply.lock().unwrap() = reply;
            let result = f
                .start("check", f.check_name(), selection(selected), false)
                .await
                .unwrap();
            if case == 0 {
                assert!(
                    result
                        .unwrap()
                        .result
                        .text_content()
                        .contains("owner-correction")
                );
            } else {
                assert!(result.is_err());
            }
            assert!(f.install().await.unwrap().is_err());
            assert_eq!(f.count(), 0);
        }
        let f = Fixture::new(selected, 0, DECISION_LIFETIME);
        f.check().await;
        f.package.reply.lock().unwrap()["extra"] = "changed same validation ID".into();
        assert!(
            f.start("duplicate", f.check_name(), selection(selected), false)
                .await
                .unwrap()
                .is_err()
        );
        // A conflicting response cannot replace the first exact proposal.
        let args = serde_json::value::to_raw_value(&install(selected)).unwrap();
        assert_eq!(
            f.owner
                .retain_action(
                    ToolCallView {
                        id: "install",
                        name: f.install_name(),
                        args: &args
                    },
                    &f.session
                )
                .unwrap()
                .review()["owner_proposal"],
            proposal(selected)
        );
    }
    let duplicate =
        RawValue::from_string("{\"validation_id\":\"a\",\"validation_id\":\"b\"}".into()).unwrap();
    let f = Fixture::new(false, 0, DECISION_LIFETIME);
    assert!(
        f.owner
            .retain_action(
                ToolCallView {
                    id: "x",
                    name: "setup_install",
                    args: &duplicate
                },
                &f.session
            )
            .is_err()
    );
}

#[tokio::test]
async fn checked_install_decline_expiry_and_retired_decision_make_no_submission() {
    for mode in 0..4 {
        let mut f = Fixture::new(
            false,
            0,
            if mode == 1 {
                Duration::from_millis(40)
            } else {
                DECISION_LIFETIME
            },
        );
        f.check().await;
        let run = f.install();
        let mut prompt = f.prompt().await;
        match mode {
            0 => prompt.decide(ApprovalDecision::Deny).unwrap(),
            1 => {
                tokio::time::sleep(Duration::from_millis(60)).await;
                assert!(prompt.decide(ApprovalDecision::Approve).is_err());
            }
            2 => {
                prompt.decide(ApprovalDecision::Approve).unwrap();
                f.approvals.retire();
                f.owner.retire();
            }
            _ => drop(prompt),
        }
        assert!(run.await.unwrap().is_err());
        assert_eq!(f.count(), 0);
    }
}

#[tokio::test]
async fn checked_install_lost_or_malformed_reply_stays_uncertain_without_job_or_replay() {
    for selected in [false, true] {
        for behavior in [1, 3] {
            let mut f = Fixture::new(selected, behavior, DECISION_LIFETIME);
            f.check().await;
            let run = f.install();
            f.prompt().await.decide(ApprovalDecision::Approve).unwrap();
            assert!(run.await.unwrap().is_err());
            assert_eq!(f.count(), 1);
            assert!(f.approvals.has_uncertain_dispatch());
            assert!(!f.approvals.permits_uncertain_status_readback());
            for (name, args) in [
                (f.install_name(), install(selected)),
                (f.check_name(), selection(selected)),
                ("setup_prepare", json!({})),
                ("setup_job", json!({"job_id":"3".repeat(32)})),
            ] {
                assert!(
                    f.start("no-replay", name, args, false)
                        .await
                        .unwrap()
                        .is_err()
                );
            }
            assert_eq!(f.package.checks.load(Ordering::SeqCst), 1);
            assert_eq!(f.count(), 1);
        }
    }
}

#[tokio::test]
async fn checked_install_entered_drop_and_retirement_preserve_exact_uncertainty() {
    for dropped in [false, true] {
        let mut f = Fixture::new(false, 2, DECISION_LIFETIME);
        f.check().await;
        let run = f.install();
        f.prompt().await.decide(ApprovalDecision::Approve).unwrap();
        tokio::time::timeout(Duration::from_secs(2), f.package.entered.notified())
            .await
            .unwrap();
        if dropped {
            run.abort();
            assert!(run.await.unwrap_err().is_cancelled());
            assert!(!f.approvals.permits_uncertain_status_readback());
        } else {
            f.owner.retire();
            f.package.release.notify_one();
            assert!(run.await.unwrap().is_err());
            assert!(f.approvals.permits_uncertain_status_readback());
            for name in ["setup_job", "setup_reconcile", "setup_stop"] {
                assert!(
                    f.start("exact", name, json!({"job_id":"3".repeat(32)}), false)
                        .await
                        .unwrap()
                        .is_ok()
                );
                assert!(
                    f.start("other", name, json!({"job_id":"4".repeat(32)}), false)
                        .await
                        .unwrap()
                        .is_err()
                );
                assert!(
                    f.start(
                        "extra",
                        name,
                        json!({"job_id":"3".repeat(32),"retry":true}),
                        false
                    )
                    .await
                    .unwrap()
                    .is_err()
                );
            }
        }
        assert!(f.approvals.has_uncertain_dispatch());
        assert!(f.install().await.unwrap().is_err());
        assert!(
            f.start("fresh", f.check_name(), selection(false), false)
                .await
                .unwrap()
                .is_err()
        );
        f.approvals.retire();
        assert!(
            f.start(
                "closed",
                "setup_job",
                json!({"job_id":"3".repeat(32)}),
                false
            )
            .await
            .unwrap()
            .is_err()
        );
        assert_eq!(f.count(), 1);
    }
}

#[tokio::test]
async fn checked_install_bounded_job_wait_preserves_exact_uncertain_readback() {
    let mut f = Fixture::new(false, 2, DECISION_LIFETIME);
    f.check().await;
    let run = f.install();
    f.prompt().await.decide(ApprovalDecision::Approve).unwrap();
    tokio::time::timeout(Duration::from_secs(2), f.package.entered.notified())
        .await
        .unwrap();
    f.owner.retire();
    f.package.release.notify_one();
    assert!(run.await.unwrap().is_err());
    assert!(f.approvals.has_uncertain_dispatch());

    for wait_ms in [
        json!(true),
        json!(false),
        json!(-1),
        json!(10001),
        json!(1.0),
        json!("1"),
        Value::Null,
        json!([]),
        json!({}),
    ] {
        let before = f.package.calls.lock().unwrap().len();
        assert!(
            f.start(
                "invalid-wait",
                "setup_job",
                json!({"job_id":"3".repeat(32),"wait_ms":wait_ms}),
                false
            )
            .await
            .unwrap()
            .is_err()
        );
        assert_eq!(f.package.calls.lock().unwrap().len(), before);
    }
    for (name, arguments) in [
        ("setup_job", json!({"job_id":"4".repeat(32),"wait_ms":10})),
        (
            "setup_job",
            json!({"job_id":"3".repeat(32),"wait_ms":10,"retry":true}),
        ),
        ("setup_stop", json!({"job_id":"3".repeat(32),"wait_ms":0})),
        (
            "setup_reconcile",
            json!({"job_id":"3".repeat(32),"wait_ms":0}),
        ),
    ] {
        let before = f.package.calls.lock().unwrap().len();
        assert!(
            f.start("wrong-shape", name, arguments, false)
                .await
                .unwrap()
                .is_err()
        );
        assert_eq!(f.package.calls.lock().unwrap().len(), before);
    }
    for wait_ms in [0, 1, 10000] {
        let arguments = json!({"job_id":"3".repeat(32),"wait_ms":wait_ms});
        assert!(
            f.start("exact-wait", "setup_job", arguments.clone(), false)
                .await
                .unwrap()
                .is_ok()
        );
        assert_eq!(
            serde_json::from_str::<Value>(&f.package.calls.lock().unwrap().last().unwrap().1)
                .unwrap(),
            arguments
        );
    }
    assert!(f.approvals.has_uncertain_dispatch());
    assert_eq!(f.count(), 1);
    f.package.epoch.store(8, Ordering::SeqCst);
    let before = f.package.calls.lock().unwrap().len();
    assert!(
        f.start(
            "stale-wait",
            "setup_job",
            json!({"job_id":"3".repeat(32),"wait_ms":1}),
            false
        )
        .await
        .unwrap()
        .is_err()
    );
    assert_eq!(f.package.calls.lock().unwrap().len(), before);
}

#[tokio::test]
async fn checked_install_changed_binding_never_advertises_unusable_recovery() {
    let mut f = Fixture::new(true, 2, DECISION_LIFETIME);
    f.check().await;
    let run = f.install();
    f.prompt().await.decide(ApprovalDecision::Approve).unwrap();
    tokio::time::timeout(Duration::from_secs(2), f.package.entered.notified())
        .await
        .unwrap();
    f.package.epoch.store(8, Ordering::SeqCst);
    f.package.release.notify_one();
    assert!(run.await.unwrap().is_err());
    assert!(f.approvals.has_uncertain_dispatch());
    assert!(!f.approvals.permits_uncertain_status_readback());
    assert!(
        f.start(
            "stale-binding",
            "setup_job",
            json!({"job_id":"3".repeat(32)}),
            false
        )
        .await
        .unwrap()
        .is_err()
    );
    assert!(f.install().await.unwrap().is_err());
    assert_eq!(f.count(), 1);
}

#[tokio::test]
async fn checked_install_native_ceiling_still_refuses_after_private_approval() {
    let mut f = Fixture::new(false, 0, DECISION_LIFETIME);
    f.check().await;
    f.gate = Arc::new(
        ExecutionPolicyGatedDispatcher::new(
            f.owner.clone(),
            ToolExecutionPolicy::resolve(ToolAccessPolicy::DenyList(
                ["setup_install"].into_iter().collect(),
            ))
            .unwrap(),
        )
        .with_dispatch_admission(Arc::new(Admission(
            f.approvals.shared.clone(),
            false,
            None,
            None,
            None,
            Some(f.owner.clone()),
        ))),
    );
    // This invokes the native gate directly: its policy refuses after consent,
    // before the wrapped package operation, as on the existing terminal path.
    let run = f.start("native-refused", "setup_install", install(false), false);
    f.prompt().await.decide(ApprovalDecision::Approve).unwrap();
    assert!(run.await.unwrap().is_err());
    assert_eq!(f.count(), 0);
    assert!(!f.approvals.has_uncertain_dispatch());
}

#[tokio::test]
async fn checked_install_poison_is_infrastructure_failure_before_entry() {
    for selected in [false, true] {
        let f = Fixture::new(selected, 0, DECISION_LIFETIME);
        assert!(matches!(
            f.install().await.unwrap(),
            Err(ToolError::AccessDenied { .. })
        ));
        f.check().await;
        let args = serde_json::value::to_raw_value(&install(selected)).unwrap();
        let call = ToolCallView {
            id: "install",
            name: f.install_name(),
            args: &args,
        };
        let held = f.owner.retain_action(call, &f.session).unwrap();
        f.owner.poison_fixture_state();
        assert!(matches!(
            f.owner.retain_action(call, &f.session),
            Err(ToolError::ExecutionFailed { .. })
        ));
        assert!(matches!(
            f.owner.arm_action(held),
            Err(ToolError::ExecutionFailed { .. })
        ));
        assert!(matches!(
            f.owner.discard_unused_arm(call),
            Err(ToolError::ExecutionFailed { .. })
        ));
        assert!(matches!(
            f.start("no-check", f.check_name(), selection(selected), false)
                .await
                .unwrap(),
            Err(ToolError::ExecutionFailed { .. })
        ));
        assert!(matches!(
            f.install().await.unwrap(),
            Err(ToolError::ExecutionFailed { .. })
        ));
        assert_eq!(f.package.checks.load(Ordering::SeqCst), 1);
        assert_eq!(f.count(), 0);
    }
}

// Fault injection occurs only after the native gate observes its outcome,
// immediately before it asks the unchanged local admission owner to settle.
struct PoisonAtSettlement {
    admission: Admission,
    owner: Arc<CheckedInstalls>,
    expected: LiveBridgeEffectOutcome,
}

#[async_trait]
impl ToolDispatchAdmission for PoisonAtSettlement {
    async fn await_dispatch_admission(
        &self,
        call: ToolCallView<'_>,
        context: Option<&ToolDispatchContext>,
        effect_kind: LiveBridgeEffectKind,
    ) -> Result<(), ToolError> {
        self.admission
            .await_dispatch_admission(call, context, effect_kind)
            .await
    }

    async fn record_dispatch_outcome(
        &self,
        call: ToolCallView<'_>,
        context: Option<&ToolDispatchContext>,
        effect_kind: LiveBridgeEffectKind,
        outcome: LiveBridgeEffectOutcome,
    ) -> Result<(), ToolError> {
        if crate::service_install::INSTALL_TOOLS.contains(&call.name) {
            assert_eq!(outcome, self.expected);
            self.owner.poison_fixture_state();
        }
        self.admission
            .record_dispatch_outcome(call, context, effect_kind, outcome)
            .await
    }
}

#[tokio::test]
async fn checked_install_poison_after_observation_preserves_native_result_and_custody() {
    for selected in [false, true] {
        for outcome in [
            LiveBridgeEffectOutcome::Committed,
            LiveBridgeEffectOutcome::Failed,
            LiveBridgeEffectOutcome::Unknown,
        ] {
            let mut f = Fixture::new(
                selected,
                if outcome == LiveBridgeEffectOutcome::Unknown {
                    1
                } else {
                    0
                },
                DECISION_LIFETIME,
            );
            f.check().await;
            let policy = if outcome == LiveBridgeEffectOutcome::Failed {
                ToolAccessPolicy::DenyList([f.install_name()].into_iter().collect())
            } else {
                ToolAccessPolicy::AllowList(NAMES.iter().copied().collect())
            };
            f.gate = Arc::new(
                ExecutionPolicyGatedDispatcher::new(
                    f.owner.clone(),
                    ToolExecutionPolicy::resolve(policy).unwrap(),
                )
                .with_dispatch_admission(Arc::new(PoisonAtSettlement {
                    admission: Admission(
                        f.approvals.shared.clone(),
                        false,
                        None,
                        None,
                        None,
                        Some(f.owner.clone()),
                    ),
                    owner: f.owner.clone(),
                    expected: outcome,
                })),
            );
            let run = if outcome == LiveBridgeEffectOutcome::Failed {
                // A resolved plan would refuse the denied tool before admission.
                // Exercise native policy failure after this exact approval.
                f.start("install", f.install_name(), install(selected), false)
            } else {
                f.install()
            };
            f.prompt().await.decide(ApprovalDecision::Approve).unwrap();
            let result = run.await.unwrap();
            match outcome {
                LiveBridgeEffectOutcome::Committed => {
                    let result = result.unwrap();
                    assert_eq!(result.result.tool_use_id, "install");
                    assert!(!result.result.is_error);
                    assert!(result.async_ops.is_empty());
                    assert!(result.session_effects.is_empty());
                    assert_eq!(
                        serde_json::from_str::<Value>(&result.result.text_content()).unwrap(),
                        json!({"job":{"id":"3".repeat(32), "kind":"install", "family":if selected {"selected-python"} else {"services"}, "status":"running"}})
                    );
                }
                LiveBridgeEffectOutcome::Failed => assert!(matches!(
                    result, Err(ToolError::AccessDenied { name }) if name == f.install_name()
                )),
                LiveBridgeEffectOutcome::Unknown => assert!(matches!(
                    result, Err(ToolError::ExecutionFailed { message }) if message == "fixture lost reply"
                )),
            }
            let uncertain = outcome == LiveBridgeEffectOutcome::Unknown;
            assert_eq!(f.approvals.has_uncertain_dispatch(), uncertain);
            assert_eq!(
                f.approvals
                    .shared
                    .state()
                    .unwrap()
                    .attempt
                    .as_ref()
                    .unwrap()
                    .custody,
                if uncertain {
                    Custody::SpentUncertain
                } else {
                    Custody::Settled(outcome)
                },
            );
            assert!(!f.approvals.permits_uncertain_status_readback());
            for (name, args) in [
                (f.check_name(), selection(selected)),
                (f.install_name(), install(selected)),
            ] {
                let result = f.start("no-rearm", name, args, false).await.unwrap();
                if uncertain {
                    assert!(matches!(result, Err(ToolError::AccessDenied { .. })));
                } else {
                    assert!(matches!(result, Err(ToolError::ExecutionFailed { .. })));
                }
            }
            assert!(f.presenter.as_mut().unwrap().prompts.try_recv().is_err());
            assert_eq!(f.package.checks.load(Ordering::SeqCst), 1);
            assert_eq!(
                f.count(),
                usize::from(outcome != LiveBridgeEffectOutcome::Failed)
            );
        }
    }
}


// These controls run only a synthetic package. Default native contexts in the
// positive tests do not qualify governed helper authority or provider effects.
#[tokio::test]
async fn owner_local_review_covers_both_dispatch_forms_without_research_gate() {
    for selected in [false, true] {
        for resolved in [false, true] {
            let mut f = Fixture::local(selected, 0);
            f.check().await;
            f.start("research", "research_fixture", json!({}), resolved)
                .await.unwrap().unwrap();
            assert!(f.presenter.as_mut().unwrap().prompts.try_recv().is_err());
            let run = f.start("install", f.install_name(), install(selected), resolved);
            let mut prompt = f.prompt().await;
            assert_eq!(f.count(), 0);
            let id = prompt.record.approval_id.clone();
            let body = prompt.record.proposed_action.body.as_ref().unwrap();
            assert_eq!(body["selection"], selection(selected));
            assert_eq!(body["owner_proposal"], proposal(selected));
            prompt.decide(ApprovalDecision::Approve).unwrap();
            let result = run.await.unwrap().unwrap();
            assert!(result.settlement_failures().is_empty());
            assert_eq!(f.count(), 1);
            assert_eq!(f.approvals.shared.service.get(&id).unwrap().status, ApprovalStatus::Approved);
            assert_eq!(f.approvals.shared.state().unwrap().attempt.as_ref().unwrap().custody,
                Custody::Settled(LiveBridgeEffectOutcome::Committed));
            assert_eq!(f.package.calls.lock().unwrap().last().unwrap().3, resolved);
            assert!(f.install().await.unwrap().is_err());
            assert!(f.presenter.as_mut().unwrap().prompts.try_recv().is_err());
        }
    }
}

#[tokio::test]
async fn owner_local_review_denial_retirement_and_replacement_never_enter() {
    for refusal in ["denied", "retired", "binding_replaced"] {
        let mut f = Fixture::local(false, 0);
        f.check().await;
        let run = f.install();
        let mut prompt = f.prompt().await;
        match refusal {
            "denied" => prompt.decide(ApprovalDecision::Deny).unwrap(),
            "retired" => {
                f.approvals.retire();
                assert!(prompt.decide(ApprovalDecision::Approve).is_err());
                drop(prompt);
            }
            _ => {
                f.package.epoch.store(8, Ordering::SeqCst);
                prompt.decide(ApprovalDecision::Approve).unwrap();
            }
        }
        assert!(run.await.unwrap().is_err());
        assert_eq!(f.count(), 0);
        assert!(!f.approvals.has_uncertain_dispatch());
    }
}

#[tokio::test]
async fn owner_local_review_preserves_raw_entry_result_across_capture_failure() {
    for behavior in [1, 3] {
        let mut f = Fixture::local(false, behavior);
        f.check().await;
        let run = f.install();
        f.prompt().await.decide(ApprovalDecision::Approve).unwrap();
        assert!(run.await.unwrap().is_err());
        assert_eq!(f.count(), 1);
        assert_eq!(f.approvals.shared.state().unwrap().attempt.as_ref().unwrap().custody,
            if behavior == 1 { Custody::SpentUncertain }
            else { Custody::Settled(LiveBridgeEffectOutcome::Committed) });
        assert!(f.install().await.unwrap().is_err());
        assert!(f.presenter.as_mut().unwrap().prompts.try_recv().is_err());
    }
}

#[tokio::test]
async fn owner_local_dropped_synthetic_inner_remains_spent_and_cannot_rearm() {
    let mut f = Fixture::local(false, 2);
    f.check().await;
    let run = f.install();
    f.prompt().await.decide(ApprovalDecision::Approve).unwrap();
    f.package.entered.notified().await;
    // Test only: dropping this inert pending fake checks the custody invariant.
    // Production owners must instead stop admission and join entered work.
    run.abort();
    assert!(run.await.unwrap_err().is_cancelled());
    assert!(f.approvals.has_uncertain_dispatch());
    assert_eq!(f.count(), 1);
    assert!(f.install().await.unwrap().is_err());
    assert!(f.presenter.as_mut().unwrap().prompts.try_recv().is_err());
}

#[tokio::test]
async fn owner_local_settlement_matches_call_and_session_and_retains_failure_metadata() {
    let mut f = Fixture::local(false, 2);
    f.check().await;
    let run = f.install();
    f.prompt().await.decide(ApprovalDecision::Approve).unwrap();
    f.package.entered.notified().await;
    let raw = serde_json::value::to_raw_value(&install(false)).unwrap();
    let current = ToolDispatchContext::default().with_runtime_identity(f.session.clone(), None);
    let foreign = ToolDispatchContext::default().with_runtime_identity(SessionId::new(), None);
    let exact = ToolCallView { id: "install", name: f.install_name(), args: &raw };
    let wrong = ToolCallView { id: "different-call", name: f.install_name(), args: &raw };
    for (call, context) in [(wrong, &current), (exact, &foreign)] {
        let result = f.approvals.settle_service_result(&f.owner, call, context,
            LiveBridgeEffectOutcome::Committed,
            Ok(ToolResult::new(call.id.into(), "actual inner result".into(), false).into()),
        ).unwrap();
        assert_eq!(result.result.text_content(), "actual inner result");
        assert_eq!(result.settlement_failures().len(), 1);
        assert_eq!(result.settlement_failures()[0].physical_outcome,
            LiveBridgeEffectOutcome::Committed);
        assert!(f.approvals.has_uncertain_dispatch());
    }
    f.package.release.notify_one();
    run.await.unwrap().unwrap();
    assert_eq!(f.approvals.shared.state().unwrap().attempt.as_ref().unwrap().custody,
        Custody::Settled(LiveBridgeEffectOutcome::Committed));
}

struct RefusePreparedOperation;
impl meerkat_core::authorization::WorkAuthorization for RefusePreparedOperation {
    fn prepare(&self, _: &meerkat_core::authorization::PreparedAuthorizationBinding)
        -> Result<Arc<dyn meerkat_core::authorization::PreparedOperationAuthorization>,
            meerkat_core::authorization::OperationAuthorizationError> {
        Err(meerkat_core::authorization::OperationAuthorizationError::Unavailable)
    }
}

#[tokio::test]
async fn owner_local_checks_actual_native_context_after_approval_before_entry() {
    let mut f = Fixture::local(false, 0);
    f.check().await;
    let owner = f.owner.clone();
    let session = f.session.clone();
    let run = tokio::spawn(async move {
        // Deliberately malformed native work context: a public work host with
        // no native prepared check. The actual current-call API must refuse it.
        // This fixture never manufactures an admitted authorization payload.
        let context = ToolDispatchContext::default().with_runtime_identity(session, None)
            .with_work_authorization(Some(meerkat_core::authorization::WorkAuthorizationContext::new(
                Arc::new(RefusePreparedOperation),
                meerkat_core::exact_operation::OperationExecutionScope::Domain,
            )));
        let raw = serde_json::value::to_raw_value(&install(false)).unwrap();
        let call = ToolCallView { id: "install", name: "setup_install", args: &raw };
        owner.dispatch_with_context(call, &context).await
    });
    f.prompt().await.decide(ApprovalDecision::Approve).unwrap();
    assert!(matches!(run.await.unwrap().unwrap_err().primary_error(),
        ToolError::AuthorizationRefused { .. }));
    assert_eq!(f.count(), 0);
    assert_eq!(f.approvals.shared.state().unwrap().attempt.as_ref().unwrap().custody,
        Custody::Settled(LiveBridgeEffectOutcome::Failed));
}

#[tokio::test]
async fn checked_install_keeps_exact_package_recovery_reference_and_refuses_rebinding() {
    let mut f = Fixture::new(false, 0, DECISION_LIFETIME);
    f.check().await;
    let run = f.install();
    f.prompt().await.decide(ApprovalDecision::Approve).unwrap();
    run.await.unwrap().unwrap();
    let mut exact = job_readback(false);
    exact["job"]["recovery_request"] = json!({"id":"3".repeat(32),"sha256":"9".repeat(64)});
    *f.package.readback_reply.lock().unwrap() = Some(exact.clone());
    f.start("job", "setup_job", json!({"job_id":"3".repeat(32)}), true).await.unwrap().unwrap();
    assert_eq!(f.owner.installed_selection(&f.realm, &f.session, &"3".repeat(32)).unwrap()
        .installed.unwrap().recovery_request_sha256, Some("9".repeat(64)));
    for change in ["id", "sha256", "missing"] {
        let mut changed = exact.clone();
        if change == "missing" { changed["job"].as_object_mut().unwrap().remove("recovery_request"); }
        else { changed["job"]["recovery_request"][change] = if change == "id" { "4".repeat(32) } else { "8".repeat(64) }.into(); }
        *f.package.readback_reply.lock().unwrap() = Some(changed);
        assert!(f.start("changed", "setup_job", json!({"job_id":"3".repeat(32)}), true).await.unwrap().is_err());
        let held = f.owner.installed_selection(&f.realm, &f.session, &"3".repeat(32)).unwrap();
        assert!(!held.matched);
        assert_eq!(held.installed.unwrap().recovery_request_sha256, Some("9".repeat(64)));
    }
    *f.package.readback_reply.lock().unwrap() = Some(exact);
    f.start("recover", "setup_job", json!({"job_id":"3".repeat(32)}), true).await.unwrap().unwrap();
    assert!(f.owner.installed_selection(&f.realm, &f.session, &"3".repeat(32)).unwrap().matched);
    assert_eq!(f.count(), 1, "readback never installs again");
}

#[path = "service_install_tests/console_lifetime.rs"]
mod console_lifetime;

#[tokio::test]
async fn checked_install_single_structured_reply_retains_proposal_and_exact_receipt() {
    for selected in [false, true] {
        let mut f = Fixture::new(selected, 0, DECISION_LIFETIME);
        f.package.structured_reply.store(true, Ordering::SeqCst);
        f.check().await;
        assert_eq!(f.package.checks.load(Ordering::SeqCst), 1);
        assert!(f.presenter.as_mut().unwrap().prompts.try_recv().is_err());
        assert_eq!(f.count(), 0);
        let run = f.install();
        let mut prompt = f.prompt().await;
        let body = prompt.record.proposed_action.body.as_ref().unwrap();
        assert_eq!(body["selection"], selection(selected));
        assert_eq!(body["owner_proposal"], proposal(selected));
        assert_eq!(body["scope"], "software_installation_only");
        assert_eq!(f.count(), 0);
        prompt.decide(ApprovalDecision::Approve).unwrap();
        let outcome = run.await.unwrap().unwrap();
        assert_eq!(outcome.result.tool_use_id, "install");
        assert!(!outcome.result.is_error);
        assert!(matches!(outcome.result.content.as_slice(),
            [meerkat_core::ContentBlock::Structured { .. }]));
        assert!(outcome.async_ops.is_empty() && outcome.session_effects.is_empty());
        assert_eq!(f.count(), 1);
        assert!(!f.approvals.has_uncertain_dispatch());
        assert!(f.install().await.unwrap().is_err());
        assert_eq!(f.count(), 1);

        *f.package.readback_reply.lock().unwrap() = Some(job_readback(selected));
        f.start("job", "setup_job", json!({"job_id":"3".repeat(32)}), true)
            .await.unwrap().unwrap();
        let capture = f.owner.installed_selection(
            &f.realm, &f.session, &"3".repeat(32),
        ).unwrap();
        assert!(capture.matched);
        let installed = capture.installed.unwrap();
        assert_eq!(installed.realm, f.realm);
        assert_eq!(installed.session, f.session);
        assert_eq!(installed.validation_id,
            if selected { "b".repeat(32) } else { "a".repeat(32) });
        assert!(!format!("{installed:?}").contains("do-not-retain-this-value"));
        assert!(!format!("{installed:?}").contains("FIXTURE_SECRET"));
        assert!(f.owner.installed_selection(
            &f.realm, &SessionId::new(), &"3".repeat(32),
        ).is_err());
        f.package.epoch.store(8, Ordering::SeqCst);
        assert!(!f.owner.installed_selection(
            &f.realm, &f.session, &"3".repeat(32),
        ).unwrap().matched);
        assert_eq!(f.count(), 1);
        let calls = f.package.calls.lock().unwrap();
        assert_eq!(calls.len(), 3);
        assert!(calls.iter().all(|(_, _, session, resolved)|
            session == &f.session && *resolved));
    }
}

#[tokio::test]
async fn checked_install_refused_structured_reply_leaves_no_approvable_proposal() {
    use meerkat_core::ContentBlock;
    let structured = |raw: &str| ContentBlock::Structured {
        data: RawValue::from_string(raw.into()).unwrap(),
    };
    for selected in [false, true] {
        let valid = proposal(selected).to_string();
        let duplicated = format!("{},\"scope\":\"other\"}}", valid.strip_suffix('}').unwrap());
        let mut wrong_scope = proposal(selected);
        wrong_scope["scope"] = "different_scope".into();
        for blocks in [
            vec![structured(&valid), structured(&valid)],
            vec![ContentBlock::Text { text: valid.clone() }, structured(&valid)],
            vec![structured(&valid), ContentBlock::Text { text: valid.clone() }],
            vec![structured(&duplicated)],
            vec![structured("[]")],
            vec![structured(&wrong_scope.to_string())],
        ] {
            let mut f = Fixture::new(selected, 0, DECISION_LIFETIME);
            *f.package.check_blocks.lock().unwrap() = Some(blocks);
            assert!(f.start("check", f.check_name(), selection(selected), true)
                .await.unwrap().is_err());
            assert_eq!(f.package.checks.load(Ordering::SeqCst), 1);
            assert_eq!(f.package.calls.lock().unwrap().len(), 1);
            assert_eq!(f.count(), 0);
            assert!(f.presenter.as_mut().unwrap().prompts.try_recv().is_err());
            // An invalid check cannot mint the actual owner-held action that
            // would be needed to arm an install. Do not submit another call.
            let args = serde_json::value::to_raw_value(&install(selected)).unwrap();
            assert!(f.owner.retain_action(ToolCallView {
                id: "install", name: f.install_name(), args: &args,
            }, &f.session).is_err());
            assert!(!f.approvals.has_uncertain_dispatch());
        }
    }
}

#[cfg(all(unix, feature = "console"))]
mod application_policy_tests {
use super::*;

// Actual installed file/phase owner and native ApprovalService, with the same
// synthetic package used above. This does not qualify a governed native helper,
// account, live installation, application policy deployment or service grant.
struct PolicyOwnerFixture {
    lock: Arc<tokio::sync::RwLock<()>>,
    owner: meerkat_core::PrincipalRef,
    calls: AtomicUsize,
    released: Arc<AtomicUsize>,
    mode: u8,
    exact_effect: String,
    change_epoch: Mutex<Option<Arc<Package>>>,
}
struct PolicyReservationFixture {
    _reservation: tokio::sync::OwnedRwLockReadGuard<()>,
    owner: meerkat_core::PrincipalRef,
    evidence: serde_json::Map<String, Value>,
    released: Arc<AtomicUsize>,
    stale: bool,
}
impl Drop for PolicyReservationFixture {
    fn drop(&mut self) { self.released.fetch_add(1, Ordering::SeqCst); }
}
impl installation_policy::InstallationPolicyReservation for PolicyReservationFixture {
    fn owner(&self) -> &meerkat_core::PrincipalRef { &self.owner }
    fn provenance(&self) -> &serde_json::Map<String, Value> { &self.evidence }
    fn check_current(&self) -> Result<(), meerkat_toolkit::host_lifetime::control::HostLifetimeError> {
        if self.stale { Err(meerkat_toolkit::host_lifetime::control::HostLifetimeError::Denied) } else { Ok(()) }
    }
}
impl installation_policy::ApplicationInstallationPolicy for PolicyOwnerFixture {
    fn reserve(&self, request: &installation_policy::InstallationPolicyRequest<'_>)
        -> Result<installation_policy::InstallationPolicyDecision, meerkat_toolkit::host_lifetime::control::HostLifetimeError>
    {
        use installation_policy::InstallationPolicyDecision;
        use meerkat_toolkit::host_lifetime::control::HostLifetimeError;
        self.calls.fetch_add(1, Ordering::SeqCst);
        if request.proposal["owner_proposal"]["effect_sha256"] != self.exact_effect
            || request.call.name != "setup_selected_install"
            || request.context.origin_session_id().is_none()
        { return Err(HostLifetimeError::Denied); }
        match self.mode {
            1 => return Ok(InstallationPolicyDecision::RequiresHuman),
            2 => return Err(HostLifetimeError::Denied),
            3 => return Err(HostLifetimeError::Unavailable),
            _ => {},
        }
        let reservation = self.lock.clone().try_read_owned().map_err(|_| HostLifetimeError::Unavailable)?;
        if let Some(package) = self.change_epoch.lock().unwrap().take() {
            package.epoch.fetch_add(1, Ordering::SeqCst);
        }
        let owner = if self.mode == 4 {
            meerkat_core::PrincipalRef::in_domain(meerkat_core::PrincipalKind::ServiceAccount,
                "other-policy", meerkat_core::TrustDomainId::new("fixture").unwrap()).unwrap()
        } else { self.owner.clone() };
        Ok(InstallationPolicyDecision::Permit(Box::new(PolicyReservationFixture {
            _reservation: reservation, owner,
            evidence: serde_json::Map::from_iter([("rule".into(), json!("exact-fixture-effect"))]),
            released: self.released.clone(), stale: self.mode == 5,
        })))
    }
}

fn policy_lifetime(root: &Path, realm: &meerkat_core::RealmId)
    -> (Arc<meerkat_toolkit::host_lifetime::NativeHostLifetime>, PathBuf)
{
    policy_lifetime_with_selection(root, realm, None, false)
}

fn policy_lifetime_with_selection(root: &Path, realm: &meerkat_core::RealmId,
    selected: Option<crate::application_acquisition::selected_policy::StandingPolicySelection>,
    administration: bool)
    -> (Arc<meerkat_toolkit::host_lifetime::NativeHostLifetime>, PathBuf)
{
    use meerkat_toolkit::host_lifetime::{NativeHostLifetime, control::{HostBootstrapSelection, SelectedHostArtifact}, policy::*};
    use sha2::{Digest, Sha256};
    let hash = |bytes: &[u8]| format!("{:x}", Sha256::digest(bytes));
    let deployment = root.canonicalize().unwrap().join("application");
    let bundle = deployment.join("shared/policy/bundle.1");
    std::fs::create_dir_all(&bundle).unwrap();
    std::fs::create_dir_all(deployment.join("shared/activations")).unwrap();
    std::fs::create_dir_all(deployment.join("shared/control")).unwrap();
    let write = |path: PathBuf, bytes: &[u8]| {
        std::fs::write(&path, bytes).unwrap();
        SelectedHostArtifact { path, sha256: hash(bytes) }
    };
    let source = write(bundle.join("policy.json"), br#"{"schema":1}"#);
    let principal = |name| meerkat_core::PrincipalRef::in_domain(
        meerkat_core::PrincipalKind::ServiceAccount, name,
        meerkat_core::TrustDomainId::new("fixture").unwrap()).unwrap();
    let application = principal("application");
    let tool = write(bundle.join("tools.json"), b"[]");
    let reference = CompiledArtifactReference { name: "tools.json".into(), sha256: tool.sha256.clone() };
    let setup = selected.map(|selected| {
        let artifact = write(bundle.join("native-setup.json"), &toolkit_package::canonical_json(&selected).unwrap());
        CompiledArtifactReference { name: "native-setup.json".into(), sha256: artifact.sha256 }
    });
    let mut policy = CompiledLocalPolicy { schema: 7, deployment: toolkit_package::Id::new("fixture").unwrap(),
        source_policy_sha256: source.sha256.clone(), components: vec![], channels: vec![],
        native_host: Some(CompiledNativeHost { realm: realm.clone(), grant_administrator: application.clone(),
            remote_operator: None, service_parent_administration: None, setup: setup.clone(),
            ingress_domain: meerkat_authorization_contracts::constraints::ResourceDomain {
                authority: application.clone(), namespace: "fixture".into(),
            }, application_tool_policy: reference.clone(), routes: vec![], members: vec![CompiledNativeMember {
                member: meerkat_core::MobMemberBinding { mob_id: "unattended-fixture".into(),
                    role: "setup".into(), member: "helper".into() }, logical_owner: principal("helper"),
                context_generation: 1, audience: meerkat_authorization_contracts::constraints::AudienceRef::Principal {
                    principal: application,
                }, initial_surface: reference, setup_capability: None,
            }],
        }),
    };
    if administration {
        // Only a selected-owner fixture: this actual stock principal value is
        // not an opened grant owner or proof of administrative authentication.
        let native = policy.native_host.as_mut().unwrap();
        native.remote_operator = Some(meerkat::governed_realm::stock::StockPrincipals::for_local_operator(realm).unwrap().operator);
        let route = toolkit_package::Id::new("policy-service-route").unwrap();
        let application = native.grant_administrator.clone();
        let domain = native.ingress_domain.clone();
        native.routes.push(CompiledNativeSourceRoute {
            id: route.clone(), origin: CompiledNativeSourceOrigin::LocalConsole {
                member: native.members[0].member.clone(), requester: application.clone(), ingress_actor: application.clone(),
            }, application_parent: None, controller_parent: None,
            logical_executor: application.clone(), represented_subject: None, source_authority: application,
            authentication_domain: domain.clone(), event_domain: domain,
        });
        native.service_parent_administration = Some(ServiceParentAdministration::ApplicationPolicyTasks {
            routes: [route].into_iter().collect(),
        });
    }
    let local = write(bundle.join("toolkit-local-policy.json"), &policy.canonical_bytes().unwrap());
    let mut artifacts = vec![json!({"path":"policy.json","sha256":source.sha256}),
        json!({"path":"toolkit-local-policy.json","sha256":local.sha256}),
        json!({"path":"tools.json","sha256":tool.sha256})];
    if let Some(setup) = setup { artifacts.push(json!({"path":setup.name,"sha256":setup.sha256})); }
    let manifest = write(bundle.join("manifest.json"), &toolkit_package::canonical_json(&json!({"artifacts":artifacts})).unwrap());
    let activation = write(deployment.join("shared/activations/1.json"), &toolkit_package::canonical_json(&json!({
        "activation_id":"1","policy_bundle":{"path":"shared/policy/bundle.1",
            "policy_digest":format!("hc-policy-v1:sha256:{}",manifest.sha256)}
    })).unwrap());
    std::os::unix::fs::symlink("shared/activations/1.json", deployment.join("current")).unwrap();
    let mut lifetime = NativeHostLifetime::bootstrap(HostBootstrapSelection {
        deployment_root: deployment, activation, policy_manifest: manifest, local_policy: local,
    }).unwrap();
    lifetime.bind_native_authorization_publication(meerkat_authorization::publication::LocalAuthorizationPublication::new()).unwrap();
    (Arc::new(lifetime), source.path)
}

fn policy_fixture(behavior: u8, mode: u8) -> (Fixture, Arc<PolicyOwnerFixture>, PathBuf) {
    let mut retained = None;
    let realm = meerkat_core::mob_realm_id("unattended-fixture").unwrap();
    let mut fixture = Fixture::build(true, behavior, realm.as_str(), |root, realm| {
        let (lifetime, source) = policy_lifetime(root, &realm.realm);
        let provider = Arc::new(PolicyOwnerFixture { lock: Arc::new(tokio::sync::RwLock::new(())),
            owner: lifetime.selected_policy().native_host.as_ref().unwrap().grant_administrator.clone(),
            calls: AtomicUsize::new(0), released: Arc::new(AtomicUsize::new(0)), mode,
            exact_effect: "d".repeat(64), change_epoch: Mutex::new(None) });
        let policy = installation_policy::InstalledInstallationPolicy::new(lifetime, provider.clone()).unwrap();
        retained = Some((provider, source));
        (LocalApprovals::create_for_installation_policy(root, policy).unwrap(), None)
    });
    fixture.owner = CheckedInstalls::fixture_with_review(fixture.package.clone(),
        Arc::new(LocalApprovals { shared: fixture.approvals.shared.clone(), service_tasks: None }));
    fixture.owner.bind_realm(&fixture.realm).unwrap();
    fixture.gate = Arc::new(ExecutionPolicyGatedDispatcher::new(fixture.owner.clone(),
        ToolExecutionPolicy::resolve(ToolAccessPolicy::AllowList(NAMES.iter().copied().collect())).unwrap()));
    let (provider, source) = retained.unwrap();
    (fixture, provider, source)
}

#[tokio::test]
async fn application_policy_installs_exact_action_without_a_presenter_and_records_real_owner() {
    let (f, policy, _) = policy_fixture(0, 0);
    f.check().await;
    let result = f.install().await.unwrap().unwrap();
    assert!(!result.result.is_error);
    assert!(f.presenter.is_none());
    assert!(f.approvals.shared.prompts.is_closed());
    assert_eq!(f.count(), 1);
    let records = f.approvals.shared.service.list(Default::default()).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].status, ApprovalStatus::Approved);
    assert_eq!(records[0].decision.as_ref().unwrap().actor,
        ApprovalPrincipalId::new(serde_json::to_string(&policy.owner).unwrap()).unwrap());
    assert_eq!(records[0].request_provenance.as_ref().unwrap()["application"], json!(policy.owner));
    assert_eq!(records[0].request_provenance.as_ref().unwrap()["decision"], json!({"rule":"exact-fixture-effect"}));
    assert_eq!(records[0].proposed_action.body.as_ref().unwrap()["owner_proposal"]["effect_sha256"], "d".repeat(64));
    assert!(policy.lock.try_write().is_ok());
    assert_eq!(policy.released.load(Ordering::SeqCst), 1);
    assert!(f.install().await.unwrap().is_err());
    assert_eq!(f.count(), 1);
}

#[tokio::test]
async fn application_policy_human_step_and_refusals_create_no_approval_or_install_arm() {
    for mode in 1..=5 {
        let (f, policy, _) = policy_fixture(0, mode);
        f.check().await;
        let result = f.install().await.unwrap();
        if mode == 1 {
            let result = result.unwrap();
            assert!(!result.result.is_error);
            assert_eq!(serde_json::from_str::<Value>(&result.result.text_content()).unwrap(),
                json!({"state":"requires_action","action":"human_installation_review","installed":false}));
        } else {
            let error = result.unwrap_err();
            if mode == 3 {
                assert_eq!(std::mem::discriminant(&error), std::mem::discriminant(&ToolError::unavailable(
                    f.install_name(), meerkat_core::ToolUnavailableReason::TemporarilyUnavailable)));
            } else { assert!(matches!(error, ToolError::AccessDenied { .. })); }
        }
        assert!(f.approvals.shared.service.list(Default::default()).unwrap().is_empty());
        assert!(f.approvals.shared.state().unwrap().attempt.is_none());
        assert_eq!(f.count(), 0);
        assert!(policy.lock.try_write().is_ok());
        assert!(f.presenter.is_none());
    }
}

#[tokio::test]
async fn application_policy_rechecks_selected_artifact_and_exact_arm_after_reservation() {
    let (f, policy, source) = policy_fixture(0, 0);
    f.check().await;
    std::fs::write(source, b"changed selected application policy").unwrap();
    assert!(f.install().await.unwrap().is_err());
    assert_eq!(policy.calls.load(Ordering::SeqCst), 0);
    assert!(f.approvals.shared.service.list(Default::default()).unwrap().is_empty());
    assert_eq!(f.count(), 0);

    let (f, policy, _) = policy_fixture(0, 0);
    f.check().await;
    *policy.change_epoch.lock().unwrap() = Some(f.package.clone());
    assert!(f.install().await.unwrap().is_err());
    let records = f.approvals.shared.service.list(Default::default()).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].status, ApprovalStatus::Approved);
    assert_eq!(f.count(), 0);
    assert!(policy.lock.try_write().is_ok());
    assert_eq!(policy.released.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn application_policy_drop_releases_reservation_but_keeps_uncertain_replay_exclusion() {
    let (f, policy, _) = policy_fixture(2, 0);
    f.check().await;
    let entered = f.package.entered.notified();
    let run = f.install();
    tokio::pin!(entered);
    let observed = tokio::time::timeout(Duration::from_secs(2), entered.as_mut()).await;
    let held = policy.lock.try_write().is_err();
    run.abort();
    let cancelled = run.await;
    assert!(observed.is_ok());
    assert!(held);
    assert!(cancelled.unwrap_err().is_cancelled());
    assert!(policy.lock.try_write().is_ok());
    assert_eq!(policy.released.load(Ordering::SeqCst), 1);
    assert!(f.approvals.has_uncertain_dispatch());
    assert!(f.install().await.unwrap().is_err());
    assert_eq!(f.count(), 1);
}

#[tokio::test]
async fn application_policy_lost_reply_releases_reservation_without_rearming() {
    let (f, policy, _) = policy_fixture(1, 0);
    f.check().await;
    assert!(f.install().await.unwrap().is_err());
    assert!(policy.lock.try_write().is_ok());
    assert_eq!(policy.released.load(Ordering::SeqCst), 1);
    assert!(f.approvals.has_uncertain_dispatch());
    assert!(f.install().await.unwrap().is_err());
    assert_eq!(f.count(), 1);
}

#[tokio::test]
async fn application_policy_closed_lifetime_refuses_before_dispatch_and_releases_policy() {
    use meerkat_toolkit::host_lifetime::control::{HostCloseReason, PrivateHostLifetimeControl};
    let (f, policy, _) = policy_fixture(0, 0);
    f.check().await;
    let args = serde_json::value::to_raw_value(&install(true)).unwrap();
    let call = ToolCallView { id: "install", name: f.install_name(), args: &args };
    let context = ToolDispatchContext::default().with_runtime_identity(f.session.clone(), None);
    assert!(matches!(f.approvals.await_service_dispatch(&f.owner, call, &context, None).await.unwrap(),
        SoftwareReviewOutcome::Approved));
    let guard = f.approvals.service_dispatch_guard(call, &context).unwrap();
    let lifetime = f.approvals.shared.installation_policy.as_ref().unwrap().resolve().unwrap().unwrap().lifetime().clone();
    let closing = lifetime.begin_close_activation(HostCloseReason::OperatorDisable).unwrap();
    let refused = f.approvals.check_service_entry(call, &context);
    let result = f.approvals.settle_service_result(&f.owner, call, &context,
        LiveBridgeEffectOutcome::Failed, Err(ToolError::access_denied(call.name)));
    drop(guard);
    let closed = lifetime.join_close(&closing.handle).await;
    assert!(closed.is_ok());
    assert!(refused.is_err());
    assert!(result.is_err());
    assert_eq!(f.count(), 0);
    assert!(policy.lock.try_write().is_ok());
    assert_eq!(policy.released.load(Ordering::SeqCst), 1);
    assert!(!f.approvals.has_uncertain_dispatch());
}


#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn application_policy_record_and_entry_do_not_invert_phase_and_approval_state() {
    use std::sync::mpsc;
    for entry_check in [false, true] {
        let (f, policy, _) = policy_fixture(0, 0);
        f.check().await;
        let args = serde_json::value::to_raw_value(&install(true)).unwrap();
        let call = ToolCallView { id: "install", name: f.install_name(), args: &args };
        let context = ToolDispatchContext::default().with_runtime_identity(f.session.clone(), None);
        let installed_policy = f.approvals.shared.installation_policy.as_ref().unwrap().resolve().unwrap().unwrap();
        let lifetime = installed_policy.lifetime().clone();
        let retained = if entry_check {
            f.approvals.await_service_dispatch(&f.owner, call, &context, None).await.unwrap();
            None
        } else {
            // Reserve before holding phase so this reaches the real decision
            // seam, not the earlier reserve-time selection observation.
            let held = f.owner.retain_action(call, &f.session).unwrap();
            let mut reviewed = held.review();
            reviewed["tool"] = call.name.into();
            reviewed["call_id"] = call.id.into();
            let entry = installed_policy.reserve(held.realm(), call, &context, &reviewed).unwrap().unwrap();
            Some((held, reviewed, entry))
        };
        // Keep this wrapper alive through settlement: its Drop retires Shared.
        let approvals = Arc::new(LocalApprovals { shared: f.approvals.shared.clone(), service_tasks: None });
        let (phase_ready, phase_observed) = tokio::sync::oneshot::channel();
        let (phase_release, release) = mpsc::sync_channel(1);
        let observing = approvals.clone();
        let phase = tokio::task::spawn_blocking(move || lifetime.with_current_setup(|| {
            // Nonblocking acquisition only: never wait for State under entry.
            // Once both are held, an entry-first contender must refuse without
            // trying State; a State-first contender cannot complete yet.
            let state = observing.shared.state.try_lock()
                .map_err(|_| meerkat_toolkit::host_lifetime::control::HostLifetimeError::Unavailable)?;
            let custody = state.attempt.as_ref().map(|attempt| attempt.custody);
            let _ = phase_ready.send(custody);
            let released = release.recv_timeout(Duration::from_secs(5)).is_ok();
            drop(state);
            Ok((released, custody))
        }));
        let ready = tokio::time::timeout(Duration::from_secs(2), phase_observed).await;
        let runtime = tokio::runtime::Handle::current();
        let owner = f.owner.clone();
        let worker_approvals = approvals.clone();
        let (attempt_completed, completed) = tokio::sync::oneshot::channel();
        let operation = tokio::task::spawn_blocking(move || {
            let call = ToolCallView { id: "install", name: "setup_selected_install", args: &args };
            let result = match retained {
                Some((held, reviewed, entry)) => runtime.block_on(worker_approvals.shared.review_dispatch(
                    call, Some(&context), Some(HeldAction::Service(held)), reviewed, None, Some(entry), |held| match held {
                        HeldAction::Service(held) => owner.arm_action(held),
                        _ => Err(ToolError::access_denied(call.name)),
                    })),
                None => worker_approvals.check_service_entry(call, &context),
            };
            let _ = attempt_completed.send(result);
        });
        // Completion must occur while setup holds both entry and State.
        // A State-first path times out without trapping either worker: release
        // and join before inspecting any result.
        let contended = tokio::time::timeout(Duration::from_secs(2), completed).await;
        let effects_while_held = f.count();
        let released_while_held = policy.released.load(Ordering::SeqCst);
        let _ = phase_release.send(());
        let phase_result = phase.await;
        let operation_result = operation.await;

        // The refused record did not consume its proposal. After release, the
        // same real review/entry paths must succeed without changing authority.
        let args = serde_json::value::to_raw_value(&install(true)).unwrap();
        let call = ToolCallView { id: "install", name: f.install_name(), args: &args };
        let context = ToolDispatchContext::default().with_runtime_identity(f.session.clone(), None);
        let uncontended_review = if entry_check { None } else {
            Some(f.approvals.await_service_dispatch(&f.owner, call, &context, None).await)
        };
        let uncontended_entry = f.approvals.check_service_entry(call, &context);
        let current = f.approvals.check_session(&f.session);
        let records = f.approvals.shared.service.list(Default::default());
        // No installation is dispatched. Settle the unused arm with a known
        // failure, then check that real policy/custody/arm ownership was released.
        let guard = f.approvals.service_dispatch_guard(call, &context).ok();
        let had_guard = guard.is_some();
        let settled = f.approvals.settle_service_result(&f.owner, call, &context,
            LiveBridgeEffectOutcome::Failed, Err(ToolError::access_denied(call.name)));
        drop(guard);
        let reusable = f.owner.retain_action(call, &f.session).is_ok();

        let expected_custody = if entry_check { Some(Custody::SpentUncertain) } else { None };
        assert_eq!(ready.unwrap().unwrap(), expected_custody);
        assert_eq!(phase_result.unwrap().unwrap(), (true, expected_custody));
        assert!(operation_result.is_ok());
        assert_eq!(contended.unwrap().unwrap(), Err(ToolError::unavailable(
            f.install_name(), meerkat_core::ToolUnavailableReason::TemporarilyUnavailable)));
        assert_eq!(effects_while_held, 0);
        assert_eq!(released_while_held, if entry_check { 0 } else { 1 });
        if !entry_check { assert!(matches!(uncontended_review, Some(Ok(SoftwareReviewOutcome::Approved)))); }
        assert!(uncontended_entry.is_ok());
        assert!(current.is_ok());
        let records = records.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].status, ApprovalStatus::Approved);
        assert!(had_guard);
        assert_eq!(settled.unwrap_err(), ToolError::access_denied(f.install_name()));
        assert!(!f.approvals.has_uncertain_dispatch());
        assert!(reusable);
        assert_eq!(f.count(), 0);
        assert!(policy.lock.try_write().is_ok());
        assert_eq!(policy.calls.load(Ordering::SeqCst), if entry_check { 1 } else { 2 });
        assert_eq!(policy.released.load(Ordering::SeqCst), if entry_check { 1 } else { 2 });
    }
}


// This low-level selected-owner fixture pins a small policy document in a real
// activation. The full acquisition document is exercised separately by the
// application_setup closure control; the package dispatcher here is synthetic.
fn selected_fixture(behavior: u8,
    selection: crate::application_acquisition::selected_policy::StandingPolicySelection,
    presenter: bool,
) -> (Fixture, Arc<crate::application_acquisition::selected_policy::SelectedPolicyBinding>,
    Arc<meerkat_toolkit::host_lifetime::NativeHostLifetime>, PathBuf)
{
    selected_fixture_with_administration(behavior, selection, presenter, false)
}
fn selected_fixture_with_administration(behavior: u8,
    selection: crate::application_acquisition::selected_policy::StandingPolicySelection,
    presenter: bool, administration: bool,
) -> (Fixture, Arc<crate::application_acquisition::selected_policy::SelectedPolicyBinding>,
    Arc<meerkat_toolkit::host_lifetime::NativeHostLifetime>, PathBuf)
{
    use crate::application_acquisition::selected_policy::SelectedPolicyBinding;
    let mut retained = None;
    let realm = meerkat_core::mob_realm_id("unattended-fixture").unwrap();
    let mut fixture = Fixture::build(true, behavior, realm.as_str(), |root, realm| {
        let (lifetime, source) = policy_lifetime_with_selection(root, &realm.realm, Some(selection), administration);
        let native = lifetime.selected_policy().native_host.as_ref().unwrap();
        let slot = SelectedPolicyBinding::new(selection, native.grant_administrator.clone(),
            lifetime.selected_policy().deployment.clone(), realm.realm.clone(),
            native.members.iter().map(|row| row.member.member.clone()).collect());
        let approvals = LocalApprovals::create_for_selected_policy(root, slot.clone(), presenter.then_some(false)).unwrap();
        retained = Some((slot, lifetime, source));
        approvals
    });
    fixture.owner = CheckedInstalls::fixture_with_review(fixture.package.clone(),
        Arc::new(LocalApprovals { shared: fixture.approvals.shared.clone(), service_tasks: None }));
    fixture.owner.bind_realm(&fixture.realm).unwrap();
    fixture.gate = Arc::new(ExecutionPolicyGatedDispatcher::new(fixture.owner.clone(),
        ToolExecutionPolicy::resolve(ToolAccessPolicy::AllowList(NAMES.iter().copied().collect())).unwrap()));
    let (slot, lifetime, source) = retained.unwrap();
    (fixture, slot, lifetime, source)
}
fn checked_selection() -> crate::application_acquisition::selected_policy::StandingPolicySelection {
    use crate::application_acquisition::selected_policy::*;
    StandingPolicySelection { installation: InstallationSelection::CheckedSoftware, access: AccessSelection::ConfiguredParent, tool_policy: None }
}
fn bind_selected(slot: &crate::application_acquisition::selected_policy::SelectedPolicyBinding,
    lifetime: &Arc<meerkat_toolkit::host_lifetime::NativeHostLifetime>,
    selection: crate::application_acquisition::selected_policy::StandingPolicySelection)
    -> Result<(), meerkat_toolkit::host_lifetime::control::HostLifetimeError> {
    let reference = lifetime.selected_policy().native_host.as_ref().unwrap().setup.as_ref().unwrap();
    slot.bind(lifetime.clone(), selection, reference)
}

#[tokio::test]
async fn selected_policy_defers_to_exact_adopted_owner_and_cannot_rebind() {
    use crate::application_acquisition::selected_policy::InstallationSelection;
    use meerkat_toolkit::host_lifetime::control::HostLifetimeError;
    let selection = checked_selection();
    let (mut f, slot, lifetime, _) = selected_fixture(0, selection, true);
    f.check().await;
    let unavailable = f.install().await.unwrap().unwrap_err();
    assert_eq!(std::mem::discriminant(&unavailable), std::mem::discriminant(&ToolError::unavailable(
        f.install_name(), meerkat_core::ToolUnavailableReason::TemporarilyUnavailable)));
    assert!(matches!(slot.service(), Err(HostLifetimeError::Unavailable)));
    assert!(f.approvals.shared.service.list(Default::default()).unwrap().is_empty());
    assert!(f.approvals.shared.state().unwrap().attempt.is_none());
    assert!(f.presenter.as_mut().unwrap().prompts.try_recv().is_err());
    assert_eq!(f.count(), 0);
    let mut wrong = selection;
    wrong.installation = InstallationSelection::HumanReview;
    assert!(bind_selected(&slot, &lifetime, wrong).is_err());
    bind_selected(&slot, &lifetime, selection).unwrap();
    let bound = slot.installation().unwrap().unwrap();
    assert!(Arc::ptr_eq(bound.lifetime(), &lifetime));
    assert!(slot.service().unwrap().unwrap().belongs_to(&lifetime));
    assert!(bind_selected(&slot, &lifetime, selection).is_err());
    let other = tempfile::tempdir().unwrap();
    let (other_owner, _) = policy_lifetime_with_selection(other.path(), &f.realm.realm, Some(selection), false);
    assert!(bind_selected(&slot, &other_owner, selection).is_err());
    assert!(Arc::ptr_eq(slot.installation().unwrap().unwrap().lifetime(), &lifetime));
    assert!(f.install().await.unwrap().is_ok());
    assert_eq!(f.count(), 1);
    // A real available presenter was not used for pending state or policy permit.
    assert!(f.presenter.as_mut().unwrap().prompts.try_recv().is_err());
}

#[tokio::test]
async fn selected_policy_records_exact_outside_catalog_action_and_real_owner_without_presenter() {
    let selection = checked_selection();
    let (f, slot, lifetime, _) = selected_fixture(0, selection, false);
    bind_selected(&slot, &lifetime, selection).unwrap();
    f.check().await;
    let result = f.install().await.unwrap().unwrap();
    assert!(!result.result.is_error);
    assert!(f.presenter.is_none());
    assert!(f.approvals.shared.prompts.is_closed());
    let records = f.approvals.shared.service.list(Default::default()).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].status, ApprovalStatus::Approved);
    assert_eq!(records[0].decision.as_ref().unwrap().actor,
        ApprovalPrincipalId::new(serde_json::to_string(slot.owner()).unwrap()).unwrap());
    let provenance = records[0].request_provenance.as_ref().unwrap();
    assert_eq!(provenance["application"], json!(slot.owner()));
    assert_eq!(provenance["decision"]["kind"], "selected_application_setup_policy");
    assert_eq!(provenance["decision"]["selection"], json!(selection));
    assert_eq!(provenance["decision"]["setup"], json!(lifetime.selected_policy().native_host.as_ref().unwrap().setup));
    assert_eq!(records[0].proposed_action.body.as_ref().unwrap()["owner_proposal"]["effect_sha256"], "d".repeat(64));
    assert_eq!(f.package.reply.lock().unwrap()["package"]["publisher"], "fixture");
    assert!(f.install().await.unwrap().is_err());
    assert_eq!(f.count(), 1);
}

#[tokio::test]
async fn selected_human_mode_uses_same_presenter_or_returns_action_without_arming() {
    use crate::application_acquisition::selected_policy::InstallationSelection;
    let mut selection = checked_selection();
    selection.installation = InstallationSelection::HumanReview;
    let (f, slot, lifetime, _) = selected_fixture(0, selection, false);
    bind_selected(&slot, &lifetime, selection).unwrap();
    f.check().await;
    let action = f.install().await.unwrap().unwrap();
    assert_eq!(serde_json::from_str::<Value>(&action.result.text_content()).unwrap(),
        json!({"state":"requires_action","action":"human_installation_review","installed":false}));
    assert_eq!(f.count(), 0);
    assert!(f.approvals.shared.service.list(Default::default()).unwrap().is_empty());
    assert!(f.approvals.shared.state().unwrap().attempt.is_none());

    let (mut f, slot, lifetime, _) = selected_fixture(0, selection, true);
    bind_selected(&slot, &lifetime, selection).unwrap();
    f.check().await;
    let run = f.install();
    let prompt = tokio::time::timeout(Duration::from_secs(2), f.presenter.as_mut().unwrap().next()).await;
    let observed = match prompt {
        Ok(Some(mut prompt)) => {
            let id = prompt.record.approval_id.clone();
            let pending = prompt.record.status == ApprovalStatus::Pending;
            let decision = prompt.decide(ApprovalDecision::Approve);
            if decision.is_err() { run.abort(); }
            Some((id, pending, decision))
        }
        _ => { run.abort(); None }
    };
    let completed = run.await;
    let (id, pending, decision) = observed.expect("same presenter receives native review");
    assert!(pending);
    decision.unwrap();
    completed.unwrap().unwrap();
    let record = f.approvals.shared.service.get(&id).unwrap();
    assert_eq!(record.decision.as_ref().unwrap().actor, ApprovalPrincipalId::new("connect-local-terminal").unwrap());
    assert_eq!(f.count(), 1);
}


#[tokio::test]
async fn task_parent_selection_without_admin_rule_never_arms_or_falls_back() {
    use crate::application_acquisition::selected_policy::AccessSelection;
    use meerkat_toolkit::host_lifetime::control::HostLifetimeError;
    let mut selection = checked_selection();
    selection.access = AccessSelection::ProvisionTaskParent;
    let (mut f, slot, lifetime, _) = selected_fixture(0, selection, true);
    assert!(matches!(slot.service(), Err(HostLifetimeError::Unavailable)));
    assert!(bind_selected(&slot, &lifetime, selection).is_err());
    // Do not reinterpret the request as its otherwise available configured or
    // human branch after the independent administrative selection was refused.
    for access in [AccessSelection::ConfiguredParent, AccessSelection::HumanReview] {
        let mut alternate = selection;
        alternate.access = access;
        assert!(matches!(bind_selected(&slot, &lifetime, alternate), Err(HostLifetimeError::WrongOwner)));
    }
    f.check().await;
    let result = f.install().await.unwrap().unwrap_err();
    assert_eq!(std::mem::discriminant(&result), std::mem::discriminant(&ToolError::unavailable(
        f.install_name(), meerkat_core::ToolUnavailableReason::TemporarilyUnavailable)));
    assert!(matches!(slot.service(), Err(HostLifetimeError::Unavailable)));
    assert!(f.approvals.shared.service.list(Default::default()).unwrap().is_empty());
    assert!(f.approvals.shared.state().unwrap().attempt.is_none());
    assert!(f.presenter.as_mut().unwrap().prompts.try_recv().is_err());
    assert_eq!(f.count(), 0);
}

#[tokio::test]
async fn task_parent_fields_without_actual_producer_never_authorize_or_bind() {
    use crate::application_acquisition::selected_policy::AccessSelection;
    use meerkat_toolkit::host_lifetime::control::HostLifetimeError;
    let mut selection = checked_selection();
    selection.access = AccessSelection::ProvisionTaskParent;
    let (mut f, slot, lifetime, _) = selected_fixture_with_administration(0, selection, true, true);
    assert!(matches!(bind_selected(&slot, &lifetime, selection), Err(HostLifetimeError::Unavailable)));
    // Even an exact selected administrative label and rule cannot substitute
    // for the actual protected parent's independently retained auth owner.
    assert!(matches!(slot.service(), Err(HostLifetimeError::Unavailable)));
    assert!(matches!(slot.service_parent(), Err(HostLifetimeError::Unavailable)));
    f.check().await;
    assert!(f.install().await.unwrap().is_err());
    assert!(f.approvals.shared.service.list(Default::default()).unwrap().is_empty());
    assert!(f.approvals.shared.state().unwrap().attempt.is_none());
    assert!(f.presenter.as_mut().unwrap().prompts.try_recv().is_err());
    assert_eq!(f.count(), 0);
}

#[tokio::test]
async fn selected_policy_drift_and_entered_cancellation_never_fall_back_or_rearm() {
    let selection = checked_selection();
    let (mut f, slot, lifetime, source) = selected_fixture(0, selection, true);
    bind_selected(&slot, &lifetime, selection).unwrap();
    f.check().await;
    std::fs::write(source, b"changed selected source").unwrap();
    assert!(f.install().await.unwrap().is_err());
    assert!(f.presenter.as_mut().unwrap().prompts.try_recv().is_err());
    assert!(f.approvals.shared.service.list(Default::default()).unwrap().is_empty());
    assert_eq!(f.count(), 0);

    let (mut f, slot, lifetime, _) = selected_fixture(2, selection, true);
    bind_selected(&slot, &lifetime, selection).unwrap();
    f.check().await;
    let entered = f.package.entered.notified();
    let run = f.install();
    tokio::pin!(entered);
    let observed = tokio::time::timeout(Duration::from_secs(2), entered.as_mut()).await;
    run.abort();
    let cancelled = run.await;
    assert!(observed.is_ok());
    assert!(cancelled.unwrap_err().is_cancelled());
    assert!(f.approvals.has_uncertain_dispatch());
    assert!(f.install().await.unwrap().is_err());
    assert!(f.presenter.as_mut().unwrap().prompts.try_recv().is_err());
    assert!(f.approvals.shared.state().unwrap().attempt.as_ref().unwrap().policy_entry.is_none());
    assert_eq!(f.count(), 1);
}

#[tokio::test]
async fn selected_human_recovery_keeps_exact_participant_and_whole_member_policy_scope() {
    use crate::application_acquisition::selected_policy::{AccessSelection, MemberToolPolicySelection, SelectedPolicyBinding};
    use toolkit_mobkit::private_originals::{ActiveMemberToolPolicyDecision, ApplicationToolPolicyRecoveryRequest,
        ApplicationToolPolicyRecoveryDecision, ServiceToolPolicyProposal, ServiceToolPolicySelection,
        ServiceToolInvocation, ServiceToolPolicyVersion, MemberPolicyLifetime};
    use meerkat_core::{CompiledApplicationToolPolicy, CompiledMemberToolGrants, CompiledMemberToolGrant,
        CompiledMemberToolAction, CompiledToolConsequence, CompiledPolicySourceProvenance,
        PolicyId, PolicyProviderId, PolicyRevision, PolicyDigest};
    use meerkat_core::approval::{ApprovalRequest, ApprovalPrincipalId, ApprovalOwnerRef,
        ApprovalResourceRef, ApprovalResourceKind, ApprovalResourceId, ApprovalProposedAction,
        ApprovalActionKind, ApprovalRisk};
    let mut selection = checked_selection();
    selection.access = AccessSelection::HumanReview;
    selection.tool_policy = Some(MemberToolPolicySelection::UntilSuperseded);
    let (fixture, binding, lifetime, _source) = selected_fixture(0, selection, true);
    let participant = binding.participant();
    assert!(Arc::ptr_eq(&participant, &binding.participant()));
    let native = lifetime.selected_policy().native_host.as_ref().unwrap();
    let member = native.members[0].member.member.clone();
    let compiled = CompiledApplicationToolPolicy::new(PolicyProviderId::new("fixture-policy-owner").unwrap(),
        PolicyId::new("fixture-policy").unwrap(), PolicyRevision(2),
        CompiledPolicySourceProvenance { source_id: "fixture".into(), source_digest: PolicyDigest::from_canonical_bytes(b"fixture") },
        native.members.iter().map(|row| CompiledMemberToolGrants { member_identity: row.member.member.clone(), grants: vec![
            CompiledMemberToolGrant { tool_name: "exact_tool".into(), action: CompiledMemberToolAction::Invoke,
                consequence: CompiledToolConsequence::R0 }] }).collect()).unwrap();
    let canonical = compiled.canonical_json().unwrap();
    let proposal = ServiceToolPolicyProposal { provider_id: compiled.provider_id.clone(), policy_id: compiled.policy_id.clone(), member,
        selection: ServiceToolPolicySelection { tools: vec![ServiceToolInvocation { name: "exact_tool".into(), consequence: CompiledToolConsequence::R0 }], catalog: None },
        catalog_session: None, prior: ServiceToolPolicyVersion { revision: PolicyRevision(1), digest: PolicyDigest::from_canonical_bytes(b"prior") },
        candidate: ServiceToolPolicyVersion { revision: compiled.revision, digest: PolicyDigest::from_canonical_bytes(&canonical) },
        candidate_canonical_json: String::from_utf8(canonical.clone()).unwrap(), lifetime: MemberPolicyLifetime::UntilSuperseded };
    // This directly exercises the selected participant, not canonical-slot
    // authentication. Production supplies the real active decision and record.
    let decision = ActiveMemberToolPolicyDecision { approval_id: "fixture-observation".into(), owner: native.grant_administrator.clone(),
        realm: native.realm.as_str().into(), deployment: lifetime.selected_policy().deployment.to_string(),
        original: toolkit_mobkit::private_originals::PrivateOriginalRef {
            route: toolkit_package::Id::new("fixture-route").unwrap(),
            original: meerkat_authorization_contracts::evidence::EvidenceId::new("fixture-original").unwrap(),
            event_sha256: "a".repeat(64) },
        acquisition: serde_json::from_value(json!({ "realm": native.realm.as_str(),
            "owner": "fixture-owner", "id": "fixture-acquisition" })).unwrap(),
        proposal, human_review: None };
    assert!(matches!(participant.reserve_tool_policy_recovery(&ApplicationToolPolicyRecoveryRequest { decision: &decision, canonical_json: &canonical }).unwrap(),
        ApplicationToolPolicyRecoveryDecision::Permit(_)));
    let mut wrong = decision.clone(); wrong.realm = "another-realm".into();
    assert!(participant.reserve_tool_policy_recovery(&ApplicationToolPolicyRecoveryRequest { decision: &wrong, canonical_json: &canonical }).is_err());
    wrong = decision.clone(); wrong.proposal.member = "unselected-member".into();
    assert!(participant.reserve_tool_policy_recovery(&ApplicationToolPolicyRecoveryRequest { decision: &wrong, canonical_json: &canonical }).is_err());
    let mut changed = canonical.clone(); changed.push(b' ');
    assert!(participant.reserve_tool_policy_recovery(&ApplicationToolPolicyRecoveryRequest { decision: &decision, canonical_json: &changed }).is_err());
    bind_selected(&binding, &lifetime, selection).unwrap();
    assert!(binding.service().unwrap().is_none(), "recovery never enables automatic HumanReview access");
    assert!(Arc::ptr_eq(&participant, &binding.participant()), "final binding retains the exact recovery participant");
    let mut legacy = selection; legacy.tool_policy = None;
    let (_fixture, legacy_binding, _lifetime, _source) = selected_fixture(0, legacy, true);
    assert!(matches!(legacy_binding.participant().reserve_tool_policy_recovery(&ApplicationToolPolicyRecoveryRequest {
        decision: &decision, canonical_json: &canonical }).unwrap(), ApplicationToolPolicyRecoveryDecision::RequiresHuman));

    // Exercise the real existing approval presenter and exact record body, not
    // native Human authentication or canonical-store acceptance. The default
    // participant has no standing selection or installed service policy.
    let default_participant = || SelectedPolicyBinding::human_review_participant(
        native.grant_administrator.clone(), lifetime.selected_policy().deployment.clone(), native.realm.clone(),
        native.members.iter().map(|row| row.member.member.clone()).collect());
    assert!(matches!(default_participant().reserve_tool_policy_recovery(&ApplicationToolPolicyRecoveryRequest {
        decision: &decision, canonical_json: &canonical }).unwrap(), ApplicationToolPolicyRecoveryDecision::RequiresHuman));
    let request = ApprovalRequest {
        requester: ApprovalPrincipalId::new("fixture-presenter").unwrap(),
        owner: ApprovalOwnerRef::Session { session_id: fixture.session.clone() },
        resource: ApprovalResourceRef { kind: ApprovalResourceKind::Runtime,
            id: ApprovalResourceId::new("fixture-member-policy") },
        proposed_action: ApprovalProposedAction { kind: ApprovalActionKind::Other,
            summary: "Approve the complete member policy until superseded".into(),
            body: Some(json!({"access_request":{"member_tool_policy":{"proposal":&decision.proposal}}})) },
        risk: ApprovalRisk::High, request_body: None,
        allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny].into_iter().collect(),
        expires_at: Some((std::time::SystemTime::now() + Duration::from_secs(30)).into()),
        metadata: Default::default(), request_provenance: None,
    };
    let approval_root = tempfile::tempdir().unwrap();
    let (approvals, mut presenter) = LocalApprovals::create(approval_root.path()).unwrap();
    approvals.bind_session(&fixture.session).unwrap();
    let (record, answer) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(approvals.review_access(request), async {
            let mut prompt = presenter.next().await.ok_or(ConnectError::ReviewCancelled)?;
            prompt.decide(ApprovalDecision::Approve)
        })
    }).await.unwrap();
    answer.unwrap();
    let mut human = decision.clone();
    human.approval_id = human.proposal.sha256().unwrap();
    human.human_review = Some(record.unwrap());
    // A fresh same-scope participant is how ordinary Restore rechecks the
    // canonical record. This does not execute native Restore or a store replay.
    for participant in [default_participant(), default_participant(), legacy_binding.participant()] {
        assert!(matches!(participant.reserve_tool_policy_recovery(&ApplicationToolPolicyRecoveryRequest {
            decision: &human, canonical_json: &canonical }).unwrap(), ApplicationToolPolicyRecoveryDecision::Permit(_)));
        let mut service_only = human.clone();
        service_only.human_review.as_mut().unwrap().proposed_action.body = Some(json!({"access_request":{"service_instruction":{}}}));
        assert!(matches!(participant.reserve_tool_policy_recovery(&ApplicationToolPolicyRecoveryRequest {
            decision: &service_only, canonical_json: &canonical }).unwrap(), ApplicationToolPolicyRecoveryDecision::RequiresHuman));
        let mut changed = human.clone();
        changed.human_review.as_mut().unwrap().proposed_action.body = Some(json!({"access_request":{"member_tool_policy":{"proposal":{}}}}));
        assert!(matches!(participant.reserve_tool_policy_recovery(&ApplicationToolPolicyRecoveryRequest {
            decision: &changed, canonical_json: &canonical }).unwrap(), ApplicationToolPolicyRecoveryDecision::RequiresHuman));
    }
}

}

#[path = "service_install_tests/review_observation.rs"]
mod review_observation;

#[path = "service_install_tests/warm_reuse.rs"]
mod warm_reuse;

#[path = "service_install_tests/public_selection_feedback.rs"]
mod public_selection_feedback;
