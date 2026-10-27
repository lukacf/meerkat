//! Actual runtime-origin dispatch, canonical lineage and durable document tests.
#![cfg(feature = "extension-state")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use meerkat_client::{LlmClient, LlmDoneOutcome, LlmError, LlmEvent, LlmRequest};
use meerkat_core::types::{
    ContentInput, HandlingMode, StopReason, ToolCallView, ToolDef, ToolResult,
};
use meerkat_core::{
    AgentToolDispatcher, ToolDispatchContext, ToolDispatchOutcome, ToolError, ToolMutationClass,
};
use meerkat_meta::extension_state::documents::*;
use meerkat_meta::extension_state::{ToolBundleContext, ToolBundleRequirements};
use meerkat_meta::{MobBootstrapOptions, MobBootstrapSpec, UnifiedRuntime};
use meerkat_mob::{AgentIdentity, MobDefinition, SpawnMemberSpec};
use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

const NAME: &str = "durable-documents-test";

#[path = "support/extension_cross_mob.rs"]
mod cross_mob;

struct Probe {
    context: ToolBundleContext,
    calls: Arc<tokio::sync::Mutex<Vec<(meerkat_core::SessionId, Principal)>>>,
    cross_mob: Option<Arc<cross_mob::Scenario>>,
}
#[async_trait::async_trait]
impl AgentToolDispatcher for Probe {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        ["documents_read", "documents_apply"]
            .into_iter()
            .map(|name| {
                Arc::new(
                    ToolDef::new(
                        name,
                        "Probe extension authority",
                        serde_json::json!({"type":"object","properties":{}}),
                    )
                    .with_provenance(meerkat_core::ToolProvenance::new(
                        meerkat_core::ToolSourceKind::RustBundle,
                        NAME,
                    )),
                )
            })
            .collect::<Vec<_>>()
            .into()
    }
    fn tool_mutation_class(&self, name: &str) -> ToolMutationClass {
        if name == "documents_read" {
            ToolMutationClass::ReadOnly
        } else {
            ToolMutationClass::Mutating
        }
    }
    async fn dispatch(
        &self,
        _: ToolCallView<'_>,
    ) -> std::result::Result<ToolDispatchOutcome, ToolError> {
        panic!("native dispatch must carry runner identity")
    }
    async fn dispatch_with_context(
        &self,
        call: ToolCallView<'_>,
        context: &ToolDispatchContext,
    ) -> std::result::Result<ToolDispatchOutcome, ToolError> {
        // Deliberate identity/lineage-looking tool arguments have no authority.
        let caller = self
            .context
            .caller_resolver
            .resolve(context, None)
            .await
            .unwrap();
        if let Some(scenario) = &self.cross_mob {
            return scenario
                .dispatch(&self.context, &caller, call, context)
                .await;
        }
        if call.name == "documents_apply" {
            self.context
                .documents
                .mutate(
                    &caller,
                    &request("first-native-create"),
                    Mutation::Create(NewDocument {
                        content: content(),
                        owner: None,
                        access: DocumentAccess::default(),
                    }),
                )
                .await
                .unwrap();
        }
        self.context
            .documents
            .list(&caller, ListRequest::default())
            .await
            .unwrap();
        self.calls.lock().await.push((
            context.origin_session_id().unwrap().clone(),
            caller.principal().clone(),
        ));
        Ok(ToolResult::new(call.id.to_owned(), "{}".into(), false).into())
    }
}

struct ScriptClient {
    calls: AtomicUsize,
    tool: &'static str,
}
#[async_trait::async_trait]
impl LlmClient for ScriptClient {
    fn project_replay_messages(
        &self,
        messages: &[meerkat_core::Message],
    ) -> std::result::Result<Vec<meerkat_core::Message>, LlmError> {
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
        let first = self.calls.fetch_add(1, Ordering::SeqCst) == 0;
        let mut events = Vec::new();
        if first {
            events.push(Ok(LlmEvent::ToolCallComplete {
                id: "native-origin".into(),
                name: self.tool.into(),
                args: serde_json::json!({"principal":"forged-admin","lineage":["forged-root"]}),
                meta: None,
            }));
        } else {
            events.push(Ok(LlmEvent::TextDelta {
                delta: "done".into(),
                meta: None,
            }));
        }
        events.push(Ok(LlmEvent::UsageUpdate {
            usage: meerkat_core::TurnUsage::host_declared(
                self.provider(),
                &request.model,
                meerkat_core::Usage::default(),
            ),
        }));
        events.push(Ok(LlmEvent::Done {
            outcome: LlmDoneOutcome::success(if first {
                StopReason::ToolUse
            } else {
                StopReason::EndTurn
            }),
        }));
        Box::pin(futures::stream::iter(events))
    }
}

fn definition(path: &std::path::Path) -> MobDefinition {
    MobDefinition::from_toml(
        r#"
[mob]
id = "{MOB_ID}"
[profiles.worker]
model = "gpt-5.5"
runtime_mode = "turn_driven"
external_addressable = true
[profiles.worker.tools]
comms = true
rust_bundles = ["durable-documents-test"]
"#
        .replace(
            "{MOB_ID}",
            &format!(
                "extension-{}",
                uuid::Uuid::new_v5(
                    &uuid::Uuid::NAMESPACE_OID,
                    path.as_os_str().as_encoded_bytes()
                )
            ),
        )
        .as_str(),
    )
    .unwrap()
}

async fn harness(
    path: &std::path::Path,
    access: Option<meerkat_meta::access::AccessController>,
) -> (
    UnifiedRuntime,
    ToolBundleContext,
    Arc<tokio::sync::Mutex<Vec<(meerkat_core::SessionId, Principal)>>>,
) {
    harness_with_client(
        path,
        access,
        Arc::new(ScriptClient {
            calls: AtomicUsize::new(0),
            tool: "documents_read",
        }),
        None,
    )
    .await
}

async fn harness_with_client(
    path: &std::path::Path,
    access: Option<meerkat_meta::access::AccessController>,
    client: Arc<dyn LlmClient>,
    cross_mob: Option<Arc<cross_mob::Scenario>>,
) -> (
    UnifiedRuntime,
    ToolBundleContext,
    Arc<tokio::sync::Mutex<Vec<(meerkat_core::SessionId, Principal)>>>,
) {
    let (storage, provenance) =
        meerkat_meta::mob_composition_manifest::persistent_mob_storage(path.join("mob.sqlite3"))
            .unwrap();
    let mut definition = definition(path);
    if cross_mob.is_some() {
        let meerkat_mob::ProfileBinding::Inline(profile) = definition
            .profiles
            .get_mut(&meerkat_mob::ProfileName::from("worker"))
            .unwrap()
        else {
            panic!("fixture declares an inline worker profile");
        };
        profile.tools.mob = true;
    }
    let spec =
        MobBootstrapSpec::persistent(definition, storage, provenance, path.to_path_buf(), 16)
            .await
            .unwrap()
            // Force final child-policy installation so delegate/fork coverage
            // also proves extension bundles and authority survive replacement
            // of the preliminary agent mob-tool state.
            .with_child_application_tool_policy(
                meerkat_core::ApplicationToolPolicyBinding::Unmanaged,
            )
            .with_options(MobBootstrapOptions {
                allow_ephemeral_sessions: false,
                notify_orchestrator_on_resume: false,
                default_llm_client: Some(client),
            });
    let captured = Arc::new(Mutex::new(None));
    let calls = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let factory_capture = captured.clone();
    let factory_calls = calls.clone();
    let mut builder = UnifiedRuntime::builder()
        .mob_spec(spec)
        .module_config(meerkat_meta::MetaConfig {
            modules: vec![],
            discovery: meerkat_meta::DiscoverySpec {
                namespace: "extension-test".into(),
                modules: vec![],
            },
            pre_spawn: vec![],
        })
        .timeout(Duration::from_secs(10))
        .persistent_state(path)
        .register_tool_bundle_factory(
            NAME,
            ToolBundleRequirements::durable_documents(NAME, "documents_read", "documents_apply"),
            Arc::new(move |context: ToolBundleContext| {
                *factory_capture.lock().unwrap() = Some(context.clone());
                Ok(Arc::new(Probe {
                    context,
                    calls: factory_calls.clone(),
                    cross_mob: cross_mob.clone(),
                }) as Arc<dyn AgentToolDispatcher>)
            }),
        );
    if let Some(access) = access {
        builder = builder.access_controller(access);
    }
    let runtime = Box::pin(builder.build()).await.unwrap();
    let context = captured.lock().unwrap().take().unwrap();
    (runtime, context, calls)
}
fn worker(id: &str) -> SpawnMemberSpec {
    SpawnMemberSpec::host_root("worker", AgentIdentity::from(id))
}

/// Shuts `runtime` down before a reopen over the same state in this process.
/// The shutdown stands the durable child root down within its terminal
/// window; a loaded host can outrun that window, and the root then stays
/// claimed until a later stand-down releases it, so a reopen here would be
/// refused with `AlreadyClaimed`. Stand down again, bounded, until the root
/// is released; a stand-down that never releases fails here with its outcome.
async fn shut_down_for_reopen(runtime: &UnifiedRuntime) {
    let mut outcome = runtime.shutdown().await.child_mob_stand_down;
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !child_root_released(&outcome) && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
        outcome = runtime.mob_runtime().stand_down_child_mobs(deadline).await;
    }
    assert!(
        child_root_released(&outcome),
        "the durable child root is released before the reopen: {outcome:?}"
    );
}

fn child_root_released(outcome: &meerkat_meta::ChildMobStandDownOutcome) -> bool {
    matches!(
        outcome,
        meerkat_meta::ChildMobStandDownOutcome::Released { .. }
            | meerkat_meta::ChildMobStandDownOutcome::NotConfigured
    )
}

