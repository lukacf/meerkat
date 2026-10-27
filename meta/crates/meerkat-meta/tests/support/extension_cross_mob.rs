//! Native delegate -> fork_off acceptance through the registered probe bundle.
//! Delegate retirement cascades to its fork. Meta currently gives implicit
//! child mobs in-memory journals, so this test proves the live access route
//! and durable document/ACL storage without claiming a persistent child mob.

use super::*;
use meerkat_core::{Message, SessionId};
use meerkat_mob::{MemberCreationProvenance, MobHandle, MobId};
use tokio::sync::{Semaphore, mpsc};

const OWNER: &str = "CROSS_MOB_OWNER";
const HELPER: &str = "CROSS_MOB_HELPER";
const FORK: &str = "CROSS_MOB_FORK";
const REOPEN_OWNER: &str = "CROSS_MOB_REOPEN_OWNER";
const ROOT_OWNER: &str = "PERSISTENT_ROOT_OWNER";
const ROOT_FORK: &str = "PERSISTENT_ROOT_FORK";
const ROOT_BARRIER: &str = "PERSISTENT_ROOT_BARRIER";
const ROOT_REVOKED: &str = "PERSISTENT_ROOT_REVOKED";
const ROOT_REOPEN_DENIED: &str = "PERSISTENT_ROOT_REOPEN_DENIED";
const ROOT_REOPEN_EDITOR: &str = "PERSISTENT_ROOT_REOPEN_EDITOR";
const ROOT_REOPEN_REVOKED: &str = "PERSISTENT_ROOT_REOPEN_REVOKED";
const POLICY_OWNER: &str = "RESTRICTED_POLICY_OWNER";
const POLICY_SOURCE: &str = "RESTRICTED_POLICY_SOURCE";
const POLICY_FORK: &str = "RESTRICTED_POLICY_FORK";
const POLICY_BARRIER: &str = "RESTRICTED_POLICY_BARRIER";

#[derive(Default)]
struct Evidence {
    document: Option<Document>,
    actors: BTreeMap<String, (SessionId, Principal)>,
    sibling_documents: BTreeMap<String, Document>,
    backend_denials: std::collections::BTreeSet<String>,
}

pub(super) struct Scenario {
    evidence: tokio::sync::Mutex<Evidence>,
    events: mpsc::UnboundedSender<String>,
    grant: Semaphore,
    revoke: Semaphore,
    finish: Semaphore,
    finish_fork: Semaphore,
}

impl Scenario {
    fn new() -> (Arc<Self>, mpsc::UnboundedReceiver<String>) {
        let (events, receiver) = mpsc::unbounded_channel();
        (
            Arc::new(Self {
                evidence: Default::default(),
                events,
                grant: Semaphore::new(0),
                revoke: Semaphore::new(0),
                finish: Semaphore::new(0),
                finish_fork: Semaphore::new(0),
            }),
            receiver,
        )
    }

