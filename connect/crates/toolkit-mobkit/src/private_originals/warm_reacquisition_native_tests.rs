//! Actual configured A/helper/B/helper/C composition in one live native host.
//! Model output and package readback are mechanical; approval, contributor
//! capture, source binding, attachment readiness and operation admission are
//! the production owners. Identical X readback does not qualify CheckedInstalls
//! retained-job reuse, installation reuse, restart or Console authentication.
use super::*;
use crate::installed_host::acquisition::{AcquisitionBootstrap, InstalledServiceTaskSource};
use crate::installed_host::tool_policy::{ServiceToolInvocation, ServiceToolPolicySelection};
use meerkat_authorization_contracts::work_association::GrantTarget;
use meerkat_core::turn_execution_authority::TurnTerminalOutcome;
use meerkat_core::{ApprovalDecision, ApprovalPrincipalId, ApprovalService, McpServerConfig};
use meerkat_runtime::completion::CompletionOutcome;
use meerkat_runtime::input_state::{InputLifecycleState, InputTerminalOutcome, StoredInputState};
use meerkat_runtime::terminal_status::{
    InputTerminalReceipt, InputTerminalReceiptRead, InteractionSelector,
};
use meerkat_runtime::{LogicalRuntimeId, RuntimeStore};
use std::collections::BTreeSet;
use std::path::PathBuf;
use tokio::sync::{Notify, Semaphore};

const X: &str = "warm_x";
const Y: &str = "warm_y";
const X_READ: &str = "warm_x_read";
const Y_READ: &str = "warm_y_read";
const FINAL: &str = "Fresh task grants and independent targets checked";

struct Endpoint {
    directory: tempfile::TempDir,
    listed: PathBuf,
    released: PathBuf,
    called: PathBuf,
}
impl Endpoint {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        Self {
            listed: directory.path().join("listed"),
            released: directory.path().join("released"),
            called: directory.path().join("called"),
            directory,
        }
    }
    fn config(&self, name: &str) -> McpServerConfig {
        // Positional arguments keep all owned paths out of shell program text.
        let program = r#"while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"warm-fixture","version":"0"}}}\n' "$id" ;;
    *'"method":"tools/list"'*)
      printf 'list\n' >> "$1"
      while [ ! -e "$2" ]; do sleep 0.02; done
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"read","inputSchema":%s}]}}\n' "$id" "$4" ;;
    *'"method":"tools/call"'*)
      printf '%s\n' "$line" >> "$3"
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"actual warm fixture read"}],"isError":false}}\n' "$id" ;;
  esac
done"#;
        let mut configuration = McpServerConfig::stdio(name, "/bin/sh", vec![
            "-c".into(), program.into(), "warm-fixture".into(),
            self.listed.to_str().unwrap().into(), self.released.to_str().unwrap().into(),
            self.called.to_str().unwrap().into(),
            serde_json::json!({"type":"object","description":"A harmless read with an intentionally substantial exact schema. Native deferred catalog discovery must load this operation before the Original can select it. The schema is fixture data, never an authorization source."}).to_string(),
        ], Default::default());
        configuration.access = Some(meerkat_core::McpServerAccess::allowing(["read"]));
        configuration
            .tool_names
            .insert("read".into(), format!("{name}_read"));
        configuration
    }
    fn release(&self) {
        std::fs::write(&self.released, b"release\n").unwrap();
    }
    fn calls(&self) -> usize {
        match std::fs::read_to_string(&self.called) {
            Ok(rows) => rows
                .lines()
                .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
                .count(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => panic!("owned endpoint observation failed: {error}"),
        }
    }
}
impl Drop for Endpoint {
    fn drop(&mut self) {
        let _keep_directory_until_release = &self.directory;
        let _ = std::fs::write(&self.released, b"release\n");
    }
}

struct PackageReadback {
    configurations: Vec<McpServerConfig>,
}
#[async_trait]
impl ServiceInstallationReadback for PackageReadback {
    async fn readback(
        &self,
        _: &meerkat_core::ApprovalRecord,
    ) -> Result<McpServerConfig, PrivateOriginalError> {
        Err(PrivateOriginalError::Denied)
    }
    async fn readback_source(
        &self,
        source: &ServiceInstallationReadbackSource,
    ) -> Result<McpServerConfig, PrivateOriginalError> {
        let ServiceInstallationReadbackSource::ReviewedBatchMember(member) = source else {
            return Err(PrivateOriginalError::Denied);
        };
        self.configurations
            .iter()
            .find(|configuration| {
                member.document()["fixture_configuration_sha256"]
                    == digest(&canonical_json(configuration).unwrap())
            })
            .cloned()
            .ok_or(PrivateOriginalError::Denied)
    }
}

fn request(
    fixture: &Fixture,
    configuration: Option<&McpServerConfig>,
) -> meerkat_core::ApprovalRequest {
    use meerkat_core::{
        ApprovalActionKind, ApprovalOwnerRef, ApprovalProposedAction, ApprovalResourceId,
        ApprovalResourceKind, ApprovalResourceRef, ApprovalRisk,
    };
    let body = serde_json::json!({"fixture":"actual-warm-native-composition",
        "fixture_configuration_sha256":configuration.map(|value| digest(&canonical_json(value).unwrap()))});
    meerkat_core::ApprovalRequest {
        requester: ApprovalPrincipalId::new("actual-warm-helper").unwrap(),
        owner: ApprovalOwnerRef::Session {
            session_id: fixture.session.clone(),
        },
        resource: ApprovalResourceRef {
            kind: ApprovalResourceKind::Runtime,
            id: ApprovalResourceId::new("actual-warm-native-composition"),
        },
        proposed_action: ApprovalProposedAction {
            kind: ApprovalActionKind::Other,
            summary: "Review this complete service set and fresh task setup scope".into(),
            body: Some(body.clone()),
        },
        risk: ApprovalRisk::Medium,
        request_body: Some(body),
        allowed_decisions: [ApprovalDecision::Approve, ApprovalDecision::Deny].into(),
        expires_at: Some((std::time::SystemTime::now() + Duration::from_secs(60)).into()),
        metadata: Default::default(),
        request_provenance: None,
    }
}