async fn member_session(
    runtime: &UnifiedRuntime,
    identity: &AgentIdentity,
) -> meerkat_core::SessionId {
    runtime
        .mob_handle()
        .get_member(identity)
        .await
        .unwrap()
        .unwrap()
        .bridge_session_id()
        .unwrap()
        .clone()
}
async fn caller(
    context: &ToolBundleContext,
    session: &meerkat_core::SessionId,
) -> HostAccessContext {
    caller_at(context, session, "native caller").await
}
async fn caller_at(
    context: &ToolBundleContext,
    session: &meerkat_core::SessionId,
    stage: &str,
) -> HostAccessContext {
    context
        .caller_resolver
        .resolve(
            &ToolDispatchContext::default().with_runtime_identity(session.clone(), None),
            None,
        )
        .await
        .unwrap_or_else(|error| {
            panic!("{stage}: caller resolution failed for {session}: {error:?}")
        })
}
fn request(id: &str) -> RequestIdentity {
    RequestIdentity::new(id, id.as_bytes()).unwrap()
}
fn content() -> DocumentContent {
    DocumentContent {
        title: "private".into(),
        content_type: "application/test".into(),
        schema_version: 1,
        payload: vec![1, 2, 3],
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn extension_native_dispatch_private_fork_spawn_revocation_and_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, context, calls) = harness(dir.path(), None).await;
    let root = runtime.spawn(worker("root")).await.unwrap();
    let root_session = member_session(&runtime, &root.agent_identity).await;
    let admission = runtime
        .start_member_turn(
            "root",
            ContentInput::Text("probe".into()),
            HandlingMode::Queue,
            meerkat_mob::MemberTurnOptions::new(),
            None,
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(30), admission.turn.wait())
        .await
        .unwrap()
        .unwrap();
    let owner = caller(&context, &root_session).await;
    assert_eq!(
        calls.lock().await.as_slice(),
        &[(root_session.clone(), owner.principal().clone())]
    );
    assert!(matches!(
        context
            .caller_resolver
            .resolve(&ToolDispatchContext::default(), None)
            .await,
        Err(Error::AuthorityUnavailable)
    ));
    let book = context
        .documents
        .mutate(
            &owner,
            &request("create"),
            Mutation::Create(NewDocument {
                content: content(),
                owner: None,
                access: DocumentAccess::default(),
            }),
        )
        .await
        .unwrap()
        .receipt;
    let stranger = runtime.spawn(worker("stranger")).await.unwrap();
    let stranger = caller(
        &context,
        &member_session(&runtime, &stranger.agent_identity).await,
    )
    .await;
    assert!(matches!(
        context.documents.get(&stranger, &book.document_id).await,
        Err(Error::NotFound)
    ));
    let fork = runtime
        .mob_handle()
        .fork_member(&AgentIdentity::from("root"), worker("fork"), None)
        .await
        .unwrap();
    let fork_session = fork.session_id;
    let fork_caller = caller(&context, &fork_session).await;
    assert!(
        context
            .documents
            .get(&fork_caller, &book.document_id)
            .await
            .is_ok()
    );
    let source = runtime
        .mob_handle()
        .capture_member_creation_source(&root_session)
        .await
        .unwrap();
    let child = runtime
        .mob_handle()
        .spawn_spec(worker("child").with_creation_source(source))
        .await
        .unwrap();
    let child_session = member_session(&runtime, &child.agent_identity).await;
    let child_caller = caller(&context, &child_session).await;
    assert!(matches!(
        context
            .documents
            .get(&child_caller, &book.document_id)
            .await,
        Err(Error::NotFound)
    ));
    let shared = context
        .documents
        .mutate(
            &owner,
            &request("share"),
            Mutation::SetAccess {
                id: book.document_id.clone(),
                expected_revision: book.revision,
                access: DocumentAccess {
                    owner_reach: Reach::Descendants,
                    ..Default::default()
                },
            },
        )
        .await
        .unwrap()
        .receipt;
    assert!(
        context
            .documents
            .get(&caller(&context, &child_session).await, &book.document_id)
            .await
            .is_ok()
    );
    let revoked = context
        .documents
        .mutate(
            &owner,
            &request("revoke"),
            Mutation::SetAccess {
                id: book.document_id.clone(),
                expected_revision: shared.revision,
                access: DocumentAccess {
                    owner_reach: Reach::SelfOnly,
                    ..Default::default()
                },
            },
        )
        .await
        .unwrap()
        .receipt;
    assert!(matches!(
        context
            .documents
            .get(&caller(&context, &fork_session).await, &book.document_id)
            .await,
        Err(Error::NotFound)
    ));
    assert!(matches!(
        context
            .documents
            .get(&caller(&context, &child_session).await, &book.document_id)
            .await,
        Err(Error::NotFound)
    ));
    let shared = context
        .documents
        .mutate(
            &owner,
            &request("restore-forks"),
            Mutation::SetAccess {
                id: book.document_id.clone(),
                expected_revision: revoked.revision,
                access: DocumentAccess::default(),
            },
        )
        .await
        .unwrap()
        .receipt;
    runtime
        .mob_handle()
        .retire(AgentIdentity::from("root"))
        .await
        .unwrap();
    assert!(
        context
            .documents
            .get(&caller(&context, &fork_session).await, &book.document_id)
            .await
            .is_ok()
    );
    shut_down_for_reopen(&runtime).await;
    drop(runtime);
    drop(context);
    let (reopened, context, _) = harness(dir.path(), None).await;
    let fork_caller = caller(&context, &fork_session).await;
    assert_eq!(
        context
            .documents
            .get(&fork_caller, &book.document_id)
            .await
            .unwrap()
            .revision,
        shared.revision
    );
    let replacement = reopened.spawn(worker("root")).await.unwrap();
    let replacement = caller(
        &context,
        &member_session(&reopened, &replacement.agent_identity).await,
    )
    .await;
    assert_ne!(owner.principal(), replacement.principal());
    assert!(matches!(
        context.documents.get(&replacement, &book.document_id).await,
        Err(Error::NotFound)
    ));
    reopened.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn extension_native_current_role_and_roster_labels_bound_host_abac() {
    use meerkat_meta::access::{AccessControlConfig, AccessController, AccessEffect, AccessRule};
    let allow = AccessRule {
        id: "allow".into(),
        actions: vec!["extension.*".into()],
        ..Default::default()
    };
    let deny = AccessRule {
        id: "deny".into(),
        effect: AccessEffect::Deny,
        actions: vec!["extension.read".into()],
        roles: vec!["worker".into()],
        match_labels: BTreeMap::from([("restricted".into(), "true".into())]),
        ..Default::default()
    };
    let access = AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec!["operator".into()],
        rules: vec![allow, deny],
        ..Default::default()
    })
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let (runtime, context, _) = harness(dir.path(), Some(access.clone())).await;
    let mut spec = worker("restricted");
    spec.labels = Some(BTreeMap::from([("restricted".into(), "true".into())]));
    let member = runtime.spawn(spec).await.unwrap();
    let session = member_session(&runtime, &member.agent_identity).await;
    let denied = caller(&context, &session).await;
    assert!(matches!(
        context
            .documents
            .mutate(
                &denied,
                &request("create"),
                Mutation::Create(NewDocument {
                    content: content(),
                    owner: None,
                    access: DocumentAccess::default()
                })
            )
            .await,
        Err(Error::NotFound)
    ));
    access.delete_rule("deny").unwrap();
    let permitted = caller(&context, &session).await;
    assert!(
        context
            .documents
            .mutate(
                &permitted,
                &request("create"),
                Mutation::Create(NewDocument {
                    content: content(),
                    owner: None,
                    access: DocumentAccess::default()
                })
            )
            .await
            .is_ok()
    );
    runtime.shutdown().await;
}

#[derive(serde::Serialize, serde::Deserialize)]
struct RestartEvidence {
    root: meerkat_core::SessionId,
    fork: meerkat_core::SessionId,
    document: DocumentId,
    revision: Revision,
}

/// Three OS processes share only the configured disk provider. In particular,
/// no resolver, dispatcher, grant or lineage cache survives a boundary.
#[test]
fn extension_native_fresh_process_restart_and_current_revocation() {
    let dir = tempfile::tempdir().unwrap();
    for phase in ["create", "revoke", "verify"] {
        subprocess_phase(dir.path(), phase);
    }
}

/// Upgrade: a member Meta 0.9.3 spawned carries the identity's label but not
/// Core's durable-identity marker, so its session resolves no principal for
/// extension documents. The first boot whose roster lists the identity
/// respawns it onto the same session at a higher Core generation, with the
/// marker, and from then on that session resolves as the identity's agent.
/// A marked identity of the same roster keeps its generation: the boot
/// respawns only unmarked members.
#[test]
fn extension_upgraded_unmarked_member_resolves_as_its_agent_after_the_first_boot() {
    let dir = tempfile::tempdir().unwrap();
    for phase in ["upgrade-seed", "upgrade-boot"] {
        subprocess_phase(dir.path(), phase);
    }
}

#[test]
fn extension_native_late_birth_resolves_from_core_member_history() {
    let dir = tempfile::tempdir().unwrap();
    for phase in ["late-create", "late-reopen"] {
        subprocess_phase(dir.path(), phase);
    }
}

fn subprocess_phase(path: &std::path::Path, phase: &str) {
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "extension_subprocess_fixture",
            "--ignored",
            "--nocapture",
        ])
        .env("META_EXTENSION_RESTART_PATH", path)
        .env("META_EXTENSION_RESTART_PHASE", phase)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(90);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!(
                "restart phase {phase} timed out:\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "restart phase {phase}:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "launched by extension_native_fresh_process_restart_and_current_revocation"]
async fn extension_subprocess_fixture() {
    let path = std::path::PathBuf::from(std::env::var_os("META_EXTENSION_RESTART_PATH").unwrap());
    let phase = std::env::var("META_EXTENSION_RESTART_PHASE").unwrap();
    let evidence_path = path.join("restart-evidence.json");
    if phase.starts_with("late-") {
        late_birth_phase(&path, &phase).await;
        return;
    }
    if phase.starts_with("upgrade-") {
        upgrade_respawn_phase(&path, &phase).await;
        return;
    }
    let (runtime, context, _) = harness(&path, None).await;
    if phase == "create" {
        let root = runtime.spawn(worker("root")).await.unwrap();
        let root = member_session(&runtime, &root.agent_identity).await;
        let owner = caller(&context, &root).await;
        let created = context
            .documents
            .mutate(
                &owner,
                &request("process-create"),
                Mutation::Create(NewDocument {
                    content: content(),
                    owner: None,
                    access: DocumentAccess::default(),
                }),
            )
            .await
            .unwrap()
            .receipt;
        let fork = runtime
            .mob_handle()
            .fork_member(&AgentIdentity::from("root"), worker("fork"), None)
            .await
            .unwrap()
            .session_id;
        let editor = caller(&context, &fork).await;
        assert_eq!(
            context
                .documents
                .get(&editor, &created.document_id)
                .await
                .unwrap()
                .revision,
            created.revision
        );
        let edited = context
            .documents
            .mutate(
                &editor,
                &request("process-edit"),
                Mutation::Replace {
                    id: created.document_id.clone(),
                    expected_revision: created.revision,
                    content: content(),
                },
            )
            .await
            .unwrap()
            .receipt;
        std::fs::write(
            &evidence_path,
            serde_json::to_vec(&RestartEvidence {
                root,
                fork,
                document: edited.document_id,
                revision: edited.revision,
            })
            .unwrap(),
        )
        .unwrap();
    } else {
        let evidence: RestartEvidence =
            serde_json::from_slice(&std::fs::read(&evidence_path).unwrap()).unwrap();
        let editor = caller(&context, &evidence.fork).await;
        if phase == "revoke" {
            assert_eq!(
                context
                    .documents
                    .get(&editor, &evidence.document)
                    .await
                    .unwrap()
                    .revision,
                evidence.revision
            );
            assert!(
                context
                    .documents
                    .lookup_receipt(&editor, &request("process-edit"))
                    .await
                    .unwrap()
                    .is_some()
            );
            let owner = caller(&context, &evidence.root).await;
            context
                .documents
                .mutate(
                    &owner,
                    &request("process-revoke"),
                    Mutation::SetAccess {
                        id: evidence.document.clone(),
                        expected_revision: evidence.revision,
                        access: DocumentAccess {
                            owner_reach: Reach::SelfOnly,
                            ..Default::default()
                        },
                    },
                )
                .await
                .unwrap();
        } else {
            assert_eq!(phase, "verify");
            assert!(matches!(
                context.documents.get(&editor, &evidence.document).await,
                Err(Error::NotFound)
            ));
            assert!(matches!(
                context
                    .documents
                    .lookup_receipt(&editor, &request("process-edit"))
                    .await,
                Err(Error::NotFound)
            ));
        }
    }
    runtime.shutdown().await;
}

struct StableRoster;
#[async_trait::async_trait]
impl meerkat_meta::identity_first::contracts::RosterProvider for StableRoster {
    async fn roster(
        &self,
        _: &meerkat_meta::identity_first::RosterContext,
    ) -> std::result::Result<
        Vec<meerkat_meta::identity_first::DurableAgentSpec>,
        meerkat_meta::identity_first::RosterError,
    > {
        use meerkat_meta::identity_first::{AgentAddressability, DurableAgentSpec};
        Ok(vec![DurableAgentSpec {
            identity: meerkat_meta::identity_first::AgentIdentity::parse("durable:alice").unwrap(),
            profile: meerkat_mob::ProfileName::from("worker"),
            addressability: AgentAddressability::Addressable,
            display_name: None,
            labels: BTreeMap::new(),
            context: None,
            additional_instructions: vec![],
            initial_message: Some(ContentInput::Text("create a document".into())),
            runtime_mode_override: None,
            backend: None,
            binding: None,
            placement: None,
        }])
    }
}
/// [`StableRoster`] and one more identity, `durable:upgraded`: the roster
/// of the upgrade boot.
struct UpgradeRoster;
#[async_trait::async_trait]
impl meerkat_meta::identity_first::contracts::RosterProvider for UpgradeRoster {
    async fn roster(
        &self,
        context: &meerkat_meta::identity_first::RosterContext,
    ) -> std::result::Result<
        Vec<meerkat_meta::identity_first::DurableAgentSpec>,
        meerkat_meta::identity_first::RosterError,
    > {
        let mut roster =
            meerkat_meta::identity_first::contracts::RosterProvider::roster(&StableRoster, context)
                .await?;
        roster.push(worker_identity_spec("durable:upgraded"));
        Ok(roster)
    }
}