    pub(super) async fn dispatch(
        &self,
        bundle: &ToolBundleContext,
        caller: &HostAccessContext,
        call: ToolCallView<'_>,
        execution: &ToolDispatchContext,
    ) -> std::result::Result<ToolDispatchOutcome, ToolError> {
        let args: serde_json::Value = call.parse_args().unwrap();
        let action = args["action"].as_str().unwrap();
        let actor = args["actor"].as_str().unwrap();
        assert_ne!(
            action, "policy-forbidden",
            "the native execution policy must reject documents_apply before dispatch"
        );
        let session = execution.origin_session_id().unwrap().clone();
        {
            let mut evidence = self.evidence.lock().await;
            let actual = (session, caller.principal().clone());
            if let Some(previous) = evidence.actors.get(actor) {
                assert_eq!(previous, &actual, "native actor identity must stay exact");
            } else {
                evidence.actors.insert(actor.to_owned(), actual);
            }
        }
        let mut denied = false;
        if matches!(action, "create" | "sibling-create") {
            if action == "create" {
                assert_eq!(actor, "owner");
            }
            let receipt = bundle
                .documents
                .mutate(
                    caller,
                    &request(&format!("{actor}-{action}")),
                    Mutation::Create(NewDocument {
                        content: content(),
                        owner: None,
                        access: DocumentAccess::default(),
                    }),
                )
                .await
                .unwrap()
                .receipt;
            let document = bundle
                .documents
                .get(caller, &receipt.document_id)
                .await
                .unwrap();
            assert_eq!(document.owner, Owner::Agent(caller.principal().clone()));
            let mut evidence = self.evidence.lock().await;
            if action == "create" {
                evidence.document = Some(document);
            } else {
                assert_ne!(document.id, evidence.document.as_ref().unwrap().id);
                assert!(
                    evidence
                        .sibling_documents
                        .insert(actor.into(), document)
                        .is_none()
                );
            }
        } else {
            let document = self.evidence.lock().await.document.clone().unwrap();
            if action == "owner-persisted" {
                let visible = bundle.documents.get(caller, &document.id).await.unwrap();
                assert_eq!(visible.id, document.id);
                assert_eq!(visible.revision, document.revision);
                assert_eq!(visible.owner, Owner::Agent(caller.principal().clone()));
                assert_eq!(visible.content.payload, content().payload);
                assert!(visible.access.grants.is_empty());
            } else if action.starts_with("editor") {
                let visible = bundle.documents.get(caller, &document.id).await.unwrap();
                assert_eq!(visible.id, document.id);
                assert_eq!(visible.content.payload, content().payload);
                assert_eq!(visible.owner, document.owner);
                bundle
                    .documents
                    .mutate(
                        caller,
                        &request(&format!("{actor}-{action}-edit")),
                        Mutation::Replace {
                            id: document.id.clone(),
                            expected_revision: visible.revision,
                            content: content(),
                        },
                    )
                    .await
                    .unwrap();
                let updated = bundle.documents.get(caller, &document.id).await.unwrap();
                assert!(matches!(
                    bundle
                        .documents
                        .mutate(
                            caller,
                            &request(&format!("{actor}-{action}-admin")),
                            Mutation::SetAccess {
                                id: document.id,
                                expected_revision: updated.revision.clone(),
                                access: DocumentAccess::default(),
                            }
                        )
                        .await,
                    Err(Error::NotFound)
                ));
                self.evidence.lock().await.document = Some(updated);
            } else if matches!(action, "reader-edit-denied" | "reader-admin-denied") {
                let before = bundle.documents.get(caller, &document.id).await.unwrap();
                let mutation = if action == "reader-edit-denied" {
                    Mutation::Replace {
                        id: document.id.clone(),
                        expected_revision: before.revision.clone(),
                        content: DocumentContent {
                            payload: vec![9],
                            ..content()
                        },
                    }
                } else {
                    Mutation::SetAccess {
                        id: document.id.clone(),
                        expected_revision: before.revision.clone(),
                        access: DocumentAccess::default(),
                    }
                };
                assert!(matches!(
                    bundle
                        .documents
                        .mutate(caller, &request(&format!("{actor}-{action}")), mutation)
                        .await,
                    Err(Error::NotFound)
                ));
                let after = bundle.documents.get(caller, &document.id).await.unwrap();
                assert_eq!(after.revision, before.revision);
                assert_eq!(after.content.payload, before.content.payload);
                denied = true;
            } else if action.starts_with("reader") {
                let visible = bundle.documents.get(caller, &document.id).await.unwrap();
                assert_eq!(visible.id, document.id);
                assert_eq!(visible.content.payload, content().payload);
                assert_eq!(visible.owner, document.owner);
                if action.starts_with("reader-policy") {
                    // This read tool really executes. The provider independently
                    // refuses a write using its runtime-resolved caller context.
                    assert!(matches!(
                        bundle
                            .documents
                            .mutate(
                                caller,
                                &request(&format!("{actor}-{action}-backend-write")),
                                Mutation::Replace {
                                    id: document.id.clone(),
                                    expected_revision: visible.revision.clone(),
                                    content: DocumentContent {
                                        payload: vec![9],
                                        ..content()
                                    },
                                }
                            )
                            .await,
                        Err(Error::NotFound)
                    ));
                    assert_eq!(
                        bundle
                            .documents
                            .get(caller, &document.id)
                            .await
                            .unwrap()
                            .revision,
                        visible.revision
                    );
                    self.evidence
                        .lock()
                        .await
                        .backend_denials
                        .insert(actor.into());
                }
            } else {
                assert!(matches!(
                    bundle.documents.get(caller, &document.id).await,
                    Err(Error::NotFound)
                ));
                denied = true;
            }
        }
        self.events.send(format!("{actor}:{action}")).unwrap();
        let gate = match action {
            "private" => Some(&self.grant),
            "reader-hold" => Some(&self.revoke),
            "revoked-hold" if actor == "fork" => Some(&self.finish_fork),
            "revoked-hold" => Some(&self.finish),
            _ => None,
        };
        if let Some(gate) = gate {
            tokio::time::timeout(Duration::from_mins(1), gate.acquire())
                .await
                .expect("test did not release the native probe barrier")
                .unwrap()
                .forget();
        }
        Ok(ToolResult::new(
            call.id.to_owned(),
            if denied {
                r#"{"error":"not_found"}"#
            } else {
                "{}"
            }
            .into(),
            denied,
        )
        .into())
    }

    async fn set_shared(
        &self,
        bundle: &ToolBundleContext,
        owner_session: &SessionId,
        id: &str,
        shared: bool,
    ) {
        let helper = self.evidence.lock().await.actors["helper"].1.clone();
        let grants = if shared {
            vec![Grant {
                audience: Audience::Agent {
                    principal: helper,
                    reach: Reach::Forks,
                },
                role: Role::Reader,
            }]
        } else {
            vec![]
        };
        self.set_access(
            bundle,
            owner_session,
            id,
            DocumentAccess {
                grants,
                ..Default::default()
            },
        )
        .await;
    }