async fn prepare_followup(
    fixture: &Fixture,
    acquisition: &Arc<acquisition_denial::Script>,
    report: &Arc<Report>,
    configurations: &[McpServerConfig],
    expected_parent: &PrivateOriginalRef,
) -> (PrivateOriginalRef, SessionId) {
    let context = tokio::time::timeout(WAIT, report.take_context())
        .await
        .unwrap();
    let helper_session = context.origin_session_id().unwrap().clone();
    let owner = acquisition.installed_owner();
    let mut bootstrap = AcquisitionBootstrap::selected(owner.clone());
    assert!(Arc::ptr_eq(
        &bootstrap.prepare(None, None).unwrap().unwrap(),
        &owner
    ));
    let bootstrap = Arc::new(Mutex::new(bootstrap));
    let task = report
        .prepare_task(
            &InstalledServiceTaskSource::new(&bootstrap),
            ReportPreparation::PublicService,
        )
        .await
        .unwrap();
    assert!(task.has_configured_setup() && task.console_login().is_some());
    let entries = configurations
        .iter()
        .map(|configuration| PublicServiceBatchEntry {
            configuration: configuration.clone(),
            operations: [Id::new("read").unwrap()].into(),
            request: request(fixture, Some(configuration)),
        })
        .collect();
    let prepared = task
        .prepare_public_batch(
            entries,
            Some(60_000),
            request(fixture, None),
            ServiceToolPolicySelection {
                tools: configurations
                    .iter()
                    .map(|configuration| ServiceToolInvocation {
                        name: format!("{}_read", configuration.name),
                        consequence: meerkat_core::CompiledToolConsequence::R0,
                    })
                    .collect(),
                catalog: Some(meerkat_core::CompiledToolConsequence::R0),
            },
        )
        .await
        .unwrap();
    let body = &prepared.request().proposed_action.body.as_ref().unwrap()["access_request"];
    let fresh = body["service_instruction"]["fresh_original"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        body["configured_setup"]["processors"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        body["configured_setup"]["sessions"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let approvals = ApprovalService::new();
    let pending = approvals.request(prepared.request().clone()).unwrap();
    let approved = approvals
        .decide(
            &pending.approval_id,
            ApprovalDecision::Approve,
            ApprovalPrincipalId::new("actual-protected-warm-review").unwrap(),
            None,
            None,
        )
        .unwrap();
    assert!(prepared.approve(approved, || true).await.unwrap());
    let lookup = PrivateOriginalRef {
        route: expected_parent.route.clone(),
        original: EvidenceId::new(fresh).unwrap(),
        event_sha256: String::new(),
    };
    let stored = fixture
        .store
        .get(&fixture.source.key(&lookup).unwrap())
        .await
        .unwrap()
        .unwrap();
    let record = fixture.source.decode_stored(&stored).unwrap();
    let task = record.approved_task.as_ref().unwrap();
    assert!(&task.parent == expected_parent);
    assert_eq!(task.setup().unwrap().helper.session_id, helper_session);
    assert_eq!(task.attachments().len(), configurations.len());
    assert!(record.final_association.is_none() && record.service_admission.is_none());
    assert!(record.service_setup_children.is_empty());
    let reference = PrivateOriginalRef {
        event_sha256: digest(&canonical_json(&record.event).unwrap()),
        ..lookup
    };
    (reference, helper_session)
}

async fn source_record(fixture: &Fixture, reference: &PrivateOriginalRef) -> SourceRecord {
    let stored = fixture
        .store
        .get(&fixture.source.key(reference).unwrap())
        .await
        .unwrap()
        .unwrap();
    fixture.source.decode_record(&stored, reference).unwrap()
}

async fn sweep(fixture: &Fixture) {
    let mut cursor = None;
    loop {
        let page = fixture
            .source
            .step_service_inputs(&fixture.store, cursor.as_deref(), 128)
            .await
            .unwrap();
        assert!(
            !page.had_unavailable_records,
            "ordinary recovery sweep must keep this exact task available"
        );
        cursor = page.next_after_id;
        if cursor.is_none() {
            break;
        }
    }
}

async fn attach_and_enter(
    fixture: &Fixture,
    reference: &PrivateOriginalRef,
    new_endpoint: &Endpoint,
    script: &Arc<WarmScript>,
    mode: usize,
) -> SourceRecord {
    let approved = source_record(fixture, reference).await;
    let mut expected_contexts = Vec::new();
    for member in approved.approved_task.as_ref().unwrap().attachments() {
        let stored = fixture
            .store
            .get(&fixture.source.access_key(&member.grant).unwrap())
            .await
            .unwrap()
            .unwrap();
        let access = fixture
            .source
            .decode_access(&stored, &member.grant)
            .unwrap();
        let crate::private_originals::service_grants::AccessDecision::Mcp(decision) =
            access.decision
        else {
            panic!("approved attachment must name an exact MCP access decision");
        };
        expected_contexts.push(serde_json::json!({
            "schema": "toolkit.approved-service-context.v1",
            "operation": member.acquisition.id,
            "original": reference.original.as_str(),
            "connection": decision.permission.connection,
            "resource": decision.permission.resource,
            "allowed_operations": decision.permission.operations,
            "expires_at_ms": member.before_ms.min(decision.permission.lifetime.expires_at_ms.unwrap()),
            "configuration_sha256": decision.connection_revision,
        }));
    }
    script.arm(mode, expected_contexts);
    // The new endpoint holds actual tools/list. No B/C input can enter while
    // this selected member is pending, even when X was already usable for B.
    tokio::time::timeout(WAIT, async {
        while !new_endpoint.listed.exists() {
            sweep(fixture).await;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let pending = source_record(fixture, reference).await;
    assert!(
        pending.final_association.is_none()
            && pending.bound_service_input.is_none()
            && pending.service_admission.is_none()
    );
    assert_eq!(new_endpoint.calls(), 0);
    new_endpoint.release();
    tokio::time::timeout(WAIT, async {
        loop {
            sweep(fixture).await;
            let record = source_record(fixture, reference).await;
            if record.service_admission.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(WAIT, script.reached.notified())
        .await
        .unwrap();
    let record = source_record(fixture, reference).await;
    assert!(record.bound_service_input.is_some() && record.service_admission.is_some());
    let association = record.final_association.as_ref().unwrap();
    assert_eq!(
        association.candidate().original_work.work,
        reference.original
    );
    assert!(matches!(
        &association.candidate().authority_basis,
        WorkAuthorityBasis::TargetedGrantLineages { base: Some(_), .. }
    ));
    assert!(
        association
            .candidate()
            .authority_basis
            .lineage_for(Some(&GrantTarget::McpServer {
                server: "unselected-warm-target".into(),
            }))
            .is_none()
    );
    assert!(
        !record.service_setup_children.is_empty(),
        "fresh setup children were observed by their real source owner"
    );
    record
}

fn leaves(association: &InputAuthorityAssociation) -> Vec<GrantLineageRef> {
    association
        .candidate()
        .authority_basis
        .grant_lineages()
        .map(|lineage| lineage.last().unwrap().clone())
        .collect()
}

fn assert_reviewed_catalog_scope(
    fixture: &Fixture,
    reference: &PrivateOriginalRef,
    record: &SourceRecord,
) {
    use crate::installed_host::application_operations;
    use meerkat_authorization_contracts::constraints::{AudienceRef, OperationRestrictionValues};

    let task = record.approved_task.as_ref().unwrap();
    let setup = task.setup().unwrap();
    let association = record.final_association.as_ref().unwrap();
    let lineage = association
        .candidate()
        .authority_basis
        .lineage_for(None)
        .unwrap();
    let route = fixture.source.route(&reference.route).unwrap();
    let grants = fixture.source.owner.grants();
    let resolved = grants
        .resolve_lineage(
            lineage,
            &route.logical_executor,
            route.represented_subject.as_ref(),
        )
        .unwrap();
    let contributor = grants.resolve_lineage_ceiling(lineage).unwrap();
    assert!(resolved.restrictions() == contributor.restrictions());
    resolved.check_current().unwrap();
    contributor.check_current().unwrap();
    let now = fixture.source.owner.clock().now().unwrap().unix_ms;
    // Apply the actual native operation restriction checker to the issued
    // catalog leaf. The helper consumes this same leaf as a contributor ceiling,
    // not as authority issued to another executor. No prepared permit is made.
    let check = |operation: &meerkat_authorization::policy::LocalOperationValues| {
        resolved
            .restrictions()
            .check_bounds(OperationRestrictionValues {
                action: &operation.action,
                resource_domain: &operation.resource_domain,
                processor: &operation.processor,
                audience: &operation.audience,
                now_ms: now,
            })
    };
    assert_ne!(setup.configured.session_id, setup.helper.session_id);
    assert!(route.logical_executor != setup.helper.logical_executor);
    for processor in [&route.logical_executor, &setup.helper.logical_executor] {
        for session in [&setup.configured.session_id, &setup.helper.session_id] {
            let operation = application_operations::catalog_read(
                &route.grant_caller,
                processor,
                &route.grant_audience,
                session,
            );
            assert!(
                check(&operation).is_ok(),
                "each reviewed 2x2 catalog tuple is permitted"
            );
        }
    }
    let foreign = PrincipalRef::in_domain(
        meerkat_core::PrincipalKind::ServiceAccount,
        "warm-foreign-scope",
        meerkat_core::TrustDomainId::new("warm-foreign-domain").unwrap(),
    )
    .unwrap();
    assert!(foreign != route.logical_executor && foreign != setup.helper.logical_executor);
    let foreign_processor = application_operations::catalog_read(
        &route.grant_caller,
        &foreign,
        &route.grant_audience,
        &setup.configured.session_id,
    );
    assert!(check(&foreign_processor).is_err());
    let foreign_session = SessionId::new();
    assert!(
        foreign_session != setup.configured.session_id
            && foreign_session != setup.helper.session_id
    );
    let foreign_session_operation = application_operations::catalog_read(
        &route.grant_caller,
        &route.logical_executor,
        &route.grant_audience,
        &foreign_session,
    );
    assert!(check(&foreign_session_operation).is_err());
    let foreign_audience = AudienceRef::Principal { principal: foreign };
    assert!(foreign_audience != route.grant_audience);
    let foreign_audience_operation = application_operations::catalog_read(
        &route.grant_caller,
        &route.logical_executor,
        &foreign_audience,
        &setup.configured.session_id,
    );
    assert!(check(&foreign_audience_operation).is_err());
    resolved.check_current().unwrap();
    contributor.check_current().unwrap();
}

fn revoke(
    fixture: &Fixture,
    reference: &PrivateOriginalRef,
    selected: impl IntoIterator<Item = GrantLineageRef>,
) {
    let route = fixture.source.route(&reference.route).unwrap();
    let mut custody = fixture.machine.try_controller_grant_mutation().unwrap();
    for leaf in selected {
        fixture
            .source
            .owner
            .grants()
            .revoke(&route.grant_caller, &leaf, &mut custody)
            .unwrap();
    }
}

fn setup_leaves(record: &SourceRecord) -> Vec<GrantLineageRef> {
    let basis = &record
        .final_association
        .as_ref()
        .unwrap()
        .candidate()
        .authority_basis;
    record
        .approved_task
        .as_ref()
        .unwrap()
        .setup()
        .unwrap()
        .binding_targets()
        .unwrap()
        .iter()
        .map(|target| {
            basis
                .lineage_for(Some(target))
                .unwrap()
                .last()
                .unwrap()
                .clone()
        })
        .collect()
}

fn assert_helper_contributor(
    fixture: &Fixture,
    parent: &InputAuthorityAssociation,
    session: &SessionId,
    expected_count: usize,
) -> InputAuthorityAssociation {
    let helpers = fixture.source.helper_sources.lock().unwrap().clone();
    assert_eq!(helpers.len(), expected_count);
    let mut found: Vec<_> = helpers
        .iter()
        .filter(|held| {
            let binding = held.original().association();
            assert_eq!(
                binding.candidate().target.context.as_str(),
                session.to_string()
            );
            fixture
                .source
                .helper_policy_source(binding)
                .unwrap()
                .unwrap()
                .contributors
                == vec![parent.clone()]
        })
        .map(|held| held.original().association().clone())
        .collect();
    assert_eq!(
        found.len(),
        1,
        "actual helper source names exactly the current Original contributor"
    );
    found.pop().unwrap()
}

struct HelperCompletion {
    row: StoredInputState,
    receipt: Box<InputTerminalReceipt>,
}

fn receipt_value(receipt: &InputTerminalReceipt) -> serde_json::Value {
    serde_json::json!({
        "input": receipt.input_id(), "terminal": receipt.terminal(),
        "attempts": receipt.attempt_count(), "run": receipt.run_id(),
        "owner": receipt.owner_input_id(), "recipients": receipt.recipient_input_ids(),
        "outcome": receipt.outcome(),
    })
}

async fn wait_helper(
    fixture: &Fixture,
    history: &dyn RuntimeStore,
    session: &SessionId,
    association: &InputAuthorityAssociation,
    prior: Option<&HelperCompletion>,
) -> HelperCompletion {
    let runtime = LogicalRuntimeId::for_session(session);
    assert_eq!(
        association.candidate().target.context.as_str(),
        session.to_string()
    );
    assert_eq!(
        association.candidate().target.logical_runtime.as_str(),
        runtime.to_string()
    );
    tokio::time::timeout(WAIT, async {
        loop {
            let rows = history.load_input_states_strict(&runtime).await.unwrap();
            let expected_count = 1 + usize::from(prior.is_some());
            assert!(rows.len() <= expected_count, "no unexpected helper input");
            if rows.len() == expected_count {
                let fresh: Vec<_> = rows
                    .iter()
                    .filter(|row| {
                        prior.is_none_or(|old| row.state.input_id != old.row.state.input_id)
                    })
                    .collect();
                assert_eq!(fresh.len(), 1, "exactly one new governed helper input");
                let row = fresh[0];
                assert_eq!(row.state.authority_contributors.len(), 1);
                let contributor = &row.state.authority_contributors[0];
                assert_eq!(contributor.input_id(), &row.state.input_id);
                assert_eq!(contributor.association(), association);
                if row.seed.phase == InputLifecycleState::Consumed {
                    assert_eq!(
                        row.seed.terminal_outcome,
                        Some(InputTerminalOutcome::Consumed)
                    );
                    let run = row.seed.last_run_id.as_ref().unwrap();
                    let read = fixture
                        .machine
                        .input_terminal_receipt(
                            session,
                            InteractionSelector::InputId(row.state.input_id.clone()),
                        )
                        .await
                        .unwrap()
                        .unwrap();
                    if let InputTerminalReceiptRead::Finalized(receipt) = read.report {
                        assert_eq!(receipt.input_id(), &row.state.input_id);
                        assert_eq!(receipt.terminal(), &InputTerminalOutcome::Consumed);
                        assert_eq!(receipt.run_id(), Some(run));
                        assert_eq!(receipt.owner_input_id(), &row.state.input_id);
                        assert_eq!(receipt.recipient_input_ids(), &[row.state.input_id.clone()]);
                        assert!(matches!(receipt.outcome(), CompletionOutcome::Completed(_)));
                        // The live machine retains only its latest terminal run.
                        // Earlier helper runs remain proven by their exact durable
                        // input row and finalized receipt, not this live witness.
                        if fixture.machine.run_turn_terminal(session, run).await
                            == Some(TurnTerminalOutcome::Completed)
                        {
                            // Receipt finalization can advance the canonical row
                            // after the earlier history read. Retain the settled
                            // row only after the positive receipt/live witnesses.
                            let settled_rows =
                                history.load_input_states_strict(&runtime).await.unwrap();
                            assert_eq!(settled_rows.len(), expected_count);
                            let settled = settled_rows
                                .iter()
                                .find(|entry| entry.state.input_id == row.state.input_id)
                                .unwrap();
                            assert_eq!(settled.seed.phase, InputLifecycleState::Consumed);
                            assert_eq!(
                                settled.seed.terminal_outcome,
                                Some(InputTerminalOutcome::Consumed)
                            );
                            assert_eq!(settled.seed.last_run_id.as_ref(), Some(run));
                            assert_eq!(settled.seed.attempt_count, receipt.attempt_count());
                            assert_eq!(settled.state.authority_contributors.len(), 1);
                            let settled_contributor = &settled.state.authority_contributors[0];
                            assert_eq!(settled_contributor.input_id(), &row.state.input_id);
                            assert_eq!(settled_contributor.association(), association);
                            if let Some(prior) = prior {
                                let old = settled_rows
                                    .iter()
                                    .find(|old| old.state.input_id == prior.row.state.input_id)
                                    .unwrap();
                                assert_eq!(
                                    serde_json::to_value(old).unwrap(),
                                    serde_json::to_value(&prior.row).unwrap()
                                );
                                assert_ne!(old.seed.last_run_id.as_ref(), Some(run));
                                let old_read = fixture
                                    .machine
                                    .input_terminal_receipt(
                                        session,
                                        InteractionSelector::InputId(old.state.input_id.clone()),
                                    )
                                    .await
                                    .unwrap()
                                    .unwrap();
                                let InputTerminalReceiptRead::Finalized(old_receipt) =
                                    old_read.report
                                else {
                                    panic!("prior helper receipt must remain finalized");
                                };
                                assert_eq!(
                                    receipt_value(&old_receipt),
                                    receipt_value(&prior.receipt)
                                );
                            }
                            return HelperCompletion {
                                row: settled.clone(),
                                receipt,
                            };
                        }
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("exact helper input and terminal receipt must settle within the fixture deadline")
}

async fn wait_original(
    fixture: &Fixture,
    history: &dyn RuntimeStore,
    reference: &PrivateOriginalRef,
    inputs: usize,
) {
    tokio::time::timeout(WAIT, async {
        loop {
            let record = source_record(fixture, reference).await;
            let admission = record.service_admission.as_ref().unwrap();
            let rows = history
                .load_input_states_strict(&LogicalRuntimeId::for_session(&fixture.session))
                .await
                .unwrap();
            assert_eq!(rows.len(), inputs);
            let row = rows
                .iter()
                .find(|row| row.state.input_id.to_string() == admission.input_id)
                .unwrap();
            if row.seed.phase == InputLifecycleState::Consumed {
                if let Some(run) = &row.seed.last_run_id {
                    if fixture
                        .machine
                        .run_turn_terminal(&fixture.session, run)
                        .await
                        == Some(TurnTerminalOutcome::Completed)
                    {
                        let receipt = fixture
                            .machine
                            .input_terminal_receipt(
                                &fixture.session,
                                InteractionSelector::InputId(row.state.input_id.clone()),
                            )
                            .await
                            .unwrap()
                            .unwrap();
                        if let InputTerminalReceiptRead::Finalized(receipt) = receipt.report {
                            let CompletionOutcome::Completed(result) = receipt.outcome() else {
                                panic!("native Original must complete");
                            };
                            assert_eq!(result.text, FINAL);
                            break;
                        }
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[derive(Default)]
struct ScriptState {
    mode: usize,
    phase: usize,
    last: Option<String>,
    choices: usize,
    results: Vec<ToolResult>,
    started: bool,
    expected_contexts: Vec<serde_json::Value>,
    prior_contexts: Vec<serde_json::Value>,
    observed_contexts: Vec<serde_json::Value>,
}
struct WarmScript {
    state: Mutex<ScriptState>,
    reached: Notify,
    release: Semaphore,
    setup_refusal: bool,
}
impl WarmScript {
    fn new(setup_refusal: bool) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(ScriptState::default()),
            reached: Notify::new(),
            release: Semaphore::new(0),
            setup_refusal,
        })
    }
    fn arm(&self, mode: usize, expected_contexts: Vec<serde_json::Value>) {
        let mut state = self.state.lock().unwrap();
        assert!(state.mode == 0 || state.phase == 3);
        assert_eq!(expected_contexts.len(), if mode == 1 { 1 } else { 2 });
        assert_eq!(state.observed_contexts.len(), if mode == 1 { 0 } else { 1 });
        let original = &expected_contexts[0]["original"];
        assert!(
            expected_contexts
                .iter()
                .all(|value| &value["original"] == original)
        );
        assert!(
            state
                .observed_contexts
                .iter()
                .all(|value| &value["original"] != original)
        );
        let prior_contexts = state.observed_contexts.clone();
        *state = ScriptState {
            mode,
            expected_contexts,
            prior_contexts,
            ..Default::default()
        };
    }
    fn resume(&self) {
        self.release.add_permits(1);
    }
    async fn choice(
        &self,
        request: &PreparedLlmRequest,
    ) -> Option<(String, String, serde_json::Value)> {
        let first = {
            let mut state = self.state.lock().unwrap();
            if let Some(last) = &state.last {
                if let Some(result) = request
                    .request()
                    .messages
                    .iter()
                    .flat_map(|message| match message {
                        Message::ToolResults { results, .. } => results.as_slice(),
                        _ => &[],
                    })
                    .find(|result| &result.tool_use_id == last)
                {
                    let expected_refusal =
                        (state.mode == 1 && state.phase == 2 && self.setup_refusal)
                            || (state.mode == 2 && state.phase == 1);
                    if result.is_error {
                        assert_public_refusal(result);
                        if expected_refusal {
                            state.results.push(result.clone());
                            state.phase += 1;
                            state.last = None;
                        }
                        // The public refusal intentionally erases Denied versus
                        // ReprepareRequired. Other phases may make a bounded
                        // fresh choice, never infer a private terminal cause.
                    } else {
                        assert!(!expected_refusal, "revoked target cannot succeed");
                        assert!(result.settlement_failures.is_empty());
                        if state.phase == 0 || (state.mode == 2 && state.phase == 3) {
                            let catalog: serde_json::Value =
                                serde_json::from_str(&result.text_content()).unwrap();
                            assert_eq!(catalog["catalog_exact"], true);
                            let names: BTreeSet<_> = ["accepted_names", "noop_names"]
                                .into_iter()
                                .flat_map(|key| catalog[key].as_array().into_iter().flatten())
                                .map(|name| name.as_str().unwrap().to_owned())
                                .collect();
                            let expected: BTreeSet<_> = if state.mode == 1 {
                                [X_READ].into_iter().map(str::to_owned).collect()
                            } else {
                                [X_READ, Y_READ].into_iter().map(str::to_owned).collect()
                            };
                            assert_eq!(names, expected);
                        }
                        state.results.push(result.clone());
                        state.phase += 1;
                        state.last = None;
                    }
                }
            }
            if !state.started {
                let contexts: Vec<serde_json::Value> = request
                    .request()
                    .messages
                    .iter()
                    .filter_map(|message| match message {
                        Message::User(user) if user.transcript_role.is_injected_context() => {
                            serde_json::from_str(&user.text_content()).ok()
                        }
                        _ => None,
                    })
                    .filter(|value: &serde_json::Value| {
                        value["schema"] == "toolkit.approved-service-context.v1"
                    })
                    .collect();
                // C shares the Original session with B. The historical B
                // context remains verbatim; only C's exact new contexts
                // describe this admission, and neither is use authority.
                let prior = state.prior_contexts.len();
                assert_eq!(contexts.len(), prior + state.expected_contexts.len());
                assert_eq!(&contexts[..prior], state.prior_contexts.as_slice());
                for (actual, expected) in contexts[prior..].iter().zip(&state.expected_contexts) {
                    for (key, value) in expected.as_object().unwrap() {
                        if key == "expires_at_ms" {
                            // Native parent attenuation can shorten the
                            // declared task deadline, never extend it.
                            let actual = actual[key].as_u64().unwrap();
                            assert!(actual > 0 && actual <= value.as_u64().unwrap());
                        } else {
                            assert_eq!(
                                &actual[key], value,
                                "fresh context must match this source's exact approved access member: {key}"
                            );
                        }
                    }
                }
                assert!(
                    contexts.iter().all(
                        |value| value["attachment"] == "observed_usable_before_input_admission"
                    )
                );
                state.observed_contexts = contexts;
                state.started = true;
                true
            } else {
                false
            }
        };
        if first {
            self.reached.notify_one();
            self.release.acquire().await.unwrap().forget();
        }
        let mut state = self.state.lock().unwrap();
        if state.phase == if state.mode == 1 { 3 } else { 4 } {
            return None;
        }
        assert!(
            state.choices < 16,
            "bounded mechanical native choices exhausted"
        );
        state.choices += 1;
        let name = match (state.mode, state.phase) {
            (_, 0) | (2, 3) => "tool_catalog_load",
            (_, 1) => X_READ,
            (1, 2) => "find_and_install",
            (2, 2) => Y_READ,
            _ => unreachable!(),
        };
        assert!(
            request
                .request()
                .tools
                .iter()
                .any(|tool| tool.name.as_str() == name),
            "actual selected native tool not offered: {name}"
        );
        let args = match name {
            "tool_catalog_load" if state.mode == 1 => serde_json::json!({"names":[X_READ]}),
            "tool_catalog_load" => serde_json::json!({"names":[X_READ,Y_READ]}),
            "find_and_install" => {
                serde_json::json!({"request":"Use the current X reading and add a second public source for this task"})
            }
            _ => serde_json::json!({}),
        };
        let id = format!(
            "warm-native-{}-{}-{}",
            state.mode, state.phase, state.choices
        );
        state.last = Some(id.clone());
        Some((id, name.into(), args))
    }
}

fn assert_public_refusal(result: &ToolResult) {
    let expected = meerkat_core::ops::terminal_tool_outcome_for_error(
        result.tool_use_id.clone(),
        meerkat_core::ToolError::AuthorizationRefused {
            refusal: meerkat_core::OperationRefused::new(
                meerkat_core::OperationRefusalKind::Denied,
            ),
        },
    )
    .result;
    assert_eq!(
        serde_json::to_value(result).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
}

struct WarmProvider {
    inner: Arc<dyn LlmClient>,
    script: Arc<WarmScript>,
}
#[async_trait]
impl LlmClient for WarmProvider {
    async fn prepare_controller_credential(&self) -> Result<(), meerkat_core::auth::AuthError> {
        self.inner.prepare_controller_credential().await
    }
    fn plain_model_route(
        &self,
        model: &str,
    ) -> Result<PlainModelRoute, meerkat_core::ControllerFactsUnavailable> {
        self.inner.plain_model_route(model)
    }
    fn controller_model_selection(&self) -> Option<ControllerModelSelection> {
        self.inner.controller_model_selection()
    }
    fn project_replay_messages(&self, messages: &[Message]) -> Result<Vec<Message>, LlmError> {
        self.inner.project_replay_messages(messages)
    }
    fn stream<'a>(&'a self, request: &'a LlmRequest) -> LlmStream<'a> {
        self.inner.stream(request)
    }
    fn stream_prepared<'a>(&'a self, request: &'a PreparedLlmRequest) -> LlmStream<'a> {
        let stream = self.inner.stream_prepared(request);
        let helper = request
            .request()
            .tools
            .iter()
            .any(|tool| tool.name == SETUP_REPORT);
        if helper || self.script.state.lock().unwrap().mode == 0 {
            return stream;
        }
        Box::pin(futures::stream::unfold(
            (stream, false),
            move |(mut stream, mut tool)| async move {
                let event = stream.next().await?;
                let event = match event {
                    Ok(LlmEvent::TextDelta { meta, .. }) => match self.script.choice(request).await
                    {
                        Some((id, name, args)) => {
                            tool = true;
                            Ok(LlmEvent::ToolCallComplete {
                                id,
                                name,
                                args,
                                meta: None,
                            })
                        }
                        None => Ok(LlmEvent::TextDelta {
                            delta: FINAL.into(),
                            meta,
                        }),
                    },
                    Ok(LlmEvent::Done {
                        outcome: LlmDoneOutcome::Success { .. },
                    }) if tool => Ok(LlmEvent::Done {
                        outcome: LlmDoneOutcome::success(StopReason::ToolUse),
                    }),
                    other => other,
                };
                Some((event, (stream, tool)))
            },
        ))
    }
    fn provider(&self) -> Provider {
        self.inner.provider()
    }
    async fn health_check(&self) -> Result<(), LlmError> {
        self.inner.health_check().await
    }
}

async fn warm_case(setup_refusal: bool) {
    let acquisition = acquisition_denial::Script::paused_for_helper_entry();
    let report = Report::new(acquisition.tools().result_tool());
    let script = WarmScript::new(setup_refusal);
    let selected = script.clone();
    let (fixture, history) = Fixture::for_warm_helper_report_with_provider(
        acquisition.clone(),
        report.clone(),
        Arc::new(move |inner| {
            Arc::new(WarmProvider {
                inner,
                script: selected.clone(),
            })
        }),
    )
    .await;
    let endpoints = [Endpoint::new(), Endpoint::new()];
    let configurations = [endpoints[0].config(X), endpoints[1].config(Y)];
    fixture
        .source
        .bind_service_installation_readback(Arc::new(PackageReadback {
            configurations: configurations.to_vec(),
        }))
        .unwrap();
    let (_pipe, _logins, login) = super::super::helper_entry_native::enrolled_login();
    let outcome = std::panic::AssertUnwindSafe(tokio::time::timeout(
        WAIT * 6,
        Box::pin(async {
            let first = fixture.submit().await;
            acquisition.first_response_waiting().await;
            let a = fixture.binding().await.unwrap().unwrap();
            super::super::helper_entry_native::retain_actual_a_login(&fixture, &a, &login);
            acquisition.release_first_response();
            fixture.finish(&first).await.unwrap();
            let (b, helper_session) = prepare_followup(
                &fixture,
                &acquisition,
                &report,
                &configurations[..1],
                fixture.original_reference(),
            )
            .await;
            let first_helper_binding = assert_helper_contributor(&fixture, &a, &helper_session, 1);
            let b_record = attach_and_enter(&fixture, &b, &endpoints[0], &script, 1).await;
            assert_reviewed_catalog_scope(&fixture, &b, &b_record);
            let b_binding = b_record.final_association.as_ref().unwrap();
            assert!(
                leaves(&a)
                    .iter()
                    .all(|leaf| !leaves(b_binding).contains(leaf))
            );
            let selected_b = fixture
                .source
                .service_task_snapshot(&b_record)
                .await
                .unwrap()
                .approved_service()
                .unwrap()
                .clone();
            // Native revocation, not parent source cancellation: cancelling A
            // would correctly cancel the business-linked descendants too.
            revoke(&fixture, fixture.original_reference(), leaves(&a));
            let route = fixture.source.route(&b.route).unwrap();
            for lineage in a.candidate().authority_basis.grant_lineages() {
                assert!(
                    fixture
                        .source
                        .owner
                        .grants()
                        .resolve_lineage(
                            lineage,
                            &route.logical_executor,
                            route.represented_subject.as_ref()
                        )
                        .is_err()
                );
            }
            report.release();
            let first_helper = wait_helper(
                &fixture,
                history.as_ref(),
                &helper_session,
                &first_helper_binding,
                None,
            )
            .await;
            let old_feedback = report.feedback.lock().unwrap().clone();
            assert!(!old_feedback.is_empty() && old_feedback.iter().all(|result| result.is_error));
            assert!(
                fixture
                    .source
                    .service_task_snapshot(&source_record(&fixture, &b).await)
                    .await
                    .unwrap()
                    .approved_service()
                    == Some(&selected_b),
                "late old report cannot replace selected B"
            );
            if setup_refusal {
                revoke(&fixture, &b, setup_leaves(&b_record));
            } else {
                // The actual first helper turn is settled before rearming only
                // mechanical output capture for a new governed helper occurrence.
                assert!(report.leaf.context.lock().unwrap().is_none());
                report.leaf.released.store(false, Ordering::SeqCst);
                report.first.store(true, Ordering::SeqCst);
                report.feedback.lock().unwrap().clear();
            }
            script.resume();
            if setup_refusal {
                wait_original(&fixture, history.as_ref(), &b, 2).await;
                let state = script.state.lock().unwrap();
                assert_eq!(state.results.len(), 3);
                assert!(
                    !state.results[0].is_error
                        && !state.results[1].is_error
                        && state.results[2].is_error
                );
                assert_eq!(
                    endpoints[0].calls(),
                    1,
                    "B service use survives B setup-only revoke"
                );
                assert_eq!(
                    fixture.source.helper_sources.lock().unwrap().len(),
                    1,
                    "refused find cannot admit another helper"
                );
            } else {
                wait_original(&fixture, history.as_ref(), &b, 2).await;
                let (c, second_helper) =
                    prepare_followup(&fixture, &acquisition, &report, &configurations, &b).await;
                assert_eq!(
                    second_helper, helper_session,
                    "same actual warm helper session"
                );
                let second_helper_binding =
                    assert_helper_contributor(&fixture, b_binding, &helper_session, 2);
                assert_eq!(endpoints[0].calls(), 1);
                let c_record = attach_and_enter(&fixture, &c, &endpoints[1], &script, 2).await;
                let c_binding = c_record.final_association.as_ref().unwrap();
                assert!(
                    leaves(c_binding)
                        .iter()
                        .all(|leaf| !leaves(b_binding).contains(leaf))
                );
                assert!(
                    leaves(c_binding)
                        .iter()
                        .all(|leaf| !leaves(&a).contains(leaf))
                );
                assert!(c_binding.candidate().original_work != b_binding.candidate().original_work);
                let b_task = b_record.approved_task.as_ref().unwrap();
                let c_task = c_record.approved_task.as_ref().unwrap();
                assert!(
                    b_task
                        .grants()
                        .iter()
                        .all(|prior| !c_task.grants().contains(prior))
                );
                let selected_c = fixture
                    .source
                    .service_task_snapshot(&c_record)
                    .await
                    .unwrap()
                    .approved_service()
                    .unwrap()
                    .clone();
                revoke(&fixture, &b, setup_leaves(&b_record));
                report.release();
                let second_helper = wait_helper(
                    &fixture,
                    history.as_ref(),
                    &helper_session,
                    &second_helper_binding,
                    Some(&first_helper),
                )
                .await;
                assert_ne!(
                    second_helper.row.state.input_id,
                    first_helper.row.state.input_id
                );
                let old_feedback = report.feedback.lock().unwrap().clone();
                assert!(
                    !old_feedback.is_empty() && old_feedback.iter().all(|result| result.is_error)
                );
                assert!(
                    fixture
                        .source
                        .service_task_snapshot(&source_record(&fixture, &c).await)
                        .await
                        .unwrap()
                        .approved_service()
                        == Some(&selected_c)
                );
                let x = c_binding
                    .candidate()
                    .authority_basis
                    .lineage_for(Some(&GrantTarget::McpServer { server: X.into() }))
                    .unwrap()
                    .last()
                    .unwrap()
                    .clone();
                revoke(&fixture, &c, [x]);
                script.resume();
                wait_original(&fixture, history.as_ref(), &c, 3).await;
                assert_eq!(
                    endpoints[0].calls(),
                    1,
                    "revoked C/X never enters the actual server"
                );
                assert_eq!(
                    endpoints[1].calls(),
                    1,
                    "independent C/Y survives X-only revoke"
                );
                let state = script.state.lock().unwrap();
                assert_eq!(state.results.len(), 4);
                assert!(state.results[1].is_error);
                assert!(
                    !state.results[0].is_error
                        && !state.results[2].is_error
                        && !state.results[3].is_error
                );
                drop(state);
                let before = history
                    .load_input_states_strict(&LogicalRuntimeId::for_session(&fixture.session))
                    .await
                    .unwrap();
                sweep(&fixture).await;
                let after = history
                    .load_input_states_strict(&LogicalRuntimeId::for_session(&fixture.session))
                    .await
                    .unwrap();
                assert_eq!(
                    serde_json::to_value(before).unwrap(),
                    serde_json::to_value(after).unwrap(),
                    "settled replay does not admit another C"
                );
            }
            // Real admitted cancellation supplies the other side of the partial
            // issuance cleanup guard; bound/admitted B is never unbound cleanup.
            fixture
                .source
                .record_task_group_cancelled(&b)
                .await
                .unwrap();
            let stored = fixture
                .store
                .get(&fixture.source.key(&b).unwrap())
                .await
                .unwrap()
                .unwrap();
            let cancelled = fixture.source.decode_record(&stored, &b).unwrap();
            assert!(
                cancelled.operation_cancelled
                    && cancelled.service_admission.is_some()
                    && cancelled.bound_service_input.is_some()
                    && cancelled.final_association.is_some()
            );
            let journal = fixture.source_path.with_file_name("grants.jsonl");
            let before = std::fs::read(&journal).unwrap();
            assert!(
                fixture
                    .source
                    .prepare_service_setup_cleanup(&b, &stored)
                    .await
                    .is_err()
            );
            assert_eq!(std::fs::read(&journal).unwrap(), before);
        }),
    ))
    .catch_unwind()
    .await;
    acquisition.release_first_response();
    script.resume();
    report.release();
    for endpoint in &endpoints {
        endpoint.release();
    }
    report.leaf.context.lock().unwrap().take();
    fixture.source.join_service_attachments().await;
    let cleanup = fixture.shutdown().await;
    assert!(cleanup.is_ok(), "{cleanup:?}");
    match outcome {
        Ok(Ok(())) => (),
        Ok(Err(_)) => panic!("warm native composition exceeded its bounded deadline"),
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn warm_helper_reacquisition_uses_fresh_c_grants_and_independent_native_targets() {
    warm_case(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn admitted_b_setup_revoke_preserves_b_service_and_refuses_another_helper() {
    warm_case(true).await;
}