/// A roster entry for `identity` under the `worker` profile.
fn worker_identity_spec(identity: &str) -> meerkat_meta::identity_first::DurableAgentSpec {
    meerkat_meta::identity_first::DurableAgentSpec {
        identity: meerkat_meta::identity_first::AgentIdentity::parse(identity).unwrap(),
        profile: meerkat_mob::ProfileName::from("worker"),
        addressability: meerkat_meta::identity_first::AgentAddressability::Addressable,
        display_name: None,
        labels: BTreeMap::new(),
        context: None,
        additional_instructions: vec![],
        initial_message: None,
        runtime_mode_override: None,
        backend: None,
        binding: None,
        placement: None,
    }
}

async fn stable_harness(
    path: &std::path::Path,
    access: meerkat_meta::access::AccessController,
) -> (
    UnifiedRuntime,
    ToolBundleContext,
    Arc<tokio::sync::Mutex<Vec<(meerkat_core::SessionId, Principal)>>>,
) {
    stable_harness_with_roster(path, access, Arc::new(StableRoster)).await
}

async fn stable_harness_with_roster(
    path: &std::path::Path,
    access: meerkat_meta::access::AccessController,
    roster: Arc<dyn meerkat_meta::identity_first::contracts::RosterProvider>,
) -> (
    UnifiedRuntime,
    ToolBundleContext,
    Arc<tokio::sync::Mutex<Vec<(meerkat_core::SessionId, Principal)>>>,
) {
    let (storage, provenance) =
        meerkat_meta::mob_composition_manifest::persistent_mob_storage(path.join("mob.sqlite3"))
            .unwrap();
    // Session/continuity persistence alone does not retain the native mob
    // journal. These tests require the original creation facts after reopen.
    let spec = MobBootstrapSpec::persistent(
        definition(path),
        storage,
        provenance,
        path.to_path_buf(),
        16,
    )
    .await
    .unwrap()
    .with_options(MobBootstrapOptions {
        allow_ephemeral_sessions: false,
        notify_orchestrator_on_resume: false,
        default_llm_client: Some(Arc::new(ScriptClient {
            calls: AtomicUsize::new(0),
            tool: "documents_apply",
        })),
    });
    let captured = Arc::new(Mutex::new(None));
    let calls = Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let capture = captured.clone();
    let probe_calls = calls.clone();
    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .mob_spec(spec)
            .module_config(meerkat_meta::MetaConfig {
                modules: vec![],
                discovery: meerkat_meta::DiscoverySpec {
                    namespace: "extension-stable".into(),
                    modules: vec![],
                },
                pre_spawn: vec![],
            })
            .timeout(Duration::from_secs(10))
            .persistent_state(path)
            .continuity_from_state_dir(path)
            .await
            .unwrap()
            .roster_provider(roster)
            .identity_bootstrap_mode(
                meerkat_meta::identity_first::IdentityBootstrapMode::EagerMaterialize,
            )
            .identity_runtime_instance_id("extension-stable")
            .comms(true)
            .access_controller(access)
            .register_tool_bundle_factory(
                NAME,
                ToolBundleRequirements::durable_documents(
                    NAME,
                    "documents_read",
                    "documents_apply",
                ),
                Arc::new(move |context: ToolBundleContext| {
                    *capture.lock().unwrap() = Some(context.clone());
                    Ok(Arc::new(Probe {
                        context,
                        calls: probe_calls.clone(),
                        cross_mob: None,
                    }) as Arc<dyn AgentToolDispatcher>)
                }),
            )
            .build(),
    )
    .await
    .unwrap();
    let context = captured.lock().unwrap().take().unwrap();
    (runtime, context, calls)
}

#[tokio::test(flavor = "multi_thread")]
async fn extension_native_first_turn_creation_has_stable_owner_after_reset_and_restore() {
    use meerkat_meta::access::{AccessControlConfig, AccessController, AccessEffect, AccessRule};
    let access = AccessController::new(AccessControlConfig {
        enabled: true,
        admins: vec!["operator".into()],
        rules: vec![AccessRule {
            id: "allow".into(),
            actions: vec!["extension.*".into()],
            ..Default::default()
        }],
        ..Default::default()
    })
    .unwrap();
    let path = tempfile::tempdir().unwrap();
    let (runtime, context, calls) = stable_harness(path.path(), access.clone()).await;
    tokio::time::timeout(Duration::from_secs(30), async {
        while calls.lock().await.is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("initial extension call should wait for publication without blocking spawn reply");
    let (session, principal) = calls.lock().await[0].clone();
    assert_eq!(principal, Principal::Agent("durable:alice".into()));
    let creation = runtime
        .mob_handle()
        .member_creation_for_session(&session)
        .await
        .unwrap()
        .unwrap();
    assert!(creation.creation.creation_id.is_some());
    assert!(matches!(
        creation.creation.provenance,
        meerkat_mob::MemberCreationProvenance::Root
    ));
    let owner = caller_at(&context, &session, "stable owner before reset").await;
    let receipt = context
        .documents
        .lookup_receipt(&owner, &request("first-native-create"))
        .await
        .unwrap()
        .unwrap();
    let book = context
        .documents
        .get(&owner, &receipt.document_id)
        .await
        .unwrap();
    assert_eq!(book.owner, Owner::Agent(principal.clone()));
    access
        .upsert_rule(AccessRule {
            id: "stable-selector".into(),
            effect: AccessEffect::Deny,
            actions: vec!["extension.read".into()],
            agents: vec!["durable:alice".into()],
            ..Default::default()
        })
        .unwrap();
    assert!(matches!(
        context
            .documents
            .get(&caller(&context, &session).await, &receipt.document_id)
            .await,
        Err(Error::NotFound)
    ));
    access.delete_rule("stable-selector").unwrap();
    let identity = meerkat_meta::identity_first::AgentIdentity::parse("durable:alice").unwrap();
    let successor = runtime
        .identity_runtime()
        .unwrap()
        .reset(&identity)
        .await
        .unwrap();
    let successor_session = successor
        .session_id()
        .cloned()
        .expect("the reset successor binds a session");
    let successor_creation = runtime
        .mob_handle()
        .member_creation_for_session(&successor_session)
        .await
        .unwrap()
        .expect("reset successor has a native creation record");
    assert_eq!(
        successor_creation.creation.creation_id,
        creation.creation.creation_id
    );
    assert!(matches!(
        &successor_creation.creation.provenance,
        meerkat_mob::MemberCreationProvenance::Successor {
            predecessor_session_id,
            ..
        } if *predecessor_session_id == session
    ));
    let after_reset = caller_at(&context, &successor_session, "stable owner after reset").await;
    assert_eq!(after_reset.principal(), &principal);
    assert!(
        context
            .documents
            .get(&after_reset, &receipt.document_id)
            .await
            .is_ok()
    );
    shut_down_for_reopen(&runtime).await;
    drop(runtime);
    drop(context);
    let (runtime, context, _) = stable_harness(path.path(), access).await;
    let session = runtime
        .identity_runtime()
        .unwrap()
        .status(&identity)
        .await
        .unwrap()
        .session_id
        .unwrap();
    assert_eq!(
        session, successor_session,
        "restore keeps the reset session"
    );
    let restored_creation = runtime
        .mob_handle()
        .member_creation_for_session(&session)
        .await
        .unwrap()
        .expect("restored successor retains the native creation journal");
    assert_eq!(restored_creation.creation, successor_creation.creation);
    let restored = caller_at(&context, &session, "stable owner after reopen").await;
    assert_eq!(restored.principal(), &principal);
    assert!(
        context
            .documents
            .get(&restored, &receipt.document_id)
            .await
            .is_ok()
    );
    runtime.shutdown().await;
}

async fn late_birth_step<T>(
    phase: &str,
    step: &str,
    future: impl std::future::Future<Output = T>,
) -> T {
    eprintln!("{phase}: starting {step}");
    let result = tokio::time::timeout(Duration::from_secs(30), future)
        .await
        .unwrap_or_else(|_| panic!("{phase}: {step} exceeded 30 seconds"));
    eprintln!("{phase}: completed {step}");
    result
}

/// A member's principal comes from Core's member binding history alone: a
/// native spawn that carries Core's durable-identity marker resolves as the
/// identity it encodes although no materialization caller ever saw it (one
/// canceled after the spawn was submitted). A live member the identity-first
/// runtime labelled but Core never marked resolves no principal, in this
/// process and in a fresh one, and an unrelated worker stays a worker. The
/// fresh process's boot roster compile retires the late identity (the
/// roster does not list it), so only the other two are resolved there.
async fn late_birth_phase(path: &std::path::Path, phase: &str) {
    let (runtime, context, _) = late_birth_step(
        phase,
        "stable host bootstrap",
        stable_harness(
            path,
            meerkat_meta::access::AccessController::new(Default::default()).unwrap(),
        ),
    )
    .await;
    let late_identity = "durable:late";
    let late = meerkat_meta::member_comms_id::mob_member_id(late_identity);
    let labelled = meerkat_meta::member_comms_id::mob_member_id("durable:labelled");
    let evidence = path.join("late-sessions.json");
    let (late_session, labelled_session) = if phase == "late-create" {
        // The spawn every identity-first materialization submits (Core's
        // marker and the identity-first label), committed here with no Meta
        // caller around it.
        let mut late_spec = SpawnMemberSpec::host_root("worker", late.clone()).with_labels(
            BTreeMap::from([("agent_identity".to_string(), late_identity.to_string())]),
        );
        late_spec.continuity_intent = meerkat_mob::SpawnContinuityIntent::DurableIdentity {
            continuity_key: late_identity.to_string(),
        };
        late_birth_step(
            phase,
            "commit native late birth",
            runtime.mob_handle().spawn_spec(late_spec),
        )
        .await
        .unwrap();
        // An identity-first label without Core's marker: what a member Meta
        // 0.9.3 spawned looks like until Meta respawns it.
        let labelled_spec = SpawnMemberSpec::host_root("worker", labelled.clone()).with_labels(
            BTreeMap::from([("agent_identity".to_string(), "durable:labelled".to_string())]),
        );
        late_birth_step(
            phase,
            "commit labelled member without the marker",
            runtime.mob_handle().spawn_spec(labelled_spec),
        )
        .await
        .unwrap();
        let sessions = (
            member_session(&runtime, &late).await,
            member_session(&runtime, &labelled).await,
        );
        std::fs::write(&evidence, serde_json::to_vec(&sessions).unwrap()).unwrap();
        sessions
    } else {
        serde_json::from_slice(&std::fs::read(&evidence).unwrap()).unwrap()
    };
    if phase == "late-create" {
        assert_eq!(
            caller_at(&context, &late_session, "resolve native late birth")
                .await
                .principal(),
            &Principal::Agent(late_identity.to_string())
        );
    }
    if phase == "late-create" {
        assert_eq!(
            late_birth_step(
                phase,
                "verify live labelled session",
                member_session(&runtime, &labelled),
            )
            .await,
            labelled_session
        );
    } else {
        // The fresh process's roster pass retires a live member that
        // carries the identity-first label for an identity the roster does
        // not list (StableRoster lists only alice), marker or not.
        assert!(
            late_birth_step(
                phase,
                "verify the roster pass retired the labelled member",
                runtime.mob_handle().get_member(&labelled),
            )
            .await
            .unwrap()
            .is_none(),
            "the roster pass retires the labelled member the roster does not list"
        );
    }
    // Live and labelled, or retired: either way its session resolves no
    // principal.
    eprintln!("{phase}: resolving the labelled member without the marker");
    assert!(matches!(
        tokio::time::timeout(
            Duration::from_secs(2),
            context.caller_resolver.resolve(
                &ToolDispatchContext::default().with_runtime_identity(labelled_session, None),
                None,
            )
        )
        .await
        .unwrap(),
        Err(Error::AuthorityUnavailable)
    ));
    // An unrelated worker target still gets a fresh immutable worker identity.
    let worker_member = late_birth_step(
        phase,
        "spawn unrelated worker",
        runtime.spawn(worker(if phase == "late-create" {
            "unreserved-before"
        } else {
            "unreserved-after"
        })),
    )
    .await
    .unwrap();
    assert!(matches!(
        late_birth_step(phase, "resolve unrelated worker", async {
            caller(
                &context,
                &member_session(&runtime, &worker_member.agent_identity).await,
            )
            .await
        })
        .await
        .principal(),
        Principal::Worker { .. }
    ));
    late_birth_step(phase, "shutdown stable host", runtime.shutdown()).await;
}

/// The upgrade phases of
/// [`extension_upgraded_unmarked_member_resolves_as_its_agent_after_the_first_boot`]:
/// `upgrade-seed` leaves a member as Meta 0.9.3 spawned it, beside alice's
/// marked one; `upgrade-boot` boots with its identity rostered.
async fn upgrade_respawn_phase(path: &std::path::Path, phase: &str) {
    let upgraded_identity = "durable:upgraded";
    let upgraded = meerkat_meta::member_comms_id::mob_member_id(upgraded_identity);
    let alice = meerkat_meta::member_comms_id::mob_member_id("durable:alice");
    let evidence = path.join("upgrade-seed.json");
    let access = meerkat_meta::access::AccessController::new(Default::default()).unwrap();
    let resolve = |context: &ToolBundleContext, session: &meerkat_core::SessionId| {
        let context = context.clone();
        let session = session.clone();
        async move {
            tokio::time::timeout(
                Duration::from_secs(2),
                context.caller_resolver.resolve(
                    &ToolDispatchContext::default().with_runtime_identity(session, None),
                    None,
                ),
            )
            .await
            .unwrap()
            .map(|caller| caller.principal().clone())
        }
    };
    if phase == "upgrade-seed" {
        let (runtime, context, _) = late_birth_step(
            phase,
            "host bootstrap without the identity rostered",
            stable_harness(path, access),
        )
        .await;
        // The identity's label without Core's marker: a member Meta 0.9.3
        // spawned, as Core restores it after the upgrade.
        let spec = SpawnMemberSpec::host_root("worker", upgraded.clone()).with_labels(
            BTreeMap::from([("agent_identity".to_string(), upgraded_identity.to_string())]),
        );
        late_birth_step(
            phase,
            "commit the 0.9.3-era member",
            runtime.mob_handle().spawn_spec(spec),
        )
        .await
        .unwrap();
        let session = member_session(&runtime, &upgraded).await;
        let history = runtime.mob_handle().member_binding_history().await.unwrap();
        let seeded = history
            .last_binding(&upgraded)
            .expect("Core binds the seeded member");
        assert!(
            !matches!(
                seeded.continuity_intent(),
                meerkat_mob::SpawnContinuityIntent::DurableIdentity { .. }
            ),
            "{phase}: the seeded member carries no durable-identity marker"
        );
        let alice_binding = history.last_binding(&alice).expect("Core binds alice");
        assert!(
            alice_binding.is_live()
                && matches!(
                    alice_binding.continuity_intent(),
                    meerkat_mob::SpawnContinuityIntent::DurableIdentity { .. }
                ),
            "{phase}: alice's member is live and marked"
        );
        std::fs::write(
            &evidence,
            serde_json::to_vec(&serde_json::json!({
                "session": session,
                "generation": seeded.generation().get(),
                "alice_generation": alice_binding.generation().get(),
            }))
            .unwrap(),
        )
        .unwrap();
        let principal = resolve(&context, &session).await;
        assert!(
            matches!(principal, Err(Error::AuthorityUnavailable)),
            "{phase}: before the upgrade boot the unmarked member resolves no principal: \
             {principal:?}"
        );
        late_birth_step(phase, "shutdown the seeded host", runtime.shutdown()).await;
        return;
    }
    let seed: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&evidence).unwrap()).unwrap();
    let session: meerkat_core::SessionId = serde_json::from_value(seed["session"].clone()).unwrap();
    let seeded_generation = seed["generation"].as_u64().unwrap();
    let alice_generation = seed["alice_generation"].as_u64().unwrap();
    let (runtime, context, _) = late_birth_step(
        phase,
        "upgrade boot with the identity rostered",
        stable_harness_with_roster(path, access, Arc::new(UpgradeRoster)),
    )
    .await;
    // The boot's roster compile ran before the build returned.
    let history = runtime.mob_handle().member_binding_history().await.unwrap();
    let respawned = history
        .last_binding(&upgraded)
        .expect("Core binds the upgraded member");
    assert!(
        respawned.is_live() && respawned.generation().get() > seeded_generation,
        "{phase}: the boot respawns the unmarked member live under a new Core generation \
         ({seeded_generation} -> {:?})",
        respawned.generation()
    );
    assert_eq!(
        respawned.session_id(),
        Some(&session),
        "{phase}: the respawn keeps the session"
    );
    assert_eq!(
        respawned.continuity_intent(),
        &meerkat_mob::SpawnContinuityIntent::DurableIdentity {
            continuity_key: upgraded_identity.to_string(),
        },
        "{phase}: the respawned binding carries the identity's marker"
    );
    assert_eq!(
        history
            .last_binding(&alice)
            .expect("Core binds alice")
            .generation()
            .get(),
        alice_generation,
        "{phase}: the boot does not respawn a marked identity"
    );
    let principal = resolve(&context, &session).await;
    assert!(
        matches!(&principal, Ok(Principal::Agent(identity)) if identity == upgraded_identity),
        "{phase}: the upgraded member's session resolves as its agent after the first boot \
         respawns it: {principal:?}"
    );
    assert_eq!(
        member_session(&runtime, &upgraded).await,
        session,
        "{phase}: Core's live member runs the kept session"
    );
    late_birth_step(phase, "shutdown the upgraded host", runtime.shutdown()).await;
}

