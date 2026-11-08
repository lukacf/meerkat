//! Public collector call on the real gateway factory/machine, before this
//! identity-free fixture's independently expected Ready refusal. No agent,
//! runnable, external account, substitute grant owner or audit sink is created.
use super::*;
use crate::installed_host::operator_setup::CollectorSetupHost;
use crate::private_originals::collector_setup::{CollectorSetupError, CollectorSource};
use axum::{
    Json,
    routing::{get, post},
};
use futures::FutureExt;
use meerkat::{ConnectorOAuthCallback, ConnectorOAuthTarget, HostAuthService};
use meerkat_authorization_contracts::{
    audit::{AuditObservation, AuditTarget},
    constraints::{AudienceRef, LifetimeRestriction},
    evidence::EvidenceId,
};
use meerkat_core::{
    AdministrationEntry, AdministrationEntryRefusal, EnteredAdministration, ToolCallView,
    ToolDeadlineChain, ToolDeadlineContributor, ToolDeadlineOwner, ToolExecutionResolutionContext,
};
use meerkat_mcp::{HttpEgressProfile, McpRouterShutdownObservation, McpStdioLaunchProfile};
use meerkat_mobkit::rpc_gateway::GatewayRealmMcpInstallation;
use meerkat_runtime::{
    RuntimeStore, ServiceCallAuditKey, ServiceCallAuditRecord,
    input_authority::{ServiceCallRequest, ServiceOccurrence},
    store::SqliteRuntimeStore,
};
use serde_json::json;
use std::{collections::BTreeMap, panic::AssertUnwindSafe, path::PathBuf};

const TEST: &str = "installed_host::tests::gateway_identity::collector::public_occurrence_reads_and_retains_native_audit_before_joined_close";
const ACCESS: &str = "collector-loopback-access";
const SUBJECT: &str = "collector-loopback-subject";
const REDIRECT: &str = "http://127.0.0.1:41001/collector/callback";