    async fn set_access(
        &self,
        bundle: &ToolBundleContext,
        owner_session: &SessionId,
        id: &str,
        access: DocumentAccess,
    ) {
        // The host refreshes the executing owner's exact native authority.
        // No fixture-created HostAccessContext or lineage enters the provider.
        let owner = caller(bundle, owner_session).await;
        let document = self.evidence.lock().await.document.clone().unwrap();
        let current = bundle.documents.get(&owner, &document.id).await.unwrap();
        bundle
            .documents
            .mutate(
                &owner,
                &request(id),
                Mutation::SetAccess {
                    id: document.id.clone(),
                    expected_revision: current.revision,
                    access,
                },
            )
            .await
            .unwrap();
        let updated = bundle.documents.get(&owner, &document.id).await.unwrap();
        self.evidence.lock().await.document = Some(updated);
    }
}

#[derive(Default)]
struct CrossMobClient {
    calls: Mutex<BTreeMap<&'static str, usize>>,
    pending: Mutex<BTreeMap<&'static str, Vec<ExpectedResult>>>,
    observed: Mutex<std::collections::BTreeSet<String>>,
    completed: Mutex<std::collections::BTreeSet<&'static str>>,
}

struct ExpectedResult {
    id: String,
    label: String,
    error: Option<&'static str>,
}

struct ScriptCall {
    name: &'static str,
    args: serde_json::Value,
    error: Option<&'static str>,
}

impl CrossMobClient {
    fn after_restart() -> Self {
        // Replayed job-completion notifications may revive an owner. They
        // must not repeat this test's already-completed creation script.
        Self {
            calls: Mutex::new(BTreeMap::from([(OWNER, 10), (ROOT_OWNER, 10)])),
            ..Default::default()
        }
    }

    fn assert_observed(&self, labels: &[&str], completed: &[&str]) {
        let observed = self.observed.lock().unwrap();
        for label in labels {
            assert!(
                observed.contains(*label),
                "next model request did not observe {label}"
            );
        }
        let actual = self.completed.lock().unwrap();
        for role in completed {
            assert!(
                actual.contains(role),
                "scripted native turn did not complete: {role}"
            );
        }
    }
}

#[async_trait::async_trait]
impl LlmClient for CrossMobClient {
    fn project_replay_messages(
        &self,
        messages: &[Message],
    ) -> std::result::Result<Vec<Message>, LlmError> {
        Ok(messages.to_vec())
    }

    fn provider(&self) -> meerkat_core::Provider {
        meerkat_core::Provider::OpenAI
    }

    async fn health_check(&self) -> std::result::Result<(), LlmError> {
        Ok(())
    }