/// The mob databases, with their WAL and shared-memory sidecars, in the
/// durable child root of the runtime composed over `path`.
fn child_mob_storage_files(path: &std::path::Path) -> std::collections::BTreeSet<String> {
    let root = meerkat_mob_mcp::MobMcpState::persistent_forked_participant_store_path(path)
        .parent()
        .expect("the child root holds the custody database")
        .to_path_buf();
    let Ok(entries) = std::fs::read_dir(root) else {
        return Default::default();
    };
    entries
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| {
            name.starts_with("mob--")
                && [".db", ".db-wal", ".db-shm"]
                    .iter()
                    .any(|suffix| name.ends_with(suffix))
        })
        .collect()
}

/// A child mob of an owner member's bridge session, created through the
/// agent mob-tool state as an agent `mob_create` creates one. The child
/// supply gives its profile the host's document bundle, the extension
/// authority registry binds it before it activates, and its member's
/// document caller resolves through that registry.
struct OwnedChild {
    state: Arc<meerkat_mob_mcp::MobMcpState>,
    owner_session: meerkat_core::SessionId,
    child: meerkat_mob::MobId,
}

impl OwnedChild {
    async fn create(
        runtime: &UnifiedRuntime,
        context: &ToolBundleContext,
        path: &std::path::Path,
    ) -> Self {
        let owner = runtime.spawn(worker("owner")).await.unwrap();
        let owner_session = member_session(runtime, &owner.agent_identity).await;
        let state = runtime
            .mob_runtime()
            .agent_mob_mcp_state()
            .expect("the runtime composes the agent mob-tool state");
        let child = state
            .mob_create_definition_with_owner_bridge_session(
                MobDefinition::from_toml(&format!(
                    "[mob]\nid = \"{}-child\"\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\
                     runtime_mode = \"turn_driven\"\n[profiles.worker.tools]\ncomms = true\n",
                    definition(path).id
                ))
                .unwrap(),
                owner_session.clone(),
                true,
                false,
            )
            .await
            .unwrap();
        assert!(
            child_mob_storage_files(path).contains(&format!("mob--{child}.db")),
            "the child keeps a durable mob database"
        );
        let handle = state.handle_for(&child).await.unwrap();
        assert!(
            handle.definition().profiles[&meerkat_mob::ProfileName::from("worker")]
                .as_inline()
                .expect("an inline child profile")
                .tools
                .rust_bundles
                .iter()
                .any(|bundle| bundle == NAME),
            "the child supply gives the child the host's document bundle"
        );
        let member = AgentIdentity::from("child-worker");
        handle.spawn_spec(worker("child-worker")).await.unwrap();
        let member_session = handle
            .get_member(&member)
            .await
            .unwrap()
            .unwrap()
            .bridge_session_id()
            .unwrap()
            .clone();
        // The test keeps nothing of the child: only the runtime holds it.
        drop(handle);
        assert!(matches!(
            caller(context, &member_session).await.principal(),
            Principal::Worker { mob_id, .. } if mob_id == child.as_str()
        ));
        Self {
            state,
            owner_session,
            child,
        }
    }

    fn child_owner(&self) -> Owner {
        Owner::Mob(self.child.to_string())
    }

    /// Destroy the child through the mob-tool state, as an operator or the
    /// owner's mob tools do, and require its native destroy to complete.
    /// Whether the destroy's storage cleanup settled too is the subject of
    /// `extension_authority_registry_releases_destroyed_child_mob_storage`.
    async fn destroy_natively(&self) {
        match self.state.mob_destroy(&self.child).await {
            Ok(_) => {}
            Err(meerkat_mob_mcp::MobMcpDestroyError::Incomplete { report })
                if report
                    .errors
                    .iter()
                    .all(|error| error.starts_with("storage cleanup outstanding: ")) => {}
            Err(error) => panic!("the child's native destroy must complete: {error}"),
        }
        assert!(
            !self
                .state
                .mob_list()
                .await
                .unwrap()
                .iter()
                .any(|(id, _)| id == &self.child),
            "the destroyed child is no longer managed"
        );
    }
}

/// Destroying a child mob whose documents resolved through the extension
/// authority registry completes: the registry holds nothing of the destroyed
/// child, so the destroy's storage cleanup drains the child's database
/// observer and removes the database and its sidecars.
#[tokio::test(flavor = "multi_thread")]
async fn extension_authority_registry_releases_destroyed_child_mob_storage() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, context, _) = harness(dir.path(), None).await;
    let owned = OwnedChild::create(&runtime, &context, dir.path()).await;
    let report = owned
        .state
        .mob_destroy(&owned.child)
        .await
        .unwrap_or_else(|error| panic!("the child's destroy must complete: {error}"));
    assert!(report.errors.is_empty(), "{report:?}");
    let remaining = child_mob_storage_files(dir.path());
    assert!(
        !remaining
            .iter()
            .any(|name| name.starts_with(&format!("mob--{}.db", owned.child))),
        "the destroyed child's storage is removed: {remaining:?}"
    );
    drop(owned);
    runtime.shutdown().await;
}