struct Captured {
    collector: Arc<CollectorSetupHost>,
    auth: HostAuthService,
    installation: GatewayRealmMcpInstallation,
}
pub(super) struct Fixture {
    directory: PathBuf,
    issuer: String,
    captured: Mutex<Option<Captured>>,
    stop: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    server: Mutex<Option<tokio::task::JoinHandle<std::io::Result<()>>>>,
    failed: Mutex<Option<Box<dyn std::any::Any + Send>>>,
    retained: Mutex<Option<(ServiceCallAuditKey, Vec<ServiceCallAuditRecord>)>>,
}
impl Fixture {
    pub(super) async fn new(directory: &Path) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let issuer = format!("http://{}", listener.local_addr().unwrap());
        let metadata = json!({"issuer": issuer, "authorization_endpoint": format!("{issuer}/authorize"),
            "token_endpoint": format!("{issuer}/token"), "code_challenge_methods_supported": ["S256"]});
        let openid = json!({"issuer": issuer, "userinfo_endpoint": format!("{issuer}/userinfo")});
        let app = axum::Router::new()
            .route("/.well-known/oauth-authorization-server", get(move || {
                let value = metadata.clone(); async move { Json(value) }
            }))
            .route("/.well-known/openid-configuration", get(move || {
                let value = openid.clone(); async move { Json(value) }
            }))
            .route("/token", post(|axum::Form(form): axum::Form<BTreeMap<String, String>>| async move {
                assert_eq!(form.get("code").map(String::as_str), Some("collector-loopback-code"));
                assert_eq!(form.get("client_id").map(String::as_str), Some("client"));
                assert_eq!(form.get("redirect_uri").map(String::as_str), Some(REDIRECT));
                assert!(form.get("code_verifier").is_some_and(|value| !value.is_empty()));
                Json(json!({"access_token": ACCESS, "token_type": "Bearer", "expires_in": 3600,
                    "scope": "openid gmail.readonly"}))
            }))
            .route("/userinfo", get(|headers: axum::http::HeaderMap| async move {
                assert_eq!(headers.get("authorization").unwrap().to_str().unwrap(),
                    format!("Bearer {ACCESS}"));
                Json(json!({"sub": SUBJECT}))
            }));
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
        });
        Self {
            directory: directory.to_path_buf(),
            issuer,
            captured: Mutex::new(None),
            stop: Mutex::new(Some(stop)),
            server: Mutex::new(Some(server)),
            failed: Mutex::new(None),
            retained: Mutex::new(None),
        }
    }
    pub(super) fn capture(
        &self,
        host: &ObservedHost,
        offer: &GatewayFactoryConstruction<'_>,
    ) -> bool {
        let checked = std::panic::catch_unwind(AssertUnwindSafe(|| {
            let setup = host.inner.local_operator_setup().unwrap();
            let value = Captured {
                collector: Arc::new(setup.collector_setup_host(offer).unwrap()),
                auth: offer.host_auth_service().unwrap(),
                installation: offer.realm_mcp_installation().unwrap(),
            };
            assert!(
                self.captured.lock().unwrap().replace(value).is_none(),
                "one actual persistent factory"
            );
        }));
        if let Err(panic) = checked {
            *self.failed.lock().unwrap() = Some(panic);
            return false;
        }
        true
    }
    fn store(&self) -> SqliteRuntimeStore {
        let layout = meerkat_mobkit::storage_layout::MetaStorageLayout::standalone(
            self.directory.join("state"),
            self.directory.join("gateway-home"),
        );
        // Match the gateway's head-canonical profile on its actual database;
        // no new machine, mirror table or synthetic audit append.
        SqliteRuntimeStore::new_head_canonical(layout.runtime_db()).unwrap()
    }
    pub(super) async fn exercise(&self, host: &ObservedHost) -> bool {
        let mut opened = None;
        let checked = AssertUnwindSafe(async {
            let captured = self
                .captured
                .lock()
                .unwrap()
                .take()
                .expect("real factory offer preceded runtime");
            let mut config =
                crate::host_gateway::collector_test_configuration(&self.directory, "ready");
            let binding = config.connector.as_mut().unwrap();
            binding.realm_id = captured.installation.realm().to_string();
            binding.issuer = self.issuer.clone();
            binding.strategy_id = "oidc-userinfo-v1".into();
            binding.account = SUBJECT.into();
            let target = ConnectorOAuthTarget {
                slot: meerkat_core::CredentialAccountRef {
                    realm: captured.installation.realm().clone(),
                    account: meerkat_core::CredentialAccountId::parse(&binding.slot_id).unwrap(),
                },
                issuer: binding.issuer.clone(),
                client: binding.client.clone(),
                resource: binding.resource.clone(),
                scopes: ["openid".into(), "gmail.readonly".into()].into(),
                strategy_id: binding.strategy_id.clone(),
                account: SUBJECT.to_owned().into(),
                client_auth_profile: None,
            };
            // Real native auth admission, exchange, OIDC UserInfo verification
            // and persistence, using the factory's own account and flow owner.
            eprintln!("collector_fixture auth.start.begin");
            let started = captured
                .auth
                .connector_login_start(&target, REDIRECT)
                .await
                .unwrap();
            eprintln!("collector_fixture auth.start.end");
            eprintln!("collector_fixture auth.complete.begin");
            captured
                .auth
                .connector_login_complete(
                    &target,
                    ConnectorOAuthCallback {
                        redirect_uri: REDIRECT.into(),
                        state: started.state,
                        code: "collector-loopback-code".into(),
                        host_attempt_handle: None,
                    },
                )
                .await
                .unwrap();
            eprintln!("collector_fixture auth.complete.end");
            eprintln!("collector_fixture auth.status.begin");
            let status = captured.auth.connector_status(&target.slot).await.unwrap();
            eprintln!("collector_fixture auth.status.end");
            assert_eq!(status.phase, meerkat::ConnectorAuthPhase::Authorized);
            assert_eq!(status.verified_account.as_ref().unwrap().subject, SUBJECT);
            eprintln!("collector_fixture installation.write.begin");
            let (_, generation) = captured.installation.servers().await.unwrap();
            captured
                .installation
                .install(
                    config.clone(),
                    generation,
                    Arc::new(ConfigurationEntry(host.owner.clone())),
                )
                .await
                .unwrap();
            eprintln!("collector_fixture installation.write.end");
            eprintln!("collector_fixture installation.select.begin");
            let installation = captured
                .collector
                .select_installation("gmail", ["get_profile".into()].into())
                .await
                .unwrap();
            eprintln!("collector_fixture installation.select.end");
            assert_eq!(installation.account().subject, SUBJECT);
            let setup = host.inner.local_operator_setup().unwrap();
            let principal = setup
                .bind_collector_principal(
                    super::super::principal("collector"),
                    AudienceRef::Principal {
                        principal: super::super::principal("operator"),
                    },
                )
                .unwrap();
            let review = setup
                .prepare_collector_enrollment(
                    &principal,
                    installation,
                    CollectorSource {
                        mob_id: id("factory"),
                        source_id: id("inbox"),
                        generation: 1,
                    },
                    LifetimeRestriction::unrestricted(),
                    review_request(),
                )
                .unwrap();
            // Existing ApprovalService is the real record owner. This controlled
            // presenter does not claim a human/browser review acceptance.
            let approvals = meerkat_core::ApprovalService::new();
            let pending = approvals.request(review.request().clone()).unwrap();
            let approval = approvals
                .decide(
                    &pending.approval_id,
                    meerkat_core::ApprovalDecision::Approve,
                    meerkat_core::ApprovalPrincipalId::new("fixture-protected-presenter").unwrap(),
                    None,
                    None,
                )
                .unwrap();
            eprintln!("collector_fixture enrollment.begin");
            let setup_id = setup
                .finalize_collector_enrollment(
                    &host.originals,
                    &captured.collector,
                    &review,
                    approval,
                    &|| true,
                )
                .await
                .unwrap();
            let selected = host
                .originals
                .resolve_collector_setup(&setup_id, &captured.collector)
                .await
                .unwrap();
            eprintln!("collector_fixture enrollment.end");
            let occurrence = || {
                ServiceOccurrence::new(
                    id("collector-schedule"),
                    id("collector-occurrence"),
                    id("collector-delivery"),
                )
            };
            let expected = ServiceCallRequest::from_trusted_host(
                selected.association().unwrap(),
                occurrence(),
            )
            .unwrap();
            let expected_key = expected.audit_key().clone();
            eprintln!("collector_fixture audit.initial.begin");
            assert!(
                self.store()
                    .read_service_call_audit(&expected_key)
                    .await
                    .unwrap()
                    .is_empty()
            );
            eprintln!("collector_fixture audit.initial.end");
            eprintln!("collector_fixture occurrence.open.begin");
            opened = Some(
                host.inner
                    .open_collector_occurrence(
                        captured.collector.clone(),
                        setup_id.clone(),
                        occurrence(),
                        McpStdioLaunchProfile::trusted_host(),
                        HttpEgressProfile::trusted_host(),
                        Duration::from_secs(15),
                    )
                    .await
                    .unwrap(),
            );
            eprintln!("collector_fixture occurrence.open.end");
            let public = opened.as_mut().unwrap();
            assert_eq!(public.audit_key(), &expected_key);
            assert_eq!(public.audit_key().occurrence(), "collector-occurrence");
            let catalog = public.tool_catalog();
            assert_eq!(catalog.len(), 1);
            let args = serde_json::value::RawValue::from_string("{}".into()).unwrap();
            eprintln!("collector_fixture occurrence.read.begin");
            let outcome = public
                .read(
                    ToolCallView {
                        id: "collector-read",
                        name: "get_profile",
                        args: &args,
                    },
                    resolution(),
                )
                .await
                .unwrap();
            eprintln!("collector_fixture occurrence.read.end");
            assert!(
                outcome.audit.is_ok(),
                "public API must preserve native durable audit success"
            );
            let result = outcome.result.expect("real native governed MCP result");
            assert!(!result.result.is_error);
            assert_eq!(result.result.tool_use_id, "collector-read");
            assert!(result.result.settlement_failures.is_empty());
            assert_eq!(result.result.text_content(), "controlled native reply");
            assert!(result.async_ops.is_empty() && result.session_effects.is_empty());
            assert_eq!(
                fs::read_to_string(self.directory.join("calls")).unwrap(),
                "get_profile\n"
            );
            eprintln!("collector_fixture audit.read.begin");
            let rows = self
                .store()
                .read_service_call_audit(&expected_key)
                .await
                .unwrap();
            eprintln!("collector_fixture audit.read.end");
            assert_audit(&rows);
            let foreign =
                ServiceCallAuditKey::new(expected_key.original_digest(), "another-occurrence");
            assert!(
                self.store()
                    .read_service_call_audit(&foreign)
                    .await
                    .unwrap()
                    .is_empty()
            );
            // A real canonical revocation between reads exercises the public
            // pre-entry recheck. It adds no tool call and no false native outcome.
            eprintln!("collector_fixture enrollment.revoke.begin");
            host.originals
                .revoke_collector_setup(&setup_id)
                .await
                .unwrap();
            eprintln!("collector_fixture enrollment.revoke.end");
            eprintln!("collector_fixture occurrence.revoked_read.begin");
            assert!(matches!(
                public
                    .read(
                        ToolCallView {
                            id: "after-revoke",
                            name: "get_profile",
                            args: &args
                        },
                        resolution()
                    )
                    .await,
                Err(crate::host_gateway::CollectorReadError::Setup(
                    CollectorSetupError::SetupRevoked
                ))
            ));
            eprintln!("collector_fixture occurrence.revoked_read.end");
            assert_eq!(
                fs::read_to_string(self.directory.join("calls")).unwrap(),
                "get_profile\n"
            );
            assert!(
                self.store()
                    .read_service_call_audit(&expected_key)
                    .await
                    .unwrap()
                    == rows
            );
            *self.retained.lock().unwrap() = Some((expected_key, rows));
            eprintln!("collector_fixture auth.logout.begin");
            captured.auth.connector_logout(&target.slot).await.unwrap();
            eprintln!("collector_fixture auth.logout.end");
        })
        .catch_unwind()
        .await;
        // Join the actual occurrence on success and every assertion failure.
        // Unexpected errors cannot be swallowed by the later Ready refusal.
        eprintln!("collector_fixture occurrence.close.begin");
        let close_ok = if let Some(opened) = opened {
            matches!(opened.close().await, McpRouterShutdownObservation::Reported(report)
                if report.is_settled() && report.guards_completed() && report.servers().len() == 1)
        } else {
            false
        };
        eprintln!("collector_fixture occurrence.close.end");
        if let Err(panic) = checked {
            *self.failed.lock().unwrap() = Some(panic);
            return false;
        }
        if !close_ok {
            *self.failed.lock().unwrap() = Some(Box::new("occurrence close did not settle"));
            return false;
        }
        true
    }
    pub(super) async fn finish(&self) {
        if let Some(stop) = self.stop.lock().unwrap().take() {
            let _ = stop.send(());
        }
        let server = self.server.lock().unwrap().take();
        eprintln!("collector_fixture http.join.begin");
        if let Some(server) = server {
            server.await.unwrap().unwrap();
        }
        eprintln!("collector_fixture http.join.end");
        if let Some(panic) = self.failed.lock().unwrap().take() {
            std::panic::resume_unwind(panic);
        }
        let retained = self
            .retained
            .lock()
            .unwrap()
            .take()
            .expect("public read completed independently");
        assert!(
            self.store()
                .read_service_call_audit(&retained.0)
                .await
                .unwrap()
                == retained.1,
            "gateway close preserves the exact native audit rows"
        );
    }
}
struct ConfigurationEntry(Arc<NativeHostLifetime>);
impl AdministrationEntry for ConfigurationEntry {
    fn enter(&self) -> Result<EnteredAdministration, AdministrationEntryRefusal> {
        self.0
            .validate_current_selection()
            .map_err(|_| AdministrationEntryRefusal::NotAdministrator)?;
        Ok(EnteredAdministration::admitted())
    }
}
fn id(value: &str) -> EvidenceId {
    EvidenceId::new(value).unwrap()
}
fn resolution() -> ToolExecutionResolutionContext {
    ToolExecutionResolutionContext::new(
        ToolDeadlineChain::new(vec![ToolDeadlineContributor::finite(
            ToolDeadlineOwner::DirectCaller,
            Duration::from_secs(15),
        )])
        .unwrap(),
    )
}
fn assert_audit(rows: &[ServiceCallAuditRecord]) {
    use meerkat_core::authorization::{OperationEntryBoundary, OperationObservedOutcome};
    assert!(!rows.is_empty());
    let operation = &rows[0].observation.observation.operation_id;
    let mut prepared = 0;
    let mut entry = 0;
    let mut outcome = 0;
    for (index, row) in rows.iter().enumerate() {
        assert_eq!(row.sequence, index as u64);
        assert_eq!(&row.observation.observation.operation_id, operation);
        assert!(row.observation.observation.run_id.is_none());
        assert_eq!(
            row.observation.observation.execution_scope,
            meerkat_core::exact_operation::OperationExecutionScope::Domain
        );
        assert!(
            row.observation.contributors.is_empty(),
            "service audit does not invent agent inputs"
        );
        match &row.observation.observation.observation {
            AuditObservation::Prepared { target, policy } => {
                assert_eq!(policy.operation_authorities.len(), 1);
                assert!(policy.controller_lineages.is_empty());
                assert_eq!(index, 0);
                assert!(
                    matches!(target.as_ref(), AuditTarget::Tool { call_id, tool_name, .. }
                    if call_id == "collector-read" && tool_name == "get_profile")
                );
                prepared += 1;
            }
            AuditObservation::Entry {
                boundary: Some(OperationEntryBoundary::ToolEffect),
            } => entry += 1,
            AuditObservation::Entry { .. } => {}
            AuditObservation::Outcome {
                outcome:
                    OperationObservedOutcome::ToolDispatchReturned {
                        asynchronous_operations,
                        result_is_error: false,
                        terminal_error: None,
                    },
            } => {
                assert!(asynchronous_operations.is_empty());
                assert_eq!(index + 1, rows.len());
                outcome += 1;
            }
            _ => panic!("unexpected native service audit observation"),
        }
    }
    assert_eq!((prepared, entry, outcome), (1, 1, 1));
}
fn review_request() -> meerkat_core::ApprovalRequest {
    use meerkat_core::*;
    ApprovalRequest {
        requester: ApprovalPrincipalId::new("collector-test").unwrap(),
        owner: ApprovalOwnerRef::Session {
            session_id: SessionId::new(),
        },
        resource: ApprovalResourceRef {
            kind: ApprovalResourceKind::Runtime,
            id: ApprovalResourceId::new("collector"),
        },
        proposed_action: ApprovalProposedAction {
            kind: ApprovalActionKind::Other,
            summary: "collector".into(),
            body: None,
        },
        risk: ApprovalRisk::High,
        request_body: None,
        allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny].into(),
        expires_at: None,
        metadata: Default::default(),
        request_provenance: None,
    }
}
#[test]
fn public_occurrence_reads_and_retains_native_audit_before_joined_close() {
    if let Some(path) = std::env::var_os(CHILD_REPORT) {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
            runtime.block_on(run_child(Path::new(&path), true));
        }));
        // Keep the existing stdin shutdown path on failure too: runtime Drop
        // can wait for EOF and mask the original assertion behind the backstop.
        runtime.shutdown_background();
        if let Err(panic) = outcome {
            std::panic::resume_unwind(panic);
        }
        std::process::exit(0);
    }
    let (report, messages) = child_report(TEST);
    assert!(
        report.trace.collector_completed,
        "public call/audit/close must succeed independently"
    );
    assert!(report.trace.prepared && report.trace.runtime_bound && report.trace.close_joined);
    assert_eq!((report.sessions, report.inputs), (Some(0), Some(0)));
    assert_eq!(
        (report.trace.identity_calls, report.trace.ready_calls),
        (0, 1)
    );
    assert!(!report.trace.identity_at_ready && report.trace.selection_current_at_ready);
    assert!(report.trace.ready_refused_unavailable && report.returned_host_unavailable);
    assert!(
        !messages
            .iter()
            .any(|m| m["id"] == "init" && m["result"]["http_base_url"].is_string())
    );
    assert!(
        !messages
            .iter()
            .any(|m| m["method"] == "meta/init_settled" && m["params"]["outcome"] == "ready")
    );
}