    fn stream<'a>(
        &'a self,
        request: &'a LlmRequest,
    ) -> Pin<Box<dyn futures::Stream<Item = std::result::Result<LlmEvent, LlmError>> + Send + 'a>>
    {
        let role = request
            .messages
            .iter()
            .rev()
            .filter_map(|message| match message {
                Message::User(user) => Some(user.text_content()),
                _ => None,
            })
            .find_map(|text| {
                [
                    POLICY_OWNER,
                    POLICY_SOURCE,
                    POLICY_FORK,
                    POLICY_BARRIER,
                    REOPEN_OWNER,
                    ROOT_REOPEN_DENIED,
                    ROOT_REOPEN_EDITOR,
                    ROOT_REOPEN_REVOKED,
                    ROOT_REVOKED,
                    ROOT_BARRIER,
                    ROOT_FORK,
                    ROOT_OWNER,
                    FORK,
                    HELPER,
                    OWNER,
                ]
                .into_iter()
                .find(|role| text.contains(role))
            })
            .expect("scripted native member task marker");
        let index = {
            let mut calls = self.calls.lock().unwrap();
            let index = calls.entry(role).or_default();
            let current = *index;
            *index += 1;
            current
        };
        // Inspect the very next model request, not just dispatcher internals.
        // Forks may also inherit older parent results, so match exact call IDs.
        if let Some(expected) = self.pending.lock().unwrap().remove(role) {
            for expected in expected {
                let result = request
                    .messages
                    .iter()
                    .find_map(|message| {
                        if let Message::ToolResults { results, .. } = message {
                            results
                                .iter()
                                .find(|result| result.tool_use_id == expected.id)
                        } else {
                            None
                        }
                    })
                    .unwrap_or_else(|| panic!("next model request omitted {}", expected.id));
                assert_eq!(
                    result.is_error,
                    expected.error.is_some(),
                    "{}: {}",
                    expected.label,
                    result.text_content()
                );
                if let Some(error) = expected.error {
                    assert!(
                        result.text_content().contains(error),
                        "{}: {}",
                        expected.label,
                        result.text_content()
                    );
                }
                self.observed.lock().unwrap().insert(expected.label);
            }
        }
        let expected_provenance =
            meerkat_core::ToolProvenance::new(meerkat_core::ToolSourceKind::RustBundle, NAME);
        let name_restricted = matches!(role, POLICY_SOURCE | POLICY_FORK | POLICY_BARRIER);
        for name in ["documents_read", "documents_apply"] {
            let tool = request.tools.iter().find(|tool| tool.name == name);
            if name_restricted && name == "documents_apply" {
                // Static name denials also restrict native session visibility.
                assert!(tool.is_none(), "{role}: denied tool must not be advertised");
            } else {
                let tool = tool.unwrap_or_else(|| panic!("{role}: missing {name}"));
                assert_eq!(tool.provenance.as_ref(), Some(&expected_provenance));
            }
        }
        let probe = |actor: &str, action: &str| ScriptCall {
            name: if matches!(
                action,
                "create"
                    | "sibling-create"
                    | "reader-edit-denied"
                    | "reader-admin-denied"
                    | "policy-forbidden"
            ) || action.starts_with("editor")
            {
                "documents_apply"
            } else {
                "documents_read"
            },
            args: serde_json::json!({"actor":actor,"action":action,
                    "principal":"forged-admin","lineage":["forged-root"]}),
            error: if action == "policy-forbidden" {
                Some("policy")
            } else if matches!(
                action,
                "private"
                    | "reader-edit-denied"
                    | "reader-admin-denied"
                    | "revoked-hold"
                    | "root-revoked"
                    | "root-reopen-denied"
                    | "root-reopen-revoked"
            ) {
                Some("not_found")
            } else {
                None
            },
        };
        let native = |name, args| ScriptCall {
            name,
            args,
            error: None,
        };
        let calls = match (role, index) {
            (OWNER | POLICY_OWNER, 0) => vec![probe("owner", "create")],
            (OWNER, 1) => vec![native(
                "delegate",
                serde_json::json!({"member_id":"cross-mob-helper","task":HELPER,
                    "result_label":"cross-mob-helper-result","max_text_bytes":1024,
                    "tooling":{"mode":"inherit_parent"}}),
            )],
            (HELPER, 0) => vec![probe("helper", "private")],
            (HELPER, 1) => vec![
                probe("helper", "reader-edit-denied"),
                probe("helper", "sibling-create"),
            ],
            (HELPER, 2) => vec![probe("helper", "reader-admin-denied")],
            (HELPER, 3) => vec![probe("helper", "reader-before-fork")],
            (HELPER, 4) => vec![native(
                "fork_off",
                serde_json::json!({"member_id":"cross-mob-fork","task":FORK,
                    "result_label":"cross-mob-fork-result","max_text_bytes":1024,
                    "idle_retire_secs":3600}),
            )],
            (HELPER, 5) => vec![probe("helper", "reader-hold")],
            (HELPER, 6) => vec![probe("helper", "revoked-hold")],
            (FORK, 0) => vec![
                probe("fork", "reader-edit-denied"),
                probe("fork", "sibling-create"),
            ],
            (FORK, 1) => vec![probe("fork", "reader-admin-denied")],
            (FORK, 2) => vec![probe("fork", "reader-hold")],
            (FORK, 3) => vec![probe("fork", "revoked-hold")],
            (REOPEN_OWNER, 0) => vec![probe("owner", "owner-persisted")],
            (ROOT_OWNER, 0) => vec![probe("owner", "create")],
            (ROOT_OWNER, 1) => vec![native(
                "fork_off",
                serde_json::json!({
                    "member_id":"root-persistent-fork", "task":ROOT_FORK,
                    "idle_retire_secs":3600, "max_text_bytes":1024,
                }),
            )],
            (ROOT_FORK, 0) => vec![probe("fork", "editor-initial")],
            (ROOT_REVOKED, 0) => vec![probe("fork", "root-revoked")],
            (ROOT_REOPEN_DENIED, 0) => vec![probe("fork", "root-reopen-denied")],
            (ROOT_REOPEN_EDITOR, 0) => vec![probe("fork", "editor-reopen")],
            (ROOT_REOPEN_REVOKED, 0) => vec![probe("fork", "root-reopen-revoked")],
            (POLICY_SOURCE, 0) => vec![
                probe("source", "policy-forbidden"),
                probe("source", "reader-policy-source"),
            ],
            (POLICY_SOURCE, 1) => vec![native(
                "fork_off",
                serde_json::json!({
                    "member_id":"policy-inherited-fork", "task":POLICY_FORK,
                    "idle_retire_secs":3600, "max_text_bytes":1024,
                }),
            )],
            (POLICY_FORK, 0) => vec![
                probe("policy-fork", "policy-forbidden"),
                probe("policy-fork", "reader-policy-fork"),
            ],
            _ => vec![],
        };
        let mut events = Vec::new();
        let mut expected = Vec::new();
        for (slot, call) in calls.iter().enumerate() {
            if name_restricted && call.name == "documents_apply" {
                // Deliberately forge the hidden call to exercise dispatch policy,
                // independently of the model-visible surface restriction.
                assert_eq!(call.error, Some("policy"));
            } else {
                assert!(
                    request.tools.iter().any(|tool| tool.name == call.name),
                    "native inherited surface must offer {}",
                    call.name
                );
            }
            let id = format!("{role}-{index}-{slot}");
            expected.push(ExpectedResult {
                id: id.clone(),
                label: format!(
                    "{role}:{}",
                    call.args["action"].as_str().unwrap_or(call.name)
                ),
                error: call.error,
            });
            events.push(Ok(LlmEvent::ToolCallComplete {
                id,
                name: call.name.into(),
                args: call.args.clone(),
                meta: None,
            }));
        }
        if calls.is_empty() {
            self.completed.lock().unwrap().insert(role);
            events.push(Ok(LlmEvent::TextDelta {
                delta: "done".into(),
                meta: None,
            }));
        } else {
            assert!(
                self.pending
                    .lock()
                    .unwrap()
                    .insert(role, expected)
                    .is_none()
            );
        }
        events.push(Ok(LlmEvent::UsageUpdate {
            usage: meerkat_core::TurnUsage::host_declared(
                self.provider(),
                &request.model,
                meerkat_core::Usage::default(),
            ),
        }));
        events.push(Ok(LlmEvent::Done {
            outcome: LlmDoneOutcome::success(if calls.is_empty() {
                StopReason::EndTurn
            } else {
                StopReason::ToolUse
            }),
        }));
        Box::pin(futures::stream::iter(events))
    }
}