/// A destroyed child mob leaves its owner's managed mobs: the owner, still
/// live, no longer manages the child as a document owner, nor the documents
/// the child owns.
#[tokio::test(flavor = "multi_thread")]
async fn extension_authority_registry_drops_destroyed_mob_from_owner_managed_mobs() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, context, _) = harness(dir.path(), None).await;
    let owned = OwnedChild::create(&runtime, &context, dir.path()).await;
    let owner = caller(&context, &owned.owner_session).await;
    assert!(
        can_create(&owner, &owned.child_owner()),
        "the owner manages its live child"
    );
    let receipt = context
        .documents
        .mutate(
            &owner,
            &request("child-owned"),
            Mutation::Create(NewDocument {
                content: content(),
                owner: Some(owned.child_owner()),
                access: DocumentAccess::default(),
            }),
        )
        .await
        .unwrap()
        .receipt;
    let document = context
        .documents
        .get(&owner, &receipt.document_id)
        .await
        .unwrap();
    assert_eq!(document.owner, owned.child_owner());
    assert!(authorize(&owner, &document, Operation::Manage));

    owned.destroy_natively().await;
    let former_owner = caller(&context, &owned.owner_session).await;
    assert!(
        !can_create(&former_owner, &owned.child_owner()),
        "the destroyed child is no longer among the owner's managed mobs"
    );
    assert!(!authorize(&former_owner, &document, Operation::Manage));
    assert!(matches!(
        context
            .documents
            .get(&former_owner, &receipt.document_id)
            .await,
        Err(Error::NotFound)
    ));
    drop(owned);
    runtime.shutdown().await;
}

/// A former owner that is still live cannot create a new document owned by
/// the child mob it destroyed: the destroyed mob is no valid owner to name,
/// and the owner's own authority does not create one for it either.
#[tokio::test(flavor = "multi_thread")]
async fn extension_authority_registry_refuses_new_documents_owned_by_a_destroyed_mob() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, context, _) = harness(dir.path(), None).await;
    let owned = OwnedChild::create(&runtime, &context, dir.path()).await;
    let dispatch =
        ToolDispatchContext::default().with_runtime_identity(owned.owner_session.clone(), None);
    assert!(
        context
            .caller_resolver
            .resolve(&dispatch, Some(&owned.child_owner()))
            .await
            .is_ok(),
        "a live child is a valid document owner"
    );

    owned.destroy_natively().await;
    assert!(matches!(
        context
            .caller_resolver
            .resolve(&dispatch, Some(&owned.child_owner()))
            .await,
        Err(Error::AuthorityUnavailable)
    ));
    let former_owner = caller(&context, &owned.owner_session).await;
    let created = context
        .documents
        .mutate(
            &former_owner,
            &request("after-destroy"),
            Mutation::Create(NewDocument {
                content: content(),
                owner: Some(owned.child_owner()),
                access: DocumentAccess::default(),
            }),
        )
        .await;
    assert!(
        created.is_err(),
        "no new document is committed for the destroyed child: {created:?}"
    );
    drop(owned);
    runtime.shutdown().await;
}

/// The document tools an embedding host serves over MCP. The host serves
/// the calls; this bundle carries their declaration and the caller context.
struct HostDocumentTools;
#[async_trait::async_trait]
impl AgentToolDispatcher for HostDocumentTools {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        ["host_documents_read", "host_documents_apply"]
            .into_iter()
            .map(|name| {
                Arc::new(ToolDef::new(
                    name,
                    "Host document service",
                    serde_json::json!({"type":"object","properties":{}}),
                ))
            })
            .collect::<Vec<_>>()
            .into()
    }
    fn tool_mutation_class(&self, name: &str) -> ToolMutationClass {
        match name {
            "host_documents_read" => ToolMutationClass::ReadOnly,
            "host_documents_apply" => ToolMutationClass::Mutating,
            _ => ToolMutationClass::Unknown,
        }
    }
    async fn dispatch(
        &self,
        _: ToolCallView<'_>,
    ) -> std::result::Result<ToolDispatchOutcome, ToolError> {
        panic!("the embedding host serves document calls, not this bundle")
    }
}

struct NoDocumentCalls;
#[async_trait::async_trait]
impl meerkat_mcp::McpCallContextProvider for NoDocumentCalls {
    async fn prepare(
        &self,
        _: meerkat_mcp::McpCallTarget<'_>,
        _: ToolCallView<'_>,
        _: &ToolDispatchContext,
    ) -> std::result::Result<Option<meerkat_mcp::McpCallContext>, meerkat_mcp::McpCallContextError>
    {
        panic!("no member makes a document call in this fixture")
    }
}

/// A runtime composed as an embedding host that supplies its document
/// service to child mobs over MCP only: a public stdio descriptor child
/// profiles carry, its host-only runtime twin, and the document authority
/// behind both. Returns the binding's document context and the public
/// descriptor.
async fn document_mcp_harness(
    path: &std::path::Path,
) -> (
    UnifiedRuntime,
    ToolBundleContext,
    meerkat_core::McpServerConfig,
) {
    // No document call reaches this command in these fixtures.
    let public = meerkat_core::McpServerConfig::stdio(
        "host-documents",
        "host-documents-fixture",
        vec![],
        std::collections::HashMap::from([("HOST_DOCUMENTS_TOKEN".to_string(), String::new())]),
    );
    let mut runtime_config = public.clone();
    let meerkat_core::mcp_config::McpTransportConfig::Stdio(stdio) = &mut runtime_config.transport
    else {
        panic!("the fixture descriptor is stdio");
    };
    stdio
        .env
        .insert("HOST_DOCUMENTS_TOKEN".into(), "host-secret".into());
    let captured = Arc::new(Mutex::new(None));
    let capture = captured.clone();
    let binding = meerkat_meta::extension_state::HostMcpDocumentBinding::new(
        public,
        runtime_config,
        ToolBundleRequirements::durable_documents(
            "host-documents",
            "host_documents_read",
            "host_documents_apply",
        ),
        Arc::new(
            move |context: ToolBundleContext| -> std::result::Result<
                Arc<dyn AgentToolDispatcher>,
                String,
            > {
                *capture.lock().unwrap() = Some(context);
                Ok(Arc::new(HostDocumentTools))
            },
        ),
        Arc::new(NoDocumentCalls),
        meerkat_meta::extension_state::ChildMcpAvailability::ChildAvailable,
    )
    .unwrap();
    let public = binding.public_config().clone();
    let (storage, provenance) =
        meerkat_meta::mob_composition_manifest::persistent_mob_storage(path.join("mob.sqlite3"))
            .unwrap();
    let mut definition = definition(path);
    let meerkat_mob::ProfileBinding::Inline(profile) = definition
        .profiles
        .get_mut(&meerkat_mob::ProfileName::from("worker"))
        .unwrap()
    else {
        panic!("fixture declares an inline worker profile");
    };
    // This host offers its documents over MCP only.
    profile.tools.rust_bundles.clear();
    // A document MCP binding composes the builder's own session services.
    let runtime = Box::pin(
        UnifiedRuntime::builder()
            .definition(definition)
            .persistent_state(path)
            .mob_storage(storage, provenance)
            .default_llm_client(Arc::new(ScriptClient {
                calls: AtomicUsize::new(0),
                tool: "host_documents_read",
            }))
            .child_application_tool_policy(meerkat_core::ApplicationToolPolicyBinding::Unmanaged)
            .module_config(meerkat_meta::MetaConfig {
                modules: vec![],
                discovery: meerkat_meta::DiscoverySpec {
                    namespace: "extension-mcp".into(),
                    modules: vec![],
                },
                pre_spawn: vec![],
            })
            .timeout(Duration::from_secs(10))
            .register_mcp_tool_bundle_factory(binding)
            .build(),
    )
    .await
    .unwrap();
    let context = captured.lock().unwrap().take().unwrap();
    (runtime, context, public)
}

/// The embedding-host flow: an owner member's bridge session gets a delegate
/// mob and a child it authored, both carrying the host's document MCP
/// supply, and their members' document callers resolve through the
/// extension authority registry. Owner cleanup then destroys both children
/// completely: nothing of them stays held, and their storage is removed.
#[tokio::test(flavor = "multi_thread")]
async fn extension_authority_registry_owner_cleanup_destroys_document_mcp_children() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, context, public) = document_mcp_harness(dir.path()).await;
    let owner = runtime.spawn(worker("owner")).await.unwrap();
    let owner_session = member_session(&runtime, &owner.agent_identity).await;
    let state = runtime
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the runtime composes the agent mob-tool state");
    let delegate = state
        .get_or_create_implicit_mob_for_bridge_session(&owner_session.to_string(), "gpt-5.5")
        .await
        .unwrap();
    let authored = state
        .mob_create_definition_with_owner_bridge_session(
            MobDefinition::from_toml(&format!(
                "[mob]\nid = \"{}-authored\"\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\
                 runtime_mode = \"turn_driven\"\n[profiles.worker.tools]\ncomms = true\n",
                definition(dir.path()).id
            ))
            .unwrap(),
            owner_session.clone(),
            true,
            false,
        )
        .await
        .unwrap();
    for (child, profile, member) in [
        (&delegate, "delegate", "helper"),
        (&authored, "worker", "author"),
    ] {
        let handle = state.handle_for(child).await.unwrap();
        assert_eq!(
            handle.definition().profiles[&meerkat_mob::ProfileName::from(profile)]
                .as_inline()
                .expect("an inline child profile")
                .tools
                .mcp_servers,
            vec![public.clone()],
            "the child supply gives {child} the host's public document descriptor"
        );
        drop(handle);
        state
            .mob_spawn(
                child,
                meerkat_mob::ProfileName::from(profile),
                AgentIdentity::from(member),
                Some(meerkat_mob::MobRuntimeMode::TurnDriven),
                None,
                None,
            )
            .await
            .unwrap();
        let handle = state.handle_for(child).await.unwrap();
        let session = handle
            .get_member(&AgentIdentity::from(member))
            .await
            .unwrap()
            .unwrap()
            .bridge_session_id()
            .unwrap()
            .clone();
        drop(handle);
        assert!(
            matches!(
                caller(&context, &session).await.principal(),
                Principal::Worker { mob_id, .. } if mob_id == child.as_str()
            ),
            "{child}'s member resolves its document caller"
        );
    }
    let databases = child_mob_storage_files(dir.path());
    for child in [&delegate, &authored] {
        assert!(
            databases.contains(&format!("mob--{child}.db")),
            "{child} keeps a durable mob database: {databases:?}"
        );
    }

    state
        .destroy_bridge_session_mobs(&owner_session.to_string())
        .await
        .unwrap_or_else(|error| panic!("owner cleanup must destroy both children: {error}"));
    let managed = state.mob_list().await.unwrap();
    let remaining = child_mob_storage_files(dir.path());
    for child in [&delegate, &authored] {
        assert!(
            !managed.iter().any(|(id, _)| id == child),
            "{child} is destroyed"
        );
        assert!(
            !remaining
                .iter()
                .any(|name| name.starts_with(&format!("mob--{child}.db"))),
            "{child}'s storage is removed: {remaining:?}"
        );
    }
    drop(state);
    runtime.shutdown().await;
}

/// A child mob restored with its runtime is released on destroy as a fresh
/// one is: the restored runtime binds the child's extension authority again,
/// and nothing it composes keeps the child's handle past the destroy, so the
/// destroy completes and removes the child's storage.
#[tokio::test(flavor = "multi_thread")]
async fn extension_authority_registry_releases_a_restored_child_mob_storage() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, context, _) = harness(dir.path(), None).await;
    let owned = OwnedChild::create(&runtime, &context, dir.path()).await;
    let child = owned.child.clone();
    drop(owned);
    shut_down_for_reopen(&runtime).await;
    drop(runtime);
    drop(context);

    let (reopened, context, _) = harness(dir.path(), None).await;
    let state = reopened
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the runtime composes the agent mob-tool state");
    let handle = state
        .handle_for(&child)
        .await
        .expect("the child is restored with its runtime");
    let member_session = handle
        .get_member(&AgentIdentity::from("child-worker"))
        .await
        .unwrap()
        .unwrap()
        .bridge_session_id()
        .unwrap()
        .clone();
    drop(handle);
    assert!(
        matches!(
            caller(&context, &member_session).await.principal(),
            Principal::Worker { mob_id, .. } if mob_id == child.as_str()
        ),
        "the restored child's member resolves its document caller again"
    );
    let report = state
        .mob_destroy(&child)
        .await
        .unwrap_or_else(|error| panic!("the restored child's destroy must complete: {error}"));
    assert!(report.errors.is_empty(), "{report:?}");
    let remaining = child_mob_storage_files(dir.path());
    assert!(
        !remaining
            .iter()
            .any(|name| name.starts_with(&format!("mob--{child}.db"))),
        "the restored child's storage is removed: {remaining:?}"
    );
    drop(state);
    reopened.shutdown().await;
}

/// A child mob definition under `mob_id`, as an agent's `mob_create` passes
/// one.
fn child_definition(mob_id: &meerkat_mob::MobId) -> MobDefinition {
    MobDefinition::from_toml(&format!(
        "[mob]\nid = \"{mob_id}\"\n[profiles.worker]\nmodel = \"gpt-5.5\"\n\
         runtime_mode = \"turn_driven\"\n[profiles.worker.tools]\ncomms = true\n"
    ))
    .unwrap()
}

/// A session-owned create of `mob_id` through the agent mob-tool state, as
/// an agent's `mob_create` runs it for its session.
async fn create_for_session(
    state: &meerkat_mob_mcp::MobMcpState,
    mob_id: &meerkat_mob::MobId,
    session: meerkat_core::SessionId,
) -> std::result::Result<meerkat_mob::MobId, meerkat_mob::MobError> {
    state
        .mob_create_definition_with_owner_bridge_session(
            child_definition(mob_id),
            session,
            true,
            false,
        )
        .await
}

/// The refusal of a create under an id documents still name: recognizable,
/// and naming every way to clear the references.
fn assert_refused_with_remedy(error: &meerkat_mob::MobError, mob_id: &meerkat_mob::MobId) {
    assert!(
        meerkat_mob_mcp::MobMcpState::is_create_refusal(error),
        "{error}"
    );
    let text = error.to_string();
    for expected in [
        mob_id.as_str(),
        "Choose another mob id",
        "transfer each document the id owns to another owner",
        "remove or replace each Audience::Mob grant or deny",
        "The document tools alone cannot manage",
    ] {
        assert!(text.contains(expected), "{expected:?} missing: {text}");
    }
    let data = error.structured_data().expect("a structured refusal");
    assert_eq!(data["kind"], "mob_create_refused");
    assert!(
        data["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("Audience::Mob grant or deny")),
        "{data}"
    );
}

/// Documents keep naming a destroyed child mob by its id. A new mob under
/// that id is refused while they do: from another owner session (the
/// takeover), from the child's own owner session alike (ownership is by id,
/// not by mob instance), and without any owner session, in the same process
/// and after the store is reopened.
#[tokio::test(flavor = "multi_thread")]
async fn extension_mob_id_reuse_is_refused_while_documents_name_it() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, context, _) = harness(dir.path(), None).await;
    let owned = OwnedChild::create(&runtime, &context, dir.path()).await;
    let owner = caller(&context, &owned.owner_session).await;
    context
        .documents
        .mutate(
            &owner,
            &request("child-owned"),
            Mutation::Create(NewDocument {
                content: content(),
                owner: Some(owned.child_owner()),
                access: DocumentAccess::default(),
            }),
        )
        .await
        .unwrap();
    owned.destroy_natively().await;

    let other = runtime.spawn(worker("other-owner")).await.unwrap();
    let other_session = member_session(&runtime, &other.agent_identity).await;
    let refused = create_for_session(&owned.state, &owned.child, other_session)
        .await
        .expect_err("another session cannot take the destroyed child's id over");
    assert_refused_with_remedy(&refused, &owned.child);
    let refused = create_for_session(&owned.state, &owned.child, owned.owner_session.clone())
        .await
        .expect_err("the child's own owner session cannot recreate the explicit id either");
    assert_refused_with_remedy(&refused, &owned.child);
    let child = owned.child.clone();
    drop(owned);
    shut_down_for_reopen(&runtime).await;
    drop(runtime);
    drop(context);

    let (reopened, _context, _) = harness(dir.path(), None).await;
    let state = reopened
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the runtime composes the agent mob-tool state");
    let late = reopened.spawn(worker("late-owner")).await.unwrap();
    let late_session = member_session(&reopened, &late.agent_identity).await;
    let refused = create_for_session(&state, &child, late_session)
        .await
        .expect_err("the documents still name the id after the store is reopened");
    assert_refused_with_remedy(&refused, &child);
    // A create without an owner session gets no exemption: the state's
    // public create, as an embedder's own surface reaches it, is checked
    // the same way.
    let refused = state
        .mob_create_definition(child_definition(&child))
        .await
        .expect_err("a create without an owner session is checked too");
    assert_refused_with_remedy(&refused, &child);
    drop(state);
    reopened.shutdown().await;
}

/// An audience entry alone keeps a destroyed mob's id named: a document
/// shared with the mob (Audience::Mob) blocks a new mob under the id after
/// its owner was transferred away from the mob, until the grant is removed.
#[tokio::test(flavor = "multi_thread")]
async fn extension_mob_id_reuse_is_refused_for_an_audience_entry_until_it_is_removed() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, context, _) = harness(dir.path(), None).await;
    let owned = OwnedChild::create(&runtime, &context, dir.path()).await;
    let owner = caller(&context, &owned.owner_session).await;
    let shared = DocumentAccess {
        grants: vec![Grant {
            audience: Audience::Mob(owned.child.to_string()),
            role: Role::Reader,
        }],
        ..DocumentAccess::default()
    };
    let created = context
        .documents
        .mutate(
            &owner,
            &request("shared-with-child"),
            Mutation::Create(NewDocument {
                content: content(),
                owner: Some(owned.child_owner()),
                access: shared,
            }),
        )
        .await
        .unwrap()
        .receipt;
    // While the child lives, its owner session moves the document to its
    // own principal; the grant to the child stays.
    let transferred = context
        .documents
        .mutate(
            &owner,
            &request("transfer-from-child"),
            Mutation::Transfer {
                id: created.document_id.clone(),
                expected_revision: created.revision.clone(),
                owner: Owner::Agent(owner.principal().clone()),
                access: None,
            },
        )
        .await
        .unwrap()
        .receipt;
    owned.destroy_natively().await;

    let other = runtime.spawn(worker("other-owner")).await.unwrap();
    let other_session = member_session(&runtime, &other.agent_identity).await;
    let refused = create_for_session(&owned.state, &owned.child, other_session.clone())
        .await
        .expect_err("the grant alone still names the id after the owner transfer");
    assert_refused_with_remedy(&refused, &owned.child);

    let owner = caller(&context, &owned.owner_session).await;
    context
        .documents
        .mutate(
            &owner,
            &request("unshare-child"),
            Mutation::SetAccess {
                id: created.document_id.clone(),
                expected_revision: transferred.revision.clone(),
                access: DocumentAccess::default(),
            },
        )
        .await
        .unwrap();
    create_for_session(&owned.state, &owned.child, other_session)
        .await
        .expect("with no document naming the id, the create proceeds");
    drop(owned);
    runtime.shutdown().await;
}

/// The implicit delegation mob that a session's own runtime path recreates,
/// as for a model change, keeps the documents its id owns, also after a
/// restart restored it with its owner session. A foreign create cannot
/// claim that id, neither as an explicit mob nor with the implicit flag for
/// another session, and a temporary council's model-chosen id is checked
/// like any other.
#[tokio::test(flavor = "multi_thread")]
async fn extension_mob_id_reuse_keeps_the_sessions_implicit_mob_and_checks_councils() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, context, _) = harness(dir.path(), None).await;
    let state = runtime
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the runtime composes the agent mob-tool state");
    let owner = runtime.spawn(worker("owner")).await.unwrap();
    let session = member_session(&runtime, &owner.agent_identity).await;
    let (implicit, created) = state
        .ensure_implicit_mob_for_model(&session.to_string(), "gpt-5.5", None)
        .await
        .unwrap();
    assert!(created);
    let owner_caller = caller(&context, &session).await;
    let named = DocumentAccess {
        grants: vec![Grant {
            audience: Audience::Mob(implicit.to_string()),
            role: Role::Reader,
        }],
        ..DocumentAccess::default()
    };
    context
        .documents
        .mutate(
            &owner_caller,
            &request("delegation-notes"),
            Mutation::Create(NewDocument {
                content: content(),
                owner: Some(Owner::Mob(implicit.to_string())),
                access: named,
            }),
        )
        .await
        .unwrap();
    let (recreated, created) = state
        .ensure_implicit_mob_for_model(&session.to_string(), "claude-sonnet-4-5", Some(&implicit))
        .await
        .expect("the session's own implicit mob is recreated for a new model");
    assert!(created);
    assert_eq!(recreated, implicit);
    drop(state);
    shut_down_for_reopen(&runtime).await;
    drop(runtime);
    drop(context);

    let (reopened, context, _) = harness(dir.path(), None).await;
    let state = reopened
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the runtime composes the agent mob-tool state");
    assert_eq!(
        state
            .find_implicit_mob_for_bridge_session(&session.to_string())
            .await,
        Some(implicit.clone()),
        "the restored implicit mob keeps its owner session"
    );
    let (recreated, created) = state
        .ensure_implicit_mob_for_model(&session.to_string(), "gpt-5.5", Some(&implicit))
        .await
        .expect("the restored session recreates its implicit mob again");
    assert!(created);
    assert_eq!(recreated, implicit);

    state
        .destroy_bridge_session_mobs(&session.to_string())
        .await
        .unwrap();
    let foreign = reopened.spawn(worker("foreign")).await.unwrap();
    let foreign_session = member_session(&reopened, &foreign.agent_identity).await;
    let refused = create_for_session(&state, &implicit, foreign_session.clone())
        .await
        .expect_err("another session cannot claim the implicit mob's id");
    assert_refused_with_remedy(&refused, &implicit);
    let refused = state
        .mob_create_definition_with_owner_bridge_session(
            child_definition(&implicit),
            foreign_session.clone(),
            true,
            true,
        )
        .await
        .expect_err("the implicit flag counts only for the id of its own session");
    assert_refused_with_remedy(&refused, &implicit);

    let council = meerkat_mob::temporary_council::TemporaryCouncilId::new("model-chosen")
        .unwrap()
        .temporary_mob_id();
    let foreign_caller = caller(&context, &foreign_session).await;
    context
        .documents
        .mutate(
            &foreign_caller,
            &request("shared-with-council"),
            Mutation::Create(NewDocument {
                content: content(),
                owner: None,
                access: DocumentAccess {
                    grants: vec![Grant {
                        audience: Audience::Mob(council.to_string()),
                        role: Role::Reader,
                    }],
                    ..DocumentAccess::default()
                },
            }),
        )
        .await
        .unwrap();
    // A temporary council creates its mob for a session of its own.
    let refused = state
        .mob_create_definition_with_owner_bridge_session(
            child_definition(&council),
            meerkat_core::SessionId::new(),
            false,
            false,
        )
        .await
        .expect_err("a council's model-chosen id is checked");
    assert_refused_with_remedy(&refused, &council);
    drop(state);
    reopened.shutdown().await;
}