async fn expect_events(receiver: &mut mpsc::UnboundedReceiver<String>, expected: &[&str]) {
    let mut expected: std::collections::BTreeSet<String> =
        expected.iter().map(|event| (*event).to_owned()).collect();
    while !expected.is_empty() {
        let event = tokio::time::timeout(Duration::from_mins(1), receiver.recv())
            .await
            .unwrap_or_else(|_| panic!("waiting for native probe events: {expected:?}"))
            .expect("native probe channel closed");
        assert!(
            expected.remove(&event),
            "unexpected native probe event {event}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn extension_native_delegate_cross_mob_fork_sharing_revocation_and_document_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (scenario, mut events) = Scenario::new();
    let client = Arc::new(CrossMobClient::default());
    let (runtime, context, _) =
        harness_with_client(dir.path(), None, client.clone(), Some(scenario.clone())).await;
    let root = runtime.spawn(worker("cross-mob-owner")).await.unwrap();
    let owner_session = member_session(&runtime, &root.agent_identity).await;
    let owner_turn = runtime
        .start_member_turn(
            "cross-mob-owner",
            ContentInput::Text(OWNER.into()),
            HandlingMode::Queue,
            meerkat_mob::MemberTurnOptions::new(),
            None,
        )
        .await
        .unwrap();
    expect_events(&mut events, &["owner:create", "helper:private"]).await;
    let (helper_session, helper_principal) =
        scenario.evidence.lock().await.actors["helper"].clone();
    let Principal::Worker {
        mob_id: child_mob,
        member_id,
        ..
    } = &helper_principal
    else {
        panic!("native delegate must resolve its own Worker principal");
    };
    assert_ne!(child_mob, runtime.mob_handle().mob_id().as_str());
    assert_eq!(member_id, "cross-mob-helper");
    assert_ne!(helper_session, owner_session);
    let child_mob = MobId::from(child_mob.as_str());
    let state = runtime.mob_runtime().agent_mob_mcp_state().unwrap();
    let child = state.handle_for(&child_mob).await.unwrap();
    let helper_creation = child
        .member_creation_for_session(&helper_session)
        .await
        .unwrap()
        .unwrap();
    let MemberCreationProvenance::Spawn { source } = &helper_creation.creation.provenance else {
        panic!("delegate must record a native Spawn edge");
    };
    assert_eq!(source.session_id, owner_session);
    assert_eq!(
        source.member_binding.mob_id,
        runtime.mob_handle().mob_id().as_str()
    );

    scenario
        .set_shared(&context, &owner_session, "cross-mob-share", true)
        .await;
    scenario.grant.add_permits(1);
    expect_events(
        &mut events,
        &[
            "helper:reader-edit-denied",
            "helper:sibling-create",
            "helper:reader-admin-denied",
            "helper:reader-before-fork",
            "helper:reader-hold",
            "fork:reader-edit-denied",
            "fork:sibling-create",
            "fork:reader-admin-denied",
            "fork:reader-hold",
        ],
    )
    .await;
    let (fork_session, fork_principal) = scenario.evidence.lock().await.actors["fork"].clone();
    assert_ne!(fork_principal, helper_principal);
    let fork_creation = child
        .member_creation_for_session(&fork_session)
        .await
        .unwrap()
        .unwrap();
    let MemberCreationProvenance::Fork { source_creation_id } = &fork_creation.creation.provenance
    else {
        panic!("fork_off must record a native Fork edge");
    };
    assert_eq!(
        Some(*source_creation_id),
        helper_creation.creation.creation_id
    );
    assert_eq!(
        fork_creation
            .fork_source
            .as_ref()
            .unwrap()
            .source_session_id,
        helper_session
    );
    scenario
        .set_shared(&context, &owner_session, "cross-mob-revoke", false)
        .await;
    scenario.revoke.add_permits(2);
    expect_events(&mut events, &["helper:revoked-hold", "fork:revoked-hold"]).await;
    // The inherited helper profile keeps its fork private. Observe the
    // existing native job's exact terminal receipt instead of submitting an
    // external turn. Keep the helper alive until its fork actually finishes.
    let fork_identity = AgentIdentity::from("cross-mob-fork");
    let fork_job = child
        .get_member(&fork_identity)
        .await
        .unwrap()
        .unwrap()
        .fork_job
        .expect("native fork_off records its detached job");
    assert_eq!(fork_job.owner_session_id, helper_session);
    let delivery = fork_job
        .turn_delivery
        .as_ref()
        .expect("runtime-backed fork job has its exact admitted delivery identity");
    let result_spec =
        meerkat_mob::BoundedResultSpec::new(fork_job.result_label, fork_job.max_text_bytes)
            .unwrap();
    scenario.finish_fork.add_permits(1);
    let report = child
        .wait_bounded_work_for_identity_with_delivery_identity(
            &fork_identity,
            delivery,
            &result_spec,
            meerkat_core::time_compat::Instant::now() + Duration::from_secs(45),
        )
        .await
        .unwrap();
    let meerkat_mob::runtime::DeliveryTerminalWait::Terminal(record) = report.work() else {
        panic!(
            "native fork job did not become terminal: {:?}",
            report.work()
        );
    };
    let meerkat_mob::runtime::DeliveryTerminalResolution::Receipt {
        runtime_run_id: Some(_),
        result: Ok(result),
        ..
    } = record.resolution()
    else {
        panic!(
            "native fork job did not complete successfully: {:?}",
            record.resolution()
        );
    };
    assert_eq!(result.session_id(), &fork_session);
    assert_eq!(result.result().text(), "done");
    assert_eq!(
        result.result().status(),
        meerkat_mob::BoundedHelperResultStatus::Completed
    );
    scenario.finish.add_permits(1);
    tokio::time::timeout(Duration::from_mins(1), owner_turn.turn.wait())
        .await
        .unwrap()
        .unwrap();
    client.assert_observed(
        &[
            "CROSS_MOB_HELPER:private",
            "CROSS_MOB_HELPER:reader-edit-denied",
            "CROSS_MOB_HELPER:sibling-create",
            "CROSS_MOB_HELPER:reader-admin-denied",
            "CROSS_MOB_HELPER:revoked-hold",
            "CROSS_MOB_FORK:reader-edit-denied",
            "CROSS_MOB_FORK:sibling-create",
            "CROSS_MOB_FORK:reader-admin-denied",
            "CROSS_MOB_FORK:revoked-hold",
        ],
        &[OWNER, HELPER, FORK],
    );
    {
        let evidence = scenario.evidence.lock().await;
        assert_eq!(evidence.sibling_documents.len(), 2);
        for actor in ["helper", "fork"] {
            let document = &evidence.sibling_documents[actor];
            assert_eq!(
                document.owner,
                Owner::Agent(evidence.actors[actor].1.clone())
            );
            assert_eq!(document.content.payload, content().payload);
        }
    }
    // Delegate always retires its helper and cascades that retirement to
    // fork_off descendants. The fixture must not override that lifetime.
    assert!(
        child
            .get_member(&AgentIdentity::from("cross-mob-helper"))
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        child
            .get_member(&AgentIdentity::from("cross-mob-fork"))
            .await
            .unwrap()
            .is_none()
    );
    let sessions = state.session_service();
    assert!(
        sessions
            .load_persisted_session_metadata(&helper_session)
            .await
            .unwrap()
            .is_none()
    );
    let retained = sessions
        .load_retained_session_metadata(&helper_session)
        .await
        .unwrap()
        .unwrap()
        .session_metadata
        .unwrap();
    assert_eq!(
        retained.mob_member_binding.as_ref(),
        Some(&helper_creation.member_binding)
    );
    // Stop through the native child handle before releasing the old host.
    state.mob_stop(&child_mob).await.unwrap();
    runtime.shutdown().await;
    drop(sessions);
    drop(child);
    drop(state);
    drop(context);
    drop(runtime);

    let (reopened, context, _) = harness_with_client(
        dir.path(),
        None,
        Arc::new(CrossMobClient::after_restart()),
        Some(scenario.clone()),
    )
    .await;
    let state = reopened.mob_runtime().agent_mob_mcp_state().unwrap();
    // 0.9 composes agent-created child mobs durably under a persistent root,
    // so the delegate's child mob is restored under its owner bridge session:
    // neither absent nor held. Its retired helper and fork stay retired.
    let restored = state.handle_for(&child_mob).await.unwrap();
    assert_eq!(restored.mob_id(), &child_mob);
    assert_eq!(
        restored
            .owner_bridge_session_lifecycle_authority()
            .map(|authority| (
                authority.bridge_session_id,
                authority.destroy_on_owner_archive,
                authority.implicit_delegation_mob
            )),
        Some((owner_session.clone(), true, true))
    );
    assert_eq!(
        restored.status_observation_snapshot(),
        meerkat_mob::MobState::Stopped
    );
    assert!(state.held_child_mobs().await.is_empty());
    for retired in ["cross-mob-helper", "cross-mob-fork"] {
        assert!(
            restored
                .get_member(&AgentIdentity::from(retired))
                .await
                .unwrap()
                .is_none()
        );
    }
    drop(restored);
    let retained_after = state
        .session_service()
        .load_retained_session_metadata(&helper_session)
        .await
        .unwrap()
        .unwrap()
        .session_metadata
        .unwrap();
    assert_eq!(
        retained_after.mob_member_binding,
        retained.mob_member_binding
    );
    assert_eq!(
        retained_after.tooling.tool_access_policy,
        retained.tooling.tool_access_policy
    );
    let owner_turn = reopened
        .start_member_turn(
            "cross-mob-owner",
            ContentInput::Text(REOPEN_OWNER.into()),
            HandlingMode::Queue,
            meerkat_mob::MemberTurnOptions::new(),
            None,
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_mins(1), owner_turn.turn.wait())
        .await
        .unwrap()
        .unwrap();
    expect_events(&mut events, &["owner:owner-persisted"]).await;
    drop(context);
    reopened.shutdown().await;
}

async fn run_root_fork_turn(handle: &MobHandle, task: &str) {
    run_member_turn(handle, "root-persistent-fork", task).await;
}

async fn run_member_turn(handle: &MobHandle, identity: &str, task: &str) {
    let member = handle.member(&AgentIdentity::from(identity)).await.unwrap();
    let turn = member
        .start_turn(
            ContentInput::Text(task.into()),
            HandlingMode::Queue,
            meerkat_mob::MemberTurnOptions::new(),
            None,
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_mins(1), turn.wait())
        .await
        .unwrap()
        .unwrap();
}

/// A fork of the persistent root member remains seated after its work ends.
/// This is separate from delegate: the durable owner is not retired here.
#[tokio::test(flavor = "multi_thread")]
async fn extension_native_root_fork_off_document_access_survives_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (scenario, mut events) = Scenario::new();
    let (runtime, context, _) = harness_with_client(
        dir.path(),
        None,
        Arc::new(CrossMobClient::default()),
        Some(scenario.clone()),
    )
    .await;
    let root = runtime.spawn(worker("persistent-owner")).await.unwrap();
    let owner_session = member_session(&runtime, &root.agent_identity).await;
    let owner_turn = runtime
        .start_member_turn(
            "persistent-owner",
            ContentInput::Text(ROOT_OWNER.into()),
            HandlingMode::Queue,
            meerkat_mob::MemberTurnOptions::new(),
            None,
        )
        .await
        .unwrap();
    expect_events(&mut events, &["owner:create", "fork:editor-initial"]).await;
    tokio::time::timeout(Duration::from_mins(1), owner_turn.turn.wait())
        .await
        .unwrap()
        .unwrap();
    // A queued tracked turn is a native completion barrier for the initial
    // fork job, whose public fork_off result can return before it finishes.
    run_root_fork_turn(&runtime.mob_handle(), ROOT_BARRIER).await;
    let (fork_session, fork_principal) = scenario.evidence.lock().await.actors["fork"].clone();
    let owner_principal = scenario.evidence.lock().await.actors["owner"].1.clone();
    assert_ne!(fork_principal, owner_principal);
    let original = runtime
        .mob_handle()
        .member_creation_for_session(&fork_session)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        original.creation.provenance,
        MemberCreationProvenance::Fork { .. }
    ));
    assert_eq!(
        original.fork_source.as_ref().unwrap().source_session_id,
        owner_session
    );
    assert_eq!(
        original.member_binding.mob_id,
        runtime.mob_handle().mob_id().as_str()
    );

    let private = || DocumentAccess {
        owner_reach: Reach::SelfOnly,
        ..Default::default()
    };
    scenario
        .set_access(&context, &owner_session, "root-fork-revoke", private())
        .await;
    run_root_fork_turn(&runtime.mob_handle(), ROOT_REVOKED).await;
    expect_events(&mut events, &["fork:root-revoked"]).await;
    runtime.shutdown().await;
    drop(context);
    drop(runtime);

    let (reopened, context, _) = harness_with_client(
        dir.path(),
        None,
        Arc::new(CrossMobClient::after_restart()),
        Some(scenario.clone()),
    )
    .await;
    let restored = reopened
        .mob_handle()
        .member_creation_for_session(&fork_session)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(restored, original);
    assert_eq!(
        member_session(&reopened, &AgentIdentity::from("root-persistent-fork")).await,
        fork_session
    );
    // The revoked ACL survives the new host and freshly built dispatcher.
    run_root_fork_turn(&reopened.mob_handle(), ROOT_REOPEN_DENIED).await;
    expect_events(&mut events, &["fork:root-reopen-denied"]).await;
    scenario
        .set_access(
            &context,
            &owner_session,
            "root-fork-restore-reach",
            DocumentAccess::default(),
        )
        .await;
    // Restoring default Forks reach grants data editing, never ACL control.
    run_root_fork_turn(&reopened.mob_handle(), ROOT_REOPEN_EDITOR).await;
    expect_events(&mut events, &["fork:editor-reopen"]).await;
    scenario
        .set_access(
            &context,
            &owner_session,
            "root-fork-revoke-again",
            private(),
        )
        .await;
    run_root_fork_turn(&reopened.mob_handle(), ROOT_REOPEN_REVOKED).await;
    expect_events(&mut events, &["fork:root-reopen-revoked"]).await;
    reopened.shutdown().await;
}