/// The check fails closed: when the documents cannot be read, a
/// session-owned create is refused rather than let through.
#[tokio::test(flavor = "multi_thread")]
async fn extension_mob_id_reuse_refuses_when_the_documents_cannot_be_read() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, context, _) = harness(dir.path(), None).await;
    let owner = runtime.spawn(worker("owner")).await.unwrap();
    let session = member_session(&runtime, &owner.agent_identity).await;
    let owner_caller = caller(&context, &session).await;
    context
        .documents
        .mutate(
            &owner_caller,
            &request("unreadable"),
            Mutation::Create(NewDocument {
                content: content(),
                owner: None,
                access: DocumentAccess::default(),
            }),
        )
        .await
        .unwrap();
    rusqlite::Connection::open(dir.path().join("extension_state.sqlite3"))
        .unwrap()
        .execute("UPDATE extension_documents SET metadata = '{'", [])
        .unwrap();
    let state = runtime
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the runtime composes the agent mob-tool state");
    let mob_id = meerkat_mob::MobId::from("never-named");
    let refused = create_for_session(&state, &mob_id, session)
        .await
        .expect_err("an unreadable document store refuses the create");
    assert!(
        meerkat_mob_mcp::MobMcpState::is_create_refusal(&refused),
        "{refused}"
    );
    assert!(
        refused.to_string().contains("could not be checked"),
        "{refused}"
    );
    drop(state);
    runtime.shutdown().await;
}

/// Document operations are ordered against a create under an id. A caller
/// resolved while a child mob lived, whose write commits after the child was
/// destroyed and another session created a mob under its id, is refused, as
/// owner and as audience alike: the write was authorized for the destroyed
/// child, not for the new mob. Its reads are refused too, since its
/// authority still names the id. The steps run one after the other, forcing
/// that interleaving: resolve, destroy, create, commit. A caller resolved
/// after the change reads and writes as before.
#[tokio::test(flavor = "multi_thread")]
async fn extension_mob_id_reuse_refuses_a_write_resolved_before_the_id_changed() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, context, _) = harness(dir.path(), None).await;
    let owned = OwnedChild::create(&runtime, &context, dir.path()).await;
    let stale = caller(&context, &owned.owner_session).await;
    assert!(
        can_create(&stale, &owned.child_owner()),
        "resolved while the child lives, its owner session manages it"
    );
    let before = context
        .documents
        .mutate(
            &stale,
            &request("before-destroy"),
            Mutation::Create(NewDocument {
                content: content(),
                owner: None,
                access: DocumentAccess::default(),
            }),
        )
        .await
        .unwrap()
        .receipt;
    owned.destroy_natively().await;
    let other = runtime.spawn(worker("other-owner")).await.unwrap();
    let other_session = member_session(&runtime, &other.agent_identity).await;
    create_for_session(&owned.state, &owned.child, other_session)
        .await
        .expect("no document names the id yet");

    let as_owner = context
        .documents
        .mutate(
            &stale,
            &request("stale-owner"),
            Mutation::Create(NewDocument {
                content: content(),
                owner: Some(owned.child_owner()),
                access: DocumentAccess::default(),
            }),
        )
        .await;
    assert!(
        matches!(as_owner, Err(Error::AuthorityUnavailable)),
        "a write resolved for the destroyed child is refused as owner: {as_owner:?}"
    );
    let shared = || DocumentAccess {
        grants: vec![Grant {
            audience: Audience::Mob(owned.child.to_string()),
            role: Role::Editor,
        }],
        ..DocumentAccess::default()
    };
    let as_audience = context
        .documents
        .mutate(
            &stale,
            &request("stale-audience"),
            Mutation::Create(NewDocument {
                content: content(),
                owner: None,
                access: shared(),
            }),
        )
        .await;
    assert!(
        matches!(as_audience, Err(Error::AuthorityUnavailable)),
        "a write resolved for the destroyed child is refused as audience: {as_audience:?}"
    );
    for refused in [
        context
            .documents
            .get(&stale, &before.document_id)
            .await
            .map(drop),
        context
            .documents
            .list(&stale, ListRequest::default())
            .await
            .map(drop),
        context
            .documents
            .lookup_receipt(&stale, &request("before-destroy"))
            .await
            .map(drop),
    ] {
        assert!(
            matches!(refused, Err(Error::AuthorityUnavailable)),
            "a read by a caller resolved before the id changed is refused: {refused:?}"
        );
    }
    let fresh = caller(&context, &owned.owner_session).await;
    context
        .documents
        .get(&fresh, &before.document_id)
        .await
        .expect("a caller resolved after the change reads as before");
    context
        .documents
        .mutate(
            &fresh,
            &request("fresh-audience"),
            Mutation::Create(NewDocument {
                content: content(),
                owner: None,
                access: shared(),
            }),
        )
        .await
        .expect("a caller resolved after the change writes as before");
    drop(owned);
    runtime.shutdown().await;
}

/// The document bundle of a runtime composed without the extension: the
/// same tool declarations, no document authority behind them, so no
/// extension hook is installed on the agent mob-tool state.
struct UncomposedDocuments;
#[async_trait::async_trait]
impl AgentToolDispatcher for UncomposedDocuments {
    fn tools(&self) -> Arc<[Arc<ToolDef>]> {
        ["documents_read", "documents_apply"]
            .into_iter()
            .map(|name| {
                Arc::new(
                    ToolDef::new(
                        name,
                        "Document tools without the extension",
                        serde_json::json!({"type":"object","properties":{}}),
                    )
                    .with_provenance(meerkat_core::ToolProvenance::new(
                        meerkat_core::ToolSourceKind::RustBundle,
                        NAME,
                    )),
                )
            })
            .collect::<Vec<_>>()
            .into()
    }
    fn tool_mutation_class(&self, name: &str) -> ToolMutationClass {
        if name == "documents_read" {
            ToolMutationClass::ReadOnly
        } else {
            ToolMutationClass::Mutating
        }
    }
    async fn dispatch(
        &self,
        _: ToolCallView<'_>,
    ) -> std::result::Result<ToolDispatchOutcome, ToolError> {
        panic!("no member calls a document tool while the extension is not composed")
    }
}

/// The runtime over `path` composed as [`harness`] composes it, but with the
/// extension switched off: the document bundle registered as a plain tool
/// bundle, no tool bundle factory and no document binding, so the agent
/// mob-tool state creates mobs without the extension's create check.
async fn harness_without_extension(path: &std::path::Path) -> UnifiedRuntime {
    let (storage, provenance) =
        meerkat_meta::mob_composition_manifest::persistent_mob_storage(path.join("mob.sqlite3"))
            .unwrap();
    let spec = MobBootstrapSpec::persistent(
        definition(path),
        storage,
        provenance,
        path.to_path_buf(),
        16,
    )
    .await
    .unwrap()
    .with_child_application_tool_policy(meerkat_core::ApplicationToolPolicyBinding::Unmanaged)
    .with_options(MobBootstrapOptions {
        allow_ephemeral_sessions: false,
        notify_orchestrator_on_resume: false,
        default_llm_client: Some(Arc::new(ScriptClient {
            calls: AtomicUsize::new(0),
            tool: "documents_read",
        })),
    });
    Box::pin(
        UnifiedRuntime::builder()
            .mob_spec(spec)
            .module_config(meerkat_meta::MetaConfig {
                modules: vec![],
                discovery: meerkat_meta::DiscoverySpec {
                    namespace: "extension-test".into(),
                    modules: vec![],
                },
                pre_spawn: vec![],
            })
            .timeout(Duration::from_secs(10))
            .persistent_state(path)
            .register_tool_bundle(NAME, Arc::new(UncomposedDocuments))
            .build(),
    )
    .await
    .unwrap()
}

/// The holder a store records for `mob_id`, read from the document store
/// file directly, as an operator inspecting it would.
fn recorded_holder(path: &std::path::Path, mob_id: &str) -> Option<String> {
    rusqlite::Connection::open(path.join("extension_state.sqlite3"))
        .unwrap()
        .query_row(
            "SELECT holder FROM extension_mob_holders WHERE mob_id = ?1",
            [mob_id],
            |row| row.get(0),
        )
        .ok()
}

/// The recorded holder of an owner session, as the store spells it.
fn session_holder(session: &meerkat_core::SessionId) -> String {
    serde_json::to_string(&MobIdHolder::OwnerSession(session.to_string())).unwrap()
}

/// Remove every live document naming `mob_id`, as an operator with access to
/// the extension store does: through the provider's own store, with a
/// context that manages the id (a document whose owning mob was destroyed is
/// beyond the document tools).
async fn clear_references(
    path: &std::path::Path,
    realm: &str,
    namespace: &str,
    mob_id: &str,
    documents: &[DocumentId],
) {
    let operator = HostAccessContext::new(
        realm,
        Principal::Agent("operator".into()),
        VerifiedLineage::root(),
        HostPolicy::ALLOW,
    )
    .unwrap()
    .with_memberships(
        std::collections::BTreeSet::new(),
        std::collections::BTreeSet::from([mob_id.to_string()]),
        false,
    )
    .unwrap();
    let service = DocumentService::new(
        realm,
        namespace,
        Arc::new(SqliteExtensionDocumentStore::open(path.join("extension_state.sqlite3")).unwrap()),
    )
    .unwrap();
    for (index, id) in documents.iter().enumerate() {
        let current = service.get(&operator, id).await.unwrap();
        service
            .mutate(
                &operator,
                &request(&format!("operator-clear-{index}")),
                Mutation::Delete {
                    id: id.clone(),
                    expected_revision: current.revision,
                },
            )
            .await
            .unwrap();
    }
}