/// Source launch policy reaches a real fork_off child. Both native dispatch
/// and an executing read probe's backend write must refuse mutation.
#[tokio::test(flavor = "multi_thread")]
async fn extension_native_restricted_source_fork_off_denies_apply_and_backend_write() {
    let dir = tempfile::tempdir().unwrap();
    let (scenario, mut events) = Scenario::new();
    let client = Arc::new(CrossMobClient::default());
    let (runtime, context, _) =
        harness_with_client(dir.path(), None, client.clone(), Some(scenario.clone())).await;
    let owner = runtime.spawn(worker("policy-owner")).await.unwrap();
    let owner_session = member_session(&runtime, &owner.agent_identity).await;
    run_member_turn(&runtime.mob_handle(), "policy-owner", POLICY_OWNER).await;
    expect_events(&mut events, &["owner:create"]).await;

    let mut source = worker("restricted-source");
    source.tool_access_policy = Some(meerkat_core::ops::ToolAccessPolicy::DenyList(
        ["documents_apply"].into_iter().collect(),
    ));
    let source = runtime.spawn(source).await.unwrap();
    let source_session = member_session(&runtime, &source.agent_identity).await;
    let source_caller = caller(&context, &source_session).await;
    // The owner explicitly grants Editor to the source and its real forks.
    // The source's policy still caps their access below that document grant.
    scenario
        .set_access(
            &context,
            &owner_session,
            "restricted-source-editor-grant",
            DocumentAccess {
                grants: vec![Grant {
                    audience: Audience::Agent {
                        principal: source_caller.principal().clone(),
                        reach: Reach::Forks,
                    },
                    role: Role::Editor,
                }],
                ..Default::default()
            },
        )
        .await;
    let before = scenario.evidence.lock().await.document.clone().unwrap();
    run_member_turn(&runtime.mob_handle(), "restricted-source", POLICY_SOURCE).await;
    run_member_turn(
        &runtime.mob_handle(),
        "policy-inherited-fork",
        POLICY_BARRIER,
    )
    .await;
    expect_events(
        &mut events,
        &[
            "source:reader-policy-source",
            "policy-fork:reader-policy-fork",
        ],
    )
    .await;
    client.assert_observed(
        &[
            "RESTRICTED_POLICY_SOURCE:policy-forbidden",
            "RESTRICTED_POLICY_SOURCE:reader-policy-source",
            "RESTRICTED_POLICY_SOURCE:fork_off",
            "RESTRICTED_POLICY_FORK:policy-forbidden",
            "RESTRICTED_POLICY_FORK:reader-policy-fork",
        ],
        &[POLICY_OWNER, POLICY_SOURCE, POLICY_FORK, POLICY_BARRIER],
    );
    let evidence = scenario.evidence.lock().await;
    assert_eq!(
        evidence.backend_denials,
        ["source".to_owned(), "policy-fork".to_owned()]
            .into_iter()
            .collect()
    );
    let fork_session = evidence.actors["policy-fork"].0.clone();
    drop(evidence);
    let fork_creation = runtime
        .mob_handle()
        .member_creation_for_session(&fork_session)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        fork_creation
            .fork_source
            .as_ref()
            .unwrap()
            .source_session_id,
        source_session
    );
    assert!(matches!(
        fork_creation.creation.provenance,
        MemberCreationProvenance::Fork { .. }
    ));
    let owner_caller = caller(&context, &owner_session).await;
    let after = context
        .documents
        .get(&owner_caller, &before.id)
        .await
        .unwrap();
    assert_eq!(after.revision, before.revision);
    assert_eq!(after.content.payload, before.content.payload);
    runtime.shutdown().await;
}