/// Restoring a persisted mob re-checks who holds its id. A mob that took a
/// destroyed child's id while the extension was not composed (so no create
/// check saw it) is restored, once the extension is composed again, but
/// quarantined while documents written for the child name the id: its owner
/// session manages none of them, its members resolve no document caller,
/// and the runtime reports the quarantine typed. The holder record outlives
/// the destroy that left the documents behind. Once an operator clears the
/// references, the next restore binds the mob normally.
#[tokio::test(flavor = "multi_thread")]
async fn extension_restored_mob_under_a_reused_id_is_quarantined() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, context, _) = harness(dir.path(), None).await;
    let realm = context.documents.scope().realm().to_owned();
    let namespace = context.documents.scope().namespace().to_owned();
    let owned = OwnedChild::create(&runtime, &context, dir.path()).await;
    let owner = caller(&context, &owned.owner_session).await;
    let written = context
        .documents
        .mutate(
            &owner,
            &request("child-owned"),
            Mutation::Create(NewDocument {
                content: content(),
                owner: Some(owned.child_owner()),
                access: DocumentAccess::default(),
            }),
        )
        .await
        .unwrap()
        .receipt;
    let report = owned
        .state
        .mob_destroy(&owned.child)
        .await
        .unwrap_or_else(|error| panic!("the child's destroy must complete: {error}"));
    assert!(report.errors.is_empty(), "{report:?}");
    assert_eq!(
        recorded_holder(dir.path(), owned.child.as_str()),
        Some(session_holder(&owned.owner_session)),
        "the holder record outlives the destroy while documents name the id"
    );
    let other = runtime.spawn(worker("other-owner")).await.unwrap();
    let other_session = member_session(&runtime, &other.agent_identity).await;
    let child = owned.child.clone();
    let first_owner = owned.owner_session.clone();
    drop(owned);
    shut_down_for_reopen(&runtime).await;
    drop(runtime);
    drop(context);

    let uncomposed = harness_without_extension(dir.path()).await;
    let state = uncomposed
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the runtime composes the agent mob-tool state");
    create_for_session(&state, &child, other_session.clone())
        .await
        .expect("without the extension no create check runs");
    drop(state);
    shut_down_for_reopen(&uncomposed).await;
    drop(uncomposed);

    let (reopened, context, _) = harness(dir.path(), None).await;
    let state = reopened
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the runtime composes the agent mob-tool state");
    let taken = state
        .handle_for(&child)
        .await
        .expect("the mob under the reused id is restored");
    let quarantines = reopened.extension_mob_quarantines();
    assert_eq!(quarantines.len(), 1, "{quarantines:?}");
    assert_eq!(quarantines[0].mob_id, child.as_str());
    assert_eq!(
        quarantines[0].reason,
        meerkat_meta::extension_state::MobIdQuarantineReason::HeldByAnotherMob {
            holder: MobIdHolder::OwnerSession(first_owner.to_string()),
        }
    );
    let taker = caller(&context, &other_session).await;
    assert!(
        !can_create(&taker, &Owner::Mob(child.to_string())),
        "the restored mob's owner session does not manage the reused id"
    );
    assert!(
        matches!(
            context.documents.get(&taker, &written.document_id).await,
            Err(Error::NotFound)
        ),
        "a document written for the destroyed child stays out of reach"
    );
    let as_owner = context
        .caller_resolver
        .resolve(
            &ToolDispatchContext::default().with_runtime_identity(other_session.clone(), None),
            Some(&Owner::Mob(child.to_string())),
        )
        .await;
    assert!(
        matches!(as_owner, Err(Error::AuthorityUnavailable)),
        "a quarantined mob is no valid owner: {as_owner:?}"
    );
    taken.spawn_spec(worker("taken-worker")).await.unwrap();
    let taken_member = taken
        .get_member(&AgentIdentity::from("taken-worker"))
        .await
        .unwrap()
        .unwrap()
        .bridge_session_id()
        .unwrap()
        .clone();
    drop(taken);
    let member = context
        .caller_resolver
        .resolve(
            &ToolDispatchContext::default().with_runtime_identity(taken_member, None),
            None,
        )
        .await;
    assert!(
        matches!(member, Err(Error::AuthorityUnavailable)),
        "a quarantined mob's member resolves no document caller: {member:?}"
    );
    drop(state);
    shut_down_for_reopen(&reopened).await;
    drop(reopened);
    drop(context);

    clear_references(
        dir.path(),
        &realm,
        &namespace,
        child.as_str(),
        &[written.document_id],
    )
    .await;
    let (cleared, context, _) = harness(dir.path(), None).await;
    assert!(
        cleared.extension_mob_quarantines().is_empty(),
        "with no document naming the id, the restored mob binds"
    );
    let taker = caller(&context, &other_session).await;
    assert!(
        can_create(&taker, &Owner::Mob(child.to_string())),
        "the owner session manages its restored mob"
    );
    assert_eq!(
        recorded_holder(dir.path(), child.as_str()),
        Some(session_holder(&other_session)),
        "the binding recorded the restored mob's holder"
    );
    let state = cleared
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the runtime composes the agent mob-tool state");
    let report = state
        .mob_destroy(&child)
        .await
        .unwrap_or_else(|error| panic!("the restored mob's destroy must complete: {error}"));
    assert!(report.errors.is_empty(), "{report:?}");
    drop(state);
    cleared.shutdown().await;
}

/// Copy a stopped installation's whole state directory, as a host that
/// keeps generations of it does.
fn copy_state(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let kind = entry.file_type().unwrap();
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_state(&entry.path(), &target);
        } else if kind.is_file() {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// Put a copied generation back in place of the installation's state.
fn restore_state(generation: &std::path::Path, state: &std::path::Path) {
    std::fs::remove_dir_all(state).unwrap();
    copy_state(generation, state);
}

/// Restoring a whole state generation keeps its mobs bound: the documents,
/// their holder records and the mob state travel together, so a mob restored
/// with its own generation binds with full authority, also after a later
/// generation re-claimed its id and was abandoned. The holder the store
/// records is the owner session of the first run, unchanged by the copies.
#[tokio::test(flavor = "multi_thread")]
async fn extension_whole_state_generation_restore_keeps_its_mobs_bound() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    let generation = dir.path().join("generation-a");
    std::fs::create_dir_all(&state).unwrap();
    let (runtime, context, _) = harness(&state, None).await;
    let owned = OwnedChild::create(&runtime, &context, &state).await;
    let owner = caller(&context, &owned.owner_session).await;
    let owned_document = context
        .documents
        .mutate(
            &owner,
            &request("generation-owned"),
            Mutation::Create(NewDocument {
                content: content(),
                owner: Some(owned.child_owner()),
                access: DocumentAccess::default(),
            }),
        )
        .await
        .unwrap()
        .receipt;
    let child = owned.child.clone();
    let child_owner = owned.owner_session.clone();
    drop(owned);
    shut_down_for_reopen(&runtime).await;
    drop(runtime);
    drop(context);
    copy_state(&state, &generation);
    assert_eq!(
        recorded_holder(&generation, child.as_str()),
        Some(session_holder(&child_owner)),
        "the copy carries the holder record"
    );

    // The generation restored into a fresh process.
    restore_state(&generation, &state);
    let (restored, context, _) = harness(&state, None).await;
    assert!(
        restored.extension_mob_quarantines().is_empty(),
        "{:?}",
        restored.extension_mob_quarantines()
    );
    let owner = caller(&context, &child_owner).await;
    assert!(can_create(&owner, &Owner::Mob(child.to_string())));
    let document = context
        .documents
        .get(&owner, &owned_document.document_id)
        .await
        .expect("the restored mob's owner reads what the mob owns");
    assert!(authorize(&owner, &document, Operation::Manage));

    // A later generation clears the id and re-claims it for another session.
    context
        .documents
        .mutate(
            &owner,
            &request("generation-b-delete"),
            Mutation::Delete {
                id: owned_document.document_id.clone(),
                expected_revision: document.revision.clone(),
            },
        )
        .await
        .unwrap();
    let mob_state = restored
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the runtime composes the agent mob-tool state");
    let report = mob_state
        .mob_destroy(&child)
        .await
        .unwrap_or_else(|error| panic!("the child's destroy must complete: {error}"));
    assert!(report.errors.is_empty(), "{report:?}");
    let other = restored.spawn(worker("generation-b-owner")).await.unwrap();
    let other_session = member_session(&restored, &other.agent_identity).await;
    create_for_session(&mob_state, &child, other_session.clone())
        .await
        .expect("no document names the id in this generation");
    let other_caller = caller(&context, &other_session).await;
    context
        .documents
        .mutate(
            &other_caller,
            &request("generation-b-owned"),
            Mutation::Create(NewDocument {
                content: content(),
                owner: Some(Owner::Mob(child.to_string())),
                access: DocumentAccess::default(),
            }),
        )
        .await
        .unwrap();
    assert_eq!(
        recorded_holder(&state, child.as_str()),
        Some(session_holder(&other_session))
    );
    drop(mob_state);
    shut_down_for_reopen(&restored).await;
    drop(restored);
    drop(context);

    // Generation B is abandoned: the installation rolls back to generation A.
    restore_state(&generation, &state);
    let (rolled_back, context, _) = harness(&state, None).await;
    assert!(
        rolled_back.extension_mob_quarantines().is_empty(),
        "{:?}",
        rolled_back.extension_mob_quarantines()
    );
    assert_eq!(
        recorded_holder(&state, child.as_str()),
        Some(session_holder(&child_owner)),
        "the rolled-back store records generation A's holder"
    );
    let owner = caller(&context, &child_owner).await;
    assert!(can_create(&owner, &Owner::Mob(child.to_string())));
    let document = context
        .documents
        .get(&owner, &owned_document.document_id)
        .await
        .expect("the rolled-back mob's owner reads what the mob owns");
    assert!(authorize(&owner, &document, Operation::Manage));
    rolled_back.shutdown().await;
}

/// A store written before holders were recorded (references from before
/// 0.9.3, or a mob created without the extension's hooks) has no holder for
/// a restored mob's id: the mob adopts the id and its documents, and the
/// holder is recorded once, so the next restore compares against it.
#[tokio::test(flavor = "multi_thread")]
async fn extension_restore_adopts_an_id_with_no_holder_record() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, context, _) = harness(dir.path(), None).await;
    let owned = OwnedChild::create(&runtime, &context, dir.path()).await;
    let owner = caller(&context, &owned.owner_session).await;
    let written = context
        .documents
        .mutate(
            &owner,
            &request("before-holders"),
            Mutation::Create(NewDocument {
                content: content(),
                owner: Some(owned.child_owner()),
                access: DocumentAccess::default(),
            }),
        )
        .await
        .unwrap()
        .receipt;
    let child = owned.child.clone();
    let child_owner = owned.owner_session.clone();
    drop(owned);
    shut_down_for_reopen(&runtime).await;
    drop(runtime);
    drop(context);
    rusqlite::Connection::open(dir.path().join("extension_state.sqlite3"))
        .unwrap()
        .execute("DELETE FROM extension_mob_holders", [])
        .unwrap();

    for restart in ["adopts", "compares"] {
        let (reopened, context, _) = harness(dir.path(), None).await;
        assert!(
            reopened.extension_mob_quarantines().is_empty(),
            "{restart}: {:?}",
            reopened.extension_mob_quarantines()
        );
        let owner = caller(&context, &child_owner).await;
        context
            .documents
            .get(&owner, &written.document_id)
            .await
            .unwrap_or_else(|error| panic!("{restart}: the owner keeps its documents: {error}"));
        assert_eq!(
            recorded_holder(dir.path(), child.as_str()),
            Some(session_holder(&child_owner)),
            "{restart}: the adoption recorded the holder"
        );
        reopened.shutdown().await;
    }
}

/// A holder record that does not decode is a quarantine, never a missing
/// holder, and the quarantine never aborts the restore: every other
/// persisted mob is restored and bound beside the quarantined one.
#[tokio::test(flavor = "multi_thread")]
async fn extension_an_unreadable_holder_quarantines_without_failing_the_restore() {
    let dir = tempfile::tempdir().unwrap();
    let (runtime, context, _) = harness(dir.path(), None).await;
    let owned = OwnedChild::create(&runtime, &context, dir.path()).await;
    let sibling = meerkat_mob::MobId::from(format!("{}-sibling", owned.child).as_str());
    create_for_session(&owned.state, &sibling, owned.owner_session.clone())
        .await
        .unwrap();
    let child = owned.child.clone();
    let child_owner = owned.owner_session.clone();
    drop(owned);
    shut_down_for_reopen(&runtime).await;
    drop(runtime);
    drop(context);
    rusqlite::Connection::open(dir.path().join("extension_state.sqlite3"))
        .unwrap()
        .execute(
            "UPDATE extension_mob_holders SET holder = '{' WHERE mob_id = ?1",
            [child.as_str()],
        )
        .unwrap();

    let (reopened, context, _) = harness(dir.path(), None).await;
    let state = reopened
        .mob_runtime()
        .agent_mob_mcp_state()
        .expect("the runtime composes the agent mob-tool state");
    for mob_id in [&child, &sibling] {
        drop(
            state
                .handle_for(mob_id)
                .await
                .unwrap_or_else(|error| panic!("{mob_id} is restored: {error}")),
        );
    }
    let quarantines = reopened.extension_mob_quarantines();
    assert_eq!(quarantines.len(), 1, "{quarantines:?}");
    assert_eq!(quarantines[0].mob_id, child.as_str());
    assert!(
        matches!(
            quarantines[0].reason,
            meerkat_meta::extension_state::MobIdQuarantineReason::Unverifiable { .. }
        ),
        "{quarantines:?}"
    );
    let owner = caller(&context, &child_owner).await;
    assert!(!can_create(&owner, &Owner::Mob(child.to_string())));
    assert!(
        can_create(&owner, &Owner::Mob(sibling.to_string())),
        "the sibling restored beside it binds with full authority"
    );
    drop(state);
    reopened.shutdown().await;
}
